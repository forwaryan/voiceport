//! The existing settings stay in one native scroll view, grouped by task.
use super::*;

const DOCUMENT_HEIGHT: f64 = 702.;
const HEADER_HEIGHT: f64 = 62.;

pub(super) struct InputSlots {
    pub(super) self_check: Retained<NSView>,
    pub(super) input_device: Retained<NSView>,
}

fn anchor(
    parent: &NSView,
    mtm: MainThreadMarker,
    frame: NSRect,
    id: &NSString,
) -> Retained<NSView> {
    let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
    view.setAccessibilityIdentifier(Some(id));
    parent.addSubview(&view);
    view
}

impl Delegate {
    pub(super) fn build_settings_page(&self, page: &NSView) {
        let mtm = self.mtm();
        let bounds = page.bounds();
        let width = bounds.size.width;
        let height = bounds.size.height;
        let title = self.label(page, "偏好设置", rect(0., height - 28., width, 28.), 21.);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(21., unsafe {
            NSFontWeightSemibold
        })));
        title.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        let subtitle = self.caption(
            page,
            "按你的习惯，调整每一次听写。",
            rect(0., height - 51., width, 18.),
        );
        subtitle.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);

        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(0., 0., width, (height - HEADER_HEIGHT).max(1.)),
        );
        scroll.setAccessibilityIdentifier(Some(ns_string!("voiceport.settings.scroll")));
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(false);
        scroll.setAutohidesScrollers(false);
        scroll.setScrollerStyle(NSScrollerStyle::Legacy);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        page.addSubview(&scroll);
        let doc = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., 550., DOCUMENT_HEIGHT));
        scroll.setDocumentView(Some(&doc));
        let clip = scroll.contentView();
        clip.scrollToPoint(NSPoint::new(
            0.,
            (DOCUMENT_HEIGHT - clip.bounds().size.height).max(0.),
        ));
        scroll.reflectScrolledClipView(&clip);

        let dictation = self.section(
            &doc,
            426.,
            276.,
            "听写与快捷键",
            ns_string!("voiceport.settings.dictation"),
        );
        self.build_dictation_preferences(&dictation);
        let input = self.section(
            &doc,
            216.,
            198.,
            "输入与权限",
            ns_string!("voiceport.settings.input"),
        );
        self.build_input_preferences(&input);
        let source = self.section(
            &doc,
            4.,
            200.,
            "来源与模型",
            ns_string!("voiceport.settings.sources"),
        );
        self.build_source_preferences(&source);
    }

    fn section(
        &self,
        document: &NSView,
        y: f64,
        height: f64,
        title: &str,
        id: &NSString,
    ) -> Retained<NSView> {
        let section = anchor(document, self.mtm(), rect(0., y, 550., height), id);
        self.surface(&section, section.bounds());
        let heading = self.label(&section, title, rect(16., height - 32., 518., 22.), 15.);
        heading.setFont(Some(&NSFont::systemFontOfSize_weight(15., unsafe {
            NSFontWeightSemibold
        })));
        section
    }

    fn build_dictation_preferences(&self, section: &NSView) {
        self.label(section, "保留最近听写", rect(16., 211., 320., 20.), 13.);
        self.caption(
            section,
            "仅在本次运行中保留，关闭时清空。",
            rect(16., 190., 355., 17.),
        );
        let history =
            NSSwitch::initWithFrame(NSSwitch::alloc(self.mtm()), rect(488., 200., 42., 28.));
        unsafe {
            history.setTarget(Some(self));
            history.setAction(Some(sel!(setRecentDictationsPreference:)));
        }
        history.setAccessibilityLabel(Some(ns_string!("保留最近听写")));
        history.setAccessibilityHelp(Some(ns_string!("仅在本次运行中保留，关闭时清空")));
        history
            .setAccessibilityIdentifier(Some(ns_string!("voiceport.settings.recent-dictations")));
        section.addSubview(&history);
        let _ = self.ivars().recent_dictations_switch.set(history);

        self.label(section, "快捷键模式", rect(16., 161., 300., 20.), 13.);
        self.caption(
            section,
            "按住说话仅影响全局快捷键，菜单按钮仍可开始或停止。",
            rect(16., 141., 320., 17.),
        );
        let mode = self.popup(
            section,
            &["按一下切换（默认）", "按住说话"],
            sel!(selectShortcutMode:),
            rect(352., 150., 182., 30.),
        );
        mode.setAccessibilityLabel(Some(ns_string!("快捷键模式")));
        mode.setAccessibilityIdentifier(Some(ns_string!("voiceport.settings.shortcutMode")));
        mode.selectItemAtIndex(self.ivars().shortcut_mode.get().index());
        let _ = self.ivars().shortcut_mode_control.set(mode);

        self.label(section, "听写快捷键", rect(16., 115., 320., 20.), 13.);
        let feedback = self.caption(
            section,
            "选择预设，或点自定义后试按组合键。",
            rect(16., 85., 326., 29.),
        );
        let _ = self.ivars().shortcut_feedback.set(feedback);
        let shortcut = self.popup(
            section,
            &["⌃⌥空格", "⌃⇧空格", "自定义…"],
            sel!(cycleShortcut:),
            rect(352., 100., 182., 30.),
        );
        shortcut.setAccessibilityLabel(Some(ns_string!("听写快捷键")));
        shortcut.setAccessibilityIdentifier(Some(ns_string!("voiceport.settings.shortcut")));
        let _ = self.ivars().shortcut.set(shortcut);

        self.label(section, "识别语言", rect(16., 66., 280., 20.), 13.);
        self.caption(
            section,
            "选择模型或服务支持的语言。",
            rect(16., 50., 320., 16.),
        );
        let language = self.popup(
            section,
            &voiceport_core::language::LANGUAGES
                .iter()
                .map(|(_, name)| *name)
                .collect::<Vec<_>>(),
            sel!(cycleLanguage:),
            rect(382., 56., 152., 30.),
        );
        language.setAccessibilityLabel(Some(ns_string!("识别语言")));
        let _ = self.ivars().language.set(language);
        let _ = self.ivars().dictionary_settings.set(self.button(
            section,
            "个人词典与输出…",
            sel!(showPersonalDictionary:),
            12.,
            8.,
            132.,
        ));
        let _ = self.ivars().preset_settings.set(self.button(
            section,
            "听写场景…",
            sel!(showDictationPresets:),
            152.,
            8.,
            132.,
        ));
        let _ = self.ivars().app_rules_settings.set(self.button(
            section,
            "应用输出规则…",
            sel!(showDictationAppRules:),
            292.,
            8.,
            152.,
        ));
    }

    fn build_input_preferences(&self, section: &NSView) {
        self.label(section, "自动输入到原光标", rect(16., 133., 320., 20.), 13.);
        self.caption(
            section,
            "停止后，将文字送回开始时的输入位置。",
            rect(16., 113., 355., 17.),
        );
        let automatic =
            NSSwitch::initWithFrame(NSSwitch::alloc(self.mtm()), rect(488., 122., 42., 28.));
        unsafe {
            automatic.setTarget(Some(self));
            automatic.setAction(Some(sel!(toggleAutomatic:)));
        }
        automatic.setAccessibilityLabel(Some(ns_string!("自动输入到原光标")));
        section.addSubview(&automatic);
        let _ = self.ivars().automatic.set(automatic);

        self.label(section, "辅助功能", rect(16., 84., 330., 20.), 13.);
        self.caption(
            section,
            "允许 VoicePort 将文字输入其他应用。",
            rect(16., 65., 355., 17.),
        );
        let accessibility = self.button(
            section,
            "前往授权",
            sel!(requestAccessibility:),
            418.,
            75.,
            116.,
        );
        let _ = self.ivars().accessibility.set(accessibility);
        let prepare = self.button(
            section,
            "检查麦克风与语音权限…",
            sel!(preparePermissions:),
            12.,
            20.,
            216.,
        );
        prepare.setAlignment(NSTextAlignment::Left);
        let _ = self.ivars().prepare_permissions.set(prepare);

        // Integration points: each slot accepts its own button/control at local
        // (0, 0), without moving the other rows or adding to the shared shell.
        let slots = InputSlots {
            self_check: anchor(
                section,
                self.mtm(),
                rect(236., 20., 142., 32.),
                ns_string!("voiceport.settings.input.selfCheckSlot"),
            ),
            input_device: anchor(
                section,
                self.mtm(),
                rect(386., 20., 148., 32.),
                ns_string!("voiceport.settings.input.deviceSlot"),
            ),
        };
        self.button(
            &slots.self_check,
            "首次使用与自检…",
            sel!(showFirstRunChecks:),
            0.,
            0.,
            142.,
        )
        .setAccessibilityIdentifier(Some(ns_string!("voiceport.settings.input.selfCheck")));
        self.button(
            &slots.input_device,
            "麦克风设置…",
            sel!(microphoneSettings:),
            0.,
            0.,
            148.,
        )
        .setAccessibilityIdentifier(Some(ns_string!("voiceport.microphone.open")));
        let _ = self.ivars().settings_input_slots.set(slots);
    }

    fn build_source_preferences(&self, section: &NSView) {
        self.label(section, "转写来源", rect(16., 123., 84., 20.), 13.);
        let source = self.popup(
            section,
            &[],
            sel!(selectSource:),
            rect(98., 116., 264., 30.),
        );
        source.setAccessibilityLabel(Some(ns_string!("转写来源")));
        let _ = self.ivars().source.set(source);
        let models = self.button(section, "语音模型…", sel!(manageModels:), 365., 115., 105.);
        models.setAccessibilityIdentifier(Some(ns_string!("voiceport.models.open")));
        let _ = self.ivars().manage_models.set(models);
        let _ = self.ivars().refresh_models.set(self.button(
            section,
            "刷新",
            sel!(refreshModels:),
            472.,
            115.,
            62.,
        ));
        let _ = self.ivars().source_detail.set(self.caption(
            section,
            "正在检查已安装的模型…",
            rect(16., 65., 518., 41.),
        ));

        let _ = self.ivars().manage_sources.set(self.button(
            section,
            "管理来源…",
            sel!(manageSources:),
            16.,
            17.,
            110.,
        ));
        let _ = self.ivars().test_source.set(self.button(
            section,
            "测试各来源连通性与耗时…",
            sel!(testSource:),
            134.,
            17.,
            250.,
        ));
    }
}

/// Run on the main thread with an offscreen view. No controller, application
/// delegate, preferences, registration, or system I/O is initialized here.
pub(super) fn isolated_layout_regression(mtm: MainThreadMarker) {
    use std::collections::BTreeSet;
    fn contains(outer: NSRect, inner: NSRect) -> bool {
        outer.origin.x <= inner.origin.x
            && outer.origin.y <= inner.origin.y
            && outer.max().x >= inner.max().x
            && outer.max().y >= inner.max().y
    }
    fn identifier(view: &NSView) -> String {
        NSAccessibility::accessibilityIdentifier(view)
            .unwrap()
            .to_string()
    }
    for height in [408., 260.] {
        let delegate = Delegate::new(mtm);
        let page = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., 572., height));
        delegate.build_settings_page(&page);
        let scroll = page
            .subviews()
            .lastObject()
            .unwrap()
            .downcast::<NSScrollView>()
            .unwrap();
        let doc = scroll.documentView().unwrap();
        let sections = doc.subviews();
        assert_eq!(sections.len(), 3);
        let clip = scroll.contentView();
        assert!(scroll.hasVerticalScroller() && !scroll.hasHorizontalScroller());
        assert!(clip.bounds().size.width >= doc.frame().size.width);
        assert!(clip.bounds().origin.y > 0.);
        let mut ids = BTreeSet::new();
        for section in sections.iter() {
            let local_bounds = section.bounds();
            assert!(contains(doc.bounds(), section.frame()));
            ids.insert(identifier(&section));
            for child in section.subviews().iter() {
                assert!(
                    contains(local_bounds, child.frame()),
                    "child {:?} outside {:?}",
                    child.frame(),
                    local_bounds
                );
            }
        }
        assert_eq!(
            ids,
            [
                "voiceport.settings.dictation",
                "voiceport.settings.input",
                "voiceport.settings.sources"
            ]
            .map(str::to_string)
            .into_iter()
            .collect()
        );
        assert!(
            sections.objectAtIndex(0).frame().origin.y >= sections.objectAtIndex(1).frame().max().y
        );
        assert!(
            sections.objectAtIndex(1).frame().origin.y >= sections.objectAtIndex(2).frame().max().y
        );
        assert_eq!(clip.bounds().max().y, DOCUMENT_HEIGHT);
        clip.scrollToPoint(NSPoint::ZERO);
        assert_eq!(clip.bounds().origin.y, 0.);
        let slots = delegate.ivars().settings_input_slots.get().unwrap();
        assert!(slots.self_check.frame().max().x < slots.input_device.frame().origin.x);
        assert_eq!(
            identifier(&slots.self_check),
            "voiceport.settings.input.selfCheckSlot"
        );
        assert_eq!(
            identifier(&slots.input_device),
            "voiceport.settings.input.deviceSlot"
        );
        for (slot, title, id, action) in [
            (
                &slots.self_check,
                "首次使用与自检…",
                "voiceport.settings.input.selfCheck",
                sel!(showFirstRunChecks:),
            ),
            (
                &slots.input_device,
                "麦克风设置…",
                "voiceport.microphone.open",
                sel!(microphoneSettings:),
            ),
        ] {
            assert_eq!(slot.subviews().len(), 1);
            let button = slot
                .subviews()
                .objectAtIndex(0)
                .downcast::<NSButton>()
                .unwrap();
            assert_eq!(button.title().to_string(), title);
            assert_eq!(identifier(&button), id);
            assert_eq!(button.frame(), slot.bounds());
            assert_eq!(button.action(), Some(action));
            assert!(delegate.respondsToSelector(action));
            assert!(
                button
                    .target()
                    .as_deref()
                    .is_some_and(|target| std::ptr::eq(target, &***delegate))
            );
        }
        assert!(delegate.ivars().dictionary_settings.get().is_some());
        assert!(delegate.ivars().preset_settings.get().is_some());
        assert!(delegate.ivars().app_rules_settings.get().is_some());
        assert!(delegate.ivars().manage_sources.get().is_some());
        assert!(delegate.ivars().test_source.get().is_some());
    }
    println!(
        "preferences_layout: native groups, controls, integration slots and scroll bounds passed at heights 408 and 260; no window shown"
    );
}
