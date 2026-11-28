//! Explicit confirmation for an interrupted microphone recording.
use super::{Controller, Delegate, source_manager::destination::Destination};
use crate::local_models::SessionSource;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSAlert, NSAlertSecondButtonReturn};
use objc2_foundation::NSString;

pub(super) struct FragmentPlan {
    pub generation: u64,
    pub confirmation: String,
    pub source: String,
    pub source_name: String,
    pub language: String,
    pub destination: Destination,
}

impl FragmentPlan {
    pub(super) fn read_gui(c: &Controller) -> Option<Self> {
        c.gui_fragment_retained().then(|| Self::read(c)).flatten()
    }

    pub(super) fn read(c: &Controller) -> Option<Self> {
        let (generation, confirmation, source, language) = c.fragment_offer()?;
        let destination = Destination::for_session(source);
        if matches!(
            destination,
            Destination::Unknown | Destination::HttpNotConfigured
        ) {
            return None;
        }
        let descriptor = source.descriptor();
        let (source, source_name) = match source {
            SessionSource::Plugin(snapshot) => {
                (snapshot.source.id.clone(), snapshot.source.name.clone())
            }
            SessionSource::Local(model) => {
                let definition = crate::model_library::MODELS
                    .iter()
                    .find(|d| d.kind == model.kind)?;
                (definition.id.into(), definition.name.into())
            }
            SessionSource::ChatGpt => (crate::local_models::CHATGPT.into(), descriptor.name),
        };
        Some(Self {
            generation,
            confirmation: confirmation.into(),
            source,
            source_name,
            language: if language.is_empty() {
                "auto"
            } else {
                language
            }
            .into(),
            destination,
        })
    }
}

impl Delegate {
    pub(super) fn confirm_interrupted_dictation(&self, plan: FragmentPlan) {
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str("转写不完整的录音片段？"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "原来源：{}\n原语言：{}\n音频去向：{}\n\n只转写已收到的片段。结果仅供查看或复制；不会写入任何应用。原输入框可能已有实时文字，请先检查。",
            plan.source_name,
            if plan.language == "auto" { "自动检测" } else { &plan.language },
            plan.destination.summary(),
        )));
        alert.addButtonWithTitle(&NSString::from_str("取消"));
        alert.addButtonWithTitle(&NSString::from_str("转写片段"));
        if alert.runModal() == NSAlertSecondButtonReturn
            && !self.settings_locked()
            && !self.ivars().quitting.get()
        {
            self.with_controller(|c| {
                c.transcribe_fragment(&plan.confirmation);
            });
        }
    }
}
