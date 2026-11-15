//! All catalog sources, explicit sample requests, stable row identities.
use super::*;
use crate::local_models::{Entry, SessionSource};
use objc2::Message;

struct Row {
    entry: Entry,
    result: Retained<NSTextField>,
    test: Retained<NSButton>,
}
pub(super) struct PanelState {
    owner: Retained<Delegate>,
    window: OnceCell<Retained<NSWindow>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    all: OnceCell<Retained<NSButton>>,
    cancel: OnceCell<Retained<NSButton>>,
    summary: OnceCell<Retained<NSTextField>>,
    rows: RefCell<Vec<Row>>,
    closed: Cell<bool>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PanelState]
    pub(super) struct Panel;
    unsafe impl NSObjectProtocol for Panel {}
    unsafe impl NSWindowDelegate for Panel {
        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _notification: &NSNotification) {
            let keep_alive = self.retain();
            if keep_alive.ivars().closed.replace(true) { return; }
            let owner = &keep_alive.ivars().owner;
            owner.with_controller(|c| {
                if c.source_tests.busy() { c.cancel(); }
            });
            keep_alive.window().setDelegate(None);
            owner.ivars().source_probe_panel.borrow_mut().take();
            owner.refresh();
        }
    }
    impl Panel {
        #[unsafe(method(testProbeRow:))]
        fn test_row(&self, sender: &NSButton) {
            let id = usize::try_from(sender.tag()).ok()
                .and_then(|index| self.ivars().rows.borrow().get(index).map(|row| row.entry.id.clone()));
            if let Some(id) = id { self.start(vec![id]); }
        }
        #[unsafe(method(testAllProbes:))]
        fn test_all(&self, _sender: Option<&AnyObject>) {
            let ids = self.ivars().rows.borrow().iter().map(|row| row.entry.id.clone()).collect();
            self.start(ids);
        }
        #[unsafe(method(cancelProbes:))]
        fn cancel(&self, _sender: Option<&AnyObject>) {
            self.ivars().owner.with_controller(|c| {
                if c.source_tests.busy() { c.cancel(); }
            });
            self.refresh();
        }
    }
);
impl Panel {
    fn window(&self) -> &NSWindow {
        self.ivars().window.get().unwrap()
    }
    pub(super) fn bring_forward(&self) {
        self.window().makeKeyAndOrderFront(None);
    }
    pub(super) fn close(&self) {
        self.window().performClose(None);
    }
    fn ready(&self) -> bool {
        let owner = &self.ivars().owner;
        !self.ivars().closed.get()
            && !owner.ivars().quitting.get()
            && !owner.settings_locked()
            && owner.ivars().permission_rx.borrow().is_none()
            && owner.ivars().pending_hotkey.borrow().is_none()
            && !owner.ivars().control.borrow().preparing()
            && owner
                .ivars()
                .controller
                .borrow()
                .as_ref()
                .is_some_and(|c| !c.busy() && !c.discovering())
    }
    fn start(&self, ids: Vec<String>) {
        if !self.ready() {
            return;
        }
        self.ivars().owner.with_controller(|c| {
            if let Err(error) = c.start_source_tests(&ids) {
                c.message = error;
            }
        });
        self.refresh();
        self.ivars().owner.refresh();
    }
    fn button(
        &self,
        view: &NSView,
        title: &str,
        action: objc2::runtime::Sel,
        frame: NSRect,
    ) -> Retained<NSButton> {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        };
        button.setFrame(frame);
        view.addSubview(&button);
        button
    }
    pub(super) fn refresh(&self) {
        if self.ivars().closed.get() {
            return;
        }
        let owner = &self.ivars().owner;
        let ready = self.ready();
        let slot = owner.ivars().controller.borrow();
        let Some(c) = slot.as_ref() else {
            return;
        };
        let changed = {
            let rows = self.ivars().rows.borrow();
            rows.len() != c.catalog.entries.len()
                || rows
                    .iter()
                    .zip(&c.catalog.entries)
                    .any(|(row, entry)| row.entry != *entry)
        };
        if changed {
            let scroll = self.ivars().scroll.get().unwrap();
            let width = scroll.contentSize().width.max(400.);
            let height = (c.catalog.entries.len() as f64 * 82.).max(scroll.contentSize().height);
            let view =
                NSView::initWithFrame(NSView::alloc(self.mtm()), rect(0., 0., width, height));
            let mut rows = Vec::new();
            for (index, entry) in c.catalog.entries.iter().enumerate() {
                let y = height - (index + 1) as f64 * 82.;
                let title = owner.label(
                    &view,
                    &entry.title,
                    rect(8., y + 49., width - 116., 21.),
                    13.,
                );
                title.setToolTip(Some(&NSString::from_str(&entry.id)));
                owner.caption(&view, kind(entry), rect(8., y + 30., width - 116., 17.));
                let result = owner.caption(&view, "未测试", rect(8., y + 6., width - 116., 24.));
                let test = self.button(
                    &view,
                    "测试",
                    sel!(testProbeRow:),
                    rect(width - 96., y + 28., 82., 30.),
                );
                test.setTag(index as isize);
                test.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
                    "voiceport.source-test.{}",
                    entry.id
                ))));
                test.setAccessibilityLabel(Some(&NSString::from_str(&format!(
                    "使用内置样例测试{}",
                    entry.title
                ))));
                rows.push(Row {
                    entry: entry.clone(),
                    result,
                    test,
                });
            }
            *self.ivars().rows.borrow_mut() = rows;
            scroll.setDocumentView(Some(&view));
            // Rebuild only when catalog identities/configuration actually change.
            view.scrollPoint(NSPoint::new(0., height));
        }
        for row in self.ivars().rows.borrow().iter() {
            let result = c.source_tests.row(&row.entry);
            let (text, detail) = result
                .map(|result| (result.display(), result.detail.clone()))
                .unwrap_or_else(|| ("未测试 · 耗时 —".into(), row.entry.detail.clone()));
            use crate::source_tests::Status;
            let color = match result.map(|result| result.status) {
                Some(Status::Success) => NSColor::systemGreenColor(),
                Some(Status::Failed) => NSColor::systemRedColor(),
                Some(Status::TimedOut | Status::CleanupPending) => NSColor::systemOrangeColor(),
                _ => NSColor::secondaryLabelColor(),
            };
            row.result.setTextColor(Some(&color));
            if row.result.stringValue().to_string() != text {
                row.result.setStringValue(&NSString::from_str(&text));
            }
            row.result.setToolTip(Some(&NSString::from_str(&detail)));
            row.test.setEnabled(ready);
        }
        self.ivars()
            .all
            .get()
            .unwrap()
            .setEnabled(ready && !c.catalog.entries.is_empty());
        self.ivars()
            .cancel
            .get()
            .unwrap()
            .setEnabled(c.source_tests.busy());
        let summary = if c.discovering() {
            "正在刷新来源…".into()
        } else {
            c.source_tests.summary()
        };
        self.ivars()
            .summary
            .get()
            .unwrap()
            .setStringValue(&NSString::from_str(&summary));
    }
}
fn kind(entry: &Entry) -> &'static str {
    match &entry.source {
        Ok(SessionSource::ChatGpt) => "云端 · ChatGPT 转写请求",
        Ok(SessionSource::Local(_)) => "本机 · 模型加载与解码",
        Ok(SessionSource::Plugin(plugin))
            if plugin.manifest.adapter == crate::plugins::Adapter::Http =>
        {
            "云端 / HTTP · 转写请求"
        }
        Ok(SessionSource::Plugin(_)) => "本机适配器 · 完整转写请求（可能联网）",
        Err(_) => "未就绪 · 检查来源配置或安装状态",
    }
}
pub(super) fn show(owner: &Delegate) {
    let existing = owner.ivars().source_probe_panel.borrow().clone();
    if let Some(panel) = existing {
        panel.bring_forward();
        return;
    }
    let panel = Panel::alloc(owner.mtm()).set_ivars(PanelState {
        owner: owner.retain(),
        window: OnceCell::new(),
        scroll: OnceCell::new(),
        all: OnceCell::new(),
        cancel: OnceCell::new(),
        summary: OnceCell::new(),
        rows: RefCell::new(Vec::new()),
        closed: Cell::new(false),
    });
    let panel: Retained<Panel> = unsafe { msg_send![super(panel), init] };
    let screen = NSScreen::mainScreen(owner.mtm()).map(|s| s.visibleFrame().size);
    let width = screen.map_or(700., |s| 700f64.min((s.width - 32.).max(400.)));
    let height = screen.map_or(480., |s| 480f64.min((s.height - 72.).max(300.)));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(owner.mtm()),
            rect(0., 0., width, height),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(ns_string!("转写来源测试"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*panel)));
    let view = window.contentView().unwrap();
    let all = panel.button(
        &view,
        "测试全部来源",
        sel!(testAllProbes:),
        rect(16., height - 48., 128., 30.),
    );
    all.setAccessibilityIdentifier(Some(ns_string!("voiceport.source-tests.all")));
    let cancel = panel.button(
        &view,
        "取消 / 检查清理",
        sel!(cancelProbes:),
        rect(152., height - 48., 144., 30.),
    );
    cancel.setAccessibilityIdentifier(Some(ns_string!("voiceport.source-tests.cancel")));
    let summary = owner.caption(
        &view,
        "未开始测试",
        rect(16., height - 78., width - 32., 24.),
    );
    let scroll = NSScrollView::initWithFrame(
        NSScrollView::alloc(owner.mtm()),
        rect(16., 78., width - 32., height - 164.),
    );
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    view.addSubview(&scroll);
    owner.caption(&view, "点击测试会发送内置 1 秒静音样例；本地模型在本机解码，云端或适配器可能产生用量。不会录音、切换来源或输入文字。", rect(16., 36., width - 32., 35.));
    owner.caption(&view, "显示样例总耗时（含认证、连接或模型加载），不是网络 ping。30 秒后请求取消；静音样例不验证识别质量。", rect(16., 8., width - 32., 29.));
    panel.ivars().all.set(all).ok();
    panel.ivars().cancel.set(cancel).ok();
    panel.ivars().summary.set(summary).ok();
    panel.ivars().scroll.set(scroll).ok();
    panel.ivars().window.set(window).ok();
    *owner.ivars().source_probe_panel.borrow_mut() = Some(panel.clone());
    panel.refresh();
    panel.window().center();
    panel.bring_forward();
}
