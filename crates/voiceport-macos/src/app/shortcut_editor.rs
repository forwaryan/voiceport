//! Shortcut changes commit only after native registration succeeds.
use super::*;
use crate::shortcut::{Shortcut, ShortcutMode};

impl Delegate {
    pub(super) fn settings_locked(&self) -> bool {
        self.ivars().managing_sources.get()
            || self.ivars().managing_models.get()
            || self.ivars().editing_shortcut.get()
            || self.ivars().workflow_dialog.get()
    }

    fn may_change_shortcut(&self) -> bool {
        self.ivars()
            .controller
            .borrow()
            .as_ref()
            .is_some_and(|c| !c.busy())
            && !self.settings_locked()
            && !self.ivars().quitting.get()
            && self.ivars().permission_rx.borrow().is_none()
            && self.ivars().pending_hotkey.borrow().is_none()
    }

    pub(super) fn change_shortcut_mode(&self) {
        if self.may_change_shortcut()
            && let Some(control) = self.ivars().shortcut_mode_control.get()
        {
            let mode = if control.indexOfSelectedItem() == 1 {
                ShortcutMode::Hold
            } else {
                ShortcutMode::Toggle
            };
            self.set_shortcut_mode(mode);
        }
        self.refresh();
    }

    pub(super) fn set_shortcut_mode(&self, mode: ShortcutMode) {
        if self.ivars().shortcut_mode.replace(mode) == mode {
            return;
        }
        // An edge queued under the previous mode does not start a new mode.
        if let Some(rx) = self.ivars().hotkey_rx.borrow().as_ref() {
            while rx.try_recv().is_ok() {}
        }
        self.ivars().hold_session.set(None);
        unsafe {
            NSUserDefaults::standardUserDefaults().setObject_forKey(
                Some(&NSString::from_str(mode.preference())),
                &NSString::from_str(ShortcutMode::PREFERENCE),
            );
        }
    }

    pub(super) fn change_shortcut(&self) {
        if !self.may_change_shortcut() {
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
            2 => self.record_shortcut(),
            _ => None,
        };
        self.finish_shortcut_change(candidate);
    }

    pub(super) fn open_shortcut_recorder(&self) {
        if self.may_change_shortcut() {
            let candidate = self.record_shortcut();
            self.finish_shortcut_change(candidate);
        }
    }

    fn record_shortcut(&self) -> Option<Shortcut> {
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
            |candidate| {
                let (tx, _rx) = std::sync::mpsc::channel();
                let mut probe = crate::hotkey::GlobalHotKey::new(tx);
                // Save rechecks registration before committing the preference.
                probe.register(candidate).map_err(|error| error.to_string())
            },
        );
        self.ivars().editing_shortcut.set(false);
        value
    }

    fn finish_shortcut_change(&self, candidate: Option<Shortcut>) {
        if self.ivars().quitting.get() {
            self.refresh();
            return;
        }
        if let Some(candidate) = candidate {
            match self.install_hotkey(candidate) {
                Ok(()) => {
                    self.ivars().shortcut_binding.set(candidate);
                    self.ivars().hold_session.set(None);
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
                        "{error} 保留 {}。",
                        self.ivars().shortcut_binding.get().label()
                    ));
                    self.with_controller(|c| c.message = format!("{error} 已保留原快捷键设置。"));
                }
            }
        }
        // Cancel or failed candidate registration restores the prior binding.
        // If a different app took it during editing, report the actual failure.
        let needs_restore = self.ivars().hotkey.borrow().is_none()
            && (!test_build::active()
                || NSUserDefaults::standardUserDefaults()
                    .objectForKey(ns_string!("dictationShortcut"))
                    .is_some());
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
