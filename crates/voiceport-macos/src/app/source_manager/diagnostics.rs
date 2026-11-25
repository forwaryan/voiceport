//! Read-side projection only: no keychain access, test initiation or selection change.
use super::*;
use crate::{
    local_models::{Entry, SessionSource},
    source_tests::ProbeResult,
};

pub(super) fn matching_entry<'a>(
    entries: &'a [Entry],
    source: &Source,
    manifest: Option<&Manifest>,
) -> Option<&'a Entry> {
    let manifest = manifest?;
    entries.iter().find(|entry| {
        entry.id == source.id
            && matches!(&entry.source, Ok(SessionSource::Plugin(snapshot))
                if &snapshot.source == source && &snapshot.manifest == manifest)
    })
}

pub(super) fn test_status(
    source: &Source,
    invalid: bool,
    discovering: bool,
    entry: Option<&Entry>,
    result: Option<&ProbeResult>,
) -> String {
    if !source.enabled {
        "样例测试：已停用，不执行".into()
    } else if invalid {
        "样例测试：请先修正配置".into()
    } else if discovering {
        "样例测试：正在刷新来源".into()
    } else if entry.is_none() {
        "样例测试：尚无匹配的配置".into()
    } else if let Some(result) = result {
        format!("样例测试：{}", result.display())
    } else {
        "样例测试：当前配置未测试".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_models::Catalog;
    use std::collections::BTreeMap;

    #[test]
    fn source_diagnostics_requires_exact_identity_and_configuration() {
        let sources = ["first", "second"]
            .map(|id| Source {
                id: id.into(),
                name: "Same synthetic name".into(),
                plugin_id: crate::plugins::HTTP.into(),
                plugin_version: "1".into(),
                enabled: true,
                config: BTreeMap::from([
                    (
                        "endpoint".into(),
                        "https://example.invalid/asr?synthetic=1".into(),
                    ),
                    ("model".into(), "fixture".into()),
                ]),
                credential_refs: BTreeMap::new(),
            })
            .into();
        let store = Store {
            sources,
            ..Store::default()
        };
        let mut catalog = Catalog { entries: vec![] };
        catalog.add_configured(&store);
        let source = &store.sources[1];
        let manifest = store.plugin(&source.plugin_id).unwrap();
        let lookup = |source| matching_entry(&catalog.entries, source, Some(&manifest));
        let matched = lookup(source).expect("exact same-named source");
        assert_eq!(matched.id, "second");
        assert!(test_status(source, false, false, Some(matched), None).contains("未测试"));
        let mut changed = source.clone();
        changed.config.insert("model".into(), "new fixture".into());
        assert!(lookup(&changed).is_none());
        let mut disabled = source.clone();
        disabled.enabled = false;
        assert!(lookup(&disabled).is_none());
        assert!(test_status(&disabled, false, false, None, None).contains("已停用"));
    }
}
