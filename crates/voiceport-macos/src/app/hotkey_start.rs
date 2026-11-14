//! A shortcut keeps its key-down application across later AppKit activations.
use super::*;
use crate::{
    hotkey::HotKeyEvent,
    input_target::{self, ApplicationTarget, StartRoute},
};
use serde_json::json;

fn identity(app: &NSRunningApplication) -> Option<ApplicationTarget> {
    Some(ApplicationTarget {
        pid: app.processIdentifier(),
        bundle: app.bundleIdentifier()?.to_string(),
    })
}

fn foreground() -> Option<ApplicationTarget> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .as_deref()
        .and_then(identity)
}

impl Delegate {
    pub(super) fn handle_hotkey(&self, event: HotKeyEvent) {
        if self.ivars().pending_hotkey.borrow_mut().take().is_some() {
            self.reject_hotkey("cancelled_handoff", "已取消返回输入位置。");
            return;
        }
        if self.ivars().quitting.get()
            || self.ivars().managing_sources.get()
            || self.ivars().permission_rx.borrow().is_some()
        {
            return;
        }
        let mut start = false;
        let mut automatic = false;
        self.with_controller(|c| {
            if c.state.phase == Phase::Recording {
                c.stop();
            } else {
                start = !c.busy();
                automatic = c.automatic;
            }
        });
        if !start {
            return;
        }
        *self.ivars().last_start_trace.borrow_mut() = Some(json!({
            "origin":"hotkey", "captured_pid":event.target.as_ref().map(|t|t.pid),
            "captured_bundle":event.target.as_ref().map(|t|&t.bundle),
            "dispatch_pid":foreground().map(|t|t.pid), "outcome":"checking",
        }));
        if !automatic {
            if let Some(trace) = self.ivars().last_start_trace.borrow_mut().as_mut() {
                trace["outcome"] = "panel_only".into();
            }
            self.with_controller(|c| c.start());
            return;
        }
        self.route_hotkey(event, true);
    }

    pub(super) fn poll_hotkey_start(&self) {
        let pending = self.ivars().pending_hotkey.borrow_mut().take();
        if let Some(event) = pending {
            self.route_hotkey(event, false);
        }
    }

    fn reject_hotkey(&self, code: &str, message: &str) {
        if let Some(trace) = self.ivars().last_start_trace.borrow_mut().as_mut() {
            trace["outcome"] = code.into();
        }
        self.with_controller(|c| c.reject_start(message));
    }

    fn route_hotkey(&self, event: HotKeyEvent, first: bool) {
        if self.ivars().quitting.get()
            || self.ivars().managing_sources.get()
            || self.ivars().permission_rx.borrow().is_some()
        {
            self.reject_hotkey("busy", "当前操作尚未结束。");
            return;
        }
        if event
            .application
            .as_ref()
            .is_none_or(|app| app.isTerminated())
        {
            self.reject_hotkey("target_exited", "按键时的目标应用已退出或无法确认。");
            return;
        }
        let current = foreground();
        match input_target::route(
            std::process::id() as i32,
            event.target.as_ref(),
            current.as_ref(),
            event.at.elapsed(),
        ) {
            StartRoute::Direct => {
                if let Some(trace) = self.ivars().last_start_trace.borrow_mut().as_mut() {
                    trace["outcome"] = if first {
                        "direct"
                    } else {
                        "restored_keypress_target"
                    }
                    .into();
                }
                if let Some(target) = event.target {
                    self.with_controller(|c| c.start_in_application(&target));
                }
            }
            StartRoute::RestoreCapturedApplication => {
                if first {
                    self.close_quick_panel();
                    if let Some(window) = self.ivars().window.get() {
                        window.orderOut(None);
                    }
                    let own = NSApplication::sharedApplication(self.mtm());
                    let target = event
                        .application
                        .as_ref()
                        .expect("checked target application");
                    if own.respondsToSelector(sel!(yieldActivationToApplication:)) {
                        own.yieldActivationToApplication(target);
                    }
                    if !target.activateWithOptions(NSApplicationActivationOptions::empty()) {
                        self.reject_hotkey("activation_denied", "无法返回按键时的输入应用。");
                        return;
                    }
                    if let Some(trace) = self.ivars().last_start_trace.borrow_mut().as_mut() {
                        trace["outcome"] = "waiting_for_keypress_target".into();
                    }
                    self.with_controller(|c| {
                        c.message = "正在返回按下快捷键时的输入位置，尚未开始录音。".into()
                    });
                }
                // Activation is asynchronous. Wait for the exact captured PID
                // and bundle on another tick; never activate it a second time.
                *self.ivars().pending_hotkey.borrow_mut() = Some(event);
            }
            StartRoute::Reject(code) => self.reject_hotkey(
                code,
                match code {
                    "self_at_keypress" => "按下快捷键时 VoicePort 仍在前台，请返回目标输入框。",
                    "expired_keypress" => "本次按键等待过久，已停止启动，请重新开始。",
                    _ => "按键后的前台应用已改变或无法确认，已停止本次启动。",
                },
            ),
        }
    }
}
