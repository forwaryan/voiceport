//! Workbench source selection. Browsing uses catalog snapshots and never starts an adapter.
use super::*;
use crate::local_models::{Catalog, SessionSource};
use objc2::{Message, rc::Weak};
use source_manager::destination::Destination;

#[cfg(debug_assertions)]
pub(super) mod fixture;

const NEXT_DICTATION: &str = "切换用于下一轮听写。已有录音和手动重试继续使用当时的来源与配置。";
const SOURCE_CHANGED: &str = "来源或听写设置已变化，请重新查看并确认音频去向。";
const UNAVAILABLE: &str = "所选来源不可用，请选择其他来源。";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Choice {
    id: String,
    title: String,
    source: SessionSource,
}

impl Choice {
    fn resolve(catalog: &Catalog, id: &str) -> Option<Self> {
        let entry = catalog.entries.iter().find(|entry| entry.id == id)?;
        Some(Self {
            id: entry.id.clone(),
            title: entry.title.clone(),
            source: entry.source.as_ref().ok()?.clone(),
        })
    }

    fn supports(&self, language: &str) -> bool {
        match &self.source {
            SessionSource::Local(model) => model.kind.supports_language(language),
            SessionSource::ChatGpt | SessionSource::Plugin(_) => true,
        }
    }

    fn matches(&self, controller: &Controller) -> bool {
        !controller.busy()
            && !controller.discovering()
            && self.supports(&controller.language)
            && controller.catalog.entries.iter().any(|entry| {
                entry.id == self.id
                    && entry.title == self.title
                    && entry.source.as_ref() == Ok(&self.source)
            })
    }

    fn apply(
        self,
        controller: &mut Controller,
        confirmed: bool,
        original_source: &str,
        language: &str,
        persist: impl FnOnce(&mut Controller, String),
    ) -> Result<(), &'static str> {
        if !confirmed {
            return Ok(());
        }
        if controller.source_id != original_source
            || controller.language != language
            || !self.matches(controller)
        {
            return Err(SOURCE_CHANGED);
        }
        persist(controller, self.id);
        Ok(())
    }
}

struct Controls {
    current: Retained<NSTextField>,
    source: Retained<NSPopUpButton>,
    availability: Retained<NSTextField>,
    destination: Retained<NSTextField>,
    permission: Retained<NSTextField>,
    apply: Retained<NSButton>,
}

#[derive(Default)]
struct Rows {
    input: Vec<(String, String)>,
    display: Vec<(String, String)>,
}

pub(super) struct PageIvars {
    owner: Weak<Delegate>,
    controls: OnceCell<Controls>,
    rows: RefCell<Rows>,
    current_id: RefCell<Option<String>>,
    draft: RefCell<Option<Choice>>,
}

// The root owns its controls and their native target, and weakly references the delegate.
define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PageIvars]
    pub(super) struct Page;

    impl Page {
        #[unsafe(method(previewDictationSource:))]
        fn preview_action(&self, sender: &NSPopUpButton) {
            if let Some(owner) = self.ivars().owner.load()
                && std::ptr::eq(sender, &*self.controls().source)
            {
                self.refresh_from(&owner);
            }
        }

        #[unsafe(method(applyDictationSource:))]
        fn apply_action(&self, sender: &NSButton) {
            let draft = self.ivars().draft.borrow().clone();
            if let Some(owner) = self.ivars().owner.load()
                && std::ptr::eq(sender, &*self.controls().apply)
                && sender.isEnabled()
                && let Some(choice) = draft
                && picker::selected_id(&self.controls().source).as_deref() == Some(choice.id.as_str())
            {
                owner.review_source_choice(choice);
                self.refresh_from(&owner);
            }
        }
    }
);

impl Page {
    pub(super) fn build(owner: &Delegate, frame: NSRect) -> Retained<Self> {
        let mtm = owner.mtm();
        let page = Self::alloc(mtm).set_ivars(PageIvars {
            owner: Weak::from_retained(&owner.retain()),
            controls: OnceCell::new(),
            rows: RefCell::new(Rows::default()),
            current_id: RefCell::new(None),
            draft: RefCell::new(None),
        });
        let page: Retained<Self> = unsafe { msg_send![super(page), initWithFrame: frame] };
        page.setAccessibilityIdentifier(Some(ns_string!("voiceport.sources")));
        let width = frame.size.width;
        let height = frame.size.height;
        let title = owner.label(&page, "转写来源", rect(0., height - 29., width, 28.), 21.);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(21., unsafe {
            NSFontWeightSemibold
        })));
        let subtitle = owner.caption(&page, NEXT_DICTATION, rect(0., height - 67., width, 33.));
        let current = owner.label(
            &page,
            "当前来源：正在读取…",
            rect(0., height - 112., width, 31.),
            14.,
        );
        let picker_label = owner.label(&page, "查看来源", rect(0., height - 151., 80., 20.), 13.);
        picker_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        let source = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(84., height - 157., width - 84., 30.),
            false,
        );
        source.setAccessibilityLabel(Some(ns_string!("查看来源")));
        source.setAccessibilityIdentifier(Some(ns_string!("voiceport.sources.picker")));
        unsafe {
            source.setTarget(Some(&page));
            source.setAction(Some(sel!(previewDictationSource:)));
        }
        page.addSubview(&source);

        let availability = owner.label(&page, "", rect(0., height - 219., width, 44.), 13.);
        let destination = owner.label(&page, "", rect(0., height - 268., width, 44.), 13.);
        let permission = owner.caption(&page, "", rect(0., height - 309., width, 36.));
        let apply = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("用于下一轮听写"),
                Some(&page),
                Some(sel!(applyDictationSource:)),
                mtm,
            )
        };
        apply.setFrame(rect(0., height - 349., 166., 32.));
        apply.setAccessibilityIdentifier(Some(ns_string!("voiceport.sources.apply")));
        apply.setEnabled(false);
        page.addSubview(&apply);

        let views: [&NSView; 7] = [
            &title,
            &subtitle,
            &current,
            &source,
            &availability,
            &destination,
            &permission,
        ];
        for view in views {
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinYMargin
                    | NSAutoresizingMaskOptions::ViewWidthSizable,
            );
        }
        apply.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinYMargin);
        let _ = page.ivars().controls.set(Controls {
            current,
            source,
            availability,
            destination,
            permission,
            apply,
        });
        page.refresh_from(owner);
        page
    }

    fn controls(&self) -> &Controls {
        self.ivars().controls.get().expect("source page controls")
    }

    pub(super) fn key_views(&self) -> [&NSView; 2] {
        [&self.controls().source, &self.controls().apply]
    }

    fn refresh_from(&self, owner: &Delegate) {
        if let Some(controller) = owner.ivars().controller.borrow().as_ref() {
            self.refresh(
                controller,
                !owner.settings_locked()
                    && !owner.ivars().quitting.get()
                    && owner.ivars().permission_rx.borrow().is_none()
                    && owner.ivars().pending_hotkey.borrow().is_none(),
            );
        }
    }

    pub(super) fn refresh(&self, controller: &Controller, idle: bool) {
        let controls = self.controls();
        let ivars = self.ivars();
        let current = &controller.source_id;
        let mut rows = ivars.rows.borrow_mut();
        // Rebuild and normalize native titles only when the catalog's rows change.
        let mut input: Vec<_> = controller
            .catalog
            .entries
            .iter()
            .map(|entry| (entry.id.clone(), entry.title.clone()))
            .collect();
        if !input.iter().any(|(id, _)| id == current) {
            input.push((current.clone(), format!("{current}（不可用）")));
        }
        if rows.input != input {
            let previous = picker::selected_id(&controls.source);
            rows.display.clone_from(&input);
            file_transcription::disambiguate_source_titles(&mut rows.display);
            picker::set_rows(&controls.source, &rows.display);
            if !picker::select_id(&controls.source, previous.as_deref().unwrap_or(current)) {
                picker::select_id(&controls.source, current);
            }
            rows.input = input;
        }
        if ivars.current_id.borrow().as_ref() != Some(current) {
            *ivars.current_id.borrow_mut() = Some(current.clone());
            picker::select_id(&controls.source, current);
        }
        let current_title = rows
            .display
            .iter()
            .find(|(id, _)| id == current)
            .map_or(current.as_str(), |(_, title)| title);
        set_text(&controls.current, &format!("当前来源：{current_title}"));

        let id = picker::selected_id(&controls.source);
        let choice = id
            .as_deref()
            .and_then(|id| Choice::resolve(&controller.catalog, id));
        let (availability, destination, permission) =
            describe(controller, id.as_deref(), choice.as_ref());
        set_text(&controls.availability, &availability);
        set_text(
            &controls.destination,
            &format!("音频去向：{}", destination.summary()),
        );
        set_text(&controls.permission, permission);
        controls.source.setEnabled(!rows.display.is_empty());
        controls.apply.setEnabled(
            idle && choice
                .as_ref()
                .is_some_and(|choice| choice.id != *current && choice.matches(controller)),
        );
        *ivars.draft.borrow_mut() = choice;
    }
}

fn set_text(label: &NSTextField, text: &str) {
    let value = NSString::from_str(text);
    if *label.stringValue() != *value {
        label.setStringValue(&value);
        label.setToolTip(Some(&value));
    }
}

fn describe(
    controller: &Controller,
    id: Option<&str>,
    choice: Option<&Choice>,
) -> (String, Destination, &'static str) {
    let destination = choice.map_or(Destination::Unknown, |choice| {
        Destination::for_session(&choice.source)
    });
    let availability = if controller.discovering() {
        "正在检查来源".into()
    } else if let Some(choice) = choice {
        if !choice.supports(&controller.language) {
            "不支持当前听写语言，请在设置中更改语言。".into()
        } else if destination.requires_confirmation() {
            "可选用 · 连接状态未检测".into()
        } else {
            "可选用".into()
        }
    } else {
        format!(
            "不可用 · {}",
            id.map_or(UNAVAILABLE, |id| controller.catalog.detail(id))
        )
    };
    let permission = match &destination {
        Destination::OnDevice => "许可：音频在本机处理。",
        Destination::ChatGpt | Destination::RemoteHttp(_) => "许可：切换时确认音频将发送到此服务。",
        Destination::LoopbackHttp(_) | Destination::ExternalProgram => {
            "许可：切换时确认音频将交给此服务或程序。对方可能转发音频。"
        }
        _ => "许可：请先修复来源配置，再确认音频去向。",
    };
    (format!("可用性：{availability}"), destination, permission)
}

impl Delegate {
    fn review_source_choice(&self, choice: Choice) {
        self.workflow_dialog(|| {
            let (original_source, language) = {
                let slot = self.ivars().controller.borrow();
                let controller = slot.as_ref().ok_or(UNAVAILABLE)?;
                if !choice.matches(controller) {
                    return Err(SOURCE_CHANGED.into());
                }
                if controller.source_id == choice.id {
                    return Ok(());
                }
                (controller.source_id.clone(), controller.language.clone())
            };
            let destination = Destination::for_session(&choice.source);
            let confirmed = !destination.requires_confirmation()
                || self
                    .workflow_alert(
                        "允许后续听写使用此来源？",
                        &format!(
                            "来源：{}\n来源标识：{}\n\n{}\n\n{NEXT_DICTATION}",
                            choice.title,
                            choice.id,
                            destination.details()
                        ),
                        &["确认切换", "取消"],
                    )
                    .runModal()
                    == NSAlertFirstButtonReturn;
            if self.ivars().quitting.get() {
                return Ok(());
            }
            let mut slot = self.ivars().controller.borrow_mut();
            let controller = slot.as_mut().ok_or(UNAVAILABLE)?;
            choice.apply(
                controller,
                confirmed,
                &original_source,
                &language,
                |c, id| self.set_source(c, id),
            )?;
            Ok(())
        });
    }

    /// Legacy selectors can use the same review path while they are still present.
    pub(super) fn request_dictation_source(&self, id: &str) {
        let choice = self
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .and_then(|controller| Choice::resolve(&controller.catalog, id));
        if let Some(choice) = choice {
            self.review_source_choice(choice);
        }
    }
}
