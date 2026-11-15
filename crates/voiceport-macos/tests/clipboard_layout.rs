//! Read installed layouts on the OS main thread. Never select a layout or post keys.
#[cfg(target_os = "macos")]
#[path = "../src/delivery/native/paste_key.rs"]
mod paste_key;

#[cfg(target_os = "macos")]
fn main() {
    use core_foundation::{
        array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef},
        base::{CFType, CFTypeRef, TCFType},
        string::{CFString, CFStringRef},
    };
    use std::ptr;
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCreateInputSourceList(properties: CFTypeRef, all_installed: u8) -> CFArrayRef;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, key: CFStringRef) -> CFTypeRef;
        static kTISPropertyInputSourceID: CFStringRef;
        fn LMGetKbdType() -> u8;
    }
    let _main_thread = objc2::MainThreadMarker::new().expect("requires the OS main thread");
    let before = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    assert!(!before.is_null());
    let before = unsafe { CFType::wrap_under_create_rule(before) };
    assert!(
        paste_key::current().is_some(),
        "current layout has no Command+V"
    );
    let list = unsafe { TISCreateInputSourceList(ptr::null(), 1) };
    assert!(!list.is_null());
    let _owner = unsafe { CFType::wrap_under_create_rule(list.cast()) };
    let keyboard_type = u32::from(unsafe { LMGetKbdType() });
    let mut checked = Vec::new();
    for index in 0..unsafe { CFArrayGetCount(list) } {
        let source = unsafe { CFArrayGetValueAtIndex(list, index) };
        let id = unsafe { TISGetInputSourceProperty(source, kTISPropertyInputSourceID) };
        if id.is_null() {
            continue;
        }
        let id = unsafe { CFString::wrap_under_get_rule(id.cast()) }.to_string();
        let expected = match id.as_str() {
            "com.apple.keylayout.US" => 9,
            "com.apple.keylayout.Dvorak" => 47,
            "com.apple.keylayout.DVORAK-QWERTYCMD" => 9,
            _ => continue,
        };
        let source = unsafe { CFType::wrap_under_get_rule(source) };
        assert_eq!(
            paste_key::for_source(&source, keyboard_type),
            Some(expected),
            "{id}"
        );
        println!("{id}: Command+V key={expected}");
        checked.push(id);
    }
    assert_eq!(
        checked.len(),
        3,
        "required built-in layouts not found: {checked:?}"
    );
    let after = unsafe { TISCopyCurrentKeyboardLayoutInputSource() };
    assert!(!after.is_null());
    let after = unsafe { CFType::wrap_under_create_rule(after) };
    assert_eq!(before, after, "test must not change the active layout");
    println!(
        "clipboard_layout: 3 installed layouts passed; no selection, input or clipboard writes"
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("clipboard_layout: macOS-only layout regression; not executed on this platform");
}
