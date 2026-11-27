//! Read-only readiness plus explicit repairs and an AppKit-owned input check.
use super::*;
use crate::audio::Recorder;
use objc2_foundation::NSURL;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use voiceport_control::{
    Response,
    readiness::{self, Hotkey, Permission},
};

mod field;

#[derive(Clone, Copy)]
pub(super) enum PermissionCheck {
    CurrentSource,
    Microphone,
    Speech,
}

pub(super) struct Panel {
    window: Retained<NSWindow>,
    microphone: Retained<NSTextField>,
    speech: Retained<NSTextField>,
    accessibility: Retained<NSTextField>,
    shortcut: Retained<NSTextField>,
    repair_microphone: Retained<NSButton>,
    repair_speech: Retained<NSButton>,
    repair_accessibility: Retained<NSButton>,
    repair_shortcut: Retained<NSButton>,
    transcribe_only: Retained<NSButton>,
    field: Retained<NSTextView>,
    result: Retained<NSTextField>,
    updated: Cell<Option<Instant>>,
}

impl Panel {
    fn new(delegate: &Delegate) -> Self {
        let mtm = delegate.mtm();
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0., 0., 560., 468.),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        let name = voiceport_core::test_build::application_name();
        window.setTitle(&NSString::from_str(&format!("{name} · 首次使用与自检")));
        let view = window.contentView().expect("first-run content view");
        let heading = delegate.label(&view, "听写就绪检查", rect(24., 416., 510., 28.), 21.);
        heading.setFont(Some(&NSFont::systemFontOfSize_weight(21., unsafe {
            NSFontWeightSemibold
        })));
        delegate.caption(
            &view,
            "按需处理权限，或选择仅转写。",
            rect(24., 391., 510., 20.),
        );
        delegate.surface(&view, rect(24., 191., 512., 188.));
        let row = |title, y, selector| {
            delegate.label(&view, title, rect(38., y + 12., 323., 20.), 13.);
            let status = delegate.caption(&view, "", rect(38., y - 4., 348., 18.));
            let repair = delegate.button(&view, "设置…", selector, 399., y, 123.);
            (status, repair)
        };
        let (microphone, repair_microphone) =
            row("麦克风 · 听写必需", 342., sel!(firstRunMicrophone:));
        let (speech, repair_speech) = row("系统语音识别 · 可选预览", 298., sel!(firstRunSpeech:));
        let (accessibility, repair_accessibility) = row(
            "辅助功能 · 自动输入需要",
            254.,
            sel!(firstRunAccessibility:),
        );
        let (shortcut, repair_shortcut) = row("全局快捷键", 210., sel!(firstRunShortcut:));
        repair_shortcut.setTitle(ns_string!("快捷键设置…"));
        repair_accessibility.setTitle(ns_string!("前往授权…"));

        delegate.label(&view, "原位输入自检", rect(24., 158., 512., 22.), 15.);
        delegate.caption(
            &view,
            "固定样例写入下方测试框。此项检查应用内投递，外部应用兼容性需另测。",
            rect(24., 134., 512., 23.),
        );
        let field = NSTextView::initWithFrame(NSTextView::alloc(mtm), rect(24., 82., 340., 48.));
        configure_field(&field);
        field.setAccessibilityLabel(Some(ns_string!("原位输入专用测试框")));
        field.setAccessibilityIdentifier(Some(ns_string!("voiceport.first-run.fixture")));
        view.addSubview(&field);
        delegate.button(
            &view,
            "运行输入自检",
            sel!(runFirstRunCheck:),
            380.,
            89.,
            156.,
        );
        let result = delegate.caption(&view, "起始光标在 A 与 B 之间。", rect(24., 53., 512., 26.));
        result.setAccessibilityIdentifier(Some(ns_string!("voiceport.first-run.result")));
        let transcribe_only = delegate.button(
            &view,
            "改为仅转写",
            sel!(firstRunTranscribeOnly:),
            24.,
            13.,
            132.,
        );
        let close = delegate.button(&view, "完成", sel!(closeFirstRunChecks:), 428., 13., 108.);
        close.setKeyEquivalent(ns_string!("\r"));
        window.center();
        Self {
            window,
            microphone,
            speech,
            accessibility,
            shortcut,
            repair_microphone,
            repair_speech,
            repair_accessibility,
            repair_shortcut,
            transcribe_only,
            field,
            result,
            updated: Cell::new(None),
        }
    }

    fn render(
        &self,
        readiness: readiness::Readiness,
        automatic: bool,
        uses_speech: bool,
        chord: &str,
        can_repair: bool,
    ) {
        set_label(&self.microphone, permission_label(readiness.microphone));
        let speech = if uses_speech {
            "ChatGPT 设备端预览"
        } else {
            "当前来源不使用"
        };
        set_label(
            &self.speech,
            &format!("{} · {speech}", permission_label(readiness.speech)),
        );
        let input_mode = if automatic {
            "自动输入已开启"
        } else {
            "仅转写"
        };
        set_label(
            &self.accessibility,
            &format!(
                "{} · {input_mode}",
                permission_label(readiness.accessibility)
            ),
        );
        set_label(
            &self.shortcut,
            &format!("{chord} · {}", hotkey_label(readiness.hotkey)),
        );
        render_permission_button(
            &self.repair_microphone,
            readiness.microphone,
            true,
            can_repair,
        );
        render_permission_button(
            &self.repair_speech,
            readiness.speech,
            uses_speech,
            can_repair,
        );
        self.repair_accessibility
            .setHidden(!automatic || readiness.accessibility != Permission::Denied);
        self.repair_accessibility.setEnabled(can_repair);
        self.repair_shortcut
            .setHidden(readiness.hotkey == Hotkey::Registered);
        self.repair_shortcut
            .setEnabled(can_repair && readiness.hotkey != Hotkey::Suspended);
        self.transcribe_only.setHidden(!automatic);
        self.transcribe_only.setEnabled(can_repair);
    }

    fn check_input(&self) -> Result<(), &'static str> {
        let result = field::check(&self.field);
        let message = match result {
            Ok(()) => "通过 · 样例插入 A 与 B 之间，仅写入一次。模拟光标移动时已停止。".into(),
            Err(reason) => format!("自检未通过 · {reason}"),
        };
        set_label(&self.result, &message);
        result
    }
}

impl Delegate {
    pub(super) fn show_first_run_if_new(&self) {
        let seen =
            NSUserDefaults::standardUserDefaults().boolForKey(ns_string!("didShowFirstRunChecks"));
        if !seen && !self.ivars().show_models_on_launch.get() {
            self.show_first_run_checks();
        }
    }

    pub(super) fn show_first_run_checks(&self) {
        self.close_quick_panel();
        let panel = self.ivars().first_run.get_or_init(|| Panel::new(self));
        panel.updated.set(None);
        panel.window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        NSUserDefaults::standardUserDefaults()
            .setBool_forKey(true, ns_string!("didShowFirstRunChecks"));
        self.refresh();
    }

    pub(super) fn close_first_run_checks(&self) {
        if let Some(panel) = self.ivars().first_run.get() {
            panel.window.close();
        }
    }

    pub(super) fn run_first_run_check(&self) {
        let Some(panel) = self
            .ivars()
            .first_run
            .get()
            .filter(|panel| panel.window.isVisible())
        else {
            return;
        };
        panel.window.makeFirstResponder(Some(&panel.field));
        let _ = panel.check_input();
    }

    pub(super) fn refresh_first_run(&self, c: &Controller, trusted: bool) {
        let Some(panel) = self
            .ivars()
            .first_run
            .get()
            .filter(|panel| panel.window.isVisible())
        else {
            return;
        };
        let now = Instant::now();
        if panel
            .updated
            .get()
            .is_some_and(|previous| now.duration_since(previous) < Duration::from_millis(500))
        {
            return;
        }
        panel.updated.set(Some(now));
        let (microphone, speech) = Recorder::permission_status();
        let bundle = NSBundle::mainBundle().bundlePath().to_string();
        let pid = std::process::id();
        let status = Response::ok(serde_json::json!({
            "bundle_path": bundle, "pid": pid, "source": c.source_id,
            "permissions": {"microphone": microphone, "speech": speech, "accessibility": trusted},
            "hotkey": {"registration": self.hotkey_registration()},
            "input": {"automatic": c.automatic},
        }));
        panel.render(
            readiness::assess(Path::new(&bundle), Some(pid), &status),
            c.automatic,
            c.uses_system_speech(),
            &self.ivars().shortcut_binding.get().label(),
            self.can_prepare_permissions() && !c.busy(),
        );
    }

    fn can_prepare_permissions(&self) -> bool {
        !self.ivars().quitting.get()
            && self.ivars().permission_rx.borrow().is_none()
            && self.ivars().pending_hotkey.borrow().is_none()
            && !self.settings_locked()
    }

    pub(super) fn prepare_permission_check(&self, check: PermissionCheck) {
        if !self.can_prepare_permissions() {
            return;
        }
        let Ok(slot) = self.ivars().controller.try_borrow() else {
            return;
        };
        let Some(c) = slot.as_ref().filter(|c| !c.busy()) else {
            return;
        };
        let use_speech = c.uses_system_speech();
        let (status, pane, message) = match check {
            PermissionCheck::CurrentSource if use_speech => {
                ("", "", "正在准备麦克风与系统语音权限；不会开始录音。")
            }
            PermissionCheck::CurrentSource => ("", "", "正在准备麦克风权限；不会开始录音。"),
            PermissionCheck::Microphone => (
                Recorder::permission_status().0,
                "Privacy_Microphone",
                "正在准备麦克风权限；不会开始录音。",
            ),
            PermissionCheck::Speech if use_speech => (
                Recorder::permission_status().1,
                "Privacy_SpeechRecognition",
                "正在准备预览权限；不会开始录音。",
            ),
            PermissionCheck::Speech => return,
        };
        drop(slot);
        if status == "denied" {
            let url = format!("x-apple.systempreferences:com.apple.preference.security?{pane}");
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(&url)) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
            return;
        }
        if !matches!(status, "" | "notDetermined") {
            return;
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        *self.ivars().permission_rx.borrow_mut() = Some(receiver);
        self.with_controller(|c| c.message = message.into());
        self.refresh();
        match check {
            PermissionCheck::Speech => Recorder::request_speech_preview(sender),
            _ => Recorder::request_permissions_for_source(
                0,
                sender,
                matches!(check, PermissionCheck::CurrentSource) && use_speech,
            ),
        }
    }

    pub(super) fn request_first_run_accessibility(&self) {
        let can_request = self.can_prepare_permissions()
            && self
                .ivars()
                .controller
                .try_borrow()
                .is_ok_and(|slot| slot.as_ref().is_some_and(|c| !c.busy() && c.automatic));
        if can_request && !Delivery::trusted() {
            Delivery::request_accessibility();
        }
    }

    pub(super) fn transcribe_only_from_first_run(&self) {
        if !self.can_prepare_permissions() {
            return;
        }
        self.with_controller(|c| {
            if !c.busy() && c.automatic {
                c.automatic = false;
                NSUserDefaults::standardUserDefaults()
                    .setBool_forKey(false, ns_string!("automaticPaste"));
            }
        });
        if let Some(panel) = self.ivars().first_run.get() {
            panel.updated.set(None);
        }
        self.refresh();
    }
}

fn configure_field(field: &NSTextView) {
    field.setEditable(false);
    field.setSelectable(true);
    field.setRichText(false);
    field.setDrawsBackground(true);
    field.setBackgroundColor(&NSColor::textBackgroundColor());
    field.setTextColor(Some(&NSColor::textColor()));
    field.setFont(Some(&NSFont::systemFontOfSize(17.)));
    field.setTextContainerInset(NSSize::new(8., 12.));
    field::reset(field);
}

fn set_label(label: &NSTextField, text: &str) {
    if label.stringValue().to_string() != text {
        label.setStringValue(&NSString::from_str(text));
    }
}

fn permission_label(status: Permission) -> &'static str {
    match status {
        Permission::Granted => "已授权",
        Permission::Denied => "已拒绝或未授权",
        Permission::Restricted => "受系统限制",
        Permission::NotDetermined => "尚未询问",
        Permission::Unavailable => "系统接口不可用",
        Permission::Unknown => "状态未知",
    }
}

fn hotkey_label(status: Hotkey) -> &'static str {
    match status {
        Hotkey::Registered => "已注册",
        Hotkey::Conflict => "注册冲突 · 已被占用",
        Hotkey::RegistrationFailed => "注册失败",
        Hotkey::Suspended => "编辑中 · 已暂停",
        Hotkey::Unknown => "注册状态未知",
    }
}

fn can_request_or_open(status: Permission) -> bool {
    matches!(status, Permission::NotDetermined | Permission::Denied)
}

fn render_permission_button(button: &NSButton, status: Permission, relevant: bool, enabled: bool) {
    button.setHidden(!relevant || !can_request_or_open(status));
    button.setEnabled(enabled);
    let title = if status == Permission::NotDetermined {
        "请求授权…"
    } else {
        "系统设置…"
    };
    if button.title().to_string() != title {
        button.setTitle(&NSString::from_str(title));
    }
}

/// Run offline assertions against the same AppKit panel without ordering it front.
#[doc(hidden)]
pub fn first_run_field_fixture() -> Result<(), &'static str> {
    let mtm = MainThreadMarker::new().ok_or("first-run fixture requires the main thread")?;
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    let delegate = Delegate::new(mtm);
    let panel = Panel::new(&delegate);
    assert!(!panel.window.isVisible());
    let readiness = |microphone, speech, accessibility, hotkey| readiness::Readiness {
        instance: readiness::Instance::Matched,
        microphone,
        speech,
        accessibility,
        hotkey,
        next_steps: Vec::new(),
    };
    panel.render(
        readiness(
            Permission::NotDetermined,
            Permission::Denied,
            Permission::Denied,
            Hotkey::Conflict,
        ),
        true,
        true,
        "⌃⌥空格",
        true,
    );
    assert_eq!(panel.microphone.stringValue().to_string(), "尚未询问");
    assert_eq!(panel.repair_microphone.title().to_string(), "请求授权…");
    assert_eq!(panel.repair_speech.title().to_string(), "系统设置…");
    assert!(panel.speech.stringValue().to_string().contains("ChatGPT"));
    assert!(
        panel
            .shortcut
            .stringValue()
            .to_string()
            .contains("注册冲突")
    );
    for action in [
        &panel.repair_microphone,
        &panel.repair_speech,
        &panel.repair_accessibility,
        &panel.repair_shortcut,
        &panel.transcribe_only,
    ] {
        assert!(!action.isHidden() && action.isEnabled());
    }
    panel.render(
        readiness(
            Permission::Granted,
            Permission::Denied,
            Permission::Denied,
            Hotkey::Registered,
        ),
        false,
        false,
        "⌃⌥空格",
        true,
    );
    assert_eq!(panel.microphone.stringValue().to_string(), "已授权");
    assert!(
        panel
            .speech
            .stringValue()
            .to_string()
            .contains("当前来源不使用")
    );
    assert!(
        panel
            .accessibility
            .stringValue()
            .to_string()
            .contains("仅转写")
    );
    assert!(panel.shortcut.stringValue().to_string().contains("已注册"));
    for action in [
        &panel.repair_microphone,
        &panel.repair_speech,
        &panel.repair_accessibility,
        &panel.repair_shortcut,
        &panel.transcribe_only,
    ] {
        assert!(action.isHidden());
    }
    panel.check_input()?;
    panel.check_input()?;
    assert!(panel.result.stringValue().to_string().starts_with("通过"));
    assert!(!panel.window.isVisible());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_distinguishes_conflict_from_other_registration_failures() {
        assert!(hotkey_label(Hotkey::Conflict).contains("冲突"));
        for status in [
            Hotkey::Registered,
            Hotkey::RegistrationFailed,
            Hotkey::Suspended,
            Hotkey::Unknown,
        ] {
            assert!(!hotkey_label(status).contains("冲突"));
        }
    }

    #[test]
    fn first_run_permission_repair_requires_a_requestable_or_denied_status() {
        assert!(can_request_or_open(Permission::NotDetermined));
        assert!(can_request_or_open(Permission::Denied));
        for status in [
            Permission::Granted,
            Permission::Restricted,
            Permission::Unavailable,
            Permission::Unknown,
        ] {
            assert!(!can_request_or_open(status));
        }
    }
}
