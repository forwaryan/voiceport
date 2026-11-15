//! WeChat's unreadable chat window can still expose native Edit/Paste actions.
//! This path posts no keyboard/mouse events and never claims text readback.
use super::*;
use core_foundation::{
    array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex},
    base::CFRetain,
};

const MENU_WAIT: Duration = Duration::from_millis(800);
const MENU_TURN: Duration = Duration::from_millis(50);

fn elements(element: &OwnedCf) -> Result<Vec<OwnedCf>, Failure> {
    let array = match attribute(element.0, "AXChildren") {
        Ok(array) => array,
        Err(AX_NO_VALUE) => return Ok(Vec::new()),
        Err(_) => return Err(Failure::Menu),
    };
    unsafe {
        if CFGetTypeID(array.0) != CFArrayGetTypeID() {
            return Err(Failure::Menu);
        }
        let count = CFArrayGetCount(array.0.cast());
        if !(0..=64).contains(&count) {
            return Err(Failure::Menu);
        }
        (0..count)
            .map(|i| {
                let raw = CFArrayGetValueAtIndex(array.0.cast(), i);
                if raw.is_null() || CFGetTypeID(raw) != AXUIElementGetTypeID() {
                    return Err(Failure::Menu);
                }
                let owned = OwnedCf::new(CFRetain(raw)).ok_or(Failure::Menu)?;
                if AXUIElementSetMessagingTimeout(owned.0, 0.25) != 0 {
                    return Err(Failure::Menu);
                }
                Ok(owned)
            })
            .collect()
    }
}

fn text(element: &OwnedCf, key: &str) -> Result<String, Failure> {
    attribute(element.0, key)
        .ok()
        .and_then(|value| value.string(256))
        .ok_or(Failure::Menu)
}

fn boolean(element: &OwnedCf, key: &str) -> Result<bool, Failure> {
    let value = attribute(element.0, key).map_err(|_| Failure::Menu)?;
    unsafe {
        if CFGetTypeID(value.0) != CFBooleanGetTypeID() {
            return Err(Failure::Menu);
        }
        Ok(CFEqual(value.0, kCFBooleanTrue.cast()) != 0)
    }
}

fn edit_title(title: &str) -> bool {
    matches!(title, "编辑" | "編輯" | "Edit")
}
fn paste_title(title: &str) -> bool {
    matches!(title, "粘贴" | "粘貼" | "貼上" | "Paste")
}
fn main_title(title: &str) -> bool {
    matches!(title, "微信" | "WeChat" | "Weixin")
}

fn edit_menu(app: &OwnedCf) -> Result<OwnedCf, Failure> {
    let bar = attribute(app.0, "AXMenuBar").map_err(|_| Failure::Menu)?;
    let mut found = None;
    for item in elements(&bar)? {
        if text(&item, "AXRole")? == "AXMenuBarItem" && edit_title(&text(&item, "AXTitle")?) {
            if found.is_some() {
                return Err(Failure::Menu);
            }
            found = Some(item);
        }
    }
    found.ok_or(Failure::Menu)
}

fn paste_menu(edit: &OwnedCf) -> Result<Option<OwnedCf>, Failure> {
    let mut found = None;
    for menu in elements(edit)? {
        if text(&menu, "AXRole")? != "AXMenu" {
            continue;
        }
        for item in elements(&menu)? {
            // Separators are allowed to omit titles; never select by position.
            if text(&item, "AXRole").ok().as_deref() != Some("AXMenuItem") {
                continue;
            }
            if text(&item, "AXTitle")
                .ok()
                .as_deref()
                .is_some_and(paste_title)
            {
                if found.is_some() {
                    return Err(Failure::Menu);
                }
                found = Some(item);
            }
        }
    }
    Ok(found)
}

fn action(element: &OwnedCf, name: &str) -> Result<(), Failure> {
    let name = CFString::new(name);
    if unsafe { AXUIElementPerformAction(element.0, name.as_CFTypeRef()) } == 0 {
        Ok(())
    } else {
        // AX timeouts can mean an action did happen. Never retry it.
        Err(Failure::PasteRequest)
    }
}

pub(super) struct WechatMenuTarget {
    pid: libc::pid_t,
    application: OwnedCf,
    window: OwnedCf,
    edit: OwnedCf,
    monitor: UserMonitor,
    initial_clipboard: isize,
    owned_clipboard: Option<isize>,
    opened_at: Option<Instant>,
    _main_only: std::marker::PhantomData<Rc<()>>,
}

impl WechatMenuTarget {
    pub(super) fn capture(pid: libc::pid_t, monitor: UserMonitor) -> Result<Self, Failure> {
        if !main_thread() {
            return Err(Failure::MainThread);
        }
        let application =
            OwnedCf::new(unsafe { AXUIElementCreateApplication(pid) }).ok_or(Failure::Focus)?;
        if unsafe { AXUIElementSetMessagingTimeout(application.0, 0.25) } != 0 {
            return Err(Failure::Menu);
        }
        // Do not substitute AXMainWindow: a call/login/dialog may be focused.
        let window = attribute(application.0, "AXFocusedWindow").map_err(Failure::FocusRead)?;
        if unsafe { CFGetTypeID(window.0) != AXUIElementGetTypeID() } {
            return Err(Failure::Focus);
        }
        if unsafe { AXUIElementSetMessagingTimeout(window.0, 0.25) } != 0 {
            return Err(Failure::Menu);
        }
        let edit = edit_menu(&application)?;
        let target = Self {
            pid,
            application,
            window,
            edit,
            monitor,
            initial_clipboard: NSPasteboard::generalPasteboard().changeCount(),
            owned_clipboard: None,
            opened_at: None,
            _main_only: std::marker::PhantomData,
        };
        target.validate()?;
        Ok(target)
    }

    fn validate(&self) -> Result<(), Failure> {
        if !main_thread() {
            return Err(Failure::MainThread);
        }
        if !trusted() {
            return Err(Failure::Accessibility);
        }
        if secure_input() {
            return Err(Failure::SecureInput);
        }
        self.monitor.check()?;
        let (pid, bundle) = frontmost()?;
        if pid != self.pid || bundle != WECHAT {
            return Err(Failure::FocusApplicationChanged);
        }
        let current =
            attribute(self.application.0, "AXFocusedWindow").map_err(Failure::FocusRead)?;
        if unsafe { CFEqual(current.0, self.window.0) } == 0 {
            return Err(Failure::FocusElementChanged);
        }
        if text(&self.window, "AXRole")? != "AXWindow"
            || !main_title(&text(&self.window, "AXTitle")?)
        {
            return Err(Failure::Role);
        }
        match attribute(self.window.0, "AXModal") {
            Ok(value) => unsafe {
                if CFGetTypeID(value.0) != CFBooleanGetTypeID()
                    || CFEqual(value.0, kCFBooleanTrue.cast()) != 0
                {
                    return Err(Failure::Role);
                }
            },
            Err(AX_UNSUPPORTED | AX_NO_VALUE) => {}
            Err(_) => return Err(Failure::Menu),
        }
        let expected = self.owned_clipboard.unwrap_or(self.initial_clipboard);
        if expected < 0 || NSPasteboard::generalPasteboard().changeCount() != expected {
            return Err(Failure::ClipboardChanged);
        }
        Ok(())
    }
}

impl PasteTarget for WechatMenuTarget {
    fn check(&mut self) -> Result<(), Failure> {
        self.validate()
    }

    fn prepare(&mut self, text: &str) -> Result<(), Failure> {
        if self.owned_clipboard.is_some() || self.opened_at.is_some() {
            return Err(Failure::Ended);
        }
        self.validate()?;
        let board = NSPasteboard::generalPasteboard();
        let count = board.clearContents();
        self.owned_clipboard = Some(count);
        if count < 0 || board.changeCount() != count {
            return Err(Failure::ClipboardChanged);
        }
        if !unsafe { board.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) } {
            return Err(Failure::Clipboard);
        }
        self.validate()?;
        self.edit = edit_menu(&self.application)?;
        self.opened_at = Some(Instant::now());
        action(&self.edit, "AXPress")
    }

    fn ready(&mut self) -> Result<bool, Failure> {
        self.validate()?;
        let elapsed = self.opened_at.ok_or(Failure::Menu)?.elapsed();
        if elapsed >= MENU_WAIT {
            return Err(Failure::Menu);
        }
        if elapsed < MENU_TURN {
            return Ok(false);
        }
        let Some(paste) = paste_menu(&self.edit)? else {
            return Ok(false);
        };
        if self.opened_at.ok_or(Failure::Menu)?.elapsed() >= MENU_WAIT {
            return Err(Failure::Menu);
        }
        if boolean(&paste, "AXEnabled")? {
            return Ok(true);
        }
        Ok(false)
    }

    fn paste(&mut self) -> Result<(), Failure> {
        self.validate()?;
        if self.opened_at.ok_or(Failure::Menu)?.elapsed() >= MENU_WAIT {
            return Err(Failure::Menu);
        }
        let paste = paste_menu(&self.edit)?.ok_or(Failure::Menu)?;
        if !boolean(&paste, "AXEnabled")? {
            return Err(Failure::Menu);
        }
        self.validate()?;
        // Mark the menu consumed before dispatch: a timeout is not proof that
        // Paste did not happen. No retry, rollback or simulated key follows.
        self.opened_at = None;
        action(&paste, "AXPress")
    }

    fn abandon(&mut self) {
        if self.opened_at.take().is_some()
            && self.monitor.check().is_ok()
            && frontmost().is_ok_and(|(pid, bundle)| pid == self.pid && bundle == WECHAT)
            && attribute(self.application.0, "AXFocusedWindow")
                .is_ok_and(|window| unsafe { CFEqual(window.0, self.window.0) } != 0)
        {
            let _ = action(&self.edit, "AXCancel");
        }
        // Final text stays in the clipboard. Never read/restore old contents or
        // overwrite a newer clipboard owner during cleanup.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_exact_native_paste_and_main_window_titles_are_accepted() {
        for title in ["Paste", "粘贴", "貼上"] {
            assert!(paste_title(title));
        }
        for title in ["Paste and Match Style", "粘贴并发送", "发送", ""] {
            assert!(!paste_title(title));
        }
        for title in ["微信", "WeChat", "Weixin"] {
            assert!(main_title(title));
        }
        for title in ["视频聊天", "登录", "微信登录", ""] {
            assert!(!main_title(title));
        }
    }
}
