//! Real AppKit views on the OS main thread; no window is ordered front, and copy
//! goes to a test-owned closure rather than any system pasteboard.
#[cfg(target_os = "macos")]
#[path = "../src/app/dictation_page.rs"]
mod dictation_page;

#[cfg(target_os = "macos")]
fn main() {
    use block2::RcBlock;
    use dictation_page::{Activity, Page, ResultDetails, Snapshot, TranscriptOrigin};
    use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
    use objc2_app_kit::*;
    use objc2_foundation::{NSObject, NSPoint, NSRange, NSRect, NSSize, NSString};
    use std::cell::Cell;
    use voiceport_control::{
        CaptureSignal, CaptureSummary, DeliveryReceipt, DeliveryState, DictationBlockReason,
        DictationTimings, GuiDictationDiagnostics, GuiDictationReceipt, TranscriptionState,
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
        assert!(std::ptr::eq(page.view(), &*view));
        let start = NSButton::initWithFrame(NSButton::alloc(mtm), NSRect::ZERO);
        view.addSubview(&start);
        let start: &NSView = &start;
        let keys = page.key_views();
        for (from, to) in std::iter::once(start)
            .chain(keys)
            .zip(keys.into_iter().chain(std::iter::once(start)))
        {
            unsafe { from.setNextKeyView(Some(to)) };
        }
        assert!(std::ptr::eq(keys[0], &***page.disclosure));
        assert!(std::ptr::eq(keys[4], &***page.copy));
        assert!(std::ptr::eq(keys[5], &***page.result));
        assert_eq!(
            page.disclosure.action(),
            Some(sel!(toggleDictationDiagnostics:))
        );
        assert_eq!(page.edit.action(), Some(sel!(editDictationCopy:)));
        assert_eq!(page.copy.action(), Some(sel!(copyDictationResult:)));

        let receipt = GuiDictationReceipt {
            session: 18,
            generation: 18,
            automatic: true,
            transcription: TranscriptionState::Completed,
            delivery: DeliveryReceipt {
                state: DeliveryState::Confirmed,
                submitted_operations: 1,
                confirmed_operations: 1,
                ..Default::default()
            },
            method: Some("accessibility".into()),
            failure_stage: None,
            block_reason: None,
        };
        let diagnostics = GuiDictationDiagnostics {
            session: 18,
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
        let attempt = ResultDetails {
            receipt: &receipt,
            diagnostics: Some(&diagnostics),
            source: Some("合成来源 · 简体中文 · 在本机处理"),
            target: Some("原目标夹具"),
        };
        let refresh = |generation, final_text| {
            page.refresh(Snapshot {
                attempt: Some(attempt),
                retained: None,
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
            "投递详情（只读）"
        );
        let detail = page.diagnostic.string().to_string();
        assert!(detail.contains("原位确认：已读回原输入框并确认全部写入"));
        assert!(detail.contains("原目标应用：原目标夹具"));
        assert!(detail.contains("来源设置：合成来源 · 简体中文 · 在本机处理"));
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

        let show = |attempt: Option<ResultDetails<'_>>,
                    retained: Option<ResultDetails<'_>>,
                    generation: Option<u64>,
                    text: &str| {
            page.refresh(Snapshot {
                attempt,
                retained,
                origin: generation.map(|generation| TranscriptOrigin {
                    generation,
                    final_text: true,
                }),
                original: text,
                raw_available: false,
                can_open_raw: false,
                shortcut: "",
                activity,
            });
        };
        show(Some(attempt), None, Some(18_u64), &original);
        page.toggle_details();
        show(Some(attempt), None, Some(18), &original);
        assert!(window.makeFirstResponder(Some(&page.result)));
        let selection = NSRange::new(1, 2);
        page.result.setSelectedRange(selection);
        page.toggle_details();
        assert!(
            focus_is(&page.result),
            "collapse preserves a focused transcript"
        );
        assert_eq!(NSTextInputClient::selectedRange(&*page.result), selection);
        page.toggle_details();
        assert!(window.makeFirstResponder(Some(&page.diagnostic)));
        page.toggle_details();
        assert!(focus_is(&page.disclosure), "hidden details release focus");
        page.toggle_details();

        for (state, confirmed, detail) in [
            (DeliveryState::Submitted, 0, "已提交，接收未验证"),
            (DeliveryState::Uncertain, 0, "投递结果不明"),
            (DeliveryState::PartiallySent, 1, "已执行部分写入"),
        ] {
            let indeterminate = GuiDictationReceipt {
                delivery: DeliveryReceipt {
                    state,
                    submitted_operations: 2,
                    confirmed_operations: confirmed,
                    stopped: state != DeliveryState::Submitted,
                },
                failure_stage: (state != DeliveryState::Submitted).then(|| "focus".to_owned()),
                ..receipt.clone()
            };
            let shown = ResultDetails {
                receipt: &indeterminate,
                ..attempt
            };
            show(Some(shown), None, Some(18), &original);
            assert!(!page.edit.isEnabled() && page.copy.isEnabled());
            let details = page.diagnostic.string().to_string();
            assert!(details.contains(detail));
            assert!(details.contains("原目标应用：原目标夹具"));
            assert!(!details.contains("确认全部写入"));
            if confirmed > 0 {
                assert!(details.contains("整轮未确认"));
            }
            page.copy(|copied| {
                assert_eq!(copied, original);
                true
            });
        }

        let rejected = GuiDictationReceipt {
            generation: 19,
            session: 19,
            transcription: TranscriptionState::NotStarted,
            delivery: DeliveryReceipt::default(),
            block_reason: Some(DictationBlockReason::StartRejected),
            method: None,
            ..receipt.clone()
        };
        let latest = ResultDetails {
            receipt: &rejected,
            diagnostics: None,
            source: Some("新来源夹具"),
            target: Some("切换后的应用夹具"),
        };
        show(Some(latest), Some(attempt), Some(18), &original);
        let old_detail = page.diagnostic.string().to_string();
        assert!(old_detail.contains("原目标应用：原目标夹具"));
        assert!(old_detail.contains("来源设置：合成来源"));
        assert!(old_detail.contains("最近一次尝试：本轮未开始"));
        assert!(!old_detail.contains("切换后的应用夹具") && !old_detail.contains("新来源夹具"));
        page.copy(|copied| {
            assert_eq!(copied, original);
            true
        });
        let retry = GuiDictationReceipt {
            generation: 20,
            block_reason: Some(DictationBlockReason::RetryPanelOnly),
            ..receipt.clone()
        };
        show(
            Some(ResultDetails {
                receipt: &retry,
                ..attempt
            }),
            None,
            Some(20),
            "重试正文",
        );
        let retry_detail = page.diagnostic.string().to_string();
        assert!(retry_detail.contains("原尝试的投递回执"));
        assert!(retry_detail.contains("读回记录属于原尝试"));
        assert!(
            !retry_detail.contains("转写：50 ms"),
            "wrong-generation diagnostics were injected"
        );
        show(None, None, None, "");
        assert!(!page.copy.isEnabled() && !page.edit.isEnabled());
        assert!(!page.disclosure.isAccessibilityExpanded() && !page.disclosure.isEnabled());
        page.copy(|_| panic!("empty result must not enter final-copy action"));

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
