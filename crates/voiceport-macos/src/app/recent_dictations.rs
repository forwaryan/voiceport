//! The persisted opt-in is shared by the preferences switch and control routes.
use super::*;

pub(super) fn preference(defaults: &NSUserDefaults) -> bool {
    defaults.boolForKey(ns_string!("keepRecentDictations"))
}

impl Delegate {
    pub(super) fn set_recent_dictations_enabled(&self, enabled: bool) -> bool {
        if self.ivars().quitting.get() {
            return false;
        }
        let Ok(mut slot) = self.ivars().controller.try_borrow_mut() else {
            return false;
        };
        let Some(controller) = slot.as_mut() else {
            return false;
        };
        controller.set_recent_dictations_enabled(enabled);
        let defaults = NSUserDefaults::standardUserDefaults();
        #[cfg(debug_assertions)]
        let defaults = self
            .ivars()
            .recent_dictations_fixture_defaults
            .get()
            .unwrap_or(&defaults);
        defaults.setBool_forKey(enabled, ns_string!("keepRecentDictations"));
        drop(slot);
        #[cfg(debug_assertions)]
        if self
            .ivars()
            .recent_dictations_fixture_defaults
            .get()
            .is_some()
        {
            return true;
        }
        self.refresh();
        true
    }
}
