//! Content-free presentation and action policy for the current GUI result.
use crate::lifecycle::Phase;
use std::cell::Cell;
use voiceport_control::{
    DeliveryState, DictationBlockReason, GuiDictationReceipt, TranscriptionState,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    NotSent,
    WaitingForInput,
    WaitingForReadback,
    Submitted,
    Partial,
    Uncertain,
    Confirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Presentation {
    pub(super) generation: u64,
    pub(super) outcome: Outcome,
    pub(super) status: &'static str,
    pub(super) next: &'static str,
    pub(super) reason: Option<&'static str>,
    settled: bool,
}

impl Presentation {
    pub(super) fn from_receipt(
        phase: Phase,
        generation: u64,
        has_text: bool,
        receipt: &GuiDictationReceipt,
    ) -> Option<Self> {
        if !matches!(phase, Phase::Ready | Phase::Delivering)
            || receipt.generation != generation
            || receipt.transcription != TranscriptionState::Completed
            || !has_text
        {
            return None;
        }

        if receipt.block_reason == Some(DictationBlockReason::InterruptedPanelOnly) {
            return Some(Self {
                generation,
                outcome: Outcome::NotSent,
                status: "不完整 · 仅供查看或复制",
                next: "原输入框可能已有实时文字，请先检查。",
                reason: None,
                settled: phase == Phase::Ready,
            });
        }

        // A retry transcribes only. Its receipt retains the prior attempt's writes.
        let retried = receipt.session != generation
            || receipt.block_reason == Some(DictationBlockReason::RetryPanelOnly);
        let delivery = receipt.delivery;
        let outcome = if retried {
            Outcome::NotSent
        } else {
            match delivery.state {
                DeliveryState::NotSent | DeliveryState::Waiting
                    if delivery.submitted_operations > 0 =>
                {
                    Outcome::Partial
                }
                DeliveryState::NotSent => Outcome::NotSent,
                DeliveryState::Waiting if delivery.stopped => Outcome::NotSent,
                DeliveryState::Waiting => Outcome::WaitingForInput,
                DeliveryState::AwaitingConfirmation
                    if phase == Phase::Delivering && !delivery.stopped =>
                {
                    Outcome::WaitingForReadback
                }
                DeliveryState::Submitted => Outcome::Submitted,
                DeliveryState::PartiallySent => Outcome::Partial,
                DeliveryState::AwaitingConfirmation | DeliveryState::Uncertain => {
                    Outcome::Uncertain
                }
                DeliveryState::Confirmed
                    if delivery.submitted_operations == 0
                        || delivery.confirmed_operations != delivery.submitted_operations =>
                {
                    Outcome::Uncertain
                }
                DeliveryState::Confirmed
                    if !delivery.stopped && receipt.failure_stage.is_none() =>
                {
                    Outcome::Confirmed
                }
                DeliveryState::Confirmed => Outcome::Partial,
            }
        };
        let (status, next) = match outcome {
            Outcome::NotSent if retried => (
                "本次重试未输入",
                "先检查原输入框是否有上次的文字，再决定是否复制和粘贴。",
            ),
            Outcome::NotSent => ("未输入", "可查看并复制文字，再到可编辑处手动粘贴。"),
            Outcome::WaitingForInput => ("等待输入", "请等待本次输入完成，暂勿手动粘贴。"),
            Outcome::WaitingForReadback => ("等待确认", "正在核对原输入框，暂勿手动粘贴。"),
            Outcome::Submitted => (
                "已提交输入请求",
                "尚未确认收到。请先检查原输入框，再决定是否粘贴。",
            ),
            Outcome::Partial if phase == Phase::Delivering => {
                ("已部分投递", "本次输入尚未结束，请先等待，暂勿手动粘贴。")
            }
            Outcome::Partial => ("已部分投递", "原框内可能已有文字，请先检查，避免重复粘贴。"),
            Outcome::Uncertain => (
                "输入结果不明",
                "文字可能已输入。请先检查原输入框，避免重复粘贴。",
            ),
            Outcome::Confirmed => ("已确认输入", "已在原输入框核对文字，无需再次粘贴。"),
        };
        let reason = if retried {
            None
        } else if receipt.block_reason == Some(DictationBlockReason::PanelOnly) {
            Some("本次仅转写，未启用自动输入。")
        } else if receipt.block_reason == Some(DictationBlockReason::Cancelled)
            && receipt.failure_stage.is_none()
        {
            Some("本次自动输入已取消。")
        } else {
            receipt.failure_stage.as_deref().map(stage_hint)
        };
        Some(Self {
            generation,
            outcome,
            status,
            next,
            reason,
            settled: phase == Phase::Ready,
        })
    }

    pub(super) fn allows_reveal(self, displayed_generation: u64) -> bool {
        self.settled
            && self.generation == displayed_generation
            && !matches!(
                self.outcome,
                Outcome::WaitingForInput | Outcome::WaitingForReadback
            )
    }
}

// Render only stable, user-safe reason text. Never surface raw AX diagnostics.
fn stage_hint(stage: &str) -> &'static str {
    match stage {
        "accessibility" => "请在系统设置中检查辅助功能授权。",
        "secureInput" | "sensitive" => "安全输入或密码框不支持自动输入。",
        "application" | "focus" => "无法确认原输入框，未切换输入目标。",
        "role" | "disabled" | "writable" => "原控件只读或不可安全编辑。",
        "content" | "selection" | "ownership" | "userInput" | "revision" => {
            "原文字或光标位置已变，已停止。"
        }
        "modifiers" => "修饰键未松开，已停止自动输入。",
        "clipboardChanged" => "剪贴板有新内容，已停止粘贴。",
        "clipboard" => "未能准备剪贴板文字。",
        "write" | "event" | "pasteRequest" | "readback" => "无法核对本次写入。",
        _ => "本次自动输入已停止。",
    }
}

/// One attempt per Controller generation, not per temporary popover/card.
#[derive(Default)]
pub(super) struct CopyOnce(Cell<Option<u64>>);

impl CopyOnce {
    pub(super) fn attempted(&self, generation: u64) -> bool {
        self.0.get().is_some_and(|last| last >= generation)
    }

    pub(super) fn claim(
        &self,
        displayed_generation: u64,
        expanded: bool,
        result: Presentation,
    ) -> bool {
        if !expanded
            || !result.allows_reveal(displayed_generation)
            || self.attempted(displayed_generation)
        {
            return false;
        }
        // Consume before touching the clipboard, even if native copy then fails.
        self.0.set(Some(displayed_generation));
        true
    }
}

#[cfg(test)]
mod tests;
