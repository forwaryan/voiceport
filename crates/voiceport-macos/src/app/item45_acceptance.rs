use super::*;
use crate::app_update;
use std::ffi::{CString, c_char, c_int, c_void};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use objc2_foundation::{NSDate, NSProcessInfo, NSRunLoop};

const BLOCK: usize = 256 * 1024;
static PAYLOAD: [u8; BLOCK * 4] = [b'x'; BLOCK * 4];
const FAILED: &str = "无法准备更新…";
const VERIFY: &str = "下载完成 100% · 正在校验并暂存…";
type Check = Result<(), String>;

fn trace(file: Option<&File>, phase: &str, event: &str) {
    if let Some(mut file) = file {
        // All callers use fixed labels. One append keeps this stream independent of stdio buffering.
        let _ = file.write_all(format!("{phase} {event}\n").as_bytes());
    }
}

type Attach =
    unsafe extern "C" fn(*const NSMenu, *const NSStatusBarButton, *const c_char, c_int) -> c_int;
type ExpireObservation = unsafe extern "C" fn();
type ReadObservation = unsafe extern "C" fn() -> c_int;
type StartObservation = unsafe extern "C" fn(*const c_char, *const c_char) -> c_int;

struct Presentation {
    attach: Attach,
    tracking: ReadObservation,
    start: StartObservation,
    result: ReadObservation,
    expire: ExpireObservation,
    trace: Option<File>,
}

impl Presentation {
    fn load(path: &Path, output: &Path) -> Result<Self, String> {
        let trace_file = OpenOptions::new()
            .append(true)
            .open(output.join("trace"))
            .ok();
        trace(trace_file.as_ref(), "scenario", "starting");
        let path = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err("presentation library could not load".into());
        }
        let symbol = |name: &std::ffi::CStr| {
            let pointer = unsafe { libc::dlsym(handle, name.as_ptr()) };
            (!pointer.is_null())
                .then_some(pointer)
                .ok_or("presentation symbol missing")
        };
        // The library stays loaded until process exit because it owns observers and callbacks.
        unsafe {
            Ok(Self {
                attach: std::mem::transmute::<*mut c_void, Attach>(symbol(c"item45_attach")?),
                tracking: std::mem::transmute::<*mut c_void, ReadObservation>(symbol(
                    c"item45_tracking",
                )?),
                start: std::mem::transmute::<*mut c_void, StartObservation>(symbol(
                    c"item45_sample",
                )?),
                result: std::mem::transmute::<*mut c_void, ReadObservation>(symbol(
                    c"item45_sample_result",
                )?),
                expire: std::mem::transmute::<*mut c_void, ExpireObservation>(symbol(
                    c"item45_expire_sample",
                )?),
                trace: trace_file,
            })
        }
    }
}

fn pump(seconds: f64) {
    let run_loop = NSRunLoop::currentRunLoop();
    let until = NSDate::dateWithTimeIntervalSinceNow(seconds);
    if let Some(mode) = run_loop.currentMode() {
        run_loop.runMode_beforeDate(&mode, &until);
    } else {
        run_loop.runUntilDate(&until);
    }
}

fn wait(label: &str, duration: Duration, mut predicate: impl FnMut() -> bool) -> Check {
    let deadline = Instant::now() + duration;
    while !predicate() {
        if Instant::now() >= deadline {
            return Err(format!("timed out: {label}"));
        }
        pump(0.03);
    }
    Ok(())
}

fn serve(
    listener: TcpListener,
    commands: mpsc::Receiver<bool>,
    started: Instant,
    trace_file: Option<&File>,
) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    for (attempt, chunks) in [(1, 3), (2, 4)] {
        let deadline = Instant::now() + Duration::from_secs(28);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e),
            }
        };
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))?;
        assert_fixture_request(&socket)?;
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            PAYLOAD.len()
        )?;
        for count in 1..=chunks {
            let delayed = commands
                .recv_timeout(Duration::from_secs(18))
                .map_err(std::io::Error::other)?;
            if delayed {
                std::thread::sleep(Duration::from_millis(640));
            }
            socket.write_all(&PAYLOAD[..BLOCK])?;
            trace(trace_file, "wire", "chunk-written");
            println!(
                "wire attempt={attempt} cumulative_bytes={} elapsed_ms={}",
                count * BLOCK,
                started.elapsed().as_millis()
            );
        }
        if attempt == 1 {
            commands
                .recv_timeout(Duration::from_secs(18))
                .map_err(std::io::Error::other)?;
            println!("wire attempt=1 truncated");
        }
    }
    Ok(())
}

fn assert_fixture_request(socket: &TcpStream) -> std::io::Result<()> {
    let mut request = BufReader::new(socket);
    let mut line = String::new();
    request.read_line(&mut line)?;
    if line != "GET /item45 HTTP/1.1\r\n" {
        return Err(std::io::Error::other("unexpected fixture request"));
    }
    loop {
        line.clear();
        if request.read_line(&mut line)? == 0 || line == "\r\n" {
            return Ok(());
        }
    }
}

struct Scenario {
    started: Instant,
    owner: Retained<Delegate>,
    menu: Retained<NSMenu>,
    item: Retained<NSMenuItem>,
    wire: mpsc::Sender<bool>,
    finish_verification: Arc<AtomicBool>,
    presentation: Option<Presentation>,
    observation_failed: Cell<bool>,
}

impl Scenario {
    fn trace(&self, phase: &str, event: &str) {
        trace(
            self.presentation.as_ref().and_then(|p| p.trace.as_ref()),
            phase,
            event,
        );
    }

    fn advance_wire(&self, delayed: bool) -> Check {
        self.wire.send(delayed).map_err(|_| {
            self.trace("wire", "server-stopped");
            "fixture server stopped".into()
        })
    }

    fn title(&self) -> String {
        self.item.title().to_string()
    }

    fn action(&self) -> Check {
        let index = self.menu.indexOfItem(&self.item);
        if index < 0 || !self.item.isEnabled() {
            return Err("update action was not enabled and attached".into());
        }
        self.menu.performActionForItemAtIndex(index);
        Ok(())
    }

    fn download(&self) -> Check {
        self.action()?;
        wait("new release offered", Duration::from_secs(4), || {
            self.title().starts_with("发现新版本")
        })?;
        self.action()?;
        self.observe("connecting", "正在连接下载…", false, false)
    }

    fn observe(&self, label: &'static str, expected: &str, rate: bool, expanded: bool) -> Check {
        let matches = |title: &str| {
            if rate {
                title.starts_with(&format!("{expected} · ")) && title.ends_with("/s")
            } else {
                title == expected
            }
        };
        self.trace(label, "title-wait");
        let timer_date = || {
            self.owner
                .ivars()
                .timer
                .get()
                .map(|timer| timer.fireDate().timeIntervalSinceReferenceDate())
        };
        let prior_fire = timer_date();
        wait(label, Duration::from_secs(5), || matches(&self.title())).map_err(|e| {
            self.trace(
                label,
                if prior_fire == timer_date() {
                    "title-timeout-timer-unchanged"
                } else {
                    "title-timeout-timer-advanced"
                },
            );
            format!("{e}, title={}", self.title())
        })?;
        let title = self.title();
        if self.item.isEnabled() || self.owner.update_snapshot()["state"] != "downloading" {
            self.trace(label, "busy-assertion-failed");
            return Err(format!(
                "{label}: install action available during download/verification"
            ));
        }
        if rate
            && !title
                .split_once(" · ")
                .and_then(|(_, rate)| rate.strip_suffix(" KB/s"))
                .and_then(|rate| rate.parse::<f64>().ok())
                .is_some_and(|rate| (150. ..=600.).contains(&rate))
        {
            self.trace(label, "rate-assertion-failed");
            return Err(format!(
                "{label}: rate outside the controlled slow-transfer range: {title}"
            ));
        }
        self.trace(label, "title-verified");
        println!(
            "menu phase={label} enabled=false elapsed_ms={} title={title}",
            self.started.elapsed().as_millis()
        );
        if expanded && let Some(presentation) = &self.presentation {
            let phase = CString::new(label).unwrap();
            let title = CString::new(title).unwrap();
            if unsafe { (presentation.tracking)() } <= 0 {
                self.trace(label, "closed-before-sample");
                return Err("menu closed before presentation sample".into());
            }
            let started = unsafe { (presentation.start)(phase.as_ptr(), title.as_ptr()) };
            if started != 1 {
                self.trace(label, "sample-rejected");
                self.observation_failed.set(true);
            } else {
                let result = wait("visible menu sample", Duration::from_secs(3), || unsafe {
                    (presentation.result)() != 0 || (presentation.tracking)() <= 0
                });
                if result.is_err() {
                    self.trace(label, "sample-timeout");
                }
                if unsafe { (presentation.result)() } != 1 {
                    self.observation_failed.set(true);
                    unsafe { (presentation.expire)() };
                }
            }
            if unsafe { (presentation.tracking)() } <= 0 {
                self.trace(label, "closed-during-sample");
                return Err("menu closed before presentation sample completed".into());
            }
        }
        Ok(())
    }

    fn track(
        self: &Rc<Self>,
        phase: &'static str,
        exercise: impl FnOnce(&Self) -> Check + 'static,
    ) -> Check {
        let Some(presentation) = &self.presentation else {
            self.owner.prepare_status_menu();
            self.menu.update();
            return exercise(self);
        };
        let button = self
            .owner
            .ivars()
            .status
            .get()
            .and_then(|s| s.button(self.owner.mtm()))
            .ok_or("status button missing")?;
        let window = button.window().ok_or("status button window missing")?;
        let bounds = button.bounds();
        let point = button.convertPoint_toView(NSPoint::new(bounds.mid().x, bounds.mid().y), None);
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::RightMouseUp, point, NSEventModifierFlags::empty(),
            NSProcessInfo::processInfo().systemUptime(), window.windowNumber(), None, 0, 1, 0.,
        ).ok_or("could not construct AppKit event")?;
        let exercise = RefCell::new(Some(exercise));
        let result = Rc::new(RefCell::new(None));
        let returned = result.clone();
        let scenario = self.clone();
        let block = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
            if let Some(exercise) = exercise.borrow_mut().take() {
                let outcome = if unsafe { (scenario.presentation.as_ref().unwrap().tracking)() } > 0
                {
                    scenario.trace(phase, "driver-while-open");
                    exercise(&scenario)
                } else {
                    scenario.trace(phase, "driver-without-open-menu");
                    Err("missing NSMenuDidBeginTracking".into())
                };
                scenario.trace(
                    phase,
                    if outcome.is_ok() {
                        "exercise-ok"
                    } else {
                        "exercise-failed"
                    },
                );
                *returned.borrow_mut() = Some(outcome);
                scenario.trace(phase, "cancel-requested");
                scenario.menu.cancelTracking();
            }
        });
        let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(0.25, false, &block) };
        // Only the observer driver is registered explicitly. Product ticks use the production modes.
        let run_loop = NSRunLoop::currentRunLoop();
        unsafe {
            run_loop.addTimer_forMode(&timer, NSEventTrackingRunLoopMode);
            run_loop.addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
        self.trace(phase, "popup-enter");
        self.owner.pop_up_status_menu(&self.menu, &event, &button);
        self.trace(phase, "popup-return");
        timer.invalidate();
        if unsafe { (presentation.tracking)() } >= 0 {
            self.trace(phase, "end-not-observed");
            return Err("missing NSMenuDidEndTracking".into());
        }
        result.borrow_mut().take().ok_or_else(|| {
            self.trace(phase, "returned-before-driver");
            "menu returned before fixture tracking exercise".to_owned()
        })?
    }

    fn exercise(self: &Rc<Self>) -> Check {
        self.download()?;
        self.track("first-menu", |run| {
            run.advance_wire(false)?;
            run.observe("first-25", "下载中 25%", false, false)?;
            run.advance_wire(true)?;
            run.observe("rate-50", "下载中 50%", true, true)?;
            run.observe("stale-50", "下载中 50%", false, true)
        })?;
        self.advance_wire(false)?;
        self.observe("closed-75", "下载中 75%", false, false)?;
        self.track("reopen-menu", |run| {
            run.observe("reopened-75", "下载中 75%", false, true)
        })?;
        self.advance_wire(false)?;
        self.failure("network-failure", "合成回环下载中断")?;
        self.action()?; // Acknowledge the real failure and return to Idle.
        self.download()?;
        self.track("retry-menu", |run| {
            run.advance_wire(false)?;
            run.observe("retry-25", "下载中 25%", false, true)?;
            run.advance_wire(false)?;
            wait("second retry chunk", Duration::from_secs(4), || {
                run.title().starts_with("下载中 50%")
            })?;
            run.advance_wire(false)?;
            wait("third retry chunk", Duration::from_secs(4), || {
                run.title().starts_with("下载中 75%")
            })?;
            run.advance_wire(false)?;
            run.observe("verifying-100", VERIFY, false, true)?;
            run.finish_verification.store(true, Ordering::Release);
            Ok(())
        })?;
        self.failure("zip-failure-after-digest", "更新包不是有效的 ZIP")?;
        if self.observation_failed.get() {
            return Err(
                "tracked title verified but AX or own-window pixels were not established".into(),
            );
        }
        Ok(())
    }

    fn failure(&self, label: &str, detail: &str) -> Check {
        wait(label, Duration::from_secs(5), || {
            self.title().starts_with(FAILED)
        })
        .map_err(|e| format!("{e}, title={}", self.title()))?;
        if !self.item.isEnabled()
            || self.owner.update_snapshot()["busy"] != false
            || !self
                .item
                .toolTip()
                .is_some_and(|value| value.to_string().contains(detail))
        {
            return Err(format!("{label}: wrong failure state or reason"));
        }
        println!("menu phase={label} enabled=true reason={detail}");
        Ok(())
    }
}

pub fn run(home: &Path, aqua_library: Option<&Path>, output: &Path) -> Check {
    let mtm = MainThreadMarker::new().ok_or("requires main thread")?;
    let presentation = aqua_library
        .map(|path| Presentation::load(path, output))
        .transpose()?;
    let wire_trace = presentation
        .as_ref()
        .and_then(|p| p.trace.as_ref())
        .and_then(|file| file.try_clone().ok());
    let app = NSApplication::sharedApplication(mtm);
    if presentation.is_none() {
        if app.activationPolicy() != NSApplicationActivationPolicy::Prohibited || app.isActive() {
            return Err("detached fixture requires prohibited, inactive AppKit".into());
        }
    } else {
        println!(
            "aqua initial_activation_policy={:?}",
            app.activationPolicy()
        );
        if app.activationPolicy() != NSApplicationActivationPolicy::Accessory {
            return Err("LaunchServices did not establish the fixture's Accessory policy".into());
        }
        app.finishLaunching();
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let finish_verification = Arc::new(AtomicBool::new(false));
    let stage = app_update::Item45Stage {
        home: home.to_owned(),
        port: listener.local_addr().map_err(|e| e.to_string())?.port(),
        finish_verification: finish_verification.clone(),
    };
    let (wire, commands) = mpsc::channel();
    let started = Instant::now();
    let server = std::thread::spawn(move || {
        let result = serve(listener, commands, started, wire_trace.as_ref());
        if result.is_err() {
            trace(wire_trace.as_ref(), "wire", "server-error");
        }
        result
    });
    let (requests, releases) = mpsc::channel::<mpsc::Sender<Result<Vec<u8>, &'static str>>>();
    let release = serde_json::json!({
        "tag_name": "v99.0.0", "draft": false, "prerelease": false,
        "assets": [{"name": "VoicePort-macos-arm64.zip", "id": 45, "state": "uploaded",
                    "size": PAYLOAD.len(), "digest": format!("sha256:{}", ring::digest::digest(&ring::digest::SHA256, &PAYLOAD).as_ref().iter().map(|b| format!("{b:02x}")).collect::<String>())}]
    }).to_string().into_bytes();
    let metadata = std::thread::spawn(move || {
        for attempt in 1..=2 {
            let reply = releases
                .recv_timeout(Duration::from_secs(35))
                .map_err(|e| e.to_string())?;
            reply.send(Ok(release.clone())).map_err(|e| e.to_string())?;
            println!("metadata action check={attempt}");
        }
        Ok::<(), String>(())
    });
    let owner = Delegate::new(mtm);
    owner.configure_update_fixture(home.to_owned(), stage.installed(), requests);
    owner.configure_item45_stage(stage);
    let menu = if let Some(presentation) = &presentation {
        owner.build_menu();
        let menu = owner
            .ivars()
            .context_menu
            .get()
            .ok_or("context menu not built")?;
        let button = owner
            .ivars()
            .status
            .get()
            .and_then(|s| s.button(mtm))
            .ok_or("status button not built")?;
        button.setAccessibilityIdentifier(Some(ns_string!("voiceport.item45.status")));
        let out = CString::new(output.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
        if unsafe {
            (presentation.attach)(
                &**menu,
                &*button,
                out.as_ptr(),
                presentation.trace.as_ref().map_or(-1, AsRawFd::as_raw_fd),
            )
        } != 1
        {
            return Err("cannot attach owned-window / tracking observer".into());
        }
        menu.clone()
    } else {
        owner.build_context_menu(true)
    };
    let item = owner
        .ivars()
        .update_item
        .get()
        .ok_or("update item not built")?
        .clone();
    owner.install_tick_timer();
    let scenario = Rc::new(Scenario {
        started,
        owner,
        menu,
        item,
        wire,
        finish_verification,
        presentation,
        observation_failed: Cell::new(false),
    });
    let result = scenario.exercise();
    scenario.trace(
        "scenario",
        if result.is_ok() {
            "exercise-ok"
        } else {
            "exercise-failed"
        },
    );
    scenario.owner.ivars().timer.get().unwrap().invalidate();
    if let Some(status) = scenario.owner.ivars().status.get() {
        NSStatusBar::systemStatusBar().removeStatusItem(status);
    }
    result?;
    server
        .join()
        .map_err(|_| "fixture server panicked")?
        .map_err(|e| e.to_string())?;
    metadata.join().map_err(|_| "metadata server panicked")??;
    println!(
        "PASS item45 {} action-to-stage",
        if scenario.presentation.is_some() {
            "aqua"
        } else {
            "detached"
        }
    );
    Ok(())
}
