//! Exercise production start admission and completion with a synthetic worker.
use super::*;

fn complete(c: &mut Controller, generation: u64, original: &str) {
    let (jobs, _incoming) = mpsc::sync_channel(1);
    let (outgoing, results) = mpsc::channel();
    c.worker = Some(Worker { jobs, results });
    c.active = Some(ActiveJob {
        generation,
        cancel: CancellationToken::new(),
        custody: Custody::default(),
        _recording: None,
    });
    outgoing
        .send(Completion {
            generation,
            elapsed: None,
            result: Ok(original.into()),
        })
        .unwrap();
    c.poll();
    assert_eq!(c.state.phase, Phase::Ready);
}

#[test]
fn dictation_workflow_frozen_gui_profile_preserves_original_and_keeps_cli_literal() {
    use crate::dictation_dictionary::parse_rules;

    let mut c = Controller::new();
    c.workflows.rules = parse_rules("北经 → 北京\nvp → VoicePort").unwrap();
    c.workflows.dictionary_enabled = true;
    const ORIGINAL: &str = "  北经的vp🙂\r\n第二行 ";
    for gui in [true, false] {
        c.workflows.output_rules.trim_edges = true;
        c.workflows.output_rules.normalize_newlines = true;
        assert!(c.state.can_start());
        let (generation, _) = c
            .prepare_start_with(
                SessionSource::ChatGpt,
                "zh".into(),
                false,
                gui,
                None,
                |_| panic!("panel-only session cannot capture an input target"),
            )
            .unwrap();
        if !gui {
            assert_eq!(c.raw_transcript.as_deref(), Some(ORIGINAL));
        }
        c.capture_started(generation, false, None, None);
        assert!(c.raw_transcript.is_none());
        assert!(c.state.transcribe());
        c.workflows.rules = parse_rules("北经 → 南京\nvp → later").unwrap();
        c.workflows.output_rules = Default::default();
        c.source_id = "later-source".into();
        c.language = "en".into();
        assert_eq!(c.recorded_source, SessionSource::ChatGpt);
        assert_eq!(c.recorded_language, "zh");

        complete(&mut c, generation, ORIGINAL);
        if gui {
            assert_eq!(c.transcript, "北京的VoicePort🙂\n第二行");
            assert_eq!(c.raw_transcript.as_deref(), Some(ORIGINAL));
        } else {
            assert_eq!(c.transcript, ORIGINAL);
            assert!(c.raw_transcript.is_none());
        }
    }
}

#[test]
fn app_rules_gui_start_freezes_scene_or_global_output_only() {
    use crate::{
        dictation_dictionary::parse_rules, dictation_output::OutputRules,
        dictation_workflows::Preset,
    };

    let editor = ApplicationTarget {
        pid: 1,
        bundle: "com.example.Editor".into(),
    };
    let different_case = ApplicationTarget {
        bundle: "com.example.editor".into(),
        ..editor.clone()
    };
    let own = ApplicationTarget {
        pid: std::process::id() as i32,
        ..editor.clone()
    };
    const ORIGINAL: &str = "  vp\r\n🙂  ";
    const GLOBAL: &str = "  VoicePort\n🙂  ";
    for (gui, enabled, target, expected, rule) in [
        (true, true, Some(&editor), "vp\r\n🙂", Some("场景「编辑」")),
        (true, true, None, GLOBAL, Some("全局规则")),
        (true, true, Some(&different_case), GLOBAL, Some("全局规则")),
        (true, true, Some(&own), GLOBAL, Some("全局规则")),
        (true, false, Some(&editor), GLOBAL, None),
        (false, true, Some(&editor), ORIGINAL, None),
    ] {
        let mut c = Controller::new();
        c.workflows.rules = parse_rules("vp → VoicePort").unwrap();
        c.workflows.dictionary_enabled = true;
        c.workflows.output_rules.normalize_newlines = true;
        c.workflows
            .add_preset(Preset {
                name: "编辑".into(),
                source_id: "unavailable-fixture-source".into(),
                language: "ja".into(),
                dictionary_enabled: false,
                output_rules: OutputRules {
                    trim_edges: true,
                    ..Default::default()
                },
            })
            .unwrap();
        c.workflows.app_rules.enabled = enabled;
        c.workflows
            .app_rules
            .assign(&editor.bundle, "编辑", &c.workflows.presets)
            .unwrap();
        let (generation, _) = c
            .prepare_start_with(
                SessionSource::ChatGpt,
                "zh".into(),
                false,
                gui,
                target,
                |_| panic!("panel-only session cannot capture an input target"),
            )
            .unwrap();
        assert_eq!(c.active_app_rule(), rule);
        // Change the binding, switches and dictionary before either preview or final text.
        c.workflows = Workflows::default();
        c.source_id = "later-source".into();
        c.language = "en".into();
        c.capture_started(generation, false, None, None);
        assert_eq!(c.active_app_rule(), rule);
        assert_eq!(c.recorded_source, SessionSource::ChatGpt);
        assert_eq!(c.recorded_language, "zh");
        assert!(c.state.transcribe());
        complete(&mut c, generation, ORIGINAL);
        assert_eq!(c.transcript, expected);
        assert_eq!(c.raw_transcript.as_deref(), gui.then_some(ORIGINAL));
        assert_eq!(c.active_app_rule(), None);
    }
}

#[cfg(debug_assertions)]
#[test]
fn app_rules_manual_scene_preserves_processed_history_and_repaste() {
    use crate::{
        controller::recent_dictations::fixture::{SyntheticDictations, SyntheticTarget},
        dictation_dictionary::parse_rules, dictation_output::OutputRules,
        dictation_workflows::Preset,
    };
    use voiceport_control::HistoryDelivery;

    let mut rig = SyntheticDictations::new();
    let editor = ApplicationTarget {
        pid: 1,
        bundle: "com.example.Editor".into(),
    };
    let manual = Preset {
        name: "手选".into(),
        source_id: CHATGPT.into(),
        language: "zh".into(),
        dictionary_enabled: true,
        output_rules: OutputRules {
            trim_edges: true,
            ..Default::default()
        },
    };
    let bound = Preset {
        name: "绑定".into(),
        dictionary_enabled: false,
        output_rules: OutputRules::default(),
        ..manual.clone()
    };
    let c = &mut rig.controller;
    c.set_recent_dictations_enabled(true);
    c.workflows.rules = parse_rules("vp → VoicePort").unwrap();
    c.workflows.add_preset(manual.clone()).unwrap();
    c.workflows.add_preset(bound).unwrap();
    c.workflows.app_rules.set_enabled(true);
    c.workflows
        .app_rules
        .assign(&editor.bundle, "绑定", &c.workflows.presets)
        .unwrap();
    c.workflows.apply_preset_output(&manual);
    let saved = serde_json::to_string(&c.workflows).unwrap();
    c.workflows = Workflows::load(Some(&saved)).unwrap();

    let (generation, receiver) = rig.begin_with_rule_target(true, false, Some(&editor));
    assert_eq!(rig.controller.active_app_rule(), Some("手动场景「手选」"));
    rig.controller.workflows.app_rules.set_enabled(true);
    assert_eq!(rig.controller.active_app_rule(), Some("手动场景「手选」"));
    const RAW: &str = "  vp🙂  ";
    const FINAL: &str = "VoicePort🙂";
    rig.finish(generation, RAW);
    let c = &mut rig.controller;
    assert_eq!(c.transcript, FINAL);
    assert_eq!(c.raw_transcript.as_deref(), Some(RAW));
    let entries = c.search_recent_dictations(5, "voiceport", Some(HistoryDelivery::NotSent));
    assert_eq!(entries.len(), 1);
    assert_eq!(c.read_recent_dictation(&entries[0].id), Some(FINAL));
    assert!(c.search_recent_dictations(5, "vp", None).is_empty());
    assert!(c.search_recent_dictations(5, "voiceport", Some(HistoryDelivery::Confirmed)).is_empty());

    let original_receipt = c.gui_dictation_receipt().unwrap();
    let repaste = c.repaste_latest_with(|delivery| {
        delivery.begin_fixture(Box::new(SyntheticTarget(receiver.clone())))
    });
    assert!(repaste.error.is_none());
    assert_eq!(repaste.data["delivery"]["state"], "confirmed");
    assert_eq!(receiver.borrow().snapshot.text, format!("A{FINAL}B"));
    assert_eq!(receiver.borrow().writes, 1);
    assert_eq!(c.gui_dictation_receipt().unwrap(), original_receipt);
    assert_eq!(c.search_recent_dictations(5, "voiceport", Some(HistoryDelivery::NotSent))[0].id, entries[0].id);
    let duplicate = c.repaste_latest_with(|_| panic!("duplicate must not capture a target"));
    assert_eq!(duplicate.error.unwrap().code, "possible_duplicate");

    let (next, _) = rig.begin_with_rule_target(true, false, Some(&editor));
    assert_eq!(rig.controller.active_app_rule(), Some("场景「绑定」"));
    rig.finish(next, RAW);
    assert_eq!(rig.controller.transcript, RAW);
}
