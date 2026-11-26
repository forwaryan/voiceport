//! The persisted opt-in is shared by the preferences switch and control routes.
use super::*;
use crate::controller::recent_dictations::RecentDictationMetadata;
use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle};
use voiceport_control::DeliveryState;

// The workbench shell mounts this page in the integration change.
pub(super) mod workbench;

pub(super) fn row_presentation(
    summary: &RecentDictationMetadata,
) -> (Retained<NSString>, &'static str) {
    let date = NSDate::dateWithTimeIntervalSince1970(summary.created_at_unix_ms as f64 / 1_000.);
    let date = NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
        &date,
        NSDateFormatterStyle::ShortStyle,
        NSDateFormatterStyle::MediumStyle,
    );
    let receipt = summary.delivery;
    let status = match (receipt.state, receipt.stopped) {
        (DeliveryState::NotSent, true) => "投递已停止",
        (DeliveryState::NotSent, false) => "未投递",
        (DeliveryState::Waiting, _) => "等待投递",
        (DeliveryState::AwaitingConfirmation, _) => "等待确认",
        (DeliveryState::PartiallySent, _) => "部分投递",
        (DeliveryState::Confirmed, _) => "已确认",
        (DeliveryState::Submitted, _) => "已提交，未验证",
        (DeliveryState::Uncertain, _) => "结果不明",
    };
    (date, status)
}

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
        #[cfg(not(debug_assertions))]
        let defaults = NSUserDefaults::standardUserDefaults();
        #[cfg(debug_assertions)]
        let defaults = self
            .ivars()
            .recent_dictations_fixture_defaults
            .get()
            .cloned()
            .unwrap_or_else(NSUserDefaults::standardUserDefaults);
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
