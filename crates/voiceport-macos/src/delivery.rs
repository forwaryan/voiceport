//! Focused macOS text delivery. Construction is inert; `begin` captures the
//! target only after the user starts dictation. Call every operation on the
//! application main thread. WeChat's unreadable main window has a guarded
//! clipboard/Command+V path; no menu, helper process or Return key is used.
#[cfg(test)]
mod callback_tests;
#[cfg(test)]
mod compatibility_matrix;
#[doc(hidden)]
pub mod model;
mod native;
pub(crate) use native::overlay_focus::OverlayFocus;

use model::{
    ChunkedInput, Failure, MenuPaste, OwnedSpan, PasteTarget, Target, Terminal, TerminalTarget,
};

const RETAINED: &str = "无法确认原输入框支持安全写入，文字保留在面板。";
const PASTE_REQUESTED: &str = "已请求原输入位置粘贴，请检查输入框；最终文字已复制。";
const PREPARING: &str = "正在准备原输入框，请保持当前输入位置。";

enum Session {
    Ordinary(OwnedSpan<Box<dyn Target>>),
    Keyboard(ChunkedInput<Box<dyn Target>>),
    Terminal(Terminal<Box<dyn TerminalTarget>>),
    MenuPaste(MenuPaste<Box<dyn PasteTarget>>),
}

impl Session {
    fn receipt(&self) -> model::DeliveryReceipt {
        match self {
            Self::Ordinary(span) => span.receipt(),
            Self::Keyboard(input) => input.receipt(),
            Self::Terminal(terminal) => terminal.receipt(),
            Self::MenuPaste(paste) => paste.receipt(),
        }
    }
}

/// Holds text only for the currently captured field and never logs its content.
/// The native target and monitor make this type intentionally !Send / !Sync.
pub struct Delivery {
    session: Option<Session>,
    failure: Option<Failure>,
    preparing: Option<Box<dyn native::Preparation>>,
    prepared_text: Option<(String, bool)>,
    method: Option<&'static str>,
    receipt: model::DeliveryReceipt,
    final_accepted: bool,
    stop_shortcut: Option<model::StopShortcut>,
}

impl Default for Delivery {
    fn default() -> Self {
        Self::new()
    }
}

impl Delivery {
    pub fn new() -> Self {
        Self {
            session: None,
            failure: None,
            preparing: None,
            prepared_text: None,
            method: None,
            receipt: model::DeliveryReceipt::default(),
            final_accepted: false,
            stop_shortcut: None,
        }
    }

    /// Affects the next capture; existing native monitors keep their own copy.
    /// Supply only a successfully registered chord, or None after unregistering.
    #[doc(hidden)]
    pub fn set_stop_shortcut(&mut self, shortcut: Option<model::StopShortcut>) {
        self.stop_shortcut = shortcut;
    }

    fn on_main(&mut self) -> Result<(), Failure> {
        if native::main_thread() {
            Ok(())
        } else {
            self.failure = Some(Failure::MainThread);
            Err(Failure::MainThread)
        }
    }

    /// Records every preparation failure without prompting or modifying the
    /// frontend's permission state. Only Accessibility and our own frontmost
    /// application block recording; other failures allow panel-only dictation.
    pub fn begin(&mut self) -> Result<(), String> {
        self.begin_in_application(None)
    }
    #[doc(hidden)]
    pub fn begin_in_application(
        &mut self,
        target: Option<&crate::delivery::model::ApplicationTarget>,
    ) -> Result<(), String> {
        let shortcut = self.stop_shortcut;
        self.begin_with_capture(|| native::capture(target, shortcut))
    }

    /// Bind an owned fixture directly to the ordinary delivery path. The
    /// caller supplies the whole target boundary, so native capture is skipped.
    pub(crate) fn begin_fixture(&mut self, target: Box<dyn Target>) -> Result<(), String> {
        self.begin_with_capture(|| Ok(native::Captured::Ordinary(target)))
    }

    fn begin_with_capture(
        &mut self,
        capture: impl FnOnce() -> Result<native::Captured, Failure>,
    ) -> Result<(), String> {
        self.on_main().map_err(Failure::message)?;
        self.clear_inner();
        if self.has_pending() {
            return Err("正在确认上一轮输入，请稍后重新开始。".into());
        }
        self.failure = None;
        self.method = None;
        self.receipt = model::DeliveryReceipt::default();
        self.final_accepted = false;
        let result = capture().and_then(|capture| self.accept_capture(capture));
        match result {
            Ok(()) => Ok(()),
            Err(failure) => {
                self.failure = Some(failure);
                Err(failure.message())
            }
        }
    }

    fn accept_capture(&mut self, capture: native::Captured) -> Result<(), Failure> {
        self.session = Some(match capture {
            native::Captured::Ordinary(target) => {
                self.method = Some("accessibility");
                Session::Ordinary(OwnedSpan::capture(target)?)
            }
            native::Captured::Keyboard(target, pacing) => {
                self.method = Some("keyboard_final");
                Session::Keyboard(ChunkedInput::capture_with_pacing(target, pacing)?)
            }
            native::Captured::Terminal(target) => {
                self.method = Some("terminal");
                Session::Terminal(Terminal::capture(target)?)
            }
            native::Captured::ClipboardPaste(target) => {
                self.method = Some("clipboard_final");
                Session::MenuPaste(MenuPaste::capture(target)?)
            }
            native::Captured::Preparing(preparation) => {
                self.preparing = Some(preparation);
                return Ok(());
            }
        });
        Ok(())
    }

    fn queue_prepared_text(&mut self, text: &str, final_text: bool) -> String {
        if self
            .prepared_text
            .as_ref()
            .is_some_and(|(_, final_)| *final_)
        {
            return PREPARING.to_owned();
        }
        if text.encode_utf16().count() > model::MAX_TEXT {
            self.discard_preparation();
            self.failure = Some(Failure::Content);
            return self.retained();
        }
        // A late preview cannot replace an already completed transcription.
        if !self
            .prepared_text
            .as_ref()
            .is_some_and(|(_, final_text)| *final_text)
        {
            self.prepared_text = Some((text.to_owned(), final_text));
        }
        PREPARING.to_owned()
    }

    /// Returns true once the bounded native preparation has completed or failed.
    fn advance_preparation(&mut self) -> Result<bool, Failure> {
        let Some(mut preparation) = self.preparing.take() else {
            return Ok(true);
        };
        let result = match preparation.poll() {
            Ok(Some(capture)) => self.accept_capture(capture).map(|()| true),
            Ok(None) => {
                self.preparing = Some(preparation);
                Ok(false)
            }
            Err(failure) => Err(failure),
        };
        if let Err(failure) = result {
            self.discard_preparation();
            self.failure = Some(failure);
        }
        result
    }

    fn discard_preparation(&mut self) {
        self.preparing = None;
        self.prepared_text = None;
    }

    pub fn blocks_recording(&self) -> bool {
        self.has_pending() || self.failure.is_some_and(Failure::blocks_recording)
    }

    /// A content-free failure stage is useful for permission/status UI.
    pub fn failure_stage(&self) -> Option<&'static str> {
        self.failure.map(Failure::stage)
    }
    pub(crate) fn original_focus_changed(&self) -> bool {
        matches!(
            self.failure,
            Some(Failure::FocusApplicationChanged | Failure::FocusElementChanged)
        )
    }

    /// Content-free evidence retained after completion or failure. Submitted
    /// clipboard/terminal events are never presented as confirmed field text.
    pub fn receipt(&self) -> model::DeliveryReceipt {
        let mut receipt = self
            .session
            .as_ref()
            .map(Session::receipt)
            .unwrap_or(self.receipt);
        if self.preparing.is_some() {
            receipt.state = model::DeliveryState::Waiting;
        }
        if self.failure.is_some() {
            receipt.stopped = true;
        }
        receipt
    }

    #[doc(hidden)]
    pub fn method(&self) -> Option<&'static str> {
        self.method
    }

    #[doc(hidden)]
    pub fn uses_clipboard(&self) -> bool {
        self.method == Some("clipboard_final")
    }

    fn result_message(&mut self, result: Result<(), Failure>, success: &str) -> String {
        match result {
            Ok(()) => success.to_owned(),
            Err(failure) => {
                self.failure = Some(failure);
                failure.message()
            }
        }
    }

    fn retained(&self) -> String {
        self.failure
            .map(Failure::message)
            .unwrap_or_else(|| RETAINED.to_owned())
    }

    fn final_status(&self) -> String {
        if self.failure.is_some() {
            self.retained()
        } else if self.has_pending() {
            "正在完成已接收的最终文字，请保持原输入位置。".into()
        } else {
            match self.receipt().state {
                model::DeliveryState::Confirmed => {
                    "已确认原输入框中的最终文字，请检查后自行发送。".into()
                }
                model::DeliveryState::Submitted => {
                    "已提交原输入位置的输入请求，请检查接收结果。".into()
                }
                _ => "本轮自动输入已结束，请检查输入框；完整结果保留在面板。".into(),
            }
        }
    }

    pub fn preview(&mut self, text: &str) -> String {
        if let Err(failure) = self.on_main() {
            return failure.message();
        }
        self.preview_inner(text)
    }

    fn preview_inner(&mut self, text: &str) -> String {
        if self.final_accepted {
            return self.final_status();
        }
        if self.failure.is_some() {
            return self.retained();
        }
        if self.preparing.is_some() {
            return self.queue_prepared_text(text, false);
        }
        let (result, success) = match self.session.as_mut() {
            Some(Session::Ordinary(span)) => {
                let result = span.update(text, false);
                (
                    result,
                    if span.has_pending() {
                        "正在等待原输入框确认文字更新。"
                    } else {
                        "正在原输入框实时听写；停录后校正。"
                    },
                )
            }
            Some(Session::Terminal(terminal)) => (
                terminal.update(text, false),
                "正在向原 Orca 终端追加稳定文字；不会删除内容或自动发送。",
            ),
            Some(Session::Keyboard(input)) => {
                (input.verify(), "实时预览保留在面板；停录后输入最终文字。")
            }
            Some(Session::MenuPaste(paste)) => (
                paste.verify(),
                "原输入位置兼容听写：停录后复制最终文字并请求粘贴。",
            ),
            None => return self.retained(),
        };
        self.result_message(result, success)
    }

    /// Flush the last local terminal result at Stop, before cloud correction.
    /// Ordinary previews already contain the latest result and need no write.
    pub fn finish_preview(&mut self) -> String {
        if let Err(failure) = self.on_main() {
            return failure.message();
        }
        if self.final_accepted {
            return self.final_status();
        }
        if self.failure.is_some() {
            return self.retained();
        }
        if self.preparing.is_some() {
            return PREPARING.to_owned();
        }
        let Some(session) = self.session.as_mut() else {
            return self.retained();
        };
        match session {
            Session::Ordinary(_) => "已保留原输入框中的实时文字。".to_owned(),
            Session::Keyboard(_) => "正在等待最终转写，完成后输入原光标。".into(),
            Session::MenuPaste(paste) => {
                let result = paste.verify();
                self.result_message(result, "正在等待最终文字，完成后复制并请求原输入位置粘贴。")
            }
            Session::Terminal(terminal) => {
                let result = terminal.flush();
                self.result_message(
                    result,
                    "已安排向原 Orca 终端追加本地识别文字，请检查后自行发送。",
                )
            }
        }
    }

    pub fn deliver(&mut self, text: &str, automatic: bool) -> String {
        if let Err(failure) = self.on_main() {
            return failure.message();
        }
        self.deliver_inner(text, automatic)
    }

    fn deliver_inner(&mut self, text: &str, automatic: bool) -> String {
        if !automatic {
            self.clear();
            return "自动输入已关闭，文字保留在面板。".to_owned();
        }
        if self.final_accepted {
            return self.final_status();
        }
        if self.failure.is_some() {
            self.retain_preview_inner();
            return self.retained();
        }
        if self
            .prepared_text
            .as_ref()
            .is_some_and(|(_, final_text)| *final_text)
        {
            return self.final_status();
        }
        if text.trim().is_empty() {
            // An empty final must never replace the original selection. Undo
            // only a confirmed preview when its original span is still owned.
            self.clear_inner();
            return "没有识别到可输入的文字。".into();
        }
        if self.preparing.is_some() {
            return self.queue_prepared_text(text, true);
        }
        self.final_accepted = true;
        let Some(mut session) = self.session.take() else {
            return self.retained();
        };
        match &mut session {
            Session::Ordinary(span) => {
                let result = span.update(text, true);
                self.receipt = span.receipt();
                match result {
                    Ok(()) if span.has_pending() => {
                        self.session = Some(session);
                        "正在确认原输入框中的最终文字，请等待完成。".into()
                    }
                    _ => self.result_message(
                        result,
                        "已在原输入框完成听写和最终校正，请检查后自行发送。",
                    ),
                }
            }
            Session::Keyboard(input) => {
                let result = input.submit(text);
                self.receipt = input.receipt();
                if result.is_ok() && input.has_pending() {
                    self.session = Some(session);
                    "正在向原输入框键入最终文字，请等待完成。".into()
                } else {
                    self.result_message(result, "已在原输入框输入最终文字，请检查后自行发送。")
                }
            }
            Session::MenuPaste(paste) => {
                let result = paste.submit(text);
                self.receipt = paste.receipt();
                if result.is_ok() && paste.has_pending() {
                    self.session = Some(session);
                    "最终文字已复制，正在请求原输入位置粘贴。".into()
                } else {
                    self.result_message(result, "没有可粘贴的文字。")
                }
            }
            Session::Terminal(terminal) => {
                let result = terminal.update(text, true);
                self.receipt = terminal.receipt();
                match result {
                    Ok(()) => {
                        if terminal.has_pending() {
                            self.session = Some(session);
                            "正在分段向原 Orca 终端输入文字，请等待完成后检查并自行发送。"
                                .to_owned()
                        } else {
                            "已向原 Orca 终端输入文字，请检查后自行发送。".to_owned()
                        }
                    }
                    Err(failure) => {
                        self.failure = Some(failure);
                        format!(
                            "{} 终端保留已输入文字；无法安全改写的最终校正结果留在面板。",
                            failure.message()
                        )
                    }
                }
            }
        }
    }

    /// Whether the original target is still preparing its accessibility tree.
    pub fn preparing_input(&self) -> bool {
        self.preparing.is_some()
    }

    pub fn has_pending(&self) -> bool {
        (self.preparing.is_some()
            && self
                .prepared_text
                .as_ref()
                .is_some_and(|(_, final_text)| *final_text))
            || match self.session.as_ref() {
                Some(Session::Ordinary(span)) => span.has_pending(),
                Some(Session::Keyboard(input)) => input.has_pending(),
                Some(Session::Terminal(terminal)) => terminal.has_pending(),
                Some(Session::MenuPaste(paste)) => paste.has_pending(),
                None => false,
            }
    }

    /// Call once from each main run-loop timer turn. One bounded chunk gives
    /// AppKit a turn to deliver real user-input monitor callbacks before more
    /// Unicode events are posted. The returned status replaces delivery UI text.
    pub fn poll(&mut self) -> Option<String> {
        self.poll_observing_preparation(|| {})
    }

    /// Observe the preparation outcome before consuming any queued text.
    pub(crate) fn poll_observing_preparation(&mut self, prepared: impl FnOnce()) -> Option<String> {
        if self.preparing.is_some() {
            if let Err(failure) = self.on_main() {
                return Some(failure.message());
            }
            let outcome = self.advance_preparation();
            if matches!(outcome, Ok(false)) {
                return None;
            }
            prepared();
            match outcome {
                Ok(_) => {
                    if let Some((text, final_text)) = self.prepared_text.take() {
                        return Some(if final_text {
                            self.deliver(&text, true)
                        } else {
                            self.preview(&text)
                        });
                    }
                    return Some("原输入框已准备好。".to_owned());
                }
                Err(failure) => {
                    self.prepared_text = None;
                    self.failure = Some(failure);
                    return Some(failure.message());
                }
            }
        }
        match self.session.as_ref() {
            None => return None,
            Some(Session::Ordinary(span)) if !span.has_pending() => return None,
            Some(Session::Keyboard(input)) if !input.has_pending() => return None,
            Some(Session::MenuPaste(paste)) if !paste.has_pending() => return None,
            _ => {}
        }
        if let Err(failure) = self.on_main() {
            return Some(failure.message());
        }
        self.poll_captured()
    }

    fn poll_captured(&mut self) -> Option<String> {
        if let Some(Session::Ordinary(span)) = self.session.as_mut() {
            let pending = span.has_pending();
            if !pending {
                return None;
            }
            let restoring = span.is_restoring();
            let result = span.poll();
            self.receipt = span.receipt();
            return match result {
                Ok(true) => {
                    self.session = None;
                    Some(if restoring {
                        "已还原原输入框中已确认的听写预览。".into()
                    } else {
                        "已在原输入框完成听写和最终校正，请检查后自行发送。".into()
                    })
                }
                Ok(false) if span.has_pending() => Some("正在等待原输入框确认文字更新。".into()),
                Ok(false) => Some("已在原输入框更新实时文字；停录后校正。".into()),
                Err(failure) => {
                    self.failure = Some(failure);
                    self.session = None;
                    Some(failure.message())
                }
            };
        }
        if let Some(Session::Keyboard(input)) = self.session.as_mut() {
            let result = input.poll();
            self.receipt = input.receipt();
            return match result {
                Ok(true) => {
                    self.session = None;
                    Some("已在原输入框输入最终文字，请检查后自行发送。".into())
                }
                Ok(false) => Some("正在准备或确认最终文字，请松开快捷键并保持原输入位置。".into()),
                Err(failure) => {
                    self.failure = Some(failure);
                    self.session = None;
                    Some(failure.message())
                }
            };
        }
        if let Some(Session::MenuPaste(paste)) = self.session.as_mut() {
            let result = paste.poll();
            self.receipt = paste.receipt();
            return match result {
                Ok(true) => {
                    let requested = paste.requested();
                    self.session = None;
                    Some(if requested {
                        PASTE_REQUESTED.into()
                    } else {
                        "没有请求原输入位置粘贴。".into()
                    })
                }
                Ok(false) => {
                    Some("最终文字已复制，正在准备粘贴；请松开快捷键并保持原输入位置。".into())
                }
                Err(failure) => {
                    self.failure = Some(failure);
                    self.session = None;
                    Some(failure.message())
                }
            };
        }
        let Some(Session::Terminal(terminal)) = self.session.as_mut() else {
            return None;
        };
        let pending = terminal.has_pending();
        let result = terminal.poll();
        self.receipt = terminal.receipt();
        match result {
            Ok(true) => {
                self.session = None;
                Some("已向原 Orca 终端提交输入事件，请检查文字后自行发送。".to_owned())
            }
            Ok(false) if pending => {
                Some("正在向原 Orca 终端分段提交输入事件；用户输入会停止后续追加。".to_owned())
            }
            Ok(false) => None,
            Err(failure) => {
                self.failure = Some(failure);
                self.session = None;
                Some(format!(
                    "{} 终端保留已输入文字；未输入的结果留在面板。",
                    failure.message()
                ))
            }
        }
    }

    /// Keep useful local text on cloud failure. Already accepted terminal
    /// chunks finish through poll; late recognition updates are disconnected.
    pub fn retain_preview(&mut self) {
        if self.on_main().is_err() {
            return;
        }
        self.retain_preview_inner();
    }

    fn retain_preview_inner(&mut self) {
        self.final_accepted = true;
        self.discard_preparation();
        if self.failure.is_none() {
            match self.session.as_mut() {
                Some(Session::Ordinary(span)) => span.seal(),
                Some(Session::Terminal(terminal)) => terminal.seal(),
                Some(Session::Keyboard(_)) => {}
                Some(Session::MenuPaste(paste)) => paste.end(),
                None => {}
            }
            if self.has_pending() {
                return;
            }
        }
        self.receipt = self.receipt();
        self.session = None;
    }

    /// Cancellation restores only an unchanged ordinary owned span. Terminal
    /// input is append-only and is never retracted.
    pub fn clear(&mut self) {
        if self.on_main().is_err() {
            return;
        }
        self.clear_inner();
    }

    fn clear_inner(&mut self) {
        self.receipt = self.receipt();
        self.receipt.stop();
        self.final_accepted = true;
        self.discard_preparation();
        if let Some(mut session) = self.session.take() {
            match &mut session {
                Session::Ordinary(span) => {
                    let result = span.cancel();
                    self.receipt = span.receipt();
                    if let Err(failure) = result {
                        self.failure = Some(failure);
                    } else if span.has_pending() {
                        self.session = Some(session);
                    }
                }
                Session::Terminal(terminal) => {
                    terminal.end();
                    self.receipt = terminal.receipt();
                }
                Session::Keyboard(input) => {
                    input.end();
                    self.receipt = input.receipt();
                }
                Session::MenuPaste(paste) => {
                    paste.end();
                    self.receipt = paste.receipt();
                }
            }
        }
    }

    /// Opt-in, content-free probe of the current input target. Never writes.
    pub fn diagnose_input() -> serde_json::Value {
        native::diagnose_input()
    }

    /// Metadata-only diagnosis for an explicitly selected running application.
    /// Does not activate it, read field text, or create a delivery session.
    pub fn diagnose_application(pid: i32) -> serde_json::Value {
        native::diagnose_application(pid)
    }

    /// Exercises the production delivery path only against our isolated fixture.
    pub fn input_self_test() -> serde_json::Value {
        fn settle(delivery: &mut Delivery) -> Option<String> {
            use objc2_foundation::{NSDate, NSRunLoop};
            let started = std::time::Instant::now();
            let mut status = None;
            while delivery.has_pending() && started.elapsed() < std::time::Duration::from_secs(2) {
                // This explicit fixture-only diagnostic owns its Delivery;
                // no application Controller is borrowed by this run loop.
                NSRunLoop::currentRunLoop()
                    .runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
                if let Some(message) = delivery.poll() {
                    status = Some(message);
                }
            }
            status
        }
        let mut report = Self::diagnose_input();
        if !native::self_test_fixture_focused() {
            report["selfTest"] = "refused: dedicated untouched input fixture is not focused".into();
            return report;
        }
        let mut delivery = Self::new();
        match delivery.begin() {
            Ok(()) => {
                report["begin"] = "ok".into();
                report["preview"] = delivery.preview("测试").into();
                if let Some(status) = settle(&mut delivery) {
                    report["preview"] = status.into();
                }
                report["final"] = delivery.deliver("输入验证🙂", true).into();
                if let Some(status) = settle(&mut delivery) {
                    report["final"] = status.into();
                }
                report["pending"] = delivery.has_pending().into();
            }
            Err(error) => report["begin"] = error.into(),
        }
        report
    }

    pub fn copy(text: &str) -> bool {
        native::copy(text)
    }
    pub fn trusted() -> bool {
        native::trusted()
    }
    pub fn secure_input() -> bool {
        native::secure_input()
    }
    pub(crate) fn history_focus_safe() -> bool {
        native::history_focus_safe()
    }
    /// Use only in response to an explicit user permission-button click.
    pub fn request_accessibility() {
        native::request_accessibility();
    }
}

#[cfg(test)]
mod preparation_tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    struct Field;
    impl Target for Field {
        fn read(&mut self) -> Result<model::Snapshot, Failure> {
            Ok(model::Snapshot {
                text: "left|right".into(),
                selection: model::TextRange {
                    location: 4,
                    length: 1,
                },
            })
        }
        fn replace(
            &mut self,
            _: model::TextRange,
            _: &str,
            _: &model::Snapshot,
        ) -> Result<(), Failure> {
            panic!("preparation must not write text")
        }
    }
    struct Delayed {
        polls: Rc<Cell<usize>>,
        failure: Option<Failure>,
        keyboard: bool,
    }
    impl native::Preparation for Delayed {
        fn poll(&mut self) -> Result<Option<native::Captured>, Failure> {
            self.polls.set(self.polls.get() + 1);
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            Ok((self.polls.get() > 1).then(|| {
                if self.keyboard {
                    native::Captured::Keyboard(Box::new(Field), model::KeyboardPacing::Chunked)
                } else {
                    native::Captured::Ordinary(Box::new(Field))
                }
            }))
        }
    }
    fn pending(failure: Option<Failure>) -> (Delivery, Rc<Cell<usize>>) {
        pending_for_method(failure, false)
    }
    fn pending_for_method(failure: Option<Failure>, keyboard: bool) -> (Delivery, Rc<Cell<usize>>) {
        let polls = Rc::new(Cell::new(0));
        let mut delivery = Delivery::new();
        delivery
            .accept_capture(native::Captured::Preparing(Box::new(Delayed {
                polls: polls.clone(),
                failure,
                keyboard,
            })))
            .unwrap();
        (delivery, polls)
    }

    #[test]
    fn final_result_waits_for_capture_and_survives_late_preview() {
        for keyboard in [false, true] {
            let (mut delivery, _) = pending_for_method(None, keyboard);
            delivery.queue_prepared_text("preview", false);
            assert!(!delivery.has_pending());
            delivery.queue_prepared_text("final🙂", true);
            delivery.queue_prepared_text("late preview", false);
            delivery.queue_prepared_text(&"迟".repeat(model::MAX_TEXT + 1), false);
            delivery.queue_prepared_text("duplicate final", true);
            assert_eq!(delivery.failure_stage(), None);
            assert!(delivery.has_pending());
            assert!(!delivery.advance_preparation().unwrap());
            assert!(delivery.session.is_none());
            assert!(delivery.advance_preparation().unwrap());
            assert!(delivery.session.is_some());
            assert_eq!(
                delivery.method(),
                Some(if keyboard {
                    "keyboard_final"
                } else {
                    "accessibility"
                })
            );
            assert_eq!(
                delivery.prepared_text.as_ref(),
                Some(&("final🙂".into(), true))
            );
            assert_eq!(delivery.failure_stage(), None);
        }
    }

    #[test]
    fn keyboard_pacing_survives_direct_and_delayed_capture() {
        use std::cell::RefCell;

        struct RecordingField(Rc<RefCell<Vec<String>>>);
        impl Target for RecordingField {
            fn read(&mut self) -> Result<model::Snapshot, Failure> {
                Ok(model::Snapshot {
                    text: String::new(),
                    selection: model::TextRange {
                        location: 0,
                        length: 0,
                    },
                })
            }
            fn replace(
                &mut self,
                _: model::TextRange,
                text: &str,
                _: &model::Snapshot,
            ) -> Result<(), Failure> {
                self.0.borrow_mut().push(text.to_owned());
                Ok(())
            }
        }
        struct Ready(Option<native::Captured>);
        impl native::Preparation for Ready {
            fn poll(&mut self) -> Result<Option<native::Captured>, Failure> {
                Ok(self.0.take())
            }
        }

        for (pacing, first) in [
            (model::KeyboardPacing::Chunked, "中文🙂"),
            (model::KeyboardPacing::Scalar, "中"),
        ] {
            for delayed in [false, true] {
                let writes = Rc::new(RefCell::new(Vec::new()));
                let captured =
                    native::Captured::Keyboard(Box::new(RecordingField(writes.clone())), pacing);
                let mut delivery = Delivery::new();
                if delayed {
                    delivery
                        .accept_capture(native::Captured::Preparing(Box::new(Ready(Some(
                            captured,
                        )))))
                        .unwrap();
                    delivery.queue_prepared_text("中文🙂", true);
                    assert!(delivery.advance_preparation().unwrap());
                } else {
                    delivery.accept_capture(captured).unwrap();
                }
                assert!(writes.borrow().is_empty());
                let Some(Session::Keyboard(input)) = delivery.session.as_mut() else {
                    panic!("keyboard capture must retain its delivery path");
                };
                input.submit("中文🙂").unwrap();
                assert_eq!(&*writes.borrow(), &[first.to_owned()]);
            }
        }
    }

    #[test]
    fn clipboard_capture_is_final_only_and_survives_delayed_preparation() {
        use std::cell::RefCell;
        #[derive(Default)]
        struct State {
            copied: Vec<String>,
            pastes: usize,
            ready: bool,
        }
        struct Paste(Rc<RefCell<State>>);
        impl PasteTarget for Paste {
            fn check(&mut self) -> Result<(), Failure> {
                Ok(())
            }
            fn prepare(&mut self, text: &str) -> Result<(), Failure> {
                self.0.borrow_mut().copied.push(text.into());
                Ok(())
            }
            fn ready(&mut self) -> Result<bool, Failure> {
                Ok(self.0.borrow().ready)
            }
            fn paste(&mut self) -> Result<(), Failure> {
                self.0.borrow_mut().pastes += 1;
                Ok(())
            }
            fn abandon(&mut self) {}
        }
        struct Ready(Option<native::Captured>);
        impl native::Preparation for Ready {
            fn poll(&mut self) -> Result<Option<native::Captured>, Failure> {
                Ok(self.0.take())
            }
        }
        for delayed in [false, true] {
            let state = Rc::new(RefCell::new(State::default()));
            let capture = native::Captured::ClipboardPaste(Box::new(Paste(state.clone())));
            let mut delivery = Delivery::new();
            if delayed {
                delivery
                    .accept_capture(native::Captured::Preparing(Box::new(Ready(Some(capture)))))
                    .unwrap();
                delivery.queue_prepared_text("最终🙂\n文字", true);
                delivery.queue_prepared_text("late preview", false);
                assert!(delivery.advance_preparation().unwrap());
                assert_eq!(delivery.prepared_text.as_ref().unwrap().0, "最终🙂\n文字");
            } else {
                delivery.accept_capture(capture).unwrap();
            }
            assert_eq!(delivery.method(), Some("clipboard_final"));
            let Some(Session::MenuPaste(paste)) = delivery.session.as_mut() else {
                panic!("clipboard route must use final-only paste policy");
            };
            paste.verify().unwrap();
            assert!(state.borrow().copied.is_empty());
            paste.submit("最终🙂\n文字").unwrap();
            assert!(!paste.poll().unwrap());
            state.borrow_mut().ready = true;
            assert!(paste.poll().unwrap());
            assert!(paste.poll().unwrap());
            assert_eq!(paste.submit("duplicate"), Err(Failure::Ended));
            assert_eq!(state.borrow().copied, ["最终🙂 文字"]);
            assert_eq!(state.borrow().pastes, 1);
            assert!(!delivery.has_pending());
        }
    }

    #[test]
    fn failed_focus_preparation_discards_text_and_never_rebinds() {
        for failure in [
            Failure::UserInput,
            Failure::FocusApplicationChanged,
            Failure::FocusElementChanged,
            Failure::SecureInput,
            Failure::FocusRead(-25212),
        ] {
            let (mut delivery, polls) = pending(Some(failure));
            delivery.queue_prepared_text("final", true);
            assert_eq!(delivery.advance_preparation(), Err(failure));
            assert_eq!(delivery.failure_stage(), Some(failure.stage()));
            assert!(!delivery.has_pending());
            assert!(delivery.session.is_none() && delivery.prepared_text.is_none());
            assert!(delivery.advance_preparation().unwrap());
            assert_eq!(polls.get(), 1);
        }
    }

    #[test]
    fn cancellation_discards_preparation_without_polling_or_writing() {
        let (mut delivery, polls) = pending(None);
        delivery.queue_prepared_text("final", true);
        delivery.discard_preparation();
        assert!(delivery.advance_preparation().unwrap());
        assert!(!delivery.has_pending());
        assert!(delivery.session.is_none() && delivery.prepared_text.is_none());
        assert_eq!(polls.get(), 0);
    }

    #[test]
    fn pending_text_is_bounded_by_utf16_units() {
        let (mut delivery, _) = pending(None);
        delivery.queue_prepared_text(&"🙂".repeat(model::MAX_TEXT / 2 + 1), true);
        assert_eq!(delivery.failure_stage(), Some("content"));
        assert!(!delivery.has_pending());
        assert!(delivery.prepared_text.is_none());
    }
}
