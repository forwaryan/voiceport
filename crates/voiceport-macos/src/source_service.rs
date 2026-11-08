//! Shared GUI/CLI source transactions. Caller serializes edits against sessions.
use crate::{
    plugin_credentials::Keychain,
    plugins::{Source, Store},
};
use std::path::Path;

pub(crate) fn save_source(
    store: &mut Store,
    path: &Path,
    index: Option<usize>,
    mut source: Source,
    pending: Vec<(String, String)>,
) -> Result<(), String> {
    let old = index.and_then(|i| store.sources.get(i)).cloned();
    let mut validate = source.clone();
    validate.enabled = true;
    store.validate_source(&validate)?;
    let mut created = vec![];
    let outcome = (|| {
        for (field, value) in pending {
            let reference = Keychain.create(&value)?;
            created.push(reference.clone());
            source.credential_refs.insert(field, reference);
        }
        let mut next = store.clone();
        if let Some(i) = index {
            next.sources[i] = source.clone();
        } else {
            next.sources.push(source.clone());
        }
        next.save(path)?;
        *store = next;
        Ok::<(), String>(())
    })();
    if let Err(e) = outcome {
        for reference in created {
            if Store::reference_is_confirmed_unused(path, &reference) {
                let _ = Keychain.delete(&reference);
            }
        }
        return Err(e);
    }
    if let Some(old) = old {
        for reference in old.credential_refs.values() {
            if !source.credential_refs.values().any(|r| r == reference) {
                Keychain.delete(reference)?;
            }
        }
    }
    Ok(())
}
pub(crate) fn remove(store: &mut Store, path: &Path, id: &str) -> Result<(), String> {
    let index = store
        .sources
        .iter()
        .position(|s| s.id == id)
        .ok_or("Source does not exist.")?;
    let removed = store.sources.remove(index);
    store.save(path)?;
    for reference in removed.credential_refs.values() {
        Keychain.delete(reference)?;
    }
    Ok(())
}
