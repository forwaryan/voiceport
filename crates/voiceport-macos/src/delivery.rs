//! Focused macOS text delivery. Construction is inert; `begin` captures the
//! target only after the user starts dictation. Call every operation on the
//! application main thread. No helper process, clipboard paste, or Return key is
//! used for automatic delivery.
#[doc(hidden)]
pub mod model;
mod native;

use model::{Failure, OwnedSpan, Target, Terminal, TerminalTarget};

const RETAINED: &str = "无法确认原输入框支持安全写入，文字保留在面板。";

enum Session {
    Ordinary(OwnedSpan<Box<dyn Target>>),
    Terminal(Terminal<Box<dyn TerminalTarget>>),
}

/// Holds text only for the currently captured field and never logs its content.
/// The native target and monitor make this type intentionally !Send / !Sync.
pub struct Delivery {
    session: Option<Session>,
    failure: Option<Failure>,
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
        let result = native::capture().and_then(|capture| match capture {
            native::Captured::Ordinary(target) => OwnedSpan::capture(target).map(Session::Ordinary),
            native::Captured::Terminal(target) => Terminal::capture(target).map(Session::Terminal),
        });
        match result {
            Ok(session) => {
                self.session = Some(session);
                Ok(())
            }
            Err(failure) => {
                self.failure = Some(failure);
                Err(failure.message())
            }
        }
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

    pub fn has_pending(&self) -> bool {
        matches!(self.session.as_ref(), Some(Session::Terminal(terminal)) if terminal.has_pending())
    }

    /// Call once from each main run-loop timer turn. One bounded chunk gives
    /// AppKit a turn to deliver real user-input monitor callbacks before more
    /// Unicode events are posted. The returned status replaces delivery UI text.
    pub fn poll(&mut self) -> Option<String> {
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
