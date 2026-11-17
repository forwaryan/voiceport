//! Native model library. One retained window owns installation and its admission lock.
use super::*;
use crate::model_library::{Definition, Installation, Job, MODELS, Operation, Paths};
use block2::RcBlock;
use objc2::Message;
use std::path::PathBuf;
mod cloud;
mod presentation;
use presentation::ROW_HEIGHT;

struct Row {
    definition: &'static Definition,
    view: Retained<NSView>,
    surface: Retained<NSBox>,
    status: Retained<NSTextField>,
    compatibility: Retained<NSTextField>,
    primary: Retained<NSButton>,
    import: Retained<NSButton>,
    update: Retained<NSButton>,
    details: Retained<NSButton>,
    remove: Retained<NSButton>,
}
pub(super) struct State {
    owner: Retained<Delegate>,
    paths: Paths,
    cloud: cloud::State,
    page_tabs: OnceCell<Retained<NSSegmentedControl>>,
    caption: OnceCell<Retained<NSTextField>>,
    footer: OnceCell<Retained<NSTextField>>,
    window: OnceCell<Retained<NSWindow>>,
    rows: RefCell<Vec<Row>>,
    notice: OnceCell<Retained<NSTextField>>,
    cancel: OnceCell<Retained<NSButton>>,
    engine: OnceCell<Retained<NSButton>>,
    language: OnceCell<Retained<NSPopUpButton>>,
    tabs: OnceCell<Retained<NSSegmentedControl>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    list: OnceCell<Retained<NSView>>,
    empty: OnceCell<Retained<NSTextField>>,
    current: OnceCell<Retained<NSTextField>>,
    summary: OnceCell<Retained<NSTextField>>,
    progress: OnceCell<Retained<NSProgressIndicator>>,
    active_model: Cell<Option<&'static str>>,
    cancelling: Cell<bool>,
    job: RefCell<Option<Job>>,
    panel: RefCell<Option<Retained<NSOpenPanel>>>,
    alert: RefCell<Option<Retained<NSAlert>>>,
    closed: Cell<bool>,
    shutting_down: Cell<bool>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = State]
    pub(super) struct Manager;
    unsafe impl NSObjectProtocol for Manager {}
    unsafe impl NSWindowDelegate for Manager {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _window: &NSWindow) -> bool {
            if self.ivars().job.borrow().is_some() {
                self.cancel_job();
                self.notice("正在取消，请等待文件清理完成后关闭。");
                false
            } else { self.window().attachedSheet().is_none() }
        }
        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _notification: &NSNotification) {
            let keep_alive = self.retain();
            if keep_alive.ivars().closed.replace(true) { return; }
            keep_alive.window().setDelegate(None);
            let owner = &keep_alive.ivars().owner;
            owner.ivars().model_manager.borrow_mut().take();
            // Carbon captures on this main thread. Discard keys captured under
            // the admission lock before allowing a later tick to start recording.
            if let Some(receiver) = owner.ivars().hotkey_rx.borrow().as_ref() {
                discard_queued_hotkeys(receiver);
            }
            owner.ivars().managing_models.set(false);
            owner.refresh();
        }
    }
    impl Manager {
        #[unsafe(method(modelPrimary:))]
        fn primary_action(&self, sender: &NSButton) {
            let Some(d) = self.definition(sender) else { return; };
            if !self.ready() { return; }
            let usable = self.ivars().owner.ivars().controller.borrow().as_ref()
                .is_some_and(|c| c.catalog.snapshot(d.id).is_ok());
            if usable && !self.ivars().owner.ivars().controller.borrow().as_ref().is_some_and(|c| d.kind.supports_language(&c.language)) {
                self.notice("此模型不支持所选语言，请更改语言后使用。"); return;
            }
            if usable {
                self.ivars().owner.with_controller(|c| self.ivars().owner.set_source(c, d.id.into()));
                self.refresh(); self.ivars().owner.refresh();
            } else { self.start(d, Operation::Download); }
        }
        #[unsafe(method(modelUpdate:))]
        fn update_action(&self, sender: &NSButton) {
            if let Some(d) = self.definition(sender) { self.start(d, Operation::Download); }
        }
        #[unsafe(method(modelDetails:))]
        fn details_action(&self, sender: &NSButton) {
            if let Some(d) = self.definition(sender) { self.show_details(d); }
        }
        #[unsafe(method(modelImport:))]
        fn import_action(&self, sender: &NSButton) {
            if let Some(d) = self.definition(sender) { self.choose_import(d); }
        }
        #[unsafe(method(modelRemove:))]
        fn remove_action(&self, sender: &NSButton) {
            if let Some(d) = self.definition(sender) { self.confirm_remove(d); }
        }
        #[unsafe(method(modelFilter:))]
        fn filter_action(&self, _sender: Option<&AnyObject>) {
            self.refresh();
            let list = self.ivars().list.get().unwrap();
            list.scrollPoint(NSPoint::new(0., list.frame().size.height));
        }
        #[unsafe(method(modelPage:))]
        fn page_action(&self, _sender: Option<&AnyObject>) {
            self.refresh();
            if self.ivars().job.borrow().is_none() {
                self.notice(if self.ivars().page_tabs.get().unwrap().selectedSegment() == 1 { "保存配置不会发送音频。点击「使用」后切换来源。" } else { "安装后点击「使用」切换识别来源。" });
            }
        }
        #[unsafe(method(modelCloudItem:))]
        fn cloud_item_action(&self, sender: &NSButton) { self.cloud_action(sender); }
        #[unsafe(method(modelCancel:))]
        fn cancel_action(&self, _sender: Option<&AnyObject>) { self.cancel_job(); }
        #[unsafe(method(modelEngine:))]
        fn engine_action(&self, _sender: Option<&AnyObject>) { self.start(&MODELS[0], Operation::Engine); }
        #[unsafe(method(modelLanguage:))]
        fn language_action(&self, _sender: Option<&AnyObject>) {
            if !self.ready() { return; }
            let language = voiceport_core::language::code(self.ivars().language.get().unwrap().indexOfSelectedItem() as usize);
            self.ivars().owner.with_controller(|c| {
                c.language = language.into();
                unsafe { NSUserDefaults::standardUserDefaults().setObject_forKey(Some(&NSString::from_str(language)), ns_string!("language")); }
            });
            self.ivars().owner.refresh();
        }
    }
);
impl Manager {
    pub(in crate::app) fn window(&self) -> &NSWindow {
        self.ivars().window.get().unwrap()
    }
    pub(super) fn bring_forward(&self) {
        self.window().makeKeyAndOrderFront(None);
    }
    fn definition(&self, button: &NSButton) -> Option<&'static Definition> {
        usize::try_from(button.tag())
            .ok()
            .and_then(|i| MODELS.get(i))
    }
    fn ready(&self) -> bool {
        let iv = self.ivars();
        !iv.closed.get()
            && !iv.shutting_down.get()
            && iv.job.borrow().is_none()
            && self.window().attachedSheet().is_none()
            && iv
                .owner
                .ivars()
                .controller
                .borrow()
                .as_ref()
                .is_some_and(|c| !c.busy() && !c.discovering())
    }
    fn notice(&self, text: &str) {
        let label = self.ivars().notice.get().unwrap();
        label.setStringValue(&NSString::from_str(text));
        label.setToolTip(Some(&NSString::from_str(text)));
    }
    fn cancel_job(&self) {
        if let Some(job) = self.ivars().job.borrow().as_ref() {
            job.cancel();
            self.ivars().cancelling.set(true);
            self.ivars().cancel.get().unwrap().setEnabled(false);
            self.notice("正在取消并清理临时文件…");
        }
    }
    pub(super) fn shutdown(&self) {
        self.close_cloud_editor();
        self.ivars().shutting_down.set(true);
        if self.ivars().job.borrow().is_some() {
            self.cancel_job();
            return;
        }
        if let Some(sheet) = self.window().attachedSheet() {
            self.window()
                .endSheet_returnCode(&sheet, NSModalResponseCancel);
            sheet.orderOut(None);
        }
        self.ivars().panel.borrow_mut().take();
        self.ivars().alert.borrow_mut().take();
        self.window().close();
    }
    fn start(&self, d: &'static Definition, operation: Operation) {
        if !self.ready() {
            return;
        }
        let active_model = if matches!(operation, Operation::Engine) {
            None
        } else {
            Some(d.id)
        };
        match Job::start(self.ivars().paths.clone(), d, operation) {
            Ok(job) => {
                self.ivars().active_model.set(active_model);
                self.ivars().cancelling.set(false);
                self.ivars().progress.get().unwrap().setDoubleValue(0.);
                *self.ivars().job.borrow_mut() = Some(job);
                self.notice("正在准备模型文件…");
            }
            Err(error) => self.notice(&error),
        }
        self.refresh();
    }
    pub(super) fn refresh(&self) {
        if self.ivars().closed.get() {
            return;
        }
        let result = self.ivars().job.borrow().as_ref().and_then(Job::poll);
        if let Some(result) = result {
            self.ivars().job.borrow_mut().take();
            self.ivars().active_model.set(None);
            self.ivars().cancelling.set(false);
            self.notice(&match result {
                Ok(message) => message,
                Err(error) => error,
            });
            self.ivars().owner.with_controller(|c| c.refresh_catalog());
        }
        if self.ivars().shutting_down.get() && self.ivars().job.borrow().is_none() {
            self.shutdown();
            return;
        }
        if let Some(job) = self.ivars().job.borrow().as_ref() {
            let progress = job
                .progress
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if progress.total > 0 {
                self.ivars().progress.get().unwrap().setDoubleValue(
                    (progress.completed as f64 * 100. / progress.total as f64).clamp(0., 100.),
                );
            }
            if !progress.message.is_empty() && !self.ivars().cancelling.get() {
                self.notice(&if progress.total > 0 {
                    format!(
                        "{} · {:.0}% · {:.1} / {:.1} MB",
                        progress.message,
                        progress.completed as f64 * 100. / progress.total as f64,
                        progress.completed as f64 / 1_000_000.,
                        progress.total as f64 / 1_000_000.
                    )
                } else {
                    progress.message
                });
            }
        }
        let ready = self.ready();
        let cloud_page = self.ivars().page_tabs.get().unwrap().selectedSegment() == 1;
        self.ivars().scroll.get().unwrap().setHidden(cloud_page);
        self.ivars().tabs.get().unwrap().setHidden(cloud_page);
        self.ivars().engine.get().unwrap().setHidden(cloud_page);
        self.ivars()
            .cloud
            .scroll
            .get()
            .unwrap()
            .setHidden(!cloud_page);
        self.ivars().page_tabs.get().unwrap().setEnabled(ready);
        self.ivars()
            .caption
            .get()
            .unwrap()
            .setStringValue(&NSString::from_str(if cloud_page {
                "开始听写后，音频才会发送至所选服务。"
            } else {
                "本机识别，音频留在这台电脑。"
            }));
        self.ivars()
            .footer
            .get()
            .unwrap()
            .setStringValue(&NSString::from_str(if cloud_page {
                "凭据保存在系统钥匙串；连接未检测，识别质量尚未测量。"
            } else {
                "模型来源与许可见详情；更新失败保留旧副本。识别质量尚未测量。"
            }));
        let engine_ready = crate::local_models::validate_runtime(
            &self.ivars().paths.runtime(),
            std::env::consts::ARCH,
        )
        .is_ok();
        let engine = self.ivars().engine.get().unwrap();
        engine.setEnabled(ready && !engine_ready);
        engine.setTitle(&NSString::from_str(if engine_ready {
            "识别引擎已安装"
        } else {
            "安装识别引擎"
        }));
        let slot = self.ivars().owner.ivars().controller.borrow();
        let Some(c) = slot.as_ref() else {
            return;
        };
        if cloud_page {
            self.render_cloud(&c.source_id, ready && self.cloud_store().is_ok());
        }
        let current = c.catalog.entries.iter().find(|e| e.id == c.source_id);
        self.ivars()
            .current
            .get()
            .unwrap()
            .setStringValue(&NSString::from_str(&format!(
                "当前来源：{}",
                current.map(|e| e.title.as_str()).unwrap_or("尚不可用")
            )));
        let installed_only = self.ivars().tabs.get().unwrap().selectedSegment() == 1;
        let rows = self.ivars().rows.borrow();
        let visible = rows
            .iter()
            .filter(|row| {
                !installed_only
                    || c.catalog.snapshot(row.definition.id).is_ok()
                    || self
                        .ivars()
                        .paths
                        .model(row.definition)
                        .symlink_metadata()
                        .is_ok()
            })
            .count();
        let list = self.ivars().list.get().unwrap();
        let list_height = (visible as f64 * ROW_HEIGHT)
            .max(self.ivars().scroll.get().unwrap().contentSize().height);
        let old_size = list.frame().size;
        if old_size.height != list_height {
            list.setFrameSize(NSSize::new(old_size.width, list_height));
        }
        let empty = self.ivars().empty.get().unwrap();
        empty.setFrameOrigin(NSPoint::new(28., list_height - 104.));
        empty.setHidden(visible != 0);
        let summary = self.ivars().summary.get().unwrap();
        summary.setStringValue(&NSString::from_str(&if cloud_page {
            "添加或选择服务".into()
        } else if installed_only {
            format!("{visible} 款已安装")
        } else {
            format!("{visible} 款可选")
        }));
        let summary_frame = summary.frame();
        summary.setFrameOrigin(NSPoint::new(
            if cloud_page { 24. } else { 206. },
            summary_frame.origin.y,
        ));
        let mut position = 0;
        for row in rows.iter() {
            let own = self
                .ivars()
                .paths
                .model(row.definition)
                .symlink_metadata()
                .is_ok();
            let source = c.catalog.snapshot(row.definition.id);
            let usable = source.is_ok();
            let selected = c.source_id == row.definition.id;
            let hidden = installed_only && !usable && !own;
            row.view.setHidden(hidden);
            if !hidden {
                row.view.setFrameOrigin(NSPoint::new(
                    0.,
                    list_height - (position + 1) as f64 * ROW_HEIGHT,
                ));
                position += 1;
            }
            let installation = self.ivars().paths.installation(row.definition);
            let working = self.ivars().active_model.get() == Some(row.definition.id);
            let status = if working {
                if self.ivars().cancelling.get() {
                    "正在取消…"
                } else {
                    "正在处理文件…"
                }
            } else if usable {
                if installation == Installation::Legacy {
                    if selected {
                        "✓ 使用中 · 可更新"
                    } else {
                        "旧副本可用 · 可更新"
                    }
                } else if selected {
                    "✓ 使用中"
                } else if own {
                    "已安装"
                } else {
                    "已安装 · 复用 Orca"
                }
            } else if own {
                "文件或引擎不可用"
            } else {
                "未安装"
            };
            let compatible = row.definition.kind.supports_language(&c.language);
            row.compatibility
                .setStringValue(&NSString::from_str(if compatible {
                    ""
                } else {
                    "不支持所选语言"
                }));
            let compatibility_color = if compatible {
                NSColor::secondaryLabelColor()
            } else {
                NSColor::systemOrangeColor()
            };
            row.compatibility.setTextColor(Some(&compatibility_color));
            row.status.setStringValue(&NSString::from_str(status));
            let status_color = if selected && usable {
                NSColor::controlAccentColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            row.status.setTextColor(Some(&status_color));
            let border_color = if selected && usable {
                NSColor::controlAccentColor().colorWithAlphaComponent(0.55)
            } else {
                NSColor::separatorColor()
            };
            row.surface.setBorderColor(&border_color);
            row.surface
                .setBorderWidth(if selected && usable { 1. } else { 0.5 });
            row.import.setHidden(own || usable);
            row.remove.setHidden(!own);
            row.primary.setTitle(&NSString::from_str(if usable {
                if selected { "使用中" } else { "使用" }
            } else if own {
                "更新 / 重装"
            } else {
                "下载"
            }));
            row.primary
                .setEnabled(ready && !(usable && selected) && (!usable || compatible));
            row.import.setEnabled(ready && !own);
            row.update.setHidden(!own || !usable);
            row.update.setTitle(&NSString::from_str(
                if installation == Installation::Legacy {
                    "更新"
                } else {
                    "重装"
                },
            ));
            row.update.setEnabled(ready && own && usable);
            row.update.setToolTip(Some(ns_string!(
                "校验完整的新副本后再替换；失败或取消会保留旧副本。"
            )));
            row.details.setEnabled(ready);
            row.remove.setEnabled(ready && own && !selected);
            row.remove.setToolTip(Some(&NSString::from_str(if selected {
                "请先选择其他转写来源，再删除此模型。"
            } else {
                "仅删除 VoicePort 的副本；不会改动 Orca 文件。"
            })));
            row.status
                .setToolTip(Some(&NSString::from_str(&source.err().unwrap_or_default())));
        }
        self.ivars()
            .cancel
            .get()
            .unwrap()
            .setEnabled(self.ivars().job.borrow().is_some() && !self.ivars().cancelling.get());
        self.ivars()
            .progress
            .get()
            .unwrap()
            .setHidden(self.ivars().job.borrow().is_none());
        self.ivars().language.get().unwrap().setEnabled(ready);
        self.ivars()
            .cancel
            .get()
            .unwrap()
            .setHidden(self.ivars().job.borrow().is_none());
        let index = voiceport_core::language::index(&c.language) as isize;
        let language = self.ivars().language.get().unwrap();
        if language.indexOfSelectedItem() != index {
            language.selectItemAtIndex(index);
        }
    }
    fn show_details(&self, d: &'static Definition) {
        if !self.ready() {
            return;
        }
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(d.name));
        alert.setInformativeText(ns_string!(
            "固定 ONNX int8 文件集。下载和导入均校验完整大小与 SHA-256；质量未测量。"
        ));
        alert.addButtonWithTitle(ns_string!("完成"));
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(self.mtm()), rect(0., 0., 540., 300.));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let text =
            NSTextView::initWithFrame(NSTextView::alloc(self.mtm()), rect(0., 0., 520., 300.));
        text.setEditable(false);
        text.setSelectable(true);
        text.setFont(Some(&NSFont::systemFontOfSize(12.)));
        text.setTextColor(Some(&NSColor::textColor()));
        text.setTextContainerInset(NSSize::new(6., 6.));
        let mut details = format!(
            "来源 ID：{}\n支持语言：{}\n模型下载：{}（{} bytes）\n共享引擎：sherpa-onnx {}，另计 {}\n\n",
            d.id,
            d.languages,
            model_size(d.bytes()),
            d.bytes(),
            crate::local_models::RUNTIME_VERSION,
            model_size(crate::model_library::runtime_bytes())
        );
        for asset in d.assets {
            details.push_str(&format!(
                "{}\n{} bytes\nSHA-256：{}\n{}\n\n",
                asset.name, asset.bytes, asset.sha256, asset.url
            ));
        }
        for (name, material) in crate::model_library::bundled_notices(d.kind) {
            details.push_str(&format!("{name}\n{material}\n\n"));
        }
        text.setString(&NSString::from_str(&details));
        text.setAccessibilityLabel(Some(ns_string!("模型固定版本与许可详情")));
        scroll.setDocumentView(Some(&text));
        alert.setAccessoryView(Some(&scroll));
        let manager = self.retain();
        let callback = RcBlock::new(move |_| {
            manager.ivars().alert.borrow_mut().take();
            manager.refresh();
        });
        *self.ivars().alert.borrow_mut() = Some(alert.clone());
        alert.beginSheetModalForWindow_completionHandler(self.window(), Some(&callback));
    }
    fn choose_import(&self, d: &'static Definition) {
        if !self.ready() {
            return;
        }
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setCanChooseFiles(false);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(false);
        panel.setMessage(Some(&NSString::from_str(&format!(
            "选择 {} 官方 ONNX 模型文件夹。文件名和校验值必须匹配；导入会复制到 VoicePort。",
            d.name
        ))));
        let manager = self.retain();
        let callback = RcBlock::new(move |response| {
            let chosen = manager
                .ivars()
                .panel
                .borrow_mut()
                .take()
                .and_then(|p| p.URL())
                .and_then(|url| url.path())
                .map(|p| PathBuf::from(p.to_string()));
            if response == NSModalResponseOK
                && let Some(path) = chosen
            {
                manager.start(d, Operation::Import(path));
            }
            manager.refresh();
        });
        *self.ivars().panel.borrow_mut() = Some(panel.clone());
        panel.beginSheetModalForWindow_completionHandler(self.window(), &callback);
    }
    fn confirm_remove(&self, d: &'static Definition) {
        if !self.ready() {
            return;
        }
        let selected = self
            .ivars()
            .owner
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .is_some_and(|c| c.source_id == d.id);
        if selected {
            self.notice("请先选择其他转写来源，再删除此模型。");
            return;
        }
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(&format!("删除 {}？", d.name)));
        alert.setInformativeText(ns_string!(
            "仅删除 VoicePort 中的模型副本，之后可重新下载。Orca 的模型和设置不受影响。"
        ));
        alert.addButtonWithTitle(ns_string!("取消"));
        alert.addButtonWithTitle(ns_string!("删除模型"));
        let manager = self.retain();
        let callback = RcBlock::new(move |response| {
            manager.ivars().alert.borrow_mut().take();
            if response == NSAlertSecondButtonReturn {
                manager.start(d, Operation::Remove);
            }
            manager.refresh();
        });
        *self.ivars().alert.borrow_mut() = Some(alert.clone());
        alert.beginSheetModalForWindow_completionHandler(self.window(), Some(&callback));
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
        view.addSubview(&b);
        b
    }
}

fn discard_queued_hotkeys(receiver: &std::sync::mpsc::Receiver<crate::hotkey::HotKeyEvent>) {
    while receiver.try_recv().is_ok() {}
}

fn page_height(height: f64) -> f64 {
    height.max(480.)
}

fn model_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.2} GB", bytes as f64 / 1_000_000_000.)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_000_000.)
    }
}

pub(super) fn show(owner: &Delegate) -> Result<(), String> {
    let paths = Paths::system()?;
    let manager = Manager::alloc(owner.mtm()).set_ivars(State {
        owner: owner.retain(),
        paths,
        cloud: cloud::State::new()?,
        page_tabs: OnceCell::new(),
        caption: OnceCell::new(),
        footer: OnceCell::new(),
        window: OnceCell::new(),
        rows: RefCell::new(vec![]),
        notice: OnceCell::new(),
        cancel: OnceCell::new(),
        engine: OnceCell::new(),
        language: OnceCell::new(),
        tabs: OnceCell::new(),
        scroll: OnceCell::new(),
        list: OnceCell::new(),
        empty: OnceCell::new(),
        current: OnceCell::new(),
        summary: OnceCell::new(),
        progress: OnceCell::new(),
        active_model: Cell::new(None),
        cancelling: Cell::new(false),
        job: RefCell::new(None),
        panel: RefCell::new(None),
        alert: RefCell::new(None),
        closed: Cell::new(false),
        shutting_down: Cell::new(false),
    });
    let manager: Retained<Manager> = unsafe { msg_send![super(manager), init] };
    presentation::build(&manager, owner);
    owner.ivars().managing_models.set(true);
    *owner.ivars().model_manager.borrow_mut() = Some(manager.clone());
    manager.refresh();
    manager.window().center();
    manager.bring_forward();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_queued_under_model_lock_do_not_survive_close() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let event = || crate::hotkey::HotKeyEvent {
            at: std::time::Instant::now(),
            application: None,
            target: None,
        };
        sender.send(event()).unwrap();
        sender.send(event()).unwrap();
        discard_queued_hotkeys(&receiver);
        assert!(
            receiver.try_recv().is_err(),
            "locked-period keys must not reach the next tick"
        );
        sender.send(event()).unwrap();
        assert!(
            receiver.try_recv().is_ok(),
            "a fresh key after close remains usable"
        );
    }

    #[test]
    fn short_screen_uses_scrollable_page_without_filter_overlap() {
        for content_height in [1., 383., 503., 623., 740.] {
            let layout = page_height(content_height);
            let filter_bottom = layout - 158.;
            let list = presentation::list_frame(620., layout);
            let list_top = list.origin.y + list.size.height;
            assert!(
                list_top <= filter_bottom - 12.,
                "screen content height {content_height}"
            );
            assert!(list.origin.y >= 76., "list must not cover the task footer");
            assert!(
                list.size.height >= ROW_HEIGHT * 2.,
                "short screens retain useful list space"
            );
            assert!(layout >= content_height);
        }
    }
}
