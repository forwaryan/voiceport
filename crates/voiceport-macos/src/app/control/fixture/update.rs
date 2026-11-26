use super::*;
use std::sync::mpsc::{self, TryRecvError};

const CHECK: &[&str] = &["app", "update", "check"];
const STATUS: &[&str] = &["app", "update", "status"];
const QUERY_FAILURE: &str = "合成夹具无法查询发行版本。";

fn status(fixture: &Fixture<'_>) -> Value {
    let reply = fixture.call(STATUS, 0);
    assert_eq!(reply["schema_version"], 1);
    reply["data"].clone()
}

fn wait_for(fixture: &Fixture<'_>, expected: &str) -> Value {
    let deadline = Instant::now() + LIMIT;
    loop {
        fixture.owner.update_tick();
        let result = status(fixture);
        if result["state"] == expected {
            assert_eq!(result["last_check_state"], expected);
            assert_eq!(result["busy"], false);
            return result;
        }
        assert_eq!(result["state"], "checking", "{result}");
        assert!(Instant::now() < deadline, "{result}");
    }
}

fn release(version: &str, with_asset: bool) -> Vec<u8> {
    let assets = if with_asset {
        json!([{
            "id": 23,
            "name": "VoicePort-macos-arm64.zip",
            "digest": format!("sha256:{}", "ab".repeat(32)),
            "size": 1234,
            "state": "uploaded",
        }])
    } else {
        json!([])
    };
    serde_json::to_vec(&json!({
        "tag_name": format!("v{version}"),
        "draft": false,
        "prerelease": false,
        "assets": assets,
    }))
    .unwrap()
}

pub(super) fn verify(fixture: &Fixture<'_>, folder: &Path) {
    let installed = folder.join("Applications/VoicePort.app");
    let (requests, received) = mpsc::channel();
    fixture
        .owner
        .configure_update_fixture(folder.to_path_buf(), installed.clone(), requests);
    let version = env!("CARGO_PKG_VERSION");
    let capabilities = fixture.call(&["app", "capabilities"], 0);
    assert_eq!(
        capabilities["data"]["app_update"],
        json!(["check", "status"])
    );
    assert_eq!(capabilities["data"]["version"], version);
    let cli_version = Command::new(fixture.cli)
        .args(["--json", "--version"])
        .output()
        .unwrap();
    assert!(cli_version.status.success());
    let cli_version: Value = serde_json::from_slice(&cli_version.stdout).unwrap();
    assert_eq!(cli_version["data"]["version"], version);
    assert_eq!(cli_version["schema_version"], 1);

    let read_only = || assert!(matches!(received.try_recv(), Err(TryRecvError::Empty)));
    let initial = status(fixture);
    assert_eq!(initial["state"], "idle");
    assert_eq!(initial["current_version"], version);
    assert_eq!(initial["latest_version"], Value::Null);
    assert_eq!(initial["latest_version_stale"], false);
    assert_eq!(initial, status(fixture));
    read_only();
    let unsupported = fixture.call(CHECK, 7);
    assert_eq!(unsupported["error"]["code"], "unsupported");
    assert_eq!(unsupported["data"]["state"], "failed");
    read_only();
    fs::create_dir_all(&installed).unwrap();
    let marker = installed.join("synthetic-install-marker");
    fs::write(&marker, b"old fixture bundle").unwrap();

    let begin = || {
        let started = fixture.call(CHECK, 0);
        assert_eq!(started["schema_version"], 1);
        assert_eq!(started["data"]["state"], "checking");
        assert_eq!(started["data"]["busy"], true);
        received.recv_timeout(LIMIT).unwrap()
    };
    let first = begin();
    let duplicate = fixture.call(CHECK, 4);
    assert_eq!(duplicate["error"]["code"], "busy");
    assert_eq!(duplicate["data"]["state"], "checking");
    assert_eq!(duplicate["data"], status(fixture));
    first.send(Err(QUERY_FAILURE)).unwrap();
    let failed = wait_for(fixture, "failed");
    assert_eq!(failed["reason"], QUERY_FAILURE);
    assert_eq!(failed["latest_version"], Value::Null);
    read_only();

    let newer = "9000.1.0";
    begin().send(Ok(release(newer, true))).unwrap();
    let available = wait_for(fixture, "update_available");
    assert_eq!(available["latest_version"], newer);
    assert_eq!(available["latest_version_stale"], false);
    assert_eq!(available["reason"], Value::Null);
    assert_eq!(available, status(fixture));
    read_only();

    let refresh = begin();
    let checking = status(fixture);
    assert_eq!(checking["latest_version"], newer);
    assert_eq!(checking["latest_version_stale"], true);
    assert_eq!(checking["last_check_state"], "update_available");
    refresh.send(Err(QUERY_FAILURE)).unwrap();
    let stale = wait_for(fixture, "failed");
    assert_eq!(stale["latest_version"], newer);
    assert_eq!(stale["latest_version_stale"], true);
    assert_eq!(stale["reason"], QUERY_FAILURE);
    assert_eq!(stale, status(fixture));
    read_only();

    begin().send(Ok(release(version, true))).unwrap();
    let current = wait_for(fixture, "up_to_date");
    assert_eq!(current["latest_version"], version);
    assert_eq!(current["latest_version_stale"], false);
    assert_eq!(current["reason"], Value::Null);

    begin().send(Ok(release("0.0.1", true))).unwrap();
    assert_eq!(wait_for(fixture, "local_newer")["latest_version"], "0.0.1");
    begin().send(Ok(release(newer, false))).unwrap();
    let incompatible = wait_for(fixture, "incompatible");
    assert_eq!(incompatible["latest_version"], newer);
    assert_eq!(incompatible["reason"], "此版本缺少 macOS arm64 更新包。");
    begin().send(Ok(release(newer, true))).unwrap();
    wait_for(fixture, "update_available");
    fixture.owner.verify_update_progress_views(newer);
    read_only();
    assert_eq!(fs::read(&marker).unwrap(), b"old fixture bundle");
    assert_eq!(fs::read_dir(&installed).unwrap().count(), 1);
    assert_eq!(
        fs::read_dir(folder.join("Applications")).unwrap().count(),
        1
    );
    assert!(!fixture.owner.ivars().quitting.get());
    assert!(fixture.owner.ivars().update_commit.borrow().is_none());
    println!(
        "PASS production update IPC: versions, explicit lookup, duplicate busy, cached stale after refresh failure, no update, local newer, incompatible release, read-only status and untouched fixture bundle"
    );
}
