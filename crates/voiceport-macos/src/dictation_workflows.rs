//! Preferences that are independent of the legacy source and language keys.
use crate::{
    dictation_dictionary::{Dictionary, Rule},
    dictation_output::{OutputPipeline, OutputRules},
    local_models::{Catalog, SessionSource},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Workflows {
    pub(crate) rules: Vec<Rule>,
    pub(crate) dictionary_enabled: bool,
    pub(crate) output_rules: OutputRules,
    pub(crate) presets: Vec<Preset>,
    pub(crate) app_rules: crate::dictation_app_rules::AppRules,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Preset {
    pub(crate) name: String,
    pub(crate) source_id: String,
    pub(crate) language: String,
    pub(crate) dictionary_enabled: bool,
    #[serde(default)]
    pub(crate) output_rules: OutputRules,
}

impl Workflows {
    pub(crate) fn load(saved: Option<&str>) -> Result<Self, String> {
        let Some(saved) = saved else {
            return Ok(Self::default());
        };
        let preferences: Self = serde_json::from_str(saved).map_err(|_| "无法读取听写设置。")?;
        Dictionary::compile(&preferences.rules)?;
        Ok(preferences)
    }

    pub(crate) fn output_for_session(
        &self,
        dictionary_enabled: bool,
        output_rules: OutputRules,
    ) -> OutputPipeline {
        let rules = if dictionary_enabled {
            &self.rules[..]
        } else {
            &[]
        };
        // If validated settings somehow become invalid, preserve the source text.
        OutputPipeline::compile(rules, output_rules).unwrap_or_default()
    }

    pub(crate) fn add_preset(&mut self, mut preset: Preset) -> Result<(), String> {
        let name = preset.name.trim();
        if name.is_empty() || name.chars().any(char::is_control) || name.chars().count() > 40 {
            return Err("场景名称应为 1–40 个可显示字符。".into());
        }
        if self.presets.iter().any(|entry| entry.name == name) {
            return Err("已有同名场景。请更换名称，或先删除原场景。".into());
        }
        if self.presets.len() >= 32 {
            return Err("最多保存 32 个听写场景。".into());
        }
        preset.name = name.to_owned();
        self.presets.push(preset);
        Ok(())
    }

    pub(crate) fn remove_preset(&mut self, index: usize) {
        let preset = self.presets.remove(index);
        self.app_rules.remove_scene(&preset.name);
    }
}

impl Preset {
    pub(crate) fn source(&self, catalog: &Catalog) -> Result<SessionSource, String> {
        let source = catalog.snapshot(&self.source_id)?;
        if !voiceport_core::language::supported(&self.language)
            || matches!(&source, SessionSource::Local(model) if !model.kind.supports_language(&self.language))
        {
            return Err("此场景的来源不支持保存的识别语言。".into());
        }
        Ok(source)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn workflow_preferences_accept_legacy_and_roundtrip_named_presets() {
        use super::*;
        use crate::local_models::CHATGPT;

        for input in [None, Some("{}"), Some(r#"{"legacy":true}"#)] {
            let old = Workflows::load(input).unwrap();
            assert!(!old.dictionary_enabled);
            assert!(old.presets.is_empty());
            assert_eq!(old.output_rules, OutputRules::default());
            assert!(
                old.output_for_session(false, old.output_rules)
                    .is_identity()
            );
        }
        let legacy = Workflows::load(Some(r#"{"presets":[{"name":"test","source_id":"chatgpt","language":"zh","dictionary_enabled":true}]}"#)).unwrap();
        assert_eq!(legacy.presets[0].output_rules, OutputRules::default());
        let mut config = Workflows::default();
        let preset = Preset {
            name: "中英会议".into(),
            source_id: CHATGPT.into(),
            language: "zh".into(),
            dictionary_enabled: true,
            output_rules: OutputRules {
                trim_edges: true,
                ..Default::default()
            },
        };
        config.add_preset(preset.clone()).unwrap();
        assert!(config.add_preset(preset).is_err());
        let saved = serde_json::to_string(&config).unwrap();
        let loaded = Workflows::load(Some(&saved)).unwrap();
        let preset = &loaded.presets[0];
        assert!(
            !loaded.dictionary_enabled,
            "saving does not activate a preset"
        );
        assert!(preset.dictionary_enabled);
        assert!(preset.output_rules.trim_edges);
        assert_eq!(preset.language, "zh");
        let catalog = Catalog::pending(CHATGPT);
        assert_eq!(preset.source(&catalog), Ok(SessionSource::ChatGpt));
        let mut unavailable = preset.clone();
        unavailable.source_id = "removed-source".into();
        assert!(unavailable.source(&catalog).is_err());
        unavailable.source_id = CHATGPT.into();
        unavailable.language = "unsupported-language".into();
        assert!(unavailable.source(&catalog).is_err());
    }

    #[test]
    fn workflow_output_uses_explicit_session_choices() {
        use super::*;
        use crate::dictation_dictionary::parse_rules;

        let config = Workflows {
            rules: parse_rules("vp → VoicePort").unwrap(),
            ..Workflows::default()
        };
        let rules = OutputRules {
            trim_edges: true,
            ..Default::default()
        };
        assert_eq!(
            config
                .output_for_session(false, rules)
                .process(" vp ")
                .final_text(),
            "vp"
        );
        assert_eq!(
            config
                .output_for_session(true, rules)
                .process(" vp ")
                .final_text(),
            "VoicePort"
        );
    }
}
