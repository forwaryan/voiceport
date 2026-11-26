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

#[derive(Clone, Copy)]
pub(super) struct ResultDetails<'a> {
    pub(super) receipt: &'a GuiDictationReceipt,
    pub(super) diagnostics: Option<&'a GuiDictationDiagnostics>,
    pub(super) source: Option<&'a str>,
    pub(super) target: Option<&'a str>,
}

pub(super) struct Snapshot<'a> {
    pub(super) attempt: Option<ResultDetails<'a>>,
    pub(super) retained: Option<ResultDetails<'a>>,
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
        Completed if receipt.block_reason == Some(Reason::RetryPanelOnly) => {
            "重试完成 · 正文保留在工作台"
        }
        Completed => delivery_text(receipt),
    }
}

fn delivery_text(receipt: &GuiDictationReceipt) -> &'static str {
    match receipt.delivery.state {
        DeliveryState::NotSent if receipt.block_reason == Some(Reason::PanelOnly) => {
            "仅在工作台显示"
        }
        DeliveryState::NotSent => "未提交写入",
        DeliveryState::Waiting => "等待写入",
        DeliveryState::AwaitingConfirmation => "等待原输入框读回",
        DeliveryState::PartiallySent => "已执行部分写入",
        DeliveryState::Confirmed if confirmed_in_place(receipt) => "原输入框已确认",
        DeliveryState::Confirmed => "保留原输入框的读回记录",
        DeliveryState::Submitted => "已提交，接收未验证",
        DeliveryState::Uncertain => "投递结果不明",
    }
}

fn confirmed_in_place(receipt: &GuiDictationReceipt) -> bool {
    let delivery = receipt.delivery;
    delivery.state == DeliveryState::Confirmed
        && delivery.submitted_operations > 0
        && delivery.confirmed_operations == delivery.submitted_operations
        && !delivery.stopped
        && receipt.failure_stage.is_none()
}

fn needs_inspection(receipt: &GuiDictationReceipt) -> bool {
    let delivery = receipt.delivery;
    matches!(
        delivery.state,
        DeliveryState::Uncertain | DeliveryState::Submitted | DeliveryState::PartiallySent
    ) || (delivery.state == DeliveryState::Confirmed && !confirmed_in_place(receipt))
        || (delivery.stopped && delivery.submitted_operations > 0)
}

impl<'a> Snapshot<'a> {
    fn text_details(&self) -> Option<ResultDetails<'a>> {
        let origin = self.origin?;
        self.attempt
            .into_iter()
            .chain(self.retained)
            .find(|details| details.receipt.generation == origin.generation)
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
        Reason::EmptyTranscript => "展开投递详情可查看本轮采集状况。",
        Reason::Cancelled => "已停止本轮操作；不会撤回已提交的文字。",
    }
}

fn next_step(receipt: Option<&GuiDictationReceipt>) -> &'static str {
    let Some(receipt) = receipt else {
        return "用快捷键或本页按钮开始听写。";
    };
    if !receipt.delivery.stopped && receipt.failure_stage.is_none() {
        match receipt.delivery.state {
            DeliveryState::Waiting | DeliveryState::AwaitingConfirmation => {
                return "请等待本轮投递结算后再检查原输入框。";
            }
            DeliveryState::PartiallySent => {
                return "已有部分写入，请等待本轮投递结算后再检查原输入框。";
            }
            _ => {}
        }
    }
    if needs_inspection(receipt) {
        return "先检查原输入框已有的内容，再决定是否复制正文并手动补写。";
    }
    if let Some(reason) = receipt.block_reason {
        return reason_text(reason);
    }
    if receipt.transcription == TranscriptionState::Completed
        && receipt.delivery.state == DeliveryState::NotSent
    {
        return if receipt.failure_stage.as_deref() == Some("focus") {
            "原输入焦点不可用或已改变。完整文字可手动复制。"
        } else {
            "完整文字可手动复制。"
        };
    }
    ""
}

fn details(shown: ResultDetails<'_>, newer: Option<ResultDetails<'_>>) -> String {
    let r = shown.receipt;
    let automatic = if r.automatic { "是" } else { "否" };
    let stopped = if r.delivery.stopped { "是" } else { "否" };
    let target = shown.target.unwrap_or(if r.automatic {
        "未取得原应用信息"
    } else {
        "仅工作台"
    });
    let confirmation = if confirmed_in_place(r) {
        "已读回原输入框并确认全部写入"
    } else if r.delivery.confirmed_operations > 0 {
        "已有操作通过读回；整轮未确认"
    } else {
        "未取得原输入框的读回确认"
    };
    let is_retry = r.session != r.generation;
    let delivery_label = if is_retry {
        "原尝试的投递回执"
    } else {
        "投递回执"
    };
    let count = |n: Option<u64>| n.map_or_else(|| "—".into(), |n| n.to_string());
    let ms = |n: Option<u64>| n.map_or_else(|| "—".into(), |n| format!("{n} ms"));
    let mut text = format!(
        "原目标应用：{target}\n{delivery_label}：{}\n原位确认：{confirmation}\n来源设置：{}\n\n会话 {} · 尝试 {}\n自动输入请求：{automatic}\n路径：{}\n失败阶段：{}\n写入操作：提交 {} / 读回确认 {}\n已停止：{stopped}",
        delivery_text(r),
        shown.source.unwrap_or("无本轮来源记录"),
        r.session,
        r.generation,
        r.method.as_deref().unwrap_or("—"),
        r.failure_stage.as_deref().unwrap_or("—"),
        r.delivery.submitted_operations,
        r.delivery.confirmed_operations,
    );
    if is_retry {
        text.push_str("\n上方写入与读回记录属于原尝试。重试转写仅在工作台保留正文。");
    }
    if let Some(reason) = r.block_reason {
        text.push_str(&format!("\n原因：{}", reason_text(reason)));
    }
    if let Some(d) = shown
        .diagnostics
        .filter(|d| (d.session, d.generation) == (r.session, r.generation))
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
    if let Some(newer) = newer {
        text.push_str(&format!(
            "\n\n最近一次尝试：{}",
            receipt_text(newer.receipt)
        ));
        if let Some(reason) = newer.receipt.block_reason {
            text.push_str(&format!("\n{}", reason_text(reason)));
        }
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
    empty: Retained<NSTextField>,
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
        let NSSize { width, height } = area.bounds().size;
        let top = |y| height - 408. + y;
        let trailing = |x| width - 572. + x;
        let make = |title, action, id, x, y, w| {
            button(mtm, target, area, title, action, id, rect(x, y, w, 32.))
        };
        let heading = label(mtm, area, "听写结果", rect(0., top(376.), 180., 30.), 21.);
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(21.)));
        let open_file = make(
            "转写音频文件…",
            sel!(chooseFileTranscription:),
            "voiceport.dictation.open-file",
            trailing(270.),
            top(375.),
            145.,
        );
        let disclosure = make(
            "展开投递详情",
            sel!(toggleDictationDiagnostics:),
            "voiceport.dictation.diagnostics-toggle",
            trailing(422.),
            top(375.),
            150.,
        );
        disclosure.setAccessibilityExpanded(false);
        let headline = label(mtm, area, "就绪", rect(2., top(344.), width - 4., 25.), 16.);
        let guidance = label(mtm, area, "", rect(2., top(305.), width - 4., 35.), 12.);
        let result_label = label(
            mtm,
            area,
            "转写正文",
            rect(2., top(276.), trailing(175.), 24.),
            13.,
        );
        let empty = label(
            mtm,
            area,
            "完成听写后，正文会显示在这里。",
            rect(2., top(233.), width - 4., 28.),
            13.,
        );
        empty.setTextColor(Some(&NSColor::secondaryLabelColor()));
        empty.setHidden(true);
        let raw_original = make(
            "查看词典处理前文字",
            sel!(showOriginalTranscript:),
            "voiceport.dictation.raw-original",
            trailing(178.),
            top(272.),
            146.,
        );
        raw_original.setHidden(true);
        let edit = make(
            "编辑副本",
            sel!(editDictationCopy:),
            "voiceport.dictation.edit-copy",
            trailing(331.),
            top(272.),
            116.,
        );
        let copy = make(
            "复制原文",
            sel!(copyDictationResult:),
            "voiceport.dictation.copy",
            trailing(454.),
            top(272.),
            118.,
        );
        let (result_scroll, result) = text_area(
            mtm,
            area,
            rect(0., 104., width, top(162.)),
            "voiceport.transcript",
            "转写正文（只读）",
        );
        let (diagnostic_scroll, diagnostic) = text_area(
            mtm,
            area,
            rect(width / 2. + 7., 104., width / 2. - 7., top(162.)),
            "voiceport.dictation.diagnostics",
            "投递详情（只读）",
        );
        diagnostic_scroll.setHidden(true);
        let feedback = label(mtm, area, "", rect(2., 69., width - 4., 31.), 12.);
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
            trailing(348.),
            27.,
            104.,
        );
        let retry = make(
            "重试转写",
            sel!(retry:),
            "voiceport.dictation.retry-recording",
            trailing(460.),
            27.,
            112.,
        );
        let shortcut = label(mtm, area, "", rect(152., 30., 187., 25.), 12.);
        Self {
            area: area.retain(),
            headline,
            guidance,
            result_label,
            empty,
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

    pub(super) fn view(&self) -> &NSView {
        &self.area
    }

    // Link document views themselves, not the NSScrollView wrappers. Native
    // AppKit still decides which disabled/hidden views are valid key targets.
    pub(super) fn key_views(&self) -> [&NSView; 10] {
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

    pub(super) fn refresh(&self, s: Snapshot<'_>) {
        let attempt = s.attempt.map(|details| details.receipt);
        let text_details = s.text_details();
        let text_receipt = text_details.map(|details| details.receipt);
        let title = s
            .activity
            .title
            .or_else(|| attempt.map(receipt_text))
            .unwrap_or("就绪");
        set_label(&self.headline, title);

        let current_round = s
            .origin
            .zip(attempt)
            .is_some_and(|(origin, receipt)| origin.generation == receipt.generation);
        let has_text = !s.original.trim().is_empty();
        let is_final = s.origin.is_some_and(|origin| origin.final_text) && has_text;
        let displayed_receipt = if has_text { text_receipt } else { attempt };
        let guidance = if has_text && !current_round {
            if text_receipt.is_some_and(needs_inspection) {
                "下方是保留正文。先检查它的原输入框，再决定是否复制并补写。"
            } else {
                "下方是保留正文。最新尝试的状态显示在上方。"
            }
        } else {
            next_step(displayed_receipt)
        };
        set_label(&self.guidance, guidance);

        let can_edit = is_final
            && text_receipt.is_none_or(|receipt| {
                !needs_inspection(receipt)
                    && !matches!(
                        receipt.delivery.state,
                        DeliveryState::Waiting | DeliveryState::AwaitingConfirmation
                    )
            });
        let origin_changed = self.origin.replace(s.origin) != s.origin;
        if origin_changed
            || self.original.borrow().as_str() != s.original
            || (self.editing.get() && !can_edit)
        {
            *self.original.borrow_mut() = s.original.into();
            self.editing.set(false);
            self.result.setEditable(false);
            self.result.setString(&NSString::from_str(s.original));
            set_label(&self.feedback, "");
        }
        let name = if self.editing.get() {
            "编辑副本"
        } else if !has_text {
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
        self.edit.setEnabled(can_edit);
        self.copy.setEnabled(is_final);
        let raw_visible = is_final && current_round && s.raw_available;
        self.raw_original.setHidden(!raw_visible);
        self.raw_original.setEnabled(raw_visible && s.can_open_raw);
        self.disclosure
            .setEnabled(s.attempt.is_some() || text_details.is_some());
        if !has_text
            && !self.result_scroll.isHidden()
            && let Some(w) = self.area.window()
            && w.firstResponder()
                .as_deref()
                .is_some_and(|r| std::ptr::eq(r, &****self.result))
        {
            let fallback = if self.disclosure.isEnabled() {
                &self.disclosure
            } else {
                &self.open_file
            };
            w.makeFirstResponder(Some(fallback));
        }
        if self.result_scroll.isHidden() == has_text {
            self.result_scroll.setHidden(!has_text);
            self.empty.setHidden(has_text);
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
        self.retry
            .setEnabled(s.activity.can_retry && !attempt.is_some_and(needs_inspection));
        set_label(&self.shortcut, s.shortcut);
        if !self.diagnostic_scroll.isHidden() {
            if !self.disclosure.isEnabled() {
                self.toggle_details();
                return;
            }
            let detail = if let Some(shown) = text_details {
                let newer = s
                    .attempt
                    .filter(|attempt| attempt.receipt.generation != shown.receipt.generation);
                details(shown, newer)
            } else if let Some(attempt) = s.attempt {
                let description = details(attempt, None);
                if has_text {
                    format!("当前正文没有对应的 GUI 回执。\n\n最近一次 GUI 尝试\n{description}")
                } else {
                    description
                }
            } else {
                String::new()
            };
            let detail = NSString::from_str(&detail);
            if *self.diagnostic.string() != *detail {
                self.diagnostic.setString(&detail);
            }
        }
    }

    pub(super) fn toggle_details(&self) {
        let open = self.diagnostic_scroll.isHidden();
        if open && !self.disclosure.isEnabled() {
            return;
        }
        if !open
            && let Some(w) = self.area.window()
            && w.firstResponder()
                .as_deref()
                .is_some_and(|r| std::ptr::eq(r, &****self.diagnostic))
        {
            let fallback = if self.disclosure.isEnabled() {
                &self.disclosure
            } else {
                &self.open_file
            };
            w.makeFirstResponder(Some(fallback));
        }
        self.diagnostic_scroll.setHidden(!open);
        if !open {
            self.diagnostic.setString(&NSString::from_str(""));
        }
        let NSSize { width, height } = self.area.bounds().size;
        self.result_scroll.setFrame(rect(
            0.,
            104.,
            if open { width / 2. - 7. } else { width },
            height - 246.,
        ));
        set_title(
            &self.disclosure,
            if open {
                "收起投递详情"
            } else {
                "展开投递详情"
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
