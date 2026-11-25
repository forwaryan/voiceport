use super::*;
use crate::Action;
use std::time::Duration;
fn socket() -> (tempfile::TempDir, PathBuf) {
    // Honor the transport's path bound regardless of the caller's TMPDIR length.
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let path = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("private/control.sock");
    (temp, path)
}
fn next(server: &Server) -> Pending {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(pending) = server.next() {
            return pending;
        }
        assert!(Instant::now() < deadline, "request did not arrive");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn actual_same_uid_round_trip_and_owned_socket_cleanup() {
    let (_temp, path) = socket();
    let server = Server::bind(&path).unwrap();
    assert_eq!(
        fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    let client_path = path.clone();
    let client = std::thread::spawn(move || call(&client_path, Action::Status));
    let pending = next(&server);
    assert!(!pending.expired());
    pending.respond(Response::ok(serde_json::json!({"state":"idle"})));
    assert_eq!(client.join().unwrap().unwrap().data["state"], "idle");
    assert!(Server::bind(&path).is_err());
    assert!(path.exists());
    drop(server);
    assert!(!path.exists());
}
#[test]
fn unsafe_modes_links_unknown_files_and_replacements_are_not_removed() {
    let (_temp, path) = socket();
    assert!(Server::bind(&_temp.path().join("x".repeat(101))).is_err());
    fs::create_dir(path.parent().unwrap()).unwrap();
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Server::bind(&path).is_err());
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&path, b"unknown file").unwrap();
    assert!(Server::bind(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"unknown file");
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("missing", &path).unwrap();
    assert!(Server::bind(&path).is_err());
    assert!(call(&path, Action::Status).is_err());
    fs::remove_file(&path).unwrap();
    let server = Server::bind(&path).unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"replacement").unwrap();
    drop(server);
    assert_eq!(fs::read(&path).unwrap(), b"replacement");
    let linked = path.parent().unwrap().parent().unwrap().join("linked");
    std::os::unix::fs::symlink(path.parent().unwrap(), &linked).unwrap();
    assert!(Server::bind(&linked.join("other.sock")).is_err());
}
#[test]
fn startup_preserves_unowned_stale_sockets_and_skips_partial_publications() {
    let (_temp, path) = socket();
    private_parent(&path, true).unwrap();
    let unknown = UnixListener::bind(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let identity = socket_identity(&path).unwrap();
    drop(unknown);
    assert!(lock_quiet(&path).is_err());
    assert_eq!(socket_identity(&path).unwrap(), identity);
    fs::remove_file(&path).unwrap();

    let first_slot = owned_socket_names(&path).next().unwrap();
    let partial = UnixListener::bind(&first_slot).unwrap();
    fs::set_permissions(&first_slot, fs::Permissions::from_mode(0o600)).unwrap();
    let identity = socket_identity(&first_slot).unwrap();
    drop(partial);
    let server = Server::bind(&path).unwrap();
    let owned = server._guard.owned.clone();
    assert_ne!(owned, first_slot);
    assert_eq!(
        socket_identity(&path).unwrap(),
        socket_identity(&owned).unwrap()
    );
    drop(server);
    assert!(!path.exists());
    assert!(!owned.exists());
    assert_eq!(socket_identity(&first_slot).unwrap(), identity);
}

#[test]
fn invalid_version_and_oversize_frames_never_reach_dispatch() {
    let (_temp, path) = socket();
    let server = Server::bind(&path).unwrap();
    for kind in ["version", "oversize", "json"] {
        let mut client = UnixStream::connect(&path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        match kind {
            "version" => write_frame(
                &mut client,
                &Request {
                    schema_version: 99,
                    action: Action::Status,
                },
                deadline,
            )
            .unwrap(),
            "oversize" => client
                .write_all(&((MAX_FRAME + 1) as u32).to_be_bytes())
                .unwrap(),
            _ => {
                client.write_all(&1u32.to_be_bytes()).unwrap();
                client.write_all(b"?").unwrap();
            }
        }
        let response: Response = read_frame(&mut client, deadline).unwrap();
        assert_eq!(
            response.error.unwrap().code,
            match kind {
                "version" => "version_mismatch",
                "oversize" => "frame_too_large",
                _ => "invalid_request",
            }
        );
        assert!(server.next().is_none());
    }
}
#[test]
fn read_deadline_is_total_even_with_partial_progress_and_disconnects() {
    let (mut a, mut b) = UnixStream::pair().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = b.write_all(&100u32.to_be_bytes());
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(50));
            if b.write_all(b" ").is_err() {
                break;
            }
        }
    });
    let start = Instant::now();
    let response = read_frame::<Request>(&mut a, start + Duration::from_millis(150))
        .err()
        .unwrap();
    assert_eq!(response.error.unwrap().code, "timeout");
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(a);
    writer.join().unwrap();
    let (mut a, b) = UnixStream::pair().unwrap();
    drop(b);
    assert!(read_frame::<Request>(&mut a, Instant::now() + Duration::from_secs(1)).is_err());
}
#[test]
fn connection_limit_returns_busy_without_unbounded_threads() {
    let (_temp, path) = socket();
    let server = Server::bind(&path).unwrap();
    let mut clients = vec![];
    let mut held = vec![];
    for _ in 0..4 {
        let mut stream = UnixStream::connect(&path).unwrap();
        write_frame(
            &mut stream,
            &Request {
                schema_version: VERSION,
                action: Action::Status,
            },
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        held.push(next(&server));
        clients.push(stream);
    }
    let response = call(&path, Action::Status).unwrap();
    assert_eq!(response.error.unwrap().code, "busy");
    for p in held {
        p.respond(Response::ok(serde_json::Value::Null));
    }
    for mut c in clients {
        let _: Response = read_frame(&mut c, Instant::now() + Duration::from_secs(1)).unwrap();
    }
}
