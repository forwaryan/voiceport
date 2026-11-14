//! Pure delivery policy. Boundaries in tests never inspect or modify real applications.
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub const MAX_TEXT: usize = 100_000;
pub const MAX_FIELD: usize = 1_000_000;
pub const EVENT_TAG: i64 = 0x56504F5254545950;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    MainThread,
    Accessibility,
    SecureInput,
    Application,
    Focus,
    FocusRead(i32),
    FocusApplicationChanged,
    FocusElementChanged,
    Role,
    Sensitive,
    Disabled,
    Writable,
    Content,
    Selection,
    Ownership,
    ReadbackTimeout,
    Write,
    Monitor,
    UserInput,
    Revision,
    Event,
    Ended,
}

impl Failure {
    pub fn stage(self) -> &'static str {
        match self {
            Self::MainThread => "mainThread",
            Self::Accessibility => "accessibility",
            Self::SecureInput => "secureInput",
            Self::Application => "application",
            Self::Focus
            | Self::FocusRead(_)
            | Self::FocusApplicationChanged
            | Self::FocusElementChanged => "focus",
            Self::Role => "role",
            Self::Sensitive => "sensitive",
            Self::Disabled => "disabled",
            Self::Writable => "writable",
            Self::Content => "content",
            Self::Selection => "selection",
            Self::Ownership => "ownership",
            Self::ReadbackTimeout => "readback",
            Self::Write => "write",
            Self::Monitor => "monitor",
            Self::UserInput => "userInput",
            Self::Revision => "revision",
            Self::Event => "event",
            Self::Ended => "ended",
        }
    }

    pub fn blocks_recording(self) -> bool {
        matches!(self, Self::Accessibility | Self::Application)
    }

    pub fn message(self) -> String {
        if let Self::FocusRead(status) = self {
            let reason = match status {
                -25212 => {
                    "系统未返回目标输入框；光标已在输入框内时，可能是目标应用的输入接口未就绪"
                }
                -25208 => "目标应用未实现系统输入接口，当前控件无法确认自动写入能力",
                -25204 => "目标应用的辅助功能接口暂未响应，请稍后重新开始",
                -25211 => "目标应用的辅助功能接口不可用，请检查授权",
                _ => "读取目标输入框失败，请重新定位输入光标后开始",
            };
            return format!("[focus] {reason}（AX {status}）；结果保留在面板。");
        }
        let detail = match self {
            Self::Accessibility => {
                "当前版本的辅助功能授权未生效。请在系统设置 → 隐私与安全性 → 辅助功能中重新授权当前 VoicePort。"
            }
            Self::Application => "请先回到要输入文字的应用，点好光标，再按全局快捷键开始。",
            Self::SecureInput | Self::Sensitive => "当前处于安全输入或密码字段，已停止自动输入。",
            Self::MainThread => "自动输入必须在主线程执行；结果保留在面板。",
            Self::Focus => "无法确认原应用的输入焦点；结果保留在面板。",
            Self::FocusApplicationChanged => {
                "前台应用已改变，已停止自动输入；请回到目标输入框后重新开始。"
            }
            Self::FocusElementChanged => "焦点输入框已改变，已停止自动输入；请重新开始听写。",
            Self::Role | Self::Disabled | Self::Writable => {
                "原控件未确认支持文字输入；结果保留在面板。"
            }
            Self::Content | Self::Selection => {
                "无法安全读取原输入框的文字或光标位置；结果保留在面板。"
            }
            Self::Monitor => "无法监测用户输入，已停止自动输入；结果保留在面板。",
            Self::ReadbackTimeout => {
                "目标输入框未及时确认本次文字更新，已停止后续输入；请检查输入框和结果面板。"
            }
            _ => "原输入框、光标、文字或终端输入已改变，已停止自动写入；结果保留在面板。",
        };
        format!("[{}] {}", self.stage(), detail)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRange {
    pub location: usize,
    pub length: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub text: String,
    pub selection: TextRange,
}

/// Application metadata bound at an explicit start action, never field contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplicationTarget {
    pub pid: i32,
    pub bundle: String,
}

/// Offsets are UTF-16, but returned offsets are UTF-8 scalar boundaries. Reject
/// overflow and surrogate splits rather than repairing malformed AX snapshots.
pub fn byte_range(text: &str, range: TextRange) -> Option<Range<usize>> {
    let end = range.location.checked_add(range.length)?;
    let mut units = 0;
    let mut start_byte = None;
    for (byte, ch) in text.char_indices() {
        if units == range.location {
            start_byte = Some(byte);
        }
        if units == end {
            return Some(start_byte?..byte);
        }
        units += ch.len_utf16();
        if units > end {
            return None;
        }
    }
    if units == range.location {
        start_byte = Some(text.len());
    }
    (units == end).then_some(start_byte?..text.len())
}

/// Foundation's control set includes Unicode categories Cc AND Cf. Rust's
/// is_control covers only Cc. Keep Cf classification deterministic here so the
/// state machine does not call any native boundary (including in tests).
fn control(c: char) -> bool {
    c.is_control()
        || matches!(c as u32,
        0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f |
        0x0890..=0x0891 | 0x08e2 | 0x180e | 0x200b..=0x200f |
        0x202a..=0x202e | 0x2060..=0x2064 | 0x2066..=0x206f |
        0xfeff | 0xfff9..=0xfffb | 0x110bd | 0x110cd |
        0x13430..=0x1343f | 0x1bca0..=0x1bca3 | 0x1d173..=0x1d17a |
        0xe0001 | 0xe0020..=0xe007f)
}

/// Match Swift Character-based sanitization, including CRLF as one space and
/// an entire grapheme containing a control scalar as one space.
pub fn automatic(text: &str) -> String {
    text.graphemes(true)
        .map(|g| {
            if g.chars()
                .any(|c| control(c) || matches!(c, '\u{2028}' | '\u{2029}'))
            {
                " "
            } else {
                g
            }
        })
        .collect()
}

pub trait Target {
    fn read(&mut self) -> Result<Snapshot, Failure>;
    /// Must recheck the exact snapshot immediately before writing. A failed
    /// write is never retried, because it may have partially succeeded.
    fn replace(&mut self, range: TextRange, text: &str, expected: &Snapshot)
    -> Result<(), Failure>;
    /// Observe the one submitted replacement. False means it is still settling;
    /// implementations must bound that wait and never resubmit an uncertain write.
    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, Failure> {
        if self.read()? == *expected {
            Ok(true)
        } else {
            Err(Failure::Ownership)
        }
    }
    /// Discard unfinished follow-up steps without writing or retrying.
    fn abandon(&mut self) {}
}

impl<T: Target + ?Sized> Target for Box<T> {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        (**self).read()
    }
    fn replace(
        &mut self,
        range: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), Failure> {
        (**self).replace(range, text, expected)
    }
    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, Failure> {
        (**self).confirm(expected)
    }
    fn abandon(&mut self) {
        (**self).abandon();
    }
}

struct PendingSpan {
    next: Snapshot,
    owned: TextRange,
    final_text: bool,
}

pub struct OwnedSpan<T: Target> {
    target: T,
    expected: Snapshot,
    original: String,
    owned: TextRange,
    changed: bool,
    ended: bool,
    failure: Option<Failure>,
    pending: Option<PendingSpan>,
    queued: Option<(String, bool)>,
    cancel_requested: bool,
}

impl<T: Target> OwnedSpan<T> {
    pub fn capture(mut target: T) -> Result<Self, Failure> {
        let expected = target.read()?;
        if expected.text.encode_utf16().count() > MAX_FIELD {
            return Err(Failure::Content);
        }
        let bytes = byte_range(&expected.text, expected.selection).ok_or(Failure::Selection)?;
        let original = expected.text[bytes].to_owned();
        Ok(Self {
            owned: expected.selection,
            target,
            expected,
            original,
            changed: false,
            ended: false,
            failure: None,
            pending: None,
            queued: None,
            cancel_requested: false,
        })
    }

    fn check(&mut self) -> Result<(), Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if self.ended {
            return Err(Failure::Ended);
        }
        if self.target.read()? != self.expected {
            return Err(Failure::Ownership);
        }
        Ok(())
    }

    pub fn update(&mut self, raw: &str, final_text: bool) -> Result<(), Failure> {
        let result = (|| {
            if raw.encode_utf16().count() > MAX_TEXT {
                return Err(Failure::Content);
            }
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            if self.ended || self.cancel_requested {
                return Err(Failure::Ended);
            }
            if self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.final_text)
                || (!final_text && self.queued.as_ref().is_some_and(|(_, final_)| *final_))
            {
                return Ok(());
            }
            if self.pending.is_some() {
                self.poll_inner()?;
                if self.pending.is_some() {
                    if !self.queued.as_ref().is_some_and(|(_, final_)| *final_) || final_text {
                        self.queued = Some((raw.to_owned(), final_text));
                    }
                    return Ok(());
                }
            }
            self.update_inner(raw, final_text)
        })();
        if let Err(failure) = result {
            self.fail(failure);
        }
        result
    }

    fn update_inner(&mut self, raw: &str, final_text: bool) -> Result<(), Failure> {
        self.check()?;
        // Bound the raw input as well, before allocating segmentation output.
        if raw.encode_utf16().count() > MAX_TEXT {
            return Err(Failure::Content);
        }
        let text = automatic(raw);
        let bytes = byte_range(&self.expected.text, self.owned).ok_or(Failure::Selection)?;
        if self.changed && self.expected.text[bytes.clone()] == text {
            self.ended = final_text;
            return Ok(());
        }
        let mut next_text = self.expected.text.clone();
        next_text.replace_range(bytes, &text);
        if next_text.encode_utf16().count() > MAX_FIELD {
            return Err(Failure::Content);
        }
        let owned = TextRange {
            location: self.owned.location,
            length: text.encode_utf16().count(),
        };
        let next = Snapshot {
            text: next_text,
            selection: TextRange {
                location: owned.location + owned.length,
                length: 0,
            },
        };
        // An empty initial preview must not send a synthetic edit to an editor.
        if next == self.expected {
            self.owned = owned;
            self.ended = final_text;
            return Ok(());
        }
        self.target.replace(self.owned, &text, &self.expected)?;
        self.pending = Some(PendingSpan {
            next,
            owned,
            final_text,
        });
        self.poll_inner()?;
        Ok(())
    }

    fn fail(&mut self, failure: Failure) {
        self.failure = Some(failure);
        self.pending = None;
        self.queued = None;
        self.target.abandon();
    }

    fn poll_inner(&mut self) -> Result<bool, Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if let Some(pending) = &self.pending {
            if !self.target.confirm(&pending.next)? {
                return Ok(false);
            }
            let pending = self.pending.take().expect("confirmed pending span");
            self.expected = pending.next;
            self.owned = pending.owned;
            self.changed = true;
            self.ended = pending.final_text;
            if self.ended {
                self.queued = None;
            }
        }
        if !self.ended
            && let Some((text, final_text)) = self.queued.take()
        {
            self.update_inner(&text, final_text)?;
        }
        Ok(self.ended && !self.has_pending())
    }

    pub fn poll(&mut self) -> Result<bool, Failure> {
        let result = self.poll_inner();
        if let Err(failure) = result {
            self.fail(failure);
        }
        result
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some() || self.queued.is_some()
    }

    pub fn is_restoring(&self) -> bool {
        self.cancel_requested
    }

    /// Finish accepted preview work after transcription failure, without taking
    /// another source of text or restoring any unconfirmed editor contents.
    pub fn seal(&mut self) {
        if let Some((_, final_text)) = &mut self.queued {
            *final_text = true;
        } else if let Some(pending) = &mut self.pending {
            pending.final_text = true;
        } else {
            self.ended = true;
        }
    }

    pub fn cancel(&mut self) -> Result<(), Failure> {
        if self.cancel_requested {
            return Ok(());
        }
        self.cancel_requested = true;
        if self.has_pending() {
            // The submitted edit may already have taken effect. Stop follow-up
            // writes and preserve it rather than guessing what to undo.
            self.target.abandon();
            self.pending = None;
            self.queued = None;
            self.ended = true;
            return Ok(());
        }
        if !self.changed || self.ended {
            self.ended = true;
            return Ok(());
        }
        let result = self.check().and_then(|()| {
            let bytes = byte_range(&self.expected.text, self.owned).ok_or(Failure::Selection)?;
            let mut next = self.expected.clone();
            next.text.replace_range(bytes, &self.original);
            let owned = TextRange {
                location: self.owned.location,
                length: self.original.encode_utf16().count(),
            };
            next.selection = TextRange {
                location: owned.location + owned.length,
                length: 0,
            };
            self.target
                .replace(self.owned, &self.original, &self.expected)?;
            self.pending = Some(PendingSpan {
                next,
                owned,
                final_text: true,
            });
            self.poll_inner().map(|_| ())
        });
        if let Err(failure) = result {
            self.ended = true;
            self.fail(failure);
        }
        result
    }
}

/// Final-only, bounded character input. Every chunk must be acknowledged before
/// the next one is submitted; uncertain input is never repeated or undone.
pub struct ChunkedInput<T: Target> {
    target: T,
    expected: Snapshot,
    pending: Option<Snapshot>,
    queue: std::collections::VecDeque<String>,
    submitted: bool,
    ended: bool,
    failure: Option<Failure>,
}

impl<T: Target> ChunkedInput<T> {
    pub fn capture(mut target: T) -> Result<Self, Failure> {
        let expected = target.read()?;
        if expected.text.encode_utf16().count() > MAX_FIELD {
            return Err(Failure::Content);
        }
        byte_range(&expected.text, expected.selection).ok_or(Failure::Selection)?;
        Ok(Self {
            target,
            expected,
            pending: None,
            queue: Default::default(),
            submitted: false,
            ended: false,
            failure: None,
        })
    }

    fn check(&mut self) -> Result<(), Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if self.ended {
            return Err(Failure::Ended);
        }
        if self.target.read()? != self.expected {
            return Err(Failure::Ownership);
        }
        Ok(())
    }

    pub fn verify(&mut self) -> Result<(), Failure> {
        if self.submitted {
            return Err(self.failure.unwrap_or(Failure::Ended));
        }
        let result = self.check();
        if let Err(failure) = result {
            self.fail(failure);
        }
        result
    }

    pub fn submit(&mut self, raw: &str) -> Result<(), Failure> {
        if self.submitted || self.ended {
            return Err(self.failure.unwrap_or(Failure::Ended));
        }
        let result = (|| {
            if raw.encode_utf16().count() > MAX_TEXT {
                return Err(Failure::Content);
            }
            self.verify()?;
            let text = automatic(raw);
            let total = self
                .expected
                .text
                .encode_utf16()
                .count()
                .checked_sub(self.expected.selection.length)
                .and_then(|n| n.checked_add(text.encode_utf16().count()))
                .ok_or(Failure::Content)?;
            if total > MAX_FIELD {
                return Err(Failure::Content);
            }
            self.queue
                .extend(unicode_chunks(&text).into_iter().map(str::to_owned));
            self.submitted = true;
            self.poll_inner().map(|_| ())
        })();
        if let Err(failure) = result {
            self.fail(failure);
        }
        result
    }

    fn fail(&mut self, failure: Failure) {
        self.failure = Some(failure);
        self.end();
    }

    fn poll_inner(&mut self) -> Result<bool, Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if self.ended {
            return Ok(true);
        }
        if !self.submitted {
            return Ok(false);
        }
        if let Some(next) = &self.pending {
            if !self.target.confirm(next)? {
                return Ok(false);
            }
            self.expected = self.pending.take().expect("confirmed chunk");
        }
        let Some(chunk) = self.queue.pop_front() else {
            self.ended = true;
            return Ok(true);
        };
        self.check()?;
        let selection = self.expected.selection;
        let bytes = byte_range(&self.expected.text, selection).ok_or(Failure::Selection)?;
        let mut next = self.expected.clone();
        next.text.replace_range(bytes, &chunk);
        next.selection = TextRange {
            location: selection.location + chunk.encode_utf16().count(),
            length: 0,
        };
        self.target.replace(selection, &chunk, &self.expected)?;
        self.pending = Some(next);
        Ok(false)
    }

    pub fn poll(&mut self) -> Result<bool, Failure> {
        let result = self.poll_inner();
        if let Err(failure) = result {
            self.fail(failure);
        }
        result
    }

    pub fn has_pending(&self) -> bool {
        self.submitted && !self.ended && self.failure.is_none()
    }

    pub fn end(&mut self) {
        self.target.abandon();
        self.pending = None;
        self.queue.clear();
        self.ended = true;
    }
}

pub trait TerminalTarget {
    /// Includes exact app/element, permissions, monitor and user input checks.
    fn current(&mut self) -> Result<(), Failure>;
    /// Revalidates for each Unicode event chunk. No Delete or Return events.
    fn post(&mut self, text: &str) -> Result<(), Failure>;
    fn end(&mut self);
}

impl<T: TerminalTarget + ?Sized> TerminalTarget for Box<T> {
    fn current(&mut self) -> Result<(), Failure> {
        (**self).current()
    }
    fn post(&mut self, text: &str) -> Result<(), Failure> {
        (**self).post(text)
    }
    fn end(&mut self) {
        (**self).end();
    }
}

pub struct Terminal<T: TerminalTarget> {
    target: T,
    previous: String,
    committed: String,
    pending: std::collections::VecDeque<String>,
    finalizing: bool,
    ended: bool,
    failure: Option<Failure>,
}

impl<T: TerminalTarget> Terminal<T> {
    pub fn capture(mut target: T) -> Result<Self, Failure> {
        target.current()?;
        Ok(Self {
            target,
            previous: String::new(),
            committed: String::new(),
            pending: std::collections::VecDeque::new(),
            finalizing: false,
            ended: false,
            failure: None,
        })
    }

    fn check(&mut self) -> Result<(), Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if self.ended {
            return Err(Failure::Ended);
        }
        self.target.current()
    }

    fn append(&mut self, candidate: &str) -> Result<(), Failure> {
        let suffix = candidate
            .strip_prefix(&self.committed)
            .ok_or(Failure::Revision)?;
        // Queue only. Native monitors must get an event-loop turn between
        // bounded posts, rather than starving while a large result is typed.
        self.pending
            .extend(unicode_chunks(suffix).into_iter().map(str::to_owned));
        self.committed = candidate.to_owned();
        Ok(())
    }

    pub fn update(&mut self, raw: &str, final_text: bool) -> Result<(), Failure> {
        // Late recognition callbacks must not cancel a valid final queue.
        if self.finalizing || self.ended {
            return Err(self.failure.unwrap_or(Failure::Ended));
        }
        let result = (|| {
            self.check()?;
            if raw.encode_utf16().count() > MAX_TEXT {
                return Err(Failure::Content);
            }
            let text = automatic(raw);
            let stable = if final_text {
                text.clone()
            } else {
                self.previous
                    .graphemes(true)
                    .zip(text.graphemes(true))
                    .take_while(|(a, b)| a == b)
                    .map(|(a, _)| a)
                    .collect()
            };
            self.previous = text;
            self.append(&stable)
        })();
        if let Err(failure) = result {
            self.failure = Some(failure);
            self.end();
        }
        if final_text && result.is_ok() {
            self.seal();
        }
        result
    }

    pub fn flush(&mut self) -> Result<(), Failure> {
        let result = self
            .check()
            .and_then(|()| self.append(&self.previous.clone()));
        if let Err(failure) = result {
            self.failure = Some(failure);
            self.end();
        }
        result
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Stop accepting new candidates but finish already accepted local text.
    pub fn seal(&mut self) {
        self.finalizing = true;
        if self.pending.is_empty() {
            self.end();
        }
    }

    /// Exactly one event chunk per main run-loop turn. Returning true means a
    /// sealed queue has finished and its native monitor has been removed.
    pub fn poll(&mut self) -> Result<bool, Failure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if self.ended {
            return Ok(true);
        }
        let result = (|| {
            self.check()?;
            if let Some(chunk) = self.pending.pop_front() {
                self.target.post(&chunk)?;
            }
            if self.finalizing && self.pending.is_empty() {
                self.end();
                return Ok(true);
            }
            Ok(false)
        })();
        if let Err(failure) = result {
            self.failure = Some(failure);
            self.end();
        }
        result
    }

    /// Cancellation discards unsent text; already posted terminal input stays.
    pub fn end(&mut self) {
        self.pending.clear();
        self.ended = true;
        self.target.end();
    }
}

impl<T: TerminalTarget> Drop for Terminal<T> {
    fn drop(&mut self) {
        self.target.end();
    }
}

/// At most 20 UTF-16 units, never split a surrogate pair. A single grapheme can
/// be longer than the transport limit; scalar boundaries are required here.
pub fn unicode_chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units + ch.len_utf16() > 20 {
            chunks.push(&text[start..byte]);
            start = byte;
            units = 0;
        }
        units += ch.len_utf16();
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}

/// Only the two configured stop chords and our tagged events are exempt.
pub fn invalidates_input(tag: i64, key_down: bool, key_code: u16, modifiers: u64) -> bool {
    const SHIFT: u64 = 1 << 17;
    const CONTROL: u64 = 1 << 18;
    const OPTION: u64 = 1 << 19;
    const COMMAND: u64 = 1 << 20;
    let modifiers = modifiers & (SHIFT | CONTROL | OPTION | COMMAND);
    let stop = key_down
        && key_code == 49
        && (modifiers == CONTROL | OPTION || modifiers == CONTROL | SHIFT);
    tag != EVENT_TAG && !stop
}
