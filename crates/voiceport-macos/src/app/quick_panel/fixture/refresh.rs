//! Offscreen native views and a real CLI on a fixture-owned, same-UID socket.
use super::*;
use std::{
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const SLOW: Duration = Duration::from_secs(2);
const DEADLINE: Duration = Duration::from_secs(7);

fn cli_call(d: &Delegate, cli: &Path, socket: &Path, verb: &str, exit: i32) -> serde_json::Value {
    let mut child = Command::new(cli)
        .args(["--json", "--socket"])
        .arg(socket)
        .args(["app", "update", verb])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let until = Instant::now() + DEADLINE;
    while child.try_wait().unwrap().is_none() {
        d.poll_control();
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("private CLI {verb} timed out");
        }
        thread::sleep(Duration::from_millis(2));
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

pub(super) fn run(mtm: MainThreadMarker, cli: &Path) {
    let d = Delegate::new(mtm);
    // The production transport rejects linked ancestors, including macOS /tmp.
    let dir = tempfile::Builder::new()
        .prefix("vp-menu-")
        .permissions(Permissions::from_mode(0o700))
        .tempdir_in("/private/tmp")
        .unwrap();
    let socket = dir.path().join("ipc/control.sock");
    let bundle = dir.path().join("Applications/VoicePort.app");
    std::fs::create_dir_all(&bundle).unwrap();
    let (requests, update_requests) = mpsc::channel();
    d.configure_update_fixture(dir.path().to_owned(), bundle, requests);
    d.ivars().control.borrow_mut().server =
        Some(voiceport_control::transport::Server::bind(&socket).unwrap());
    d.ivars().controller.replace(Some(Controller::new()));
    d.build_context_menu();
    let panel = d.ivars().quick_panel.get_or_init(|| d.build_quick_panel());
    d.with_controller(|c| {
        c.automatic = false;
        c.microphone_uid = Some("fixture-a".into());
        d.refresh_menu_snapshot(c, false);
    });
    assert!(panel.device.stringValue().is_empty());
    d.with_controller(|c| d.refresh_quick_panel(c, false));
    assert!(panel.device.stringValue().is_empty());
    d.prepare_quick_panel(panel);
    assert!(panel.toggle.isEnabled());
    assert_eq!(panel.device.stringValue().to_string(), "麦克风：指定输入");
    assert_eq!(
        panel.update.title().to_string(),
        format!("软件更新：当前版本 {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(!panel.is_shown());

    let started = cli_call(&d, cli, &socket, "check", 0);
    assert_eq!(started["data"]["state"], "checking");
    let release = update_requests.recv_timeout(DEADLINE).unwrap();
    let update = thread::spawn(move || {
        thread::sleep(SLOW);
        release.send(Err("合成更新读取结束")).unwrap();
    });
    let before = Instant::now();
    let mut device_worker = None;
    d.ivars()
        .menu_device
        .borrow_mut()
        .poll(Some("fixture-a"), true, before, |uid| {
            assert_eq!(uid.as_deref(), Some("fixture-a"));
            let (old, receive) = mpsc::channel();
            device_worker = Some(thread::spawn(move || {
                thread::sleep(SLOW);
                old.send(Ok(Some("旧设备".into()))).unwrap();
            }));
            Ok(receive)
        });
    d.with_controller(|c| c.microphone_uid = Some("fixture-b".into()));
    let no_query = |_| panic!("unexpected duplicate or hidden device query");
    d.ivars()
        .menu_device
        .borrow_mut()
        .poll(Some("fixture-b"), true, Instant::now(), no_query);
    d.ivars().context_menu_visible.set(true);
    d.prepare_context_menu();
    let item = d.ivars().update_item.get().unwrap();
    assert!(item.title().to_string().starts_with("正在检查更新"));
    assert!(item.isEnabled()); // The update page remains reachable during an update.

    let mut open = Vec::new();
    for _ in 0..24 {
        let start = Instant::now();
        d.prepare_quick_panel(panel);
        assert!(panel.toggle.isEnabled());
        assert!(panel.files.isEnabled());
        open.push(start.elapsed());
    }
    open.sort_unstable();
    let p95 = open[(open.len() * 95).div_ceil(100) - 1];
    assert!(p95 <= Duration::from_millis(100), "open prep p95 {p95:?}");
    assert_eq!(panel.device.stringValue().to_string(), "麦克风：指定输入");
    assert_eq!(
        panel.update.title().to_string(),
        format!("软件更新：{}", item.title())
    );
    let mut queries = Vec::new();
    for _ in 0..16 {
        let start = Instant::now();
        assert_eq!(
            cli_call(&d, cli, &socket, "status", 0)["data"]["state"],
            "checking"
        );
        queries.push(start.elapsed());
    }
    queries.sort_unstable();
    let query = queries[(queries.len() * 95).div_ceil(100) - 1];
    assert!(
        query <= Duration::from_millis(250),
        "private status p95 {query:?}"
    );
    assert_eq!(
        cli_call(&d, cli, &socket, "check", 4)["error"]["code"],
        "busy"
    );
    assert!(update_requests.try_recv().is_err());
    assert!(
        before.elapsed() < SLOW,
        "open preparation and IPC outlasted slow query"
    );

    // The old device and update workers both finish with the UI tracking state
    // explicitly controlled by the fixture. No global menu or window is opened.
    device_worker.unwrap().join().unwrap();
    update.join().unwrap();
    d.ivars()
        .menu_device
        .borrow_mut()
        .poll(Some("fixture-b"), false, Instant::now(), no_query);
    let hidden = d.ivars().menu_device.borrow().label(Some("fixture-b"));
    assert_eq!(hidden, "指定输入");
    let (fresh, receive) = mpsc::channel();
    d.ivars()
        .menu_device
        .borrow_mut()
        .poll(Some("fixture-b"), true, Instant::now(), |uid| {
            assert_eq!(uid.as_deref(), Some("fixture-b"));
            Ok(receive)
        });
    fresh.send(Ok(Some("新设备".into()))).unwrap();
    d.ivars()
        .menu_device
        .borrow_mut()
        .poll(Some("fixture-b"), false, Instant::now(), no_query);
    d.with_controller(|c| d.refresh_quick_panel(c, false));
    assert_eq!(panel.device.stringValue().to_string(), "麦克风：指定输入");
    d.prepare_quick_panel(panel);
    assert_eq!(
        panel.device.stringValue().to_string(),
        "麦克风：指定输入 · 新设备"
    );

    let until = Instant::now() + DEADLINE;
    while d.update_snapshot()["state"] == "checking" {
        assert!(Instant::now() < until);
        d.update_tick();
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(d.update_snapshot()["state"], "failed");
    assert!(item.title().to_string().starts_with("无法检查更新"));
    assert!(item.isEnabled());
    let previous = panel.update.title();
    d.with_controller(|c| d.refresh_quick_panel(c, false));
    assert_eq!(panel.update.title(), previous);
    let due = panel.next_update_refresh.get().unwrap();
    panel.refresh_update(&d, false, due - Duration::from_millis(1));
    assert_eq!(panel.update.title(), previous);
    panel.refresh_update(&d, false, due);
    assert_eq!(
        panel.update.title().to_string(),
        format!("软件更新：{}", item.title())
    );
    let toggle = d.ivars().menu_toggle.get().unwrap();
    d.with_controller(|c| {
        let generation = c.state.begin().unwrap();
        assert!(c.state.permissions(generation, true));
        d.refresh_menu_snapshot(c, false);
    });
    assert_eq!(toggle.title().to_string(), "停止并转写");
    assert!(d.ivars().context_menu_visible.replace(false));
    d.with_controller(|c| {
        c.state.cancel(false);
        d.refresh_menu_snapshot(c, false);
    });
    assert_eq!(toggle.title().to_string(), "停止并转写");
    d.prepare_context_menu();
    assert_eq!(toggle.title().to_string(), "开始听写");
    d.ivars().control.borrow_mut().server.take();
    assert!(!socket.exists());
    println!(
        "silent refresh: 24 offscreen opens p95={p95:?}, private CLI status={query:?}; 2 s independent device/update reads, stale device rejected, visible menu item updated, hidden panel unchanged"
    );
}
