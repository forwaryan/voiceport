//! A local plugin library, with separate adapter cards and configured source rows.
//! Browsing, filtering and rendering never resolve credentials or run an adapter.
use super::*;
use objc2::Message;
use std::path::PathBuf;

#[derive(Clone)]
enum Action {
    Add(String),
    Edit(String),
    Remove(String),
    Details(String),
}
struct LibraryState {
    owner: Retained<Delegate>,
    store: RefCell<Store>,
    path: PathBuf,
    window: OnceCell<Retained<NSWindow>>,
    tabs: OnceCell<Retained<NSSegmentedControl>>,
    search: OnceCell<Retained<NSSearchField>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    notice: OnceCell<Retained<NSTextField>>,
    actions: RefCell<Vec<Action>>,
    blocked: Cell<bool>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = LibraryState]
    struct Library;
    unsafe impl NSObjectProtocol for Library {}
    unsafe impl NSWindowDelegate for Library {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _window: &NSWindow) -> bool {
            NSApplication::sharedApplication(self.mtm()).stopModal();
            false
        }
    }
    impl Library {
        #[unsafe(method(closeLibrary:))]
        fn close_action(&self, _sender: Option<&AnyObject>) {
            NSApplication::sharedApplication(self.mtm()).stopModal();
        }
        #[unsafe(method(filterLibrary:))]
        fn filter_action(&self, _sender: Option<&AnyObject>) { self.render(); }
        #[unsafe(method(addLibraryConnection:))]
        fn add_action(&self, _sender: Option<&AnyObject>) { self.add_connection(); }
        #[unsafe(method(libraryItem:))]
        fn item_action(&self, sender: &NSButton) {
            let action = usize::try_from(sender.tag()).ok()
                .and_then(|i| self.ivars().actions.borrow().get(i).cloned());
            if let Some(action) = action { self.perform(Some(action)); }
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
        Adapter::Chatgpt => "使用 ChatGPT 登录；系统语音预览，停止后校正。",
        Adapter::Paraformer => "使用 Orca 已安装的本地模型，边说边预览。",
        Adapter::Whisper => "使用 Orca 已安装的本地模型，停止后转写。",
        Adapter::Http => "有 API 地址、模型和密钥？直接连接兼容转写服务，无需导入文件。",
        Adapter::Jsonl => "插件已导入。点击「配置此插件」，填写信息并启用后即可选用。",
    }
}
fn matching_plugins(store: &Store, query: &str) -> Vec<Manifest> {
    let mut plugins: Vec<_> = store
        .descriptors()
        .into_iter()
        .filter(|p| matches_query(query, &[&p.name, &p.id, &p.origin, description(p)]))
        .collect();
    // Import hands off using the complete stable ID. Keep that destination first,
    // even when a short ID also matches builtin fields or other plugin names.
    plugins.sort_by_key(|p| p.id != query.trim());
    plugins
}
fn source_status(store: &Store, source: &Source) -> (String, bool) {
    // A valid local configuration is not a successful connection/ASR test.
    let mut candidate = source.clone();
    candidate.enabled = true;
    match store.validate_source(&candidate) {
        Err(error) => (format!("需要处理 · {error}"), true),
        Ok(_) if !source.enabled => ("已停用".into(), false),
        Ok(_) => ("已启用 · 连接状态未检测".into(), false),
    }
}
impl Library {
    fn add_connection(&self) {
        if self.ivars().blocked.get() {
            return;
        }
        let choice = alert(
            &self.ivars().owner,
            "你准备接入哪种转写方式？",
            "有兼容 API 的服务地址、模型名和 API Key\n选择「连接兼容 API」，直接填表，无需下载或导入插件。服务需支持 OpenAI 兼容的音频转写接口。\n\n已有插件作者提供的 VoicePort 清单（.json）\n选择「导入本地插件」。清单指向的适配器程序必须已在本机安装；导入后还需要填写配置。\n\n只有官网、GitHub 链接、压缩包或模型文件？\n这些不能直接导入。请向服务商确认兼容 API，或向插件作者索取 VoicePort 清单与适配器。",
            &["连接兼容 API…", "导入本地插件…", "用 AI 帮我接入…", "取消"],
        );
        match choice.runModal() {
            1000 => self.perform(Some(Action::Add(crate::plugins::HTTP.into()))),
            1001 => self.perform(None),
            1002 => self.ai_help(),
            _ => {}
        }
    }
    fn ai_help(&self) {
        let cli = PathBuf::from(NSBundle::mainBundle().bundlePath().to_string())
            .join("Contents/MacOS/voiceport");
        let prompt = format!(
            "请用 VoicePort 接入 Skill，根据我接下来提供的服务文档判断：直接配置兼容 API，还是生成 JSONL 适配器和清单。给出具体配置、文件路径和使用步骤；缺失信息不要猜。\n\n当前 App 的 CLI 完整路径：{}\n请用该路径运行离线命令：\nskills get plugins\nskills get plugin-examples\n\n只生成和检查方案；不要读取真实密钥、录音、上传音频或修改配置。",
            cli.display()
        );
        let a = alert(
            &self.ivars().owner,
            "把接入指南交给你的 AI",
            "复制下面的说明给 AI，再提供服务商文档或本地项目说明。随 App 附有三种例子：兼容 API、本机服务、自定义 JSON 接口。AI 会先判断是否需要插件；你不必从写 JSON 开始。",
            &["复制接入说明", "关闭"],
        );
        let text = NSTextField::wrappingLabelWithString(&NSString::from_str(&prompt), self.mtm());
        text.setFrame(rect(0., 0., 480., 280.));
        a.setAccessoryView(Some(&text));
        if a.runModal() == 1000 {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            let copied = pasteboard.setString_forType(&NSString::from_str(&prompt), unsafe {
                NSPasteboardTypeString
            });
            self.ivars()
                .notice
                .get()
                .unwrap()
                .setStringValue(&NSString::from_str(if copied {
                    "接入说明已复制。粘贴给 AI，再提供服务文档即可开始。"
                } else {
                    "未能复制接入说明。请重新打开指南并手动选择文字复制。"
                }));
        }
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
        // Identity is independent of duplicate display names, sorting and filtering.
        let id = match &action {
            Action::Add(id) | Action::Edit(id) | Action::Remove(id) | Action::Details(id) => id,
        };
        b.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
            "voiceport.library.{}.{id}",
            match action {
                Action::Add(_) => "add",
                Action::Edit(_) => "edit",
                Action::Remove(_) => "remove",
                Action::Details(_) => "details",
            }
        ))));
        b.setEnabled(!self.ivars().blocked.get());
        self.ivars().actions.borrow_mut().push(action);
    }
    fn render(&self) {
        let iv = self.ivars();
        let d = &iv.owner;
        let store = iv.store.borrow();
        let query = iv.search.get().unwrap().stringValue().to_string();
        let sources = iv.tabs.get().unwrap().selectedSegment() == 1;
        iv.actions.borrow_mut().clear();
        let width = 776.;
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
        let content_height = if sources {
            66. + rows.len() as f64 * 118.
        } else {
            66. + plugins.len().div_ceil(2) as f64 * 196.
        };
        let height = content_height.max(408.);
        let view = NSView::initWithFrame(NSView::alloc(self.mtm()), rect(0., 0., width, height));
        let title = if sources {
            format!("我的来源  ·  {} 个", rows.len())
        } else {
            format!("可用接入方式  ·  {} 个", plugins.len())
        };
        d.label(&view, &title, rect(12., height - 32., 700., 24.), 15.);
        d.caption(
            &view,
            if sources {
                "每份配置独立保存；启用不代表连接成功。样例测试请在听写页选择来源后发起。"
            } else {
                "有兼容 API 可直接填表；本地插件需先导入清单，再填写配置。"
            },
            rect(12., height - 58., 750., 22.),
        );
        if (sources && rows.is_empty()) || (!sources && plugins.is_empty()) {
            d.icon(
                &view,
                if query.trim().is_empty() {
                    "puzzlepiece.extension"
                } else {
                    "magnifyingglass"
                },
                rect(362., height - 160., 44., 44.),
            );
            let empty = if !query.trim().is_empty() {
                "没有找到匹配项"
            } else {
                "添加第一个转写来源"
            };
            d.label(&view, empty, rect(260., height - 206., 380., 28.), 20.);
            d.caption(
                &view,
                if !query.trim().is_empty() {
                    "试试插件名称或来源名称，也可以清空搜索。"
                } else {
                    "点击「添加转写方式」，选择连接兼容 API 或导入本地插件。"
                },
                rect(180., height - 244., 550., 32.),
            );
            if query.trim().is_empty() {
                self.button(
                    &view,
                    "添加转写方式…",
                    sel!(addLibraryConnection:),
                    rect(310., height - 294., 156., 32.),
                );
            }
        } else if sources {
            for (i, s) in rows.iter().enumerate() {
                let y = height - 66. - (i + 1) as f64 * 118.;
                d.surface(&view, rect(8., y + 8., 752., 104.));
                d.icon(&view, "waveform", rect(24., y + 64., 22., 22.));
                d.label(&view, &s.name, rect(58., y + 62., 430., 26.), 15.);
                let plugin = store
                    .plugin(&s.plugin_id)
                    .map(|p| p.name)
                    .unwrap_or_else(|_| "插件未安装".into());
                d.caption(
                    &view,
                    &format!("{} · v{}", plugin, s.plugin_version),
                    rect(58., y + 38., 450., 20.),
                );
                let (status, invalid) = source_status(&store, s);
                let label = d.caption(&view, &status, rect(58., y + 12., 450., 24.));
                if invalid {
                    label.setTextColor(Some(&NSColor::systemOrangeColor()));
                }
                label.setToolTip(Some(&NSString::from_str(&status)));
                self.action_button(
                    &view,
                    "编辑…",
                    Action::Edit(s.id.clone()),
                    rect(528., y + 43., 100., 30.),
                );
                self.action_button(
                    &view,
                    "删除…",
                    Action::Remove(s.id.clone()),
                    rect(634., y + 43., 100., 30.),
                );
            }
        } else {
            for (i, p) in plugins.iter().enumerate() {
                let x = 8. + (i % 2) as f64 * 384.;
                let y = height - 66. - (i / 2 + 1) as f64 * 196.;
                d.surface(&view, rect(x, y + 10., 368., 180.));
                let symbol = match p.adapter {
                    Adapter::Http => "network",
                    Adapter::Jsonl => "puzzlepiece.extension",
                    Adapter::Chatgpt => "bubble.left.and.bubble.right",
                    _ => "desktopcomputer",
                };
                d.icon(&view, symbol, rect(x + 16., y + 142., 28., 28.));
                d.label(&view, &p.name, rect(x + 56., y + 145., 296., 28.), 15.);
                d.caption(
                    &view,
                    &format!(
                        "{} · v{}",
                        if p.adapter == Adapter::Jsonl {
                            "本地导入"
                        } else {
                            "内置"
                        },
                        p.version
                    ),
                    rect(x + 56., y + 122., 290., 20.),
                );
                d.label(
                    &view,
                    description(p),
                    rect(x + 16., y + 69., 336., 44.),
                    12.,
                );
                let count = store.sources.iter().filter(|s| s.plugin_id == p.id).count();
                let capability = if p.capabilities.streaming {
                    "本地流式"
                } else if p.capabilities.system_preview {
                    "语音预览 + 最终转写"
                } else {
                    "停止后转写"
                };
                d.caption(
                    &view,
                    &format!(
                        "{}{}",
                        capability,
                        if configurable(p) {
                            format!("  ·  {count} 个来源")
                        } else {
                            String::new()
                        }
                    ),
                    rect(x + 16., y + 45., 336., 20.),
                );
                self.action_button(
                    &view,
                    "详情",
                    Action::Details(p.id.clone()),
                    rect(x + 12., y + 14., 64., 28.),
                );
                if configurable(p) {
                    self.action_button(
                        &view,
                        if p.adapter == Adapter::Http {
                            "填写服务信息…"
                        } else {
                            "配置此插件…"
                        },
                        Action::Add(p.id.clone()),
                        rect(x + 228., y + 14., 124., 28.),
                    );
                } else {
                    d.caption(&view, "在听写页选择", rect(x + 240., y + 18., 112., 20.));
                }
            }
        }
        iv.scroll.get().unwrap().setDocumentView(Some(&view));
        view.scrollPoint(NSPoint::new(0., height));
    }
    fn perform(&self, action: Option<Action>) {
        let iv = self.ivars();
        if iv.blocked.get() {
            return;
        }
        let d = &iv.owner;
        if let Some(Action::Details(id)) = &action {
            let plugin = iv.store.borrow().plugin(id);
            if let Ok(p) = plugin {
                let detail = format!(
                    "{}\n\n版本：{}\n插件 ID：{}\n声明来源：{}{}",
                    description(&p),
                    p.version,
                    p.id,
                    p.origin,
                    p.program
                        .as_ref()
                        .map(|p| format!(
                            "\n本地程序：{}\n参数：{}",
                            p.executable.display(),
                            serde_json::to_string(&p.args).unwrap_or_default()
                        ))
                        .unwrap_or_default()
                );
                alert(d, &p.name, &detail, &["完成"]).runModal();
            }
            return;
        }
        // The manager stays open across edits, unlike the old one-shot alert.
        // Serialize every mutation against the previous catalog reconciliation;
        // refresh_catalog intentionally drops requests during an active discovery.
        let ready = mutation_ready(d.ivars().controller.borrow().as_ref());
        if !ready {
            let notice = iv.notice.get().unwrap();
            notice.setStringValue(ns_string!(
                "正在刷新来源，请稍候再操作。你仍可浏览和搜索插件。"
            ));
            notice.setTextColor(Some(&NSColor::secondaryLabelColor()));
            return;
        }
        let before = iv.store.borrow().clone();
        let adding = matches!(action, Some(Action::Add(_)));
        let mut imported_id = None;
        let result = {
            let mut store = iv.store.borrow_mut();
            match action {
                Some(Action::Add(id)) => store
                    .plugin(&id)
                    .and_then(|p| edit_with_plugin(d, &mut store, &iv.path, None, p)),
                Some(Action::Edit(id)) => store
                    .sources
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or_else(|| "来源不存在。".to_owned())
                    .and_then(|i| edit(d, &mut store, &iv.path, Some(i))),
                Some(Action::Remove(id)) => remove(d, &mut store, &iv.path, &id),
                None => import(d, &mut store, &iv.path).map(|id| imported_id = id),
                Some(Action::Details(_)) => unreachable!(),
            }
        };
        // A failed credential cleanup or directory fsync may follow a committed
        // save. Reload durable state on every return, including failed removals.
        // If reloading fails, prevent further writes from this stale snapshot.
        let reload = Store::load(&iv.path);
        d.with_controller(|c| c.refresh_catalog());
        let (message, failed) = match reload {
            Ok(store) => {
                let changed = store != before;
                *iv.store.borrow_mut() = store;
                if let Some(id) = &imported_id {
                    iv.tabs.get().unwrap().setSelectedSegment(0);
                    iv.search
                        .get()
                        .unwrap()
                        .setStringValue(&NSString::from_str(id));
                } else if changed && adding {
                    iv.tabs.get().unwrap().setSelectedSegment(1);
                    iv.search.get().unwrap().setStringValue(ns_string!(""));
                }
                match result {
                    Err(error) => (format!("操作未完全完成：{error}"), true),
                    Ok(()) if imported_id.is_some() => (
                        "已定位到导入的插件。下一步点击「配置此插件…」，填写信息并启用来源。"
                            .into(),
                        false,
                    ),
                    Ok(()) if changed => (
                        "更改已保存。启用后，回到听写页选择此来源即可使用。".into(),
                        false,
                    ),
                    Ok(()) => ("未更改配置。".into(), false),
                }
            }
            Err(error) => {
                iv.blocked.set(true);
                (
                    format!("无法重新读取配置：{error} 请关闭此窗口后重新打开。"),
                    true,
                )
            }
        };
        let notice = iv.notice.get().unwrap();
        notice.setStringValue(&NSString::from_str(&message));
        let color = if failed {
            NSColor::systemOrangeColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        notice.setTextColor(Some(&color));
        notice.setToolTip(Some(&NSString::from_str(&message)));
        self.render();
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
        blocked: Cell::new(false),
    });
    let library: Retained<Library> = unsafe { msg_send![super(library), init] };
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(d.mtm()),
            rect(0., 0., 824., 610.),
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
    d.label(&view, "插件与转写来源", rect(24., 558., 570., 30.), 22.);
    d.caption(
        &view,
        "添加自己的转写服务：兼容 API 直接填表，本地插件按清单导入。",
        rect(24., 530., 700., 22.),
    );
    let tabs = NSSegmentedControl::initWithFrame(
        NSSegmentedControl::alloc(d.mtm()),
        rect(24., 480., 300., 32.),
    );
    tabs.setSegmentCount(2);
    tabs.setLabel_forSegment(ns_string!("插件库"), 0);
    tabs.setLabel_forSegment(ns_string!("我的来源"), 1);
    tabs.setSelectedSegment(if library.ivars().store.borrow().sources.is_empty() {
        0
    } else {
        1
    });
    unsafe {
        tabs.setTarget(Some(&library));
        tabs.setAction(Some(sel!(filterLibrary:)));
    }
    view.addSubview(&tabs);
    let search =
        NSSearchField::initWithFrame(NSSearchField::alloc(d.mtm()), rect(488., 482., 312., 28.));
    search.setPlaceholderString(Some(ns_string!("搜索插件或来源")));
    search.setAccessibilityLabel(Some(ns_string!("搜索插件或来源")));
    search.setSendsWholeSearchString(false);
    search.setSendsSearchStringImmediately(true);
    unsafe {
        search.setTarget(Some(&library));
        search.setAction(Some(sel!(filterLibrary:)));
    }
    view.addSubview(&search);
    let scroll =
        NSScrollView::initWithFrame(NSScrollView::alloc(d.mtm()), rect(24., 66., 776., 408.));
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    view.addSubview(&scroll);
    let notice = d.caption(
        &view,
        "不确定如何接入？点击「添加转写方式」。填写和保存不会发送音频。",
        rect(24., 14., 500., 44.),
    );
    library.button(
        &view,
        "添加转写方式…",
        sel!(addLibraryConnection:),
        rect(528., 22., 136., 32.),
    );
    let done = library.button(
        &view,
        "完成",
        sel!(closeLibrary:),
        rect(688., 22., 112., 32.),
    );
    done.setKeyEquivalent(ns_string!("\u{1b}"));
    let iv = library.ivars();
    let _ = iv.window.set(window.clone());
    let _ = iv.tabs.set(tabs);
    let _ = iv.search.set(search);
    let _ = iv.scroll.set(scroll);
    let _ = iv.notice.set(notice);
    library.render();
    window.center();
    window.makeKeyAndOrderFront(None);
    NSApplication::sharedApplication(d.mtm()).runModalForWindow(&window);
    window.orderOut(None);
    window.setDelegate(None);
}

#[cfg(test)]
mod tests {
    use super::*;
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
            ("已启用 · 连接状态未检测".into(), false)
        );
        source.config.remove("model");
        assert!(source_status(&store, &source).1);
        source.enabled = false;
        assert!(source_status(&store, &source).1);
    }
}
