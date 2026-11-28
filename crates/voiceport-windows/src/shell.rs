//! Native shell. The callback only records bounded, coalesced action bits;
//! Rust state remains on the message-loop stack and is never an FFI raw pointer.
use crate::{
    actions::{Action, Actions, ManualClipboard},
    hotkey::Hotkey,
};
use std::{
    cell::Cell,
    mem::size_of,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{DataExchange::*, LibraryLoader::*, Memory::*},
    UI::{Shell::*, WindowsAndMessaging::*},
};
const TRAY: u32 = WM_APP + 1;
const SHOW_MENU: u32 = 8;
thread_local! {static PENDING:Cell<u32>=const {Cell::new(0)};}
fn queue(id: u32) {
    if id < 32 {
        PENDING.set(PENDING.get() | (1 << id));
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
unsafe extern "system" fn callback(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let guarded = std::panic::catch_unwind(|| match msg {
        WM_CLOSE => {
            queue(Action::Exit as u32);
            0
        }
        WM_TIMER => {
            queue(Action::Poll as u32);
            0
        }
        TRAY => {
            if l as u32 == WM_RBUTTONUP || l as u32 == WM_LBUTTONUP {
                queue(SHOW_MENU);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    });
    match guarded {
        Ok(v) => v,
        Err(_) => {
            unsafe { PostQuitMessage(1) };
            0
        }
    }
}
struct Clipboard(HWND);
impl ManualClipboard for Clipboard {
    fn copy(&mut self, text: &str) -> Result<(), String> {
        unsafe {
            let data = wide(text);
            if data.len() > 1_000_001 {
                return Err("Clipboard unavailable".into());
            }
            let block = GlobalAlloc(GMEM_MOVEABLE, data.len() * 2);
            if block.is_null() {
                return Err("Clipboard unavailable".into());
            }
            let memory = GlobalLock(block);
            if memory.is_null() {
                GlobalFree(block);
                return Err("Clipboard unavailable".into());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), memory.cast::<u16>(), data.len());
            GlobalUnlock(block);
            if OpenClipboard(self.0) == 0 {
                GlobalFree(block);
                return Err("Clipboard unavailable".into());
            }
            let success = EmptyClipboard() != 0 && !SetClipboardData(13, block).is_null();
            CloseClipboard();
            if !success {
                GlobalFree(block);
                return Err("Clipboard unavailable".into());
            }
            Ok(())
        }
    }
}
struct Window {
    hwnd: HWND,
    instance: HINSTANCE,
    class: Vec<u16>,
    notify: NOTIFYICONDATAW,
}
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &self.notify);
            KillTimer(self.hwnd, 1);
            DestroyWindow(self.hwnd);
            UnregisterClassW(self.class.as_ptr(), self.instance);
        }
    }
}
fn menu(hwnd: HWND, actions: &Actions) -> u32 {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        for (action, text, enabled) in [
            (Action::Start, "Start dictation", actions.can_start()),
            (Action::Stop, "Stop and transcribe", actions.can_stop()),
            (Action::Cancel, "Cancel", true),
            (Action::Copy, "Copy result manually", actions.can_copy()),
            (Action::Retry, "Retry transcription", actions.can_retry()),
            (
                Action::RecoverCapture,
                "Recover capture",
                actions.can_recover_capture(),
            ),
            (Action::Exit, "Exit", true),
        ] {
            if AppendMenuW(
                menu,
                MF_STRING | if enabled { 0 } else { MF_GRAYED },
                action as usize,
                wide(text).as_ptr(),
            ) == 0
            {
                DestroyMenu(menu);
                return 0;
            }
        }
        let mut point = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) == 0 {
            DestroyMenu(menu);
            return 0;
        }
        SetForegroundWindow(hwnd);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY,
            point.x,
            point.y,
            0,
            hwnd,
            null(),
        ) as u32;
        DestroyMenu(menu);
        // Finish notification-area menu tracking even when no item was selected.
        PostMessageW(hwnd, WM_NULL, 0, 0);
        selected
    }
}
/// Only explicit --run enters here. Help/smoke never register any native object.
pub fn run() -> Result<(), String> {
    unsafe {
        let class = wide("VoicePort.Windows.v1");
        let instance = GetModuleHandleW(null());
        if instance.is_null() {
            return Err("Native shell unavailable".into());
        }
        let definition = WNDCLASSW {
            lpfnWndProc: Some(callback),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&definition) == 0 {
            return Err("Native shell unavailable".into());
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("VoicePort").as_ptr(),
            0,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            UnregisterClassW(class.as_ptr(), instance);
            return Err("Native shell unavailable".into());
        }
        let mut notify: NOTIFYICONDATAW = std::mem::zeroed();
        notify.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        notify.hWnd = hwnd;
        notify.uID = 1;
        notify.uFlags = NIF_MESSAGE | NIF_TIP | NIF_ICON;
        notify.uCallbackMessage = TRAY;
        notify.hIcon = LoadIconW(null_mut(), IDI_APPLICATION);
        let tip = wide("VoicePort");
        notify.szTip[..tip.len()].copy_from_slice(&tip);
        let mut window = Window {
            hwnd,
            instance,
            class,
            notify,
        };
        if Shell_NotifyIconW(NIM_ADD, &window.notify) == 0 {
            return Err("Tray unavailable".into());
        }
        if SetTimer(hwnd, 1, 100, None) == 0 {
            return Err("Native timer unavailable".into());
        }
        #[cfg(not(feature = "service"))]
        let mut actions = Actions::default();
        #[cfg(feature = "service")]
        let mut actions = Actions::with_capture(
            crate::executor::Executor::new(std::sync::Arc::new(
                crate::executor::CoreService::default(),
            )),
            crate::capture::owner::NativeFactory::default,
        );
        let hotkey = if actions.can_start() {
            Some(Hotkey::register())
        } else {
            None
        };
        let hotkey_registered = hotkey.as_ref().is_some_and(Result::is_ok);
        let hotkey_conflict = hotkey.as_ref().is_some_and(Result::is_err);
        let mut clipboard = Clipboard(hwnd);
        let mut message: MSG = std::mem::zeroed();
        loop {
            let got = GetMessageW(&mut message, null_mut(), 0, 0);
            if got == 0 {
                if actions.act(Action::Exit, &mut clipboard) {
                    break;
                }
                // WM_QUIT is consumed; the still-owned timer continues normal
                // stop polling while a synchronous worker may remain live.
                continue;
            }
            if got < 0 {
                return Err("Native message loop failed".into());
            }
            if message.message == WM_HOTKEY
                && message.hwnd.is_null()
                && message.wParam == Hotkey::ID as usize
                && hotkey_registered
            {
                queue(Action::ToggleCapture as u32);
            } else {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let mut pending = PENDING.replace(0);
            if pending & (1 << SHOW_MENU) != 0
                && !Action::Exit.queued(pending)
                && !Action::Cancel.queued(pending)
            {
                let selected = menu(hwnd, &actions);
                if selected != 0 {
                    queue(selected);
                }
                // TrackPopupMenu can dispatch window messages before returning.
                pending |= PENDING.replace(0);
            }
            let exit = actions.dispatch(pending, &mut clipboard);
            let status = if hotkey_conflict {
                format!("{}; Ctrl+Alt+Space unavailable", actions.status)
            } else {
                actions.status.clone()
            };
            SetWindowTextW(hwnd, wide(&status).as_ptr());
            window.notify.szTip.fill(0);
            let tip: Vec<u16> = status.encode_utf16().take(127).collect();
            window.notify.szTip[..tip.len()].copy_from_slice(&tip);
            Shell_NotifyIconW(NIM_MODIFY, &window.notify);
            if exit {
                break;
            }
        }
        drop(hotkey);
        drop(window);
        Ok(())
    }
}
