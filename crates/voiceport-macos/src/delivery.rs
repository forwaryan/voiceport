//! Focused macOS text delivery. Construction is inert; `begin` captures the
//! target only after the user starts dictation. Call every operation on the
//! application main thread. No helper process, clipboard paste, or Return key is
//! used for automatic delivery.
#[doc(hidden)]
pub mod model;
mod native;

use model::{Failure, OwnedSpan, Target, Terminal, TerminalTarget};

const RETAINED: &str = "无法确认原输入框支持安全写入，文字保留在面板。";
const PREPARING: &str = "正在准备原输入框，请保持当前输入位置。";

enum Session {
    Ordinary(OwnedSpan<Box<dyn Target>>),
    Terminal(Terminal<Box<dyn TerminalTarget>>),
}

/// Holds text only for the currently captured field and never logs its content.
/// The native target and monitor make this type intentionally !Send / !Sync.
pub struct Delivery {
    session: Option<Session>,
    failure: Option<Failure>,
    preparing: Option<Box<dyn native::Preparation>>,
    prepared_text: Option<(String, bool)>,
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
        }
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
        self.on_main().map_err(Failure::message)?;
        self.clear();
        self.failure = None;
        let result = native::capture().and_then(|capture| self.accept_capture(capture));
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
            native::Captured::Ordinary(target) => Session::Ordinary(OwnedSpan::capture(target)?),
            native::Captured::Terminal(target) => Session::Terminal(Terminal::capture(target)?),
            native::Captured::Preparing(preparation) => {
                self.preparing = Some(preparation);
                return Ok(());
            }
        });
        Ok(())
    }

    fn queue_prepared_text(&mut self, text: &str, final_text: bool) -> String {
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
        self.failure.is_some_and(Failure::blocks_recording)
    }

    /// A content-free failure stage is useful for permission/status UI.
    pub fn failure_stage(&self) -> Option<&'static str> {
        self.failure.map(Failure::stage)
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

    pub fn preview(&mut self, text: &str) -> String {
        if let Err(failure) = self.on_main() {
            return failure.message();
        }
        if self.failure.is_some() {
            return self.retained();
        }
        if self.preparing.is_some() {
            return self.queue_prepared_text(text, false);
        }
        let (result, success) = match self.session.as_mut() {
            Some(Session::Ordinary(span)) => (
                span.update(text, false),
                "正在原输入框实时听写；停录后校正。",
            ),
            Some(Session::Terminal(terminal)) => (
                terminal.update(text, false),
                "正在向原 Orca 终端追加稳定文字；不会删除内容或自动发送。",
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
        if !automatic {
            self.clear();
            return "自动输入已关闭，文字保留在面板。".to_owned();
        }
        if self.failure.is_some() {
            self.retain_preview();
            return self.retained();
        }
        if self.preparing.is_some() {
            return self.queue_prepared_text(text, true);
        }
        let Some(mut session) = self.session.take() else {
            return self.retained();
        };
        match &mut session {
            Session::Ordinary(span) => {
                let result = span.update(text, true);
                self.result_message(result, "已在原输入框完成听写和最终校正，请检查后自行发送。")
            }
            Session::Terminal(terminal) => {
                let result = terminal.update(text, true);
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
            || matches!(self.session.as_ref(), Some(Session::Terminal(terminal)) if terminal.has_pending())
    }

    /// Call once from each main run-loop timer turn. One bounded chunk gives
    /// AppKit a turn to deliver real user-input monitor callbacks before more
    /// Unicode events are posted. The returned status replaces delivery UI text.
    pub fn poll(&mut self) -> Option<String> {
        if self.preparing.is_some() {
            if let Err(failure) = self.on_main() {
                return Some(failure.message());
            }
            match self.advance_preparation() {
                Ok(false) => return None,
                Ok(true) => {
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
        if !matches!(self.session.as_ref(), Some(Session::Terminal(_))) {
            return None;
        }
        if let Err(failure) = self.on_main() {
            return Some(failure.message());
        }
        let Some(Session::Terminal(terminal)) = self.session.as_mut() else {
            return None;
        };
        let pending = terminal.has_pending();
        let result = terminal.poll();
        match result {
            Ok(true) => {
                self.session = None;
                Some("已向原 Orca 终端输入已确认文字，请检查后自行发送。".to_owned())
            }
            Ok(false) if pending => {
                Some("正在向原 Orca 终端分段输入已确认文字；用户输入会停止后续追加。".to_owned())
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
        self.discard_preparation();
        if self.failure.is_none()
            && let Some(Session::Terminal(terminal)) = self.session.as_mut()
        {
            terminal.seal();
            if terminal.has_pending() {
                return;
            }
        }
        self.session = None;
    }

    /// Cancellation restores only an unchanged ordinary owned span. Terminal
    /// input is append-only and is never retracted.
    pub fn clear(&mut self) {
        if self.on_main().is_err() {
            return;
        }
        self.discard_preparation();
        if let Some(mut session) = self.session.take() {
            match &mut session {
                Session::Ordinary(span) => {
                    if let Err(failure) = span.cancel() {
                        self.failure = Some(failure);
                    }
                }
                Session::Terminal(terminal) => terminal.end(),
            }
        }
    }

    /// Opt-in, content-free probe of the current input target. Never writes.
    pub fn diagnose_input() -> serde_json::Value {
        native::diagnose_input()
    }

    /// Exercises the production delivery path only against our isolated fixture.
    pub fn input_self_test() -> serde_json::Value {
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
                report["final"] = delivery.deliver("输入验证🙂", true).into();
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
    }
    impl native::Preparation for Delayed {
        fn poll(&mut self) -> Result<Option<native::Captured>, Failure> {
            self.polls.set(self.polls.get() + 1);
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            Ok((self.polls.get() > 1).then(|| native::Captured::Ordinary(Box::new(Field))))
        }
    }
    fn pending(failure: Option<Failure>) -> (Delivery, Rc<Cell<usize>>) {
        let polls = Rc::new(Cell::new(0));
        let mut delivery = Delivery::new();
        delivery
            .accept_capture(native::Captured::Preparing(Box::new(Delayed {
                polls: polls.clone(),
                failure,
            })))
            .unwrap();
        (delivery, polls)
    }

    #[test]
    fn final_result_waits_for_capture_and_survives_late_preview() {
        let (mut delivery, _) = pending(None);
        delivery.queue_prepared_text("preview", false);
        assert!(!delivery.has_pending());
        delivery.queue_prepared_text("final🙂", true);
        delivery.queue_prepared_text("late preview", false);
        assert!(delivery.has_pending());
        assert!(!delivery.advance_preparation().unwrap());
        assert!(delivery.session.is_none());
        assert!(delivery.advance_preparation().unwrap());
        assert!(delivery.session.is_some());
        assert_eq!(
            delivery.prepared_text.as_ref(),
            Some(&("final🙂".into(), true))
        );
        assert_eq!(delivery.failure_stage(), None);
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
