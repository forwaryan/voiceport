//! Non-interactive AppKit HUD; no text, app name or device metadata crosses into it.
use crate::{
    delivery::OverlayFocus,
    status_overlay::{CaptureStage, Snapshot, Stage, Visibility},
};
use objc2::{MainThreadOnly, define_class, msg_send, rc::Retained};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSPoint, NSRect, NSSize, NSString, NSUserDefaults, ns_string,
};

#[cfg(test)]
#[allow(dead_code)] // The opt-in native fixture runs on the OS main thread.
#[path = "status_overlay_fixture.rs"]
pub(super) mod fixture;
use std::time::{Duration, Instant};

const WIDTH: f64 = 180.;
const HEIGHT: f64 = 36.;
const MARGIN: f64 = 14.;
const GEOMETRY_INTERVAL: Duration = Duration::from_millis(300);

pub(super) fn enabled() -> bool {
    !NSUserDefaults::standardUserDefaults().boolForKey(ns_string!("hideDictationStatusOverlay"))
}

pub(super) fn set_enabled(enabled: bool) {
    NSUserDefaults::standardUserDefaults()
        .setBool_forKey(!enabled, ns_string!("hideDictationStatusOverlay"));
}

pub(super) fn checked(enabled: bool) -> NSControlStateValue {
    if enabled {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    }
}

#[derive(Default)]
pub(super) struct Overlay {
    generation: Option<u64>,
    visibility: Visibility,
    anchor: Option<Anchor>,
    panel: Option<Panel>,
}

impl Overlay {
    pub(super) fn update(
        &mut self,
        mtm: MainThreadMarker,
        snapshot: Snapshot,
        enabled: bool,
        capture_level: impl FnOnce() -> Option<u8>,
    ) {
        self.update_with_motion(mtm, snapshot, enabled, capture_level, || {
            NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
        });
    }

    fn update_with_motion(
        &mut self,
        mtm: MainThreadMarker,
        snapshot: Snapshot,
        enabled: bool,
        capture_level: impl FnOnce() -> Option<u8>,
        reduce_motion: impl FnOnce() -> bool,
    ) {
        if self
            .generation
            .is_some_and(|current| current > snapshot.generation)
        {
            return;
        }
        if self.generation != Some(snapshot.generation) {
            self.hide();
            self.generation = Some(snapshot.generation);
        }
        let Some(stage) = self.visibility.observe(snapshot, enabled) else {
            self.hide();
            return;
        };
        if self.anchor.is_none() {
            self.anchor = snapshot
                .origin_pid
                .and_then(|pid| Anchor::capture(mtm, pid));
        }
        if !self
            .anchor
            .as_mut()
            .is_some_and(|anchor| anchor.update(mtm))
        {
            self.visibility.dismiss();
            self.hide();
            return;
        }
        let Some(frame) = self.anchor.as_ref().and_then(|anchor| anchor.frame(mtm)) else {
            if let Some(panel) = &self.panel {
                panel.hide();
            }
            return;
        };
        let level = (snapshot.stage == Some(stage) && stage.shows_level() && !reduce_motion())
            .then(capture_level)
            .flatten();
        self.panel
            .get_or_insert_with(|| Panel::new(mtm))
            .show(frame, stage, level);
    }

    fn hide(&mut self) {
        self.anchor = None;
        if let Some(panel) = &self.panel {
            panel.hide();
        }
    }

    pub(super) fn close(&mut self) {
        self.visibility.close();
        self.hide();
    }
}

#[derive(Clone, Copy)]
struct Screen {
    frame: NSRect,
    visible: NSRect,
}

#[derive(Clone, Copy, Default)]
struct Bounds {
    field: Option<NSRect>,
    window: Option<NSRect>,
}

enum Focus {
    Local {
        window: Option<Retained<NSWindow>>,
        responder: Option<Retained<NSResponder>>,
    },
    External(Option<OverlayFocus>),
}

struct Anchor {
    application: Retained<NSRunningApplication>,
    focus: Focus,
    mouse_at_start: NSPoint,
    last_checked: Option<Instant>,
    bounds: Bounds,
}

impl Anchor {
    fn capture(mtm: MainThreadMarker, pid: i32) -> Option<Self> {
        let application = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        if application.processIdentifier() != pid {
            return None;
        }
        let focus = if pid as u32 == std::process::id() {
            let window = NSApplication::sharedApplication(mtm).keyWindow();
            let responder = window.as_ref().and_then(|window| window.firstResponder());
            Focus::Local { window, responder }
        } else {
            Focus::External(OverlayFocus::capture(pid))
        };
        Some(Self {
            application,
            focus,
            mouse_at_start: NSEvent::mouseLocation(),
            last_checked: None,
            bounds: Bounds::default(),
        })
    }

    fn update(&mut self, mtm: MainThreadMarker) -> bool {
        if !self.application.isActive() {
            return false;
        }
        match &self.focus {
            Focus::Local { window, responder } => {
                let current = NSApplication::sharedApplication(mtm).keyWindow();
                if current.as_deref().map(std::ptr::from_ref)
                    != window.as_deref().map(std::ptr::from_ref)
                    || current
                        .as_ref()
                        .and_then(|window| window.firstResponder())
                        .as_deref()
                        .map(std::ptr::from_ref)
                        != responder.as_deref().map(std::ptr::from_ref)
                {
                    return false;
                }
                self.bounds = Bounds {
                    field: window.as_ref().and_then(|window| {
                        let view = responder.clone()?.downcast::<NSView>().ok()?;
                        Some(
                            window
                                .convertRectToScreen(view.convertRect_toView(view.bounds(), None)),
                        )
                    }),
                    window: window.as_ref().map(|window| window.frame()),
                };
            }
            Focus::External(Some(focus)) => {
                let now = Instant::now();
                if self
                    .last_checked
                    .is_none_or(|previous| now.duration_since(previous) >= GEOMETRY_INTERVAL)
                {
                    self.last_checked = Some(now);
                    let Some((field, window)) = focus.observe() else {
                        return false;
                    };
                    // AppKit's global origin is at the lower-left of the primary
                    // display; AX gives upper-left coordinates in screen points.
                    let primary_top = NSScreen::screens(mtm).firstObject().map(|s| top(s.frame()));
                    let convert = |r: core_graphics::geometry::CGRect| -> Option<NSRect> {
                        Some(rect(
                            r.origin.x,
                            primary_top? - r.origin.y - r.size.height,
                            r.size.width,
                            r.size.height,
                        ))
                    };
                    // A transient geometry timeout must not discard a
                    // previously known exclusion region for the same field.
                    let field = field.and_then(convert);
                    if field.is_some() || self.bounds.field.is_none() {
                        self.bounds.field = field;
                    }
                    if let Some(window) = window.and_then(convert) {
                        self.bounds.window = Some(window);
                    }
                }
            }
            Focus::External(None) => {} // No AX grant/metadata: original app identity still gates visibility.
        }
        self.application.isActive()
    }

    fn frame(&self, mtm: MainThreadMarker) -> Option<NSRect> {
        let screens: Vec<_> = NSScreen::screens(mtm)
            .iter()
            .map(|screen| Screen {
                frame: screen.frame(),
                visible: screen.visibleFrame(),
            })
            .collect();
        place(&screens, self.bounds, self.mouse_at_start)
    }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn right(r: NSRect) -> f64 {
    r.origin.x + r.size.width
}
fn top(r: NSRect) -> f64 {
    r.origin.y + r.size.height
}
fn overlap(a: NSRect, b: NSRect) -> f64 {
    (right(a).min(right(b)) - a.origin.x.max(b.origin.x)).max(0.)
        * (top(a).min(top(b)) - a.origin.y.max(b.origin.y)).max(0.)
}

fn place(screens: &[Screen], bounds: Bounds, pointer: NSPoint) -> Option<NSRect> {
    let screen = [bounds.field, bounds.window]
        .into_iter()
        .flatten()
        .find_map(|target| {
            screens
                .iter()
                .filter_map(|screen| {
                    let area = overlap(screen.frame, target);
                    (area > 0.).then_some((screen, area))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(screen, _)| screen)
        })
        .or_else(|| {
            screens.iter().find(|screen| {
                let s = screen.frame;
                (s.origin.x..right(s)).contains(&pointer.x)
                    && (s.origin.y..top(s)).contains(&pointer.y)
            })
        })
        .or_else(|| screens.first())?;
    let s = screen.visible;
    if s.size.width < WIDTH + 2. * MARGIN || s.size.height < HEIGHT + 2. * MARGIN {
        return None;
    }
    let left = s.origin.x + MARGIN;
    let right = right(s) - WIDTH - MARGIN;
    let center = s.origin.x + (s.size.width - WIDTH) / 2.;
    let upper = top(s) - HEIGHT - MARGIN;
    let lower = s.origin.y + MARGIN;
    let avoid = bounds.field.map(|f| {
        rect(
            f.origin.x - 8.,
            f.origin.y - 8.,
            f.size.width + 16.,
            f.size.height + 16.,
        )
    });
    [
        (center, upper),
        (right, upper),
        (left, upper),
        (center, lower),
        (right, lower),
        (left, lower),
    ]
    .into_iter()
    .map(|(x, y)| rect(x, y, WIDTH, HEIGHT))
    .find(|candidate| avoid.is_none_or(|field| overlap(*candidate, field) == 0.))
}

define_class!(
    #[unsafe(super = NSPanel)]
    #[thread_kind = MainThreadOnly]
    struct PassivePanel;

    impl PassivePanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { false }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
    }
);

struct Panel {
    window: Retained<PassivePanel>,
    label: Retained<NSTextField>,
    icon: Retained<NSImageView>,
    bars: [Retained<NSBox>; 4],
    last: Option<(Stage, Option<u8>)>,
}

impl Panel {
    fn new(mtm: MainThreadMarker) -> Self {
        let window: Retained<PassivePanel> = unsafe {
            msg_send![super(PassivePanel::alloc(mtm).set_ivars(())),
                initWithContentRect: rect(0., 0., WIDTH, HEIGHT),
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false]
        };
        // The Rust Retained is the sole owner of the panel's lifetime.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setHidesOnDeactivate(false);
        window.setExcludedFromWindowsMenu(true);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSFloatingWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        let card = NSBox::initWithFrame(NSBox::alloc(mtm), rect(0., 0., WIDTH, HEIGHT));
        card.setBoxType(NSBoxType::Custom);
        card.setTitlePosition(NSTitlePosition::NoTitle);
        card.setBorderWidth(0.5);
        card.setBorderColor(&NSColor::separatorColor());
        card.setCornerRadius(12.);
        card.setFillColor(&NSColor::windowBackgroundColor());
        let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., WIDTH, HEIGHT));
        content.addSubview(&card);
        let icon = NSImageView::initWithFrame(NSImageView::alloc(mtm), rect(13., 10., 16., 16.));
        icon.setAccessibilityElement(false);
        content.addSubview(&icon);
        let label = NSTextField::labelWithString(ns_string!(""), mtm);
        label.setFrame(rect(37., 9., 102., 18.));
        label.setFont(Some(&NSFont::systemFontOfSize(13.)));
        label.setAccessibilityElement(true);
        content.addSubview(&label);
        let bars = std::array::from_fn(|i| {
            let height = 4. + 3. * i as f64;
            let bar = NSBox::initWithFrame(
                NSBox::alloc(mtm),
                rect(142. + 6. * i as f64, (HEIGHT - height) / 2., 3., height),
            );
            bar.setBoxType(NSBoxType::Custom);
            bar.setBorderWidth(0.);
            bar.setCornerRadius(1.);
            bar.setFillColor(&NSColor::labelColor());
            bar.setAccessibilityElement(false);
            content.addSubview(&bar);
            bar
        });
        window.setContentView(Some(&content));
        Self {
            window,
            label,
            icon,
            bars,
            last: None,
        }
    }

    fn show(&mut self, frame: NSRect, stage: Stage, level: Option<u8>) {
        if self.last.is_none_or(|(last, _)| last != stage) {
            let (text, icon) = match stage {
                Stage::Preparing => ("正在准备听写", "mic"),
                Stage::Capture(capture) => (
                    capture.title(),
                    match capture {
                        CaptureStage::Opening => "mic",
                        CaptureStage::Recording | CaptureStage::Resumed => "record.circle.fill",
                        CaptureStage::Recovering => "arrow.clockwise",
                        CaptureStage::Disconnected => "mic.slash",
                        CaptureStage::Finalizing => "stop.circle",
                    },
                ),
                Stage::Transcribing => ("正在转写", "waveform"),
                Stage::Delivering => ("正在输入文字", "text.cursor"),
            };
            let text = NSString::from_str(text);
            self.label.setStringValue(&text);
            self.label.setAccessibilityLabel(Some(&text));
            let icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(icon),
                None,
            );
            if let Some(icon) = &icon {
                icon.setTemplate(true);
            }
            self.icon.setImage(icon.as_deref());
            let tint = if stage.shows_level() {
                NSColor::systemRedColor()
            } else {
                NSColor::labelColor()
            };
            self.icon.setContentTintColor(Some(&tint));
        }
        if self.last != Some((stage, level)) {
            for (i, bar) in self.bars.iter().enumerate() {
                bar.setHidden(level.is_none_or(|level| i >= level as usize));
            }
            self.last = Some((stage, level));
        }
        if self.window.frame() != frame {
            self.window.setFrame_display_animate(frame, true, false);
        }
        if !self.window.isVisible() {
            self.window.orderFrontRegardless();
        }
    }

    fn hide(&self) {
        if self.window.isVisible() {
            self.window.orderOut(None);
        }
    }
}
