//! Same-name sources and approvals with synthetic catalogs and in-memory preferences.
use super::*;
use crate::plugins::{HTTP, Source, Store};
use std::collections::BTreeMap;

fn store() -> Store {
    Store {
        sources: [("team-a", true), ("team-b", true), ("disabled", false)]
            .into_iter()
            .map(|(id, enabled)| Source {
                id: id.into(),
                name: "工作转写".into(),
                plugin_id: HTTP.into(),
                plugin_version: "1".into(),
                enabled,
                config: BTreeMap::from([
                    (
                        "endpoint".into(),
                        format!("https://{id}.example.invalid/transcribe?token=fixture"),
                    ),
                    ("model".into(), "fixture-model".into()),
                ]),
                credential_refs: BTreeMap::new(),
            })
            .collect(),
        ..Store::default()
    }
}

pub(in crate::app) fn native(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let owner = Delegate::new(mtm);
    let mut controller = Controller::new();
    controller.catalog.add_configured(&store());
    controller.source_id = "team-a".into();
    owner.ivars().controller.replace(Some(controller));
    let page = Page::build(&owner, rect(0., 0., 572., 408.));
    let controls = page.controls();
    assert!(page.window().is_none());
    let [picker, apply] = page.key_views();
    assert_eq!(picker.frame(), controls.source.frame());
    assert_eq!(apply.frame(), controls.apply.frame());
    assert!(
        controls
            .current
            .stringValue()
            .to_string()
            .contains("工作转写")
    );
    assert!(!controls.apply.isEnabled());

    let select = |id| {
        assert!(picker::select_id(&controls.source, id));
        assert!(unsafe {
            app.sendAction_to_from(
                controls.source.action().unwrap(),
                Some(&*page),
                Some(&*controls.source),
            )
        });
    };
    let title_a = controls.source.titleOfSelectedItem().unwrap();
    select("team-b");
    assert_ne!(controls.source.titleOfSelectedItem().unwrap(), title_a);
    assert_eq!(
        picker::selected_id(&controls.source).as_deref(),
        Some("team-b")
    );
    assert!(controls.apply.isEnabled());
    let destination = controls.destination.stringValue().to_string();
    assert!(destination.contains("https://team-b.example.invalid"));
    assert!(!destination.contains("token=fixture"));
    assert!(
        controls
            .permission
            .stringValue()
            .to_string()
            .contains("切换时确认")
    );
    assert_eq!(
        owner
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .unwrap()
            .source_id,
        "team-a"
    );
    select("disabled");
    assert!(!controls.apply.isEnabled());
    assert!(
        controls
            .availability
            .stringValue()
            .to_string()
            .contains("不可用")
    );
    assert!(
        controls
            .destination
            .stringValue()
            .to_string()
            .contains("未知")
    );
    println!(
        "source_page: offscreen AppKit stable IDs, current choice, availability, private destination and confirmation label passed"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_cancellation_and_confirmation_only_change_the_future_preference() {
        let mut controller = Controller::new();
        controller.catalog.add_configured(&store());
        controller.source_id = "team-a".into();
        let original = controller.catalog.snapshot("team-a").unwrap();
        let target = Choice::resolve(&controller.catalog, "team-b").unwrap();
        assert_eq!(
            target.title,
            Choice::resolve(&controller.catalog, "team-a")
                .unwrap()
                .title
        );
        assert_ne!(target.source, original);
        assert!(Destination::for_session(&target.source).requires_confirmation());
        let language = controller.language.clone();
        target
            .clone()
            .apply(&mut controller, false, "team-a", &language, |_, _| {
                panic!("cancel writes a preference")
            })
            .unwrap();
        assert_eq!(controller.source_id, "team-a");
        target
            .apply(&mut controller, true, "team-a", &language, |c, id| {
                c.source_id = id
            })
            .unwrap();
        assert_eq!(controller.source_id, "team-b");
        assert_ne!(
            controller.catalog.snapshot(&controller.source_id).unwrap(),
            original
        );
        assert_eq!(controller.catalog.snapshot("team-a").unwrap(), original);
    }

    #[test]
    fn confirmation_rejects_a_changed_destination_for_the_same_id() {
        let mut controller = Controller::new();
        let mut configuration = store();
        controller.catalog.add_configured(&configuration);
        controller.source_id = "team-a".into();
        let choice = Choice::resolve(&controller.catalog, "team-b").unwrap();
        configuration.sources[1].config.insert(
            "endpoint".into(),
            "https://changed.example.invalid/transcribe".into(),
        );
        controller.catalog.add_configured(&configuration);
        let language = controller.language.clone();
        assert_eq!(
            choice.apply(&mut controller, true, "team-a", &language, |_, _| panic!(
                "stale consent writes a preference"
            )),
            Err(SOURCE_CHANGED)
        );
        assert_eq!(controller.source_id, "team-a");
    }
}
