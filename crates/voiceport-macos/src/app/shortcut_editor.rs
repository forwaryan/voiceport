//! Shortcut changes commit only after native registration succeeds.
use super::*;
use crate::shortcut::Shortcut;

impl Delegate {
    pub(super) fn settings_locked(&self) -> bool {
        self.ivars().managing_sources.get() || self.ivars().editing_shortcut.get()
    }

    pub(super) fn change_shortcut(&self) {
        let idle = self
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .is_some_and(|c| !c.busy());
        if !idle
            || self.settings_locked()
            || self.ivars().quitting.get()
            || self.ivars().permission_rx.borrow().is_some()
            || self.ivars().pending_hotkey.borrow().is_some()
        {
            self.refresh();
            return;
        }
        let index = self
            .ivars()
            .shortcut
            .get()
            .map(|c| c.indexOfSelectedItem())
            .unwrap_or(-1);
        let candidate = match index {
            0 => Some(Shortcut::DEFAULT),
            1 => Some(Shortcut::ALTERNATIVE),
            2 => {
                self.ivars().editing_shortcut.set(true);
                // Drop both registration and queue: neither a captured key nor
                // an event queued before the editor can later start dictation.
                self.ivars().hotkey.borrow_mut().take();
                self.ivars().hotkey_rx.borrow_mut().take();
                self.with_controller(|c| c.set_stop_shortcut(None));
                self.refresh();
                let value = crate::shortcut_recorder::record(
                    self.mtm(),
                    self.ivars().shortcut_binding.get(),
                );
                self.ivars().editing_shortcut.set(false);
                value
            }
            _ => None,
        };
        if self.ivars().quitting.get() {
            self.refresh();
            return;
        }
        if let Some(candidate) = candidate {
            match self.install_hotkey(candidate) {
                Ok(()) => {
                    self.ivars().shortcut_binding.set(candidate);
                    *self.ivars().shortcut_notice.borrow_mut() =
                        Some(format!("已保存：{}", candidate.label()));
                    let defaults = NSUserDefaults::standardUserDefaults();
                    unsafe {
                        defaults.setObject_forKey(
                            Some(&NSString::from_str(
                                &serde_json::to_string(&candidate).expect("shortcut serialization"),
                            )),
                            ns_string!("dictationShortcut"),
                        );
                    }
                    defaults.setBool_forKey(
                        candidate == Shortcut::ALTERNATIVE,
                        ns_string!("alternativeShortcut"),
                    );
                    self.with_controller(|c| {
                        c.message = format!("快捷键已设为 {}。", candidate.label())
                    });
                }
                Err(error) => {
                    *self.ivars().shortcut_notice.borrow_mut() = Some(format!(
                        "无法注册；保留 {}。",
                        self.ivars().shortcut_binding.get().label()
                    ));
                    self.with_controller(|c| c.message = format!("{error} 已保留原快捷键设置。"));
                }
            }
        }
        // Cancel or failed candidate registration restores the prior binding.
        // If a different app took it during editing, report the actual failure.
        let needs_restore = self.ivars().hotkey.borrow().is_none();
        if needs_restore
            && let Err(error) = self.install_hotkey(self.ivars().shortcut_binding.get())
        {
            *self.ivars().hotkey_error.borrow_mut() = Some(error.clone());
            *self.ivars().shortcut_notice.borrow_mut() =
                Some("原快捷键也无法恢复，请重新选择。".into());
            self.with_controller(|c| c.message = format!("{error} 请重新选择快捷键。"));
        }
        self.refresh();
    }
}
