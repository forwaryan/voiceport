//! Local application-to-scene bindings, independent of source and delivery.
use crate::dictation_workflows::Preset;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_RULES: usize = 32;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub(crate) struct AppRules {
    pub(crate) enabled: bool,
    bindings: BTreeMap<String, String>,
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
    pub(crate) fn matched_scene<'a>(
        &self,
        bundle_id: Option<&str>,
        presets: &'a [Preset],
    ) -> Option<&'a Preset> {
        let id = bundle_id.filter(|id| self.enabled && valid_bundle_id(id))?;
        let scene = self.bindings.get(id)?;
        presets.iter().find(|preset| &preset.name == scene)
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
        self.bindings.insert(bundle_id.into(), scene.into());
        Ok(())
    }

    pub(crate) fn remove(&mut self, bundle_id: &str) -> bool {
        self.bindings.remove(bundle_id).is_some()
    }

    pub(crate) fn remove_scene(&mut self, name: &str) {
        self.bindings.retain(|_, scene| scene != name);
    }

    pub(crate) fn from_editor(
        enabled: bool,
        text: &str,
        presets: &[Preset],
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
            if next.bindings.contains_key(id) {
                return Err(format!("第 {line_number} 行的 Bundle ID 已有规则。"));
            }
            next.assign(id, scene.trim(), presets)
                .map_err(|error| format!("第 {line_number} 行：{error}"))?;
        }
        Ok(next)
    }

    pub(crate) fn editor_text(&self) -> String {
        self.bindings
            .iter()
            .map(|(id, scene)| format!("{id} → {scene}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut prefs = preferences();
        let rules = &mut prefs.app_rules;
        assert_eq!(*rules, AppRules::default());
        rules
            .assign("com.example.Editor", "写作", &prefs.presets)
            .unwrap();
        assert!(
            rules
                .matched_scene(Some("com.example.Editor"), &prefs.presets)
                .is_none()
        );
        rules.enabled = true;
        assert_eq!(
            rules
                .matched_scene(Some("com.example.Editor"), &prefs.presets)
                .unwrap()
                .name,
            "写作"
        );
        for id in [
            None,
            Some(""),
            Some("Editor"),
            Some("com.example.editor"),
            Some("com.example.Editor.beta"),
            Some(" com.example.Editor"),
        ] {
            assert!(rules.matched_scene(id, &prefs.presets).is_none(), "{id:?}");
        }
        assert!(
            rules
                .matched_scene(Some("com.example.Editor"), &[])
                .is_none()
        );
        let saved = serde_json::to_string(&prefs).unwrap();
        assert_eq!(
            Workflows::load(Some(&saved)).unwrap().app_rules,
            prefs.app_rules
        );
    }

    #[test]
    fn app_rules_edit_roundtrip_validation_and_scene_removal() {
        let mut prefs = preferences();
        let rules = &mut prefs.app_rules;
        *rules = AppRules::from_editor(
            true,
            "com.example.Chat → 聊天 → 简短\n\ncom.example.Editor → 写作",
            &prefs.presets,
        )
        .unwrap();
        assert_eq!(
            AppRules::from_editor(true, &rules.editor_text(), &prefs.presets).unwrap(),
            *rules
        );
        assert!(
            AppRules::from_editor(
                true,
                "com.example.Chat → 写作\ncom.example.Chat → 写作",
                &prefs.presets
            )
            .is_err()
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
        assert_eq!(
            rules
                .matched_scene(Some("com.example.Chat"), &prefs.presets)
                .unwrap()
                .name,
            "聊天 → 简短"
        );
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
        assert!(
            prefs
                .app_rules
                .matched_scene(Some("com.example.Editor"), &prefs.presets)
                .is_none()
        );
        assert!(!prefs.app_rules.remove("com.example.Chat"));
        assert!(prefs.app_rules.remove("com.example.App2"));
    }
}
