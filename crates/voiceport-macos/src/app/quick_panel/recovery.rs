use super::*;
use objc2::{Message, rc::Weak};
use voiceport_control::{
    DeliveryState, DictationBlockReason, GuiDictationReceipt, TranscriptionState,
};

const COLLAPSED_HEIGHT: f64 = 104.;
const EXPANDED_HEIGHT: f64 = 208.;

#[derive(Clone, Copy)]
struct ResultText<'a> {
    generation: u64,
    text: &'a str,
    delivery: &'static str,
    detail: Option<&'static str>,
}

impl<'a> ResultText<'a> {
    fn current(c: &'a Controller) -> Option<Self> {
        // Only the GUI receipt for this Controller generation owns this text.
        Self::from_receipt(
            c.state.phase,
            c.state.generation(),
            &c.transcript,
            &c.gui_dictation_receipt()?,
        )
    }

    fn from_receipt(
        phase: Phase,
        generation: u64,
        text: &'a str,
        receipt: &GuiDictationReceipt,
    ) -> Option<Self> {
        if phase != Phase::Ready
            || generation != receipt.generation
            || receipt.transcription != TranscriptionState::Completed
            || text.trim().is_empty()
        {
            return None;
        }
        let (delivery, detail) = if receipt.session != generation
            || receipt.block_reason == Some(DictationBlockReason::RetryPanelOnly)
        {
            // The receipt preserves earlier operations after an explicit retry.
            ("本次重试结果未投递，请检查原输入框是否已有文字。", None)
        } else {
            let delivery = match receipt.delivery.state {
                DeliveryState::NotSent if receipt.delivery.submitted_operations == 0 => {
                    "本次文字未投递。"
                }
                DeliveryState::Waiting => "投递结果不明，请先检查原输入框。",
                DeliveryState::Confirmed
                    if !receipt.delivery.stopped && receipt.failure_stage.is_none() =>
                {
                    "已确认投递。"
                }
                DeliveryState::NotSent
                | DeliveryState::PartiallySent
                | DeliveryState::Confirmed => "已尝试投递，可能已有部分文字。请先检查原输入框。",
                DeliveryState::Submitted => "已提交投递请求，请先检查原输入框是否收到。",
                DeliveryState::AwaitingConfirmation | DeliveryState::Uncertain => {
                    "投递结果不明，请先检查原输入框，避免重复粘贴。"
                }
            };
            (
                delivery,
                completed_delivery_failure(phase, true, receipt.failure_stage.as_deref()),
            )
        };
        Some(Self {
            generation,
            text,
            delivery,
            detail,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    Collapsed,
    Expanded,
    CopyAttempted,
}

impl Access {
    fn reveal(self, displayed: u64, result: ResultText<'_>) -> Option<Self> {
        (self == Self::Collapsed && displayed == result.generation).then_some(Self::Expanded)
    }

    fn claim_copy<'a>(
        access: &Cell<Self>,
        displayed: u64,
        result: ResultText<'a>,
    ) -> Option<&'a str> {
        if access.get() != Self::Expanded || displayed != result.generation {
            return None;
        }
        // Consume before native clipboard access so duplicate callbacks cannot write twice.
        access.set(Self::CopyAttempted);
        Some(result.text)
    }
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<Delegate>]
    pub(super) struct RecoveryActions;
    unsafe impl NSObjectProtocol for RecoveryActions {}
    impl RecoveryActions {
        #[unsafe(method(revealQuickText:))]
        fn reveal_action(&self, sender: &NSButton) {
            if let Some(owner) = self.ivars().load() {
                owner.reveal_quick_result(sender);
            }
        }
        #[unsafe(method(copyQuickText:))]
        fn copy_action(&self, sender: &NSButton) {
            if let Some(owner) = self.ivars().load() {
                owner.copy_quick_recovery(sender);
            }
        }
    }
);

impl RecoveryActions {
    pub(super) fn new(owner: &Delegate) -> Retained<Self> {
        let actions = Self::alloc(owner.mtm()).set_ivars(Weak::from_retained(&owner.retain()));
        unsafe { msg_send![super(actions), init] }
    }
}

pub(super) struct Card {
    generation: u64,
    access: Cell<Access>,
    view: Retained<NSView>,
    delivery: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    show: Retained<NSButton>,
    copy: Retained<NSButton>,
    feedback: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    text: Retained<NSTextView>,
}

impl Card {
    fn new(d: &Delegate, parent: &NSView, target: &RecoveryActions, generation: u64) -> Self {
        let mtm = d.mtm();
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0., HEADER_Y, WIDTH, COLLAPSED_HEIGHT),
        );
        parent.addSubview(&view);
        d.surface(&view, rect(12., 4., WIDTH - 24., COLLAPSED_HEIGHT - 10.));
        let title = d.label(&view, "本次转写", rect(24., 75., WIDTH - 48., 17.), 12.);
        title.setFont(Some(&NSFont::boldSystemFontOfSize(12.)));
        let delivery = d.caption(&view, "", rect(24., 46., WIDTH - 48., 30.));
        delivery.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.delivery")));
        let detail = d.caption(&view, "", rect(24., 29., WIDTH - 48., 16.));
        detail.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let action = |title, selector, id, x, width| {
            let button = d.button(&view, title, selector, x, 2., width);
            unsafe { button.setTarget(Some(target)) };
            button.setControlSize(NSControlSize::Small);
            button.setAccessibilityIdentifier(Some(id));
            button
        };
        let show = action(
            "查看本次文字",
            sel!(revealQuickText:),
            ns_string!("voiceport.quick.show-recovery"),
            20.,
            110.,
        );
        let copy = action(
            "复制本次文字",
            sel!(copyQuickText:),
            ns_string!("voiceport.quick.copy-recovery"),
            20.,
            110.,
        );
        copy.setHidden(true);
        let feedback = d.caption(&view, "", rect(134., 10., WIDTH - 158., 16.));
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(24., COLLAPSED_HEIGHT - 4., WIDTH - 48., 96.),
        );
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setHidden(true);
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            rect(0., 0., scroll.contentSize().width, 96.),
        );
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(false);
        text.setDrawsBackground(false);
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        // The freshly initialized text view owns its default text container.
        if let Some(container) = unsafe { text.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        text.setFont(Some(&NSFont::systemFontOfSize(13.)));
        text.setTextColor(Some(&NSColor::textColor()));
        text.setTextContainerInset(NSSize::new(4., 4.));
        text.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.recovery-text")));
        text.setAccessibilityLabel(Some(ns_string!("本次转写文字")));
        scroll.setDocumentView(Some(&text));
        view.addSubview(&scroll);
        Self {
            generation,
            access: Cell::new(Access::Collapsed),
            view,
            delivery,
            detail,
            show,
            copy,
            feedback,
            scroll,
            text,
        }
    }

    fn reveal(&self, sender: &NSButton, result: ResultText<'_>) {
        if !std::ptr::eq(&*self.show, sender) {
            return;
        }
        let Some(access) = self.access.get().reveal(self.generation, result) else {
            return;
        };
        self.access.set(access);
        self.text.setString(&NSString::from_str(result.text));
        self.text.scrollPoint(NSPoint::ZERO);
        self.scroll.setHidden(false);
        self.show.setHidden(true);
        self.copy.setHidden(false);
        self.view.setFrameSize(NSSize::new(WIDTH, EXPANDED_HEIGHT));
    }

    fn copy(&self, result: ResultText<'_>) -> Option<String> {
        let text = Access::claim_copy(&self.access, self.generation, result)?;
        self.copy.setEnabled(false);
        Some(text.into())
    }

    fn feedback(&self, copied: bool) {
        set_label_text(
            &self.feedback,
            if copied {
                "文字已复制"
            } else {
                "复制失败，可选中文字"
            },
        );
    }
}

impl Drop for Card {
    fn drop(&mut self) {
        self.text.setString(ns_string!(""));
        self.view.removeFromSuperview();
    }
}

impl QuickPanel {
    pub(super) fn clear_recovery(&self) {
        self.recovery.borrow_mut().take();
    }

    pub(super) fn update_recovery(&self, d: &Delegate, c: &Controller, idle: bool) -> f64 {
        let Some(result) = ResultText::current(c).filter(|_| idle && !d.ivars().quitting.get())
        else {
            self.clear_recovery();
            return 0.;
        };
        let mut card = self.recovery.borrow_mut();
        if card
            .as_ref()
            .is_none_or(|card| card.generation != result.generation)
        {
            let parent = self.popover.contentViewController().unwrap().view();
            *card = Some(Card::new(
                d,
                &parent,
                &self.recovery_actions,
                result.generation,
            ));
        }
        let card = card.as_ref().unwrap();
        set_label_text(&card.delivery, result.delivery);
        set_label_text(&card.detail, result.detail.unwrap_or_default());
        card.detail
            .setToolTip(result.detail.map(NSString::from_str).as_deref());
        card.view.frame().size.height
    }
}

impl Delegate {
    fn recovery_panel(&self) -> Option<&QuickPanel> {
        let panel = self.ivars().quick_panel.get()?;
        (panel.popover.isShown() && self.quick_actions_unlocked()).then_some(panel)
    }

    fn reveal_quick_result(&self, sender: &NSButton) {
        let Some(panel) = self.recovery_panel() else {
            return;
        };
        self.with_controller(|c| {
            if !c.busy()
                && let Some(card) = panel.recovery.borrow().as_ref()
                && let Some(result) = ResultText::current(c)
            {
                card.reveal(sender, result);
            }
        });
        self.refresh();
    }

    fn copy_quick_recovery(&self, sender: &NSButton) {
        let Some(panel) = self.recovery_panel() else {
            return;
        };
        let transcript = {
            let controller = self.ivars().controller.borrow();
            let card = panel.recovery.borrow();
            controller.as_ref().filter(|c| !c.busy()).and_then(|c| {
                let card = card.as_ref()?;
                if !std::ptr::eq(&*card.copy, sender) {
                    return None;
                }
                card.copy(ResultText::current(c)?)
            })
        };
        if let Some(transcript) = transcript {
            let copied = Delivery::copy(&transcript);
            if let Some(card) = panel.recovery.borrow().as_ref()
                && std::ptr::eq(&*card.copy, sender)
            {
                card.feedback(copied);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(generation: u64, state: DeliveryState) -> GuiDictationReceipt {
        GuiDictationReceipt {
            session: generation,
            generation,
            automatic: true,
            transcription: TranscriptionState::Completed,
            delivery: voiceport_control::DeliveryReceipt {
                state,
                ..Default::default()
            },
            method: None,
            failure_stage: None,
            block_reason: None,
        }
    }

    #[test]
    fn result_requires_current_completed_nonempty_gui_generation() {
        let mut receipt = receipt(7, DeliveryState::Submitted);
        assert!(ResultText::from_receipt(Phase::Ready, 8, "old text", &receipt).is_none());
        for phase in [
            Phase::Idle,
            Phase::Recording,
            Phase::Stopping,
            Phase::Failed,
        ] {
            assert!(ResultText::from_receipt(phase, 7, "old text", &receipt).is_none());
        }
        assert!(ResultText::from_receipt(Phase::Ready, 7, "  \n", &receipt).is_none());
        receipt.transcription = TranscriptionState::Cancelled;
        assert!(ResultText::from_receipt(Phase::Ready, 7, "cancelled text", &receipt).is_none());
    }

    #[test]
    fn explicit_reveal_and_copy_require_the_same_generation_once() {
        let receipt = receipt(7, DeliveryState::NotSent);
        let result = ResultText::from_receipt(Phase::Ready, 7, "最终🙂\n文字", &receipt).unwrap();
        let mut writes = Vec::new();
        let access = Cell::new(Access::Collapsed);
        let clicked_copy = |generation, writes: &mut Vec<&str>| {
            if let Some(text) = Access::claim_copy(&access, generation, result) {
                writes.push(text);
            }
        };
        clicked_copy(7, &mut writes);
        assert!(writes.is_empty());
        assert_eq!(access.get().reveal(6, result), None);
        access.set(access.get().reveal(7, result).unwrap());
        assert_eq!(access.get().reveal(7, result), None);
        clicked_copy(6, &mut writes);
        assert!(writes.is_empty());
        clicked_copy(7, &mut writes);
        clicked_copy(7, &mut writes);
        assert_eq!(writes, ["最终🙂\n文字"]);
        access.set(Access::Collapsed);
        clicked_copy(7, &mut writes);
        assert_eq!(writes.len(), 1);
    }

    #[test]
    fn attempts_uncertainty_confirmation_and_retry_have_distinct_feedback() {
        for (state, prefix) in [
            (DeliveryState::NotSent, "本次文字未投递"),
            (DeliveryState::PartiallySent, "已尝试投递"),
            (DeliveryState::Submitted, "已提交投递请求"),
            (DeliveryState::AwaitingConfirmation, "投递结果不明"),
            (DeliveryState::Uncertain, "投递结果不明"),
            (DeliveryState::Confirmed, "已确认投递"),
        ] {
            let receipt = receipt(7, state);
            let result = ResultText::from_receipt(Phase::Ready, 7, "文字", &receipt).unwrap();
            assert!(result.delivery.starts_with(prefix));
        }
        let mut interrupted = receipt(7, DeliveryState::Confirmed);
        interrupted.delivery.stopped = true;
        interrupted.failure_stage = Some("focus".into());
        let result = ResultText::from_receipt(Phase::Ready, 7, "文字", &interrupted).unwrap();
        assert!(result.delivery.starts_with("已尝试投递"));
        assert!(result.detail.unwrap().contains("无法确认原输入框"));

        let mut receipt = receipt(8, DeliveryState::Submitted);
        receipt.session = 7;
        receipt.failure_stage = Some("readback".into());
        let result = ResultText::from_receipt(Phase::Ready, 8, "重试文字", &receipt).unwrap();
        assert!(result.delivery.contains("本次重试结果未投递"));
        assert_eq!(result.detail, None);
    }
}
