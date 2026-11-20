//! Native model catalog presentation; installation and source ownership stay in Manager.
use super::*;
use crate::{local_models::ModelKind, model_library::Progress};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ProgressDisplay {
    Hidden,
    Indeterminate(String),
    Transfer {
        message: String,
        completed: u64,
        total: u64,
    },
}

impl ProgressDisplay {
    pub(super) fn from_job(progress: Option<Progress>, cancelling: bool) -> Self {
        if cancelling {
            return Self::Indeterminate("正在取消并清理临时文件…".into());
        }
        match progress {
            None => Self::Hidden,
            Some(
                Progress::Stage(message)
                | Progress::Transfer {
                    message, total: 0, ..
                },
            ) => Self::Indeterminate(message),
            Some(Progress::Transfer {
                message,
                completed,
                total,
            }) => Self::Transfer {
                message,
                completed,
                total,
            },
        }
    }

    fn animation_change(&self, previous: &Self) -> Option<bool> {
        let indeterminate = matches!(self, Self::Indeterminate(_));
        let was_indeterminate = matches!(previous, Self::Indeterminate(_));
        (indeterminate != was_indeterminate).then_some(indeterminate)
    }
}

pub(super) fn render_progress(manager: &Manager, display: ProgressDisplay) {
    let mut previous = manager.ivars().progress_display.borrow_mut();
    if *previous == display {
        return;
    }
    let indicator = manager.ivars().progress.get().unwrap();
    let animation_change = display.animation_change(&previous);
    if animation_change == Some(false) {
        unsafe { indicator.stopAnimation(None) };
        indicator.setIndeterminate(false);
    }
    match &display {
        ProgressDisplay::Hidden => indicator.setHidden(true),
        ProgressDisplay::Indeterminate(message) => {
            manager.notice(message);
            if animation_change == Some(true) {
                indicator.setIndeterminate(true);
                indicator.setHidden(false);
                unsafe { indicator.startAnimation(None) };
            }
        }
        ProgressDisplay::Transfer {
            message,
            completed,
            total,
        } => {
            let percentage = (*completed as f64 * 100. / *total as f64).clamp(0., 100.);
            indicator.setDoubleValue(percentage);
            indicator.setHidden(false);
            manager.notice(&format!(
                "{message} · {percentage:.0}% · {:.1} / {:.1} MB",
                *completed as f64 / 1_000_000.,
                *total as f64 / 1_000_000.,
            ));
        }
    }
    *previous = display;
}

pub(super) const ROW_HEIGHT: f64 = 104.;

pub(super) fn list_frame(width: f64, height: f64) -> NSRect {
    rect(20., 76., width - 40., height - 246.)
}

pub(super) fn surface(owner: &Delegate, view: &NSView, frame: NSRect) -> Retained<NSBox> {
    let panel = NSBox::initWithFrame(NSBox::alloc(owner.mtm()), frame);
    panel.setBoxType(NSBoxType::Custom);
    panel.setTitlePosition(NSTitlePosition::NoTitle);
    panel.setCornerRadius(9.);
    panel.setBorderWidth(0.5);
    panel.setBorderColor(&NSColor::separatorColor());
    panel.setFillColor(&NSColor::controlBackgroundColor());
    view.addSubview(&panel);
    panel
}

fn divider(owner: &Delegate, view: &NSView, frame: NSRect) {
    let line = surface(owner, view, frame);
    line.setCornerRadius(0.);
    line.setBorderWidth(0.);
    line.setFillColor(&NSColor::separatorColor());
}

fn description(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::SenseVoice => "自动识别中日韩英粤，适合日常听写。",
        ModelKind::WhisperTurbo => "覆盖多种语言；运行时需要更多内存。",
        ModelKind::Whisper => "体积小，适合先体验本地识别。",
        ModelKind::Paraformer => "中英双语，边说边显示文字。",
        ModelKind::ParakeetTdtV3 => "欧洲多语言识别，录音结束后转写。",
        ModelKind::ParakeetTdtV2 => "英语识别，录音结束后转写。",
        ModelKind::ParakeetUnified => "英语流式识别，边说边显示文字。",
        ModelKind::Nemotron => "多语言流式识别，边说边显示文字。",
    }
}

fn languages(d: &Definition) -> String {
    match d.kind {
        ModelKind::SenseVoice => "5 种语言".into(),
        ModelKind::Paraformer => "中文 / 英语".into(),
        ModelKind::ParakeetTdtV3 => format!(
            "{} 种语言",
            crate::local_models::PARAKEET_V3_LANGUAGES.len()
        ),
        ModelKind::Nemotron => format!("{} 种语言", crate::local_models::NEMOTRON_LANGUAGES.len()),
        _ => d
            .languages
            .split('·')
            .next()
            .unwrap_or(d.languages)
            .trim()
            .into(),
    }
}

fn row(
    manager: &Manager,
    owner: &Delegate,
    list: &NSView,
    d: &'static Definition,
    index: usize,
    frame: NSRect,
) -> Row {
    let width = frame.size.width;
    let view = NSView::initWithFrame(NSView::alloc(owner.mtm()), frame);
    list.addSubview(&view);
    let panel = surface(owner, &view, rect(2., 4., width - 4., ROW_HEIGHT - 8.));
    let tile = surface(owner, &view, rect(18., 48., 36., 36.));
    tile.setBorderWidth(0.);
    tile.setFillColor(&NSColor::controlAccentColor().colorWithAlphaComponent(0.08));
    let icon = owner.icon(
        &view,
        if d.kind.streaming() {
            "waveform"
        } else {
            "cpu"
        },
        rect(25., 55., 22., 22.),
    );
    icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
    let name = owner.label(&view, d.name, rect(60., 68., width - 252., 23.), 15.);
    name.setFont(Some(&NSFont::systemFontOfSize_weight(15., unsafe {
        NSFontWeightMedium
    })));
    name.setToolTip(Some(&NSString::from_str(&format!(
        "{}\n{}",
        d.name, d.description
    ))));
    owner.caption(
        &view,
        description(d.kind),
        rect(60., 43., width - 244., 21.),
    );
    let metadata = owner.caption(
        &view,
        &format!(
            "{} · {} · {}",
            d.recommendation,
            languages(d),
            model_size(d.bytes())
        ),
        rect(18., 17., width - 246., 20.),
    );
    metadata.setToolTip(Some(&NSString::from_str(&format!(
        "{}\n{}\n模型大小：{}",
        d.description,
        d.languages,
        model_size(d.bytes())
    ))));
    let status = owner.caption(&view, "正在检查…", rect(width - 182., 69., 108., 21.));
    status.setAlignment(NSTextAlignment::Right);
    let compatibility = owner.caption(&view, "", rect(width - 178., 44., 160., 19.));
    compatibility.setAlignment(NSTextAlignment::Right);
    let details = manager.button(
        &view,
        "详情",
        sel!(modelDetails:),
        rect(width - 66., 65., 48., 26.),
    );
    details.setBordered(false);
    let update = manager.button(
        &view,
        "更新",
        sel!(modelUpdate:),
        rect(width - 222., 14., 58., 28.),
    );
    let import = manager.button(
        &view,
        "导入…",
        sel!(modelImport:),
        rect(width - 160., 14., 74., 28.),
    );
    let remove = manager.button(
        &view,
        "删除",
        sel!(modelRemove:),
        rect(width - 160., 14., 74., 28.),
    );
    let primary = manager.button(
        &view,
        "下载",
        sel!(modelPrimary:),
        rect(width - 82., 14., 76., 28.),
    );
    primary.setBezelColor(Some(&NSColor::controlAccentColor()));
    for (button, action, label) in [
        (&primary, "primary", "下载或使用"),
        (&import, "import", "导入模型文件夹"),
        (&update, "update", "更新或重新安装"),
        (&details, "details", "查看固定版本、文件与许可"),
        (&remove, "remove", "删除 VoicePort 副本"),
    ] {
        button.setFont(Some(&NSFont::systemFontOfSize(12.)));
        button.setTag(index as isize);
        button.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
            "voiceport.model.{}.{action}",
            d.directory
        ))));
        button.setAccessibilityLabel(Some(&NSString::from_str(&format!("{}：{label}", d.name))));
    }
    Row {
        index,
        definition: d,
        view,
        surface: panel,
        status,
        compatibility,
        primary,
        import,
        update,
        details,
        remove,
    }
}

pub(super) fn build(manager: &Manager, owner: &Delegate) {
    let size = source_manager::content_size(owner, NSSize::new(620., 520.));
    let (width, available_height) = (size.width, size.height);
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(owner.mtm()),
            rect(0., 0., width, available_height),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(ns_string!("语音模型"));
    window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
    window.setDelegate(Some(ProtocolObject::from_ref(manager)));
    let content = window.contentView().unwrap();
    let height = page_height(available_height);
    let view = if available_height < height {
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(owner.mtm()),
            rect(0., 0., width, available_height),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        content.addSubview(&scroll);
        let canvas = NSView::initWithFrame(NSView::alloc(owner.mtm()), rect(0., 0., width, height));
        scroll.setDocumentView(Some(&canvas));
        canvas.scrollPoint(NSPoint::new(0., height));
        canvas
    } else {
        content
    };

    let icon = owner.icon(&view, "waveform", rect(24., height - 47., 24., 24.));
    icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
    let title = owner.label(
        &view,
        "语音模型",
        rect(60., height - 48., width - 290., 28.),
        20.,
    );
    title.setFont(Some(&NSFont::systemFontOfSize_weight(20., unsafe {
        NSFontWeightSemibold
    })));
    let caption = owner.label(
        &view,
        "本机识别，音频留在这台电脑。",
        rect(24., height - 76., width - 48., 23.),
        12.,
    );
    caption.setTextColor(Some(&NSColor::secondaryLabelColor()));
    manager.ivars().caption.set(caption).ok();
    let page_tabs = NSSegmentedControl::initWithFrame(
        NSSegmentedControl::alloc(owner.mtm()),
        rect(width - 218., height - 48., 194., 28.),
    );
    page_tabs.setSegmentCount(2);
    page_tabs.setLabel_forSegment(ns_string!("这台电脑"), 0);
    page_tabs.setLabel_forSegment(ns_string!("云端"), 1);
    page_tabs.setSelectedSegment(0);
    page_tabs.setAccessibilityLabel(Some(ns_string!("模型位置")));
    page_tabs.setAccessibilityIdentifier(Some(ns_string!("voiceport.models.location")));
    unsafe {
        page_tabs.setTarget(Some(manager));
        page_tabs.setAction(Some(sel!(modelPage:)));
    }
    view.addSubview(&page_tabs);
    manager.ivars().page_tabs.set(page_tabs).ok();
    owner.icon(
        &view,
        "arrow.triangle.branch",
        rect(24., height - 105., 15., 15.),
    );
    let current = owner.label(
        &view,
        "正在检查当前来源…",
        rect(46., height - 108., width - 248., 23.),
        12.,
    );
    let engine = manager.button(
        &view,
        "安装识别引擎",
        sel!(modelEngine:),
        rect(width - 178., height - 110., 154., 26.),
    );
    engine.setBordered(false);
    engine.setFont(Some(&NSFont::systemFontOfSize(11.)));
    engine.setAccessibilityIdentifier(Some(ns_string!("voiceport.models.engine")));
    engine.setToolTip(Some(&NSString::from_str(&format!(
        "模型下载时也会自动安装共享引擎（{}）。",
        model_size(crate::model_library::runtime_bytes())
    ))));
    manager.ivars().engine.set(engine).ok();
    divider(owner, &view, rect(24., height - 120., width - 48., 0.5));

    let tabs = NSSegmentedControl::initWithFrame(
        NSSegmentedControl::alloc(owner.mtm()),
        rect(24., height - 158., 172., 28.),
    );
    tabs.setSegmentCount(2);
    tabs.setLabel_forSegment(ns_string!("全部模型"), 0);
    tabs.setLabel_forSegment(ns_string!("已安装"), 1);
    tabs.setSelectedSegment(0);
    tabs.setAccessibilityLabel(Some(ns_string!("模型列表筛选")));
    tabs.setAccessibilityIdentifier(Some(ns_string!("voiceport.models.filter")));
    unsafe {
        tabs.setTarget(Some(manager));
        tabs.setAction(Some(sel!(modelFilter:)));
    }
    view.addSubview(&tabs);
    let summary = owner.caption(&view, "", rect(206., height - 153., width - 456., 22.));
    manager.ivars().summary.set(summary).ok();
    owner.caption(
        &view,
        "识别语言",
        rect(width - 250., height - 152., 58., 22.),
    );
    let language = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(owner.mtm()),
        rect(width - 190., height - 158., 166., 28.),
        false,
    );
    for (_, title) in voiceport_core::language::LANGUAGES {
        language.addItemWithTitle(&NSString::from_str(title));
    }
    unsafe {
        language.setTarget(Some(manager));
        language.setAction(Some(sel!(modelLanguage:)));
    }
    language.setFont(Some(&NSFont::systemFontOfSize(12.)));
    language.setAccessibilityLabel(Some(ns_string!("模型识别语言")));
    language.setAccessibilityIdentifier(Some(ns_string!("voiceport.models.language")));
    view.addSubview(&language);

    let frame = list_frame(width, height);
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(owner.mtm()), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    view.addSubview(&scroll);
    let list_height = (MODELS.len() as f64 * ROW_HEIGHT).max(scroll.contentSize().height);
    let list = NSView::initWithFrame(
        NSView::alloc(owner.mtm()),
        rect(0., 0., scroll.contentSize().width, list_height),
    );
    // Attaching the tall document may reveal a legacy scroller and narrow its clip view.
    // Size rows only after AppKit has reserved that gutter.
    scroll.setDocumentView(Some(&list));
    let row_width = scroll.contentSize().width - 12.; // Keep overlay scrollers off the actions.
    list.setFrameSize(NSSize::new(row_width, list_height));
    let mut definitions: Vec<_> = MODELS.iter().enumerate().collect();
    definitions.sort_by_key(|(_, d)| match d.id {
        crate::local_models::SENSEVOICE => 0,
        crate::local_models::WHISPER_TURBO => 1,
        crate::local_models::PARAFORMER => 2,
        _ => 3,
    });
    let rows = definitions
        .into_iter()
        .enumerate()
        .map(|(position, (index, d))| {
            row(
                manager,
                owner,
                &list,
                d,
                index,
                rect(
                    0.,
                    list_height - (position + 1) as f64 * ROW_HEIGHT,
                    row_width,
                    ROW_HEIGHT,
                ),
            )
        })
        .collect();
    let empty = owner.label(
        &list,
        "还没有安装模型\n切换到「全部模型」，选择下载或导入。",
        rect(28., list_height - 104., row_width - 56., 66.),
        14.,
    );
    empty.setHidden(true);
    list.scrollPoint(NSPoint::new(0., list_height));
    let cloud_scroll = NSScrollView::initWithFrame(NSScrollView::alloc(owner.mtm()), frame);
    cloud_scroll.setHasVerticalScroller(true);
    cloud_scroll.setAutohidesScrollers(true);
    cloud_scroll.setDrawsBackground(false);
    cloud_scroll.setHidden(true);
    view.addSubview(&cloud_scroll);
    manager.ivars().cloud.scroll.set(cloud_scroll).ok();

    divider(owner, &view, rect(28., 68., width - 56., 0.5));
    let progress = NSProgressIndicator::initWithFrame(
        NSProgressIndicator::alloc(owner.mtm()),
        rect(28., 64., width - 56., 12.),
    );
    progress.setStyle(NSProgressIndicatorStyle::Bar);
    progress.setControlSize(NSControlSize::Small);
    progress.setIndeterminate(false);
    progress.setMinValue(0.);
    progress.setMaxValue(100.);
    progress.setHidden(true);
    progress.setAccessibilityLabel(Some(ns_string!("模型安装进度")));
    view.addSubview(&progress);
    let notice = owner.caption(
        &view,
        "安装后点击「使用」切换识别来源。",
        rect(28., 34., width - 174., 23.),
    );
    let cancel = manager.button(
        &view,
        "取消任务",
        sel!(modelCancel:),
        rect(width - 124., 29., 96., 28.),
    );
    let footer = owner.caption(
        &view,
        "模型来源与许可保存在详情中；更新失败会保留旧副本。",
        rect(28., 12., width - 56., 19.),
    );
    footer.setFont(Some(&NSFont::systemFontOfSize(10.)));
    manager.ivars().footer.set(footer).ok();
    manager.ivars().tabs.set(tabs).ok();
    manager.ivars().scroll.set(scroll).ok();
    manager.ivars().list.set(list).ok();
    manager.ivars().empty.set(empty).ok();
    manager.ivars().current.set(current).ok();
    manager.ivars().progress.set(progress).ok();
    manager.ivars().rows.replace(rows);
    manager.ivars().notice.set(notice).ok();
    manager.ivars().cancel.set(cancel).ok();
    manager.ivars().language.set(language).ok();
    manager.ivars().window.set(window).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_modes_follow_bytes_stages_cancellation_and_exit() {
        let stage = |message: &str| Some(Progress::Stage(message.into()));
        let transfer = |message: &str, completed, total| {
            Some(Progress::Transfer {
                message: message.into(),
                completed,
                total,
            })
        };
        let render = ProgressDisplay::from_job;
        for message in [
            "正在准备识别引擎…",
            "正在解包并校验识别引擎…",
            "正在发布模型副本…",
        ] {
            assert_eq!(
                render(stage(message), false),
                ProgressDisplay::Indeterminate(message.into())
            );
        }
        assert_eq!(
            render(transfer("未知", 3, 0), false),
            ProgressDisplay::Indeterminate("未知".into())
        );
        assert_eq!(
            render(transfer("正在下载识别引擎", 6, 6), false),
            ProgressDisplay::Transfer {
                message: "正在下载识别引擎".into(),
                completed: 6,
                total: 6,
            }
        );
        assert_eq!(
            render(transfer("正在下载模型", 2, 8), false),
            ProgressDisplay::Transfer {
                message: "正在下载模型".into(),
                completed: 2,
                total: 8,
            }
        );
        assert_eq!(
            render(transfer("正在下载模型", 2, 8), true),
            ProgressDisplay::Indeterminate("正在取消并清理临时文件…".into())
        );
        assert_eq!(render(None, false), ProgressDisplay::Hidden);
        let mut last = ProgressDisplay::Hidden;
        for (next, animation) in [
            (render(stage("准备引擎"), false), Some(true)),
            (render(transfer("传输引擎", 6, 6), false), Some(false)),
            (render(stage("解包引擎"), false), Some(true)),
            (render(stage("准备模型"), false), None),
            (render(transfer("传输模型", 2, 8), false), Some(false)),
            (render(None, true), Some(true)),
            (render(None, false), Some(false)),
        ] {
            assert_eq!(next.animation_change(&last), animation);
            last = next;
        }
    }
}
