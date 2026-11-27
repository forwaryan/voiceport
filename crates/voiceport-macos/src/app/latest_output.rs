use super::*;
use crate::{delivery::model::ApplicationTarget, hotkey::KeyPress, shortcut::Shortcut};
use objc2::rc::Weak;
use std::time::Duration;
use voiceport_control::Response;

impl Delegate {
    pub(super) fn install_repaste_shortcut(&self) {
        if test_build::active() {
            return;
        }
        let slot = &self.ivars().repaste_hotkey;
        if slot.borrow().is_some() || self.ivars().shortcut_binding.get() == Shortcut::REPASTE {
            return;
        }
        let owner = Weak::new(self);
        let mut hotkey = crate::hotkey::GlobalHotKey::on_press(move |press| {
            if let Some(owner) = owner.load() {
                owner.handle_repaste_shortcut(press);
            }
        });
        if hotkey.register(Shortcut::REPASTE).is_ok() {
            *slot.borrow_mut() = Some(hotkey);
        }
    }

    pub(super) fn handle_repaste_shortcut(&self, press: KeyPress) {
        let ivars = self.ivars();
        if ivars.quitting.get()
            || self.settings_locked()
            || ivars.permission_rx.borrow().is_some()
            || ivars.pending_hotkey.borrow().is_some()
            || ivars
                .control
                .try_borrow()
                .map_or(true, |control| control.preparing())
        {
            return;
        }
        // Capture the field on this Carbon key-down, before a timer or window
        // activation could redirect the action to another field in the same app.
        self.with_controller(|c| {
            if c.busy() {
                return;
            }
            match press
                .target
                .filter(|_| press.at.elapsed() <= Duration::from_millis(250))
            {
                Some(target) => {
                    self.paste_latest_dictation(c, target);
                }
                None => c.reject_repaste_without_target(),
            }
        });
    }

    pub(super) fn copy_latest_dictation(&self, c: &mut Controller) -> Response {
        #[cfg(debug_assertions)]
        if let Some(fixture) = self.ivars().latest_output_fixture.get() {
            return c.copy_latest_with(|text| (fixture.copy)(text));
        }
        c.copy_latest_with(Delivery::copy)
    }

    pub(super) fn paste_latest_dictation(
        &self,
        c: &mut Controller,
        target: ApplicationTarget,
    ) -> Response {
        #[cfg(debug_assertions)]
        if let Some(fixture) = self.ivars().latest_output_fixture.get() {
            return c.repaste_latest_with(|delivery| (fixture.capture)(&target, delivery));
        }
        c.repaste_latest(target)
    }
}

#[cfg(debug_assertions)]
type FixtureCopy = dyn Fn(&str) -> bool;
#[cfg(debug_assertions)]
type FixtureCapture = dyn Fn(&ApplicationTarget, &mut Delivery) -> Result<(), String>;

#[cfg(debug_assertions)]
pub(super) struct Fixture {
    pub copy: Box<FixtureCopy>,
    pub capture: Box<FixtureCapture>,
}
