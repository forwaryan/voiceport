//! Carbon global shortcut registration. Construction never installs a handler.
use crate::delivery::model::ApplicationTarget;
use objc2::rc::Retained;
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use std::{
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

const EVENT_NOT_HANDLED: i32 = -9874;
const SIGNATURE: u32 = u32::from_be_bytes(*b"VPOR");
const SPACE: u32 = 49;
const CONTROL: u32 = 1 << 12;
const OPTION: u32 = 1 << 11;
const SHIFT: u32 = 1 << 9;
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

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

struct Context {
    sender: Sender<HotKeyEvent>,
    id: u32,
}

pub(crate) struct HotKeyEvent {
    pub at: Instant,
    pub application: Option<Retained<NSRunningApplication>>,
    pub target: Option<ApplicationTarget>,
}

impl HotKeyEvent {
    fn capture() -> Self {
        let at = Instant::now();
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
            at,
            application,
            target,
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
        Self {
            reference: ptr::null_mut(),
            handler: ptr::null_mut(),
            context: Box::new(Context {
                sender,
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            }),
            _main_thread: PhantomData,
        }
    }

    /// The default is Ctrl+Option+Space; the alternative is Ctrl+Shift+Space.
    /// An unsuccessful registration removes the handler and leaves no hotkey.
    pub fn register(&mut self, alternative: bool) -> Result<(), String> {
        if unsafe { libc::pthread_main_np() } != 1 {
            return Err("全局快捷键必须在主线程注册。".into());
        }
        self.unregister();
        let event_type = EventTypeSpec {
            class: u32::from_be_bytes(*b"keyb"),
            kind: 5,
        };
        let target = unsafe { GetApplicationEventTarget() };
        let status = unsafe {
            InstallEventHandler(
                target,
                pressed,
                1,
                &event_type,
                (&mut *self.context as *mut Context).cast(),
                &mut self.handler,
            )
        };
        if status != 0 {
            self.unregister();
            return Err(format!("无法安装全局快捷键处理器（{status}）。"));
        }
        let status = unsafe {
            RegisterEventHotKey(
                SPACE,
                CONTROL | if alternative { SHIFT } else { OPTION },
                HotKeyId {
                    signature: SIGNATURE,
                    id: self.context.id,
                },
                target,
                0,
                &mut self.reference,
            )
        };
        if status != 0 {
            self.unregister();
            return Err(format!("无法注册全局快捷键，可能已被占用（{status}）。"));
        }
        Ok(())
    }

    pub fn unregister(&mut self) {
        if !self.reference.is_null() {
            unsafe { UnregisterEventHotKey(self.reference) };
            self.reference = ptr::null_mut();
        }
        if !self.handler.is_null() {
            unsafe { RemoveEventHandler(self.handler) };
            self.handler = ptr::null_mut();
        }
    }
}

impl Drop for GlobalHotKey {
    fn drop(&mut self) {
        self.unregister();
    }
}

unsafe extern "C" fn pressed(_: NativeRef, event: NativeRef, context: *mut c_void) -> i32 {
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
    // Bind the application here, before the AppKit timer may process a later
    // activation. No field contents or Accessibility target are read here.
    let _ = context.sender.send(HotKeyEvent::capture());
    0
}
