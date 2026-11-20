use super::*;
use std::net::TcpListener;

fn preparing() -> Progress {
    Progress::Stage("正在准备…".into())
}

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
        let result =
            tokio::time::timeout(Duration::from_secs(2), network(pending, &cancel, "test"))
                .await
                .unwrap();
        assert!(result.unwrap_err().contains("取消"));
        setter.await.unwrap();
    });
}

#[test]
fn transfer_checks_http_size_hash_and_truncation() {
    for (status, body, length, expected_error) in [
        ("200 OK", "abc", "3", None),
        ("200 OK", "xyz", "3", Some("完整性校验失败")),
        ("200 OK", "abcd", "4", Some("大小与模型目录不符")),
        ("200 OK", "ab", "3", Some("连接失败或中断")),
        ("404 Not Found", "", "0", Some("HTTP 404 Not Found")),
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
                "HTTP/1.1 {status}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
            )
            .unwrap();
        });
        let temp = tempfile::tempdir().unwrap();
        let asset = Asset {
            name: "encoder.int8.onnx",
            url: Box::leak(url.into_boxed_str()),
            bytes: 3,
            sha256: HASH,
        };
        let destination = temp.path().join(asset.name);
        let progress = Mutex::new(preparing());
        let result = runtime().block_on(download(
            &reqwest::Client::builder().no_proxy().build().unwrap(),
            &asset,
            &destination,
            &AtomicBool::new(false),
            &progress,
            Transfer {
                subject: "模型",
                offset: 0,
                total: 3,
            },
        ));
        worker.join().unwrap();
        if let Some(expected_error) = expected_error {
            let error = result.unwrap_err();
            assert!(error.contains(asset.name), "{error}");
            assert!(error.contains(expected_error), "{error}");
        } else {
            result.unwrap();
        }
        let progress = progress.into_inner().unwrap();
        if status == "404 Not Found" {
            assert!(!destination.exists());
            assert!(
                matches!(progress, Progress::Stage(message) if message.contains(asset.name) && message.contains("连接"))
            );
        } else if expected_error.is_none() {
            assert!(
                matches!(progress, Progress::Stage(message) if message.contains(asset.name) && message.contains("校验"))
            );
        }
    }
}

#[test]
fn partial_download_reports_scoped_bytes_and_cancels_while_waiting_for_more() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/engine", listener.local_addr().unwrap());
    let (release, hold) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert!(stream.read(&mut [0; 4096]).unwrap() > 0);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\na")
            .unwrap();
        let _ = hold.recv_timeout(Duration::from_secs(5));
    });
    let temp = tempfile::tempdir().unwrap();
    let asset = Asset {
        name: "engine.tgz",
        url: Box::leak(url.into_boxed_str()),
        bytes: 3,
        sha256: HASH,
    };
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let cancel = AtomicBool::new(false);
    let progress = Mutex::new(preparing());
    runtime().block_on(async {
        let destination = temp.path().join(asset.name);
        let mut fetch = std::pin::pin!(download(&client, &asset, &destination, &cancel, &progress,
            Transfer { subject: "识别引擎", offset: 0, total: 3 }));
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    result = &mut fetch => panic!("transfer finished before cancellation: {result:?}"),
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {
                        if matches!(&*progress.lock().unwrap(), Progress::Transfer { message, completed: 1, total: 3 }
                            if message.contains("识别引擎") && message.contains(asset.name)) {
                            break;
                        }
                    }
                }
            }
        }).await.unwrap();
        cancel.store(true, Ordering::Release);
        let error = tokio::time::timeout(Duration::from_secs(2), &mut fetch).await.unwrap().unwrap_err();
        assert!(error.contains("取消"));
    });
    release.send(()).unwrap();
    worker.join().unwrap();
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
    // Exercise the current downloadable engine even when a compatible legacy
    // Orca runtime is present on this development machine.
    runtime()
        .block_on(install_runtime(
            &paths,
            &client().unwrap(),
            &AtomicBool::new(false),
            &Mutex::new(preparing()),
        ))
        .unwrap();
    let import = std::env::var_os("VOICEPORT_TEST_IMPORT_DIR").map(PathBuf::from);
    let operation = if let Some(import) = import {
        assert!(import.is_absolute(), "explicit import must be absolute");
        println!(
            "Importing pinned {} from {} ({} model bytes)",
            definition.id,
            import.display(),
            definition.bytes()
        );
        Operation::Import(import)
    } else {
        println!(
            "Downloading pinned {} ({} model bytes)",
            definition.id,
            definition.bytes()
        );
        Operation::Download
    };
    let started = std::time::Instant::now();
    runtime()
        .block_on(install(
            &paths,
            definition,
            operation,
            &AtomicBool::new(false),
            &Mutex::new(preparing()),
        ))
        .unwrap();
    let model = paths.discover(definition).unwrap();
    assert_eq!(model.runtime, paths.runtime());
    crate::local_runtime::probe(&model, "", &voiceport_core::CancellationToken::new()).unwrap();
    assert_eq!(stages(&paths), 0);
    for (name, contents) in catalog::RUNTIME_NOTICES {
        assert_eq!(fs::read(paths.runtime().join(name)).unwrap(), *contents);
    }
    assert_eq!(paths.installation(definition), Installation::Current);
    for (name, text) in bundled_notices(definition.kind) {
        assert_eq!(
            fs::read_to_string(paths.model(definition).join(name)).unwrap(),
            text
        );
    }
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
        br#"{"version":"2.0.1"}"#,
    )
    .unwrap();
    for name in ["libsherpa-onnx-c-api.dylib", "libonnxruntime.dylib"] {
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
    let progress = Mutex::new(preparing());
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
    fs::write(paths.model(&TEST_MODEL).join("old-format-marker"), "old").unwrap();
    runtime()
        .block_on(install(
            &paths,
            &TEST_MODEL,
            Operation::Import(source.clone()),
            &cancel,
            &progress,
        ))
        .unwrap();
    assert!(!paths.model(&TEST_MODEL).join("old-format-marker").exists());
    assert_eq!(paths.installation(&TEST_MODEL), Installation::Current);
    assert_eq!(stages(&paths), 0);
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
                    &Mutex::new(preparing())
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
                &Mutex::new(preparing()),
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
                assert_eq!(paths.installation(&definition), Installation::Current);
                for (name, text) in bundled_notices(official.kind) {
                    assert_eq!(
                        fs::read_to_string(paths.model(&definition).join(name)).unwrap(),
                        text
                    );
                }
                if matches!(
                    official.kind,
                    ModelKind::ParakeetTdtV3
                        | ModelKind::ParakeetTdtV2
                        | ModelKind::ParakeetUnified
                        | ModelKind::Nemotron
                ) {
                    assert!(paths.discover(&definition).unwrap().joiner.is_some());
                }
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
        [
            PARAFORMER,
            WHISPER,
            SENSEVOICE,
            WHISPER_TURBO,
            PARAKEET_TDT_V3,
            PARAKEET_TDT_V2,
            PARAKEET_UNIFIED,
            NEMOTRON
        ]
    );
    assert_eq!(
        MODELS.iter().map(Definition::bytes).collect::<Vec<_>>(),
        [
            237202501, 103609903, 239549735, 1036613791, 670576784, 661288526, 663048978, 682215356
        ]
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
fn failed_reinstallation_preserves_usable_copy_and_staging_is_removed() {
    for case in ["corrupt", "missing", "cancelled", "symlink"] {
        let (_temp, paths, source) = fixture();
        runtime()
            .block_on(install(
                &paths,
                &TEST_MODEL,
                Operation::Import(source.clone()),
                &AtomicBool::new(false),
                &Mutex::new(preparing()),
            ))
            .unwrap();
        let before = fs::read(paths.model(&TEST_MODEL).join("VOICEPORT_MODEL.json")).unwrap();
        let file = source.join(TEST_MODEL.assets[1].name);
        match case {
            "corrupt" => fs::write(&file, b"xyz").unwrap(),
            "missing" => fs::remove_file(&file).unwrap(),
            "symlink" => {
                fs::remove_file(&file).unwrap();
                std::os::unix::fs::symlink(source.join(TEST_MODEL.assets[0].name), &file).unwrap();
            }
            _ => {}
        }
        let error = runtime()
            .block_on(install(
                &paths,
                &TEST_MODEL,
                Operation::Import(source),
                &AtomicBool::new(case == "cancelled"),
                &Mutex::new(preparing()),
            ))
            .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(
            paths.installation(&TEST_MODEL),
            Installation::Current,
            "{case}"
        );
        assert_eq!(
            fs::read(paths.model(&TEST_MODEL).join("VOICEPORT_MODEL.json")).unwrap(),
            before
        );
        for asset in TEST_MODEL.assets {
            assert_eq!(
                fs::read(paths.model(&TEST_MODEL).join(asset.name)).unwrap(),
                b"abc"
            );
        }
        assert_eq!(stages(&paths), 0);
    }
}
#[test]
fn installation_distinguishes_current_legacy_incompatible_and_special_notices() {
    let (_temp, paths, source) = fixture();
    assert_eq!(paths.installation(&TEST_MODEL), Installation::Missing);
    runtime()
        .block_on(install(
            &paths,
            &TEST_MODEL,
            Operation::Import(source),
            &AtomicBool::new(false),
            &Mutex::new(preparing()),
        ))
        .unwrap();
    assert_eq!(paths.installation(&TEST_MODEL), Installation::Current);
    let receipt = paths.model(&TEST_MODEL).join("VOICEPORT_MODEL.json");
    fs::remove_file(&receipt).unwrap();
    assert_eq!(paths.installation(&TEST_MODEL), Installation::Legacy);
    assert!(paths.discover(&TEST_MODEL).is_ok());
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(receipt.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert_eq!(paths.installation(&TEST_MODEL), Installation::Legacy);
    fs::write(
        paths.model(&TEST_MODEL).join(TEST_MODEL.assets[0].name),
        "wrong-format",
    )
    .unwrap();
    assert_eq!(paths.installation(&TEST_MODEL), Installation::Incompatible);
    assert!(paths.discover(&TEST_MODEL).is_err());
}
#[test]
fn model_licenses_and_exact_multilingual_nemotron_provenance_are_bundled() {
    for (license, expected) in [
        (
            catalog::CC_BY_4_LICENSE,
            "9ba9550ad48438d0836ddab3da480b3b69ffa0aac7b7878b5a0039e7ab429411",
        ),
        (
            catalog::NVIDIA_OPEN_MODEL_LICENSE,
            "ce3f3dfe1955f49a899892ad379c02a8ac74d71899c1e2532e5a122a852b4ea7",
        ),
        (
            catalog::OPENMDW_LICENSE,
            "4b126232937611510f638632b554733444bd8523023be54bdb64adaf529f3a26",
        ),
    ] {
        assert_eq!(
            hex(ring::digest::digest(&SHA256, license.as_bytes()).as_ref()),
            expected
        );
    }
    let nemotron = MODELS
        .iter()
        .find(|d| d.kind == ModelKind::Nemotron)
        .unwrap();
    assert!(nemotron.assets.iter().all(|a| {
        a.url
            .contains("sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8")
    }));
    assert!(catalog::NEMOTRON_ATTRIBUTION.contains("nvidia/nemotron-3.5-asr-streaming-0.6b/tree/"));
    assert!(!catalog::NEMOTRON_ATTRIBUTION.contains("nemotron-speech-streaming-en"));
    assert!(catalog::OPENMDW_LICENSE.contains("all copyright notices and other notices of origin"));
}

#[test]
fn runtime_download_counts_only_the_archive_and_bundles_pinned_onnx_notices() {
    assert_eq!(runtime_bytes(), 10_013_200);
    for (filename, expected) in [
        (
            "LICENSE-ONNX-Runtime.txt",
            "2f07c72751aed99790b8a4869cf2311df85a860b22ded05fa22803587a48922c",
        ),
        (
            "ThirdPartyNotices-ONNX-Runtime.txt",
            "0e07b95f3a8d6230037707c5c4a2b554d12c4cb67369669ac255635528ffcee2",
        ),
    ] {
        let (_, contents) = catalog::RUNTIME_NOTICES
            .iter()
            .find(|(name, _)| *name == filename)
            .unwrap();
        assert_eq!(
            hex(ring::digest::digest(&SHA256, contents).as_ref()),
            expected,
            "{filename}"
        );
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
