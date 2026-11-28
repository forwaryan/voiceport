//! A shortcut keeps its key-down application across later AppKit activations.
use super::*;
use crate::{
    hotkey::{HotKeyEvent, KeyPress, Stroke, application_target},
    input_target::{self, ApplicationTarget, StartRoute},
    shortcut::ShortcutMode,
};
use serde_json::json;

fn foreground() -> Option<ApplicationTarget> {
    NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .as_deref()
        .and_then(application_target)
}

/// A release may affect only the controller generation begun by its down edge.
#[derive(Clone, Copy)]
pub(super) struct HoldSession {
    stroke: Stroke,
    generation: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum ReleaseAction {
    Ignore,
    CancelPreparation,
    StopRecording,
}

impl HoldSession {
    fn release(self, stroke: Stroke, generation: u64, phase: Phase) -> ReleaseAction {
        if self.stroke != stroke || self.generation != generation {
            return ReleaseAction::Ignore;
        }
        match phase {
            Phase::Permissions => ReleaseAction::CancelPreparation,
            Phase::Recording => ReleaseAction::StopRecording,
            _ => ReleaseAction::Ignore,
        }
    }
}

impl Delegate {
    pub(super) fn start_with_trace(&self, c: &mut Controller, start: impl FnOnce(&mut Controller)) {
        let previous_generation = c.state.generation();
        start(c);
        let generation = c.state.generation();
        if generation != previous_generation
            && let Some(trace) = self.ivars().last_start_trace.borrow_mut().as_mut()
        {
            trace["session"] = generation.into();
        }
    }

    /// Consume releases before the controller polls permissions or the app
    /// retries a foreground handoff. Neither may begin after an observed up.
    pub(super) fn poll_shortcut(&self) {
        if self.ivars().shortcut_mode.get() != ShortcutMode::Hold {
            if let Some(press) = self.ivars().pending_hotkey.take() {
                self.route_hotkey(press, false, false);
            }
            return;
        }
        self.poll_held_events(self.active_shortcut_stroke());
    }

    fn active_shortcut_stroke(&self) -> Option<Stroke> {
        #[cfg(debug_assertions)]
        if let Some(stroke) = self.ivars().audio_fixture_stroke.get() {
            return stroke;
        }
        self.ivars()
            .hotkey
            .borrow()
            .as_ref()
            .and_then(|hotkey| hotkey.active_stroke(self.ivars().shortcut_binding.get()))
    }

    pub(super) fn hold_start_allowed(
        &self,
        generation: u64,
        sample: impl FnOnce() -> Option<Stroke>,
    ) -> bool {
        let Some(session) = self
            .ivars()
            .hold_session
            .get()
            .filter(|s| s.generation == generation)
        else {
            return true;
        };
        if sample() == Some(session.stroke) {
            return true;
        }
        self.ivars().hold_session.set(None);
        false
    }

    pub(super) fn poll_dictation(&self, controller: &mut Controller) {
        controller.poll_with_start_gate(|generation| {
            self.hold_start_allowed(generation, || self.active_shortcut_stroke())
        });
    }

    fn poll_held_events(&self, active: Option<Stroke>) {
        let pending_at_entry = self
            .ivars()
            .pending_hotkey
            .borrow()
            .as_ref()
            .map(|p| p.stroke);
        loop {
            let event = self
                .ivars()
                .hotkey_rx
                .borrow()
                .as_ref()
                .and_then(|rx| rx.try_recv().ok());
            match event {
                Some(HotKeyEvent::Press(press)) if active == Some(press.stroke) => {
                    self.handle_press(press, true);
                }
                Some(HotKeyEvent::Release(stroke)) => self.handle_release(stroke),
                Some(HotKeyEvent::Press(_)) => {} // Already released before this tick.
                None => break,
            }
        }
        let pending_now = self
            .ivars()
            .pending_hotkey
            .borrow()
            .as_ref()
            .map(|p| p.stroke);
        if pending_at_entry == pending_now
            && let Some(event) = self.ivars().pending_hotkey.take()
        {
            if active == Some(event.stroke) {
                self.route_hotkey(event, false, true);
            } else {
                self.cancel_hotkey_handoff("released_before_start", "已松开快捷键，未开始录音。");
            }
        }
    }

    pub(super) fn poll_toggle_shortcut(&self) {
        if self.ivars().shortcut_mode.get() != ShortcutMode::Toggle {
            return;
        }
        if let Some(hotkey) = self.ivars().hotkey.borrow().as_ref() {
            hotkey.active_stroke(self.ivars().shortcut_binding.get());
        }
        loop {
            let event = self
                .ivars()
                .hotkey_rx
                .borrow()
                .as_ref()
                .and_then(|rx| rx.try_recv().ok());
            match event {
                Some(HotKeyEvent::Press(press)) => {
                    self.handle_press(press, false);
                    break;
                }
                Some(HotKeyEvent::Release(_)) => {} // Toggle has no key-up action.
                None => break,
            }
        }
    }

    fn handle_press(&self, event: KeyPress, held_mode: bool) {
        if self.ivars().quitting.get()
            || self.settings_locked()
            || self.ivars().permission_rx.borrow().is_some()
            || (held_mode && event.at.elapsed() >= input_target::START_DEADLINE)
        {
            return;
        }
        if self.ivars().pending_hotkey.borrow().is_some() {
            if !held_mode {
                self.ivars().pending_hotkey.take();
                self.reject_hotkey("cancelled_handoff", "已取消返回输入位置。");
            }
            return;
        }
        let mut start = false;
        let mut automatic = false;
        self.with_controller(|c| {
            if c.state.phase == Phase::Recording || c.opening_microphone() {
                if !held_mode {
                    c.stop();
                }
            } else {
                start = !c.busy();
                automatic = c.automatic;
            }
        });
        if !start {
            return;
        }
        *self.ivars().last_start_trace.borrow_mut() = Some(json!({
            "origin":"hotkey", "captured_pid":null, "captured_bundle":null,
            "dispatch_pid":null, "outcome":"checking",
        }));
        if !automatic {
            let issue = if event.at.elapsed() >= input_target::START_DEADLINE {
                Some((
                    "expired_keypress",
                    "本次按键等待过久，已停止启动，请重新开始。",
                ))
            } else if event
                .application
                .as_ref()
                .is_none_or(|app| app.isTerminated())
            {
                Some(("target_exited", "按键时的目标应用已退出或无法确认。"))
            } else {
                None
            };
            self.with_controller(|c| {
                let captured = event.target.as_ref();
                // Dropping a known local binding would authorize the global source.
                if let Some((code, message)) = issue
                    && c.requires_app_local_source(captured)
                {
                    self.reject_hotkey_start(c, code, message);
                    return;
                }
                if let Some(trace) = self.ivars().last_start_trace.borrow_mut().as_mut() {
                    trace["outcome"] = "panel_only".into();
                }
                let target = captured.filter(|_| c.workflows.app_rules.enabled && issue.is_none());
                self.start_from_press(c, event.stroke, held_mode, |c| {
                    c.start_from_shortcut(target)
                })
            });
            return;
        }
        self.route_hotkey(event, true, held_mode);
    }

    fn handle_release(&self, stroke: Stroke) {
        if self
            .ivars()
            .pending_hotkey
            .borrow()
            .as_ref()
            .is_some_and(|p| p.stroke == stroke)
            && self.ivars().shortcut_mode.get() == ShortcutMode::Hold
        {
            self.ivars().pending_hotkey.take();
            self.cancel_hotkey_handoff("released_before_start", "已松开快捷键，未开始录音。");
        }
        if let Some(session) = self
            .ivars()
            .hold_session
            .get()
            .filter(|s| s.stroke == stroke)
        {
            self.ivars().hold_session.set(None);
            self.with_controller(|c| {
                match session.release(stroke, c.state.generation(), c.state.phase) {
                    ReleaseAction::CancelPreparation => c.cancel_released_shortcut(),
                    ReleaseAction::StopRecording => c.stop(),
                    ReleaseAction::Ignore => {}
                }
            });
        }
    }

    pub(super) fn cancel_shortcut_session(&self) -> bool {
        self.ivars().hold_session.set(None);
        if self.ivars().pending_hotkey.take().is_none() {
            return false;
        }
        self.cancel_hotkey_handoff("cancelled_handoff", "已取消返回输入位置，未开始录音。");
        true
    }

    fn cancel_hotkey_handoff(&self, code: &str, message: &str) {
        if let Some(trace) = self.ivars().last_start_trace.borrow_mut().as_mut() {
            trace["outcome"] = code.into();
        }
        self.with_controller(|c| c.message = message.into());
    }

    fn start_from_press(
        &self,
        c: &mut Controller,
        stroke: Stroke,
        held_mode: bool,
        start: impl FnOnce(&mut Controller),
    ) {
        let before = c.state.generation();
        self.start_with_trace(c, start);
        let generation = c.state.generation();
        if held_mode && generation != before && c.state.phase == Phase::Permissions {
            self.ivars()
                .hold_session
                .set(Some(HoldSession { stroke, generation }));
        }
    }

    fn reject_hotkey(&self, code: &str, message: &str) {
        self.with_controller(|c| self.reject_hotkey_start(c, code, message));
    }

    fn reject_hotkey_start(&self, c: &mut Controller, code: &str, message: &str) {
        if let Some(trace) = self.ivars().last_start_trace.borrow_mut().as_mut() {
            trace["outcome"] = code.into();
        }
        self.start_with_trace(c, |c| c.reject_start(message));
    }

    fn route_hotkey(&self, event: KeyPress, first: bool, held_mode: bool) {
        if self.ivars().quitting.get()
            || self.settings_locked()
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
                    self.with_controller(|c| {
                        self.start_from_press(c, event.stroke, held_mode, |c| {
                            c.start_from_shortcut(Some(&target))
                        });
                    });
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

/// Synthetic input through the same delegate queue as Carbon. The injected
/// accepted start creates a controller generation but never requests audio.
pub(super) fn isolated_event_regression(mtm: MainThreadMarker) {
    let delegate = Delegate::new(mtm);
    delegate.ivars().shortcut_mode.set(ShortcutMode::Hold);
    *delegate.ivars().controller.borrow_mut() = Some(Controller::new());
    let (tx, rx) = std::sync::mpsc::channel();
    *delegate.ivars().hotkey_rx.borrow_mut() = Some(rx);
    let stroke = Stroke::fixture_stroke(1);
    let old_stroke = Stroke::fixture_stroke(0);
    let press = || KeyPress {
        stroke,
        at: std::time::Instant::now(),
        application: None,
        target: None,
    };
    let promised = |c: &mut Controller| {
        assert!(c.state.begin().is_some());
    };

    // A complete short tap already released before the timer must not start,
    // nor attempt a foreground lookup (the synthetic press has no target).
    tx.send(HotKeyEvent::Press(press())).unwrap();
    tx.send(HotKeyEvent::Release(stroke)).unwrap();
    delegate.poll_held_events(None);
    delegate.with_controller(|c| assert_eq!(c.state.generation(), 0));
    assert!(delegate.ivars().last_start_trace.borrow().is_none());

    // A held focus handoff is abandoned on up before it can be retried.
    *delegate.ivars().pending_hotkey.borrow_mut() = Some(press());
    *delegate.ivars().last_start_trace.borrow_mut() =
        Some(json!({"outcome":"waiting_for_keypress_target"}));
    tx.send(HotKeyEvent::Release(stroke)).unwrap();
    delegate.poll_held_events(None);
    assert!(delegate.ivars().pending_hotkey.borrow().is_none());
    assert_eq!(
        delegate.ivars().last_start_trace.borrow().as_ref().unwrap()["outcome"],
        "released_before_start"
    );

    // Route release through the actual controller cancellation path, with no
    // recorder or captured target. A stale edge must not cancel preparation.
    delegate.with_controller(|c| delegate.start_from_press(c, stroke, true, promised));
    let generation = delegate.ivars().hold_session.get().unwrap().generation;
    tx.send(HotKeyEvent::Release(old_stroke)).unwrap();
    delegate.poll_held_events(Some(stroke));
    delegate.with_controller(|c| assert_eq!(c.state.phase, Phase::Permissions));
    tx.send(HotKeyEvent::Release(stroke)).unwrap();
    delegate.poll_held_events(None);
    delegate.with_controller(|c| {
        assert_eq!(c.state.phase, Phase::Idle);
        assert_ne!(c.state.generation(), generation);
        assert!(!c.state.permissions(generation, true));
    });
    assert!(delegate.ivars().hold_session.get().is_none());

    // The first sample sees the press, but launch must use the latest sample
    // when Controller consumes the actual synthetic permission event.
    for release_while_preparing in [true, false] {
        let active = Cell::new(Some(stroke));
        let captures = Cell::new(0);
        delegate.with_controller(|c| {
            assert_eq!(active.get(), Some(stroke));
            delegate.start_from_press(c, stroke, true, |c| {
                assert!(
                    c.start_pinned_with_permissions(
                        crate::local_models::SessionSource::ChatGpt,
                        String::new(),
                        false,
                        || {
                            if release_while_preparing {
                                active.set(None);
                            }
                            ("authorized", "denied")
                        },
                    )
                    .is_some()
                );
            });
            if !release_while_preparing {
                active.set(None);
            }
            c.poll_with_capture(
                |generation| delegate.hold_start_allowed(generation, || active.get()),
                |_, _, _, _| {
                    captures.set(captures.get() + 1);
                    Ok(false)
                },
            );
            assert_eq!(
                captures.get(),
                0,
                "release before permission consumption entered capture"
            );
            assert_eq!(c.state.phase, Phase::Idle);
            assert!(!c.busy() && !c.can_retry_recording());
        });
        assert!(delegate.ivars().hold_session.get().is_none());
    }

    // A kept press can start. A stale release cannot stop it, and the matching
    // release reaches Stop (the synthetic recorder owns no audio to transcribe).
    for held_mode in [true, false] {
        delegate.with_controller(|c| {
            delegate.start_from_press(c, stroke, held_mode, |c| {
                assert!(
                    c.start_pinned_with_permissions(
                        crate::local_models::SessionSource::ChatGpt,
                        String::new(),
                        false,
                        || ("authorized", "denied"),
                    )
                    .is_some()
                );
            });
            let mut captures = 0;
            c.poll_with_capture(
                |generation| {
                    delegate.hold_start_allowed(generation, || {
                        assert!(held_mode, "toggle must not query HID");
                        Some(stroke)
                    })
                },
                |_, _, _, _| {
                    captures += 1;
                    Ok(false)
                },
            );
            assert_eq!(captures, 1);
            assert_eq!(c.state.phase, Phase::Recording);
        });
        delegate.handle_release(old_stroke);
        delegate.with_controller(|c| assert_eq!(c.state.phase, Phase::Recording));
        delegate.handle_release(stroke);
        delegate.with_controller(|c| {
            if held_mode {
                assert_ne!(c.state.phase, Phase::Recording);
            } else {
                assert_eq!(c.state.phase, Phase::Recording);
                c.stop();
            }
            assert!(!c.busy() && !c.can_retry_recording());
        });
    }
    println!(
        "shortcut_permission_boundary: pre/post-queue release blocked with zero capture or transcription, kept hold and unchanged toggle controls passed"
    );

    // Explicit Cancel before handoff is terminal and does not start anything.
    *delegate.ivars().pending_hotkey.borrow_mut() = Some(press());
    assert!(delegate.cancel_shortcut_session());
    assert!(!delegate.cancel_shortcut_session());
    println!(
        "shortcut_events: synthetic queue verified short tap, released foreground handoff, preparation cancellation and stale release; no system input or recording"
    );
    #[cfg(all(debug_assertions, target_arch = "aarch64"))]
    app_local_toggle_regression(mtm);
}

#[cfg(all(debug_assertions, target_arch = "aarch64"))]
fn app_local_toggle_regression(mtm: MainThreadMarker) {
    use crate::{
        audio::devices::fixture::Backend,
        dictation_app_rules::AppRules,
        dictation_workflows::{Preset, Workflows},
        local_models::{CHATGPT, PARAFORMER, title},
        local_runtime::fixture_catalog,
        storage::{NAMESPACE, Storage},
    };
    use std::time::{Duration, Instant};
    use voiceport_control::{DictationBlockReason, TranscriptionState};

    let _application = NSApplication::sharedApplication(mtm);
    let running = NSRunningApplication::currentApplication();
    assert!(!running.isTerminated());
    let temp = tempfile::tempdir().unwrap();
    let (catalog, _) = fixture_catalog(temp.path());
    let storage = Storage::at(temp.path().canonicalize().unwrap().join(NAMESPACE));
    let captured = ApplicationTarget {
        pid: i32::MAX,
        bundle: "com.example.Editor".into(),
    };
    let other = ApplicationTarget {
        bundle: "com.example.Other".into(),
        ..captured.clone()
    };
    let mut workflows = Workflows::load(Some(
        &json!({"presets":[{
            "name":"本机编辑", "source_id":PARAFORMER, "language":"zh", "dictionary_enabled":false
        }]})
        .to_string(),
    ))
    .unwrap();
    workflows.app_rules = AppRules::from_editor(
        true,
        "本机 com.example.Editor → 本机编辑",
        &workflows.presets,
        &catalog,
    )
    .unwrap();
    let manual_scene = Preset {
        name: "手动".into(),
        source_id: CHATGPT.into(),
        language: "en".into(),
        dictionary_enabled: false,
        output_rules: Default::default(),
    };
    let deadline = input_target::START_DEADLINE;
    for (target, age, application, manual, expected) in [
        (
            Some(&captured),
            Duration::ZERO,
            Some(&running),
            false,
            Ok("场景「本机编辑」"),
        ),
        (
            Some(&captured),
            deadline,
            Some(&running),
            false,
            Err("expired_keypress"),
        ),
        (
            Some(&captured),
            Duration::ZERO,
            None,
            false,
            Err("target_exited"),
        ),
        (None, deadline, None, false, Ok("全局规则")),
        (
            Some(&other),
            deadline,
            Some(&running),
            false,
            Ok("全局规则"),
        ),
        (
            Some(&captured),
            deadline,
            Some(&running),
            true,
            Ok("手动场景「手动」"),
        ),
    ] {
        let delegate = Delegate::new(mtm);
        delegate.ivars().shortcut_mode.set(ShortcutMode::Toggle);
        let mut controller = Controller::new();
        controller.automatic = false;
        controller.language = "en".into();
        controller.catalog = catalog.clone();
        controller.workflows = workflows.clone();
        if manual {
            controller
                .workflows
                .add_preset(manual_scene.clone())
                .unwrap();
            controller.workflows.apply_preset_output(&manual_scene);
        }
        let backend = Backend::new(storage.clone());
        let asr = controller.install_audio_fixture(backend.clone());
        delegate.ivars().controller.replace(Some(controller));
        let (tx, rx) = std::sync::mpsc::channel();
        delegate.ivars().hotkey_rx.replace(Some(rx));
        tx.send(HotKeyEvent::Press(KeyPress {
            at: Instant::now() - age,
            stroke: Stroke::fixture_stroke(1),
            application: application.cloned(),
            target: target.cloned(),
        }))
        .unwrap();
        delegate.poll_toggle_shortcut();
        delegate.with_controller(|c| match expected {
            Err(code) => {
                assert_eq!(c.state.phase, Phase::Failed);
                assert_eq!(
                    delegate.ivars().last_start_trace.borrow().as_ref().unwrap()["outcome"],
                    code
                );
                let receipt = c.gui_dictation_receipt().unwrap();
                assert_eq!(receipt.transcription, TranscriptionState::NotStarted);
                assert_eq!(
                    receipt.block_reason,
                    Some(DictationBlockReason::StartRejected)
                );
                c.poll_with_capture(
                    |_| true,
                    |_, _, _, _| panic!("rejected shortcut entered capture"),
                );
            }
            Ok(rule) => {
                assert_eq!(c.state.phase, Phase::Permissions);
                assert_eq!(c.active_app_rule(), Some(rule));
                let local = rule == "场景「本机编辑」";
                assert_eq!(
                    c.active_app_source(),
                    local.then(|| (title(PARAFORMER), "zh"))
                );
                if !local {
                    assert!(
                        c.gui_result_context()
                            .source
                            .as_ref()
                            .unwrap()
                            .starts_with("ChatGPT · English")
                    );
                }
                let mut admitted = false;
                c.poll_with_capture(
                    |_| true,
                    |_, speech, _, incoming| {
                        assert!(!speech);
                        assert_eq!(incoming.is_some(), local);
                        admitted = true;
                        Err("synthetic probe stops before opening a capture device".into())
                    },
                );
                assert!(admitted);
                assert_eq!(
                    c.gui_dictation_receipt().unwrap().block_reason,
                    Some(DictationBlockReason::CaptureFailed)
                );
            }
        });
        assert_eq!(backend.counts(), (0, 0, 0, 0));
        asr.assert_no_job();
    }
    println!(
        "app-local shortcut: known stale or unavailable targets refused before capture; fresh local, unknown, unmatched and manual controls passed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::Lifecycle;

    #[test]
    fn shortcut_release_during_preparation_invalidates_queued_permission() {
        let stroke = Stroke::fixture_stroke(1);
        let mut session = Lifecycle::default();
        let generation = session.begin().unwrap();
        let hold = HoldSession { stroke, generation };
        assert_eq!(
            hold.release(stroke, generation, session.phase),
            ReleaseAction::CancelPreparation
        );
        session.cancel(false);
        assert!(!session.permissions(generation, true));
        assert_eq!(session.phase, Phase::Idle);
    }

    #[test]
    fn shortcut_only_matching_release_stops_active_generation() {
        let stroke = Stroke::fixture_stroke(1);
        let mut session = Lifecycle::default();
        let generation = session.begin().unwrap();
        assert!(session.permissions(generation, true));
        let hold = HoldSession { stroke, generation };
        assert_eq!(
            hold.release(Stroke::fixture_stroke(2), generation, session.phase),
            ReleaseAction::Ignore
        );
        assert_eq!(
            hold.release(stroke, generation, session.phase),
            ReleaseAction::StopRecording
        );
        session.transcribe(); // manual stop, maximum duration, or end of audio
        for phase in [
            session.phase,
            Phase::Delivering,
            Phase::Ready,
            Phase::Failed,
        ] {
            assert_eq!(
                hold.release(stroke, generation, phase),
                ReleaseAction::Ignore
            );
        }
        session.cancel(false);
        let next_generation = session.begin().unwrap();
        assert!(session.permissions(next_generation, true));
        assert_eq!(
            hold.release(stroke, next_generation, session.phase),
            ReleaseAction::Ignore
        );
    }
}
