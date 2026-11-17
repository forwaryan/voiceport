//! Cloud page uses the same source drafts, Keychain transaction and controller as plugins.
use super::*;
use crate::{
    cloud_models::{self, Preset},
    plugins::{Source, Store},
};

#[derive(Clone)]
enum Action {
    Add(Preset),
    Edit(usize),
    Remove(String),
    Use(String),
}
pub(super) struct State {
    path: PathBuf,
    store: RefCell<Result<Store, String>>,
    pub(super) scroll: OnceCell<Retained<NSScrollView>>,
    editor: RefCell<Option<Retained<source_manager::CloudEditor>>>,
    actions: RefCell<Vec<Action>>,
    writing: Cell<bool>,
    rendered: RefCell<String>,
}
impl State {
    pub(super) fn new() -> Result<Self, String> {
        let path = crate::plugins::default_path()?;
        Ok(Self {
            store: RefCell::new(Store::load(&path)),
            path,
            scroll: OnceCell::new(),
            editor: RefCell::new(None),
            actions: RefCell::new(vec![]),
            writing: Cell::new(false),
            rendered: RefCell::new(String::new()),
        })
    }
}
impl Manager {
    pub(in crate::app) fn owner(&self) -> &Delegate {
        &self.ivars().owner
    }
    pub(in crate::app) fn cloud_path(&self) -> &std::path::Path {
        &self.ivars().cloud.path
    }
    pub(in crate::app) fn cloud_store(&self) -> Result<Store, String> {
        self.ivars().cloud.store.borrow().clone()
    }
    pub(in crate::app) fn cloud_writing(&self) -> bool {
        self.ivars().cloud.writing.get()
    }
    pub(in crate::app) fn cloud_ready(&self) -> bool {
        !self.ivars().closed.get()
            && !self.ivars().shutting_down.get()
            && self.ivars().job.borrow().is_none()
            && !self.cloud_writing()
            && self.ivars().cloud.store.borrow().is_ok()
            && self
                .owner()
                .ivars()
                .controller
                .borrow()
                .as_ref()
                .is_some_and(|c| !c.busy() && !c.discovering())
    }
    pub(in crate::app) fn cloud_begin_write(&self) -> bool {
        self.cloud_ready() && !self.ivars().cloud.writing.replace(true)
    }
    pub(in crate::app) fn cloud_editor_finished(&self) {
        self.ivars().cloud.editor.borrow_mut().take();
        self.refresh();
    }
    pub(in crate::app) fn cloud_reconcile(&self, result: Result<(), String>) {
        // A failed fsync/credential cleanup may follow a committed store. Always reload.
        let loaded = Store::load(self.cloud_path());
        let message = match (&result, &loaded) {
            (_, Err(error)) => format!("无法重新读取配置：{error} 请关闭后重开模型窗口。"),
            (Err(error), _) => format!("操作未完全完成：{error}"),
            (Ok(()), _) => "配置已保存；未联网、未录音。点击「使用」后才切换来源。".into(),
        };
        *self.ivars().cloud.store.borrow_mut() = loaded;
        self.ivars().cloud.writing.set(false);
        self.owner().with_controller(|c| c.refresh_catalog());
        self.notice(&message);
        self.ivars().cloud.rendered.borrow_mut().clear();
    }
    pub(super) fn close_cloud_editor(&self) {
        let editor = self.ivars().cloud.editor.borrow().clone();
        if let Some(editor) = editor {
            editor.cancel();
        }
    }
    pub(super) fn cloud_action(&self, sender: &NSButton) {
        if !self.ready() || !self.cloud_ready() {
            return;
        }
        let action = usize::try_from(sender.tag())
            .ok()
            .and_then(|i| self.ivars().cloud.actions.borrow().get(i).cloned());
        match action {
            Some(Action::Add(preset)) => {
                let editor = source_manager::show_cloud(self, None, preset);
                *self.ivars().cloud.editor.borrow_mut() = Some(editor);
            }
            Some(Action::Edit(index)) => {
                let editor = source_manager::show_cloud(self, Some(index), Preset::Compatible);
                *self.ivars().cloud.editor.borrow_mut() = Some(editor);
            }
            Some(Action::Use(id)) => {
                self.owner()
                    .with_controller(|c| self.owner().set_source(c, id));
                self.refresh();
                self.owner().refresh();
            }
            Some(Action::Remove(id)) => {
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(ns_string!("移除此云端配置？"));
                alert.setInformativeText(ns_string!(
                    "同时移除其独立密钥。若正在使用此来源，保留不可用选择，直到你主动选择其他来源。"
                ));
                alert.addButtonWithTitle(ns_string!("取消"));
                alert.addButtonWithTitle(ns_string!("移除"));
                let manager = self.retain();
                let callback = RcBlock::new(move |response| {
                    manager.ivars().alert.borrow_mut().take();
                    if response == NSAlertSecondButtonReturn && manager.cloud_begin_write() {
                        let result = manager.cloud_store().and_then(|mut store| {
                            crate::source_service::remove(&mut store, manager.cloud_path(), &id)
                        });
                        manager.cloud_reconcile(result);
                    }
                    manager.refresh();
                });
                *self.ivars().alert.borrow_mut() = Some(alert.clone());
                alert.beginSheetModalForWindow_completionHandler(self.window(), Some(&callback));
            }
            None => {}
        }
    }
    fn cloud_button(
        &self,
        view: &NSView,
        title: &str,
        action: Action,
        frame: NSRect,
        ready: bool,
        id: &str,
    ) {
        let button = self.button(view, title, sel!(modelCloudItem:), frame);
        button.setTag(self.ivars().cloud.actions.borrow().len() as isize);
        button.setEnabled(ready);
        button.setAccessibilityIdentifier(Some(&NSString::from_str(id)));
        self.ivars().cloud.actions.borrow_mut().push(action);
    }
    pub(super) fn render_cloud(&self, selected: &str, ready: bool) {
        let state = &self.ivars().cloud;
        let store = state.store.borrow();
        let fingerprint = format!("{store:?}/{selected}/{ready}");
        if *state.rendered.borrow() == fingerprint {
            return;
        }
        *state.rendered.borrow_mut() = fingerprint;
        state.actions.borrow_mut().clear();
        let scroll = state.scroll.get().unwrap();
        let width = scroll.contentSize().width;
        let sources: Vec<(usize, &Source)> = store
            .as_ref()
            .map(|s| {
                s.sources
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| cloud_models::is_http(s))
                    .collect()
            })
            .unwrap_or_default();
        let height = ((sources.len() + 2) as f64 * 116. + 48.).max(scroll.contentSize().height);
        let view = NSView::initWithFrame(NSView::alloc(self.mtm()), rect(0., 0., width, height));
        for (i, preset) in [Preset::GptTranscribe, Preset::Compatible]
            .into_iter()
            .enumerate()
        {
            let y = height - (i + 1) as f64 * 116.;
            self.owner()
                .surface(&view, rect(4., y + 4., width - 8., 108.));
            self.owner().label(
                &view,
                preset.name(),
                rect(18., y + 72., width - 220., 26.),
                17.,
            );
            self.owner().caption(
                &view,
                preset.description(),
                rect(18., y + 27., width - 220., 42.),
            );
            self.owner().caption(
                &view,
                "质量：未测量",
                rect(width - 198., y + 72., 178., 24.),
            );
            self.cloud_button(
                &view,
                if preset == Preset::GptTranscribe {
                    "添加 API 配置…"
                } else {
                    "设置兼容服务…"
                },
                Action::Add(preset),
                rect(width - 172., y + 28., 150., 30.),
                ready,
                if i == 0 {
                    "voiceport.cloud.add-gpt"
                } else {
                    "voiceport.cloud.add-compatible"
                },
            );
        }
        for (row, (index, source)) in sources.iter().enumerate() {
            let y = height - (row + 3) as f64 * 116.;
            self.owner()
                .surface(&view, rect(4., y + 4., width - 8., 108.));
            self.owner().label(
                &view,
                &source.name,
                rect(18., y + 76., width - 260., 23.),
                15.,
            );
            let endpoint = cloud_models::destination(source);
            self.owner().caption(
                &view,
                &format!("音频目的地：{endpoint}"),
                rect(18., y + 48., width - 260., 26.),
            );
            let valid = store
                .as_ref()
                .is_ok_and(|s| s.validate_source(source).is_ok());
            let status = if !valid {
                "配置不可用或未启用"
            } else if selected == source.id {
                "✓ 使用中 · 连接未检测"
            } else {
                "已配置 · 连接未检测"
            };
            self.owner()
                .caption(&view, status, rect(18., y + 18., width - 260., 22.));
            self.cloud_button(
                &view,
                "编辑…",
                Action::Edit(*index),
                rect(width - 242., y + 35., 74., 30.),
                ready,
                &format!("voiceport.cloud.edit.{}", source.id),
            );
            self.cloud_button(
                &view,
                "移除…",
                Action::Remove(source.id.clone()),
                rect(width - 166., y + 35., 74., 30.),
                ready,
                &format!("voiceport.cloud.remove.{}", source.id),
            );
            self.cloud_button(
                &view,
                if selected == source.id {
                    "使用中"
                } else {
                    "使用"
                },
                Action::Use(source.id.clone()),
                rect(width - 90., y + 35., 70., 30.),
                ready && valid && selected != source.id,
                &format!("voiceport.cloud.use.{}", source.id),
            );
        }
        if let Err(error) = store.as_ref() {
            self.owner().caption(
                &view,
                &format!("来源配置读取失败：{error}"),
                rect(18., 4., width - 36., 40.),
            );
        } else {
            self.owner().caption(
                &view,
                "保存不测试连接。语言由服务支持范围决定；质量尚未进行同音频评估。",
                rect(18., 4., width - 36., 40.),
            );
        }
        scroll.setDocumentView(Some(&view));
        view.scrollPoint(NSPoint::new(0., height));
    }
}
