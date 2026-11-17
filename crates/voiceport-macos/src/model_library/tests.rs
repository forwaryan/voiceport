use super::*;
use std::net::TcpListener;

#[test]
fn cancellation_interrupts_stalled_network_before_read_timeout() {
    runtime().block_on(async {
        let cancel = Arc::new(AtomicBool::new(false));
        let trigger = cancel.clone();
        let setter = tokio::spawn(async move {
            tokio::task::yield_now().await;
            trigger.store(true, Ordering::Release);
        });
        let pending = std::future::pending::<Result<(), reqwest::Error>>();
        let result = tokio::time::timeout(Duration::from_secs(2), network(pending, &cancel))
            .await
            .unwrap();
        assert!(result.unwrap_err().contains("取消"));
        setter.await.unwrap();
    });
}

#[test]
fn transfer_checks_http_size_hash_and_truncation() {
    for (body, length, expected_ok) in [
        ("abc", "3", true),
        ("xyz", "3", false),
        ("abcd", "4", false),
        ("ab", "3", false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0u8; 4096];
            assert!(stream.read(&mut request).unwrap() > 0);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
            )
            .unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let asset = Asset {
            name: "test",
            url: Box::leak(url.into_boxed_str()),
            bytes: 3,
            sha256: HASH,
        };
        let result = runtime().block_on(download(
            &reqwest::Client::new(),
            &asset,
            &temp.path().join("download"),
            &AtomicBool::new(false),
            &Mutex::new(Progress::default()),
            0,
            3,
        ));
        worker.join().unwrap();
        assert_eq!(result.is_ok(), expected_ok, "body={body}");
    }
}

const HASH: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
static TEST_MODEL: Definition = Definition {
    id: "test:whisper",
    directory: "whisper-tiny",
    name: "Fixture",
    languages: "en",
    description: "fixture",
    recommendation: "fixture",
    kind: ModelKind::Whisper,
    assets: &[
        Asset {
            name: "tiny-encoder.int8.onnx",
            url: "https://invalid.example/encoder",
            bytes: 3,
            sha256: HASH,
        },
        Asset {
            name: "tiny-decoder.int8.onnx",
            url: "https://invalid.example/decoder",
            bytes: 3,
            sha256: HASH,
        },
        Asset {
            name: "tiny-tokens.txt",
            url: "https://invalid.example/tokens",
            bytes: 3,
            sha256: HASH,
        },
    ],
};
#[test]
#[ignore = "explicit pinned model+engine network download; VOICEPORT_TEST_MODEL selects ID; synthetic silence only"]
fn official_download_installs_engine_and_model_and_decodes_silence() {
    let temp = tempfile::tempdir().unwrap();
    let retained = std::env::var_os("VOICEPORT_TEST_RETAIN_ROOT").map(PathBuf::from);
    let root = if let Some(root) = retained {
        assert!(root.is_absolute(), "retained root must be absolute");
        assert!(
            root.symlink_metadata()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "retained root must be a new directory; never reuse user installations"
        );
        println!(
            "Retaining downloaded runtime and model at {}",
            root.display()
        );
        root
    } else {
        temp.path().join("models")
    };
    let paths = Paths { root };
    let _lock = Lock::acquire(&paths.root).unwrap();
    let id = std::env::var("VOICEPORT_TEST_MODEL")
        .unwrap_or_else(|_| crate::local_models::WHISPER.into());
    let definition = MODELS.iter().find(|d| d.id == id).expect("known model ID");
    println!(
        "Downloading pinned {} ({} model bytes)",
        definition.id,
        definition.bytes()
    );
    let started = std::time::Instant::now();
    runtime()
        .block_on(install(
            &paths,
            definition,
            Operation::Download,
            &AtomicBool::new(false),
            &Mutex::new(Progress::default()),
        ))
        .unwrap();
    let model = paths.discover(definition).unwrap();
    assert_eq!(model.runtime, paths.runtime());
    crate::local_runtime::probe(&model, "", &voiceport_core::CancellationToken::new()).unwrap();
    assert_eq!(stages(&paths), 0);
    assert!(paths.runtime().join("LICENSE-sherpa-onnx.txt").is_file());
    assert!(
        paths
            .runtime()
            .join("ThirdPartyNotices-ONNX-Runtime.txt")
            .is_file()
    );
    if definition.kind == ModelKind::SenseVoice {
        assert_eq!(
            fs::read_to_string(paths.model(definition).join("MODEL_LICENSE.txt")).unwrap(),
            catalog::SENSEVOICE_LICENSE
        );
        assert_eq!(
            fs::read_to_string(paths.model(definition).join("ATTRIBUTION.txt")).unwrap(),
            catalog::SENSEVOICE_ATTRIBUTION
        );
    }
    if matches!(
        definition.kind,
        ModelKind::Whisper | ModelKind::WhisperTurbo
    ) {
        assert_eq!(
            fs::read_to_string(paths.model(definition).join("MODEL_LICENSE.txt")).unwrap(),
            catalog::WHISPER_LICENSE
        );
        assert_eq!(
            fs::read_to_string(paths.model(definition).join("ATTRIBUTION.txt")).unwrap(),
            catalog::WHISPER_ATTRIBUTION
        );
    }
    println!(
        "Official runtime + {} verified, installed and decoded synthetic silence in {:?}",
        definition.id,
        started.elapsed()
    );
}

fn fixture() -> (tempfile::TempDir, Paths, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let paths = Paths {
        root: temp.path().join("models"),
    };
    fs::create_dir_all(paths.runtime()).unwrap();
    fs::write(
        paths.runtime().join("package.json"),
        br#"{"version":"1.12.37"}"#,
    )
    .unwrap();
    for name in ["libsherpa-onnx-c-api.dylib", "libonnxruntime.1.23.2.dylib"] {
        fs::write(
            paths.runtime().join(name),
            [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1],
        )
        .unwrap();
    }
    let source = temp.path().join("import");
    fs::create_dir(&source).unwrap();
    for asset in TEST_MODEL.assets {
        fs::write(source.join(asset.name), b"abc").unwrap();
    }
    (temp, paths, source)
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn stages(paths: &Paths) -> usize {
    fs::read_dir(&paths.root)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(".install-"))
        .count()
}
#[test]
fn import_publishes_verified_copy_without_modifying_source() {
    let (_temp, paths, source) = fixture();
    let cancel = AtomicBool::new(false);
    let progress = Mutex::new(Progress::default());
    runtime()
        .block_on(install(
            &paths,
            &TEST_MODEL,
            Operation::Import(source.clone()),
            &cancel,
            &progress,
        ))
        .unwrap();
    assert_eq!(stages(&paths), 0);
    assert!(paths.discover(&TEST_MODEL).is_ok());
    for asset in TEST_MODEL.assets {
        assert_eq!(fs::read(source.join(asset.name)).unwrap(), b"abc");
        assert_eq!(
            fs::read(paths.model(&TEST_MODEL).join(asset.name)).unwrap(),
            b"abc"
        );
    }
    assert!(
        runtime()
            .block_on(install(
                &paths,
                &TEST_MODEL,
                Operation::Import(source.clone()),
                &cancel,
                &progress
            ))
            .is_err()
    );
    remove(&paths, &TEST_MODEL, &cancel).unwrap();
    assert!(source.is_dir());
    assert!(!paths.model(&TEST_MODEL).exists());
}
#[test]
fn corrupt_incomplete_cancelled_and_symlink_imports_never_publish() {
    for case in ["hash", "missing", "oversize", "cancel", "symlink"] {
        let (_temp, paths, source) = fixture();
        let file = source.join(TEST_MODEL.assets[1].name);
        match case {
            "hash" => fs::write(&file, b"xyz").unwrap(),
            "missing" => fs::remove_file(&file).unwrap(),
            "oversize" => fs::write(&file, b"abcd").unwrap(),
            "symlink" => {
                fs::remove_file(&file).unwrap();
                std::os::unix::fs::symlink(source.join(TEST_MODEL.assets[0].name), &file).unwrap();
            }
            _ => {}
        }
        let cancel = AtomicBool::new(case == "cancel");
        assert!(
            runtime()
                .block_on(install(
                    &paths,
                    &TEST_MODEL,
                    Operation::Import(source),
                    &cancel,
                    &Mutex::new(Progress::default())
                ))
                .is_err(),
            "{case}"
        );
        assert!(!paths.model(&TEST_MODEL).exists(), "{case}");
        assert_eq!(stages(&paths), 0, "{case}");
    }
}
#[test]
fn fifo_import_finishes_and_releases_job_lock_without_a_writer() {
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
    let (_temp, paths, source) = fixture();
    let fifo = source.join(TEST_MODEL.assets[0].name);
    fs::remove_file(&fifo).unwrap();
    let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let job = Job::start(paths.clone(), &TEST_MODEL, Operation::Import(source)).unwrap();
    let result = job.receiver.recv_timeout(Duration::from_secs(2));
    if result.is_err() {
        // Rescue the old blocking implementation so a failed regression leaves no worker.
        job.cancel();
        let _writer = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        let _ = job.receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    let error = result
        .expect("FIFO import must return without waiting for a writer")
        .unwrap_err();
    assert!(error.contains("官方模型文件"), "{error}");
    assert!(!paths.model(&TEST_MODEL).exists());
    assert_eq!(stages(&paths), 0);
    assert!(Lock::acquire(&paths.root).is_ok());
}

#[test]
fn every_catalog_model_import_requires_complete_matching_files() {
    for official in MODELS {
        let assets = Box::leak(
            official
                .assets
                .iter()
                .map(|asset| Asset {
                    name: asset.name,
                    url: asset.url,
                    bytes: 3,
                    sha256: HASH,
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let definition = Definition {
            id: official.id,
            directory: official.directory,
            name: official.name,
            languages: official.languages,
            description: official.description,
            recommendation: official.recommendation,
            kind: official.kind,
            assets,
        };
        for case in ["ok", "missing", "corrupt"] {
            let (_temp, paths, source) = fixture();
            for asset in definition.assets {
                fs::write(source.join(asset.name), b"abc").unwrap();
            }
            let file = source.join(definition.assets[0].name);
            match case {
                "missing" => fs::remove_file(file).unwrap(),
                "corrupt" => fs::write(file, b"xyz").unwrap(),
                _ => {}
            }
            let result = runtime().block_on(install(
                &paths,
                &definition,
                Operation::Import(source.clone()),
                &AtomicBool::new(false),
                &Mutex::new(Progress::default()),
            ));
            assert_eq!(
                result.is_ok(),
                case == "ok",
                "{} {case}: {result:?}",
                definition.id
            );
            assert_eq!(stages(&paths), 0);
            if case == "ok" {
                assert_eq!(paths.discover(&definition).unwrap().kind, official.kind);
                if official.kind == ModelKind::SenseVoice {
                    let license =
                        fs::read_to_string(paths.model(&definition).join("MODEL_LICENSE.txt"))
                            .unwrap();
                    assert_eq!(
                        hex(ring::digest::digest(&SHA256, license.as_bytes()).as_ref()),
                        "7dba975a2069691db4992b0592d70828b330d2f8a30a71450f4e152a554e84f8"
                    );
                    assert!(
                        fs::read_to_string(paths.model(&definition).join("ATTRIBUTION.txt"))
                            .unwrap()
                            .contains("Alibaba Group")
                    );
                }
                if matches!(official.kind, ModelKind::Whisper | ModelKind::WhisperTurbo) {
                    assert_eq!(
                        fs::read_to_string(paths.model(&definition).join("MODEL_LICENSE.txt"))
                            .unwrap(),
                        catalog::WHISPER_LICENSE
                    );
                    assert_eq!(
                        hex(
                            ring::digest::digest(&SHA256, catalog::WHISPER_LICENSE.as_bytes())
                                .as_ref()
                        ),
                        "b5d65a59060e68c4ff940e1eddfa6f94b2d68fdf58ed7f4dd57721c997e35e9d"
                    );
                    let attribution =
                        fs::read_to_string(paths.model(&definition).join("ATTRIBUTION.txt"))
                            .unwrap();
                    assert!(attribution.contains("Copyright (c) 2022 OpenAI"));
                    assert!(
                        attribution
                            .contains(official.assets[0].url.split("/resolve/").next().unwrap())
                    );
                }
            } else {
                assert!(!paths.model(&definition).exists());
            }
        }
    }
}
#[test]
fn catalog_retains_ids_and_pins_distinct_complete_artifacts() {
    use crate::local_models::*;
    assert_eq!(
        MODELS.iter().map(|d| d.id).collect::<Vec<_>>(),
        [PARAFORMER, WHISPER, SENSEVOICE, WHISPER_TURBO]
    );
    assert_eq!(
        MODELS.iter().map(Definition::bytes).collect::<Vec<_>>(),
        [237202501, 103609903, 239549735, 1036613791]
    );
    let mut ids = std::collections::BTreeSet::new();
    for definition in MODELS {
        assert!(ids.insert(definition.id));
        assert!(!definition.recommendation.is_empty());
        let mut names = std::collections::BTreeSet::new();
        for asset in definition.assets {
            assert!(names.insert(asset.name));
            assert!(asset.url.starts_with("https://huggingface.co/"));
            assert!(asset.url.ends_with(asset.name));
            let revision = asset
                .url
                .split("/resolve/")
                .nth(1)
                .unwrap()
                .split('/')
                .next()
                .unwrap();
            assert_eq!(revision.len(), 40);
            assert!(revision.bytes().all(|c| c.is_ascii_hexdigit()));
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.sha256.bytes().all(|c| c.is_ascii_hexdigit()));
            assert!(asset.bytes > 0);
        }
    }
}
#[test]
fn lock_excludes_second_writer_and_releases_after_drop() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("models");
    let first = Lock::acquire(&root).unwrap();
    assert!(Lock::acquire(&root).is_err());
    drop(first);
    assert!(Lock::acquire(&root).is_ok());
}
#[test]
fn remove_refuses_symlink_and_preserves_target() {
    let (_temp, paths, source) = fixture();
    std::os::unix::fs::symlink(&source, paths.model(&TEST_MODEL)).unwrap();
    assert!(remove(&paths, &TEST_MODEL, &AtomicBool::new(false)).is_err());
    assert_eq!(
        fs::read(source.join(TEST_MODEL.assets[0].name)).unwrap(),
        b"abc"
    );
}
