use super::{
    recovery_model::{CopyOnce, Presentation},
    *,
};
use objc2::{Message, rc::Weak};

const COLLAPSED_HEIGHT: f64 = 112.;
const EXPANDED_HEIGHT: f64 = COLLAPSED_HEIGHT + 104.;

fn current_presentation(c: &Controller) -> Option<Presentation> {
    Presentation::from_receipt(
        c.state.phase,
        c.state.generation(),
        !c.transcript.trim().is_empty(),
        &c.gui_dictation_receipt()?,
    )
}

/// Closing the popover clears its AppKit view and text, but never rearms copy.
#[derive(Default)]
pub(super) struct ResultCard {
    visible: RefCell<Option<Card>>,
    copy_once: CopyOnce,
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

struct Card {
    generation: u64,
    expanded: Cell<bool>,
    view: Retained<NSView>,
    delivery: Retained<NSTextField>,
    next: Retained<NSTextField>,
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
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., WIDTH, COLLAPSED_HEIGHT));
        parent.addSubview(&view);
        d.surface(&view, rect(12., 4., WIDTH - 24., COLLAPSED_HEIGHT - 6.));
        let title = d.label(&view, "本次转写", rect(24., 95., WIDTH - 48., 15.), 12.);
        title.setFont(Some(&NSFont::boldSystemFontOfSize(12.)));
        let delivery = d.label(&view, "", rect(24., 79., WIDTH - 48., 16.), 12.);
        delivery.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.delivery")));
        let next = d.caption(&view, "", rect(24., 50., WIDTH - 48., 29.));
        next.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.delivery.next")));
        let detail = d.caption(&view, "", rect(24., 35., WIDTH - 48., 15.));
        detail.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let action = |title, selector, id, x, width| {
            let button = panel_button(d, &view, title, selector, rect(x, 8., width, 27.));
            unsafe { button.setTarget(Some(target)) };
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
            expanded: Cell::new(false),
            view,
            delivery,
            next,
            detail,
            show,
            copy,
            feedback,
            scroll,
            text,
        }
    }

    fn update(&self, result: Presentation, unlocked: bool, attempted: bool) {
        set_label_text(&self.delivery, result.status);
        set_label_text(&self.next, result.next);
        set_label_text(&self.detail, result.reason.unwrap_or_default());
        self.detail
            .setToolTip(result.reason.map(NSString::from_str).as_deref());
        let actionable = unlocked && result.allows_reveal(self.generation);
        self.show.setEnabled(actionable);
        self.copy.setEnabled(actionable && !attempted);
    }

    fn reveal(&self, sender: &NSButton, result: Presentation, text: &str, attempted: bool) -> bool {
        if !std::ptr::eq(&*self.show, sender)
            || self.expanded.get()
            || !result.allows_reveal(self.generation)
        {
            return false;
        }
        self.expanded.set(true);
        self.text.setString(&NSString::from_str(text));
        self.text.scrollPoint(NSPoint::ZERO);
        self.scroll.setHidden(false);
        self.show.setHidden(true);
        self.copy.setHidden(false);
        if attempted {
            set_label_text(&self.feedback, "已尝试；可选中文字");
        }
        self.view.setFrameSize(NSSize::new(WIDTH, EXPANDED_HEIGHT));
        true
    }

    fn claim_copy(&self, sender: &NSButton, result: Presentation, copy_once: &CopyOnce) -> bool {
        if !std::ptr::eq(&*self.copy, sender)
            || !copy_once.claim(self.generation, self.expanded.get(), result)
        {
            return false;
        }
        self.copy.setEnabled(false);
        true
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
    pub(super) fn append_recovery_key_views(&self, controls: &mut Vec<Retained<NSView>>) {
        if let Some(card) = self.recovery.visible.borrow().as_ref() {
            // AppKit skips disabled controls. Keep the visible button linked
            // when a receipt settles without changing the card's layout.
            if card.expanded.get() {
                controls.push(card.copy.clone().into_super().into_super());
                controls.push(card.text.clone().into_super().into_super());
            } else {
                controls.push(card.show.clone().into_super().into_super());
            }
        }
    }

    pub(super) fn clear_recovery(&self) {
        self.recovery.visible.borrow_mut().take();
    }

    pub(super) fn update_recovery(&self, d: &Delegate, c: &Controller, idle: bool) -> f64 {
        self.present_recovery(
            d,
            current_presentation(c).filter(|_| !d.ivars().quitting.get()),
            idle && d.quick_actions_unlocked(),
        )
    }

    fn present_recovery(&self, d: &Delegate, result: Option<Presentation>, unlocked: bool) -> f64 {
        let Some(result) = result else {
            self.clear_recovery();
            return 0.;
        };
        let mut card = self.recovery.visible.borrow_mut();
        if card
            .as_ref()
            .is_none_or(|card| card.generation != result.generation)
        {
            *card = Some(Card::new(
                d,
                &self.recovery_slot,
                &self.recovery_actions,
                result.generation,
            ));
        }
        let card = card.as_ref().unwrap();
        card.update(
            result,
            unlocked,
            self.recovery.copy_once.attempted(result.generation),
        );
        card.view.frame().size.height
    }

    #[cfg(debug_assertions)]
    fn fixture_receipt(waiting: bool) -> Presentation {
        use crate::lifecycle::Phase;
        use voiceport_control::{
            DeliveryReceipt, DeliveryState, GuiDictationReceipt, TranscriptionState,
        };

        let (phase, state) = if waiting {
            (Phase::Delivering, DeliveryState::Waiting)
        } else {
            (Phase::Ready, DeliveryState::Uncertain)
        };
        Presentation::from_receipt(
            phase,
            7,
            true,
            &GuiDictationReceipt {
                session: 7,
                generation: 7,
                automatic: true,
                transcription: TranscriptionState::Completed,
                delivery: DeliveryReceipt {
                    state,
                    ..Default::default()
                },
                method: None,
                failure_stage: Some("readback".into()),
                block_reason: None,
            },
        )
        .expect("synthetic receipt must be displayable")
    }

    #[cfg(debug_assertions)]
    pub(super) fn fixture_recovery(&self, d: &Delegate, reveal: bool) -> (f64, Retained<NSButton>) {
        let result = Self::fixture_receipt(false);
        self.present_recovery(d, Some(result), true);
        let card = self.recovery.visible.borrow();
        let card = card.as_ref().unwrap();
        assert_eq!(card.delivery.stringValue().to_string(), result.status);
        if reveal {
            assert!(card.reveal(
                &card.show,
                result,
                "仅供测试的合成文字 · Synthetic result",
                false
            ));
        } else {
            assert!(card.text.string().is_empty());
        }
        let action = if reveal { &card.copy } else { &card.show };
        (card.view.frame().size.height, action.clone())
    }

    #[cfg(debug_assertions)]
    pub(super) fn fixture_recovery_copy_once(&self, d: &Delegate) {
        let result = Self::fixture_receipt(false);
        let waiting = Self::fixture_receipt(true);
        {
            let visible = self.recovery.visible.borrow();
            let card = visible.as_ref().unwrap();
            assert!(!card.claim_copy(&card.copy, waiting, &self.recovery.copy_once));
            assert!(card.claim_copy(&card.copy, result, &self.recovery.copy_once));
            assert!(!card.claim_copy(&card.copy, result, &self.recovery.copy_once));
        }
        self.clear_recovery();
        self.present_recovery(d, Some(waiting), true);
        {
            let visible = self.recovery.visible.borrow();
            let card = visible.as_ref().unwrap();
            assert!(!card.show.isEnabled());
            assert!(card.text.string().is_empty());
            assert!(!card.reveal(&card.show, waiting, "synthetic text", false));
        }
        self.present_recovery(d, Some(result), true);
        let visible = self.recovery.visible.borrow();
        let card = visible.as_ref().unwrap();
        assert!(card.show.isEnabled());
        assert!(card.reveal(&card.show, result, "synthetic text", true));
        assert!(!card.copy.isEnabled());
        assert!(!card.claim_copy(&card.copy, result, &self.recovery.copy_once));
        println!(
            "quick-panel fixture: waiting recovery and once-per-generation copy verified without a clipboard"
        );
    }

    #[cfg(debug_assertions)]
    pub(super) fn fixture_recovery_is_empty(&self) -> bool {
        self.recovery.visible.borrow().is_none()
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
        let focused = sender
            .window()
            .filter(|window| window.firstResponder().as_deref() == Some(&****sender));
        let mut next = None;
        self.with_controller(|c| {
            if !c.busy()
                && let Some(card) = panel.recovery.visible.borrow().as_ref()
                && let Some(result) = current_presentation(c)
                && card.reveal(
                    sender,
                    result,
                    &c.transcript,
                    panel.recovery.copy_once.attempted(result.generation),
                )
            {
                next = Some(if card.copy.isEnabled() {
                    card.copy.clone().into_super().into_super()
                } else {
                    card.text.clone().into_super().into_super()
                });
            }
        });
        self.refresh();
        if let (Some(window), Some(next)) = (focused, next) {
            window.makeFirstResponder(Some(&next));
        }
    }

    fn copy_quick_recovery(&self, sender: &NSButton) {
        let Some(panel) = self.recovery_panel() else {
            return;
        };
        let transcript = {
            let controller = self.ivars().controller.borrow();
            let card = panel.recovery.visible.borrow();
            controller.as_ref().filter(|c| !c.busy()).and_then(|c| {
                let card = card.as_ref()?;
                card.claim_copy(sender, current_presentation(c)?, &panel.recovery.copy_once)
                    .then(|| c.transcript.clone())
            })
        };
        if let Some(transcript) = transcript {
            let copied = Delivery::copy(&transcript);
            if let Some(card) = panel.recovery.visible.borrow().as_ref()
                && std::ptr::eq(&*card.copy, sender)
            {
                card.feedback(copied);
            }
        }
    }
}
