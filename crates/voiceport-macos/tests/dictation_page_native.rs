//! Real AppKit views on the OS main thread; no window is ordered front, and copy
//! goes to a test-owned closure rather than any system pasteboard.
#[cfg(target_os = "macos")]
#[path = "../src/app/dictation_page.rs"]
mod dictation_page;

#[cfg(target_os = "macos")]
fn main() {
    use block2::RcBlock;
    use dictation_page::{Activity, Page, Snapshot, TranscriptOrigin};
    use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
    use objc2_app_kit::*;
    use objc2_foundation::{NSObject, NSPoint, NSRange, NSRect, NSSize, NSString};
    use std::cell::Cell;
    use voiceport_control::{
        CaptureSignal, CaptureSummary, DeliveryReceipt, DeliveryState, DictationTimings,
        GuiDictationDiagnostics, GuiDictationReceipt, TranscriptionState,
    };

    let mtm = MainThreadMarker::new().expect("fixture must run on the OS main thread");
    objc2::rc::autoreleasepool(|_| {
        let _application = NSApplication::sharedApplication(mtm);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(572., 408.)),
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                true,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setAutorecalculatesKeyViewLoop(false);
        let view = window.contentView().unwrap();
        let target = NSObject::new();
        let page = Page::build(mtm, &target, &view);
        let start = NSButton::initWithFrame(NSButton::alloc(mtm), NSRect::ZERO);
        view.addSubview(&start);
        page.link_key_views(&start);
        assert!(
            unsafe { start.nextKeyView() }
                .as_deref()
                .is_some_and(|v| std::ptr::eq(v, &***page.disclosure))
        );
        assert!(
            unsafe { page.copy.nextKeyView() }
                .as_deref()
                .is_some_and(|v| std::ptr::eq(v, &***page.result))
        );
        assert_eq!(
            page.disclosure.action(),
            Some(sel!(toggleDictationDiagnostics:))
        );
        assert_eq!(page.edit.action(), Some(sel!(editDictationCopy:)));
        assert_eq!(page.copy.action(), Some(sel!(copyDictationResult:)));

        let receipt = GuiDictationReceipt {
            session: 17,
            generation: 18,
            automatic: true,
            transcription: TranscriptionState::Completed,
            delivery: DeliveryReceipt {
                state: DeliveryState::Submitted,
                submitted_operations: 1,
                ..Default::default()
            },
            method: Some("clipboard_final".into()),
            failure_stage: None,
            block_reason: None,
        };
        let diagnostics = GuiDictationDiagnostics {
            session: 17,
            generation: 18,
            capture: Some(CaptureSummary {
                generation: 17,
                signal: CaptureSignal::Signal,
                callbacks: Some(2),
                observed_frames: Some(320),
            }),
            timings: DictationTimings {
                capture_first_frame_ms: Some(11),
                preparation_ms: None,
                transcription_ms: Some(50),
                delivery_ms: Some(90),
            },
        };
        let original = "隔离合成正文\nSynthetic original".repeat(90);
        let activity = Activity {
            title: None,
            recording: false,
            can_toggle: true,
            can_cancel: false,
            can_retry: false,
        };
        let refresh = |generation, final_text| {
            page.refresh(Snapshot {
                receipt: Some(&receipt),
                diagnostics: Some(&diagnostics),
                origin: Some(TranscriptOrigin {
                    generation,
                    final_text,
                }),
                original: &original,
                raw_available: true,
                can_open_raw: true,
                shortcut: "Fixture shortcut · 按住说话，松开转写",
                activity,
            })
        };
        refresh(18, true);
        assert_eq!(page.result.string().to_string(), original);
        assert!(page.result.isSelectable() && !page.result.isEditable());
        assert!(!page.raw_original.isHidden() && page.raw_original.isEnabled());
        assert_eq!(page.copy.title().to_string(), "复制正文");
        assert_eq!(
            page.result.accessibilityLabel().unwrap().to_string(),
            "本轮转写正文（只读）"
        );
        page.result.setSelectedRange(NSRange::new(0, 3));
        refresh(18, true);
        assert_eq!(
            NSTextInputClient::selectedRange(&*page.result),
            NSRange::new(0, 3)
        );
        assert!(!page.disclosure.isAccessibilityExpanded());
        page.toggle_details();
        refresh(18, true);
        assert!(page.disclosure.isAccessibilityExpanded());
        assert_eq!(
            page.diagnostic.accessibilityLabel().unwrap().to_string(),
            "本轮诊断详情（只读）"
        );
        let detail = page.diagnostic.string().to_string();
        assert!(detail.contains("已提交，接收未验证") && !detail.contains("原输入框已确认"));
        assert!(detail.contains("采集尝试 17") && detail.contains("转写：50 ms"));
        page.toggle_edit();
        refresh(18, true);
        assert!(page.result.isEditable());
        assert_eq!(
            page.result.accessibilityLabel().unwrap().to_string(),
            "编辑副本（可编辑）"
        );
        assert_eq!(
            NSAccessibility::accessibilityLabel(&*page.copy)
                .unwrap()
                .to_string(),
            "复制副本"
        );
        page.result
            .setString(&NSString::from_str("Synthetic edited copy"));
        refresh(18, true);
        let focus_is = |target: &NSResponder| {
            window
                .firstResponder()
                .as_deref()
                .is_some_and(|r| std::ptr::eq(r, target))
        };
        assert!(window.makeFirstResponder(Some(&page.result)) && focus_is(&page.result));
        unsafe {
            let _: () = msg_send![&*page.result, insertTab: None::<&objc2::runtime::AnyObject>];
        }
        assert!(focus_is(&page.diagnostic));
        unsafe {
            let _: () =
                msg_send![&*page.diagnostic, insertBacktab: None::<&objc2::runtime::AnyObject>];
        }
        assert!(focus_is(&page.result));
        assert_eq!(page.result.string().to_string(), "Synthetic edited copy");
        page.copy(|copied| {
            assert_eq!(copied, "Synthetic edited copy");
            true
        });
        page.toggle_edit();
        refresh(18, true);
        page.copy(|copied| {
            assert_eq!(copied, original);
            true
        });
        page.toggle_edit();
        page.result
            .setString(&NSString::from_str("not the next experiment"));
        refresh(19, true); // same bytes, different attempt
        assert_eq!(page.result.string().to_string(), original);
        assert!(page.raw_original.isHidden() && !page.raw_original.isEnabled());
        assert_eq!(
            page.result.accessibilityLabel().unwrap().to_string(),
            "其他尝试的保留正文（只读）"
        );
        page.toggle_details();
        assert!(!page.disclosure.isAccessibilityExpanded());
        refresh(19, false);
        assert!(!page.edit.isEnabled() && !page.copy.isEnabled());
        page.copy(|_| panic!("preview must not enter final-copy action"));

        let srgb = NSColorSpace::sRGBColorSpace();
        let linear = |c: f64| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        for name in unsafe {
            [
                NSAppearanceNameAqua,
                NSAppearanceNameDarkAqua,
                NSAppearanceNameAccessibilityHighContrastAqua,
                NSAppearanceNameAccessibilityHighContrastDarkAqua,
            ]
        } {
            let appearance = NSAppearance::appearanceNamed(name).unwrap();
            view.setAppearance(Some(&appearance));
            let ratio = Cell::new(0.);
            let calc = RcBlock::new(|| {
                let luminance = |color: &NSColor| {
                    let color = color.colorUsingColorSpace(&srgb).unwrap();
                    0.2126 * linear(color.redComponent())
                        + 0.7152 * linear(color.greenComponent())
                        + 0.0722 * linear(color.blueComponent())
                };
                let foreground = luminance(&page.result.textColor().unwrap());
                let background = luminance(&page.result.backgroundColor());
                ratio
                    .set((foreground.max(background) + 0.05) / (foreground.min(background) + 0.05));
            });
            appearance.performAsCurrentDrawingAppearance(&calc);
            assert!(ratio.get() >= 4.5, "{name}: {}", ratio.get());
            println!("{name}: text contrast {:.2}:1", ratio.get());
        }
        assert!(!window.isVisible());
        println!(
            "dictation_page_native: key loop, native Tab/Backtab dispatch, AX state/names, selection, draft, simulated copy and receipt display passed; hidden window"
        );
    });
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("dictation_page_native: macOS-only native regression; not exercised on this platform");
}
