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
        let height =
            (202. + sources.len() as f64 * ROW_HEIGHT + 60.).max(scroll.contentSize().height);
        let view = NSView::initWithFrame(
            NSView::alloc(self.mtm()),
            rect(0., 0., scroll.contentSize().width, height),
        );
        scroll.setDocumentView(Some(&view));
        let width = scroll.contentSize().width - 12.; // Leave room for overlay scrollers.
        view.setFrameSize(NSSize::new(width, height));
        let card_width = (width - 24.) / 2.;
        for (i, preset) in [Preset::GptTranscribe, Preset::Compatible]
            .into_iter()
            .enumerate()
        {
            let x = 6. + i as f64 * (card_width + 12.);
            presentation::surface(
                self.owner(),
                &view,
                rect(x, height - 140., card_width, 128.),
            );
            let icon = self.owner().icon(
                &view,
                if i == 0 { "waveform" } else { "server.rack" },
                rect(x + 18., height - 57., 24., 24.),
            );
            icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
            let name = self.owner().label(
                &view,
                preset.name(),
                rect(x + 54., height - 57., card_width - 72., 27.),
                17.,
            );
            name.setFont(Some(&NSFont::systemFontOfSize_weight(17., unsafe {
                NSFontWeightMedium
            })));
            self.owner().caption(
                &view,
                preset.description(),
                rect(x + 18., height - 98., card_width - 36., 32.),
            );
            self.cloud_button(
                &view,
                if preset == Preset::GptTranscribe {
                    "添加 API 配置…"
                } else {
                    "设置兼容服务…"
                },
                Action::Add(preset),
                rect(x + 14., height - 135., card_width - 28., 26.),
                ready,
                if i == 0 {
                    "voiceport.cloud.add-gpt"
                } else {
                    "voiceport.cloud.add-compatible"
                },
            );
        }
        let heading = self.owner().label(
            &view,
            &format!("已保存的配置  {}", sources.len()),
            rect(12., height - 182., width - 24., 24.),
            13.,
        );
        heading.setFont(Some(&NSFont::systemFontOfSize_weight(13., unsafe {
            NSFontWeightMedium
        })));
        for (row, (index, source)) in sources.iter().enumerate() {
            let y = height - 202. - (row + 1) as f64 * ROW_HEIGHT;
            let panel = presentation::surface(
                self.owner(),
                &view,
                rect(6., y + 4., width - 12., ROW_HEIGHT - 8.),
            );
            if selected == source.id {
                panel.setBorderColor(&NSColor::controlAccentColor().colorWithAlphaComponent(0.55));
                panel.setBorderWidth(1.);
            }
            let name = self.owner().label(
                &view,
                &source.name,
                rect(22., y + 66., width - 44., 23.),
                15.,
            );
            name.setMaximumNumberOfLines(1);
            name.setToolTip(Some(&NSString::from_str(&source.name)));
            let endpoint = cloud_models::destination(source);
            let destination = self.owner().caption(
                &view,
                &format!("音频目的地：{endpoint}"),
                rect(22., y + 40., width - 44., 23.),
            );
            destination.setMaximumNumberOfLines(1);
            destination.setToolTip(Some(&NSString::from_str(&endpoint)));
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
                .caption(&view, status, rect(22., y + 16., width - 308., 22.));
            self.cloud_button(
                &view,
                "编辑…",
                Action::Edit(*index),
                rect(width - 242., y + 12., 74., 30.),
                ready,
                &format!("voiceport.cloud.edit.{}", source.id),
            );
            self.cloud_button(
                &view,
                "移除…",
                Action::Remove(source.id.clone()),
                rect(width - 166., y + 12., 74., 30.),
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
                rect(width - 90., y + 12., 70., 30.),
                ready && valid && selected != source.id,
                &format!("voiceport.cloud.use.{}", source.id),
            );
        }
        if let Err(error) = store.as_ref() {
            self.owner().caption(
                &view,
                &format!("来源配置读取失败：{error}"),
                rect(22., height - 266., width - 44., 58.),
            );
        } else if sources.is_empty() {
            self.owner().label(
                &view,
                "还没有云端配置",
                rect(22., height - 224., width - 44., 24.),
                14.,
            );
            self.owner().caption(
                &view,
                "从上方选择一种服务，保存后再点击「使用」。",
                rect(22., height - 252., width - 44., 24.),
            );
        }
        view.scrollPoint(NSPoint::new(0., height));
    }
}
