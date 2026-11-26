use super::*;
use crate::{
    audio::devices::fixture::Backend,
    local_models::SessionSource,
    storage::{NAMESPACE, Storage},
};

fn rig() -> (
    tempfile::TempDir,
    Controller,
    Backend,
    crate::controller::AudioFixtureAsr,
) {
    let root = tempfile::tempdir_in("/tmp").unwrap();
    let backend = Backend::new(Storage::at(
        root.path().canonicalize().unwrap().join(NAMESPACE),
    ));
    let mut controller = Controller::new();
    let asr = controller.install_audio_fixture(backend.clone());
    (root, controller, backend, asr)
}

fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "synthetic audio did not reach expected state"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn read(catalog: &mut Catalog, c: &Controller) {
    catalog.request();
    until(|| {
        catalog.poll(!c.audio_input_busy(), || c.input_device_snapshot());
        !catalog.requested() && !c.audio_input_busy()
    });
}

fn activity(c: &Controller) -> Activity {
    Activity::read(c, false, c.microphone_route().is_some())
}

#[test]
fn lists_saved_choice_and_system_snapshot_independently_then_expires_cached_rows() {
    let (_root, c, backend, asr) = rig();
    let mut catalog = Catalog::default();
    read(&mut catalog, &c);
    let now = Instant::now();
    let (rows, selected) = catalog.choices(Some("fixture-2"), now);
    assert!(matches!(&rows[selected], Choice::Device(device) if device.uid == "fixture-2"));
    assert_eq!(catalog.choices(None, now).1, 0);
    assert!(catalog.system(now).contains("Synthetic input 1"));
    assert!(catalog.permission(now).contains("已允许"));
    assert_eq!(activity(&c), Activity::Idle);
    assert_eq!(backend.counts(), (0, 0, 0, 0));

    let stale = now + SNAPSHOT_AGE;
    let (rows, selected) = catalog.choices(Some("fixture-2"), stale);
    assert_eq!(rows.len(), 2);
    assert!(matches!(&rows[selected], Choice::Retained(Some(device)) if device.uid == "fixture-2"));
    assert!(!catalog.can_choose("fixture-2", stale));
    assert!(catalog.system(stale).contains("旧快照"));
    assert!(catalog.permission(stale).contains("旧快照"));

    backend.disconnect(2);
    backend.set_authorized(false);
    read(&mut catalog, &c);
    let now = Instant::now();
    let (rows, selected) = catalog.choices(Some("fixture-2"), now);
    assert_eq!(rows[selected], Choice::Retained(None));
    assert!(catalog.permission(now).contains("已拒绝"));
    assert!(!catalog.can_choose("fixture-2", now));
    assert_eq!(backend.counts(), (0, 0, 0, 0));
    asr.assert_no_job();
}

#[test]
fn changing_default_and_hidden_query_keep_preview_separate_from_admission() {
    let (_root, c, backend, _) = rig();
    let mut catalog = Catalog::default();
    let (resume, entered) = backend.block_next_default_query();
    catalog.request();
    catalog.poll(true, || c.input_device_snapshot());
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(
        Activity::read(&c, catalog.reading(), false),
        Activity::Reading
    );
    assert!(c.read_microphones().is_err());
    backend.set_default(2);
    resume.send(()).unwrap();
    until(|| {
        catalog.poll(!c.audio_input_busy(), || {
            unreachable!("query already started")
        });
        !catalog.requested() && !c.audio_input_busy()
    });
    let now = Instant::now();
    assert!(catalog.system(now).contains("默认输入已变化"));
    assert!(catalog.can_choose("fixture-2", now));
    assert!(catalog.permission(now).contains("已允许"));
    assert_eq!(backend.counts(), (0, 0, 0, 0));

    let (resume, entered) = backend.block_next_default_query();
    catalog.request();
    catalog.poll(true, || c.input_device_snapshot());
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    catalog.suspend();
    assert_eq!(activity(&c), Activity::Releasing);
    catalog.poll(false, || panic!("busy lane must not issue another query"));
    assert!(!catalog.can_choose("fixture-2", Instant::now()));
    resume.send(()).unwrap();
    read(&mut catalog, &c);
    assert!(catalog.system(Instant::now()).contains("Synthetic input 2"));
    assert_eq!(backend.counts(), (0, 0, 0, 0));
}

#[test]
fn reports_open_recovery_and_release_only_from_controller_receipts() {
    let (root, mut c, backend, asr) = rig();
    let mut catalog = Catalog::default();
    read(&mut catalog, &c);
    assert!(catalog.system(Instant::now()).contains("Synthetic input 1"));
    c.microphone_uid = Some("fixture-1".into());
    let resume = backend.block_next_open();
    assert!(
        c.start_pinned(SessionSource::ChatGpt, String::new(), false)
            .is_some()
    );
    until(|| {
        c.poll();
        backend.counts().0 == 1
    });
    assert_eq!(activity(&c), Activity::Opening);
    assert!(c.microphone_route().is_none());
    resume.send(true).unwrap();
    until(|| {
        c.poll();
        activity(&c) == Activity::Recording
    });
    assert!(c.microphone_route().unwrap().contains("Synthetic input 1"));

    let resume = backend.block_next_open();
    backend.set_default(2);
    backend.disconnect(1);
    until(|| {
        c.poll();
        backend.counts().0 == 2 && activity(&c) == Activity::Recovering
    });
    assert!(c.microphone_route().is_none());
    assert!(Activity::Recovering.actual(None).contains("等待替代设备"));
    resume.send(true).unwrap();
    until(|| {
        c.poll();
        activity(&c) == Activity::Recording
    });
    let route = c.microphone_route().unwrap();
    assert!(route.contains("Synthetic input 2") && route.contains("已恢复录音"));
    assert_eq!(c.microphone_uid.as_deref(), Some("fixture-1"));
    assert!(catalog.system(Instant::now()).contains("Synthetic input 1"));

    let (resume, entered) = backend.block_next_release();
    c.stop();
    assert_eq!(activity(&c), Activity::Stopping);
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    c.cancel();
    assert_eq!(activity(&c), Activity::Releasing);
    assert!(c.microphone_route().is_none());
    resume.send(true).unwrap();
    until(|| {
        c.poll();
        activity(&c) == Activity::Idle
    });
    asr.assert_no_job();
    let entries: Vec<_> = std::fs::read_dir(root.path().join(NAMESPACE))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["owner.json"]);
}

#[test]
fn level_test_ignores_cached_permission_and_opens_the_current_system_route() {
    let (root, mut c, backend, asr) = rig();
    let mut catalog = Catalog::default();
    read(&mut catalog, &c);
    assert!(catalog.permission(Instant::now()).contains("已允许"));
    assert!(catalog.system(Instant::now()).contains("Synthetic input 1"));

    backend.set_authorized(false);
    c.start_microphone_test();
    until(|| {
        c.poll();
        !c.microphone_test_running() && !c.audio_input_busy()
    });
    assert!(c.microphone_test_status().contains("权限"));
    assert_eq!(backend.counts(), (0, 0, 0, 0));

    backend.set_authorized(true);
    backend.set_default(2);
    c.start_microphone_test();
    until(|| {
        c.poll();
        activity(&c) == Activity::Testing
    });
    assert!(c.microphone_test_status().contains("Synthetic input 2"));
    assert!(c.microphone_route().is_none());
    assert!(!c.microphone_test_display_is_stale(Instant::now()));
    assert!(c.microphone_test_display_is_stale(Instant::now() + Duration::from_secs(1)));
    c.stop_microphone_test();
    until(|| !c.audio_input_busy());
    asr.assert_no_job();
    assert!(!root.path().join(NAMESPACE).exists());
}
