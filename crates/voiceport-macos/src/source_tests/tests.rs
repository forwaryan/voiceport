use super::*;
use crate::{
    local_models::{Model, ModelKind},
    plugin_process::cleanup_fixture::PendingChild,
};
use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

fn entry(id: &str) -> Entry {
    Entry {
        id: id.into(),
        title: id.into(),
        detail: "synthetic source".into(),
        source: Ok(SessionSource::Local(Model {
            kind: ModelKind::Whisper,
            directory: id.into(),
            runtime: PathBuf::new(),
            encoder: PathBuf::new(),
            decoder: PathBuf::new(),
            tokens: PathBuf::new(),
            joiner: None,
        })),
    }
}
fn source_id(source: SessionSource) -> String {
    match source {
        SessionSource::Local(model) => model.directory.to_str().unwrap().into(),
        _ => panic!("expected synthetic local source"),
    }
}
fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "test condition timed out");
        thread::sleep(Duration::from_millis(1));
    }
}
fn finish(tests: &mut SourceTests) {
    wait_until(|| {
        tests.poll();
        !tests.busy()
    });
}
fn cleanup_pending(tests: &mut SourceTests, entry: &Entry) {
    wait_until(|| {
        tests.poll();
        tests.row(entry).unwrap().status == Status::CleanupPending
            && tests.active.as_ref().unwrap().settled
    });
}
fn wait_for_check(tests: &mut SourceTests) {
    wait_until(|| {
        tests.poll();
        tests
            .active
            .as_ref()
            .is_none_or(|active| active.cleanup.as_ref().unwrap().observation.is_none())
    });
}

#[test]
fn results_are_per_entry_preflight_is_immediate_and_failures_continue() {
    let launches = Arc::new(Mutex::new(Vec::new()));
    let seen = launches.clone();
    let main_thread = thread::current().id();
    let mut tests = SourceTests::with_executor(BUDGET, move |source, language, _, _| {
        assert_ne!(thread::current().id(), main_thread);
        assert_eq!(language, "zh");
        let id = source_id(source);
        seen.lock().unwrap().push(id.clone());
        if id == "fails" {
            Err("synthetic failure".into())
        } else {
            Ok(())
        }
    });
    let first = entry("works");
    let failed = entry("fails");
    let last = entry("last");
    let mut unavailable = entry("unavailable");
    unavailable.source = Err("configuration unavailable".into());
    let entries = vec![
        first.clone(),
        unavailable.clone(),
        failed.clone(),
        last.clone(),
    ];
    let snapshot = entries.clone();
    tests.start(entries, "zh".into()).unwrap();
    assert_eq!(tests.row(&unavailable).unwrap().status, Status::Failed);
    assert_eq!(tests.row(&unavailable).unwrap().elapsed, None);
    assert!(tests.row(&unavailable).unwrap().completed_at.is_some());
    finish(&mut tests);
    assert_eq!(*launches.lock().unwrap(), ["works", "fails", "last"]);
    assert_eq!(tests.row(&first).unwrap().status, Status::Success);
    assert_eq!(tests.row(&failed).unwrap().status, Status::Failed);
    assert_eq!(tests.row(&failed).unwrap().detail, "synthetic failure");
    assert_eq!(tests.row(&last).unwrap().status, Status::Success);
    assert_eq!(snapshot, [first, unavailable, failed, last]);
    assert!(tests.summary().contains("通过 2，失败 2"));
}

#[test]
fn elapsed_and_completion_are_worker_measured_not_queue_or_poll_time() {
    let mut tests = SourceTests::with_executor(BUDGET, |_, _, _, _| {
        thread::sleep(Duration::from_millis(5));
        Ok(())
    });
    let first = entry("first");
    let second = entry("second");
    tests
        .start(vec![first.clone(), second.clone()], String::new())
        .unwrap();
    wait_until(|| {
        tests
            .active
            .as_ref()
            .unwrap()
            .thread
            .as_ref()
            .unwrap()
            .is_finished()
    });
    SourceTests::receive_events(tests.active.as_mut().unwrap());
    let completion = tests.active.as_ref().unwrap().completion.as_ref().unwrap();
    let expected_elapsed = completion.finished.duration_since(completion.started);
    let expected_time = completion.completed_at;
    let first_finished = completion.finished;
    thread::sleep(Duration::from_millis(30));
    tests.poll();
    assert_eq!(tests.row(&first).unwrap().elapsed, Some(expected_elapsed));
    assert_eq!(tests.row(&first).unwrap().completed_at, Some(expected_time));
    assert!(tests.row(&second).unwrap().completed_at.is_none());
    wait_until(|| {
        tests
            .active
            .as_ref()
            .unwrap()
            .thread
            .as_ref()
            .unwrap()
            .is_finished()
    });
    SourceTests::receive_events(tests.active.as_mut().unwrap());
    let second_completion = tests.active.as_ref().unwrap().completion.as_ref().unwrap();
    assert!(second_completion.started > first_finished + Duration::from_millis(20));
    let second_elapsed = second_completion
        .finished
        .duration_since(second_completion.started);
    finish(&mut tests);
    assert_eq!(tests.row(&second).unwrap().elapsed, Some(second_elapsed));
}

#[test]
fn ordinary_cancel_stops_queue_and_waits_for_active_resources() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let launches = Arc::new(AtomicUsize::new(0));
    let seen = launches.clone();
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, cancel, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        entered_tx.send(cancel.clone()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        assert!(cancel.is_cancelled());
        Err("cancel observed".into())
    });
    let active = entry("active");
    let queued = entry("queued");
    tests
        .start(vec![active.clone(), queued.clone()], String::new())
        .unwrap();
    let token = entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    tests.cancel();
    assert!(token.is_cancelled());
    assert!(tests.busy());
    assert!(tests.start(vec![entry("another")], String::new()).is_err());
    assert_eq!(tests.row(&queued).unwrap().status, Status::Cancelled);
    assert!(tests.row(&queued).unwrap().elapsed.is_none());
    release_tx.send(()).unwrap();
    finish(&mut tests);
    assert_eq!(tests.row(&active).unwrap().status, Status::Cancelled);
    assert_eq!(launches.load(Ordering::SeqCst), 1);
}

#[test]
fn cancelling_finished_unpolled_work_preserves_success_without_dispatch() {
    let launches = Arc::new(AtomicUsize::new(0));
    let seen = launches.clone();
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, _, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let first = entry("first");
    let second = entry("second");
    tests
        .start(vec![first.clone(), second.clone()], String::new())
        .unwrap();
    wait_until(|| {
        tests
            .active
            .as_ref()
            .unwrap()
            .thread
            .as_ref()
            .unwrap()
            .is_finished()
    });
    tests.cancel();
    assert!(!tests.busy());
    assert_eq!(tests.row(&first).unwrap().status, Status::Success);
    assert_eq!(tests.row(&second).unwrap().status, Status::Cancelled);
    assert_eq!(launches.load(Ordering::SeqCst), 1);
}

#[test]
fn timeout_requests_cancel_but_never_overlaps_the_next_job() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let launches = Arc::new(AtomicUsize::new(0));
    let seen = launches.clone();
    let mut tests =
        SourceTests::with_executor(Duration::from_millis(10), move |source, _, cancel, _| {
            seen.fetch_add(1, Ordering::SeqCst);
            if source_id(source) == "slow" {
                entered_tx.send(cancel.clone()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap();
                assert!(cancel.is_cancelled());
            }
            Ok(())
        });
    let slow = entry("slow");
    let next = entry("next");
    tests
        .start(vec![slow.clone(), next.clone()], String::new())
        .unwrap();
    let token = entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    wait_until(|| {
        tests.poll();
        token.is_cancelled()
    });
    assert!(tests.busy());
    assert_eq!(tests.row(&slow).unwrap().status, Status::TimedOut);
    assert!(tests.row(&slow).unwrap().completed_at.is_none());
    assert_eq!(tests.row(&next).unwrap().status, Status::Pending);
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert!(tests.start(vec![next.clone()], String::new()).is_err());
    release_tx.send(()).unwrap();
    finish(&mut tests);
    assert_eq!(tests.row(&slow).unwrap().status, Status::TimedOut);
    assert!(tests.row(&slow).unwrap().elapsed.unwrap() >= Duration::from_millis(10));
    assert_eq!(launches.load(Ordering::SeqCst), 2);
}

#[test]
fn completion_over_budget_is_timeout_even_without_a_poll_during_execution() {
    let mut tests = SourceTests::with_executor(Duration::from_millis(1), |_, _, _, _| {
        thread::sleep(Duration::from_millis(5));
        Ok(())
    });
    let source = entry("slow");
    tests.start(vec![source.clone()], String::new()).unwrap();
    wait_until(|| {
        tests
            .active
            .as_ref()
            .unwrap()
            .thread
            .as_ref()
            .unwrap()
            .is_finished()
    });
    finish(&mut tests);
    assert_eq!(tests.row(&source).unwrap().status, Status::TimedOut);
}

#[test]
fn snapshots_invalidate_rows_and_new_suite_preserves_unrelated_results() {
    let mut tests = SourceTests::with_executor(BUDGET, |_, _, _, _| Ok(()));
    let original = entry("stable-id");
    let unrelated = entry("unrelated");
    tests
        .start(vec![original.clone(), unrelated.clone()], "zh".into())
        .unwrap();
    finish(&mut tests);
    let unrelated_time = tests.row(&unrelated).unwrap().completed_at;
    let mut changed = original.clone();
    if let Ok(SessionSource::Local(model)) = &mut changed.source {
        model.encoder = "changed-configuration".into();
    }
    assert!(tests.row(&changed).is_none());
    let mut changed_title = original.clone();
    changed_title.title = "renamed".into();
    assert!(tests.row(&changed_title).is_none());
    tests.start(vec![changed.clone()], "en".into()).unwrap();
    assert!(tests.row(&original).is_none());
    assert_eq!(tests.row(&unrelated).unwrap().completed_at, unrelated_time);
    finish(&mut tests);
    assert_eq!(tests.row(&changed).unwrap().status, Status::Success);
    assert!(tests.row(&original).is_none());
}

#[test]
fn a_delayed_under_budget_completion_replaces_provisional_timeout() {
    let (release, released) = mpsc::channel();
    let released = Mutex::new(released);
    let mut tests = SourceTests::with_executor(Duration::from_secs(1), move |_, _, _, _| {
        released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(())
    });
    let source = entry("delayed-event");
    tests.start(vec![source.clone()], String::new()).unwrap();
    let (sender, receiver) = mpsc::channel();
    let started = Instant::now() - Duration::from_secs(2);
    let active = tests.active.as_mut().unwrap();
    active.receiver = receiver;
    active.started = Some(started);
    tests.poll();
    assert_eq!(tests.row(&source).unwrap().status, Status::TimedOut);
    sender
        .send(Event::Finished(Completion {
            started,
            finished: started + Duration::from_millis(100),
            completed_at: SystemTime::now(),
            result: Ok(()),
            cleanup: None,
            _custody: HeldCustody(Custody::default()),
        }))
        .unwrap();
    tests.poll();
    assert!(
        tests.busy(),
        "an event cannot replace actual worker exit proof"
    );
    release.send(()).unwrap();
    finish(&mut tests);
    assert_eq!(tests.row(&source).unwrap().status, Status::Success);
    assert_eq!(
        tests.row(&source).unwrap().elapsed,
        Some(Duration::from_millis(100))
    );
}

#[test]
fn poll_and_cancel_do_not_join_a_worker_with_pending_tls_teardown() {
    struct Hold(Receiver<()>);
    impl Drop for Hold {
        fn drop(&mut self) {
            let _ = self.0.recv_timeout(Duration::from_secs(5));
        }
    }
    thread_local! {
        static HOLD: std::cell::RefCell<Option<Hold>> = const { std::cell::RefCell::new(None) };
    }
    let (release, released) = mpsc::channel();
    let released = Mutex::new(Some(released));
    let (entered, entry_rx) = mpsc::channel();
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, _, _| {
        HOLD.with(|hold| *hold.borrow_mut() = Some(Hold(released.lock().unwrap().take().unwrap())));
        entered.send(()).unwrap();
        Ok(())
    });
    let source = entry("tls-release");
    tests.start(vec![source.clone()], String::new()).unwrap();
    entry_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    wait_until(|| {
        SourceTests::receive_events(tests.active.as_mut().unwrap());
        tests.active.as_ref().unwrap().completion.is_some()
    });
    tests.poll();
    tests.cancel();
    assert!(
        tests.busy(),
        "the worker body has ended but TLS is still owned"
    );
    release.send(()).unwrap();
    finish(&mut tests);
    assert_eq!(tests.row(&source).unwrap().status, Status::Success);
}

#[test]
fn catalog_refresh_removes_obsolete_results_from_the_summary() {
    let mut tests = SourceTests::with_executor(BUDGET, |_, _, _, _| Ok(()));
    let original = entry("updated");
    let unchanged = entry("retained");
    tests
        .start(vec![original.clone(), unchanged.clone()], String::new())
        .unwrap();
    finish(&mut tests);
    let mut updated = original.clone();
    updated.source = Err("configuration changed".into());
    tests.retain_catalog(&[updated.clone(), unchanged.clone()]);
    assert!(tests.row(&original).is_none());
    assert!(tests.row(&updated).is_none());
    assert!(tests.row(&unchanged).is_some());
    assert!(tests.summary().contains("通过 1，失败 0"));
}

#[test]
fn invalid_suites_are_atomic_and_bounded() {
    let mut tests = SourceTests::with_executor(BUDGET, |_, _, _, _| Ok(()));
    let source = entry("saved");
    tests.start(vec![source.clone()], String::new()).unwrap();
    finish(&mut tests);
    let saved = tests.row(&source).unwrap().completed_at;
    assert!(tests.start(vec![], String::new()).is_err());
    assert!(
        tests
            .start(vec![source.clone(), source.clone()], String::new())
            .is_err()
    );
    let too_many = (0..=MAX_ENTRIES)
        .map(|i| entry(&format!("source-{i}")))
        .collect();
    assert!(tests.start(too_many, String::new()).is_err());
    assert!(!tests.busy());
    assert_eq!(tests.row(&source).unwrap().completed_at, saved);
}

#[test]
fn maximum_catalog_completes_all_preflight_failures_without_workers() {
    let mut tests = SourceTests::with_executor(BUDGET, |_, _, _, _| panic!("must not execute"));
    let entries: Vec<_> = (0..132)
        .map(|i| {
            let mut source = entry(&format!("unavailable-{i}"));
            source.source = Err("configuration unavailable".into());
            source
        })
        .collect();
    tests.start(entries.clone(), String::new()).unwrap();
    assert!(!tests.busy());
    assert!(tests.active.is_none());
    assert!(tests.queue.is_empty());
    assert_eq!(tests.rows.len(), 132);
    for source in &entries {
        let result = tests.row(source).unwrap();
        assert_eq!(result.status, Status::Failed);
        assert_eq!(result.detail, "configuration unavailable");
        assert!(result.elapsed.is_none());
        assert!(result.completed_at.is_some());
    }
    assert!(tests.summary().contains("通过 0，失败 132，超时 0，取消 0"));
}

#[test]
fn all_preflight_failures_need_no_worker() {
    let mut tests = SourceTests::with_executor(BUDGET, |_, _, _, _| panic!("must not execute"));
    let mut source = entry("disabled");
    source.source = Err("disabled".into());
    tests.start(vec![source.clone()], String::new()).unwrap();
    assert!(!tests.busy());
    assert_eq!(tests.row(&source).unwrap().status, Status::Failed);
}

#[test]
fn panic_is_a_failure_and_does_not_expose_the_payload() {
    let mut tests = SourceTests::with_executor(BUDGET, |source, _, _, _| {
        if source_id(source) == "panics" {
            panic!("synthetic private panic payload");
        }
        Ok(())
    });
    let source = entry("panics");
    let next = entry("next");
    tests
        .start(vec![source.clone(), next.clone()], String::new())
        .unwrap();
    finish(&mut tests);
    let result = tests.row(&source).unwrap();
    assert_eq!(result.status, Status::Failed);
    assert!(!result.detail.contains("private"));
    assert_eq!(tests.row(&next).unwrap().status, Status::Success);
}

#[test]
fn unknown_cleanup_blocks_admission_stops_queue_and_only_explicit_checks_observe() {
    let (fixture_tx, fixture_rx) = mpsc::channel();
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, _, custody| {
        let fixture = PendingChild::in_custody(custody.clone());
        fixture_tx.send(fixture).unwrap();
        Err(Failure {
            message: "synthetic failure".into(),
            cleanup: Some(custody.clone()),
        })
    });
    let source = entry("pending");
    let queued = entry("queued");
    tests
        .start(vec![source.clone(), queued.clone()], String::new())
        .unwrap();
    let child = fixture_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    cleanup_pending(&mut tests, &source);
    assert!(tests.busy());
    assert!(tests.row(&source).unwrap().completed_at.is_none());
    assert_eq!(tests.row(&queued).unwrap().status, Status::Cancelled);
    assert!(tests.start(vec![source.clone()], String::new()).is_err());
    for _ in 0..5 {
        tests.poll();
    }
    assert_eq!(child.observations(), 0);
    tests.cancel();
    wait_for_check(&mut tests);
    assert_eq!(child.observations(), 1);
    assert!(tests.busy()); // The check sent no signal; child remains alive.
    child.terminate();
    for _ in 0..5 {
        tests.poll();
    }
    assert!(tests.busy()); // Exit alone does not silently clear the cleanup gate.
    assert_eq!(child.observations(), 1);
    tests.cancel();
    finish(&mut tests);
    assert_eq!(child.observations(), 2);
    assert_eq!(tests.row(&source).unwrap().status, Status::Failed);
    assert!(tests.row(&source).unwrap().detail.contains("清理已确认"));
}

#[test]
fn panic_with_unresolved_parent_custody_still_requires_explicit_check() {
    let (fixture_tx, fixture_rx) = mpsc::channel();
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, _, custody| {
        fixture_tx
            .send(PendingChild::in_custody(custody.clone()))
            .unwrap();
        panic!("synthetic worker unwind");
    });
    let source = entry("panics");
    tests.start(vec![source.clone()], String::new()).unwrap();
    let child = fixture_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    cleanup_pending(&mut tests, &source);
    assert_eq!(child.observations(), 0);
    child.terminate();
    tests.cancel();
    finish(&mut tests);
    assert_eq!(child.observations(), 1);
    assert_eq!(tests.row(&source).unwrap().status, Status::Failed);
}

#[test]
fn channel_disconnect_cannot_clear_cleanup_before_worker_settles() {
    let (fixture_tx, fixture_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, _, custody| {
        fixture_tx
            .send(PendingChild::in_custody(custody.clone()))
            .unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        Ok(())
    });
    let source = entry("disconnected");
    tests
        .start(vec![source.clone(), entry("queued")], String::new())
        .unwrap();
    let child = fixture_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let (sender, receiver) = mpsc::channel();
    tests.active.as_mut().unwrap().receiver = receiver;
    drop(sender);
    tests.poll();
    assert_eq!(tests.row(&source).unwrap().status, Status::CleanupPending);
    assert!(!tests.active.as_ref().unwrap().settled);
    child.terminate();
    tests.cancel();
    wait_until(|| child.observations() == 1 && !child.custody.unresolved());
    tests.poll();
    assert!(tests.busy());
    assert!(
        tests
            .active
            .as_ref()
            .unwrap()
            .cleanup
            .as_ref()
            .unwrap()
            .observation
            .is_some()
    );
    release_tx.send(()).unwrap();
    finish(&mut tests);
    assert_eq!(tests.row(&source).unwrap().status, Status::Failed);
    assert_eq!(child.observations(), 1);
}

#[test]
fn failure_cleanup_owner_is_retained_even_when_distinct_or_already_resolved() {
    let (fixture_tx, fixture_rx) = mpsc::channel();
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, _, _| {
        let child = PendingChild::new();
        let cleanup = child.custody.clone();
        fixture_tx.send(child).unwrap();
        Err(Failure {
            message: "distinct owner".into(),
            cleanup: Some(cleanup),
        })
    });
    let source = entry("foreign-owner");
    tests.start(vec![source.clone()], String::new()).unwrap();
    let child = fixture_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    cleanup_pending(&mut tests, &source);
    assert!(!tests.active.as_ref().unwrap().custody.0.unresolved());
    child.terminate();
    tests.cancel();
    finish(&mut tests);
    assert_eq!(child.observations(), 1);

    let mut tests = SourceTests::with_executor(BUDGET, |_, _, _, _| {
        Err(Failure {
            message: "explicit cleanup marker".into(),
            cleanup: Some(Custody::default()),
        })
    });
    tests.start(vec![source.clone()], String::new()).unwrap();
    cleanup_pending(&mut tests, &source);
    assert!(tests.busy());
    tests.cancel();
    finish(&mut tests);
    assert_eq!(tests.row(&source).unwrap().status, Status::Failed);
}

#[test]
fn cleanup_observation_for_another_owner_cannot_release_this_job() {
    let (fixture_tx, fixture_rx) = mpsc::channel();
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, _, custody| {
        fixture_tx
            .send(PendingChild::in_custody(custody.clone()))
            .unwrap();
        Err("unresolved child without explicit failure marker".into())
    });
    let source = entry("pending");
    tests.start(vec![source.clone()], String::new()).unwrap();
    let child = fixture_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    cleanup_pending(&mut tests, &source);
    let (sender, receiver) = mpsc::channel();
    sender.send(true).unwrap();
    tests
        .active
        .as_mut()
        .unwrap()
        .cleanup
        .as_mut()
        .unwrap()
        .observation = Some(Observation {
        custody: Custody::default(),
        receiver,
    });
    tests.poll();
    assert!(tests.busy());
    assert_eq!(child.observations(), 0);
    child.terminate();
    tests.cancel();
    finish(&mut tests);
    assert_eq!(child.observations(), 1);
}

#[test]
fn drop_cancels_active_and_never_launches_queued_sources() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let launches = Arc::new(AtomicUsize::new(0));
    let seen = launches.clone();
    let mut tests = SourceTests::with_executor(BUDGET, move |_, _, token, _| {
        seen.fetch_add(1, Ordering::SeqCst);
        entered_tx.send(token.clone()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        Ok(())
    });
    tests
        .start(vec![entry("active"), entry("queued")], String::new())
        .unwrap();
    let token = entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let worker = tests.active.as_mut().unwrap().thread.take().unwrap();
    drop(tests);
    assert!(token.is_cancelled());
    release_tx.send(()).unwrap();
    worker.join().unwrap();
    assert_eq!(launches.load(Ordering::SeqCst), 1);
}

#[test]
fn display_is_compact_and_never_includes_detail_or_snapshots() {
    let mut result = ProbeResult::pending();
    result.detail = "synthetic private detail".into();
    assert!(result.display().contains('—'));
    assert!(!result.display().contains("private"));
    result.status = Status::Success;
    result.elapsed = Some(Duration::from_millis(42));
    result.completed_at = Some(SystemTime::now());
    assert!(result.display().contains("42 ms"));
    assert!(result.display().contains("刚刚完成"));
    result.completed_at = Some(SystemTime::now() - Duration::from_secs(120));
    assert!(result.display().contains("2 分钟前"));
}
