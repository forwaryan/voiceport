use super::*;
use crate::{
    dictation_dictionary::{RULE_LIMIT, Rule, parse_rules},
    dictation_workflows::Workflows,
    dictionary_exchange::format_csv,
};

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
    let capabilities = call(&["app", "capabilities"], 0);
    assert!(capabilities["data"]["dictionary_exchange"].is_array());
    assert!(capabilities["data"]["dictionary_output"].is_array());
    for rule in ["trim-edges", "normalize-newlines"] {
        call(&["dictionary", "output", "set", rule, "on"], 0);
    }
    let switches = call(&["dictionary", "output", "show"], 0)["data"].clone();
    assert_eq!(switches["trim_edges"], true);
    assert_eq!(switches["normalize_newlines"], true);
    let applied = call(&["dictionary", "import-apply", id, "--confirm"], 0);
    assert_eq!(applied["data"]["rule_count"], 18);
    let saved = persisted().unwrap();
    assert_eq!(saved["presets"][0]["name"], "Synthetic");
    assert_eq!(saved["dictionary_enabled"], true);
    assert_eq!(saved["output_rules"], switches);
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
    verify_large_readback(&fixture, &folder);
    println!(
        "PASS dictionary CLI -> owned socket -> Delegate -> isolated defaults: preview pages, merge, cancel, validation, snapshot, stale plan, private CSV round trip, maximum dictionary readback"
    );
}

fn verify_large_readback(fixture: &Fixture<'_>, folder: &Path) {
    let rules: Vec<_> = (0..RULE_LIMIT)
        .map(|index| Rule {
            input: format!("词{index:03}{}", "🦀".repeat(124)),
            output: "🦀".repeat(512),
        })
        .collect();
    let csv = folder.join("maximum.csv");
    fs::write(&csv, format_csv(&rules)).unwrap();
    let call = |words: &[&str], exit| fixture.call(words, exit);
    let import = call(
        &["dictionary", "import", csv.to_str().unwrap(), "--replace"],
        0,
    );
    let id = import["data"]["operation"].as_str().unwrap();
    assert_eq!(
        call(&["dictionary", "import-apply", id, "--confirm"], 0)["data"]["rule_count"],
        RULE_LIMIT
    );
    let legacy = call(&["dictionary", "show"], 1);
    assert_eq!(legacy["error"]["code"], "frame_too_large");
    assert!(
        legacy["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--offset 0")
    );

    let first = call(&["dictionary", "show", "--offset", "0"], 0);
    let revision = first["data"]["revision"].as_str().unwrap();
    let mut readback = first["data"]["rules"].as_array().unwrap().clone();
    let mut next = first["data"]["next_offset"].as_u64();
    while let Some(offset) = next {
        assert_eq!(offset as usize, readback.len());
        let page = call(
            &[
                "dictionary",
                "show",
                "--offset",
                &offset.to_string(),
                "--revision",
                revision,
            ],
            0,
        );
        let page = &page["data"];
        assert_eq!(page["enabled"], true);
        assert_eq!(page["rule_count"], RULE_LIMIT);
        assert_eq!(page["revision"], revision);
        readback.extend_from_slice(page["rules"].as_array().unwrap());
        next = page["next_offset"].as_u64();
    }
    assert_eq!(
        serde_json::from_value::<Vec<Rule>>(Value::Array(readback)).unwrap(),
        rules
    );
    assert_eq!(
        call(&["dictionary", "show", "--offset", "16"], 2)["error"]["code"],
        "invalid_arguments"
    );
    fixture.owner.with_controller(|c| {
        let mut next = c.workflows.clone();
        next.dictionary_enabled = false;
        fixture.owner.commit_workflows(c, next).unwrap();
    });
    assert_eq!(
        call(
            &[
                "dictionary",
                "show",
                "--offset",
                "16",
                "--revision",
                revision
            ],
            1
        )["error"]["code"],
        "invalid_state"
    );
}
