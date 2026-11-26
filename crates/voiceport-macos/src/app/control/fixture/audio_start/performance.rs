//! Timing diverges from the regression fixture only in workload and observation.
use super::*;
use crate::app::quick_panel;
use serde_json::json;

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn usage() -> libc::rusage_info_v4 {
    // SAFETY: Zero is valid for the integer fields. libproc fills the matching
    // Darwin structure for this process only.
    unsafe {
        let mut usage: libc::rusage_info_v4 = std::mem::zeroed();
        let status = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            std::ptr::from_mut(&mut usage).cast(),
        );
        assert_eq!(status, 0, "{}", std::io::Error::last_os_error());
        usage
    }
}

pub fn isolated_performance_baseline(args: &[String]) {
    match args {
        [mode, cli, runs, seconds] if matches!(mode.as_str(), "headless" | "detached") => {
            let cli = Path::new(cli);
            assert!(cli.is_absolute() && cli.is_file());
            let runs = runs.parse::<u32>().unwrap();
            let seconds = seconds.parse::<u32>().unwrap();
            assert!((1..=30).contains(&runs) && (1..=60).contains(&seconds));
            let mtm = MainThreadMarker::new().unwrap();
            objc2::rc::autoreleasepool(|_| run(mtm, cli, runs, seconds, mode == "detached"));
        }
        [mode, id, model, runtime, sample, language, runs] if mode == "model" => {
            crate::local_runtime::performance_file(
                id,
                Path::new(model),
                Path::new(runtime),
                Path::new(sample),
                language,
                runs.parse().unwrap(),
                || usage().ri_lifetime_max_phys_footprint,
            );
        }
        _ => panic!(
            "expected headless|detached CLI RUNS SECONDS or model ID MODEL RUNTIME WAV LANGUAGE RUNS"
        ),
    }
}

fn run(mtm: MainThreadMarker, cli: &Path, runs: u32, seconds: u32, detached: bool) {
    let folder = tempfile::tempdir_in("/tmp").unwrap();
    let root = folder.path().canonicalize().unwrap();
    let socket = root.join("ipc/control.sock");
    let storage = Storage::at(root.join(NAMESPACE));
    let backend = Backend::new(storage.clone());
    let owner = Delegate::new(mtm);
    let (updates, requests) = mpsc::channel();
    let mock_install = root.join("Applications/VoicePort.app");
    std::fs::create_dir_all(&mock_install).unwrap();
    owner.configure_update_fixture(root.clone(), mock_install, updates);
    owner.ivars().audio_fixture_stroke.set(Some(None));
    let mut controller = Controller::new();
    controller.automatic = false;
    let asr = controller.install_audio_fixture(backend.clone());
    owner.ivars().controller.replace(Some(controller));
    owner.ivars().control.borrow_mut().server = Some(Server::bind(&socket).unwrap());
    owner.install_timer();
    let fixture = Fixture { cli, socket, owner };

    if detached {
        // AppKit objects are detached throughout: there is no status item,
        // window, popover presentation, event injection, or activation.
        let _app = NSApplication::sharedApplication(mtm);
        quick_panel::prepare_detached(&fixture.owner);
        quick_panel::refresh_detached(&fixture.owner);
        measure_detached(&fixture.owner, runs, "detached_panel_preparation");

        let opens = backend.counts().0;
        let resume = backend.block_next_open();
        let operation = cli_start(&fixture);
        until(|| backend.counts().0 > opens);
        fixture.owner.start_update_check(false);
        let update_reply = requests.recv_timeout(LIMIT).unwrap();
        let both_blocked = Instant::now();
        measure_detached(&fixture.owner, runs, "detached_panel_preparation_blocked");
        until(|| both_blocked.elapsed() >= Duration::from_secs(2));
        fixture
            .owner
            .with_controller(|c| assert!(c.opening_microphone()));
        assert_eq!(fixture.owner.update_snapshot()["state"], "checking");
        assert_eq!(
            fixture.call(&["dictation", "stop", &operation], 0)["data"]["state"],
            "idle"
        );
        resume.send(true).unwrap();
        update_reply.send(Err("fixture update released")).unwrap();
        released(&fixture);
        until(|| fixture.owner.update_snapshot()["busy"] == false);
        asr.assert_no_job();
        idle(&fixture.owner, seconds, "idle_detached_hidden");
    } else {
        for iteration in 1..=runs {
            let opens = backend.counts().0;
            let resume = backend.block_next_open();
            let operation = cli_start(&fixture);
            until(|| backend.counts().0 > opens);
            assert!(
                fixture
                    .owner
                    .ivars()
                    .controller
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .opening_microphone()
            );
            let query_started = Instant::now();
            let status = fixture.call(&["status"], 0);
            let query_ms = ms(query_started.elapsed());
            assert_eq!(status["data"]["pid"], std::process::id());
            assert_eq!(status["data"]["state"], "permissions");
            assert_eq!(status["data"]["opening_microphone"], true);
            assert_eq!(
                fixture.call(&["dictation", "stop", &operation], 0)["data"]["state"],
                "idle"
            );
            assert_eq!(
                fixture.call(&["dictation", "result", &operation], 130)["error"]["code"],
                "cancelled"
            );
            resume.send(true).unwrap();
            released(&fixture);
            asr.assert_no_job();

            let start = Instant::now();
            let operation = cli_start(&fixture);
            let accepted = start.elapsed();
            phase(&fixture, Phase::Recording);
            let recording = start.elapsed();
            sampled(&fixture);
            let generation = fixture
                .owner
                .ivars()
                .controller
                .borrow()
                .as_ref()
                .unwrap()
                .state
                .generation();
            let stop = Instant::now();
            let stopped = fixture.call(&["dictation", "stop", &operation], 0);
            assert_eq!(stopped["data"]["state"], "transcribing");
            until(|| asr.complete(generation));
            let finalized_audio = stop.elapsed();
            phase(&fixture, Phase::Ready);
            let final_text = stop.elapsed();
            let receipt = fixture.call(&["dictation", "result", &operation], 0);
            let result_reply = stop.elapsed();
            assert_eq!(receipt["data"]["text"], "Synthetic capture result");
            assert_eq!(receipt["data"]["delivery"], "none");
            released(&fixture);

            let mut delivery = SyntheticDictations::new();
            let (generation, receiver) = delivery.begin(true, true);
            let injected = Instant::now();
            delivery.finish(generation, "合成回执 VoicePort");
            let result = delivery.controller.gui_dictation_receipt().unwrap();
            assert_eq!(receiver.borrow().snapshot.text, "A合成回执 VoicePortB");
            assert_eq!(receiver.borrow().writes, 1);
            let serialized = serde_json::to_value(&result).unwrap();
            assert_eq!(serialized["delivery"]["state"], "confirmed");
            println!(
                "{}",
                json!({
                    "scene": "synthetic_dictation", "iteration": iteration,
                    "blocked_cli_status_ms": query_ms,
                    "start_to_accepted_ms": ms(accepted),
                    "start_to_recording_ms": ms(recording),
                    "stop_to_audio_validated_ms": ms(finalized_audio),
                    "stop_to_final_ms": ms(final_text),
                    "stop_to_cli_result_ms": ms(result_reply),
                    "synthetic_completion_to_confirmed_ms": ms(injected.elapsed()),
                    "peak_phys_footprint_bytes": usage().ri_lifetime_max_phys_footprint,
                })
            );
        }
        idle(&fixture.owner, seconds, "idle_panel_uncreated");
    }
    fixture.owner.ivars().timer.get().unwrap().invalidate();
    fixture.owner.ivars().control.borrow_mut().server.take();
    folder.close().unwrap();
}

fn measure_detached(owner: &Delegate, runs: u32, scene: &str) {
    for iteration in 1..=runs {
        let elapsed = objc2::rc::autoreleasepool(|_| {
            let start = Instant::now();
            quick_panel::refresh_detached(owner);
            start.elapsed()
        });
        println!(
            "{}",
            json!({
                "scene": scene,
                "iteration": iteration,
                "refresh_to_enabled_ms": ms(elapsed),
            })
        );
    }
}

fn idle(owner: &Delegate, seconds: u32, scene: &str) {
    let panel_created = owner.ivars().quick_panel.get().is_some();
    quick_panel::start_counting_detached();
    let before = usage();
    let start = Instant::now();
    let duration = Duration::from_secs(u64::from(seconds));
    while start.elapsed() < duration {
        let remaining = duration.saturating_sub(start.elapsed());
        objc2::rc::autoreleasepool(|_| {
            let until = NSDate::dateWithTimeIntervalSinceNow(remaining.as_secs_f64());
            let mode = unsafe { objc2_foundation::NSDefaultRunLoopMode };
            assert!(NSRunLoop::currentRunLoop().runMode_beforeDate(mode, &until));
        });
    }
    let delta_ns = start.elapsed().as_nanos() as f64;
    let after = usage();
    let (user_ns, system_ns) = (
        after.ri_user_time - before.ri_user_time,
        after.ri_system_time - before.ri_system_time,
    );
    println!(
        "{}",
        json!({
            "scene": scene,
            "window_seconds": delta_ns / 1e9,
            "panel_created": panel_created,
            "hidden_panel_refreshes": quick_panel::finish_counting_detached(),
            "cpu_percent_one_core": (user_ns + system_ns) as f64 * 100.0 / delta_ns,
            "package_idle_wakeups": after.ri_pkg_idle_wkups - before.ri_pkg_idle_wkups,
            "interrupt_wakeups": after.ri_interrupt_wkups - before.ri_interrupt_wkups,
            "resident_bytes_end": after.ri_resident_size,
            "peak_phys_footprint_bytes": after.ri_lifetime_max_phys_footprint,
        })
    );
}
