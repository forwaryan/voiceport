//! Debug-only adapter copied into a private archive of a pinned source tree.
use super::*;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Scenario {
    directory: PathBuf,
    blocked: bool,
    stall: bool,
    source_listener: OnceCell<std::net::TcpListener>,
    sources: bool,
    hidden_started: Cell<Option<Instant>>,
    hidden_finished: Cell<bool>,
    status_actions: Cell<u8>,
    first_surface: Cell<FirstSurface>,
}

#[derive(Clone, Copy, Default)]
struct FirstSurface {
    sampled: bool,
    shown: bool,
    visible_window: bool,
    app_active: bool,
}

thread_local! {
    static SCENARIO: OnceCell<Scenario> = const { OnceCell::new() };
}

#[path = "../../fixtures/hosted_aqua_entry.rs"]
mod hosted_aqua_entry;

fn wait_for_release(directory: &Path) -> bool {
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= Duration::from_secs(45) {
            return false;
        }
        if elapsed >= Duration::from_secs(2) && directory.join("release").is_file() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn is_active() -> bool {
    SCENARIO.with(|state| state.get().is_some())
}

pub(super) fn observing_sources() -> bool {
    SCENARIO.with(|state| state.get().is_some_and(|scenario| scenario.sources))
}

pub(super) fn observe_hidden(owner: &Delegate) {
    SCENARIO.with(|state| {
        let Some(scenario) = state.get() else { return };
        if scenario.status_actions.get() == 1 {
            let app = NSApplication::sharedApplication(owner.mtm());
            let panel = owner.ivars().quick_panel.get();
            let mut observed = scenario.first_surface.get();
            observed.sampled = true;
            observed.shown |= panel.is_some_and(|panel| panel.is_shown());
            observed.visible_window |= panel.is_some_and(|panel| panel.fixture_source_state(&app).is_some());
            observed.app_active |= app.isActive();
            scenario.first_surface.set(observed);
        }
        let abort = scenario.directory.join("abort").is_file();
        if abort || scenario.directory.join("finish").is_file() {
            if owner.ivars().context_menu_visible.get() {
                owner.ivars().context_menu.get().unwrap().cancelTracking();
                // Let the native tracking call unwind before closing owned surfaces.
                return;
            }
            let app = NSApplication::sharedApplication(owner.mtm());
            if let Some(modal) = app.modalWindow() {
                app.abortModal();
                modal.close();
                assert!(!modal.isVisible());
                // Let the app return from its modal loop before cleanup.
                return;
            }
            owner.close_quick_panel();
            if let Some(window) = owner.ivars().window.get() {
                window.close();
                assert!(!window.isVisible());
            }
            assert!(owner.ivars().quick_panel.get().is_none_or(|panel| !panel.is_shown()));
            if scenario.sources {
                menu_source_accept::cleanup();
            }
            if abort && scenario.sources {
                println!("source-probe: abort cleanup complete");
            } else if abort {
                println!("menu-probe: abort cleanup complete");
            }
            let observed = scenario.first_surface.get();
            eprintln!("menu-native-first-surface sampled={} shown={} visible_window={} app_active={}",
                      observed.sampled, observed.shown, observed.visible_window, observed.app_active);
            println!("menu-probe: owned surface closed");
            std::process::exit(0);
        }
        if let Some(listener) = scenario.source_listener.get() {
            menu_source_accept::sample(owner, &scenario.directory, listener);
            let ready = scenario.directory.join("ready");
            if !ready.is_file() {
                fs::File::create_new(ready).unwrap();
            }
        }
        if scenario.hidden_finished.get() || !scenario.directory.join("hidden-request").is_file() {
            return;
        }
        let hidden = owner
            .ivars()
            .quick_panel
            .get()
            .is_some_and(|panel| !panel.is_shown());
        if !hidden {
            fs::write(scenario.directory.join("hidden-invalid"), "popover visible").unwrap();
            scenario.hidden_finished.set(true);
        } else if let Some(started) = scenario.hidden_started.get() {
            if started.elapsed() >= Duration::from_secs(60) {
                let count = quick_panel::finish_counting_detached();
                fs::write(
                    scenario.directory.join("hidden-result"),
                    format!("{count} {}", started.elapsed().as_secs_f64()),
                )
                .unwrap();
                scenario.hidden_finished.set(true);
            }
        } else {
            quick_panel::start_counting_detached();
            scenario.hidden_started.set(Some(Instant::now()));
        }
    });
}

pub(super) fn before_status_action() {
    SCENARIO.with(|state| {
        let Some(fixture) = state.get() else { return };
        let actions = fixture.status_actions.get();
        fixture.status_actions.set((actions + 1).min(2));
        if actions == 0 {
            let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
            let event = match app.currentEvent().as_deref().map(NSEvent::r#type) {
                Some(NSEventType::LeftMouseUp) => "left_up",
                Some(NSEventType::RightMouseUp) => "right_up",
                Some(_) => "other",
                None => "none",
            };
            eprintln!("menu-native-first-event {event}");
        }
        if fixture.stall {
            println!("menu-probe: main-thread stall 400ms");
            thread::sleep(Duration::from_millis(400));
        }
    });
}

pub(super) fn device_result() -> Option<io::Result<mpsc::Receiver<menu_surface::DeviceResult>>> {
    SCENARIO.with(|state| {
        let scenario = state.get()?;
        let (reply, receive) = mpsc::channel();
        if !scenario.blocked || scenario.directory.join("device-done").is_file() {
            let _ = reply.send(Ok(Some("Synthetic Input".into())));
            return Some(Ok(receive));
        }
        let directory = scenario.directory.clone();
        Some(
            thread::Builder::new()
                .name("menu-probe-device".into())
                .spawn(move || {
                    if fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(directory.join("device-started"))
                        .is_err()
                    {
                        let _ = fs::write(directory.join("device-duplicate"), "duplicate");
                        return;
                    }
                    if wait_for_release(&directory) {
                        let _ = fs::write(directory.join("device-done"), "ready");
                        let _ = reply.send(Ok(Some("Synthetic Input".into())));
                    } else {
                        let _ = fs::write(directory.join("device-timeout"), "timeout");
                    }
                })
                .map(|_| receive),
        )
    })
}

pub(super) fn run(directory: &str, scene: &str, boot: &str, aqua: &str, launch: &[String]) {
    let sources = scene == "sources";
    let bundle_id = if sources {
        let domain = std::env::var("VOICEPORT_NATIVE_SOURCE_DOMAIN").unwrap_or_default();
        let valid = domain
            .strip_prefix("org.voiceport.source-acceptance-fixture.")
            .is_some_and(|nonce| {
                nonce.len() == 32
                    && nonce
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            });
        if !valid {
            eprintln!("source-probe: missing unique preference domain");
            std::process::exit(78);
        }
        domain
    } else {
        "org.voiceport.menu-acceptance-fixture".into()
    };
    hosted_aqua_entry::enter(boot, aqua, launch, &bundle_id, 140);
    let blocked = scene == "blocked";
    let stall = scene == "stall";
    if !(blocked || stall || sources || scene == "warm") {
        eprintln!("menu-probe: requires the authorized isolated Aqua guest");
        std::process::exit(78);
    }
    let directory = PathBuf::from(directory);
    assert!(directory.is_absolute() && directory.is_dir());
    SCENARIO.with(|state| {
        assert!(
            state
                .set(Scenario {
                    directory: directory.clone(),
                    blocked,
                    stall,
                    sources,
                    source_listener: OnceCell::new(),
                    hidden_started: Cell::new(None),
                    hidden_finished: Cell::new(false),
                    status_actions: Cell::new(0),
                    first_surface: Cell::new(FirstSurface::default()),
                })
                .is_ok()
        );
    });
    let mtm = MainThreadMarker::new().expect("probe on main thread");
    let app = NSApplication::sharedApplication(mtm);
    let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    if app.activationPolicy() != NSApplicationActivationPolicy::Accessory {
        std::process::exit(78);
    }
    let delegate = Delegate::new(mtm);
    delegate.ivars().audio_fixture_stroke.set(Some(None));
    let mut controller = Controller::new();
    controller.automatic = false;
    controller.microphone_uid = Some("fixture-input".into());
    if sources {
        let listener = menu_source_accept::initialize(&delegate, &mut controller, &directory);
        SCENARIO.with(|state| state.get().unwrap().source_listener.set(listener).unwrap());
    } else {
        let mut source = controller.catalog.entries[0].clone();
        source.id = "fixture-alternative".into();
        source.title = "Synthetic Alternative".into();
        controller.catalog.entries.push(source);
    }
    delegate.ivars().controller.replace(Some(controller));
    // Keep the same native status button, selector, AppKit tracking and production tick.
    // Skip application bootstrap, global hotkeys and real discovery.
    delegate.build_menu();
    let installed = directory.join("Applications/VoicePort.app");
    fs::create_dir_all(&installed).unwrap();
    let (requests, entered) = mpsc::channel();
    delegate.configure_update_fixture(directory.clone(), installed, requests);
    if blocked {
        assert_eq!(delegate.control_update_check().data["state"], "checking");
        let release = entered.recv_timeout(Duration::from_secs(3)).unwrap();
        let output = directory.clone();
        thread::spawn(move || {
            fs::write(output.join("update-started"), "ready").unwrap();
            if wait_for_release(&output) {
                release.send(Err("合成更新检查已结束")).unwrap();
                fs::write(output.join("update-done"), "ready").unwrap();
            } else {
                fs::write(output.join("update-timeout"), "timeout").unwrap();
            }
        });
    }
    delegate.refresh();
    delegate.install_timer();
    if !sources {
        fs::write(directory.join("ready"), "ready").unwrap();
    }
    app.run();
}
