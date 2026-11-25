use super::*;
use voiceport_control::DeliveryReceipt;

fn receipt(generation: u64, delivery: DeliveryReceipt) -> GuiDictationReceipt {
    GuiDictationReceipt {
        session: generation,
        generation,
        automatic: true,
        transcription: TranscriptionState::Completed,
        delivery,
        method: None,
        failure_stage: None,
        block_reason: None,
    }
}

fn for_receipt(phase: Phase, receipt: &GuiDictationReceipt) -> Presentation {
    Presentation::from_receipt(phase, receipt.generation, true, receipt).unwrap()
}

#[test]
fn visible_receipt_requires_current_completed_nonempty_gui_result() {
    let mut receipt = receipt(7, DeliveryReceipt::default());
    assert!(Presentation::from_receipt(Phase::Ready, 8, true, &receipt).is_none());
    for phase in [
        Phase::Idle,
        Phase::Permissions,
        Phase::Recording,
        Phase::Transcribing,
        Phase::Stopping,
        Phase::Failed,
    ] {
        assert!(Presentation::from_receipt(phase, 7, true, &receipt).is_none());
    }
    assert!(Presentation::from_receipt(Phase::Ready, 7, false, &receipt).is_none());
    receipt.transcription = TranscriptionState::Cancelled;
    assert!(Presentation::from_receipt(Phase::Ready, 7, true, &receipt).is_none());
}

#[test]
fn live_and_settled_delivery_have_distinct_status_and_next_step() {
    // Use production receipt transitions, not just labels constructed by the UI.
    let mut receipt = receipt(7, DeliveryReceipt::default());
    let not_sent = for_receipt(Phase::Ready, &receipt);
    receipt.delivery.waiting();
    let waiting = for_receipt(Phase::Delivering, &receipt);
    receipt.delivery.attempting();
    receipt.delivery.submitted(true);
    let readback = for_receipt(Phase::Delivering, &receipt);
    receipt.delivery.confirmed();
    let partial = for_receipt(Phase::Delivering, &receipt);
    receipt.delivery.finish();
    let confirmed = for_receipt(Phase::Ready, &receipt);
    receipt.delivery = DeliveryReceipt::default();
    receipt.delivery.submitted(false);
    let submitted = for_receipt(Phase::Ready, &receipt);
    receipt.delivery.attempting();
    receipt.delivery.stop();
    let uncertain = for_receipt(Phase::Ready, &receipt);

    for (model, outcome, status, next, recoverable) in [
        (not_sent, Outcome::NotSent, "未输入", "手动粘贴", true),
        (
            waiting,
            Outcome::WaitingForInput,
            "等待输入",
            "暂勿手动粘贴",
            false,
        ),
        (
            readback,
            Outcome::WaitingForReadback,
            "等待确认",
            "正在核对",
            false,
        ),
        (
            partial,
            Outcome::Partial,
            "已部分投递",
            "本次输入尚未结束",
            false,
        ),
        (
            submitted,
            Outcome::Submitted,
            "已提交输入请求",
            "尚未确认收到",
            true,
        ),
        (
            uncertain,
            Outcome::Uncertain,
            "输入结果不明",
            "可能已输入",
            true,
        ),
        (
            confirmed,
            Outcome::Confirmed,
            "已确认输入",
            "无需再次粘贴",
            true,
        ),
    ] {
        assert_eq!(model.outcome, outcome);
        assert_eq!(model.status, status);
        assert!(model.next.contains(next), "{outcome:?}: {}", model.next);
        assert_eq!(model.allows_reveal(7), recoverable);
    }
}

#[test]
fn permission_readonly_focus_become_specific_terminal_guidance() {
    for (stage, hint) in [
        ("accessibility", "辅助功能授权"),
        ("writable", "只读"),
        ("disabled", "只读"),
        ("focus", "未切换输入目标"),
    ] {
        let mut delivery = DeliveryReceipt::default();
        delivery.waiting();
        delivery.stop();
        let mut receipt = receipt(7, delivery);
        receipt.failure_stage = Some(stage.into());
        let model = for_receipt(Phase::Ready, &receipt);
        assert_eq!(model.outcome, Outcome::NotSent);
        assert!(model.reason.unwrap().contains(hint));
        assert!(model.next.contains("手动粘贴"));
    }
}

#[test]
fn focus_loss_after_a_write_differs_from_a_prewrite_stop() {
    let mut delivery = DeliveryReceipt::default();
    delivery.attempting();
    delivery.submitted(true);
    delivery.confirmed();
    delivery.waiting();
    delivery.stop();
    let mut receipt = receipt(7, delivery);
    receipt.failure_stage = Some("focus".into());
    let model = for_receipt(Phase::Ready, &receipt);
    assert_eq!(model.outcome, Outcome::Partial);
    assert!(model.next.contains("先检查"));
    assert!(model.reason.unwrap().contains("未切换输入目标"));
}

#[test]
fn ambiguous_write_with_zero_submissions_is_not_not_sent() {
    let mut delivery = DeliveryReceipt::default();
    delivery.attempting();
    delivery.stop();
    assert_eq!(delivery.submitted_operations, 0);
    let mut receipt = receipt(7, delivery);
    receipt.failure_stage = Some("write".into());
    let model = for_receipt(Phase::Ready, &receipt);
    assert_eq!(model.outcome, Outcome::Uncertain);
    assert!(model.next.contains("避免重复粘贴"));
    assert_eq!(model.reason, Some("无法核对本次写入。"));
    receipt.failure_stage = Some("private unexpected diagnostic".into());
    assert_eq!(
        for_receipt(Phase::Ready, &receipt).reason,
        Some("本次自动输入已停止。")
    );
}

#[test]
fn confirmation_requires_readback_evidence_and_submission_survives_stop() {
    let mut receipt = receipt(
        7,
        DeliveryReceipt {
            state: DeliveryState::Confirmed,
            submitted_operations: 2,
            confirmed_operations: 1,
            stopped: false,
        },
    );
    let model = for_receipt(Phase::Ready, &receipt);
    assert_eq!(model.outcome, Outcome::Uncertain);
    assert!(model.next.contains("先检查"));

    receipt.delivery = DeliveryReceipt::default();
    receipt.delivery.submitted(false);
    receipt.delivery.stop();
    let model = for_receipt(Phase::Ready, &receipt);
    assert_eq!(model.outcome, Outcome::Submitted);
    assert!(model.next.contains("尚未确认收到"));
}

#[test]
fn stale_earlier_writes_remain_context_not_current_success() {
    let mut delivery = DeliveryReceipt::default();
    delivery.submitted(true);
    delivery.confirmed();
    delivery.finish();
    let mut receipt = receipt(8, delivery);
    receipt.failure_stage = Some("focus".into());
    receipt.delivery.stop();
    assert_eq!(
        for_receipt(Phase::Ready, &receipt).outcome,
        Outcome::Partial
    );
    receipt.session = 7;
    let retry = for_receipt(Phase::Ready, &receipt);
    assert_eq!(retry.outcome, Outcome::NotSent);
    assert_eq!(retry.status, "本次重试未输入");
    assert!(retry.next.contains("先检查原输入框是否有上次的文字"));
    assert_eq!(retry.reason, None);

    let mut panel_only = self::receipt(9, DeliveryReceipt::default());
    panel_only.automatic = false;
    panel_only.block_reason = Some(DictationBlockReason::PanelOnly);
    let model = for_receipt(Phase::Ready, &panel_only);
    assert_eq!(model.outcome, Outcome::NotSent);
    assert!(model.reason.unwrap().contains("仅转写"));
}

#[test]
fn unsettled_stale_or_collapsed_actions_never_write_and_copy_is_once_per_result() {
    let mut receipt = receipt(7, DeliveryReceipt::default());
    receipt.delivery.waiting();
    let waiting = for_receipt(Phase::Delivering, &receipt);
    receipt.delivery.attempting();
    receipt.delivery.stop();
    let result = for_receipt(Phase::Ready, &receipt);
    let copy = CopyOnce::default();
    let mut fake_clipboard_writes = Vec::new();
    let mut clicked_copy = |displayed, expanded, model| {
        if copy.claim(displayed, expanded, model) {
            // This fake records the attempted side effect, even if its result is unknown.
            fake_clipboard_writes.push("本次🙂\n文字");
        }
    };
    assert!(!waiting.allows_reveal(7));
    assert!(!result.allows_reveal(6));
    clicked_copy(7, true, waiting);
    clicked_copy(6, true, result);
    clicked_copy(7, false, result);
    assert!(!copy.attempted(7));

    assert!(result.allows_reveal(7));
    clicked_copy(7, true, result);
    assert!(copy.attempted(7));
    clicked_copy(7, true, result); // Duplicate AppKit callback.
    clicked_copy(7, false, result); // Reopened card starts collapsed.
    clicked_copy(7, true, result); // Explicitly reopened body still cannot duplicate the copy.
    assert!(!copy.attempted(8));

    receipt.session = 8;
    receipt.generation = 8;
    let next = for_receipt(Phase::Ready, &receipt);
    clicked_copy(7, true, next); // Stale sender/model mismatch after a new result.
    clicked_copy(8, true, next);
    clicked_copy(7, true, result);
    assert_eq!(fake_clipboard_writes, ["本次🙂\n文字", "本次🙂\n文字"]);
}
