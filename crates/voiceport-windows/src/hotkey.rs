//! Thread-owned Win32 registration primitive, not a tray/message-loop shell.
use std::{cell::Cell, marker::PhantomData, rc::Rc};
thread_local! {static OWNED:Cell<bool> = const {Cell::new(false)};}
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
};
/// !Send/!Sync: a thread hotkey must be released on the registering thread.
pub struct Hotkey {
    id: i32,
    _thread: PhantomData<Rc<()>>,
}
#[derive(Debug)]
pub struct Conflict;
impl Hotkey {
    pub fn register() -> Result<Self, Conflict> {
        // A single reserved application id; caller must run a native message loop.
        let id = 0x5650;
        if OWNED.get() {
            return Err(Conflict);
        }
        if unsafe {
            RegisterHotKey(
                std::ptr::null_mut(),
                id,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                0x20,
            )
        } == 0
        {
            return Err(Conflict);
        }
        OWNED.set(true);
        Ok(Self {
            id,
            _thread: PhantomData,
        })
    }
}
impl Drop for Hotkey {
    fn drop(&mut self) {
        unsafe {
            if UnregisterHotKey(std::ptr::null_mut(), self.id) != 0 {
                OWNED.set(false);
            }
        }
    }
}
