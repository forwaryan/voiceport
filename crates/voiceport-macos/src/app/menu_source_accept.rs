//! Private Debug instrumentation for the real quick-panel source-selection actions.
use super::*;
use crate::local_models::{Entry, Model, ModelKind, SessionSource};
use crate::plugins::{HTTP, Source, Store};
use objc2_foundation::NSBundle;
use serde_json::json;
use std::{collections::BTreeMap, fs, io, net::TcpListener, path::Path};

thread_local! {
    static WRITES: Cell<u32> = const { Cell::new(0) };
    static CLOSED_BEFORE_ALERT: Cell<u32> = const { Cell::new(0) };
}

pub(super) fn record_closed_before_alert(owner: &Delegate) {
    let app = NSApplication::sharedApplication(owner.mtm());
    assert!(app.modalWindow().is_none());
    assert!(
        owner
            .ivars()
            .quick_panel
            .get()
            .is_some_and(|panel| !panel.is_shown())
    );
    CLOSED_BEFORE_ALERT.with(|count| count.set(count.get() + 1));
}

pub(super) fn record_write(synchronized: bool) {
    assert!(
        synchronized,
        "isolated source preference could not be synchronized"
    );
    WRITES.with(|writes| writes.set(writes.get() + 1));
}

pub(super) fn initialize(
    owner: &Delegate,
    controller: &mut Controller,
    directory: &Path,
) -> TcpListener {
    let domain = NSBundle::mainBundle().bundleIdentifier().unwrap();
    let defaults = NSUserDefaults::standardUserDefaults();
    assert!(
        defaults.persistentDomainForName(&domain).is_none(),
        "source fixture domain must start without preferences"
    );
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let model = SessionSource::Local(Model {
        kind: ModelKind::Paraformer,
        directory: directory.join("unused-model"),
        runtime: directory.join("unused-runtime"),
        encoder: directory.join("unused-encoder"),
        decoder: directory.join("unused-decoder"),
        joiner: None,
        tokens: directory.join("unused-tokens"),
    });
    controller.catalog.entries = [
        ("fixture-local-a", "Synthetic Local A"),
        ("fixture-local-b", "Synthetic Local B"),
    ]
    .into_iter()
    .map(|(id, title)| Entry {
        id: id.into(),
        title: title.into(),
        detail: "Synthetic on-device source".into(),
        source: Ok(model.clone()),
    })
    .collect();
    controller.catalog.add_configured(&Store {
        sources: vec![Source {
            id: "fixture-external".into(),
            name: "Synthetic External".into(),
            plugin_id: HTTP.into(),
            plugin_version: "1".into(),
            enabled: true,
            config: BTreeMap::from([
                ("endpoint".into(), format!("{origin}/private-fixture")),
                ("model".into(), "fixture-model".into()),
            ]),
            credential_refs: BTreeMap::new(),
        }],
        ..Store::default()
    });
    assert!(controller.catalog.snapshot("fixture-external").is_ok());
    owner.set_source(controller, "fixture-local-a".into());
    fs::write(directory.join("source-origin"), origin).unwrap();
    listener
}

pub(super) fn sample(owner: &Delegate, directory: &Path, listener: &TcpListener) {
    match listener.accept() {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("a source selection tried to contact the synthetic HTTP service"),
        Err(error) => panic!("synthetic HTTP observer failed: {error}"),
    }
    let app = NSApplication::sharedApplication(owner.mtm());
    let defaults = NSUserDefaults::standardUserDefaults();
    let domain = NSBundle::mainBundle().bundleIdentifier().unwrap();
    let saved = defaults
        .persistentDomainForName(&domain)
        .and_then(|prefs| prefs.objectForKey(ns_string!("transcriptionSource")))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string());
    let state = owner.ivars().controller.borrow();
    let controller = state.as_ref().unwrap();
    assert!(!controller.busy());
    let panel = owner
        .ivars()
        .quick_panel
        .get()
        .and_then(|panel| panel.fixture_source_state(&app));
    let modal = app.modalWindow();
    let row = json!({
        "source": controller.source_id, "saved": saved, "writes": WRITES.with(Cell::get),
        "popover": panel.map(|state| state.0), "expanded": panel.is_some_and(|state| state.1),
        "native_disclosure_focus": panel.is_some_and(|state| state.2),
        "modal": modal.as_ref().is_some_and(|window| window.isVisible()),
        "modal_key": modal.as_deref().is_some_and(|window| app.isActive() && window.isKeyWindow()
            && app.keyWindow().as_deref() == Some(window) && window.firstResponder().is_some()),
        "closed_before_alert": CLOSED_BEFORE_ALERT.with(Cell::get), "external_calls": 0,
    });
    let pending = directory.join("source-state.tmp");
    fs::write(&pending, row.to_string()).unwrap();
    fs::rename(pending, directory.join("source-state.json")).unwrap();
}

pub(super) fn cleanup() {
    let defaults = NSUserDefaults::standardUserDefaults();
    let domain = NSBundle::mainBundle().bundleIdentifier().unwrap();
    defaults.removePersistentDomainForName(&domain);
    assert!(defaults.synchronize());
    assert!(defaults.persistentDomainForName(&domain).is_none());
    println!("source-probe: isolated preference domain removed");
}
