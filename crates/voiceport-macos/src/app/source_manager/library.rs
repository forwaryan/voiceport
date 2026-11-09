//! Retained ordinary management window. Only its owned sheets block this window.
use super::*;
use block2::RcBlock;
use objc2::Message;
#[cfg(test)]
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone)]
enum Action {
    Add(String),
    Edit(String),
    Remove(String),
    Details(String),
}
/// Keep the selected file's provenance with the parsed contents until review.
struct ImportReview {
    path: PathBuf,
    manifest: Manifest,
}
impl ImportReview {
    fn load(path: PathBuf) -> Result<Self, String> {
        let manifest = crate::plugins::read_manifest(&path)?;
        Ok(Self { path, manifest })
    }
    fn detail(&self) -> Result<String, String> {
        let manifest = &self.manifest;
        let program = manifest.program.as_ref().ok_or("插件没有本地程序。")?;
        Ok(format!(
            "清单文件：{}\n{} · {} · {}\n声明来源：{}\n程序：{}\n参数：{}\n\n程序具有当前用户权限，可能读文件和联网，VoicePort 不提供沙箱。只导入可信程序；导入和保存不会执行它。",
            self.path.display(),
            manifest.name,
            manifest.id,
            manifest.version,
            manifest.origin,
            program.executable.display(),
            serde_json::to_string(&program.args).unwrap_or_default()
        ))
    }
}

pub(crate) struct LibraryState {
    owner: Retained<Delegate>,
    store: RefCell<Store>,
    path: PathBuf,
    window: OnceCell<Retained<NSWindow>>,
    tabs: OnceCell<Retained<NSSegmentedControl>>,
    search: OnceCell<Retained<NSSearchField>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    notice: OnceCell<Retained<NSTextField>>,
    actions: RefCell<Vec<Action>>,
    editor: RefCell<Option<Retained<editor::Editor>>>,
    alert: RefCell<Option<Retained<NSAlert>>>,
    panel: RefCell<Option<Retained<NSOpenPanel>>>,
    blocked: Cell<bool>,
    presentation: presentation::Presentation,
    cleaned: Cell<bool>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = LibraryState]
    pub(crate) struct Library;
    unsafe impl NSObjectProtocol for Library {}
    unsafe impl NSWindowDelegate for Library {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, window: &NSWindow) -> bool { !self.ivars().presentation.writing() && window.attachedSheet().is_none() }
        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _notification: &NSNotification) {
            let keep_alive = self.retain();
            let iv = keep_alive.ivars();
            iv.presentation.close();
            if iv.cleaned.replace(true) { return; }
            iv.window.get().unwrap().setDelegate(None);
            iv.owner.ivars().source_manager.borrow_mut().take();
            iv.owner.ivars().managing_sources.set(false);
            iv.owner.refresh();
        }
    }
    impl Library {
        #[unsafe(method(closeLibrary:))]
        fn close_action(&self, _sender: Option<&AnyObject>) { self.window().performClose(None); }
        #[unsafe(method(filterLibrary:))]
        fn filter_action(&self, _sender: Option<&AnyObject>) { self.render(); }
        #[unsafe(method(addLibraryConnection:))]
        fn add_action(&self, _sender: Option<&AnyObject>) { self.add_connection(); }
        #[unsafe(method(libraryItem:))]
        fn item_action(&self, sender: &NSButton) {
            let action = usize::try_from(sender.tag()).ok().and_then(|i| self.ivars().actions.borrow().get(i).cloned());
            if let Some(action) = action { self.perform(action); }
        }
    }
);
fn mutation_ready(controller: Option<&Controller>) -> bool {
    controller.is_some_and(|c| !c.busy() && !c.discovering())
}
fn configurable(p: &Manifest) -> bool {
    matches!(p.adapter, Adapter::Http | Adapter::Jsonl)
}
fn matches_query(query: &str, values: &[&str]) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty() || values.iter().any(|s| s.to_lowercase().contains(&query))
}
fn description(p: &Manifest) -> &'static str {
    match p.adapter {
        Adapter::Chatgpt => "ChatGPT 登录 · 系统语音预览，停止后校正",
        Adapter::Paraformer => "Orca 本地模型 · 边说边预览",
        Adapter::Whisper => "Orca 本地模型 · 停止后转写",
        Adapter::Http => "填写地址、模型和密钥，连接兼容音频转写 API",
        Adapter::Jsonl => "本地插件 · 填写配置后启用",
    }
}
fn matching_plugins(store: &Store, query: &str) -> Vec<Manifest> {
    let mut plugins: Vec<_> = store
        .descriptors()
        .into_iter()
        .filter(|p| matches_query(query, &[&p.name, &p.id, &p.origin, description(p)]))
        .collect();
    // An exact imported ID must precede shorter substring/name collisions.
    plugins.sort_by_key(|p| p.id != query.trim());
    plugins
}
fn source_status(store: &Store, source: &Source) -> (String, bool) {
    let mut candidate = source.clone();
    candidate.enabled = true;
    match store.validate_source(&candidate) {
        Err(error) => (format!("需要处理 · {error}"), true),
        Ok(_) if !source.enabled => ("已停用".into(), false),
        Ok(_) => ("已启用 · 连接未检测".into(), false),
    }
}
impl Library {
    pub(super) fn window(&self) -> &NSWindow {
        self.ivars().window.get().unwrap()
    }
    pub(super) fn owner(&self) -> &Delegate {
        &self.ivars().owner
    }
    pub(super) fn store(&self) -> Store {
        self.ivars().store.borrow().clone()
    }
    pub(super) fn path(&self) -> &std::path::Path {
        &self.ivars().path
    }
    pub(in crate::app) fn bring_forward(&self) {
        self.window().makeKeyAndOrderFront(None);
    }
    /// Shutdown is explicit: discard an unsaved editor, end any owned sheet and
    /// break Delegate -> Library -> Delegate ownership on windowWillClose.
    pub(in crate::app) fn close_for_shutdown(&self) {
        self.ivars().presentation.close();
        if self.ivars().presentation.writing() {
            return;
        }
        let editor = self.ivars().editor.borrow().clone();
        if let Some(editor) = editor {
            editor.cancel();
        }
        if let Some(sheet) = self.window().attachedSheet() {
            self.window()
                .endSheet_returnCode(&sheet, NSModalResponseCancel);
            sheet.orderOut(None);
        }
        self.ivars().alert.borrow_mut().take();
        self.ivars().panel.borrow_mut().take();
        self.window().close();
    }
    pub(super) fn editor_finished(&self, token: u64) {
        self.ivars().presentation.complete(token);
        self.ivars().editor.borrow_mut().take();
    }
    fn edit(&self, index: Option<usize>, plugin: Manifest) {
        let Some(token) = self.ivars().presentation.begin() else {
            return;
        };
        let editor = editor::show(self, index, plugin, token);
        *self.ivars().editor.borrow_mut() = Some(editor);
    }
    pub(super) fn ready(&self) -> bool {
        if self.ivars().presentation.closed()
            || self.ivars().presentation.writing()
            || self.ivars().blocked.get()
        {
            return false;
        }
        if !mutation_ready(self.owner().ivars().controller.borrow().as_ref()) {
            self.set_notice("正在刷新来源，请稍候再操作。", false);
            return false;
        }
        true
    }
    pub(super) fn writing(&self) -> bool {
        self.ivars().presentation.writing()
    }
    pub(super) fn begin_write(&self) -> bool {
        self.ready() && self.ivars().presentation.begin_write()
    }
    fn available(&self) -> bool {
        self.ivars().presentation.browsing() && self.window().attachedSheet().is_none()
    }
    pub(super) fn set_notice(&self, message: &str, failed: bool) {
        let notice = self.ivars().notice.get().unwrap();
        notice.setStringValue(&NSString::from_str(message));
        notice.setToolTip(Some(&NSString::from_str(message)));
        let color = if failed {
            NSColor::systemOrangeColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        notice.setTextColor(Some(&color));
    }
    fn sheet(
        &self,
        title: &str,
        detail: &str,
        buttons: &[&str],
        action: impl Fn(&Library, isize) + 'static,
    ) {
        if !self.available() {
            return;
        }
        let Some(token) = self.ivars().presentation.begin() else {
            return;
        };
        let a = alert(self.owner(), title, detail, buttons);
        let library = self.retain();
        let callback = RcBlock::new(move |response| {
            if library.ivars().presentation.complete(token) {
                library.ivars().alert.borrow_mut().take();
                action(&library, response);
            }
        });
        *self.ivars().alert.borrow_mut() = Some(a.clone());
        a.beginSheetModalForWindow_completionHandler(self.window(), Some(&callback));
    }
    fn add_connection(&self) {
        if !self.available() || !self.ready() {
            return;
        }
        // A short routing sheet; explanations live in explicit help.
        self.sheet("添加转写方式", "已有服务地址和模型？连接兼容 API。\n本地插件需要作者提供的 VoicePort .json 清单和已安装的适配器。", &["连接兼容 API…", "导入本地插件…", "AI / Skill 帮助…", "取消"], |library, code| match code {
            1000 => library.perform(Action::Add(crate::plugins::HTTP.into())),
            1001 => library.import(),
            1002 => library.ai_help(),
            _ => {}
        });
    }
    fn ai_help(&self) {
        let cli = PathBuf::from(NSBundle::mainBundle().bundlePath().to_string())
            .join("Contents/MacOS/voiceport");
        let prompt = format!(
            "请用 VoicePort 接入 Skill，根据我接下来提供的服务文档判断：直接配置兼容 API，还是生成 JSONL 适配器和清单。给出具体配置、文件路径和使用步骤；缺失信息不要猜。\n\n当前 App 的 CLI 完整路径：{}\n请用该路径运行离线命令：\nskills get plugins\nskills get plugin-examples\n\n只生成和检查方案；不要读取真实密钥、录音、上传音频或修改配置。",
            cli.display()
        );
        self.sheet("AI / Skill 接入帮助", &format!("官网、GitHub 链接、zip 和模型文件不能直接导入。请提供服务文档，让 AI 判断接入方式。\n\n{prompt}"), &["关闭", "复制接入说明"], move |library, code| {
            if code == 1001 {
                let pasteboard = NSPasteboard::generalPasteboard();
                pasteboard.clearContents();
                let copied = pasteboard.setString_forType(&NSString::from_str(&prompt), unsafe { NSPasteboardTypeString });
                library.set_notice(if copied { "接入说明已复制；请粘贴给 AI 并提供服务文档。" } else { "复制失败，请重新打开帮助。" }, !copied);
            }
        });
    }
    fn import(&self) {
        if !self.available() || !self.ready() {
            return;
        }
        let Some(token) = self.ivars().presentation.begin() else {
            return;
        };
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setCanChooseFiles(true);
        panel.setCanChooseDirectories(false);
        panel.setAllowsMultipleSelection(false);
        panel.setTitle(Some(ns_string!("选择 VoicePort 插件清单")));
        panel.setPrompt(Some(ns_string!("审阅插件")));
        panel.setMessage(Some(ns_string!(
            "选择作者提供的 .json 清单；不会下载或运行程序。"
        )));
        let library = self.retain();
        let callback = RcBlock::new(move |response| {
            if !library.ivars().presentation.complete(token) {
                return;
            }
            let panel = library.ivars().panel.borrow_mut().take();
            if response != NSModalResponseOK {
                return;
            }
            let result = panel
                .and_then(|p| p.URL())
                .and_then(|u| u.path())
                .map(|path| PathBuf::from(path.to_string()))
                .ok_or_else(|| "未选择本地清单。".to_string())
                .and_then(ImportReview::load);
            match result {
                Ok(review) => library.review_import(review),
                Err(error) => library.set_notice(&error, true),
            }
        });
        *self.ivars().panel.borrow_mut() = Some(panel.clone());
        panel.beginSheetModalForWindow_completionHandler(self.window(), &callback);
    }
    fn review_import(&self, review: ImportReview) {
        let detail = match review.detail() {
            Ok(detail) => detail,
            Err(error) => {
                self.set_notice(&error, true);
                return;
            }
        };
        let manifest = review.manifest;
        self.sheet(
            "确认导入本地插件",
            &detail,
            &["取消", "导入"],
            move |library, code| {
                if code != 1001 || !library.begin_write() {
                    return;
                }
                let mut next = library.store();
                let result = next
                    .import(manifest.clone())
                    .and_then(|()| next.save(library.path()));
                library.reconcile(result, false, Some(&manifest.id));
            },
        );
    }
    fn button(
        &self,
        view: &NSView,
        title: &str,
        action: objc2::runtime::Sel,
        frame: NSRect,
    ) -> Retained<NSButton> {
        let b = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        };
        b.setFrame(frame);
        b.setAccessibilityLabel(Some(&NSString::from_str(title)));
        view.addSubview(&b);
        b
    }
    fn action_button(&self, view: &NSView, title: &str, action: Action, frame: NSRect) {
        let tag = self.ivars().actions.borrow().len();
        let b = self.button(view, title, sel!(libraryItem:), frame);
        b.setTag(tag as isize);
        let (kind, id) = match &action {
            Action::Add(id) => ("add", id),
            Action::Edit(id) => ("edit", id),
            Action::Remove(id) => ("remove", id),
            Action::Details(id) => ("details", id),
        };
        b.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
            "voiceport.library.{kind}.{id}"
        ))));
        b.setEnabled(kind == "details" || !self.ivars().blocked.get());
        self.ivars().actions.borrow_mut().push(action);
    }
    fn render(&self) {
        let iv = self.ivars();
        let d = self.owner();
        let store = iv.store.borrow();
        let query = iv.search.get().unwrap().stringValue().to_string();
        let sources = iv.tabs.get().unwrap().selectedSegment() == 1;
        iv.actions.borrow_mut().clear();
        let scroll = iv.scroll.get().unwrap();
        let width = scroll.contentSize().width;
        let plugins = matching_plugins(&store, &query);
        let rows: Vec<_> = store
            .sources
            .iter()
            .filter(|s| {
                let name = store
                    .plugin(&s.plugin_id)
                    .map(|p| p.name)
                    .unwrap_or_default();
                matches_query(&query, &[&s.name, &s.id, &s.plugin_id, &name])
            })
            .collect();
        let count = if sources { rows.len() } else { plugins.len() };
        let height = (count as f64 * 80.).max(scroll.contentSize().height);
        let view = NSView::initWithFrame(NSView::alloc(self.mtm()), rect(0., 0., width, height));
        if count == 0 {
            d.label(
                &view,
                if query.trim().is_empty() {
                    "还没有转写来源"
                } else {
                    "没有匹配项"
                },
                rect(24., height - 66., width - 48., 26.),
                15.,
            );
            d.caption(
                &view,
                if query.trim().is_empty() {
                    "点击右上角「添加」，连接 API 或导入本地插件。"
                } else {
                    "试试名称或完整 ID，也可以清空搜索。"
                },
                rect(24., height - 104., width - 48., 32.),
            );
        } else if sources {
            for (i, s) in rows.iter().enumerate() {
                let y = height - (i + 1) as f64 * 80.;
                d.surface(&view, rect(0., y + 3., width, 74.));
                let name = d.label(&view, &s.name, rect(14., y + 48., width - 210., 22.), 13.);
                name.setToolTip(Some(&NSString::from_str(&format!("{}\n{}", s.name, s.id))));
                let plugin = store
                    .plugin(&s.plugin_id)
                    .map(|p| p.name)
                    .unwrap_or_else(|_| "插件未安装".into());
                d.caption(&view, &plugin, rect(14., y + 29., width - 210., 18.));
                let (status, invalid) = source_status(&store, s);
                let label = d.caption(&view, &status, rect(14., y + 7., width - 210., 20.));
                label.setToolTip(Some(&NSString::from_str(&status)));
                if invalid {
                    label.setTextColor(Some(&NSColor::systemOrangeColor()));
                }
                self.action_button(
                    &view,
                    "编辑…",
                    Action::Edit(s.id.clone()),
                    rect(width - 174., y + 25., 78., 28.),
                );
                self.action_button(
                    &view,
                    "删除…",
                    Action::Remove(s.id.clone()),
                    rect(width - 90., y + 25., 78., 28.),
                );
            }
        } else {
            for (i, p) in plugins.iter().enumerate() {
                let y = height - (i + 1) as f64 * 80.;
                d.surface(&view, rect(0., y + 3., width, 74.));
                let name = d.label(&view, &p.name, rect(14., y + 46., width - 212., 24.), 13.);
                name.setToolTip(Some(&NSString::from_str(&p.name)));
                d.caption(&view, description(p), rect(14., y + 20., width - 212., 24.));
                self.action_button(
                    &view,
                    "详情",
                    Action::Details(p.id.clone()),
                    rect(width - 198., y + 25., 60., 28.),
                );
                if configurable(p) {
                    self.action_button(
                        &view,
                        if p.adapter == Adapter::Http {
                            "连接 API…"
                        } else {
                            "配置插件…"
                        },
                        Action::Add(p.id.clone()),
                        rect(width - 134., y + 25., 124., 28.),
                    );
                } else {
                    d.caption(
                        &view,
                        "在听写页选择",
                        rect(width - 128., y + 29., 118., 20.),
                    );
                }
            }
        }
        scroll.setDocumentView(Some(&view));
        view.scrollPoint(NSPoint::new(0., height));
    }
    fn perform(&self, action: Action) {
        if !self.available() {
            return;
        }
        if let Action::Details(id) = &action {
            if let Ok(p) = self.store().plugin(id) {
                let count = self
                    .store()
                    .sources
                    .iter()
                    .filter(|s| s.plugin_id == p.id)
                    .count();
                let detail = format!(
                    "{}\n\n版本：{}\n插件 ID：{}\n声明来源：{}\n配置来源数：{}{}",
                    description(&p),
                    p.version,
                    p.id,
                    p.origin,
                    count,
                    p.program
                        .as_ref()
                        .map(|p| format!(
                            "\n本地程序：{}\n参数：{}",
                            p.executable.display(),
                            serde_json::to_string(&p.args).unwrap_or_default()
                        ))
                        .unwrap_or_default()
                );
                self.sheet(&p.name, &detail, &["完成"], |_, _| {});
            }
            return;
        }
        if !self.ready() {
            return;
        }
        match action {
            Action::Add(id) => match self.store().plugin(&id) {
                Ok(plugin) => self.edit(None, plugin),
                Err(e) => self.set_notice(&e, true),
            },
            Action::Edit(id) => {
                let store = self.store();
                if let Some(index) = store.sources.iter().position(|s| s.id == id) {
                    match store.plugin(&store.sources[index].plugin_id) {
                        Ok(plugin) => self.edit(Some(index), plugin),
                        Err(e) => self.set_notice(&e, true),
                    }
                }
            }
            Action::Remove(id) => {
                if let Some(source) = self.store().sources.iter().find(|s| s.id == id) {
                    self.sheet(
                        "删除此来源？",
                        &format!(
                            "将删除「{}」及其独立密钥。当前选择若指向它将保持不可用。",
                            source.name
                        ),
                        &["取消", "删除"],
                        move |library, code| {
                            if code == 1001 && library.begin_write() {
                                let mut next = library.store();
                                let result =
                                    crate::source_service::remove(&mut next, library.path(), &id);
                                library.reconcile(result, false, None);
                            }
                        },
                    );
                }
            }
            Action::Details(_) => unreachable!(),
        }
    }
    /// Reload after every transaction, including failures after a durable commit.
    /// Never allow a stale snapshot to overwrite disk if reload itself fails.
    pub(super) fn reconcile(
        &self,
        result: Result<(), String>,
        adding: bool,
        imported: Option<&str>,
    ) {
        self.owner().with_controller(|c| c.refresh_catalog());
        let message = match Store::load(self.path()) {
            Ok(store) => {
                *self.ivars().store.borrow_mut() = store;
                if result.is_ok() {
                    if let Some(id) = imported {
                        self.ivars().tabs.get().unwrap().setSelectedSegment(0);
                        self.ivars()
                            .search
                            .get()
                            .unwrap()
                            .setStringValue(&NSString::from_str(id));
                    } else if adding {
                        self.ivars().tabs.get().unwrap().setSelectedSegment(1);
                        self.ivars()
                            .search
                            .get()
                            .unwrap()
                            .setStringValue(ns_string!(""));
                    }
                }
                match &result {
                    Err(e) => format!("操作未完全完成：{e}"),
                    Ok(()) if imported.is_some() => {
                        "已定位导入插件；点击「配置插件…」继续。".into()
                    }
                    Ok(()) => "更改已保存；保存未测试连接。".into(),
                }
            }
            Err(e) => {
                self.ivars().blocked.set(true);
                let operation = result
                    .as_ref()
                    .err()
                    .map(|error| format!("操作未完全完成：{error} "))
                    .unwrap_or_default();
                format!("{operation}无法重新读取配置：{e} 请关闭后重开。")
            }
        };
        self.set_notice(&message, result.is_err() || self.ivars().blocked.get());
        self.render();
        self.ivars().presentation.end_write();
    }
}

pub(super) fn show(d: &Delegate, store: Store, path: PathBuf) {
    let library = Library::alloc(d.mtm()).set_ivars(LibraryState {
        owner: d.retain(),
        store: RefCell::new(store),
        path,
        window: OnceCell::new(),
        tabs: OnceCell::new(),
        search: OnceCell::new(),
        scroll: OnceCell::new(),
        notice: OnceCell::new(),
        actions: RefCell::new(vec![]),
        editor: RefCell::new(None),
        alert: RefCell::new(None),
        panel: RefCell::new(None),
        blocked: Cell::new(false),
        presentation: presentation::Presentation::default(),
        cleaned: Cell::new(false),
    });
    let library: Retained<Library> = unsafe { msg_send![super(library), init] };
    let size = content_size(d, NSSize::new(700., 480.));
    let (w, h) = (size.width, size.height);
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(d.mtm()),
            rect(0., 0., w, h),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(ns_string!("插件与转写来源"));
    window.setDelegate(Some(ProtocolObject::from_ref(&*library)));
    let view = window.contentView().unwrap();
    let tabs = NSSegmentedControl::initWithFrame(
        NSSegmentedControl::alloc(d.mtm()),
        rect(16., h - 48., 200., 30.),
    );
    tabs.setSegmentCount(2);
    tabs.setLabel_forSegment(ns_string!("插件库"), 0);
    tabs.setLabel_forSegment(ns_string!("我的来源"), 1);
    tabs.setAccessibilityLabel(Some(ns_string!("管理视图")));
    tabs.setSelectedSegment(if library.store().sources.is_empty() {
        0
    } else {
        1
    });
    unsafe {
        tabs.setTarget(Some(&library));
        tabs.setAction(Some(sel!(filterLibrary:)));
    }
    view.addSubview(&tabs);
    let search = NSSearchField::initWithFrame(
        NSSearchField::alloc(d.mtm()),
        rect(228., h - 46., w - 400., 26.),
    );
    search.setPlaceholderString(Some(ns_string!("搜索名称或 ID")));
    search.setAccessibilityLabel(Some(ns_string!("搜索插件或来源")));
    search.setSendsWholeSearchString(false);
    search.setSendsSearchStringImmediately(true);
    unsafe {
        search.setTarget(Some(&library));
        search.setAction(Some(sel!(filterLibrary:)));
    }
    view.addSubview(&search);
    library.button(
        &view,
        "添加转写方式…",
        sel!(addLibraryConnection:),
        rect(w - 156., h - 48., 140., 30.),
    );
    let scroll = NSScrollView::initWithFrame(
        NSScrollView::alloc(d.mtm()),
        rect(16., 64., w - 32., h - 124.),
    );
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    view.addSubview(&scroll);
    let notice = d.caption(
        &view,
        "保存不运行插件、不发送音频。启用不代表连接成功。",
        rect(16., 10., w - 120., 42.),
    );
    let done = library.button(
        &view,
        "完成",
        sel!(closeLibrary:),
        rect(w - 96., 18., 80., 28.),
    );
    done.setKeyEquivalent(ns_string!("\u{1b}"));
    let iv = library.ivars();
    let _ = iv.window.set(window.clone());
    let _ = iv.tabs.set(tabs);
    let _ = iv.search.set(search);
    let _ = iv.scroll.set(scroll);
    let _ = iv.notice.set(notice);
    library.render();
    *d.ivars().source_manager.borrow_mut() = Some(library);
    window.center();
    window.makeKeyAndOrderFront(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local_manifest(dir: &std::path::Path) -> Manifest {
        use std::os::unix::fs::PermissionsExt;
        let program = dir.join("synthetic-program-never-executed");
        std::fs::write(&program, b"synthetic metadata fixture, never executed").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut manifest = Store::default().plugin(crate::plugins::HTTP).unwrap();
        manifest.id = "synthetic-local".into();
        manifest.adapter = Adapter::Jsonl;
        manifest.program = Some(crate::plugins::Program {
            executable: program,
            args: vec!["--synthetic".into(), "value with spaces".into()],
        });
        manifest
    }
    #[test]
    fn import_confirmation_distinguishes_same_named_files_and_retains_program_details() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = local_manifest(dir.path());
        let program = manifest.program.as_ref().unwrap();
        let mut reviews = vec![];
        for subdir in ["reviewed", "different-download"] {
            let parent = dir.path().join(subdir);
            std::fs::create_dir(&parent).unwrap();
            let path = parent.join("plugin.json");
            std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
            let review = ImportReview::load(path.clone()).unwrap();
            let detail = review.detail().unwrap();
            assert_eq!(review.path, path);
            assert_eq!(review.manifest, manifest);
            assert!(detail.contains(&path.display().to_string()));
            assert!(detail.contains(&program.executable.display().to_string()));
            assert!(detail.contains(&serde_json::to_string(&program.args).unwrap()));
            for value in [
                &manifest.name,
                &manifest.id,
                &manifest.version,
                &manifest.origin,
            ] {
                assert!(detail.contains(value));
            }
            reviews.push((path, detail));
        }
        assert_eq!(reviews[0].0.file_name(), reviews[1].0.file_name());
        assert_ne!(reviews[0].1, reviews[1].1);
        assert!(!reviews[0].1.contains(&reviews[1].0.display().to_string()));
        assert!(!reviews[1].1.contains(&reviews[0].0.display().to_string()));
        assert!(!dir.path().join("sources.json").exists());
    }
    #[test]
    fn import_review_rejects_unreadable_invalid_and_programless_manifests_before_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin.json");
        assert!(ImportReview::load(path.clone()).is_err());
        std::fs::write(&path, b"invalid JSON").unwrap();
        assert!(ImportReview::load(path.clone()).is_err());
        let mut manifest = local_manifest(dir.path());
        manifest.program = None;
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        assert!(ImportReview::load(path.clone()).is_err());
        let review = ImportReview { path, manifest };
        assert!(review.detail().is_err());
        assert!(!dir.path().join("sources.json").exists());
    }
    #[test]
    fn search_matches_identity_without_secrets() {
        assert!(matches_query(
            "HTTP",
            &["兼容音频转写", "voiceport.http-transcription"]
        ));
        assert!(matches_query("  工作  ", &["工作账号", "source-1"]));
        assert!(!matches_query("other", &["工作账号", "source-1"]));
        assert!(matches_query(" ", &[]));
    }
    #[test]
    fn imported_destination_precedes_substring_collisions() {
        let mut store = Store::default();
        for id in ["aa", "prefix-a", "a", "unrelated"] {
            let mut p = store.plugin(crate::plugins::HTTP).unwrap();
            p.id = id.into();
            p.name = format!("Plugin {id}");
            p.adapter = Adapter::Jsonl;
            store.plugins.push(p);
        }
        for id in ["a", "aa", "prefix-a", "unrelated"] {
            let matches = matching_plugins(&store, id);
            assert_eq!(matches.first().unwrap().id, id);
            assert_eq!(matches.iter().filter(|p| p.id == id).count(), 1);
        }
        let matches = matching_plugins(&store, "a");
        assert!(matches.iter().any(|p| p.id == crate::plugins::HTTP));
        assert!(matches.iter().any(|p| p.id == "aa"));
        assert!(matches.iter().any(|p| p.id == "prefix-a"));
        let all: Vec<_> = matching_plugins(&store, "")
            .into_iter()
            .map(|p| p.id)
            .collect();
        let original: Vec<_> = store.descriptors().into_iter().map(|p| p.id).collect();
        assert_eq!(all, original);
        assert!(matching_plugins(&store, "no-such-plugin-xyz").is_empty());
    }
    #[test]
    fn local_status_distinguishes_disabled_invalid_and_untested() {
        let store = Store::default();
        let mut source = Source {
            id: "one".into(),
            name: "One".into(),
            plugin_id: crate::plugins::HTTP.into(),
            plugin_version: "1".into(),
            enabled: false,
            config: BTreeMap::from([
                (
                    "endpoint".into(),
                    "https://example.invalid/transcribe".into(),
                ),
                ("model".into(), "test".into()),
            ]),
            credential_refs: BTreeMap::new(),
        };
        assert_eq!(source_status(&store, &source), ("已停用".into(), false));
        source.enabled = true;
        assert_eq!(
            source_status(&store, &source),
            ("已启用 · 连接未检测".into(), false)
        );
        source.config.remove("model");
        assert!(source_status(&store, &source).1);
        source.enabled = false;
        assert!(source_status(&store, &source).1);
    }
}
