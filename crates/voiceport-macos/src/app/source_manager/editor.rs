//! A compact parent-owned editor sheet; its controls own the unsaved draft.
use super::*;
use objc2::Message;

struct FieldControl {
    id: String,
    secret: bool,
    input: Retained<NSTextField>,
    remove: Option<Retained<NSButton>>,
}
pub(in crate::app) enum Host {
    Library(Retained<library::Library>),
    Models(Retained<model_manager::Manager>),
}
impl Host {
    fn owner(&self) -> &Delegate {
        match self {
            Self::Library(h) => h.owner(),
            Self::Models(h) => h.owner(),
        }
    }
    fn window(&self) -> &NSWindow {
        match self {
            Self::Library(h) => h.window(),
            Self::Models(h) => h.window(),
        }
    }
    fn path(&self) -> &std::path::Path {
        match self {
            Self::Library(h) => h.path(),
            Self::Models(h) => h.cloud_path(),
        }
    }
    fn store(&self) -> Result<Store, String> {
        match self {
            Self::Library(h) => Ok(h.store()),
            Self::Models(h) => h.cloud_store(),
        }
    }
    fn ready(&self) -> bool {
        match self {
            Self::Library(h) => h.ready(),
            Self::Models(h) => h.cloud_ready(),
        }
    }
    fn writing(&self) -> bool {
        match self {
            Self::Library(h) => h.writing(),
            Self::Models(h) => h.cloud_writing(),
        }
    }
    fn begin_write(&self) -> bool {
        match self {
            Self::Library(h) => h.begin_write(),
            Self::Models(h) => h.cloud_begin_write(),
        }
    }
    fn finished(&self, token: u64) {
        match self {
            Self::Library(h) => h.editor_finished(token),
            Self::Models(h) => h.cloud_editor_finished(),
        }
    }
    fn reconcile(&self, result: Result<(), String>, adding: bool) {
        match self {
            Self::Library(h) => h.reconcile(result, adding, None),
            Self::Models(h) => h.cloud_reconcile(result),
        }
    }
}
pub(in crate::app) struct EditorState {
    host: Host,
    preset: Option<crate::cloud_models::Preset>,
    plugin: Manifest,
    old: Option<Source>,
    index: Option<usize>,
    token: u64,
    window: OnceCell<Retained<NSWindow>>,
    name: OnceCell<Retained<NSTextField>>,
    enabled: OnceCell<Retained<NSButton>>,
    fields: RefCell<Vec<FieldControl>>,
    message: OnceCell<Retained<NSTextField>>,
    help: Cell<bool>,
    finished: Cell<bool>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = EditorState]
    pub(in crate::app) struct Editor;
    unsafe impl NSObjectProtocol for Editor {}
    impl Editor {
        #[unsafe(method(saveSourceDraft:))]
        fn save_action(&self, _sender: Option<&AnyObject>) { self.save(); }
        #[unsafe(method(cancelSourceDraft:))]
        fn cancel_action(&self, _sender: Option<&AnyObject>) { self.cancel(); }
        #[unsafe(method(sourceDraftHelp:))]
        fn help_action(&self, _sender: Option<&AnyObject>) {
            let iv = self.ivars();
            iv.help.set(!iv.help.get());
            self.message(if iv.help.get() { if iv.preset.is_some() {
                "填写服务文档中的 API 基础 URL（如 /v1）；已带 /audio/transcriptions 时不会重复追加。远程须 HTTPS，本地可用 loopback HTTP。旧自定义完整地址未改动时原样保留。密钥存系统钥匙串；本地服务可留空。保存不发送音频，点击使用后才切换。"
            } else if iv.plugin.adapter == Adapter::Http {
                "地址需包含完整音频转写路径（例如 /v1/audio/transcriptions），不能填官网或控制台。远程须 HTTPS，本机可用 loopback HTTP。模型按服务文档填写；密钥存入系统钥匙串。启用后回听写页选择来源。"
            } else {
                "按插件作者说明填写配置；适配器须已安装。密钥存入系统钥匙串，其他字段保存在本机。只有显式启用并选择来源后才可能执行程序。保存不执行插件或测试连接。"
            }} else { "保存不运行插件、不测试连接。密钥独立保存在系统钥匙串。" }, false);
        }
    }
);
impl Editor {
    fn message(&self, text: &str, failed: bool) {
        let label = self.ivars().message.get().unwrap();
        label.setStringValue(&NSString::from_str(text));
        label.setToolTip(Some(&NSString::from_str(text)));
        let color = if failed {
            NSColor::systemOrangeColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        label.setTextColor(Some(&color));
    }
    pub(in crate::app) fn cancel(&self) {
        if self.ivars().host.writing() {
            return;
        }
        self.finish();
    }
    fn finish(&self) {
        let keep_alive = self.retain();
        let iv = keep_alive.ivars();
        if iv.finished.replace(true) {
            return;
        }
        let window = iv.window.get().unwrap();
        iv.host
            .window()
            .endSheet_returnCode(window, NSModalResponseCancel);
        window.orderOut(None);
        // Break Library -> Editor -> Library only after the sheet has ended.
        iv.host.finished(iv.token);
    }
    fn save(&self) {
        let keep_alive = self.retain();
        let iv = keep_alive.ivars();
        if iv.finished.get() {
            return;
        }
        if !iv.host.ready() {
            self.message("暂时无法保存：正在刷新或配置读取失败。请稍候，或取消后查看管理窗口提示。已填写内容仍保留。", true);
            return;
        }
        let mut values: Vec<_> = iv
            .fields
            .borrow()
            .iter()
            .map(|f| draft::FieldValue {
                id: f.id.clone(),
                secret: f.secret,
                value: f.input.stringValue().to_string(),
                remove: f
                    .remove
                    .as_ref()
                    .is_some_and(|b| b.state() == NSControlStateValueOn),
            })
            .collect();
        if iv.preset.is_some()
            && let Some(field) = values.iter_mut().find(|f| f.id == "endpoint")
        {
            let old = iv
                .old
                .as_ref()
                .and_then(|s| s.config.get("endpoint"))
                .map(String::as_str);
            match crate::cloud_models::transcription_endpoint(&field.value, old) {
                Ok(endpoint) => field.value = endpoint,
                Err(error) => {
                    self.message(&error, true);
                    return;
                }
            }
        }
        let (source, pending) = draft::candidate(
            iv.old.as_ref(),
            &iv.plugin,
            &iv.name.get().unwrap().stringValue().to_string(),
            iv.enabled.get().unwrap().state() == NSControlStateValueOn,
            &values,
        );
        let mut store = match iv.host.store() {
            Ok(store) => store,
            Err(error) => {
                self.message(&error, true);
                return;
            }
        };
        if let Err(message) = draft::validate_with_pending(&store, iv.index, &source, &pending) {
            self.message(
                &format!("{message} 请修改后再次保存；已填写内容仍保留。"),
                true,
            );
            return;
        }
        if !iv.host.begin_write() {
            self.message("正在处理配置，请稍候；已填写内容仍保留。", true);
            return;
        }
        let result = crate::source_service::save_source(
            &mut store,
            iv.host.path(),
            iv.index,
            source,
            pending,
        );
        iv.host.owner().with_controller(|c| {
            if result.is_ok() {
                c.message = "来源配置已保存；保存未发送网络请求或执行插件。".into();
            }
        });
        // Transactions can fail after committing. Reload before deciding what is
        // editable again; the library reports the original error and blocks stale writes.
        iv.host.reconcile(result, iv.index.is_none());
        self.finish();
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
}
fn input(
    d: &Delegate,
    view: &NSView,
    label: &str,
    value: &str,
    hint: &str,
    row: NSRect,
    secret: bool,
) -> Retained<NSTextField> {
    let (y, width) = (row.origin.y, row.size.width);
    d.label(view, label, rect(8., y + 25., 112., 22.), 12.);
    let field = if secret {
        NSSecureTextField::initWithFrame(
            NSSecureTextField::alloc(d.mtm()),
            rect(124., y + 24., width - 132., 24.),
        )
        .into_super()
    } else {
        NSTextField::initWithFrame(
            NSTextField::alloc(d.mtm()),
            rect(124., y + 24., width - 132., 24.),
        )
    };
    field.setStringValue(&NSString::from_str(value));
    field.setAccessibilityLabel(Some(&NSString::from_str(label)));
    field.setPlaceholderString(Some(&NSString::from_str(hint)));
    field.setToolTip(Some(&NSString::from_str(hint)));
    view.addSubview(&field);
    d.caption(view, hint, rect(124., y + 2., width - 132., 20.));
    field
}
pub(super) fn show(
    library: &library::Library,
    index: Option<usize>,
    plugin: Manifest,
    token: u64,
) -> Retained<Editor> {
    show_host(Host::Library(library.retain()), index, plugin, token, None)
}
pub(in crate::app) fn show_cloud(
    manager: &model_manager::Manager,
    index: Option<usize>,
    preset: crate::cloud_models::Preset,
) -> Retained<Editor> {
    let plugin = Store::default()
        .plugin(crate::plugins::HTTP)
        .expect("built-in HTTP");
    show_host(
        Host::Models(manager.retain()),
        index,
        plugin,
        0,
        Some(preset),
    )
}
fn show_host(
    host: Host,
    index: Option<usize>,
    plugin: Manifest,
    token: u64,
    preset: Option<crate::cloud_models::Preset>,
) -> Retained<Editor> {
    let old = index.and_then(|i| host.store().ok()?.sources.get(i).cloned());
    let mtm = host.owner().mtm();
    let editor = Editor::alloc(mtm).set_ivars(EditorState {
        host,
        preset,
        plugin,
        old,
        index,
        token,
        window: OnceCell::new(),
        name: OnceCell::new(),
        enabled: OnceCell::new(),
        fields: RefCell::new(vec![]),
        message: OnceCell::new(),
        help: Cell::new(false),
        finished: Cell::new(false),
    });
    let editor: Retained<Editor> = unsafe { msg_send![super(editor), init] };
    let iv = editor.ivars();
    let d = iv.host.owner();
    let form_height = 36. + (iv.plugin.fields.len() + 1) as f64 * 52.;
    let size = content_size(d, NSSize::new(620., (form_height + 184.).min(480.)));
    let (w, h) = (size.width, size.height);
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(d.mtm()),
            rect(0., 0., w, h),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(if index.is_some() {
        ns_string!("编辑转写来源")
    } else {
        ns_string!("添加转写来源")
    });
    let view = window.contentView().unwrap();
    d.caption(&view, &iv.plugin.name, rect(20., h - 34., w - 40., 22.));
    let scroll = NSScrollView::initWithFrame(
        NSScrollView::alloc(d.mtm()),
        rect(16., 140., w - 32., h - 184.),
    );
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    let form_width = scroll.contentSize().width;
    let form = NSView::initWithFrame(
        NSView::alloc(d.mtm()),
        rect(0., 0., form_width, form_height),
    );
    let name = input(
        d,
        &form,
        "显示名称",
        iv.old
            .as_ref()
            .map_or(preset.map_or("", |p| p.name()), |s| s.name.as_str()),
        "例如：工作账号",
        rect(0., form_height - 52., form_width, 52.),
        false,
    );
    let mut key_views: Vec<Retained<NSView>> = vec![name.clone().into_super().into_super()];
    let _ = iv.name.set(name.clone());
    for (i, f) in iv.plugin.fields.iter().enumerate() {
        let y = form_height - (i + 2) as f64 * 52.;
        let retained = iv.old.as_ref().filter(|s| s.plugin_id == iv.plugin.id);
        let has_key = retained.is_some_and(|s| s.credential_refs.contains_key(&f.id));
        let hint = match f.id.as_str() {
            "endpoint" if preset.is_some() => {
                "例如 https://api.groq.com/openai/v1；也接受完整转写地址"
            }
            "endpoint" if iv.plugin.adapter == Adapter::Http => {
                "https://api.example.com/v1/audio/transcriptions"
            }
            "model" if iv.plugin.adapter == Adapter::Http => "如 whisper-1；按服务文档填写",
            _ if f.kind == FieldKind::Secret && has_key => "已保存；留空保留，输入替换",
            _ if f.kind == FieldKind::Secret && f.required => "必填密钥；按插件说明填写",
            _ if f.kind == FieldKind::Secret => "可选；服务需要鉴权时填写",
            _ if f.required => "必填；按插件说明填写",
            _ => "可选",
        };
        let label = if iv.plugin.adapter == Adapter::Http {
            match f.id.as_str() {
                "endpoint" if preset.is_some() => "基础 URL",
                "endpoint" => "转写地址",
                "model" => "模型",
                "api_key" => "API Key",
                _ => &f.label,
            }
        } else {
            &f.label
        };
        let value = if f.kind == FieldKind::Secret {
            String::new()
        } else if let Some(value) = retained.and_then(|s| s.config.get(&f.id)) {
            if preset.is_some() && f.id == "endpoint" {
                crate::cloud_models::editable_base(value)
            } else {
                value.clone()
            }
        } else {
            preset
                .map_or("", |p| match f.id.as_str() {
                    "endpoint" => p.base_url(),
                    "model" => p.model(),
                    _ => "",
                })
                .into()
        };
        let control = input(
            d,
            &form,
            &format!("{label}{}", if f.required { " *" } else { "" }),
            &value,
            hint,
            rect(
                0.,
                y,
                if f.kind == FieldKind::Secret {
                    form_width - 94.
                } else {
                    form_width
                },
                52.,
            ),
            f.kind == FieldKind::Secret,
        );
        key_views.push(control.clone().into_super().into_super());
        let remove = (f.kind == FieldKind::Secret).then(|| {
            let b = checkbox(
                d,
                &form,
                "移除密钥",
                false,
                rect(form_width - 96., y + 24., 94., 24.),
            );
            key_views.push(b.clone().into_super().into_super());
            b
        });
        iv.fields.borrow_mut().push(FieldControl {
            id: f.id.clone(),
            secret: f.kind == FieldKind::Secret,
            input: control,
            remove,
        });
    }
    let enabled = checkbox(
        d,
        &form,
        "启用此来源",
        iv.old.as_ref().map_or(preset.is_some(), |s| s.enabled),
        rect(124., 3., 200., 26.),
    );
    key_views.push(enabled.clone().into_super().into_super());
    let _ = iv.enabled.set(enabled);
    scroll.setDocumentView(Some(&form));
    view.addSubview(&scroll);
    form.scrollPoint(NSPoint::new(0., form_height));
    let message = d.caption(
        &view,
        "保存不运行插件、不测试连接。密钥独立保存在系统钥匙串。",
        rect(24., 54., w - 48., 76.),
    );
    let _ = iv.message.set(message);
    let help = editor.button(
        &view,
        "填写帮助",
        sel!(sourceDraftHelp:),
        rect(20., 14., 96., 28.),
    );
    let cancel = editor.button(
        &view,
        "取消",
        sel!(cancelSourceDraft:),
        rect(w - 196., 14., 80., 28.),
    );
    cancel.setKeyEquivalent(ns_string!("\u{1b}"));
    let save = editor.button(
        &view,
        "保存来源",
        sel!(saveSourceDraft:),
        rect(w - 110., 14., 90., 28.),
    );
    save.setKeyEquivalent(ns_string!("\r"));
    key_views.extend([
        help.into_super().into_super(),
        cancel.into_super().into_super(),
        save.into_super().into_super(),
    ]);
    for i in 0..key_views.len() {
        unsafe {
            key_views[i].setNextKeyView(Some(&key_views[(i + 1) % key_views.len()]));
        }
    }
    window.setInitialFirstResponder(Some(&name));
    let _ = iv.window.set(window.clone());
    iv.host.window().beginSheet_completionHandler(&window, None);
    window.makeFirstResponder(Some(&name));
    editor
}
