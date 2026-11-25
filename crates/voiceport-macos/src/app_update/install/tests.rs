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

fn fixture_target(home: &Path) -> PersonalInstall {
    let installed = home.join("Applications").join(APP);
    fs::create_dir_all(&installed).unwrap();
    PersonalInstall::new(
        &installed,
        home,
        unsafe { libc::geteuid() },
        &transport::default_path(),
    )
    .unwrap()
}

#[test]
fn a_rejected_handoff_keeps_the_staged_package_available_for_retry() {
    let mut staged = Staged::fixture("99.0.0");
    let path = staged.directory.path().join("candidate");
    fs::write(&path, b"staged candidate").unwrap();
    for _ in 0..2 {
        assert_eq!(
            staged.handoff(Path::new("not-an-installation")).err(),
            Some(MANUAL_ONLY)
        );
        assert_eq!(staged.version, "99.0.0");
        assert_eq!(fs::read(&path).unwrap(), b"staged candidate");
    }
}

#[test]
fn swap_keeps_the_previous_bundle_and_does_not_touch_adjacent_data() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let target = fixture_target(temp.path());
    let installed = &target.app;
    let old_stage = Builder::new()
        .prefix(STAGE)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(target.parent())
        .unwrap();
    let staged = old_stage.path().join(APP);
    let data = temp.path().join("VoicePort data");
    fs::create_dir(&staged).unwrap();
    fs::create_dir(&data).unwrap();
    fs::write(installed.join("old"), b"keep for rollback").unwrap();
    fs::write(staged.join("new"), b"update").unwrap();
    fs::write(data.join("model"), b"untouched").unwrap();
    let previous = target.verify().unwrap();
    let next = BundleId::read(&staged).unwrap();
    let lock = lock_installation(&target).unwrap();
    verified_swap(&target, &lock.directory, old_stage.path(), &previous, &next).unwrap();
    assert_eq!(fs::read(installed.join("new")).unwrap(), b"update");
    assert_eq!(fs::read(staged.join("old")).unwrap(), b"keep for rollback");
    assert_eq!(fs::read(data.join("model")).unwrap(), b"untouched");
    assert!(verified_swap(&target, &lock.directory, old_stage.path(), &previous, &next).is_err());
    assert_eq!(fs::read(installed.join("new")).unwrap(), b"update");

    let launch = reopen_verified(&target, &next, |path| {
        assert_eq!(path, installed);
        assert_eq!(fs::read(path.join("new")).unwrap(), b"update");
        Ok(())
    });
    finish_launch(launch, false, || {
        verified_swap(&target, &lock.directory, old_stage.path(), &next, &previous).unwrap();
        reopen_verified(&target, &previous, |path| {
            assert_eq!(path, installed);
            assert_eq!(fs::read(path.join("old")).unwrap(), b"keep for rollback");
            Ok(())
        })
    })
    .unwrap();
    assert_eq!(fs::read(staged.join("new")).unwrap(), b"update");
    assert_eq!(fs::read(data.join("model")).unwrap(), b"untouched");
}

#[test]
fn rejects_shared_wrong_owner_test_socket_and_linked_install() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let target = fixture_target(temp.path());
    let default = transport::default_path();
    let shared = Path::new("/Applications/VoicePort.app");
    assert!(PersonalInstall::new(shared, temp.path(), target.uid, &default).is_err());
    assert!(PersonalInstall::new(shared, Path::new("/"), target.uid, &default).is_err());
    assert_eq!(
        Staged::fixture("0.3.15").handoff(shared).err(),
        Some(MANUAL_ONLY)
    );
    let args = ["2", shared.to_str().unwrap(), "/unused", "0.3.15", "0", "0"].map(str::to_owned);
    assert_eq!(apply(&args), Err(MANUAL_ONLY));
    assert!(checkable_installation(&target.app, temp.path()));
    assert!(
        PersonalInstall::new(
            &target.app,
            temp.path(),
            target.uid.wrapping_add(1),
            &default
        )
        .is_err()
    );
    assert!(
        PersonalInstall::new(
            &target.app,
            temp.path(),
            target.uid,
            &temp.path().join("fixture.sock")
        )
        .is_err()
    );
    let stage = Builder::new()
        .prefix(STAGE)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(target.parent())
        .unwrap();
    let candidate = stage.path().join(APP);
    fs::create_dir(&candidate).unwrap();
    let original_id = target.verify().unwrap();
    let staged_id = BundleId::read(&candidate).unwrap();
    let lock = lock_installation(&target).unwrap();
    let original = target.parent().join("old-app");
    fs::rename(&target.app, &original).unwrap();
    std::os::unix::fs::symlink(&original, &target.app).unwrap();
    assert!(
        verified_swap(
            &target,
            &lock.directory,
            stage.path(),
            &original_id,
            &staged_id
        )
        .is_err()
    );
    assert!(
        reopen_verified(&target, &original_id, |_| panic!(
            "aliased path must not launch"
        ))
        .is_err()
    );
    assert_eq!(BundleId::read(&original).unwrap(), original_id);
    assert_eq!(BundleId::read(&candidate).unwrap(), staged_id);
    assert!(fs::symlink_metadata(&target.app).unwrap().is_symlink());
}

#[test]
fn new_and_marker_only_installers_exclude_each_other() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let target = fixture_target(temp.path());
    let marker_path = target.parent().join(".voiceport-install.lock");
    let old = || {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker_path)
    };
    let marker = old().unwrap();
    assert!(lock_installation(&target).is_err());
    assert_eq!(
        BundleId::from_metadata(&marker.metadata().unwrap()),
        BundleId::from_metadata(&fs::metadata(&marker_path).unwrap())
    );
    drop(marker);
    fs::remove_file(&marker_path).unwrap();
    let lock = lock_installation(&target).unwrap();
    assert_eq!(old().unwrap_err().kind(), io::ErrorKind::AlreadyExists);
    assert!(open_directory(target.parent()).unwrap().try_lock().is_err());
    drop(lock);
    assert!(old().is_ok());
}

#[test]
fn unknown_launch_is_not_retried_or_rolled_back() {
    let no_recovery =
        || panic!("must not recover without a successful launch receipt and failed readback");
    assert_eq!(
        finish_launch(Err("uncertain open"), false, no_recovery),
        Err(MANUAL_RECOVERY)
    );
    assert_eq!(
        finish_launch(Err("uncertain open"), true, no_recovery),
        Ok(())
    );
    assert_eq!(
        finish_launch(Ok(()), false, || Err("recovery checked")),
        Err("recovery checked")
    );
}

#[test]
fn unknown_legacy_socket_blocks_rollback_without_swapping_or_removing_it() {
    use std::os::unix::net::UnixListener;
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let target = fixture_target(temp.path());
    let stage = Builder::new()
        .prefix(STAGE)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(target.parent())
        .unwrap();
    let backup = stage.path().join(APP);
    fs::create_dir(&backup).unwrap();
    let original = BundleId::read(&backup).unwrap();
    let replacement = target.verify().unwrap();
    let parent = temp.path().join("ipc");
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    let socket = parent.join("control.sock");
    drop(UnixListener::bind(&socket).unwrap());
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let identity = BundleId::from_metadata(&fs::symlink_metadata(&socket).unwrap());
    let lock = lock_installation(&target).unwrap();
    assert_eq!(
        restore_previous(
            &target,
            &lock.directory,
            stage.path(),
            &socket,
            &original,
            &replacement
        ),
        Err(MANUAL_RECOVERY)
    );
    assert_eq!(target.verify().unwrap(), replacement);
    assert_eq!(BundleId::read(&backup).unwrap(), original);
    assert_eq!(
        BundleId::from_metadata(&fs::symlink_metadata(&socket).unwrap()),
        identity
    );
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
    let target = fixture_target(temp.path());
    let candidate = temp.path().join("candidate");
    let socket = temp.path().join("ipc/control.sock");
    let original = target.verify().unwrap();
    let lock = lock_installation(&target).unwrap();
    let _running_app = transport::Server::bind(&socket).unwrap();
    assert_eq!(
        install_new(
            &target,
            &lock.directory,
            &candidate,
            &socket,
            "0.3.14",
            &original
        ),
        Err("VoicePort 已经在运行或控制连接不可用；未替换应用。")
    );
    assert_eq!(target.verify().unwrap(), original);
    assert!(socket.exists());
}

#[test]
fn older_directory_lock_blocks_install_until_owner_exits() {
    let temp = tempfile::tempdir_in("/private/tmp").unwrap();
    let target = fixture_target(temp.path());
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
        .arg(target.parent())
        .arg(socket_parent.join("control.sock.lock"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = [0; 6];
    child.stdout.take().unwrap().read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready\n");
    assert!(lock_installation(&target).is_err());
    assert!(transport::Server::bind(&socket).is_err());
    assert!(!socket.exists());
    child.kill().unwrap();
    child.wait().unwrap();
    let _installation = lock_installation(&target).unwrap();
    let _server = transport::Server::bind(&socket).unwrap();
    assert!(target.parent().join(".voiceport-install.lock").exists());
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
    let target = fixture_target(temp.path());
    let installed = &target.app;
    let socket = temp.path().join("ipc/control.sock");
    let previous = fs::read(old_zip).unwrap();
    let candidate = fs::read(new_zip).unwrap();
    extract(&previous, target.parent()).unwrap();
    let current = env!("CARGO_PKG_VERSION");
    let mut next = Version::parse(current).unwrap().0;
    next[2] += 1;
    let next = format!("{}.{}.{}", next[0], next[1], next[2]);
    assert_eq!(
        stage(&target, current, &previous).err(),
        Some("更新包的版本或构建号不匹配，或试图降级。")
    );
    let staged = stage(&target, &next, &candidate).unwrap();
    assert_eq!(
        fs::metadata(staged.directory.path()).unwrap().mode() & 0o777,
        0o700
    );
    let replacement = staged.directory.path().join(APP);
    let ad_hoc = Builder::new()
        .prefix(STAGE)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(target.parent())
        .unwrap();
    extract(&candidate, ad_hoc.path()).unwrap();
    let wrong_signer = ad_hoc.path().join(APP);
    codesign(&["--force", "--sign", "-"], &wrong_signer).unwrap();
    assert!(inspect(&wrong_signer).is_ok());
    let lock = lock_installation(&target).unwrap();
    assert!(
        install_new(
            &target,
            &lock.directory,
            ad_hoc.path(),
            &socket,
            &next,
            &staged.original
        )
        .is_err()
    );
    assert!(BundleId::read(installed).unwrap() == staged.original);
    let data = temp.path().join("user-data");
    fs::write(&data, b"retained fixture data").unwrap();
    let new = install_new(
        &target,
        &lock.directory,
        staged.directory.path(),
        &socket,
        &next,
        &staged.original,
    )
    .unwrap();
    assert_eq!(inspect(installed).unwrap().version, next);
    assert_eq!(inspect(&replacement).unwrap().version, current);
    assert!(BundleId::read(&replacement).unwrap() == staged.original);
    assert_eq!(fs::read(data).unwrap(), b"retained fixture data");
    assert!(!socket.exists());
    assert!(target.parent().join(".voiceport-install.lock").exists());
    let server = transport::Server::bind(&socket).unwrap();
    assert!(
        restore_previous(
            &target,
            &lock.directory,
            staged.directory.path(),
            &socket,
            &staged.original,
            &new
        )
        .is_err()
    );
    assert_eq!(BundleId::read(installed).unwrap(), new);
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
    restore_previous(
        &target,
        &lock.directory,
        staged.directory.path(),
        &socket,
        &staged.original,
        &new,
    )
    .unwrap();
    assert_eq!(inspect(installed).unwrap().version, current);
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
