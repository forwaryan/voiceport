//! Full dictation page. The editable AppKit buffer is a view-local copy, never a delivery input.
use objc2::{
    MainThreadOnly, Message, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, Sel},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use std::cell::{Cell, RefCell};
use voiceport_control::{
    CaptureSignal, DeliveryState, DictationBlockReason as Reason, GuiDictationDiagnostics,
    GuiDictationReceipt, TranscriptionState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptOrigin {
    pub(crate) generation: u64,
    pub(crate) final_text: bool,
}

#[derive(Clone, Copy)]
pub(super) struct Activity {
    pub(super) title: Option<&'static str>,
    pub(super) recording: bool,
    pub(super) can_toggle: bool,
    pub(super) can_cancel: bool,
    pub(super) can_retry: bool,
}

pub(super) struct Snapshot<'a> {
    pub(super) receipt: Option<&'a GuiDictationReceipt>,
    pub(super) diagnostics: Option<&'a GuiDictationDiagnostics>,
    pub(super) origin: Option<TranscriptOrigin>,
    pub(super) original: &'a str,
    pub(super) raw_available: bool,
    pub(super) can_open_raw: bool,
    pub(super) shortcut: &'a str,
    pub(super) activity: Activity,
}

fn receipt_text(receipt: &GuiDictationReceipt) -> &'static str {
    use TranscriptionState::*;
    match receipt.transcription {
        NotStarted => "本轮未开始",
        Pending => "本轮进行中",
        Empty => "本轮没有识别到文字",
        Failed => "本轮转写失败",
        Cancelled => "本轮已取消",
        Completed => match receipt.delivery.state {
            DeliveryState::NotSent if matches!(receipt.block_reason, Some(Reason::PanelOnly)) => {
                "转写完成 · 仅在面板显示"
            }
            DeliveryState::NotSent
                if matches!(receipt.block_reason, Some(Reason::RetryPanelOnly)) =>
            {
                "重试完成 · 未自动输入"
            }
            DeliveryState::NotSent => "转写完成 · 未投递",
            DeliveryState::Waiting => "转写完成 · 等待投递",
            DeliveryState::AwaitingConfirmation => "转写完成 · 等待目标确认",
            DeliveryState::PartiallySent => "转写完成 · 已执行部分写入",
            DeliveryState::Confirmed => "转写完成 · 原输入框已确认",
            DeliveryState::Submitted => "转写完成 · 已提交，接收未验证",
            DeliveryState::Uncertain => "转写完成 · 投递结果不明",
        },
    }
}

fn reason_text(reason: Reason) -> &'static str {
    match reason {
        Reason::PanelOnly => "此轮只在面板显示。",
        Reason::RetryPanelOnly => "重试只转写，不自动向原输入框重投。",
        Reason::StartRejected => "未能开始；请检查当前任务和所选来源。",
        Reason::SourceUnavailable => "请在设置中检查转写来源。",
        Reason::UnsupportedLanguage => "所选来源不支持当前语言；请检查语言设置。",
        Reason::MicrophoneDenied => "未获得麦克风权限；请在系统设置中检查。",
        Reason::CaptureFailed => "录音未能完成；请检查所选输入设备。",
        Reason::TranscriptionFailed => "转写未完成；仅在录音可复用时提供重试。",
        Reason::EmptyTranscript => "未生成最终文字；展开诊断可查看采集状况。",
        Reason::Cancelled => "已停止本轮操作；不会撤回已提交的文字。",
    }
}

fn next_step(receipt: Option<&GuiDictationReceipt>) -> &'static str {
    let Some(receipt) = receipt else {
        return "用快捷键或本页按钮开始听写。";
    };
    if matches!(
        receipt.delivery.state,
        DeliveryState::Uncertain | DeliveryState::Submitted
    ) || (receipt.delivery.stopped && receipt.delivery.submitted_operations > 0)
    {
        return "先检查目标是否已有文字，再决定是否手动粘贴。不会自动重投。";
    }
    if let Some(reason) = receipt.block_reason {
        return reason_text(reason);
    }
    if receipt.transcription == TranscriptionState::Completed
        && receipt.delivery.state == DeliveryState::NotSent
    {
        return "可复制结果后手动粘贴；不会自动重投。";
    }
    "可展开诊断查看本轮采集、转写与投递回执。"
}

fn details(
    receipt: Option<&GuiDictationReceipt>,
    diagnostics: Option<&GuiDictationDiagnostics>,
) -> String {
    let Some(r) = receipt else {
        return "还没有 GUI 听写回执。".into();
    };
    let count = |n: Option<u64>| n.map_or_else(|| "—".into(), |n| n.to_string());
    let ms = |n: Option<u64>| n.map_or_else(|| "—".into(), |n| format!("{n} ms"));
    let mut text = format!(
        "会话 {} · 尝试 {}\n{}\n自动输入请求：{}\n路径：{}\n失败阶段：{}\n写入操作：提交 {} / 读回确认 {}\n已停止：{}",
        r.session,
        r.generation,
        receipt_text(r),
        if r.automatic { "是" } else { "否" },
        r.method.as_deref().unwrap_or("—"),
        r.failure_stage.as_deref().unwrap_or("—"),
        r.delivery.submitted_operations,
        r.delivery.confirmed_operations,
        if r.delivery.stopped { "是" } else { "否" }
    );
    if let Some(reason) = r.block_reason {
        text.push_str(&format!("\n原因：{}", reason_text(reason)));
    }
    if let Some(d) = diagnostics.filter(|d| (d.session, d.generation) == (r.session, r.generation))
    {
        if let Some(c) = d.capture {
            let signal = match c.signal {
                CaptureSignal::NoCallbacks => "无回调",
                CaptureSignal::NoFrames => "无帧",
                CaptureSignal::DigitalSilence => "数字静音",
                CaptureSignal::LowEnergy => "信号过低",
                CaptureSignal::Signal => "检测到信号",
                CaptureSignal::Unknown => "未知",
            };
            text.push_str(&format!(
                "\n采集尝试 {}：{signal}\n回调 {} · 帧 {}",
                c.generation,
                count(c.callbacks),
                count(c.observed_frames)
            ));
        } else {
            text.push_str("\n采集：无已封存记录");
        }
        let t = d.timings;
        text.push_str(&format!(
            "\n首帧：{} · 准备：{}\n转写：{} · 投递：{}",
            ms(t.capture_first_frame_ms),
            ms(t.preparation_ms),
            ms(t.transcription_ms),
            ms(t.delivery_ms)
        ));
    } else {
        text.push_str("\n无匹配的本轮采集或耗时记录。");
    }
    text
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

// NSTextView otherwise uses Tab to insert a literal tab while editing. Only this
// local text control changes its Tab behavior; no global event monitor is used.
define_class!(
    #[unsafe(super = NSTextView)]
    #[thread_kind = MainThreadOnly]
    struct ResultText;
    unsafe impl NSObjectProtocol for ResultText {}
    impl ResultText {
        #[unsafe(method(insertTab:))]
        fn tab(&self, _sender: Option<&AnyObject>) {
            if let Some(w) = self.window() { w.selectKeyViewFollowingView(self); }
        }
        #[unsafe(method(insertBacktab:))]
        fn backtab(&self, _sender: Option<&AnyObject>) {
            if let Some(w) = self.window() { w.selectKeyViewPrecedingView(self); }
        }
    }
);

pub(super) struct Page {
    area: Retained<NSView>,
    headline: Retained<NSTextField>,
    guidance: Retained<NSTextField>,
    result_label: Retained<NSTextField>,
    shortcut: Retained<NSTextField>,
    feedback: Retained<NSTextField>,
    pub(super) result: Retained<NSTextView>,
    result_scroll: Retained<NSScrollView>,
    pub(super) diagnostic: Retained<NSTextView>,
    diagnostic_scroll: Retained<NSScrollView>,
    pub(super) disclosure: Retained<NSButton>,
    pub(super) edit: Retained<NSButton>,
    pub(super) copy: Retained<NSButton>,
    pub(super) raw_original: Retained<NSButton>,
    toggle: Retained<NSButton>,
    pub(super) open_file: Retained<NSButton>,
    cancel: Retained<NSButton>,
    retry: Retained<NSButton>,
    origin: Cell<Option<TranscriptOrigin>>,
    original: RefCell<String>,
    editing: Cell<bool>,
}

fn label(
    mtm: MainThreadMarker,
    area: &NSView,
    text: &str,
    frame: NSRect,
    size: f64,
) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    label.setFrame(frame);
    label.setTextColor(Some(&NSColor::labelColor()));
    label.setFont(Some(&NSFont::systemFontOfSize(size)));
    area.addSubview(&label);
    label
}

fn button(
    mtm: MainThreadMarker,
    target: &AnyObject,
    area: &NSView,
    title: &str,
    action: Sel,
    id: &str,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    };
    b.setFrame(frame);
    b.setAccessibilityIdentifier(Some(&NSString::from_str(id)));
    b.setAccessibilityLabel(Some(&NSString::from_str(title)));
    area.addSubview(&b);
    b
}

fn text_area(
    mtm: MainThreadMarker,
    area: &NSView,
    frame: NSRect,
    id: &str,
    name: &str,
) -> (Retained<NSScrollView>, Retained<NSTextView>) {
    let scroller = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroller.setHasVerticalScroller(true);
    scroller.setAutohidesScrollers(true);
    scroller.setBorderType(NSBorderType::BezelBorder);
    // initWithFrame: is inherited by this AppKit subclass.
    let text: Retained<ResultText> = unsafe {
        msg_send![ResultText::alloc(mtm), initWithFrame: rect(0., 0., frame.size.width, frame.size.height)]
    };
    let text: Retained<NSTextView> = text.into_super();
    text.setEditable(false);
    text.setSelectable(true);
    text.setRichText(false);
    text.setImportsGraphics(false);
    text.setAutomaticLinkDetectionEnabled(false);
    text.setAutomaticTextReplacementEnabled(false);
    text.setAutomaticQuoteSubstitutionEnabled(false);
    text.setAutomaticDashSubstitutionEnabled(false);
    text.setContinuousSpellCheckingEnabled(false);
    text.setGrammarCheckingEnabled(false);
    text.setUsesFindBar(false);
    text.setFont(Some(&NSFont::systemFontOfSize(13.)));
    text.setTextColor(Some(&NSColor::textColor()));
    text.setBackgroundColor(&NSColor::textBackgroundColor());
    text.setTextContainerInset(NSSize::new(8., 7.));
    text.setVerticallyResizable(true);
    text.setHorizontallyResizable(false);
    text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    if let Some(container) = unsafe { text.textContainer() } {
        container.setWidthTracksTextView(true);
    }
    text.setAccessibilityIdentifier(Some(&NSString::from_str(id)));
    text.setAccessibilityLabel(Some(&NSString::from_str(name)));
    scroller.setDocumentView(Some(&text));
    area.addSubview(&scroller);
    (scroller, text)
}

fn set_label(label: &NSTextField, text: &str) {
    let text = NSString::from_str(text);
    if *label.stringValue() != *text {
        label.setStringValue(&text);
    }
}

fn set_title(button: &NSButton, text: &str) {
    let text = NSString::from_str(text);
    if *button.title() != *text {
        button.setTitle(&text);
        button.setAccessibilityLabel(Some(&text));
    }
}

impl Page {
    pub(super) fn build(mtm: MainThreadMarker, target: &AnyObject, area: &NSView) -> Self {
        let make = |title, action, id, x, y, w| {
            button(mtm, target, area, title, action, id, rect(x, y, w, 32.))
        };
        let heading = label(mtm, area, "听写结果", rect(0., 376., 246., 30.), 21.);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(21.)));
        let open_file = make(
            "转写音频文件…",
            sel!(chooseFileTranscription:),
            "voiceport.dictation.open-file",
            250.,
            375.,
            155.,
        );
        let disclosure = make(
            "展开本轮诊断",
            sel!(toggleDictationDiagnostics:),
            "voiceport.dictation.diagnostics-toggle",
            415.,
            375.,
            157.,
        );
        disclosure.setAccessibilityExpanded(false);
        let headline = label(mtm, area, "就绪", rect(2., 344., 568., 25.), 16.);
        let guidance = label(mtm, area, "", rect(2., 305., 568., 35.), 12.);
        let result_label = label(mtm, area, "转写正文", rect(2., 276., 153., 24.), 13.);
        let raw_original = make(
            "查看词典处理前文字",
            sel!(showOriginalTranscript:),
            "voiceport.dictation.raw-original",
            157.,
            272.,
            139.,
        );
        raw_original.setHidden(true);
        let edit = make(
            "编辑副本",
            sel!(editDictationCopy:),
            "voiceport.dictation.edit-copy",
            306.,
            272.,
            128.,
        );
        let copy = make(
            "复制原文",
            sel!(copyDictationResult:),
            "voiceport.dictation.copy",
            444.,
            272.,
            128.,
        );
        let (result_scroll, result) = text_area(
            mtm,
            area,
            rect(0., 104., 572., 162.),
            "voiceport.transcript",
            "转写正文（只读）",
        );
        let (diagnostic_scroll, diagnostic) = text_area(
            mtm,
            area,
            rect(302., 104., 270., 162.),
            "voiceport.dictation.diagnostics",
            "本轮诊断详情（只读）",
        );
        diagnostic_scroll.setHidden(true);
        let feedback = label(mtm, area, "", rect(2., 69., 568., 31.), 12.);
        let toggle = make(
            "开始听写",
            sel!(toggle:),
            "voiceport.dictation.start-stop",
            0.,
            27.,
            136.,
        );
        let cancel = make(
            "取消听写",
            sel!(cancel:),
            "voiceport.dictation.cancel",
            348.,
            27.,
            104.,
        );
        let retry = make(
            "重试转写",
            sel!(retry:),
            "voiceport.dictation.retry-recording",
            460.,
            27.,
            112.,
        );
        let shortcut = label(mtm, area, "", rect(152., 30., 187., 25.), 12.);
        label(
            mtm,
            area,
            "复制和编辑不会发送文字；重试转写不会自动输入。",
            rect(2., 0., 566., 22.),
            12.,
        );
        Self {
            area: area.retain(),
            headline,
            guidance,
            result_label,
            shortcut,
            feedback,
            result,
            result_scroll,
            diagnostic,
            diagnostic_scroll,
            disclosure,
            edit,
            copy,
            raw_original,
            toggle,
            open_file,
            cancel,
            retry,
            origin: Cell::new(None),
            original: RefCell::new(String::new()),
            editing: Cell::new(false),
        }
    }

    // Link document views themselves, not the NSScrollView wrappers. Native
    // AppKit still decides which disabled/hidden views are valid key targets.
    fn key_views(&self) -> [&NSView; 10] {
        [
            &self.disclosure,
            &self.open_file,
            &self.raw_original,
            &self.edit,
            &self.copy,
            &self.result,
            &self.diagnostic,
            &self.toggle,
            &self.cancel,
            &self.retry,
        ]
    }

    pub(super) fn link_key_views(&self, first: &NSView) {
        let views = self.key_views();
        for (previous, next) in std::iter::once(first)
            .chain(views)
            .zip(views.into_iter().chain(std::iter::once(first)))
        {
            unsafe {
                previous.setNextKeyView(Some(next));
            }
        }
    }

    pub(super) fn refresh(&self, s: Snapshot<'_>) {
        let title = s
            .activity
            .title
            .or_else(|| s.receipt.map(receipt_text))
            .unwrap_or("就绪");
        set_label(&self.headline, title);
        set_label(&self.guidance, next_step(s.receipt));
        let origin_changed = self.origin.replace(s.origin) != s.origin;
        if origin_changed || self.original.borrow().as_str() != s.original {
            *self.original.borrow_mut() = s.original.into();
            self.editing.set(false);
            self.result.setEditable(false);
            self.result.setString(&NSString::from_str(s.original));
            set_label(&self.feedback, "");
        }
        let is_final = s.origin.is_some_and(|o| o.final_text) && !s.original.trim().is_empty();
        let current_round = s
            .origin
            .zip(s.receipt)
            .is_some_and(|(o, r)| o.generation == r.generation);
        let name = if self.editing.get() {
            "编辑副本"
        } else if s.original.trim().is_empty() {
            "暂无正文"
        } else if !is_final && s.activity.recording {
            "实时预览（非最终文字）"
        } else if !is_final {
            "保留预览（非最终文字）"
        } else if current_round {
            "本轮转写正文"
        } else {
            "其他尝试的保留正文"
        };
        set_label(&self.result_label, name);
        let ax_name = NSString::from_str(&format!(
            "{name}（{}）",
            if self.editing.get() {
                "可编辑"
            } else {
                "只读"
            }
        ));
        if self.result.accessibilityLabel().as_deref() != Some(&*ax_name) {
            self.result.setAccessibilityLabel(Some(&ax_name));
        }
        self.edit.setEnabled(is_final);
        self.copy.setEnabled(is_final);
        let raw_visible = is_final && current_round && s.raw_available;
        self.raw_original.setHidden(!raw_visible);
        self.raw_original.setEnabled(raw_visible && s.can_open_raw);
        if s.original.trim().is_empty() && !self.result_scroll.isHidden() {
            if let Some(w) = self.area.window()
                && w.firstResponder()
                    .as_deref()
                    .is_some_and(|r| std::ptr::eq(r, &****self.result))
            {
                w.makeFirstResponder(Some(&self.disclosure));
            }
            self.result_scroll.setHidden(true);
        } else if !s.original.trim().is_empty() {
            self.result_scroll.setHidden(false);
        }
        set_title(
            &self.edit,
            if self.editing.get() {
                "放弃副本"
            } else {
                "编辑副本"
            },
        );
        set_title(
            &self.copy,
            if self.editing.get() {
                "复制副本"
            } else if raw_visible {
                "复制正文"
            } else {
                "复制原文"
            },
        );
        set_title(
            &self.toggle,
            if s.activity.recording {
                "停止并转写"
            } else {
                "开始听写"
            },
        );
        self.toggle.setEnabled(s.activity.can_toggle);
        self.cancel.setEnabled(s.activity.can_cancel);
        self.retry.setEnabled(s.activity.can_retry);
        set_label(&self.shortcut, s.shortcut);
        if !self.diagnostic_scroll.isHidden() {
            let detail = NSString::from_str(&details(s.receipt, s.diagnostics));
            if *self.diagnostic.string() != *detail {
                self.diagnostic.setString(&detail);
            }
        }
    }

    pub(super) fn toggle_details(&self) {
        let open = self.diagnostic_scroll.isHidden();
        if !open && let Some(w) = self.area.window() {
            w.makeFirstResponder(Some(&self.disclosure));
        }
        self.diagnostic_scroll.setHidden(!open);
        self.result_scroll
            .setFrame(rect(0., 104., if open { 288. } else { 572. }, 162.));
        set_title(
            &self.disclosure,
            if open {
                "收起本轮诊断"
            } else {
                "展开本轮诊断"
            },
        );
        self.disclosure.setAccessibilityExpanded(open);
    }

    pub(super) fn toggle_edit(&self) {
        if !self.edit.isEnabled() {
            return;
        }
        let was_editing = self.editing.replace(!self.editing.get());
        self.result.setEditable(!was_editing);
        if was_editing {
            self.result
                .setString(&NSString::from_str(self.original.borrow().as_str()));
            set_label(&self.feedback, "已放弃副本，原转写未改变。");
        } else {
            if let Some(w) = self.area.window() {
                w.makeFirstResponder(Some(&self.result));
            }
            set_label(&self.feedback, "正在编辑临时副本。原转写与投递不变。");
        }
    }

    pub(super) fn copy(&self, write: impl FnOnce(&str) -> bool) {
        if !self.copy.isEnabled() {
            return;
        }
        let text = self.result.string().to_string();
        let feedback = if text.trim().is_empty() {
            "当前副本没有可复制的文字。"
        } else if write(&text) {
            if self.editing.get() {
                "副本已复制，原转写未改变。"
            } else {
                "原文已复制。"
            }
        } else {
            "复制失败，请选择文字后手动复制。"
        };
        set_label(&self.feedback, feedback);
    }
}
