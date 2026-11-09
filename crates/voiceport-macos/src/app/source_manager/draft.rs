//! Preflight the complete proposed store before leaving the draft-retaining form.
//! Pending secret values are validated in memory and never placed in the Store.
//! References are unique UUIDs with the eventual Keychain revision shape/length.
use crate::plugins::{MAX_CONFIG, Manifest, Source, Store};
use std::collections::BTreeMap;

pub(super) struct FieldValue {
    pub id: String,
    pub secret: bool,
    pub value: String,
    pub remove: bool,
}
/// Construct without reading or changing Keychain. Remove takes precedence over
/// replacement, and blank secret fields preserve existing references.
pub(super) fn candidate(
    old: Option<&Source>,
    plugin: &Manifest,
    name: &str,
    enabled: bool,
    values: &[FieldValue],
) -> (Source, Vec<(String, String)>) {
    let retained = old.filter(|s| s.plugin_id == plugin.id);
    let mut source = Source {
        id: old.map_or_else(
            || format!("source-{}", uuid::Uuid::new_v4()),
            |s| s.id.clone(),
        ),
        name: name.trim().into(),
        plugin_id: plugin.id.clone(),
        plugin_version: plugin.version.clone(),
        enabled,
        config: BTreeMap::new(),
        credential_refs: retained.map_or_else(BTreeMap::new, |s| s.credential_refs.clone()),
    };
    let mut pending = vec![];
    for f in values {
        if f.secret {
            if f.remove {
                source.credential_refs.remove(&f.id);
            } else if !f.value.is_empty() {
                source
                    .credential_refs
                    .insert(f.id.clone(), format!("key-{}", uuid::Uuid::new_v4()));
                pending.push((f.id.clone(), f.value.clone()));
            }
        } else if !f.value.trim().is_empty() {
            source.config.insert(f.id.clone(), f.value.trim().into());
        }
    }
    (source, pending)
}

pub(super) fn validate_with_pending(
    store: &Store,
    index: Option<usize>,
    source: &Source,
    pending: &[(String, String)],
) -> Result<(), String> {
    for (_, value) in pending {
        crate::plugin_credentials::validate_secret(value)?;
    }
    validate(store, index, source)
}
fn validate(store: &Store, index: Option<usize>, source: &Source) -> Result<(), String> {
    let mut enabled = source.clone();
    enabled.enabled = true;
    store.validate_source(&enabled)?;
    let mut candidate = store.clone();
    if let Some(index) = index {
        *candidate.sources.get_mut(index).ok_or("来源不存在。")? = source.clone();
    } else {
        candidate.sources.push(source.clone());
    }
    candidate.validate()?;
    let bytes = serde_json::to_vec_pretty(&candidate).map_err(|_| "无法序列化来源配置。")?;
    if bytes.len() as u64 > MAX_CONFIG {
        return Err("来源配置过大。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::HTTP;
    use std::collections::BTreeMap;
    fn source() -> Source {
        Source {
            id: "draft-source".into(),
            name: "工作账号".into(),
            plugin_id: HTTP.into(),
            plugin_version: "1".into(),
            enabled: false,
            config: BTreeMap::from([
                (
                    "endpoint".into(),
                    "https://example.invalid/transcribe".into(),
                ),
                ("model".into(), "synthetic".into()),
            ]),
            credential_refs: BTreeMap::new(),
        }
    }
    #[test]
    fn editing_keeps_identity_and_blank_keys_but_removal_wins_over_replacement() {
        let plugin = Store::default().plugin(HTTP).unwrap();
        let mut old = source();
        old.credential_refs
            .insert("api_key".into(), "key-existing".into());
        let field = |value: &str, remove| FieldValue {
            id: "api_key".into(),
            secret: true,
            value: value.into(),
            remove,
        };
        let (kept, pending) = candidate(
            Some(&old),
            &plugin,
            "  renamed  ",
            true,
            &[field("", false)],
        );
        assert_eq!(kept.id, old.id);
        assert_eq!(kept.name, "renamed");
        assert_eq!(kept.credential_refs, old.credential_refs);
        assert!(pending.is_empty());
        let (replaced, pending) = candidate(
            Some(&old),
            &plugin,
            "renamed",
            true,
            &[field("new synthetic key", false)],
        );
        assert_ne!(replaced.credential_refs, old.credential_refs);
        assert_eq!(
            pending,
            vec![("api_key".into(), "new synthetic key".into())]
        );
        let (removed, pending) = candidate(
            Some(&old),
            &plugin,
            "renamed",
            true,
            &[field("ignored", true)],
        );
        assert!(removed.credential_refs.is_empty());
        assert!(pending.is_empty());
        assert_eq!(old.credential_refs["api_key"], "key-existing");
    }
    #[test]
    fn new_drafts_have_distinct_ids_and_trim_only_nonsecret_fields() {
        let plugin = Store::default().plugin(HTTP).unwrap();
        let values = vec![
            FieldValue {
                id: "model".into(),
                secret: false,
                value: "  synthetic  ".into(),
                remove: false,
            },
            FieldValue {
                id: "api_key".into(),
                secret: true,
                value: "  synthetic key  ".into(),
                remove: false,
            },
        ];
        let (first, pending) = candidate(None, &plugin, " new ", false, &values);
        let (second, _) = candidate(None, &plugin, " new ", false, &values);
        assert_ne!(first.id, second.id);
        assert_eq!(first.config["model"], "synthetic");
        assert!(!first.config.contains_key("api_key"));
        assert_eq!(pending[0].1, "  synthetic key  ");
        assert_ne!(
            first.credential_refs["api_key"],
            second.credential_refs["api_key"]
        );
    }
    #[test]
    fn invalid_pending_secrets_remain_form_validation_errors() {
        let store = Store::default();
        let draft = source();
        for value in [
            "x".repeat(4097),
            "x\ny".into(),
            "x\u{85}y".into(),
            "名".repeat(1366),
        ] {
            let pending = vec![("api_key".into(), value)];
            let error = validate_with_pending(&store, None, &draft, &pending).unwrap_err();
            assert_eq!(error, "密钥过长或包含控制字符。");
        }
        for value in [String::new(), "x".repeat(4096), "名".repeat(1365)] {
            assert!(
                validate_with_pending(&store, None, &draft, &[("api_key".into(), value)]).is_ok()
            );
        }
    }
    #[test]
    fn every_pending_secret_is_checked_without_changing_the_draft() {
        let store = Store::default();
        let draft = source();
        let before = draft.clone();
        let pending = vec![
            ("first".into(), "synthetic-valid".into()),
            ("second".into(), "x\ty".into()),
        ];
        assert!(validate_with_pending(&store, None, &draft, &pending).is_err());
        assert_eq!(draft, before);
        assert!(store.sources.is_empty());
        assert!(validate_with_pending(&store, None, &draft, &[]).is_ok());
    }
    #[test]
    fn names_use_persisted_utf8_and_nonempty_rules() {
        for name in ["".into(), "  ".into(), "名".repeat(50)] {
            let mut draft = source();
            draft.name = name;
            assert!(validate(&Store::default(), None, &draft).is_err());
        }
        let mut draft = source();
        draft.name = "名".repeat(42);
        assert!(validate(&Store::default(), None, &draft).is_ok());
    }
    #[test]
    fn editing_replaces_identity_but_adding_cannot_duplicate_it() {
        let draft = source();
        let mut store = Store::default();
        store.sources.push(draft.clone());
        assert!(validate(&store, Some(0), &draft).is_ok());
        assert!(validate(&store, None, &draft).is_err());
        assert!(validate(&store, Some(1), &draft).is_err());
    }
    #[test]
    fn field_errors_are_checked_even_for_disabled_drafts() {
        let mut draft = source();
        draft.config.remove("model");
        assert!(validate(&Store::default(), None, &draft).is_err());
        draft.config.insert("model".into(), "x".repeat(4097));
        assert!(validate(&Store::default(), None, &draft).is_err());
    }
    #[test]
    fn store_capacity_is_checked_before_committing_a_new_source() {
        let mut store = Store::default();
        for i in 0..128 {
            let mut row = source();
            row.id = format!("source-{i}");
            store.sources.push(row);
        }
        assert!(validate(&store, None, &source()).is_err());
        assert!(validate(&store, Some(0), &source()).is_ok());
    }
    #[test]
    fn credential_references_remain_unique_without_reading_secrets() {
        let mut a = source();
        a.credential_refs.insert("api_key".into(), "key-a".into());
        let mut store = Store::default();
        store.sources.push(a.clone());
        let mut b = a.clone();
        b.id = "another-source".into();
        assert!(validate(&store, None, &b).is_err());
        b.credential_refs.insert("api_key".into(), "key-b".into());
        assert!(validate(&store, None, &b).is_ok());
    }
    #[test]
    fn total_serialized_size_is_checked_without_writing() {
        let mut store = Store::default();
        for i in 0..70 {
            let mut row = source();
            row.id = format!("source-{i}");
            row.config.insert("model".into(), "x".repeat(4096));
            store.sources.push(row);
        }
        assert!(store.validate().is_ok());
        assert!(validate(&store, None, &source()).is_err());
    }
}
