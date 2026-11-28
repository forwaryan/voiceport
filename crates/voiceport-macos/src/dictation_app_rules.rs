//! Exact application-to-scene bindings. Local source selection is opt-in per binding.
use crate::{dictation_workflows::Preset, local_models::Catalog};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_RULES: usize = 32;
const LOCAL_PREFIX: &str = "本机 ";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub(crate) struct AppRules {
    pub(crate) enabled: bool,
    bindings: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    local_sources: BTreeSet<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    manual_scene: Option<String>,
}

pub(crate) enum AppRuleSelection<'a> {
    Global,
    Application {
        preset: &'a Preset,
        local_source: bool,
    },
    Manual(&'a str),
    MissingLocalScene(&'a str),
}

pub(crate) fn valid_bundle_id(id: &str) -> bool {
    id.len() <= 255
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

impl AppRules {
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.manual_scene = None;
    }

    pub(crate) fn prioritize_manual_scene(&mut self, name: &str) {
        self.manual_scene = self.enabled.then(|| name.to_owned());
    }

    pub(crate) fn selection<'a>(
        &'a self,
        bundle_id: Option<&str>,
        presets: &'a [Preset],
    ) -> AppRuleSelection<'a> {
        if self.enabled
            && let Some(name) = self.manual_scene.as_deref()
            && presets.iter().any(|preset| preset.name == name)
        {
            return AppRuleSelection::Manual(name);
        }
        let Some(id) = bundle_id.filter(|id| self.enabled && valid_bundle_id(id)) else {
            return AppRuleSelection::Global;
        };
        let Some(scene) = self.bindings.get(id) else {
            return AppRuleSelection::Global;
        };
        let local_source = self.local_sources.contains(id);
        match presets.iter().find(|preset| &preset.name == scene) {
            Some(preset) => AppRuleSelection::Application {
                preset,
                local_source,
            },
            // An inconsistent saved binding must not silently use a remote default.
            None if local_source => AppRuleSelection::MissingLocalScene(scene),
            None => AppRuleSelection::Global,
        }
    }

    pub(crate) fn assign(
        &mut self,
        bundle_id: &str,
        scene: &str,
        presets: &[Preset],
    ) -> Result<(), String> {
        if !valid_bundle_id(bundle_id) {
            return Err("请输入有效的应用 Bundle ID，例如 com.example.Editor。".into());
        }
        if !presets.iter().any(|preset| preset.name == scene) {
            return Err("请选择一个已保存的听写场景。".into());
        }
        if self.bindings.len() >= MAX_RULES && !self.bindings.contains_key(bundle_id) {
            return Err(format!("最多保存 {MAX_RULES} 条应用输出规则。"));
        }
        let previous = self.bindings.insert(bundle_id.into(), scene.into());
        if previous.as_deref() != Some(scene) {
            // An output-only assignment does not authorize a different scene's source.
            self.local_sources.remove(bundle_id);
        }
        Ok(())
    }

    pub(crate) fn remove(&mut self, bundle_id: &str) -> bool {
        self.local_sources.remove(bundle_id);
        self.bindings.remove(bundle_id).is_some()
    }

    pub(crate) fn remove_scene(&mut self, name: &str) {
        self.bindings.retain(|_, scene| scene != name);
        self.local_sources
            .retain(|id| self.bindings.contains_key(id));
        if self.manual_scene.as_deref() == Some(name) {
            self.manual_scene = None;
        }
    }

    pub(crate) fn from_editor(
        enabled: bool,
        text: &str,
        presets: &[Preset],
        catalog: &Catalog,
    ) -> Result<Self, String> {
        let mut next = Self {
            enabled,
            ..Self::default()
        };
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let line_number = index + 1;
            let (id, scene) = line
                .split_once('→')
                .ok_or_else(|| format!("第 {line_number} 行应使用 Bundle ID → 场景名称。"))?;
            let id = id.trim();
            let local = id.strip_prefix(LOCAL_PREFIX);
            let id = local.map_or(id, str::trim_start);
            if next.bindings.contains_key(id) {
                return Err(format!("第 {line_number} 行的 Bundle ID 已有规则。"));
            }
            let scene = scene.trim();
            next.assign(id, scene, presets)
                .map_err(|error| format!("第 {line_number} 行：{error}"))?;
            if local.is_some() {
                if enabled {
                    let preset = presets.iter().find(|preset| preset.name == scene).unwrap();
                    preset
                        .check_app_local_source(catalog)
                        .map_err(|(_, error)| format!("第 {line_number} 行：{error}"))?;
                }
                next.local_sources.insert(id.into());
            }
        }
        Ok(next)
    }

    pub(crate) fn editor_text(&self) -> String {
        self.bindings
            .iter()
            .map(|(id, scene)| {
                let prefix = if self.local_sources.contains(id) {
                    LOCAL_PREFIX
                } else {
                    ""
                };
                format!("{prefix}{id} → {scene}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use crate::dictation_workflows::Workflows;

    fn preferences() -> Workflows {
        Workflows::load(Some(
            r#"{"presets":[
                {"name":"写作","source_id":"removed-source","language":"zh","dictionary_enabled":true},
                {"name":"聊天 → 简短","source_id":"other-source","language":"en","dictionary_enabled":false}
            ]}"#,
        ))
        .unwrap()
    }

    #[test]
    fn app_rules_legacy_disabled_exact_match_and_global_fallback() {
        use super::{AppRuleSelection, AppRules};

        let mut prefs = preferences();
        let rules = &mut prefs.app_rules;
        assert_eq!(*rules, AppRules::default());
        rules
            .assign("com.example.Editor", "写作", &prefs.presets)
            .unwrap();
        assert!(matches!(
            rules.selection(Some("com.example.Editor"), &prefs.presets),
            AppRuleSelection::Global
        ));
        rules.enabled = true;
        assert!(matches!(
            rules.selection(Some("com.example.Editor"), &prefs.presets),
            AppRuleSelection::Application { preset, local_source: false } if preset.name == "写作"
        ));
        for id in [
            None,
            Some(""),
            Some("Editor"),
            Some("com.example.editor"),
            Some("com.example.Editor.beta"),
            Some(" com.example.Editor"),
        ] {
            assert!(
                matches!(
                    rules.selection(id, &prefs.presets),
                    AppRuleSelection::Global
                ),
                "{id:?}"
            );
        }
        assert!(matches!(
            rules.selection(Some("com.example.Editor"), &[]),
            AppRuleSelection::Global
        ));
        let saved = serde_json::to_string(&prefs).unwrap();
        assert!(!saved.contains("local_sources"));
        let loaded = Workflows::load(Some(&saved)).unwrap();
        assert_eq!(loaded.app_rules, prefs.app_rules);
        assert!(matches!(
            loaded
                .app_rules
                .selection(Some("com.example.Editor"), &loaded.presets),
            AppRuleSelection::Application {
                local_source: false,
                ..
            }
        ));
    }

    #[test]
    fn app_rules_edit_roundtrip_validation_and_scene_removal() {
        use super::{AppRuleSelection, AppRules, Catalog, MAX_RULES};
        use crate::local_models::CHATGPT;

        let mut prefs = preferences();
        let catalog = Catalog::pending(CHATGPT);
        let rules = &mut prefs.app_rules;
        *rules = AppRules::from_editor(
            true,
            "com.example.Chat → 聊天 → 简短\n\ncom.example.Editor → 写作",
            &prefs.presets,
            &catalog,
        )
        .unwrap();
        assert_eq!(
            AppRules::from_editor(true, &rules.editor_text(), &prefs.presets, &catalog).unwrap(),
            *rules
        );
        assert!(
            AppRules::from_editor(
                true,
                "com.example.Chat → 写作\n本机 com.example.Chat → 写作",
                &prefs.presets,
                &catalog,
            )
            .unwrap_err()
            .contains("Bundle ID 已有规则")
        );
        assert!(
            rules
                .assign("https://example.com", "写作", &prefs.presets)
                .is_err()
        );
        assert!(
            rules
                .assign("com.example.Chat", "未知场景", &prefs.presets)
                .is_err()
        );
        assert!(matches!(
            rules.selection(Some("com.example.Chat"), &prefs.presets),
            AppRuleSelection::Application { preset, .. } if preset.name == "聊天 → 简短"
        ));
        for i in 2..MAX_RULES {
            rules
                .assign(&format!("com.example.App{i}"), "写作", &prefs.presets)
                .unwrap();
        }
        assert!(
            rules
                .assign("com.example.New", "写作", &prefs.presets)
                .is_err()
        );
        rules
            .assign("com.example.Editor", "聊天 → 简短", &prefs.presets)
            .unwrap();
        prefs.remove_preset(1);
        assert!(matches!(
            prefs
                .app_rules
                .selection(Some("com.example.Editor"), &prefs.presets),
            AppRuleSelection::Global
        ));
        assert!(!prefs.app_rules.remove("com.example.Chat"));
        assert!(prefs.app_rules.remove("com.example.App2"));
    }

    #[test]
    fn app_rules_opt_in_roundtrips_and_does_not_follow_reassignment() {
        use super::{AppRuleSelection, AppRules, Catalog};
        use crate::local_models::CHATGPT;

        const EDITOR: &str = "com.example.Editor";
        const RULES: &str = "本机 com.example.Chat → 写作\n本机 com.example.Editor → 写作";
        let mut prefs = preferences();
        let catalog = Catalog::pending(CHATGPT);
        // A user can disable a stale binding without losing it; saving it enabled is rejected.
        prefs.app_rules = AppRules::from_editor(false, RULES, &prefs.presets, &catalog).unwrap();
        assert!(
            AppRules::from_editor(true, RULES, &prefs.presets, &catalog)
                .unwrap_err()
                .contains("第 1 行")
        );
        let saved = serde_json::to_string(&prefs).unwrap();
        let mut loaded = Workflows::load(Some(&saved)).unwrap();
        assert_eq!(loaded.app_rules.editor_text(), RULES);
        assert!(matches!(
            loaded.app_rules.selection(Some(EDITOR), &loaded.presets),
            AppRuleSelection::Global
        ));
        let rules = &mut loaded.app_rules;
        rules.set_enabled(true);
        rules.assign(EDITOR, "写作", &loaded.presets).unwrap();
        assert!(matches!(
            rules.selection(Some(EDITOR), &loaded.presets),
            AppRuleSelection::Application {
                local_source: true,
                ..
            }
        ));
        rules
            .assign(EDITOR, "聊天 → 简短", &loaded.presets)
            .unwrap();
        assert!(matches!(
            rules.selection(Some(EDITOR), &loaded.presets),
            AppRuleSelection::Application {
                local_source: false,
                ..
            }
        ));
        assert!(rules.remove("com.example.Chat"));
        assert!(rules.local_sources.is_empty());
        loaded.app_rules = AppRules::from_editor(false, RULES, &loaded.presets, &catalog).unwrap();
        loaded.remove_preset(0);
        assert!(loaded.app_rules.editor_text().is_empty());
        assert!(loaded.app_rules.local_sources.is_empty());
    }
}
