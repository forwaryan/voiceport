//! Real AppKit timer, local socket and CLI with a synthetic input and ASR worker.
use super::*;
use crate::{
    audio::{devices::fixture::Backend, inline_open_fixture},
    hotkey::{HotKeyEvent, KeyPress, Stroke},
    local_models::{CHATGPT, Catalog},
    shortcut::ShortcutMode,
    status_overlay::{CaptureStage, Stage, Visibility},
    storage::{NAMESPACE, Storage},
};
use objc2_foundation::NSDate;

mod performance;
pub use performance::isolated_performance_baseline;

pub(super) fn pump() {
    let end = NSDate::dateWithTimeIntervalSinceNow(0.012);
    NSRunLoop::currentRunLoop().runUntilDate(&end);
}

fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + LIMIT;
    while !predicate() {
        assert!(Instant::now() < deadline, "audio fixture deadline elapsed");
        pump();
    }
}

fn phase(fixture: &Fixture<'_>, expected: Phase) {
    until(|| {
        let mut matched = false;
        fixture
            .owner
            .with_controller(|c| matched = c.state.phase == expected);
        matched
    });
}

fn released(fixture: &Fixture<'_>) {
    until(|| {
        let mut idle = false;
        fixture
            .owner
            .with_controller(|c| idle = !c.audio_input_busy());
        idle
    });
}

fn sampled(fixture: &Fixture<'_>) {
    until(|| {
        let mut sampled = false;
        fixture
            .owner
            .with_controller(|c| sampled = c.capture_level().is_some());
        sampled
    });
}

fn cli_start(fixture: &Fixture<'_>) -> String {
    let reply = fixture.call(&["dictation", "start", "--source", CHATGPT], 0);
    assert_eq!(reply["data"]["state"], "permissions");
    assert_eq!(reply["data"]["delivery"], "none");
    reply["data"]["operation"].as_str().unwrap().into()
}

fn baseline_comparison(fixture: &Fixture<'_>, backend: &Backend) {
    let opens = backend.counts().0;
    let resume = backend.block_next_open();
    let socket = fixture.socket.clone();
    let observed = backend.clone();
    let client = thread::spawn(move || {
        let deadline = Instant::now() + LIMIT;
        while observed.counts().0 == opens {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        let started = Instant::now();
        let result = voiceport_control::transport::call(&socket, Action::Status);
        let duration = started.elapsed();
        resume.send(true).unwrap();
        (result, duration)
    });
    let completed = std::rc::Rc::new(Cell::new(false));
    let called = completed.clone();
    let input = backend.clone();
    let block = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
        inline_open_fixture(input.clone());
        called.set(true);
    });
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.01, false, &block) };
    until(|| completed.get());
    timer.invalidate();
    let (reply, duration) = client.join().unwrap();
    assert_eq!(reply.unwrap_err().error.unwrap().code, "timeout");
    assert!(duration >= Duration::from_secs(4));
    println!("audio-start inline comparison: status timed out at {duration:?}");
    fixture.call(&["status"], 0);
}

fn quit_during_release(cli: &Path) {
    let folder = tempfile::tempdir_in("/tmp").unwrap();
    let root = folder.path().canonicalize().unwrap();
    let storage = Storage::at(root.join(NAMESPACE));
    let other = storage.create_recording().unwrap();
    let payload = b"other synthetic recording";
    std::fs::write(other.path(), payload).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg(cli)
        .arg("--audio-start-quit-child")
        .arg(&root)
        .spawn()
        .unwrap();
    let started = Instant::now();
    let exit = loop {
        if let Some(exit) = child.try_wait().unwrap() {
            break exit;
        }
        if started.elapsed() >= LIMIT {
            let _ = child.kill();
            child.wait().unwrap();
            panic!("AppKit Quit child exceeded {LIMIT:?}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert!(exit.success(), "AppKit Quit child: {exit}");
    let remaining = std::fs::read_dir(storage.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        remaining,
        [
            other
                .path()
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_owned(),
            "owner.json".into()
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(std::fs::read(other.path()).unwrap(), payload);
    assert!(other.path().with_file_name("session.json").is_file());
    assert!(other.path().with_file_name("lease").is_file());
    assert!(!root.join("ipc/control.sock").exists());
    println!(
        "audio-start Quit parent: child exit={exit}, elapsed={:?}, disk before cleanup={remaining:?}",
        started.elapsed()
    );
    drop(other);
    folder.close().unwrap();
    assert!(!root.exists());
    println!("audio-start Quit parent: synthetic root removed after readback");
}

pub fn isolated_audio_start_regression(
    mtm: MainThreadMarker,
    cli: &Path,
    quit_child_root: Option<&Path>,
) {
    let app = NSApplication::sharedApplication(mtm);
    let folder = quit_child_root.is_none().then(|| {
        quit_during_release(cli);
        tempfile::tempdir_in("/tmp").unwrap()
    });
    let root = quit_child_root
        .unwrap_or_else(|| folder.as_ref().unwrap().path())
        .canonicalize()
        .unwrap();
    let socket = root.join("ipc/control.sock");
    let storage = Storage::at(root.join(NAMESPACE));
    let backend = Backend::new(storage.clone());
    let owner = Delegate::new(mtm);
    let (updates, _) = std::sync::mpsc::channel();
    owner.configure_update_fixture(root.clone(), root.join("unused.app"), updates);
    owner.ivars().audio_fixture_stroke.set(Some(None));
    let mut controller = Controller::new();
    controller.automatic = false;
    controller.source_id = CHATGPT.into();
    controller.catalog = Catalog {
        entries: vec![Entry {
            id: CHATGPT.into(),
            title: "Synthetic source".into(),
            detail: String::new(),
            source: Ok(SessionSource::ChatGpt),
        }],
    };
    controller.transcript = "Previous result".into();
    let asr = controller.install_audio_fixture(backend.clone());
    owner.ivars().controller.replace(Some(controller));
    owner.ivars().control.borrow_mut().server = Some(Server::bind(&socket).unwrap());
    owner.install_timer();
    let fixture = Fixture { cli, socket, owner };
    if quit_child_root.is_some() {
        let operation = cli_start(&fixture);
        phase(&fixture, Phase::Recording);
        sampled(&fixture);
        let before = backend.counts();
        let (_keep_blocked, entered) = backend.block_next_release();
        let stopped = fixture.call(&["dictation", "stop", &operation], 0);
        assert_eq!(stopped["data"]["state"], "transcribing");
        until(|| entered.try_recv().is_ok());
        assert_eq!(backend.counts().3, before.3);
        let status_started = Instant::now();
        let status = fixture.call(&["status"], 0);
        let status_latency = status_started.elapsed();
        assert!(status_latency < Duration::from_secs(2));
        assert_eq!(status["data"]["audio_input_busy"], true);
        assert_eq!(status["data"]["state"], "transcribing");
        assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 3);
        asr.assert_no_job();
        println!(
            "audio-start Quit child: Stop succeeded, release blocked, IPC={status_latency:?}; requesting AppKit termination"
        );
        app.setDelegate(Some(ProtocolObject::from_ref(&*fixture.owner)));
        app.terminate(None);
        panic!("AppKit Quit did not exit the fixture");
    }
    baseline_comparison(&fixture, &backend);

    // Same blocked initialization, with actual CLI requests through Delegate::tick.
    let first = backend.counts();
    let resume = backend.block_next_open();
    let operation = cli_start(&fixture);
    until(|| backend.counts().0 > first.0);
    let started = Instant::now();
    let status = fixture.call(&["status"], 0);
    let duration = started.elapsed();
    assert!(duration < Duration::from_secs(2));
    assert_eq!(status["data"]["state"], "permissions");
    assert_eq!(status["data"]["opening_microphone"], true);
    assert_eq!(status["data"]["audio_input_busy"], true);
    assert_eq!(
        fixture.call(&["microphone", "status"], 0)["data"]["active_route"],
        Value::Null
    );
    assert_eq!(
        fixture.call(&["microphone", "list"], 4)["error"]["code"],
        "busy"
    );
    for _ in 0..3 {
        assert_eq!(
            fixture.call(&["dictation", "start", "--source", CHATGPT], 4)["error"]["code"],
            "busy"
        );
    }
    assert_eq!(backend.counts().0, first.0 + 1);
    assert_eq!(
        fixture.call(&["dictation", "stop", &operation], 0)["data"]["state"],
        "idle"
    );
    assert_eq!(
        fixture.call(&["dictation", "result", &operation], 130)["error"]["code"],
        "cancelled"
    );
    assert_eq!(
        fixture.call(&["dictation", "start", "--source", CHATGPT], 4)["error"]["code"],
        "busy"
    );
    resume.send(true).unwrap();
    released(&fixture);
    assert_eq!(backend.counts(), (first.0 + 1, first.1, first.2, first.3));
    fixture
        .owner
        .with_controller(|c| assert_eq!(c.transcript, "Previous result"));
    assert!(!storage.path().exists());
    asr.assert_no_job();
    println!(
        "audio-start asynchronous: status replied in {duration:?}; repeat, stop and late return checked"
    );

    // After Prepared was accepted, pause in the last default-route query.
    // Both cancellation and expiry must revoke the already-created cache.
    for cancel in [true, false] {
        let first = backend.counts();
        let resume_open = backend.block_next_open();
        let operation = cli_start(&fixture);
        until(|| backend.counts().0 == first.0 + 1);
        let (resume_query, entered) = backend.block_next_default_query();
        resume_open.send(true).unwrap();
        until(|| entered.try_recv().is_ok());
        assert_eq!(backend.counts().2, first.2);
        assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 2);
        let started = Instant::now();
        let status = fixture.call(&["status"], 0);
        let latency = started.elapsed();
        assert!(latency < Duration::from_secs(2));
        assert_eq!(status["data"]["opening_microphone"], true);
        let (expected, stage) = if cancel {
            assert_eq!(
                fixture.call(&["dictation", "stop", &operation], 0)["data"]["state"],
                "idle"
            );
            assert_eq!(
                fixture.call(&["dictation", "result", &operation], 130)["error"]["code"],
                "cancelled"
            );
            (Phase::Idle, "cancel")
        } else {
            phase(&fixture, Phase::Failed);
            assert_eq!(
                fixture.call(&["dictation", "result", &operation], 1)["data"]["retry_available"],
                false
            );
            (Phase::Failed, "timeout")
        };
        assert_eq!(
            fixture.call(&["microphone", "list"], 4)["error"]["code"],
            "busy"
        );
        assert_eq!(
            fixture.call(&["dictation", "start", "--source", CHATGPT], 4)["error"]["code"],
            "busy"
        );
        assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 1);
        resume_query.send(()).unwrap();
        released(&fixture);
        phase(&fixture, expected);
        assert_eq!(
            backend.counts(),
            (first.0 + 1, first.1 + 1, first.2, first.3 + 1)
        );
        assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 1);
        fixture
            .owner
            .with_controller(|c| assert_eq!(c.transcript, "Previous result"));
        asr.assert_no_job();
        println!(
            "audio-start post-commit {stage}: IPC={latency:?}, no late attachment, cache or ASR"
        );
    }

    // Actual Hold edge routing, with a synthetic HUD/HID sample and no registration.
    let owner = &fixture.owner;
    owner.ivars().shortcut_mode.set(ShortcutMode::Hold);
    let (edges, incoming) = std::sync::mpsc::channel();
    owner.ivars().hotkey_rx.replace(Some(incoming));
    let stroke = Stroke::fixture_stroke(1);
    owner.ivars().audio_fixture_stroke.set(Some(Some(stroke)));
    let first = backend.counts();
    let resume = backend.block_next_open();
    edges
        .send(HotKeyEvent::Press(KeyPress {
            stroke,
            at: Instant::now(),
            application: None,
            target: None,
        }))
        .unwrap();
    until(|| backend.counts().0 > first.0);
    let mut visibility = Visibility::default();
    let mut opening = None;
    owner.with_controller(|c| {
        let snapshot = c.overlay_snapshot();
        assert_eq!(snapshot.stage, Some(Stage::Capture(CaptureStage::Opening)));
        assert_eq!(visibility.observe(snapshot, true), snapshot.stage);
        assert_eq!(c.audio_release_notice(), None);
        opening = Some(snapshot);
    });
    owner.ivars().audio_fixture_stroke.set(Some(None));
    edges.send(HotKeyEvent::Release(stroke)).unwrap();
    phase(&fixture, Phase::Idle);
    owner.with_controller(|c| {
        assert_eq!(visibility.observe(c.overlay_snapshot(), true), None);
        assert_eq!(visibility.observe(opening.unwrap(), true), None);
        assert!(c.audio_release_notice().is_some());
    });
    owner.toggle_action(sel!(toggle:), None);
    assert_eq!(backend.counts().0, first.0 + 1);
    resume.send(true).unwrap();
    released(&fixture);
    owner.with_controller(|c| {
        assert_eq!(c.audio_release_notice(), None);
        assert_eq!(c.overlay_snapshot().stage, None);
        assert_eq!(c.transcript, "Previous result");
    });
    assert_eq!(backend.counts(), (first.0 + 1, first.1, first.2, first.3));
    asr.assert_no_job();
    owner.ivars().shortcut_mode.set(ShortcutMode::Toggle);
    println!("audio-start Hold: key-up cancelled during initialization with zero attachment");

    let first = backend.counts();
    let resume = backend.block_next_open();
    let toggle = || {
        HotKeyEvent::Press(KeyPress {
            stroke,
            at: Instant::now(),
            application: None,
            target: None,
        })
    };
    edges.send(toggle()).unwrap();
    until(|| backend.counts().0 > first.0);
    edges.send(toggle()).unwrap();
    phase(&fixture, Phase::Idle);
    resume.send(true).unwrap();
    released(&fixture);
    assert_eq!(backend.counts(), (first.0 + 1, first.1, first.2, first.3));
    asr.assert_no_job();
    println!("audio-start Toggle: second shortcut cancelled during initialization");

    // Panel toggle and actual startup deadline, including an occupied lane after expiry.
    let first = backend.counts();
    let resume = backend.block_next_open();
    owner.toggle_action(sel!(toggle:), None);
    until(|| backend.counts().0 > first.0);
    phase(&fixture, Phase::Failed);
    owner.with_controller(|c| {
        assert!(c.message.contains("等待麦克风超时"));
        assert!(c.audio_input_busy());
        assert_eq!(c.transcript, "Previous result");
    });
    resume.send(true).unwrap();
    released(&fixture);
    assert_eq!(backend.counts().2, first.2);
    asr.assert_no_job();

    // Run the GUI start through recovery and stop, advancing the same overlay
    // projection used by Delegate::refresh without opening a desktop window.
    let mut stage = |expected| {
        let mut snapshot = None;
        until(|| {
            owner.with_controller(|c| snapshot = Some(c.overlay_snapshot()));
            snapshot.unwrap().stage == expected
        });
        let snapshot = snapshot.unwrap();
        assert_eq!(visibility.observe(snapshot, true), expected);
        snapshot.generation
    };
    let resume = backend.block_next_open();
    owner.toggle_action(sel!(toggle:), None);
    let generation = stage(Some(Stage::Capture(CaptureStage::Opening)));
    resume.send(true).unwrap();
    assert_eq!(
        stage(Some(Stage::Capture(CaptureStage::Recording))),
        generation
    );
    sampled(&fixture);
    let resume = backend.block_next_open();
    backend.set_default(2);
    backend.disconnect(1);
    assert_eq!(
        stage(Some(Stage::Capture(CaptureStage::Recovering))),
        generation
    );
    resume.send(true).unwrap();
    assert_eq!(
        stage(Some(Stage::Capture(CaptureStage::Resumed))),
        generation
    );
    let (resume, entered) = backend.block_next_release();
    owner.toggle_action(sel!(toggle:), None);
    assert_eq!(
        stage(Some(Stage::Capture(CaptureStage::Finalizing))),
        generation
    );
    until(|| entered.try_recv().is_ok());
    asr.assert_no_job();
    resume.send(true).unwrap();
    assert_eq!(stage(Some(Stage::Transcribing)), generation);
    until(|| asr.complete(generation));
    phase(&fixture, Phase::Ready);
    assert_eq!(stage(None), generation);
    released(&fixture);
    backend.disconnect(0);
    backend.set_default(1);
    println!(
        "audio-start GUI overlay: opening, recovery, finalization and transcribing share one generation"
    );

    // Double-check the permission and device context after a delayed open.
    for permission_changes in [true, false] {
        let first = backend.counts();
        let resume = backend.block_next_open();
        let operation = cli_start(&fixture);
        until(|| backend.counts().0 > first.0);
        if permission_changes {
            backend.set_authorized(false);
        } else {
            backend.set_default(2);
        }
        resume.send(true).unwrap();
        phase(&fixture, Phase::Failed);
        released(&fixture);
        assert_eq!(backend.counts().2, first.2);
        assert_eq!(
            fixture.call(&["dictation", "result", &operation], 1)["data"]["retry_available"],
            false
        );
        backend.set_authorized(true);
        backend.set_default(1);
    }
    for after_final_id in [false, true] {
        let first = backend.counts();
        let resume = backend.block_next_open();
        let operation = cli_start(&fixture);
        until(|| backend.counts().0 == first.0 + 1);
        let (permit, entered) = if after_final_id {
            backend.block_next_final_default_id()
        } else {
            backend.block_next_default_query()
        };
        resume.send(true).unwrap();
        until(|| entered.try_recv().is_ok());
        assert_eq!(backend.counts().1, first.1 + 1);
        assert_eq!(backend.counts().2, first.2);
        backend.set_default(2);
        if after_final_id {
            // The last ID read has already captured 1. Only the notification
            // can reject it, even if the default has already returned to 1.
            assert!(backend.notify_default_changed());
            backend.set_default(1);
        }
        let status = fixture.call(&["microphone", "status"], 0);
        assert_eq!(status["data"]["permission"], "authorized");
        assert_eq!(status["data"]["active_route"], Value::Null);
        permit.send(()).unwrap();
        phase(&fixture, Phase::Failed);
        released(&fixture);
        assert_eq!(backend.counts().2, first.2);
        assert_eq!(
            fixture.call(&["dictation", "result", &operation], 1)["data"]["retry_available"],
            false
        );
        assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 1);
        asr.assert_no_job();
        backend.set_default(1);
        println!(
            "audio-start default snapshot: after_final_id={after_final_id}, zero attachments and zero ASR jobs"
        );
    }
    asr.assert_no_job();
    let fail = backend.block_next_open();
    let operation = cli_start(&fixture);
    fail.send(false).unwrap();
    phase(&fixture, Phase::Failed);
    released(&fixture);
    asr.assert_no_job();
    assert_eq!(
        fixture.call(&["dictation", "result", &operation], 1)["data"]["retry_available"],
        false
    );
    println!(
        "audio-start failure: startup timeout, revoked permission, changed default and native-open failure checked"
    );

    // Production route selection, fallback and format conversion, plus real AAC finalization.
    assert_eq!(
        fixture.call(&["microphone", "list"], 0)["data"]["devices"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    owner.with_controller(|c| c.microphone_uid = Some("fixture-1".into()));
    let before = backend.counts();
    let resume = backend.block_next_open();
    let operation = cli_start(&fixture);
    until(|| backend.counts().0 == before.0 + 1);
    backend.set_default(2);
    assert!(!backend.notify_default_changed());
    resume.send(true).unwrap();
    phase(&fixture, Phase::Recording);
    let status = fixture.call(&["microphone", "status"], 0);
    assert!(
        status["data"]["active_route"]
            .as_str()
            .is_some_and(|route| route.contains("Synthetic input 1"))
    );
    sampled(&fixture);
    backend.disconnect(1);
    until(|| {
        let mut recovered = false;
        owner.with_controller(|c| {
            recovered = c
                .microphone_route()
                .is_some_and(|r| r.contains("已恢复录音") && r.contains("Synthetic input 2"))
        });
        recovered
    });
    // A default change delivered after attachment leaves a healthy input active.
    backend.disconnect(0);
    backend.set_default(1);
    assert!(backend.notify_default_changed());
    let status = fixture.call(&["microphone", "status"], 0);
    assert!(
        status["data"]["active_route"]
            .as_str()
            .is_some_and(|route| route.contains("Synthetic input 2"))
    );
    fixture.call(&["dictation", "stop", &operation], 0);
    released(&fixture);
    let mut generation = 0;
    owner.with_controller(|c| generation = c.state.generation());
    until(|| {
        owner.with_controller(|c| assert_ne!(c.state.phase, Phase::Failed, "{}", c.message));
        asr.complete(generation)
    });
    phase(&fixture, Phase::Ready);
    assert_eq!(
        fixture.call(&["dictation", "result", &operation], 0)["data"]["text"],
        "Synthetic capture result"
    );
    asr.assert_no_job();
    println!(
        "audio-start success: preferred input, different-rate fallback/recovery and one finalized AAC ASR submission checked"
    );

    owner.with_controller(|c| c.microphone_uid = None);
    for (changed, revoke_permission, during_route_check, after_final_id) in [
        ("permission during open", true, false, false),
        ("default input during open", false, false, false),
        ("permission during route verification", true, true, false),
        ("default input after ID before details", false, true, false),
        ("default notification after final ID", false, true, true),
    ] {
        backend.disconnect(0);
        backend.set_default(1);
        let operation = cli_start(&fixture);
        phase(&fixture, Phase::Recording);
        sampled(&fixture);
        let before = backend.counts();
        let resume = backend.block_next_open();
        backend.set_default(2);
        backend.disconnect(1);
        until(|| backend.counts().0 == before.0 + 1);
        let resume_query = if during_route_check {
            let (resume_query, entered) = if after_final_id {
                backend.block_next_final_default_id()
            } else {
                backend.block_next_default_query()
            };
            resume.send(true).unwrap();
            until(|| entered.try_recv().is_ok());
            assert_eq!(backend.counts().1, before.1 + 1);
            Some(resume_query)
        } else {
            None
        };
        if revoke_permission {
            backend.set_authorized(false);
        } else {
            // Both inputs are valid. The opened replacement (2) is stale.
            backend.disconnect(0);
            backend.set_default(1);
            if after_final_id {
                assert!(backend.notify_default_changed());
            }
        }
        let status = fixture.call(&["microphone", "status"], 0);
        let expected_permission = if revoke_permission {
            "denied"
        } else {
            "authorized"
        };
        assert_eq!(status["data"]["permission"], expected_permission);
        assert_eq!(status["data"]["active_route"], Value::Null);
        match resume_query {
            Some(resume_query) => resume_query.send(()).unwrap(),
            None => resume.send(true).unwrap(),
        }
        phase(&fixture, Phase::Failed);
        released(&fixture);
        assert_eq!(backend.counts().2, before.2);
        let status = fixture.call(&["microphone", "status"], 0);
        assert_eq!(status["data"]["active_route"], Value::Null);
        let result = fixture.call(&["dictation", "result", &operation], 1);
        assert_eq!(result["data"]["retry_available"], false);
        fixture.owner.with_controller(|c| {
            let reason = if revoke_permission {
                "麦克风权限"
            } else {
                "麦克风已变化"
            };
            assert!(c.message.contains(reason), "{}", c.message);
        });
        assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 1);
        asr.assert_no_job();
        backend.set_authorized(true);
        println!(
            "audio-start recovery: changed {changed}, zero replacement attachments and zero ASR jobs"
        );
    }

    let operation = cli_start(&fixture);
    phase(&fixture, Phase::Recording);
    let (resume, entered) = backend.block_next_release();
    fixture.call(&["dictation", "stop", &operation], 0);
    until(|| entered.try_recv().is_ok());
    assert_eq!(
        fixture.call(&["status"], 0)["data"]["state"],
        "transcribing"
    );
    phase(&fixture, Phase::Failed);
    owner.with_controller(|c| assert!(c.message.contains("等待麦克风停止超时")));
    assert_eq!(
        fixture.call(&["dictation", "result", &operation], 1)["data"]["retry_available"],
        false
    );
    assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 1);
    resume.send(true).unwrap();
    released(&fixture);
    assert_eq!(std::fs::read_dir(storage.path()).unwrap().count(), 1);
    asr.assert_no_job();
    println!(
        "audio-start stop: stalled release kept IPC responsive; cache revoked before release and no late finalization"
    );

    backend.disconnect(0);
    backend.set_default(1);
    let operation = cli_start(&fixture);
    phase(&fixture, Phase::Recording);
    let first = backend.counts();
    let resume = backend.block_next_open();
    backend.set_default(2);
    backend.disconnect(1);
    until(|| backend.counts().0 > first.0);
    assert_eq!(fixture.call(&["status"], 0)["data"]["state"], "recording");
    assert_eq!(
        fixture.call(&["microphone", "status"], 0)["data"]["active_route"],
        Value::Null
    );
    phase(&fixture, Phase::Failed);
    owner.with_controller(|c| assert!(c.message.contains("等待麦克风恢复超时")));
    resume.send(true).unwrap();
    released(&fixture);
    assert_eq!(backend.counts().2, first.2);
    assert_eq!(
        fixture.call(&["dictation", "result", &operation], 1)["data"]["retry_available"],
        false
    );
    asr.assert_no_job();
    println!("audio-start recovery timeout: no late replacement attachment or ASR");

    let cache_count = || std::fs::read_dir(storage.path()).unwrap().count();
    let prepare = || {
        backend.set_authorized(true);
        backend.disconnect(0);
        backend.set_default(1);
    };
    for (frames, amplitude, revoke_permission) in [
        (0, 0., false),
        (1024, 0., false),
        (1024, 0.0001, false),
        (1024, 0.125, true),
    ] {
        prepare();
        backend.set_samples(frames, amplitude);
        let operation = cli_start(&fixture);
        phase(&fixture, Phase::Recording);
        if frames > 0 {
            sampled(&fixture);
        }
        if revoke_permission {
            backend.set_authorized(false);
        }
        backend.disconnect(1);
        // No replacement device is available. Zero/low signal or permission loss
        // must not leave a usable offer even though capture had been admitted.
        phase(&fixture, Phase::Failed);
        released(&fixture);
        let result = fixture.call(&["dictation", "result", &operation], 1);
        assert_eq!(result["data"]["retry_available"], false);
        assert_eq!(result["data"]["fragment_available"], false);
        assert_eq!(cache_count(), 1);
        asr.assert_no_job();
    }
    backend.set_samples(1024, 0.125);

    prepare();
    let start = fixture.call(
        &[
            "dictation",
            "start",
            "--source",
            CHATGPT,
            "--language",
            "fr",
        ],
        0,
    );
    let operation = start["data"]["operation"].as_str().unwrap();
    phase(&fixture, Phase::Recording);
    sampled(&fixture);
    let before = backend.counts();
    let fail = backend.block_next_open();
    backend.set_default(2);
    backend.disconnect(1);
    until(|| backend.counts().0 == before.0 + 1);
    fail.send(false).unwrap();
    phase(&fixture, Phase::Failed);
    released(&fixture);
    assert_eq!(backend.counts().2, before.2);
    assert_eq!(cache_count(), 2);
    asr.assert_no_job();
    let result = fixture.call(&["dictation", "result", operation], 1);
    assert_eq!(result["data"]["fragment_available"], true);
    assert_eq!(result["data"]["retry_available"], false);
    let offer = fixture.call(&["dictation", "fragment", operation], 0);
    let data = &offer["data"];
    let interrupted_generation = data["generation"].as_u64().unwrap();
    let token = data["confirmation"].as_str().unwrap();
    assert_eq!(data["source"], CHATGPT);
    assert_eq!(data["language"], "fr");
    assert_eq!(data["destination"]["kind"], "chatgpt");
    assert_eq!(data["requires_accept_upload"], true);
    fixture.call(&["dictation", "retry", operation], 1);
    fixture.call(
        &[
            "dictation",
            "transcribe-fragment",
            operation,
            "--confirm",
            "stale",
            "--accept-upload",
        ],
        1,
    );
    let unconfirmed = fixture.call(
        &[
            "dictation",
            "transcribe-fragment",
            operation,
            "--confirm",
            token,
        ],
        5,
    );
    assert_eq!(unconfirmed["error"]["code"], "confirmation_required");
    asr.assert_no_job();

    let accepted = fixture.call(
        &[
            "dictation",
            "transcribe-fragment",
            operation,
            "--confirm",
            token,
            "--accept-upload",
        ],
        0,
    );
    assert_eq!(accepted["data"]["incomplete"], true);
    let attempt = accepted["data"]["operation"].as_str().unwrap();
    assert_ne!(operation, attempt);
    fixture.call(&["dictation", "fragment", operation], 6);
    let mut generation = 0;
    owner.with_controller(|c| generation = c.state.generation());
    asr.stale_reply(interrupted_generation);
    pump();
    phase(&fixture, Phase::Transcribing);
    until(|| asr.complete_checked(generation, true, "fr"));
    phase(&fixture, Phase::Ready);
    let result = fixture.call(&["dictation", "result", attempt], 0);
    assert_eq!(result["data"]["incomplete"], true);
    assert_eq!(result["data"]["delivery"], "none");
    owner.with_controller(|c| {
        assert!(matches!(c.visible_transcript, Some(crate::controller::VisibleTranscript::Incomplete(g)) if g == generation));
        let response = c.repaste_latest_with(|_| panic!("incomplete result attempted to capture a paste target"));
        assert_eq!(response.error.unwrap().code, "invalid_state");
        let copy = c.copy_latest_with(|text| text == "Synthetic capture result");
        assert_eq!(copy.data["incomplete"], true);
    });
    assert_eq!(cache_count(), 1);

    // A healthy later dictation accepts its own Stop and yields a complete result.
    prepare();
    let next = cli_start(&fixture);
    phase(&fixture, Phase::Recording);
    sampled(&fixture);
    fixture.call(&["dictation", "stop", &next], 0);
    released(&fixture);
    owner.with_controller(|c| generation = c.state.generation());
    until(|| asr.complete(generation));
    phase(&fixture, Phase::Ready);
    let result = fixture.call(&["dictation", "result", &next], 0);
    assert_eq!(result["data"]["incomplete"], false);
    fixture.call(
        &[
            "dictation",
            "transcribe-fragment",
            &next,
            "--confirm",
            token,
            "--accept-upload",
        ],
        1,
    );
    assert_eq!(cache_count(), 1);

    // Stop or Cancel can race a second disconnect's native teardown. A Stop
    // settles the one verified lease and a Cancel invalidates the pending reply.
    for cancel in [false, true] {
        prepare();
        let operation = cli_start(&fixture);
        phase(&fixture, Phase::Recording);
        sampled(&fixture);
        backend.set_default(2);
        backend.disconnect(1);
        until(|| {
            let mut recovered = false;
            owner.with_controller(|c| {
                recovered = c
                    .microphone_route()
                    .is_some_and(|route| route.contains("已恢复录音"))
            });
            recovered
        });
        let opens = backend.counts().0;
        let (resume, entered) = backend.block_next_release();
        backend.disconnect(2);
        until(|| entered.try_recv().is_ok());
        fixture.call(
            &[
                "dictation",
                if cancel { "cancel" } else { "stop" },
                &operation,
            ],
            0,
        );
        resume.send(true).unwrap();
        released(&fixture);
        phase(&fixture, if cancel { Phase::Idle } else { Phase::Failed });
        assert_eq!(backend.counts().0, opens);
        asr.assert_no_job();
        if cancel {
            fixture.call(&["dictation", "fragment", &operation], 1);
        } else {
            let confirmation = fixture.call(&["dictation", "fragment", &operation], 0);
            assert_eq!(confirmation["data"]["incomplete"], true);
            fixture.call(&["dictation", "cancel", &operation], 0);
            fixture.call(&["dictation", "result", &operation], 130);
            fixture.call(&["dictation", "fragment", &operation], 1);
        }
        assert_eq!(cache_count(), 1);
    }
    println!(
        "audio-start fragments: signal and permission admission, explicit consent, stale reply, Stop/Cancel and cleanup checked"
    );

    prepare();

    let first = backend.counts();
    let _keep_init_blocked_until_process_exit = backend.block_next_open();
    cli_start(&fixture);
    until(|| backend.counts().0 > first.0);
    assert_eq!(
        owner.should_terminate(sel!(applicationShouldTerminate:), &app),
        NSApplicationTerminateReply::TerminateNow
    );
    owner.ivars().timer.get().unwrap().invalidate();
    owner.with_controller(|c| assert!(!c.busy() && c.audio_input_busy()));
    asr.assert_no_job();
    assert_eq!(backend.counts().2, first.2);
    folder.unwrap().close().unwrap();
    assert!(!root.exists());
    println!(
        "audio-start shutdown: TerminateNow and isolated storage removed while initialization was still blocked"
    );
    app.setDelegate(Some(ProtocolObject::from_ref(&**owner)));
    app.terminate(None);
    panic!("fixture process should exit while the native call is blocked");
}
