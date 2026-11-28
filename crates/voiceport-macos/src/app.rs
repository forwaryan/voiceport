//! Native AppKit shell. Diagnostic paths precede controller and device creation.
use crate::{controller::Controller, delivery::Delivery, lifecycle::Phase};
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer, NSUserDefaults, ns_string,
};
use std::cell::{Cell, OnceCell, RefCell};
use voiceport_core::test_build;
mod control;
#[cfg(debug_assertions)]
pub use control::{
    isolated_audio_start_regression, isolated_control_regression,
    isolated_dictionary_exchange_regression, isolated_performance_baseline,
};
mod control_probes;
mod dictation_page;
mod dictation_workflows;
mod file_transcription;
mod interrupted_dictation;
#[cfg(debug_assertions)]
pub use file_transcription::isolated_file_transcription_regression;
mod first_run;
mod hotkey_start;
mod latest_output;
mod menu_surface;
#[doc(hidden)]
pub use first_run::first_run_field_fixture;
mod microphone;
mod model_manager;
mod picker;
mod preferences;
mod quick_panel;
mod recent_dictations;
mod shortcut_editor;
mod source_manager;
mod source_page;
mod source_probes;
mod status_overlay;
mod update_ui;
mod workbench;

#[doc(hidden)]
pub fn isolated_shortcut_regression(mtm: MainThreadMarker) {
    preferences::isolated_layout_regression(mtm);
    hotkey_start::isolated_event_regression(mtm);
    #[cfg(debug_assertions)]
    {
        workbench::isolated_regression(mtm);
        source_page::fixture::native(mtm);
    }
}

#[cfg(test)]
const COPY_SUCCESS: &str = "文字已复制。";
#[cfg(test)]
const COPY_FAILURE: &str = "复制失败，请选中文字手动复制。";

/// Only a completed, nonempty transcription can have a delivery warning.
/// The stage comes from the session policy, never from diagnostic text.
fn completed_delivery_failure(
    phase: Phase,
    has_transcript: bool,
    failure_stage: Option<&str>,
) -> Option<&'static str> {
    if phase != Phase::Ready || !has_transcript {
        return None;
    }
    Some(match failure_stage? {
        "accessibility" => "辅助功能授权未生效，请在设置中重新授权。",
        "secureInput" | "sensitive" => "当前为安全输入或密码框，无法自动输入。",
        "application" => "开始时未选中目标应用，请回到目标输入框。",
        "focus" => "无法确认原输入框；下次请先点好输入位置。",
        "role" | "disabled" | "writable" => "原控件无法安全写入，请使用可编辑输入框。",
        "content" | "selection" => "无法确认原文字或光标位置，请重新定位光标。",
        "monitor" => "无法监测终端输入，请重新聚焦后再开始。",
        "ownership" | "userInput" | "revision" => "输入内容或光标已改变，请检查目标输入框。",
        "write" | "event" => "未能向原输入框完成写入，请检查已有文字。",
        "readback" => "输入框尚未确认文字，请先检查已有文字，避免重复粘贴。",
        "modifiers" => "快捷键尚未松开，已停止自动输入；请松键并检查已有文字。",
        "clipboardChanged" => "剪贴板已被其他操作更新，已停止粘贴并保留新内容。",
        "clipboard" => "未能准备粘贴文字，请复制面板结果后手动粘贴。",
        "pasteRequest" => "未能确认粘贴请求，请先检查输入框，避免重复粘贴。",
        "ended" => "本次自动输入已停止，请检查目标输入框。",
        _ => "自动输入未能完成，请查看详情后重新开始。",
    })
}

#[cfg(test)]
fn delivery_failure_description(reason: &str, message: &str) -> String {
    let action = match message {
        COPY_SUCCESS => "文字已复制，可回到输入框手动粘贴。",
        COPY_FAILURE => COPY_FAILURE,
        _ => "结果保留在面板，可复制后手动粘贴。",
    };
    format!("{reason}\n{action}")
}

#[cfg(test)]
mod delivery_feedback_tests {
    use super::*;

    #[test]
    fn warning_requires_completed_text_and_a_session_failure() {
        for phase in [
            Phase::Idle,
            Phase::Permissions,
            Phase::Recording,
            Phase::Transcribing,
            Phase::Delivering,
            Phase::Stopping,
            Phase::Failed,
        ] {
            assert_eq!(completed_delivery_failure(phase, true, Some("focus")), None);
        }
        assert_eq!(
            completed_delivery_failure(Phase::Ready, false, Some("focus")),
            None
        );
        // The controller supplies None for success, disabled automatic input,
        // sample tests, and manual transcription retries.
        assert_eq!(completed_delivery_failure(Phase::Ready, true, None), None);
    }

    #[test]
    fn focus_failure_explains_recovery_without_exposing_diagnostics() {
        let reason = completed_delivery_failure(Phase::Ready, true, Some("focus")).unwrap();
        let description =
            delivery_failure_description(reason, "[focus] 目标应用没有提供焦点输入框（AX -25212）");
        assert!(description.contains("先点好输入位置"));
        assert!(description.contains("结果保留在面板，可复制后手动粘贴"));
        assert!(!description.contains("[focus]"));
        assert!(!description.contains("AX"));
        assert!(!description.contains("-25212"));
        assert_eq!(description.lines().count(), 2);
    }

    #[test]
    fn copying_preserves_failure_reason_and_reports_copy_result() {
        let reason = completed_delivery_failure(Phase::Ready, true, Some("focus")).unwrap();
        for feedback in [COPY_SUCCESS, COPY_FAILURE] {
            let description = delivery_failure_description(reason, feedback);
            assert!(description.starts_with(reason));
            assert!(description.contains(if feedback == COPY_SUCCESS {
                "文字已复制，可回到输入框手动粘贴"
            } else {
                COPY_FAILURE
            }));
        }
    }

    #[test]
    fn changed_input_and_unknown_failures_keep_a_safe_recovery_path() {
        let changed = completed_delivery_failure(Phase::Ready, true, Some("ownership")).unwrap();
        assert!(changed.contains("输入内容或光标已改变"));
        let unknown = completed_delivery_failure(Phase::Ready, true, Some("futureStage")).unwrap();
        assert!(unknown.contains("查看详情"));
        assert!(!unknown.contains("futureStage"));
        assert!(delivery_failure_description(unknown, "最终转写完成。").contains("手动粘贴"));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusAppearance {
    Ready,
    Recording,
    Working,
    Attention,
}
#[derive(Default)]
struct Ivars {
    controller: RefCell<Option<Controller>>,
    control: RefCell<control::Control>,
    status: OnceCell<Retained<NSStatusItem>>,
    quick_panel: OnceCell<quick_panel::QuickPanel>,
    first_run: OnceCell<first_run::Panel>,
    context_menu: OnceCell<Retained<NSMenu>>,
    context_menu_visible: Cell<bool>,
    menu_snapshot: RefCell<Option<menu_surface::Snapshot>>,
    menu_device: RefCell<menu_surface::DeviceRefresh>,
    menu_toggle: OnceCell<Retained<NSMenuItem>>,
    menu_state: OnceCell<Retained<NSMenuItem>>,
    menu_shortcut: OnceCell<Retained<NSMenuItem>>,
    menu_cancel: OnceCell<Retained<NSMenuItem>>,
    menu_copy: OnceCell<Retained<NSMenuItem>>,
    menu_repaste: OnceCell<Retained<NSMenuItem>>,
    menu_output_notice: OnceCell<Retained<NSMenuItem>>,
    overlay: RefCell<status_overlay::Overlay>,
    overlay_menu: OnceCell<Retained<NSMenuItem>>,
    update: RefCell<update_ui::UpdateUi>,
    update_item: OnceCell<Retained<NSMenuItem>>,
    update_refresh_item: OnceCell<Retained<NSMenuItem>>,
    update_auto_item: OnceCell<Retained<NSMenuItem>>,
    update_commit: RefCell<Option<std::process::ChildStdin>>,
    status_appearance: Cell<Option<StatusAppearance>>,
    workbench: OnceCell<Retained<workbench::Workbench>>,
    dictation_result: OnceCell<dictation_page::Page>,
    file_page: OnceCell<file_transcription::Page>,
    history_page: OnceCell<Retained<recent_dictations::workbench::HistoryPage>>,
    source_page: OnceCell<Retained<source_page::Page>>,
    window: OnceCell<Retained<NSWindow>>,
    show_window_on_launch: Cell<bool>,
    show_models_on_launch: Cell<bool>,
    dictionary_settings: OnceCell<Retained<NSButton>>,
    preset_settings: OnceCell<Retained<NSButton>>,
    app_rules_settings: OnceCell<Retained<NSButton>>,
    workflow_dialog: Cell<bool>,
    #[cfg(debug_assertions)]
    dictionary_fixture_defaults: OnceCell<Retained<NSUserDefaults>>,
    automatic: OnceCell<Retained<NSSwitch>>,
    recent_dictations_switch: OnceCell<Retained<NSSwitch>>,
    #[cfg(debug_assertions)]
    recent_dictations_fixture_defaults: OnceCell<Retained<NSUserDefaults>>,
    language: OnceCell<Retained<NSPopUpButton>>,
    source: OnceCell<Retained<NSPopUpButton>>,
    source_rows: RefCell<Vec<(String, String)>>,
    source_detail: OnceCell<Retained<NSTextField>>,
    refresh_models: OnceCell<Retained<NSButton>>,
    manage_models: OnceCell<Retained<NSButton>>,
    managing_models: Cell<bool>,
    model_manager: RefCell<Option<Retained<model_manager::Manager>>>,
    microphone_panel: OnceCell<Retained<microphone::Panel>>,
    manage_sources: OnceCell<Retained<NSButton>>,
    test_source: OnceCell<Retained<NSButton>>,
    managing_sources: Cell<bool>,
    source_manager: RefCell<Option<Retained<source_manager::Manager>>>,
    source_probe_panel: RefCell<Option<Retained<source_probes::Panel>>>,
    shortcut: OnceCell<Retained<NSPopUpButton>>,
    shortcut_mode_control: OnceCell<Retained<NSPopUpButton>>,
    settings_input_slots: OnceCell<preferences::InputSlots>,
    shortcut_feedback: OnceCell<Retained<NSTextField>>,
    shortcut_notice: RefCell<Option<String>>,
    timer: OnceCell<Retained<NSTimer>>,
    quitting: Cell<bool>,
    cli_quit_at: Cell<Option<std::time::Instant>>,
    shutdown_started: Cell<bool>,
    shortcut_binding: Cell<crate::shortcut::Shortcut>,
    editing_shortcut: Cell<bool>,
    hotkey: RefCell<Option<crate::hotkey::GlobalHotKey>>,
    repaste_hotkey: RefCell<Option<crate::hotkey::GlobalHotKey>>,
    #[cfg(debug_assertions)]
    latest_output_fixture: OnceCell<latest_output::Fixture>,
    hotkey_rx: RefCell<Option<std::sync::mpsc::Receiver<crate::hotkey::HotKeyEvent>>>,
    pending_hotkey: RefCell<Option<crate::hotkey::KeyPress>>,
    hold_session: Cell<Option<hotkey_start::HoldSession>>,
    #[cfg(debug_assertions)]
    audio_fixture_stroke: Cell<Option<Option<crate::hotkey::Stroke>>>,
    shortcut_mode: Cell<crate::shortcut::ShortcutMode>,
    last_start_trace: RefCell<Option<serde_json::Value>>,
    permission_rx: RefCell<Option<std::sync::mpsc::Receiver<(u64, crate::audio::AudioEvent)>>>,
    prepare_permissions: OnceCell<Retained<NSButton>>,
    accessibility: OnceCell<Retained<NSButton>>,
    hotkey_error: RefCell<Option<String>>,
    hotkey_registration_failure: Cell<Option<voiceport_control::readiness::Hotkey>>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowWillClose:))]
        fn workbench_will_close(&self, _notification: &NSNotification) {
            self.clear_workbench_history();
        }
        #[unsafe(method(windowDidMiniaturize:))]
        fn workbench_did_miniaturize(&self, _notification: &NSNotification) {
            self.clear_workbench_history();
        }
    }
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn launched(&self, _notification: &NSNotification) {
            self.build_menu();
            if self.ivars().show_window_on_launch.get() {
                self.show_window();
            } else {
                self.refresh();
                self.show_first_run_if_new();
            }
        }
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            if let Some(mut commit) = self.ivars().update_commit.borrow_mut().take() {
                use std::io::Write;
                let _ = commit.write_all(&crate::app_update::QUIT_ACK);
            }
        }
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            self.show_quick_panel();
            false
        }
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_window_closed(&self, _app: &NSApplication) -> bool {
            false
        }
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            self.ivars().quitting.set(true);
            self.close_quick_panel();
            self.close_file_sheet();
            if let Ok(mut overlay) = self.ivars().overlay.try_borrow_mut() { overlay.close(); }
            let manager = self.ivars().source_manager.borrow().clone();
            if let Some(manager) = manager { manager.close_for_shutdown(); }
            let probes = self.ivars().source_probe_panel.borrow().clone();
            if let Some(probes) = probes { probes.close(); }
            let models = self.ivars().model_manager.borrow().clone();
            if let Some(models) = models { models.shutdown(); }
            // Idle shutdown can finish synchronously. Reentrant termination
            // must wait until the controller borrow and any worker lease end.
            if let Ok(mut slot) = self.ivars().controller.try_borrow_mut() {
                if let Some(c) = slot.as_mut() {
                    c.file_jobs.cancel_for_shutdown();
                    if !self.ivars().shutdown_started.replace(true) {
                        c.set_recent_dictations_enabled(false);
                        c.cancel();
                    }
                    if c.busy() { return NSApplicationTerminateReply::TerminateLater; }
                }
                if self.settings_locked() { return NSApplicationTerminateReply::TerminateLater; }
                self.ivars().control.borrow_mut().server.take();
                return NSApplicationTerminateReply::TerminateNow;
            }
            NSApplicationTerminateReply::TerminateLater
        }
    }
    impl Delegate {
        #[unsafe(method(preparePermissions:))]
        fn prepare_permissions_action(&self, _sender: Option<&AnyObject>) {
            self.prepare_permission_check(first_run::PermissionCheck::CurrentSource);
        }
        #[unsafe(method(showFirstRunChecks:))]
        fn show_first_run_action(&self, _sender: Option<&AnyObject>) { self.show_first_run_checks(); }
        #[unsafe(method(closeFirstRunChecks:))]
        fn close_first_run_action(&self, _sender: Option<&AnyObject>) { self.close_first_run_checks(); }
        #[unsafe(method(runFirstRunCheck:))]
        fn run_first_run_action(&self, _sender: Option<&AnyObject>) { self.run_first_run_check(); }
        #[unsafe(method(firstRunMicrophone:))]
        fn first_run_microphone_action(&self, _sender: Option<&AnyObject>) {
            self.prepare_permission_check(first_run::PermissionCheck::Microphone);
        }
        #[unsafe(method(firstRunSpeech:))]
        fn first_run_speech_action(&self, _sender: Option<&AnyObject>) {
            self.prepare_permission_check(first_run::PermissionCheck::Speech);
        }
        #[unsafe(method(firstRunAccessibility:))]
        fn first_run_accessibility_action(&self, _sender: Option<&AnyObject>) {
            self.request_first_run_accessibility();
        }
        #[unsafe(method(firstRunShortcut:))]
        fn first_run_shortcut_action(&self, _sender: Option<&AnyObject>) {
            self.preferences_action(sel!(showPreferences:), None);
            self.close_first_run_checks();
        }
        #[unsafe(method(firstRunTranscribeOnly:))]
        fn first_run_transcribe_only_action(&self, _sender: Option<&AnyObject>) {
            self.transcribe_only_from_first_run();
        }
        #[unsafe(method(toggle:))]
        fn toggle_action(&self, _sender: Option<&AnyObject>) {
            if self.ivars().permission_rx.borrow().is_none() && !self.settings_locked() && self.ivars().pending_hotkey.borrow().is_none() {
                self.with_controller(|c| {
                    if !c.busy() && c.state.phase != Phase::Recording {
                        *self.ivars().last_start_trace.borrow_mut() = Some(serde_json::json!({
                            "origin":"window_or_menu", "outcome":"ui_start",
                            "dispatch_pid":null,
                        }));
                        self.start_with_trace(c, Controller::toggle);
                    } else {
                        c.toggle();
                    }
                });
            }
            self.refresh();
        }
        #[unsafe(method(cancel:))]
        fn cancel_action(&self, _sender: Option<&AnyObject>) {
            if !self.cancel_shortcut_session() { self.with_controller(|c| c.cancel()); }
            self.refresh();
        }
        #[unsafe(method(retry:))]
        fn retry_action(&self, _sender: Option<&AnyObject>) {
            if self.settings_locked() { return; }
            let mut fragment = None;
            self.with_controller(|c| {
                fragment = interrupted_dictation::FragmentPlan::read_gui(c);
                if fragment.is_none() { c.retry(); }
            });
            if let Some(plan) = fragment { self.confirm_interrupted_dictation(plan); }
            self.refresh();
        }
        #[unsafe(method(copyText:))]
        fn copy_action(&self, _sender: Option<&AnyObject>) {
            if self.ivars().quitting.get() { return; }
            self.with_controller(|c| { if !c.busy() { self.copy_latest_dictation(c); } });
            self.refresh();
        }
        #[unsafe(method(showQuickPanel:))]
        fn quick_panel_action(&self, _sender: Option<&AnyObject>) { self.toggle_quick_panel(); }
        #[unsafe(method(selectQuickSource:))]
        fn quick_source_action(&self, sender: &NSButton) { self.select_quick_source(sender); }
        #[unsafe(method(resolveQuickSource:))]
        fn quick_source_guidance_action(&self, _sender: Option<&AnyObject>) { self.resolve_quick_source(); }
        #[unsafe(method(quickManageSources:))]
        fn quick_manage_action(&self, _sender: Option<&AnyObject>) {
            self.show_workbench_page(workbench::Page::Settings);
            self.manage_sources_action(sel!(manageSources:), None);
        }
        #[unsafe(method(showWindow:))]
        fn show_action(&self, _sender: Option<&AnyObject>) { self.show_window(); }

        #[unsafe(method(showFileTranscription:))]
        fn show_file_action(&self, _sender: Option<&AnyObject>) { self.show_file_transcription(); }
        #[unsafe(method(chooseFileTranscription:))]
        fn choose_file_action(&self, _sender: Option<&AnyObject>) { self.choose_file_transcription(); }
        #[unsafe(method(selectFileTranscriptionSource:))]
        fn select_file_source_action(&self, _sender: Option<&AnyObject>) { self.select_file_transcription_source(); }
        #[unsafe(method(startFileTranscription:))]
        fn start_file_action(&self, _sender: Option<&AnyObject>) { self.start_file_transcription(); }
        #[unsafe(method(cancelFileTranscription:))]
        fn cancel_file_action(&self, _sender: Option<&AnyObject>) { self.cancel_file_transcription(); }
        #[unsafe(method(exportFileTranscriptionText:))]
        fn export_file_text_action(&self, _sender: Option<&AnyObject>) {
            self.export_file_transcription(crate::file_transcription::ExportFormat::Text);
        }
        #[unsafe(method(exportFileTranscriptionMarkdown:))]
        fn export_file_markdown_action(&self, _sender: Option<&AnyObject>) {
            self.export_file_transcription(crate::file_transcription::ExportFormat::Markdown);
        }
        #[unsafe(method(showPreferences:))]
        fn preferences_action(&self, _sender: Option<&AnyObject>) {
            self.show_workbench_page(workbench::Page::Settings);
        }
        #[unsafe(method(toggleDictationDiagnostics:))]
        fn toggle_dictation_diagnostics(&self, _sender: Option<&AnyObject>) {
            if let Some(page) = self.ivars().dictation_result.get() { page.toggle_details(); }
            self.refresh();
        }
        #[unsafe(method(editDictationCopy:))]
        fn edit_dictation_copy(&self, _sender: Option<&AnyObject>) {
            if let Some(page) = self.ivars().dictation_result.get() { page.toggle_edit(); }
            self.refresh();
        }
        #[unsafe(method(copyDictationResult:))]
        fn copy_dictation_result(&self, _sender: Option<&AnyObject>) {
            if let Some(page) = self.ivars().dictation_result.get() { page.copy(Delivery::copy); }
        }
        #[unsafe(method(requestAccessibility:))]
        fn accessibility_action(&self, _sender: Option<&AnyObject>) { Delivery::request_accessibility(); }
        #[unsafe(method(toggleDictationOverlay:))]
        fn dictation_overlay_action(&self, _sender: Option<&AnyObject>) {
            status_overlay::set_enabled(!status_overlay::enabled());
            self.refresh();
        }
        #[unsafe(method(setRecentDictationsPreference:))]
        fn recent_dictations_preference_action(&self, sender: &NSSwitch) {
            if self.ivars().recent_dictations_switch.get().is_some_and(|control| std::ptr::eq(&**control, sender)) {
                self.set_recent_dictations_enabled(sender.state() == NSControlStateValueOn);
            }
        }
        #[unsafe(method(toggleAutomatic:))]
        fn automatic_action(&self, _sender: Option<&AnyObject>) {
            if self.settings_locked() { return; }
            self.with_controller(|c| self.set_automatic_input(c, !c.automatic));
            self.refresh();
        }
        #[unsafe(method(selectSource:))]
        fn source_action(&self, _sender: Option<&AnyObject>) {
            if let Some(id) = self.ivars().source.get().and_then(|control| picker::selected_id(control)) {
                self.request_dictation_source(&id);
            }
            self.refresh();
        }
        #[unsafe(method(showSources:))]
        fn show_sources_action(&self, _sender: Option<&AnyObject>) {
            self.show_workbench_page(workbench::Page::Sources);
        }
        #[unsafe(method(manageSources:))]
        fn manage_sources_action(&self, _sender: Option<&AnyObject>) {
            self.open_source_manager();
        }
        #[unsafe(method(testSource:))]
        fn test_source_action(&self, _sender: Option<&AnyObject>) {
            if !self.ivars().quitting.get() && self.ivars().permission_rx.borrow().is_none() && !self.settings_locked() {
                source_probes::show(self);
            }
            self.refresh();
        }
        #[unsafe(method(microphoneSettings:))]
        fn microphone_settings_action(&self, _sender: Option<&AnyObject>) {
            self.show_workbench_page(workbench::Page::Device);
        }
        #[unsafe(method(manageModels:))]
        fn manage_models_action(&self, _sender: Option<&AnyObject>) {
            let existing = self.ivars().model_manager.borrow().clone();
            if let Some(manager) = existing { manager.bring_forward(); return; }
            let idle = self.ivars().controller.borrow().as_ref().is_some_and(|c| !c.busy() && !c.discovering());
            if self.ivars().quitting.get() || !idle || self.settings_locked() || self.ivars().permission_rx.borrow().is_some() || self.ivars().pending_hotkey.borrow().is_some() || self.ivars().control.borrow().preparing() { return; }
            if let Err(error) = model_manager::show(self) { self.with_controller(|c| c.message = error); }
            self.refresh();
        }
        #[unsafe(method(refreshModels:))]
        fn refresh_models_action(&self, _sender: Option<&AnyObject>) {
            if self.ivars().permission_rx.borrow().is_none() && !self.settings_locked() { self.with_controller(|c| c.refresh_catalog()); }
            self.refresh();
        }
        #[unsafe(method(cycleLanguage:))]
        fn language_action(&self, _sender: Option<&AnyObject>) {
            if self.settings_locked() { return; }
            self.with_controller(|c| {
                if c.busy() { return; }
                let selected = self.ivars().language.get().map(|control| control.indexOfSelectedItem()).unwrap_or(0);
                self.set_language(c, voiceport_core::language::code(selected as usize).into());
            }); self.refresh();
        }
        #[unsafe(method(showPersonalDictionary:))]
        fn dictionary_action(&self, _sender: Option<&AnyObject>) { self.show_dictionary(); }
        #[unsafe(method(showDictationPresets:))]
        fn presets_action(&self, _sender: Option<&AnyObject>) { self.show_presets(); }
        #[unsafe(method(showDictationAppRules:))]
        fn app_rules_action(&self, _sender: Option<&AnyObject>) { self.show_app_rules(); }
        #[unsafe(method(showOriginalTranscript:))]
        fn original_action(&self, _sender: Option<&AnyObject>) { self.show_original_transcript(); }
        #[unsafe(method(cycleShortcut:))]
        fn shortcut_action(&self, _sender: Option<&AnyObject>) {
            self.change_shortcut();
        }
        #[unsafe(method(selectShortcutMode:))]
        fn shortcut_mode_action(&self, _sender: Option<&AnyObject>) {
            self.change_shortcut_mode();
        }
        #[unsafe(method(updateVoicePort:))]
        fn update_voiceport_action(&self, _sender: Option<&AnyObject>) {
            self.show_workbench_page(workbench::Page::Updates);
        }
        #[unsafe(method(refreshVoicePortUpdate:))]
        fn refresh_voiceport_action(&self, _sender: Option<&AnyObject>) { self.start_update_check(true); }
        #[unsafe(method(toggleAutoUpdate:))]
        fn toggle_auto_update_action(&self, _sender: Option<&AnyObject>) { self.toggle_auto_update(); }
        #[unsafe(method(tick:))]
        fn tick(&self, _sender: Option<&AnyObject>) {
            self.poll_shortcut();
            let permission_reply = self.ivars().permission_rx.borrow().as_ref().and_then(|rx| rx.try_recv().ok());
            if let Some((_, crate::audio::AudioEvent::Permissions { .. })) = permission_reply {
                self.ivars().permission_rx.borrow_mut().take();
                let (microphone, speech) = crate::audio::Recorder::permission_status();
                self.with_controller(|c| c.message = format!("权限检查：麦克风{}；{}；辅助功能{}。未开始录音。", if microphone == "authorized" { "已允许" } else { "未允许" }, if !c.uses_system_speech() { "此来源无需系统语音识别权限" } else if speech == "authorized" { "系统语音识别已允许" } else { "系统语音识别未允许（无设备端预览）" }, if Delivery::trusted() { "已允许" } else { "未允许" }));
            }
            let opening_at_entry = self.ivars().controller.borrow().as_ref().is_some_and(Controller::opening_microphone);
            if opening_at_entry { self.poll_toggle_shortcut(); }
            self.with_controller(|c| {
                if self.ivars().quitting.get() && !self.ivars().shutdown_started.get() {
                    c.set_recent_dictations_enabled(false);
                    c.file_jobs.cancel_for_shutdown();
                    c.cancel(); self.ivars().shutdown_started.set(true);
                }
                self.poll_dictation(c);
            });
            if !opening_at_entry { self.poll_toggle_shortcut(); }
            if self.ivars().show_models_on_launch.get()
                && self.ivars().controller.borrow().as_ref().is_some_and(|c| !c.busy() && !c.discovering()) {
                self.ivars().show_models_on_launch.set(false);
                self.manage_models_action(sel!(manageModels:), None);
            }
            self.poll_control();
            if let Some(hotkey) = self.ivars().repaste_hotkey.borrow().as_ref() {
                hotkey.active_stroke(crate::shortcut::Shortcut::REPASTE);
            }
            self.update_tick();
            self.poll_menu_device();
            if self.ivars().cli_quit_at.get().is_some_and(|at| std::time::Instant::now() >= at) {
                self.ivars().cli_quit_at.set(None);
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
            self.refresh();
            self.refresh_file_page();
            let probes = self.ivars().source_probe_panel.borrow().clone();
            if let Some(probes) = probes { probes.refresh(); }
            let sources = self.ivars().source_manager.borrow().clone();
            if let Some(sources) = sources { sources.refresh_diagnostics(); }
            let models = self.ivars().model_manager.borrow().clone();
            if let Some(models) = models { models.refresh(); }
            if let Some(microphone) = self.ivars().microphone_panel.get() {
                if self.ivars().quitting.get() { microphone.close(); } else { microphone.refresh(); }
            }
            if self.ivars().quitting.get() {
                if self.ivars().editing_shortcut.get() { NSApplication::sharedApplication(self.mtm()).abortModal(); }
                let manager = self.ivars().source_manager.borrow().clone();
                if let Some(manager) = manager { manager.close_for_shutdown(); }
                let stopped = self.ivars().shutdown_started.get()
                    && self.ivars().controller.try_borrow().is_ok_and(|slot| slot.as_ref().is_none_or(|c| !c.busy()));
                if stopped && !self.settings_locked() && self.ivars().cli_quit_at.get().is_none() {
                    self.ivars().control.borrow_mut().server.take();
                    NSApplication::sharedApplication(self.mtm()).replyToApplicationShouldTerminate(true);
                }
            }
        }
    }
);
impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars::default());
        unsafe { msg_send![super(this), init] }
    }
    fn set_automatic_input(&self, c: &mut Controller, enabled: bool) {
        c.automatic = enabled;
        NSUserDefaults::standardUserDefaults()
            .setBool_forKey(enabled, ns_string!("automaticPaste"));
    }

    fn set_language(&self, c: &mut Controller, language: String) {
        c.language = language;
        unsafe {
            NSUserDefaults::standardUserDefaults().setObject_forKey(
                Some(&NSString::from_str(&c.language)),
                ns_string!("language"),
            );
        }
    }

    fn open_source_manager(&self) {
        let manager = self.ivars().source_manager.borrow().clone();
        if let Some(manager) = manager {
            manager.bring_forward();
            return;
        }
        let idle = self
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .is_some_and(|c| !c.busy() && !c.discovering());
        if self.ivars().quitting.get()
            || !idle
            || self.ivars().permission_rx.borrow().is_some()
            || self.ivars().editing_shortcut.get()
            || self.ivars().managing_models.get()
            || self.ivars().managing_sources.replace(true)
        {
            return;
        }
        if let Err(error) = source_manager::manage(self) {
            self.ivars().managing_sources.set(false);
            self.with_controller(|c| c.message = format!("无法打开来源管理：{error}"));
        }
        // The retained manager releases admission in windowWillClose.
        self.refresh();
    }

    fn with_controller(&self, action: impl FnOnce(&mut Controller)) {
        if let Ok(mut slot) = self.ivars().controller.try_borrow_mut()
            && let Some(controller) = slot.as_mut()
        {
            action(controller);
        }
    }
    fn menu_item(
        &self,
        title: &str,
        action: objc2::runtime::Sel,
        key: &str,
    ) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        if action != sel!(terminate:) {
            unsafe {
                item.setTarget(Some(self));
            }
        }
        item
    }
    fn build_context_menu(&self) {
        let mtm = self.mtm();
        let _ = self.ivars().update.borrow().running_version();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let name = test_build::application_name();
        let heading = self.menu_item(&format!("{name} · 就绪"), sel!(showWindow:), "");
        heading.setEnabled(false);
        menu.addItem(&heading);
        let _ = self.ivars().menu_state.set(heading);
        let shortcut = self.menu_item("⌃⌥空格 · 开始 / 停止听写", sel!(showWindow:), "");
        shortcut.setEnabled(false);
        menu.addItem(&shortcut);
        let _ = self.ivars().menu_shortcut.set(shortcut);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        for (title, action, symbol) in [
            ("打开结果与设置…", sel!(showWindow:), "macwindow"),
            ("音频文件转写…", sel!(showFileTranscription:), "waveform"),
            (
                "转写来源…",
                sel!(showSources:),
                "waveform.badge.magnifyingglass",
            ),
            (
                "首次使用与自检…",
                sel!(showFirstRunChecks:),
                "checkmark.circle",
            ),
            ("语音模型…", sel!(manageModels:), "internaldrive"),
            ("麦克风设置…", sel!(microphoneSettings:), "mic"),
            ("开始听写", sel!(toggle:), "mic"),
            ("复制最近听写", sel!(copyText:), "doc.on.doc"),
            ("取消听写", sel!(cancel:), "xmark.circle"),
        ] {
            let item = self.menu_item(title, action, "");
            item.setImage(symbol_image(symbol, 15.).as_deref());
            menu.addItem(&item);
            if action == sel!(toggle:) {
                let _ = self.ivars().menu_toggle.set(item);
            } else if action == sel!(cancel:) {
                let _ = self.ivars().menu_cancel.set(item);
            } else if action == sel!(copyText:) {
                let _ = self.ivars().menu_copy.set(item);
            }
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let repaste = self.menu_item("重贴最近听写", sel!(showWindow:), "");
        repaste.setEnabled(false);
        menu.addItem(&repaste);
        let _ = self.ivars().menu_repaste.set(repaste);
        let output_notice = self.menu_item("", sel!(showWindow:), "");
        output_notice.setEnabled(false);
        output_notice.setHidden(true);
        menu.addItem(&output_notice);
        let _ = self.ivars().menu_output_notice.set(output_notice);
        let overlay = self.menu_item("显示听写提示", sel!(toggleDictationOverlay:), "");
        overlay.setState(status_overlay::checked(status_overlay::enabled()));
        menu.addItem(&overlay);
        let _ = self.ivars().overlay_menu.set(overlay);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let update = self.menu_item("软件更新…", sel!(updateVoicePort:), "");
        menu.addItem(&update);
        let _ = self.ivars().update_item.set(update);
        if !test_build::active() {
            let refresh = self.menu_item("重新检查更新…", sel!(refreshVoicePortUpdate:), "");
            menu.addItem(&refresh);
            let _ = self.ivars().update_refresh_item.set(refresh);
            let automatic = self.menu_item("自动检查更新", sel!(toggleAutoUpdate:), "");
            menu.addItem(&automatic);
            let _ = self.ivars().update_auto_item.set(automatic);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&self.menu_item(&format!("退出 {name}"), sel!(terminate:), ""));
        let _ = self.ivars().context_menu.set(menu);
    }

    fn build_menu(&self) {
        let mtm = self.mtm();
        self.build_context_menu();
        // A fixed, compact slot avoids shifting neighboring menu extras as
        // recording and processing states change. Template images follow macOS.
        let status = NSStatusBar::systemStatusBar().statusItemWithLength(28.);
        let name = test_build::application_name();
        status.setAutosaveName(Some(if test_build::active() {
            ns_string!("VoicePortTestStatusItem")
        } else {
            ns_string!("VoicePortStatusItem")
        }));
        if let Some(button) = status.button(mtm) {
            self.update_status_icon(&button, StatusAppearance::Ready);
            button.setAccessibilityIdentifier(Some(ns_string!("voiceport.menu-bar")));
            button.setAccessibilityLabel(Some(&NSString::from_str(&format!("{name} · 就绪"))));
            button.setToolTip(Some(&NSString::from_str(&format!(
                "{name} · 点击选择转写方式"
            ))));
            unsafe {
                button.setTarget(Some(self));
                button.setAction(Some(sel!(showQuickPanel:)));
            }
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
        }
        status.setVisible(true);
        let _ = self.ivars().status.set(status);

        let main_menu = NSMenu::new(mtm);
        let application_item = NSMenuItem::new(mtm);
        let application_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(name));
        application_menu.addItem(&self.menu_item("结果与设置…", sel!(showWindow:), ","));
        application_menu.addItem(&self.menu_item(
            "转写音频文件…",
            sel!(chooseFileTranscription:),
            "o",
        ));
        application_menu.addItem(&NSMenuItem::separatorItem(mtm));
        application_menu.addItem(&self.menu_item(&format!("退出 {name}"), sel!(terminate:), "q"));
        application_item.setSubmenu(Some(&application_menu));
        main_menu.addItem(&application_item);
        NSApplication::sharedApplication(mtm).setMainMenu(Some(&main_menu));
    }
    fn update_status_icon(&self, button: &NSStatusBarButton, appearance: StatusAppearance) {
        if self.ivars().status_appearance.get() == Some(appearance) {
            return;
        }
        let image = crate::status_icon::image();
        button.setTitle(ns_string!(""));
        button.setImage(Some(&image));
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        let tint = (appearance == StatusAppearance::Recording).then(NSColor::systemRedColor);
        button.setContentTintColor(tint.as_deref());
        self.ivars().status_appearance.set(Some(appearance));
    }
    fn initialize(&self) {
        let defaults = NSUserDefaults::standardUserDefaults();
        let mut controller = Controller::new();
        if test_build::active() {
            controller.automatic = false;
        }
        controller.microphone_uid = microphone::preference(&defaults);
        controller.set_recent_dictations_enabled(recent_dictations::preference(&defaults));
        if defaults
            .objectForKey(ns_string!("automaticPaste"))
            .is_some()
        {
            controller.automatic = defaults.boolForKey(ns_string!("automaticPaste"));
        }
        controller.language = defaults
            .stringForKey(ns_string!("language"))
            .map(|s| s.to_string())
            .filter(|s| voiceport_core::language::supported(s))
            .unwrap_or_default();
        controller.source_id = defaults
            .stringForKey(ns_string!("transcriptionSource"))
            .map(|s| s.to_string())
            .unwrap_or_else(|| crate::local_models::CHATGPT.into());
        dictation_workflows::load(&mut controller, &defaults);
        controller.catalog = crate::local_models::Catalog::pending(&controller.source_id);
        controller.refresh_catalog();
        let saved = defaults
            .stringForKey(ns_string!("dictationShortcut"))
            .map(|s| s.to_string());
        let shortcut = crate::shortcut::Shortcut::load(
            saved.as_deref(),
            defaults.boolForKey(ns_string!("alternativeShortcut")),
        );
        self.ivars().shortcut_binding.set(shortcut);
        let mode = defaults
            .stringForKey(&NSString::from_str(
                crate::shortcut::ShortcutMode::PREFERENCE,
            ))
            .map(|v| v.to_string());
        self.ivars()
            .shortcut_mode
            .set(crate::shortcut::ShortcutMode::load(mode.as_deref()));
        *self.ivars().controller.borrow_mut() = Some(controller);
        let _ = crate::storage::cleanup_expired();
        if (!test_build::active() || saved.is_some())
            && let Err(message) = self.install_hotkey(shortcut)
        {
            *self.ivars().hotkey_error.borrow_mut() = Some(message.clone());
            self.with_controller(|c| c.message = message);
        }
        self.install_repaste_shortcut();
        self.install_timer();
    }
    fn install_timer(&self) {
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.1,
                self,
                sel!(tick:),
                None,
                true,
            )
        };
        // TerminateLater and native panels run a modal loop. The shutdown
        // poll must keep running there, as well as while a menu is tracking.
        unsafe {
            let run_loop = NSRunLoop::currentRunLoop();
            run_loop.addTimer_forMode(&timer, NSRunLoopCommonModes);
            run_loop.addTimer_forMode(&timer, NSModalPanelRunLoopMode);
            run_loop.addTimer_forMode(&timer, NSEventTrackingRunLoopMode);
        }
        let _ = self.ivars().timer.set(timer);
    }
    fn install_hotkey(&self, shortcut: crate::shortcut::Shortcut) -> Result<(), String> {
        if self.ivars().hotkey.borrow().is_some() && shortcut == self.ivars().shortcut_binding.get()
        {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let mut hotkey = crate::hotkey::GlobalHotKey::new(tx);
        hotkey.register(shortcut).map_err(|error| {
            self.ivars()
                .hotkey_registration_failure
                .set(Some(error.state()));
            error.to_string()
        })?;
        self.ivars().hotkey_registration_failure.set(None);
        // Commit only after Carbon accepted the candidate. Dropping the old
        // owner and queue discards stale presses from the previous binding.
        *self.ivars().hotkey.borrow_mut() = Some(hotkey);
        *self.ivars().hotkey_rx.borrow_mut() = Some(rx);
        self.with_controller(|c| c.set_stop_shortcut(Some(shortcut.stop_chord())));
        self.ivars().hotkey_error.borrow_mut().take();
        Ok(())
    }
    fn button(
        &self,
        view: &NSView,
        title: &str,
        action: objc2::runtime::Sel,
        x: f64,
        y: f64,
        width: f64,
    ) -> Retained<NSButton> {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        };
        button.setFrame(rect(x, y, width, 32.));
        button.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
            "voiceport.{}",
            action.name().to_string_lossy()
        ))));
        view.addSubview(&button);
        button
    }
    fn label(&self, view: &NSView, text: &str, frame: NSRect, size: f64) -> Retained<NSTextField> {
        let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), self.mtm());
        label.setFrame(frame);
        label.setFont(Some(&NSFont::systemFontOfSize(size)));
        label.setTextColor(Some(&NSColor::labelColor()));
        view.addSubview(&label);
        label
    }
    fn caption(&self, view: &NSView, text: &str, frame: NSRect) -> Retained<NSTextField> {
        let label = self.label(view, text, frame, 11.);
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        label
    }
    fn surface(&self, view: &NSView, frame: NSRect) {
        let card = NSBox::initWithFrame(NSBox::alloc(self.mtm()), frame);
        card.setBoxType(NSBoxType::Custom);
        card.setTitlePosition(NSTitlePosition::NoTitle);
        card.setCornerRadius(10.);
        card.setBorderWidth(0.5);
        card.setBorderColor(&NSColor::separatorColor());
        card.setFillColor(&NSColor::controlBackgroundColor());
        view.addSubview(&card);
    }
    fn icon(&self, view: &NSView, symbol: &str, frame: NSRect) -> Retained<NSImageView> {
        let image = NSImageView::initWithFrame(NSImageView::alloc(self.mtm()), frame);
        image.setImage(symbol_image(symbol, frame.size.height).as_deref());
        image.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        view.addSubview(&image);
        image
    }
    fn popup(
        &self,
        view: &NSView,
        titles: &[&str],
        action: objc2::runtime::Sel,
        frame: NSRect,
    ) -> Retained<NSPopUpButton> {
        let popup =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(self.mtm()), frame, false);
        for title in titles {
            popup.addItemWithTitle(&NSString::from_str(title));
        }
        unsafe {
            popup.setTarget(Some(self));
            popup.setAction(Some(action));
        }
        popup.setFont(Some(&NSFont::systemFontOfSize(12.)));
        view.addSubview(&popup);
        popup
    }
    fn select_page(&self, settings: bool) {
        if let Some(workbench) = self.ivars().workbench.get() {
            workbench.select(if settings {
                workbench::Page::Settings
            } else {
                workbench::Page::Result
            });
        }
    }
    fn build_workbench(
        &self,
        language: workbench::Language,
    ) -> (Retained<NSWindow>, Retained<workbench::Workbench>) {
        use workbench::Page;
        let mtm = self.mtm();
        let (window, shell) = workbench::Workbench::build(mtm, language);
        let frame = shell.page_bounds();
        let view = file_transcription::DropArea::new(self, frame);
        let result = self
            .ivars()
            .dictation_result
            .get_or_init(|| dictation_page::Page::build(mtm, self, &view));
        shell.mount(Page::Result, result.view(), &result.key_views());
        let settings = NSView::initWithFrame(NSView::alloc(mtm), frame);
        self.build_settings_page(&settings);
        let iv = self.ivars();
        let slots = iv.settings_input_slots.get().expect("settings input slots");
        let self_check = slots.self_check.subviews().firstObject().unwrap();
        let input_device = slots.input_device.subviews().firstObject().unwrap();
        shell.mount(
            Page::Settings,
            &settings,
            &[
                iv.recent_dictations_switch.get().unwrap(),
                iv.shortcut_mode_control.get().unwrap(),
                iv.shortcut.get().unwrap(),
                iv.language.get().unwrap(),
                iv.dictionary_settings.get().unwrap(),
                iv.preset_settings.get().unwrap(),
                iv.app_rules_settings.get().unwrap(),
                iv.automatic.get().unwrap(),
                iv.accessibility.get().unwrap(),
                iv.prepare_permissions.get().unwrap(),
                &self_check,
                &input_device,
                iv.source.get().unwrap(),
                iv.manage_models.get().unwrap(),
                iv.refresh_models.get().unwrap(),
                iv.manage_sources.get().unwrap(),
                iv.test_source.get().unwrap(),
            ],
        );
        let history = iv
            .history_page
            .get_or_init(|| recent_dictations::workbench::HistoryPage::build(self, frame));
        shell.mount(Page::History, history, &history.key_views());
        let files = self.file_page_with_bounds(frame);
        shell.mount(Page::Files, &files.content_view(), &files.key_views());
        let sources = iv
            .source_page
            .get_or_init(|| source_page::Page::build(self, frame));
        shell.mount(Page::Sources, sources, &sources.key_views());
        let device = iv
            .microphone_panel
            .get_or_init(|| microphone::build(self, frame));
        shell.mount(Page::Device, &device.view(), &device.tab_views());
        let (updates, keys) = self.build_update_page(frame);
        shell.mount(Page::Updates, &updates, &keys.each_ref().map(|key| &**key));
        shell.select(Page::default());
        (window, shell)
    }
    fn show_window(&self) {
        self.present_workbench(None);
    }
    fn show_workbench_page(&self, page: workbench::Page) {
        self.present_workbench(Some(page));
    }
    fn prepare_workbench(&self, page: Option<workbench::Page>) -> &NSWindow {
        let iv = self.ivars();
        let window = iv.window.get_or_init(|| {
            let (window, shell) = self.build_workbench(workbench::Language::preferred());
            window.setDelegate(Some(ProtocolObject::from_ref(self)));
            window.center();
            let _ = iv.workbench.set(shell);
            window
        });
        if let Some(page) = page {
            iv.workbench
                .get()
                .expect("workbench for window")
                .select(page);
        }
        window
    }
    fn present_workbench(&self, page: Option<workbench::Page>) {
        if self.ivars().quitting.get() {
            return;
        }
        self.close_quick_panel();
        let window = self.prepare_workbench(page);
        window.makeKeyAndOrderFront(None);
        self.refresh();
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
    }
    fn clear_workbench_history(&self) {
        if let Some(history) = self.ivars().history_page.get() {
            history.clear_display();
        }
    }
    fn refresh(&self) {
        self.refresh_opened_update_page();
        let Ok(slot) = self.ivars().controller.try_borrow() else {
            return;
        };
        let Some(c) = slot.as_ref() else {
            return;
        };
        #[cfg(debug_assertions)]
        let overlay_enabled =
            self.ivars().audio_fixture_stroke.get().is_none() && status_overlay::enabled();
        #[cfg(not(debug_assertions))]
        let overlay_enabled = status_overlay::enabled();
        if let Ok(mut overlay) = self.ivars().overlay.try_borrow_mut() {
            if self.ivars().quitting.get() {
                overlay.close();
            } else {
                overlay.update(self.mtm(), c.overlay_snapshot(), overlay_enabled, || {
                    c.capture_level()
                });
            }
        }
        if self.ivars().context_menu_visible.get() {
            self.apply_overlay_menu();
        }
        let trusted = c.accessibility_trusted();
        let snapshot = self.refresh_menu_snapshot(c, trusted);
        self.refresh_quick_panel(c, false);
        self.refresh_first_run(c, trusted);
        if !self
            .ivars()
            .window
            .get()
            .is_some_and(|window| window.isVisible() && !window.isMiniaturized())
        {
            return;
        }
        let preparing_permissions = self.ivars().permission_rx.borrow().is_some();
        let returning_to_input = self.ivars().pending_hotkey.borrow().is_some();
        if let Some(button) = self.ivars().prepare_permissions.get() {
            let enabled = !c.busy() && !preparing_permissions && !returning_to_input;
            if button.isEnabled() != enabled {
                button.setEnabled(enabled);
            }
            let title = if c.uses_system_speech() {
                "检查麦克风与语音权限…"
            } else {
                "检查麦克风权限（此来源）…"
            };
            if button.title().to_string() != title {
                button.setTitle(&NSString::from_str(title));
            }
        }
        let recording = c.state.phase == Phase::Recording;
        let state_text = snapshot.state;
        let shortcut = snapshot.shortcut.as_str();
        let mode = self.ivars().shortcut_mode.get();
        let shortcut_instruction = mode.instruction();
        let hotkey_failed = self.ivars().hotkey_error.borrow().is_some();
        let idle = snapshot.idle;
        if let Some(history) = self.ivars().history_page.get() {
            history.refresh(c);
        }
        if let Some(sources) = self.ivars().source_page.get()
            && !sources.isHiddenOrHasHiddenAncestor()
        {
            sources.refresh(c, idle && !c.discovering());
        }
        for button in [
            self.ivars().dictionary_settings.get(),
            self.ivars().preset_settings.get(),
            self.ivars().app_rules_settings.get(),
        ]
        .into_iter()
        .flatten()
        {
            button.setEnabled(idle);
        }
        if let Some(button) = self.ivars().manage_models.get() {
            button.setEnabled((idle && !c.discovering()) || self.ivars().managing_models.get());
        }
        if let Some(button) = self.ivars().manage_sources.get() {
            button.setEnabled(idle && !c.discovering());
        }
        if let Some(button) = self.ivars().test_source.get() {
            button.setEnabled((idle || c.source_tests.busy()) && !c.discovering());
        }
        if let Some(control) = self.ivars().source.get() {
            let rows: Vec<_> = c
                .catalog
                .entries
                .iter()
                .map(|entry| (entry.id.clone(), entry.title.clone()))
                .collect();
            if *self.ivars().source_rows.borrow() != rows {
                picker::set_rows(control, &rows);
                *self.ivars().source_rows.borrow_mut() = rows;
            }
            picker::select_id(control, &c.source_id);
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(label) = self.ivars().source_detail.get() {
            let detail = c.catalog.detail(&c.source_id);
            if label.stringValue().to_string() != detail {
                label.setStringValue(&NSString::from_str(detail));
            }
            label.setToolTip(Some(&NSString::from_str(detail)));
        }
        if let Some(button) = self.ivars().refresh_models.get() {
            let enabled = idle && !c.discovering();
            if button.isEnabled() != enabled {
                button.setEnabled(enabled);
            }
        }
        if let Some(page) = self.ivars().dictation_result.get() {
            let release_notice = c.audio_release_notice();
            let capture_stage = c.capture_stage();
            let receipt = c.gui_dictation_receipt();
            let diagnostics = c.gui_dictation_diagnostics();
            let origin = c.visible_transcript.map(|source| match source {
                crate::controller::VisibleTranscript::Preview(generation) => {
                    dictation_page::TranscriptOrigin {
                        generation,
                        final_text: false,
                        incomplete: false,
                    }
                }
                crate::controller::VisibleTranscript::Final(generation) => {
                    dictation_page::TranscriptOrigin {
                        generation,
                        final_text: true,
                        incomplete: false,
                    }
                }
                crate::controller::VisibleTranscript::Incomplete(generation) => {
                    dictation_page::TranscriptOrigin {
                        generation,
                        final_text: true,
                        incomplete: true,
                    }
                }
            });
            let context = c.gui_result_context();
            let activity_title = release_notice
                .or_else(|| capture_stage.map(|stage| stage.title()))
                .or_else(|| {
                    (!c.state.can_start()
                        || preparing_permissions
                        || returning_to_input
                        || c.source_tests.busy())
                    .then_some(state_text)
                });
            let rule_title = c
                .active_app_rule()
                .map(|rule| format!("{} · {rule}", activity_title.unwrap_or(state_text)));
            page.refresh(dictation_page::Snapshot {
                attempt: receipt
                    .as_ref()
                    .map(|receipt| dictation_page::ResultDetails {
                        receipt,
                        diagnostics: diagnostics.as_ref(),
                        source: context.source.as_deref(),
                        target: context.target.as_deref(),
                    }),
                retained: c
                    .retained_gui_result()
                    .map(|result| dictation_page::ResultDetails {
                        receipt: &result.receipt,
                        diagnostics: Some(&result.diagnostics),
                        source: result.context.source.as_deref(),
                        target: result.context.target.as_deref(),
                    }),
                origin,
                original: &c.transcript,
                raw_available: c.raw_transcript.is_some(),
                can_open_raw: idle,
                shortcut: &format!(
                    "{shortcut}  {}",
                    if hotkey_failed {
                        "快捷键未注册"
                    } else {
                        shortcut_instruction
                    }
                ),
                activity: dictation_page::Activity {
                    title: rule_title.as_deref().or(activity_title),
                    recording,
                    can_toggle: !preparing_permissions
                        && !returning_to_input
                        && !self.settings_locked()
                        && (recording || (!c.busy() && release_notice.is_none())),
                    can_cancel: (c.busy() && !c.file_jobs.busy())
                        || returning_to_input
                        || (!self.settings_locked() && c.gui_fragment_retained()),
                    can_transcribe_fragment: !self.settings_locked()
                        && interrupted_dictation::FragmentPlan::read_gui(c).is_some(),
                    can_retry: !self.settings_locked()
                        && c.can_retry_recording()
                        && receipt
                            .as_ref()
                            .is_some_and(|r| r.generation == c.state.generation()),
                },
            });
        }
        if let Some(control) = self.ivars().automatic.get() {
            let state = if c.automatic {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            };
            if control.state() != state {
                control.setState(state);
            }
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(control) = self.ivars().recent_dictations_switch.get() {
            let state = status_overlay::checked(c.recent_dictations_enabled());
            if control.state() != state {
                control.setState(state);
            }
            control.setEnabled(!self.ivars().quitting.get());
        }
        if let Some(control) = self.ivars().language.get() {
            let selected = voiceport_core::language::index(&c.language) as isize;
            // Re-selecting even the same item dismisses an open native menu.
            // Common-mode timer ticks must leave active control tracking alone.
            if control.indexOfSelectedItem() != selected {
                control.selectItemAtIndex(selected);
            }
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(label) = self.ivars().shortcut_feedback.get() {
            let notice = self.ivars().shortcut_notice.borrow();
            let text = notice.as_deref().unwrap_or(if hotkey_failed {
                "快捷键不可用，请换一组或点自定义。"
            } else {
                "选择预设，或点自定义后试按组合键。"
            });
            label.setStringValue(&NSString::from_str(text));
            label.setToolTip(Some(&NSString::from_str(&c.message)));
        }
        if let Some(control) = self.ivars().shortcut_mode_control.get() {
            if control.indexOfSelectedItem() != mode.index() {
                control.selectItemAtIndex(mode.index());
            }
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(control) = self.ivars().shortcut.get() {
            let selected = self.ivars().shortcut_binding.get().preset_index();
            if let Some(item) = control.itemAtIndex(2) {
                let title = if selected == 2 {
                    format!("{shortcut} · 自定义…")
                } else {
                    "自定义…".into()
                };
                if item.title().to_string() != title {
                    item.setTitle(&NSString::from_str(&title));
                }
            }
            if control.indexOfSelectedItem() != selected {
                control.selectItemAtIndex(selected);
            }
            control.setToolTip(Some(&NSString::from_str(if hotkey_failed {
                "快捷键未注册，请选择另一组。"
            } else {
                shortcut_instruction
            })));
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(button) = self.ivars().accessibility.get() {
            button.setTitle(if trusted {
                ns_string!("已授权 · 检查")
            } else {
                ns_string!("未授权 · 设置")
            });
            button.setAccessibilityLabel(Some(if trusted {
                ns_string!("辅助功能已授权，点击检查")
            } else {
                ns_string!("辅助功能未授权，前往系统设置")
            }));
        }
    }
}
fn symbol_image(name: &str, size: f64) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let configuration =
        NSImageSymbolConfiguration::configurationWithPointSize_weight(size, unsafe {
            NSFontWeightRegular
        });
    let image = image
        .imageWithSymbolConfiguration(&configuration)
        .unwrap_or(image);
    image.setTemplate(true);
    image.setSize(NSSize::new(size, size));
    Some(image)
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

pub fn run() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if let Some((mode, args)) = arguments.split_first()
        && mode == "--apply-staged-update"
    {
        if let Err(error) = crate::app_update::apply(args) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(debug_assertions)]
    if let [flag, directory, rest @ ..] = arguments.as_slice()
        && flag == "--quick-panel-fixture"
        && rest.len() <= 1
    {
        let mtm = MainThreadMarker::new().expect("fixture starts on the main thread");
        objc2::rc::autoreleasepool(|_| {
            quick_panel::fixture::run(
                mtm,
                std::path::Path::new(directory),
                rest.first().map(std::path::Path::new),
            )
        });
        return;
    }
    if arguments.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "{} Rust macOS\n  --show-window             Explicitly open results and settings (skip the first-run window)\n  --show-models             Open the local speech model library\n  --smoke-test              Build menu and exit without recording or network\n  --check-input-permission  Print input authorization JSON for this execution context\n  --diagnose-input          Inspect focused input metadata without writing\n  --input-self-test         Write fixed text only to the dedicated development fixture\n  --help                   Show this help",
            test_build::application_name()
        );
        return;
    }
    if arguments.iter().any(|a| {
        !matches!(
            a.as_str(),
            "--smoke-test"
                | "--check-input-permission"
                | "--show-window"
                | "--show-models"
                | "--diagnose-input"
                | "--input-self-test"
        )
    }) {
        eprintln!("Unknown option; use --help");
        std::process::exit(2);
    }
    let mtm = MainThreadMarker::new().expect("VoicePort starts on the main thread");
    if arguments
        .iter()
        .any(|a| a == "--diagnose-input" || a == "--input-self-test")
    {
        let report = if arguments.iter().any(|a| a == "--input-self-test") {
            Delivery::input_self_test()
        } else {
            Delivery::diagnose_input()
        };
        println!("{report}");
        return;
    }
    if arguments.iter().any(|a| a == "--check-input-permission") {
        let bundle = NSBundle::mainBundle();
        let version = bundle
            .objectForInfoDictionaryKey(ns_string!("CFBundleShortVersionString"))
            .and_then(|o| o.downcast::<NSString>().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
        let (microphone, speech) = crate::audio::Recorder::permission_status();
        println!(
            "{}",
            serde_json::json!({"accessibilityTrusted":Delivery::trusted(),"secureInput":Delivery::secure_input(),"microphone":microphone,"speech":speech,"bundlePath":bundle.bundlePath().to_string(),"version":version,"implementation":"rust"})
        );
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    let smoke = arguments.iter().any(|a| a == "--smoke-test");
    // A menu bar utility can still present a normal settings window without
    // occupying the Dock or replacing the foreground app's application menu.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let delegate = Delegate::new(mtm);
    delegate
        .ivars()
        .show_window_on_launch
        .set(arguments.iter().any(|a| a == "--show-window"));
    delegate
        .ivars()
        .show_models_on_launch
        .set(arguments.iter().any(|a| a == "--show-models"));
    if smoke {
        delegate.build_menu();
        println!("VoicePort Rust app launched: menu ready, no recording or network");
        return;
    }
    if let Err(response) = delegate.start_control() {
        if let Some(error) = response.error {
            eprintln!("VoicePort control: {}: {}", error.code, error.message);
        }
        std::process::exit(1);
    }
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    delegate.initialize();
    app.run();
    if let Some(timer) = delegate.ivars().timer.get() {
        timer.invalidate();
    }
}
