//! Explicit actions on the existing final result. No transcript is retained here.
use super::{Controller, Delivery};
use crate::delivery::model::{ApplicationTarget, DeliveryReceipt, DeliveryState, Failure};
use serde_json::{Value, json};
use voiceport_control::Response;

const COPY_MANUALLY: &str = "可复制最近听写，确认位置后手动粘贴。";
const DUPLICATE: &str = "之前的投递可能已写入，重贴可能重复。请先检查输入框，再按需手动复制粘贴。";

#[derive(Default)]
pub(super) struct LatestOutput {
    delivery: Delivery,
    generation: Option<u64>,
    notice: Option<&'static str>,
}

impl LatestOutput {
    pub(super) fn has_pending(&self) -> bool {
        self.delivery.has_pending()
    }

    pub(super) fn clear_notice(&mut self) {
        self.notice = None;
    }

    fn receipt(&self) -> Value {
        json!({
            "generation": self.generation,
            "delivery": self.delivery.receipt(),
            "method": self.delivery.method(),
            "failure_stage": self.delivery.failure_stage(),
        })
    }
}

fn may_have_written(receipt: DeliveryReceipt) -> bool {
    receipt.submitted_operations > 0
        || matches!(
            receipt.state,
            DeliveryState::AwaitingConfirmation | DeliveryState::Uncertain
        )
}

impl Controller {
    pub(crate) fn has_latest_result(&self) -> bool {
        self.latest_result().is_some()
    }

    pub(crate) fn latest_output_notice(&self) -> Option<&'static str> {
        self.latest_output.notice
    }

    pub(crate) fn repaste_receipt(&self) -> Option<Value> {
        self.latest_output
            .generation
            .map(|_| self.latest_output.receipt())
    }

    fn latest_feedback(&mut self, notice: &'static str, message: impl Into<String>) {
        self.latest_output.notice = Some(notice);
        self.message = message.into();
    }

    fn no_latest_result(&mut self) -> Response {
        self.latest_feedback("没有合格的最近听写", "没有可复制或重贴的最近听写。");
        Response::error("not_found", "No eligible final GUI dictation is available.")
    }

    pub(crate) fn copy_latest_with(&mut self, copy: impl FnOnce(&str) -> bool) -> Response {
        let Some(result) = self.latest_result() else {
            return self.no_latest_result();
        };
        let generation = result.generation;
        let possible_duplicate = result.possible_duplicate;
        if !copy(result.text) {
            self.latest_feedback("复制失败", "复制失败，请在结果区手动复制文字。");
            return Response::error("clipboard_error", "Could not copy the latest dictation.");
        }
        if possible_duplicate {
            self.latest_feedback("已复制 · 粘贴可能重复", format!("文字已复制。{DUPLICATE}"));
        } else {
            self.latest_feedback("文字已复制", "最近听写已复制。");
        }
        Response::ok(
            json!({"generation":generation,"copied":true,"possible_duplicate":possible_duplicate}),
        )
    }

    pub(crate) fn repaste_latest(&mut self, target: ApplicationTarget) -> Response {
        self.repaste_latest_with(|delivery| delivery.begin_repaste(&target))
    }

    pub(crate) fn repaste_latest_with(
        &mut self,
        capture: impl FnOnce(&mut Delivery) -> Result<(), String>,
    ) -> Response {
        if self.busy() {
            return Response::error(
                "busy",
                "App is busy; wait for the current operation to settle.",
            );
        }
        let Some(result) = self.latest_result() else {
            return self.no_latest_result();
        };
        let generation = result.generation;
        if result.possible_duplicate {
            self.latest_feedback("重贴可能重复 · 请检查输入框", DUPLICATE);
            let mut response = Response::error(
                "possible_duplicate",
                "An earlier delivery may have written this result. Inspect the target before copying and pasting manually.",
            );
            response.data = json!({"generation":generation,"possible_duplicate":true});
            return response;
        }
        let text = result.text.to_owned();
        self.latest_output.generation = Some(generation);
        let delivery = &mut self.latest_output.delivery;
        let message = match capture(delivery) {
            Ok(()) => delivery.deliver(&text, true),
            Err(message) => message,
        };
        self.repaste_feedback(message)
    }

    pub(crate) fn reject_repaste_without_target(&mut self) {
        self.latest_feedback(
            "未重贴 · 无法确认输入框",
            format!("{}{COPY_MANUALLY}", Failure::Focus.message()),
        );
    }

    fn repaste_feedback(&mut self, message: String) -> Response {
        let output = &self.latest_output;
        let generation = output.generation.expect("active or settled explicit paste");
        let receipt = output.delivery.receipt();
        let data = output.receipt();
        let pending = output.has_pending();
        let failed = output.delivery.failure_stage().is_some()
            || (!pending && receipt.state != DeliveryState::Confirmed);
        let writes = may_have_written(receipt);
        if writes {
            self.mark_repasted(generation);
        }
        if pending {
            self.latest_feedback("正在确认重贴", message);
        } else if failed && writes {
            self.latest_feedback(
                "重贴可能已写入 · 请检查输入框",
                format!("{message} {DUPLICATE}"),
            );
        } else if failed {
            self.latest_feedback("未重贴 · 可手动复制", format!("{message} {COPY_MANUALLY}"));
        } else {
            self.latest_feedback(
                "已重贴 · 请检查输入框",
                "已在当前输入框重贴最近听写，请检查文字。",
            );
        }
        if failed {
            let mut response = Response::error(
                "paste_blocked",
                if writes {
                    "Delivery may have written text. Inspect the target before copying and pasting manually."
                } else {
                    "Could not paste safely in the selected target. Copy the result for manual paste."
                },
            );
            response.data = data;
            response
        } else {
            Response::ok(data)
        }
    }

    pub(super) fn poll_repaste(&mut self) {
        if let Some(message) = self.latest_output.delivery.poll() {
            self.repaste_feedback(message);
        }
    }

    pub(super) fn cancel_repaste(&mut self) -> bool {
        if !self.latest_output.has_pending() {
            return false;
        }
        self.latest_output.delivery.clear();
        self.repaste_feedback("已停止重贴。".into());
        true
    }
}
