//! Carbon global shortcut registration. Construction never installs a handler.
use crate::{delivery::model::ApplicationTarget, shortcut::Shortcut};
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use std::{
    cell::Cell,
    ffi::c_void,
    marker::PhantomData,
    ptr,
    rc::Rc,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::Sender,
    },
    time::Instant,
};

use voiceport_control::readiness::Hotkey;

const EVENT_NOT_HANDLED: i32 = -9874;
// eventHotKeyExistsErr in the HIToolbox CarbonEventsCore header.
const HOTKEY_EXISTS: i32 = -9878;
const SIGNATURE: u32 = u32::from_be_bytes(*b"VPOR");
static NEXT_ID: AtomicU32 = AtomicU32::new(1);
const PRESSED: u32 = 5;
const RELEASED: u32 = 6;
const EXCLUSIVE: u32 = 1; // kEventHotKeyExclusive

#[repr(C)]
struct EventTypeSpec {
    class: u32,
    kind: u32,
}

#[repr(C)]
struct HotKeyId {
    signature: u32,
    id: u32,
}

type NativeRef = *mut c_void;
type Handler = unsafe extern "C" fn(NativeRef, NativeRef, *mut c_void) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> NativeRef;
    fn InstallEventHandler(
        target: NativeRef,
        handler: Handler,
        count: usize,
        types: *const EventTypeSpec,
        context: *mut c_void,
        result: *mut NativeRef,
    ) -> i32;
    fn RemoveEventHandler(handler: NativeRef) -> i32;
    fn RegisterEventHotKey(
        key: u32,
        modifiers: u32,
        identity: HotKeyId,
        target: NativeRef,
        options: u32,
        result: *mut NativeRef,
    ) -> i32;
    fn UnregisterEventHotKey(hotkey: NativeRef) -> i32;
    fn GetEventKind(event: NativeRef) -> u32;
    fn GetEventTime(event: NativeRef) -> f64;
    fn GetCurrentEventTime() -> f64;
    fn GetEventParameter(
        event: NativeRef,
        name: u32,
        desired_type: u32,
        actual_type: *mut u32,
        size: usize,
        actual_size: *mut usize,
        data: *mut c_void,
    ) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
}

enum Listener {
    Events(Sender<HotKeyEvent>),
    Press(Box<dyn Fn(KeyPress)>),
}

struct Context {
    listener: Listener,
    id: u32,
    edges: Cell<Edges>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stroke {
    registration: u32,
    sequence: u64,
}

impl Stroke {
    pub(crate) fn fixture_stroke(sequence: u64) -> Self {
        Self {
            registration: 1,
            sequence,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Press(Stroke),
    Release(Stroke),
}

/// Tracks one physical stroke per registration. Source timestamps exclude
/// delayed pre-sample edges; the token excludes events from replaced bindings.
#[derive(Clone, Copy, Default)]
struct Edges {
    watermark: Option<f64>,
    sequence: u64,
    active: Option<Stroke>,
    wait_for_up: bool,
}

impl Edges {
    fn transition(&mut self, registration: u32, kind: u32, at: f64) -> Option<Edge> {
        if !at.is_finite() || self.watermark.is_some_and(|old| at <= old) {
            return None;
        }
        self.watermark = Some(at);
        if self.wait_for_up {
            if kind == RELEASED {
                self.wait_for_up = false;
            }
            return None;
        }
        match kind {
            PRESSED if self.active.is_none() => {
                self.sequence = self.sequence.checked_add(1)?;
                let stroke = Stroke {
                    registration,
                    sequence: self.sequence,
                };
                self.active = Some(stroke);
                Some(Edge::Press(stroke))
            }
            RELEASED => self.active.take().map(Edge::Release),
            _ => None,
        }
    }

    fn reconcile(&mut self, registration: u32, key_down: bool, at: f64) -> Option<Edge> {
        if (self.active.is_none() && !self.wait_for_up) || key_down {
            return None;
        }
        self.transition(registration, RELEASED, at)
    }
}

pub(crate) enum HotKeyEvent {
    Press(KeyPress),
    Release(Stroke),
}

pub(crate) struct KeyPress {
    pub stroke: Stroke,
    pub at: Instant,
    pub application: Option<Retained<NSRunningApplication>>,
    pub target: Option<ApplicationTarget>,
}

impl KeyPress {
    fn capture(stroke: Stroke, native_at: f64) -> Self {
        let now = Instant::now();
        let elapsed = (unsafe { GetCurrentEventTime() } - native_at).max(0.0);
        // A stalled dispatch must not make an old native press look fresh.
        let at = now - std::time::Duration::from_secs_f64(elapsed.min(60.0));
        let application = (unsafe { libc::pthread_main_np() } == 1)
            .then(|| NSWorkspace::sharedWorkspace().frontmostApplication())
            .flatten();
        let target = application.as_ref().and_then(|app| {
            Some(ApplicationTarget {
                pid: app.processIdentifier(),
                bundle: app.bundleIdentifier()?.to_string(),
            })
        });
        Self {
            stroke,
            at,
            application,
            target,
        }
    }
}

#[derive(Debug)]
pub(crate) enum RegistrationError {
    MainThread,
    Handler(i32),
    Shortcut(i32),
}

impl RegistrationError {
    pub(crate) fn state(&self) -> Hotkey {
        match self {
            Self::Shortcut(HOTKEY_EXISTS) => Hotkey::Conflict,
            _ => Hotkey::RegistrationFailed,
        }
    }
}

#[cfg(test)]
#[test]
fn only_a_shortcut_registration_collision_is_reported_as_conflict() {
    assert_eq!(
        RegistrationError::Shortcut(HOTKEY_EXISTS).state(),
        Hotkey::Conflict
    );
    assert_eq!(
        RegistrationError::Handler(HOTKEY_EXISTS).state(),
        Hotkey::RegistrationFailed
    );
    assert_eq!(
        RegistrationError::Shortcut(-9879).state(),
        Hotkey::RegistrationFailed
    );
    assert_eq!(
        RegistrationError::MainThread.state(),
        Hotkey::RegistrationFailed
    );
}

impl std::fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MainThread => write!(f, "全局快捷键必须在主线程注册。"),
            Self::Handler(status) => write!(f, "无法安装全局快捷键处理器（{status}）。"),
            Self::Shortcut(HOTKEY_EXISTS) => write!(f, "全局快捷键已被占用。"),
            Self::Shortcut(status) => write!(f, "无法注册全局快捷键（{status}）。"),
        }
    }
}

/// Register, unregister and drop on the main thread. The stable boxed context
/// outlives Carbon's handler even when the Rust owner moves on that thread.
pub struct GlobalHotKey {
    reference: NativeRef,
    handler: NativeRef,
    context: Box<Context>,
    _main_thread: PhantomData<Rc<()>>,
}

impl GlobalHotKey {
    pub fn new(sender: Sender<HotKeyEvent>) -> Self {
        Self::with_listener(Listener::Events(sender))
    }

    pub(crate) fn on_press(action: impl Fn(KeyPress) + 'static) -> Self {
        Self::with_listener(Listener::Press(Box::new(action)))
    }

    fn with_listener(listener: Listener) -> Self {
        Self {
            reference: ptr::null_mut(),
            handler: ptr::null_mut(),
            context: Box::new(Context {
                listener,
                id: 0,
                edges: Cell::default(),
            }),
            _main_thread: PhantomData,
        }
    }

    /// Only validated bindings can reach Carbon. Use a separate owner when
    /// replacing a live binding so failure leaves the previous one installed.
    pub(crate) fn register(&mut self, shortcut: Shortcut) -> Result<(), RegistrationError> {
        if unsafe { libc::pthread_main_np() } != 1 {
            return Err(RegistrationError::MainThread);
        }
        self.unregister();
        self.context.id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let event_types = [PRESSED, RELEASED].map(|kind| EventTypeSpec {
            class: u32::from_be_bytes(*b"keyb"),
            kind,
        });
        let target = unsafe { GetApplicationEventTarget() };
        let status = unsafe {
            InstallEventHandler(
                target,
                handle_edge,
                event_types.len(),
                event_types.as_ptr(),
                (&mut *self.context as *mut Context).cast(),
                &mut self.handler,
            )
        };
        if status != 0 {
            self.unregister();
            return Err(RegistrationError::Handler(status));
        }
        let status = unsafe {
            RegisterEventHotKey(
                shortcut.key_code(),
                shortcut.modifiers(),
                HotKeyId {
                    signature: SIGNATURE,
                    id: self.context.id,
                },
                target,
                EXCLUSIVE,
                &mut self.reference,
            )
        };
        if status != 0 {
            self.unregister();
            return Err(RegistrationError::Shortcut(status));
        }
        self.context.edges.set(Edges {
            watermark: Some(unsafe { GetCurrentEventTime() }),
            wait_for_up: unsafe { CGEventSourceKeyState(1, shortcut.key_code() as u16) },
            ..Edges::default()
        });
        Ok(())
    }

    pub fn unregister(&mut self) {
        self.context.edges.set(Edges::default());
        if !self.reference.is_null() {
            unsafe { UnregisterEventHotKey(self.reference) };
            self.reference = ptr::null_mut();
        }
        if !self.handler.is_null() {
            unsafe { RemoveEventHandler(self.handler) };
            self.handler = ptr::null_mut();
        }
    }

    pub(crate) fn active_stroke(&self, shortcut: Shortcut) -> Option<Stroke> {
        let mut edges = self.context.edges.get();
        if edges.active.is_some() || edges.wait_for_up {
            // Sample only the registered non-modifier key while a stroke is
            // open; no monitor, event tap or user keystroke contents are read.
            let down = unsafe { CGEventSourceKeyState(1, shortcut.key_code() as u16) };
            let at = unsafe { GetCurrentEventTime() };
            if let Some(Edge::Release(stroke)) = edges.reconcile(self.context.id, down, at)
                && let Listener::Events(sender) = &self.context.listener
            {
                let _ = sender.send(HotKeyEvent::Release(stroke));
            }
            self.context.edges.set(edges);
        }
        edges.active
    }
}

impl Drop for GlobalHotKey {
    fn drop(&mut self) {
        self.unregister();
    }
}

unsafe extern "C" fn handle_edge(_: NativeRef, event: NativeRef, context: *mut c_void) -> i32 {
    if event.is_null() || context.is_null() {
        return EVENT_NOT_HANDLED;
    }
    let mut identity = HotKeyId {
        signature: 0,
        id: 0,
    };
    let status = unsafe {
        GetEventParameter(
            event,
            u32::from_be_bytes(*b"----"),
            u32::from_be_bytes(*b"hkid"),
            ptr::null_mut(),
            std::mem::size_of::<HotKeyId>(),
            ptr::null_mut(),
            (&mut identity as *mut HotKeyId).cast(),
        )
    };
    // Carbon invokes the handler on the application event thread. It cannot
    // outlive Context: unregister removes the handler before Context is dropped.
    let context = unsafe { &*context.cast::<Context>() };
    if status != 0 || identity.signature != SIGNATURE || identity.id != context.id {
        return EVENT_NOT_HANDLED;
    }
    let at = unsafe { GetEventTime(event) };
    let mut edges = context.edges.get();
    let edge = edges.transition(context.id, unsafe { GetEventKind(event) }, at);
    context.edges.set(edges);
    if let Some(edge) = edge {
        // Bind the application here, before the AppKit timer may process a
        // later activation. A release never recaptures the application.
        match (&context.listener, edge) {
            (Listener::Events(sender), edge) => {
                let _ = sender.send(match edge {
                    Edge::Press(stroke) => HotKeyEvent::Press(KeyPress::capture(stroke, at)),
                    Edge::Release(stroke) => HotKeyEvent::Release(stroke),
                });
            }
            (Listener::Press(action), Edge::Press(stroke)) => action(KeyPress::capture(stroke, at)),
            (Listener::Press(_), Edge::Release(_)) => {}
        }
    }
    0
}

#[test]
fn shortcut_edges_ignore_repeats_missed_release_and_late_edges() {
    let mut edges = Edges::default();
    let Some(Edge::Press(first)) = edges.transition(1, PRESSED, 1.) else {
        panic!("press")
    };
    assert_eq!(edges.transition(1, PRESSED, 2.), None);
    assert_eq!(edges.reconcile(1, true, 3.), None);
    assert_eq!(edges.reconcile(1, false, 4.), Some(Edge::Release(first)));
    assert_eq!(edges.transition(1, PRESSED, 3.5), None);
    let Some(Edge::Press(second)) = edges.transition(1, PRESSED, 5.) else {
        panic!("second press")
    };
    assert_ne!(first, second);
    assert_eq!(edges.transition(1, RELEASED, 4.5), None);
    assert_eq!(edges.active, Some(second));
    assert_eq!(
        edges.transition(1, RELEASED, 6.),
        Some(Edge::Release(second))
    );
    assert_eq!(edges.transition(1, RELEASED, 7.), None);
    assert_ne!(
        Edges::default().transition(2, PRESSED, 1.),
        Some(Edge::Press(first))
    );
}

#[test]
fn shortcut_registration_while_key_is_down_requires_a_fresh_press() {
    let mut edges = Edges {
        wait_for_up: true,
        ..Edges::default()
    };
    assert_eq!(edges.transition(1, PRESSED, 1.), None);
    assert_eq!(edges.reconcile(1, false, 2.), None);
    assert!(matches!(
        edges.transition(1, PRESSED, 3.),
        Some(Edge::Press(_))
    ));
}
