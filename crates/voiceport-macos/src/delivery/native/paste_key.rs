//! Resolve the physical Command+V key without selecting an input source.
use core_foundation::{
    base::{CFGetTypeID, CFType, CFTypeRef, TCFType},
    data::{CFDataGetBytePtr, CFDataGetLength, CFDataGetTypeID},
    string::CFStringRef,
};
use std::ffi::c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFStringRef) -> CFTypeRef;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn LMGetKbdType() -> u8;
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn UCKeyTranslate(
        layout: *const c_void,
        key: u16,
        action: u16,
        modifiers: u32,
        keyboard_type: u32,
        options: u32,
        dead_state: *mut u32,
        capacity: usize,
        length: *mut usize,
        units: *mut u16,
    ) -> i32;
}

pub(super) fn current() -> Option<u16> {
    // TIS and LMGetKbdType must run on the application's main thread.
    let _main_thread = objc2::MainThreadMarker::new()?;
    let source = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    if source.is_null() {
        return None;
    }
    let source = unsafe { CFType::wrap_under_create_rule(source) };
    for_source(&source, u32::from(unsafe { LMGetKbdType() }))
}

pub(super) fn for_source(source: &CFType, keyboard_type: u32) -> Option<u16> {
    let data = unsafe {
        TISGetInputSourceProperty(source.as_CFTypeRef(), kTISPropertyUnicodeKeyLayoutData)
    };
    if data.is_null() || unsafe { CFGetTypeID(data) != CFDataGetTypeID() } {
        return None;
    }
    // The source owns this borrowed CFData for the whole translation loop.
    if unsafe { CFDataGetLength(data.cast()) } <= 0 {
        return None;
    }
    let layout = unsafe { CFDataGetBytePtr(data.cast()) };
    if layout.is_null() {
        return None;
    }
    (0..128).find(|&key| {
        let mut dead_state = 0;
        let mut length = 0;
        let mut units = [0u16; 4];
        let status = unsafe {
            UCKeyTranslate(
                layout.cast(),
                key,
                0, // kUCKeyActionDown
                1, // Carbon cmdKey >> 8 (also handles Dvorak-QWERTY Command)
                keyboard_type,
                1, // kUCKeyTranslateNoDeadKeysMask
                &mut dead_state,
                units.len(),
                &mut length,
                units.as_mut_ptr(),
            )
        };
        status == 0 && length == 1 && matches!(units[0], 0x76 | 0x56)
    })
}
