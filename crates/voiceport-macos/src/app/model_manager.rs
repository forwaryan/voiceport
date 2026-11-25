//! Native model library. One retained window owns installation and its admission lock.
use super::*;
use crate::model_library::{Definition, Installation, Job, MODELS, Operation, Paths};
use block2::RcBlock;
use local_state::{LocalRefresh, ModelView, PrimaryAction, RefreshReason};
use objc2::Message;
use std::{os::unix::fs::MetadataExt, path::PathBuf, time::Instant};
mod browse;
mod cloud;
mod local_state;
use browse::{Browse, Order, Preview};
mod presentation;
use presentation::ROW_HEIGHT;

struct Row {
    index: usize,
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
#[derive(PartialEq, Eq)]
struct LocalView {
    rows: Vec<ModelView>,
    current_title: String,
    source_id: String,
    language: String,
    browse: Browse,
    ready: bool,
    working: Option<&'static str>,
    cancelling: bool,
    engine_ready: bool,
}

pub(super) struct State {
    owner: Retained<Delegate>,
    paths: Paths,
    orca_root: Option<PathBuf>,
    local_refresh: RefCell<LocalRefresh>,
    local_view: RefCell<Option<LocalView>>,
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
    browse_bar: OnceCell<Retained<NSView>>,
    browse_language: OnceCell<Retained<NSPopUpButton>>,
    browse_preview: OnceCell<Retained<NSPopUpButton>>,
    browse_order: OnceCell<Retained<NSPopUpButton>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    list: OnceCell<Retained<NSView>>,
    empty: OnceCell<Retained<NSTextField>>,
    current: OnceCell<Retained<NSTextField>>,
    summary: OnceCell<Retained<NSTextField>>,
    progress: OnceCell<Retained<NSProgressIndicator>>,
    progress_display: RefCell<presentation::ProgressDisplay>,
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
            presentation::render_progress(&keep_alive, presentation::ProgressDisplay::Hidden);
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
            let Some(shown) = self.model_view(d).map(|view| view.primary) else { return; };
            if !self.recheck_local() { return; }
            if self.model_view(d).is_none_or(|view| view.primary != shown) {
                self.notice("模型状态已更新，请确认后重试。");
                return;
            }
            match shown {
                PrimaryAction::Use => {
                    let owner = &self.ivars().owner;
                    if !owner.ivars().controller.borrow().as_ref().is_some_and(|c| d.kind.supports_language(&c.language)) {
                        self.notice("此模型不支持所选语言，请更改语言后使用。");
                        return;
                    }
                    owner.with_controller(|c| owner.set_source(c, d.id.into()));
                    self.refresh();
                    owner.refresh();
                }
                PrimaryAction::Download | PrimaryAction::Reinstall => self.start(d, Operation::Download),
                PrimaryAction::AwaitCatalog => {}
            }
        }
        #[unsafe(method(modelUpdate:))]
        fn update_action(&self, sender: &NSButton) {
            let Some(d) = self.definition(sender) else { return; };
            if self.recheck_local() {
                if self.model_view(d).is_some_and(|view| view.owned() && view.primary == PrimaryAction::Use) {
                    self.start(d, Operation::Download);
                } else {
                    self.notice("模型副本状态已更新，请确认后重试。");
                }
            }
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
        fn engine_action(&self, _sender: Option<&AnyObject>) {
            if self.recheck_local() {
                if self.ivars().local_view.borrow().as_ref().is_some_and(|view| view.engine_ready) {
                    self.notice("识别引擎已安装。");
                } else {
                    self.start(&MODELS[0], Operation::Engine);
                }
            }
        }
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
        self.refresh_for(RefreshReason::Open);
        if !self.ivars().closed.get() {
            self.window().makeKeyAndOrderFront(None);
        }
    }
    fn cloud_page(&self) -> bool {
        self.ivars().page_tabs.get().unwrap().selectedSegment() == 1
    }
    fn recheck_local(&self) -> bool {
        if !self.ready() || self.cloud_page() {
            return false;
        }
        self.refresh_for(RefreshReason::Action);
        let ready = self.ready();
        if !ready {
            self.notice("正在刷新模型来源，检查完成后请重试。");
        }
        ready
    }
    fn model_view(&self, d: &Definition) -> Option<std::cell::Ref<'_, ModelView>> {
        let index = MODELS.iter().position(|definition| definition.id == d.id)?;
        std::cell::Ref::filter_map(self.ivars().local_view.borrow(), |view| {
            view.as_ref().map(|view| &view.rows[index])
        })
        .ok()
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
            presentation::render_progress(
                self,
                presentation::ProgressDisplay::from_job(None, true),
            );
            self.ivars().cancel.get().unwrap().setEnabled(false);
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
                *self.ivars().job.borrow_mut() = Some(job);
            }
            Err(error) => self.notice(&error),
        }
        self.refresh();
    }
    pub(super) fn refresh(&self) {
        self.refresh_for(RefreshReason::Tick);
    }
    fn refresh_for(&self, mut reason: RefreshReason) {
        if self.ivars().closed.get() {
            return;
        }
        let result = self.ivars().job.borrow().as_ref().and_then(Job::poll);
        if let Some(result) = result {
            self.ivars().job.borrow_mut().take();
            self.ivars().active_model.set(None);
            self.ivars().cancelling.set(false);
            reason = RefreshReason::JobFinished;
            self.notice(&match result {
                Ok(message) => message,
                Err(error) => error,
            });
            self.ivars().owner.with_controller(|c| c.refresh_catalog());
        }
        self.refresh_progress();
        if self.ivars().shutting_down.get() && self.ivars().job.borrow().is_none() {
            self.shutdown();
            return;
        }

        let cloud_page = self.cloud_page();
        let local_visible =
            !cloud_page && (reason == RefreshReason::Open || self.window().isVisible());
        let sampled = self.ivars().local_refresh.borrow_mut().sample(
            Instant::now(),
            local_visible,
            reason,
            || {
                local_state::LocalDisk::probe(
                    &self.ivars().paths,
                    self.ivars().orca_root.as_deref(),
                )
            },
        );
        if sampled && self.ready() {
            let disk = self.ivars().local_refresh.borrow();
            self.ivars().owner.with_controller(|c| {
                if disk
                    .disk
                    .as_ref()
                    .unwrap()
                    .catalog_differs(&c.catalog, &c.source_id)
                {
                    c.refresh_catalog();
                }
            });
        }
        let ready = self.ready();
        self.ivars().scroll.get().unwrap().setHidden(cloud_page);
        self.ivars().tabs.get().unwrap().setHidden(cloud_page);
        self.ivars().browse_bar.get().unwrap().setHidden(cloud_page);
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
        let slot = self.ivars().owner.ivars().controller.borrow();
        let Some(c) = slot.as_ref() else {
            return;
        };
        self.ivars().language.get().unwrap().setEnabled(ready);
        let index = voiceport_core::language::index(&c.language) as isize;
        let language = self.ivars().language.get().unwrap();
        if language.indexOfSelectedItem() != index {
            language.selectItemAtIndex(index);
        }
        let current = c.catalog.entries.iter().find(|e| e.id == c.source_id);
        let title = current.map(|e| e.title.as_str()).unwrap_or("尚不可用");
        if cloud_page {
            self.ivars().local_view.borrow_mut().take();
            self.set_current(title);
            self.set_summary("添加或选择服务", 24.);
            self.render_cloud(&c.source_id, ready && self.cloud_store().is_ok());
            return;
        }

        if !local_visible {
            self.ivars().local_view.borrow_mut().take();
            return;
        }
        let disk = self.ivars().local_refresh.borrow();
        let disk = disk.disk.as_ref().unwrap();
        let rows: Vec<_> = MODELS
            .iter()
            .zip(&disk.rows)
            .map(|(definition, row)| row.view(&c.catalog, definition.id))
            .collect();
        let current_title = if let Some(row) = MODELS
            .iter()
            .zip(&rows)
            .find_map(|(definition, row)| (definition.id == c.source_id).then_some(row))
        {
            let title = crate::local_models::title(&c.source_id);
            match row.primary {
                PrimaryAction::Use => title.into(),
                PrimaryAction::AwaitCatalog => format!("{title}（正在刷新）"),
                _ => format!("{title}（不可用）"),
            }
        } else {
            title.into()
        };
        let view = LocalView {
            rows,
            current_title,
            source_id: c.source_id.clone(),
            language: c.language.clone(),
            browse: Browse {
                installed_only: self.ivars().tabs.get().unwrap().selectedSegment() == 1,
                language: voiceport_core::language::code(
                    self.ivars()
                        .browse_language
                        .get()
                        .unwrap()
                        .indexOfSelectedItem() as usize,
                ),
                preview: Preview::from_index(
                    self.ivars()
                        .browse_preview
                        .get()
                        .unwrap()
                        .indexOfSelectedItem(),
                ),
                order: Order::from_index(
                    self.ivars()
                        .browse_order
                        .get()
                        .unwrap()
                        .indexOfSelectedItem(),
                ),
            },
            ready,
            working: self.ivars().active_model.get(),
            cancelling: self.ivars().cancelling.get(),
            engine_ready: disk.engine_ready,
        };
        if self.ivars().local_view.borrow().as_ref() != Some(&view) {
            self.render_local(&view);
            self.ivars().local_view.replace(Some(view));
        }
    }
    fn refresh_progress(&self) {
        let iv = self.ivars();
        let cancelling = iv.cancelling.get();
        let job = iv.job.borrow();
        let progress = job.as_ref().filter(|_| !cancelling).map(Job::progress);
        presentation::render_progress(
            self,
            presentation::ProgressDisplay::from_job(progress, cancelling),
        );
        let cancel = iv.cancel.get().unwrap();
        cancel.setHidden(job.is_none());
        cancel.setEnabled(job.is_some() && !cancelling);
    }
    fn set_current(&self, title: &str) {
        self.ivars()
            .current
            .get()
            .unwrap()
            .setStringValue(&NSString::from_str(&format!("当前来源：{title}")));
    }
    fn set_summary(&self, text: &str, x: f64) {
        let summary = self.ivars().summary.get().unwrap();
        summary.setStringValue(&NSString::from_str(text));
        summary.setFrameOrigin(NSPoint::new(x, summary.frame().origin.y));
    }
    fn render_local(&self, view: &LocalView) {
        self.set_current(&view.current_title);
        let engine = self.ivars().engine.get().unwrap();
        engine.setEnabled(view.ready && !view.engine_ready);
        engine.setTitle(if view.engine_ready {
            ns_string!("识别引擎已安装")
        } else {
            ns_string!("安装识别引擎")
        });
        let positions = view.browse.positions(&view.rows);
        let visible = positions.iter().flatten().count();
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
        self.set_summary(&format!("{visible} 款符合条件"), 206.);
        for row in self.ivars().rows.borrow().iter() {
            let position = positions[row.index];
            row.view.setHidden(position.is_none());
            let Some(position) = position else {
                continue;
            };
            row.view.setFrameOrigin(NSPoint::new(
                0.,
                list_height - (position + 1) as f64 * ROW_HEIGHT,
            ));
            let model = &view.rows[row.index];
            let own = model.owned();
            let usable = model.primary == PrimaryAction::Use;
            let selected = view.source_id == row.definition.id;
            let installation = model.installation;
            let working = view.working == Some(row.definition.id);
            let status = if working {
                if view.cancelling {
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
            } else if model.primary == PrimaryAction::AwaitCatalog {
                "正在刷新来源…"
            } else if own {
                "文件或引擎不可用"
            } else {
                "未安装"
            };
            let compatible = row.definition.kind.supports_language(&view.language);
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
            row.import
                .setHidden(model.primary != PrimaryAction::Download);
            row.remove.setHidden(!own);
            row.primary
                .setTitle(&NSString::from_str(match model.primary {
                    PrimaryAction::Use if selected => "使用中",
                    PrimaryAction::Use => "使用",
                    PrimaryAction::Download => "下载",
                    PrimaryAction::Reinstall => "更新 / 重装",
                    PrimaryAction::AwaitCatalog => "正在刷新…",
                }));
            row.primary.setEnabled(
                view.ready
                    && match model.primary {
                        PrimaryAction::Use => !selected && compatible,
                        PrimaryAction::Download | PrimaryAction::Reinstall => true,
                        PrimaryAction::AwaitCatalog => false,
                    },
            );
            row.import
                .setEnabled(view.ready && model.primary == PrimaryAction::Download);
            row.update.setHidden(!own || !usable);
            row.update.setTitle(&NSString::from_str(
                if installation == Installation::Legacy {
                    "更新"
                } else {
                    "重装"
                },
            ));
            row.update.setEnabled(view.ready && own && usable);
            row.update.setToolTip(Some(ns_string!(
                "校验完整的新副本后再替换；失败或取消会保留旧副本。"
            )));
            row.details.setEnabled(view.ready);
            row.remove.setEnabled(view.ready && own && !selected);
            row.remove.setToolTip(Some(&NSString::from_str(if selected {
                "请先选择其他转写来源，再删除此模型。"
            } else {
                "仅删除 VoicePort 的副本；不会改动 Orca 文件。"
            })));
            row.status
                .setToolTip(Some(&NSString::from_str(&model.reason)));
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
    fn can_import(&self, d: &Definition) -> bool {
        if !self.recheck_local() {
            return false;
        }
        if self
            .model_view(d)
            .is_some_and(|view| view.primary == PrimaryAction::Download)
        {
            return true;
        }
        self.notice("模型状态已更新，请确认后重试。");
        false
    }
    fn choose_import(&self, d: &'static Definition) {
        if !self.can_import(d) {
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
                && manager.can_import(d)
            {
                manager.start(d, Operation::Import(path));
            }
            manager.refresh();
        });
        *self.ivars().panel.borrow_mut() = Some(panel.clone());
        panel.beginSheetModalForWindow_completionHandler(self.window(), &callback);
    }
    fn removable_model(&self, d: &Definition) -> Option<(u64, u64)> {
        if !self.ready() || self.cloud_page() {
            return None;
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
            return None;
        }
        let path = self.ivars().paths.model(d);
        match path.symlink_metadata() {
            Ok(metadata) if metadata.is_dir() => Some((metadata.dev(), metadata.ino())),
            _ => {
                self.ivars().local_refresh.borrow_mut().invalidate();
                self.refresh();
                self.notice("VoicePort 模型副本已变更或无法删除，请检查目录状态。");
                None
            }
        }
    }
    fn confirm_remove(&self, d: &'static Definition) {
        let Some(identity) = self.removable_model(d) else {
            return;
        };
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
            if response == NSAlertSecondButtonReturn
                && let Some(current) = manager.removable_model(d)
            {
                if current == identity {
                    manager.start(d, Operation::Remove);
                } else {
                    manager.ivars().local_refresh.borrow_mut().invalidate();
                    manager.notice("VoicePort 模型副本已替换，请重新确认后删除。");
                }
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
    height.max(516.)
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
        orca_root: crate::local_models::orca_model_root(),
        local_refresh: RefCell::default(),
        local_view: RefCell::default(),
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
        browse_bar: OnceCell::new(),
        browse_language: OnceCell::new(),
        browse_preview: OnceCell::new(),
        browse_order: OnceCell::new(),
        scroll: OnceCell::new(),
        list: OnceCell::new(),
        empty: OnceCell::new(),
        current: OnceCell::new(),
        summary: OnceCell::new(),
        progress: OnceCell::new(),
        progress_display: RefCell::new(presentation::ProgressDisplay::Hidden),
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
        let event = || {
            crate::hotkey::HotKeyEvent::Press(crate::hotkey::KeyPress {
                stroke: crate::hotkey::Stroke::fixture_stroke(1),
                at: std::time::Instant::now(),
                application: None,
                target: None,
            })
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
            let filter_bottom = layout - 194.;
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
