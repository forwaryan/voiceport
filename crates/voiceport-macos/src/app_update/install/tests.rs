use super::*;
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use zip::write::SimpleFileOptions;

fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (path, bytes) in entries {
        writer
            .start_file(
                *path,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn extracts_only_the_app_tree() {
    let directory = tempfile::tempdir().unwrap();
    let zip = make_zip(&[
        ("VoicePort.app/Contents/Info.plist", b"app data"),
        ("__MACOSX/._VoicePort.app", b"metadata"),
    ]);
    extract(&zip, directory.path()).unwrap();
    assert_eq!(
        fs::read(directory.path().join("VoicePort.app/Contents/Info.plist")).unwrap(),
        b"app data"
    );
    assert!(!directory.path().join("__MACOSX").exists());
}

#[test]
fn refuses_archive_paths_outside_the_bundle_and_true_symlinks() {
    for path in [
        "../evil",
        "/Applications/VoicePort.app/evil",
        "VoicePort.app/../evil",
        "VoicePort.app/a\\evil",
        "VoicePort.app//evil",
        "__MACOSX/../../evil",
        "VoicePort.app",
    ] {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            extract(&make_zip(&[(path, b"no")]), temp.path()).is_err(),
            "{path}"
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .add_symlink(
            "VoicePort.app/Contents/Resources/link",
            "/tmp",
            SimpleFileOptions::default(),
        )
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    assert!(extract(&writer.finish().unwrap().into_inner(), temp.path()).is_err());
}

#[test]
fn refuses_overwriting_regular_files_or_directory_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = make_zip(&[("VoicePort.app/a", b"one"), ("VoicePort.app/a/b", b"two")]);
    assert!(extract(&bytes, temp.path()).is_err());
    assert_eq!(
        fs::read(temp.path().join("VoicePort.app/a")).unwrap(),
        b"one"
    );
}

#[test]
fn rejects_duplicate_zip_entries_before_extraction() {
    let mut bytes = make_zip(&[("VoicePort.app/a", b"one"), ("VoicePort.app/b", b"two")]);
    let duplicate = b"VoicePort.app/b";
    let offsets: Vec<_> = bytes
        .windows(duplicate.len())
        .enumerate()
        .filter_map(|(i, name)| (name == duplicate).then_some(i + duplicate.len() - 1))
        .collect();
    assert_eq!(offsets.len(), 2);
    for offset in offsets {
        bytes[offset] = b'a';
    }
    let temp = tempfile::tempdir().unwrap();
    assert!(extract(&bytes, temp.path()).is_err());
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn swap_keeps_the_previous_bundle_and_does_not_touch_adjacent_data() {
    let temp = tempfile::tempdir().unwrap();
    let installed = temp.path().join(APP);
    let old_stage = temp.path().join(".voiceport-update-fixture");
    let staged = old_stage.join(APP);
    let data = temp.path().join("VoicePort data");
    fs::create_dir(&installed).unwrap();
    fs::create_dir_all(&staged).unwrap();
    fs::create_dir(&data).unwrap();
    fs::write(installed.join("old"), b"keep for rollback").unwrap();
    fs::write(staged.join("new"), b"update").unwrap();
    fs::write(data.join("model"), b"untouched").unwrap();
    swap(&staged, &installed).unwrap();
    assert_eq!(fs::read(installed.join("new")).unwrap(), b"update");
    assert_eq!(fs::read(staged.join("old")).unwrap(), b"keep for rollback");
    assert_eq!(fs::read(data.join("model")).unwrap(), b"untouched");
    assert!(swap(&staged.join("absent"), &installed).is_err());
    assert_eq!(fs::read(installed.join("new")).unwrap(), b"update");
}

#[test]
fn shutdown_requires_the_acknowledgment_byte() {
    use std::os::unix::net::UnixStream;

    for (sent, expected) in [(&QUIT_ACK[..], true), (&[0][..], false), (&[][..], false)] {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(sent).unwrap();
        drop(writer);
        assert_eq!(confirmed_shutdown(reader, 100), expected);
    }
    let (reader, _writer) = UnixStream::pair().unwrap();
    assert!(!confirmed_shutdown(reader, 1));
}

#[test]
fn installation_rejects_an_occupied_control_socket() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let installed = temp.path().join(APP);
    let candidate = temp.path().join("candidate");
    let socket = temp.path().join("ipc/control.sock");
    fs::create_dir(&installed).unwrap();
    fs::create_dir(&candidate).unwrap();
    let original = BundleId::read(&installed).unwrap();
    let _running_app = transport::Server::bind(&socket).unwrap();
    assert_eq!(
        install_new(&installed, &candidate, &socket, "0.3.14", &original),
        Err("VoicePort 已经在运行或控制连接不可用；未替换应用。")
    );
    assert!(BundleId::read(&installed).unwrap() == original);
    assert!(socket.exists());
    assert!(!temp.path().join(".voiceport-install.lock").exists());
}

#[test]
fn crash_releases_install_and_startup_locks_without_leaving_paths() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let installed = temp.path().join(APP);
    let socket_parent = temp.path().join("ipc");
    let socket = socket_parent.join("control.sock");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&socket_parent)
        .unwrap();
    let mut child = Command::new("/usr/bin/python3")
        .args([
            "-I",
            "-c",
            "import fcntl, os, sys\nfds = [os.open(sys.argv[1], os.O_RDONLY), os.open(sys.argv[2], os.O_RDWR | os.O_CREAT, 0o600)]\nfor fd in fds: fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)\nprint('ready', flush=True)\nsys.stdin.read(1)",
        ])
        .arg(temp.path())
        .arg(socket_parent.join("control.sock.lock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = [0; 6];
    child.stdout.take().unwrap().read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready\n");
    assert!(lock_installation(&installed).is_err());
    assert!(transport::Server::bind(&socket).is_err());
    assert!(!socket.exists());
    child.kill().unwrap();
    child.wait().unwrap();
    let _installation = lock_installation(&installed).unwrap();
    let _server = transport::Server::bind(&socket).unwrap();
    assert!(!temp.path().join(".voiceport-install.lock").exists());
}

#[test]
#[ignore = "requires two authorized, same-identity signed archive fixtures"]
fn signed_update_stages_privately_and_replaces_only_the_current_bundle() {
    if let Some(path) = std::env::var_os("VOICEPORT_TEST_UPDATE_CRASH_SOCKET") {
        let _server = transport::Server::bind(Path::new(&path)).unwrap();
        std::process::exit(86);
    }
    let old_zip =
        std::env::var_os("VOICEPORT_TEST_RELEASE_ZIP").expect("signed previous ZIP fixture");
    let new_zip = std::env::var_os("VOICEPORT_TEST_UPDATE_ZIP").expect("signed new ZIP fixture");
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let installed = temp.path().join(APP);
    let socket = temp.path().join("ipc/control.sock");
    let previous = fs::read(old_zip).unwrap();
    let candidate = fs::read(new_zip).unwrap();
    extract(&previous, temp.path()).unwrap();
    let current = env!("CARGO_PKG_VERSION");
    let mut next = Version::parse(current).unwrap().0;
    next[2] += 1;
    let next = format!("{}.{}.{}", next[0], next[1], next[2]);
    assert_eq!(
        stage(&installed, current, &previous).err(),
        Some("更新包的版本或构建号不匹配，或试图降级。")
    );
    let staged = stage(&installed, &next, &candidate).unwrap();
    assert_eq!(
        fs::metadata(staged.directory.path()).unwrap().mode() & 0o777,
        0o700
    );
    let replacement = staged.directory.path().join(APP);
    let ad_hoc = tempfile::tempdir_in(temp.path()).unwrap();
    extract(&candidate, ad_hoc.path()).unwrap();
    let wrong_signer = ad_hoc.path().join(APP);
    codesign(&["--force", "--sign", "-"], &wrong_signer).unwrap();
    assert!(inspect(&wrong_signer).is_ok());
    assert!(install_new(&installed, &wrong_signer, &socket, &next, &staged.original).is_err());
    assert!(BundleId::read(&installed).unwrap() == staged.original);
    let data = temp.path().join("user-data");
    fs::write(&data, b"retained fixture data").unwrap();
    let new = install_new(&installed, &replacement, &socket, &next, &staged.original).unwrap();
    assert_eq!(inspect(&installed).unwrap().version, next);
    assert_eq!(inspect(&replacement).unwrap().version, current);
    assert!(BundleId::read(&replacement).unwrap() == staged.original);
    assert_eq!(fs::read(data).unwrap(), b"retained fixture data");
    assert!(!socket.exists());
    assert!(!temp.path().join(".voiceport-install.lock").exists());
    let server = transport::Server::bind(&socket).unwrap();
    assert!(restore_previous(&installed, &replacement, &socket, &staged.original, &new).is_err());
    assert_eq!(BundleId::read(&installed).unwrap(), new);
    drop(server);

    // Exit after binding but before responding to IPC, skipping Rust cleanup.
    let crash = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "--ignored",
            "app_update::install::tests::signed_update_stages_privately_and_replaces_only_the_current_bundle",
        ])
        .env("VOICEPORT_TEST_UPDATE_CRASH_SOCKET", &socket)
        .status()
        .unwrap();
    assert_eq!(crash.code(), Some(86));
    assert!(socket.exists());
    restore_previous(&installed, &replacement, &socket, &staged.original, &new).unwrap();
    assert_eq!(inspect(&installed).unwrap().version, current);
    assert_eq!(inspect(&replacement).unwrap().version, next);
    assert_eq!(
        fs::read(temp.path().join("user-data")).unwrap(),
        b"retained fixture data"
    );
    let _restored_server = transport::Server::bind(&socket).unwrap();
}

#[test]
#[ignore = "requires the authorized v0.3.13 archive fixture"]
fn known_release_archive_can_be_checked_with_no_app_start_or_install() {
    let archive = std::env::var_os("VOICEPORT_TEST_HISTORICAL_ZIP").expect("v0.3.13 ZIP fixture");
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    extract(&fs::read(archive).unwrap(), &root).unwrap();
    let package = root.join(APP);
    let info = inspect(&package).unwrap();
    assert_eq!(info.version, "0.3.13");
    assert_eq!(info.build, "19");
    // Same-certificate app and nested CLI retain their designated requirements
    // after strict bounded extraction of an authentic ditto archive.
    same_identity(&package, &package).unwrap();
    same_identity(&package.join(CLI), &package.join(CLI)).unwrap();
    fs::write(package.join("Contents/Info.plist"), b"modified").unwrap();
    assert!(inspect(&package).is_err());
}
