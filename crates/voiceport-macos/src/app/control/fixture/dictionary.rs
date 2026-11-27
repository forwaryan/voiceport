use super::*;
use crate::{dictation_dictionary::parse_rules, dictation_workflows::Workflows};

pub fn isolated_dictionary_exchange_regression(mtm: MainThreadMarker, cli: &Path) {
    let _app = NSApplication::sharedApplication(mtm);
    let temporary = tempfile::tempdir_in("/tmp").unwrap();
    let folder = temporary.path().canonicalize().unwrap();
    let defaults = Defaults::new();
    let owner = Delegate::new(mtm);
    assert!(
        owner
            .ivars()
            .dictionary_fixture_defaults
            .set(defaults.store.clone())
            .is_ok()
    );
    let legacy = r#"{"dictionary_enabled":true,"rules":[{"input":"café","output":"kept"}],"presets":[{"name":"Synthetic","source_id":"chatgpt","language":"en","dictionary_enabled":false}]}"#;
    let mut controller = Controller::new();
    controller.workflows = Workflows::load(Some(legacy)).unwrap();
    owner.ivars().controller.replace(Some(controller));
    let socket = folder.join("ipc/control.sock");
    owner.ivars().control.borrow_mut().server = Some(Server::bind(&socket).unwrap());
    let fixture = Fixture { cli, socket, owner };
    let call = |words: &[&str], exit| fixture.call(words, exit);
    let shown = || call(&["dictionary", "show"], 0)["data"].clone();
    let persisted = || {
        defaults
            .store
            .stringForKey(ns_string!("dictationWorkflowsV1"))
            .map(|text| serde_json::from_str::<Value>(&text.to_string()).unwrap())
    };
    let initial = shown();
    assert_eq!(initial["stage"], "post_transcription");
    assert_eq!(initial["enabled"], true);
    assert!(persisted().is_none());

    let incoming = folder.join("incoming.csv");
    let path = incoming.to_str().unwrap();
    let mut input = "input,output\r\ncafe\u{301},ignored\r\n".to_string();
    for n in 0..17 {
        input.push_str(&format!("term{n},\" new, \"\"值\"\" → 🦀 \"\r\n"));
    }
    fs::write(&incoming, &input).unwrap();
    let cancelled = call(&["dictionary", "import", path, "--replace"], 0);
    let id = cancelled["data"]["operation"].as_str().unwrap();
    assert_eq!(
        call(&["dictionary", "import-cancel", id], 0)["data"]["state"],
        "cancelled"
    );
    assert_eq!(
        call(&["dictionary", "import-apply", id, "--confirm"], 6)["error"]["code"],
        "not_found"
    );
    assert_eq!(shown(), initial);
    assert!(persisted().is_none());

    let merged = call(&["dictionary", "import", path], 0);
    let data = &merged["data"];
    let id = data["operation"].as_str().unwrap();
    assert_eq!(data["skipped_duplicates"], 1);
    assert_eq!(data["result_count"], 18);
    assert_eq!(data["rules"][0]["output"], "kept");
    assert_eq!(data["next_offset"], 16);
    let second_page = call(&["dictionary", "import-preview", id, "16"], 0);
    assert_eq!(
        second_page["data"]["rules"][1]["output"],
        " new, \"值\" → 🦀 "
    );
    assert!(second_page["data"]["next_offset"].is_null());
    assert_eq!(
        call(&["dictionary", "import-apply", id], 2)["error"]["code"],
        "invalid_arguments"
    );
    assert_eq!(shown(), initial);
    assert!(persisted().is_none());

    // Confirmation applies the exact previewed snapshot even after the input file changes.
    fs::write(&incoming, "input,output\nvalid,rule\nbroken").unwrap();
    assert_eq!(
        call(&["dictionary", "import", path], 2)["error"]["code"],
        "invalid_config"
    );
    fs::File::create(&incoming)
        .unwrap()
        .set_len(1024 * 1024 + 1)
        .unwrap();
    assert_eq!(
        call(&["dictionary", "import", path], 2)["error"]["code"],
        "invalid_config"
    );
    assert_eq!(shown(), initial);
    assert!(persisted().is_none());
    let applied = call(&["dictionary", "import-apply", id, "--confirm"], 0);
    assert_eq!(applied["data"]["rule_count"], 18);
    let saved = persisted().unwrap();
    assert_eq!(saved["presets"][0]["name"], "Synthetic");
    assert_eq!(saved["dictionary_enabled"], true);
    assert_eq!(saved["rules"][17]["output"], " new, \"值\" → 🦀 ");

    let output = folder.join("export.csv");
    let path = output.to_str().unwrap();
    assert_eq!(
        call(&["dictionary", "export", "--output", path], 0)["data"]["rule_count"],
        18
    );
    assert_eq!(
        call(&["dictionary", "export", "--output", path], 2)["error"]["code"],
        "invalid_config"
    );
    let reimport = call(&["dictionary", "import", path, "--replace"], 0);
    let id = reimport["data"]["operation"].as_str().unwrap();
    assert_eq!(
        call(&["dictionary", "import-apply", id, "--confirm"], 0)["data"]["changed"],
        false
    );
    assert_eq!(persisted().unwrap(), saved);

    let stale = call(&["dictionary", "import", path, "--replace"], 0);
    let id = stale["data"]["operation"].as_str().unwrap();
    fixture.owner.with_controller(|c| {
        let mut next = c.workflows.clone();
        next.rules = parse_rules("changed → another edit").unwrap();
        fixture.owner.commit_workflows(c, next).unwrap();
    });
    let changed = persisted().unwrap();
    assert_eq!(
        call(&["dictionary", "import-apply", id, "--confirm"], 1)["error"]["code"],
        "invalid_state"
    );
    assert_eq!(persisted().unwrap(), changed);
    assert_eq!(shown()["rules"], "changed → another edit");
    println!(
        "PASS dictionary CLI -> owned socket -> Delegate -> isolated defaults: preview pages, merge, cancel, validation, snapshot, stale plan, private CSV round trip"
    );
}
