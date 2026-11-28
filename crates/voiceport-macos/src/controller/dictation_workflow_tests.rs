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
        c.language = "zh".into();
        let profile = if gui {
            StartProfile::Gui(None)
        } else {
            StartProfile::Pinned(SessionSource::ChatGpt, "zh".into())
        };
        let (generation, _) = c
            .prepare_start_with(profile, false, |_| {
                panic!("panel-only session cannot capture an input target")
            })
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
        c.language = "zh".into();
        let profile = if gui {
            StartProfile::Gui(target)
        } else {
            StartProfile::Pinned(SessionSource::ChatGpt, "zh".into())
        };
        let (generation, _) = c
            .prepare_start_with(profile, false, |_| {
                panic!("panel-only session cannot capture an input target")
            })
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

fn set_local_rule(c: &mut Controller, source_id: &str, language: &str, validate: bool) {
    use crate::dictation_app_rules::AppRules;
    c.workflows = Workflows::load(Some(
        &serde_json::json!({
            "presets": [{"name":"编辑", "source_id":source_id, "language":language,
                         "dictionary_enabled":false, "output_rules":{"trim_edges":true}}],
        })
        .to_string(),
    ))
    .unwrap();
    c.workflows.app_rules = AppRules::from_editor(
        validate,
        "本机 com.example.Editor → 编辑",
        &c.workflows.presets,
        &c.catalog,
    )
    .unwrap();
    c.workflows.app_rules.set_enabled(true);
}

fn editor() -> ApplicationTarget {
    ApplicationTarget {
        pid: 1,
        bundle: "com.example.Editor".into(),
    }
}

#[test]
fn app_local_rejects_unavailable_and_external_sources_without_dispatch() {
    use crate::{
        local_models::{Entry, Model, ModelKind},
        plugins,
    };
    let mut c = Controller::new();
    let store: plugins::Store = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "plugins": [{"schema_version":1,"protocol_version":1,"id":"fixture.jsonl",
            "version":"1","name":"Local-looking adapter","origin":"Synthetic",
            "adapter":"jsonl","capabilities":{"batch":true,"streaming":false,"system_preview":false},
            "fields":[], "program":{"executable":"/usr/bin/false","args":[]}}],
        "sources": [
            {"id":"fixture-http","name":"Loopback","plugin_id":plugins::HTTP,"plugin_version":"1",
             "enabled":true,"config":{"endpoint":"http://127.0.0.1:1/transcribe","model":"test"},"credential_refs":{}},
            {"id":"fixture-jsonl","name":"Local-looking adapter","plugin_id":"fixture.jsonl","plugin_version":"1",
             "enabled":true,"config":{},"credential_refs":{}}
        ],
    })).unwrap();
    store.validate().unwrap();
    c.catalog.add_configured(&store);
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join("absent");
    c.catalog.entries.push(Entry {
        id: "fixture-stale".into(),
        title: "Stale local".into(),
        detail: String::new(),
        source: Ok(SessionSource::Local(Model {
            kind: ModelKind::Paraformer,
            directory: absent.clone(),
            runtime: absent.clone(),
            encoder: absent.clone(),
            decoder: absent.clone(),
            tokens: absent,
            joiner: None,
        })),
    });

    for (id, language, expected, detail) in [
        (
            "missing",
            "zh",
            BlockReason::SourceUnavailable,
            "转写来源不可用",
        ),
        (CHATGPT, "zh", BlockReason::SourceUnavailable, "仅可选择"),
        (
            "fixture-http",
            "zh",
            BlockReason::SourceUnavailable,
            "仅可选择",
        ),
        (
            "fixture-jsonl",
            "zh",
            BlockReason::SourceUnavailable,
            "仅可选择",
        ),
        (
            "fixture-stale",
            "zh",
            BlockReason::SourceUnavailable,
            "缺少可读",
        ),
        (
            "fixture-stale",
            "ja",
            BlockReason::UnsupportedLanguage,
            "不支持场景",
        ),
        (
            "fixture-stale",
            "made-up",
            BlockReason::UnsupportedLanguage,
            "不支持场景",
        ),
    ] {
        set_local_rule(&mut c, id, language, false);
        c.transcript = "之前的结果".into();
        assert!(
            c.prepare_start_with(StartProfile::Gui(Some(&editor())), true, |_| {
                panic!("invalid app source reached target capture: {id}")
            })
            .is_none()
        );
        let receipt = c.gui_dictation_receipt().unwrap();
        assert_eq!(receipt.transcription, TranscriptionState::NotStarted);
        assert_eq!(receipt.block_reason, Some(expected), "{id}");
        assert!(
            c.app_source_issue().unwrap().1.contains(detail),
            "{id}: {}",
            c.message
        );
        assert!(c.message.contains("不会改用全局来源"));
        assert!(c.events.try_recv().is_err());
        assert!(c.worker.is_none() && c.capture.is_none() && c.recording.is_none());
        assert_eq!(c.transcript, "之前的结果");
    }
    c.workflows.presets.clear(); // Stale persisted mapping, unlike explicit scene deletion.
    assert!(
        c.prepare_start_with(
            StartProfile::Gui(Some(&editor())),
            false,
            |_| unreachable!()
        )
        .is_none()
    );
    assert!(c.app_source_issue().unwrap().1.contains("已不存在"));

    // Existing explicitly selected sources do not go through the automatic local gate.
    set_local_rule(&mut c, "missing", "zh", false);
    let unknown = ApplicationTarget { pid: 0, ..editor() };
    let own = ApplicationTarget {
        pid: std::process::id() as i32,
        ..editor()
    };
    let no_match = ApplicationTarget {
        bundle: "com.example.editor".into(),
        ..editor()
    };
    for target in [None, Some(&unknown), Some(&own), Some(&no_match)] {
        c.prepare_start_with(StartProfile::Gui(target), false, |_| unreachable!())
            .unwrap();
        assert_eq!(c.active_app_rule(), Some("全局规则"));
        assert_eq!(c.recorded_source, SessionSource::ChatGpt);
        c.cancel();
    }
    for id in [CHATGPT, "fixture-http", "fixture-jsonl"] {
        let source = c.catalog.snapshot(id).unwrap().pin().unwrap();
        c.source_id = id.into();
        c.language = "en".into();
        let mut manual = c.workflows.presets[0].clone();
        manual.name = format!("手动 {id}");
        manual.source_id = id.into();
        manual.language = "en".into();
        c.workflows.add_preset(manual.clone()).unwrap();
        c.workflows.apply_preset_output(&manual);
        c.prepare_start_with(
            StartProfile::Gui(Some(&editor())),
            false,
            |_| unreachable!(),
        )
        .unwrap();
        assert!(c.active_app_rule().unwrap().starts_with("手动场景"));
        assert_eq!(c.recorded_source, source);
        assert_eq!(c.recorded_language, "en");
        c.cancel();
        c.prepare_start(source.clone(), "ja".into(), false).unwrap();
        assert_eq!(c.recorded_source, source);
        assert_eq!(c.recorded_language, "ja");
        assert!(c.recorded_output.is_identity());
        c.cancel();
    }
    c.workflows.app_rules.set_enabled(true);
    assert!(
        c.prepare_start_with(
            StartProfile::Gui(Some(&editor())),
            false,
            |_| unreachable!()
        )
        .is_none()
    );
    assert!(c.app_source_issue().is_some());
}

#[cfg(all(debug_assertions, target_arch = "aarch64"))]
pub(super) fn verify_app_local_delivery_and_retry() {
    use crate::{
        audio::devices::fixture::Backend,
        controller::recent_dictations::fixture::{
            ReceiverState, SyntheticDictations, SyntheticTarget,
        },
        delivery::model::{Snapshot, TextRange},
        dictation_app_rules::AppRules,
        local_models::{LEGACY_RUNTIME_VERSION, ModelKind, PARAFORMER, RUNTIME_VERSION, title},
        local_runtime::{fixture_catalog, install_fixture_sherpa, probe},
        storage::{NAMESPACE, Storage},
    };
    use std::{cell::RefCell, fs, rc::Rc};
    assert!(objc2::MainThreadMarker::new().is_some());
    let temp = tempfile::tempdir().unwrap();
    let (catalog, [ort, sherpa]) = fixture_catalog(temp.path());
    let library_bytes = fs::read(&sherpa).unwrap();
    let cancel = CancellationToken::new();
    let storage = Storage::at(temp.path().canonicalize().unwrap().join(NAMESPACE));
    for retry in [false, true] {
        let mut rig = SyntheticDictations::new();
        let c = &mut rig.controller;
        c.catalog = catalog.clone();
        c.source_id = "unavailable-global".into();
        c.language = "en".into();
        set_local_rule(c, PARAFORMER, "zh", true);
        let saved = serde_json::to_string(&c.workflows).unwrap();
        c.workflows = Workflows::load(Some(&saved)).unwrap();
        if !retry {
            let backend = Backend::new(storage.clone());
            let asr = c.install_audio_fixture(backend.clone());
            c.automatic = false;
            c.start_from_shortcut(Some(&editor()));
            assert_eq!(c.active_app_source(), Some((title(PARAFORMER), "zh")));
            assert_eq!(
                c.events.try_recv().unwrap(),
                (
                    c.state.generation(),
                    AudioEvent::Permissions {
                        microphone: true,
                        speech: false,
                    }
                )
            );
            assert_eq!(backend.counts(), (0, 0, 0, 0));
            asr.assert_no_job();
            c.cancel();
        }
        let receiver = Rc::new(RefCell::new(ReceiverState {
            snapshot: Snapshot {
                text: "A|B".into(),
                selection: TextRange {
                    location: 1,
                    length: 1,
                },
            },
            writes: 0,
            confirm: true,
            failure: None,
        }));
        let (jobs, queued) = mpsc::sync_channel(1);
        let (replies, results) = mpsc::channel();
        c.worker = Some(Worker { jobs, results });
        let (generation, speech) = c
            .prepare_start_with(StartProfile::Gui(Some(&editor())), true, |delivery| {
                delivery.begin_fixture(Box::new(SyntheticTarget(receiver.clone())))
            })
            .unwrap();
        assert_eq!(c.active_app_source(), Some((title(PARAFORMER), "zh")));
        assert!(!speech);
        assert_eq!(c.source_id, "unavailable-global");
        assert_eq!(c.language, "en");
        let expected = c.recorded_source.clone();
        let SessionSource::Local(ref model) = expected else {
            panic!("configured native source must reach the worker");
        };
        let engine = c.recorded_app_engine.as_ref().unwrap().clone();
        let frozen_library = engine.path().join(sherpa.file_name().unwrap());
        assert_eq!(model.kind, ModelKind::Paraformer);
        assert_eq!(
            model.directory,
            temp.path().join("paraformer-bilingual-zh-en")
        );
        assert_eq!(model.runtime, engine.path());
        assert_ne!(frozen_library, sherpa);
        if retry {
            // The installed ABI changes after admission, while the recording keeps its checked copy.
            install_fixture_sherpa(temp.path(), &ort, &sherpa, RUNTIME_VERSION, false);
            assert_ne!(fs::read(&sherpa).unwrap(), library_bytes);
        }
        assert_eq!(fs::read(&frozen_library).unwrap(), library_bytes);
        let shown = c.gui_result_context().source.clone();
        assert_eq!(
            shown.as_deref(),
            Some(format!("{} · 中文 · 在本机处理", title(PARAFORMER)).as_str())
        );
        c.capture_started(generation, false, None, None);
        c.update_dictation_partial(generation, "未经处理的预览".into());
        assert_eq!(receiver.borrow().writes, 0);
        let recording = Arc::new(storage.create_recording().unwrap());
        c.finish_capture(Ok(recording.clone()));
        let check_job = |job: Job, generation| {
            assert_eq!(job.generation, generation);
            assert_eq!(job.source, expected);
            assert_eq!(job.language, "zh");
            assert!(job.proxy.is_none() && job.incoming.is_none());
            assert!(Arc::ptr_eq(job.recording.as_ref().unwrap(), &recording));
            assert!(Arc::ptr_eq(job._app_engine.as_ref().unwrap(), &engine));
            probe(model, &job.language, &cancel).unwrap();
        };
        check_job(queued.try_recv().unwrap(), generation);
        c.source_id = CHATGPT.into();
        c.catalog = Catalog::pending(CHATGPT);
        c.workflows = Workflows::default();
        let final_generation = if retry {
            replies
                .send(Completion {
                    generation,
                    elapsed: None,
                    result: Err("synthetic engine failure".into()),
                })
                .unwrap();
            c.poll();
            assert!(c.can_retry_recording());
            assert_eq!(
                c.app_source_issue().unwrap().0,
                "场景本机转写失败 · 未切换来源"
            );
            c.retry();
            let next = c.state.generation();
            assert_ne!(next, generation);
            assert_eq!(c.active_app_source(), Some((title(PARAFORMER), "zh")));
            check_job(queued.try_recv().unwrap(), next);
            next
        } else {
            generation
        };
        replies
            .send(Completion {
                generation: final_generation,
                elapsed: None,
                result: Ok("  中🙂  ".into()),
            })
            .unwrap();
        c.poll();
        assert_eq!(c.transcript, "中🙂");
        assert_eq!(c.gui_result_context().source, shown);
        assert_eq!(c.app_source_issue(), None);
        let receipt = c.gui_dictation_receipt().unwrap();
        assert_eq!(receipt.transcription, TranscriptionState::Completed);
        let target = receiver.borrow();
        if retry {
            assert_eq!(receipt.block_reason, Some(BlockReason::RetryPanelOnly));
            assert_eq!(target.writes, 0);
            assert_eq!(target.snapshot.text, "A|B");
        } else {
            assert_eq!(receipt.delivery.confirmed_operations, 1);
            assert_eq!(target.writes, 1);
            assert_eq!(target.snapshot.text, "A中🙂B");
        }
        assert!(queued.try_recv().is_err());
        assert!(c.recorded_app_engine.is_none());
        drop(engine);
        assert!(!frozen_library.exists());
    }
    println!(
        "app-local source: synthetic native decode passed on the original and retry jobs' shared engine after the installed ABI changed; retry wrote nothing"
    );

    let assert_engine_rejected = |diagnostic: &str| {
        let SessionSource::Local(model) = catalog.snapshot(PARAFORMER).unwrap() else {
            unreachable!();
        };
        model.validate().unwrap(); // Header-only discovery still sees this entry.
        let native_error = probe(&model, "zh", &cancel).unwrap_err();
        assert!(native_error.contains(diagnostic), "{native_error}");

        let mut rejected = Controller::new();
        rejected.catalog = catalog.clone();
        set_local_rule(&mut rejected, PARAFORMER, "zh", false);
        let save_error = AppRules::from_editor(
            true,
            &rejected.workflows.app_rules.editor_text(),
            &rejected.workflows.presets,
            &rejected.catalog,
        )
        .expect_err("native runtime rejection must also reject saving the enabled rule");
        assert!(save_error.contains(diagnostic), "{save_error}");
        let backend = Backend::new(storage.clone());
        let asr = rejected.install_audio_fixture(backend.clone());
        rejected.automatic = false;
        rejected.start_from_shortcut(Some(&editor()));
        let receipt = rejected.gui_dictation_receipt().unwrap();
        assert_eq!(rejected.state.phase, Phase::Failed);
        assert_eq!(receipt.transcription, TranscriptionState::NotStarted);
        assert_eq!(receipt.block_reason, Some(BlockReason::SourceUnavailable));
        let (_, error) = rejected.app_source_issue().unwrap();
        assert!(error.contains(diagnostic), "{error}");
        assert!(rejected.recorded_app_engine.is_none());
        assert!(rejected.events.try_recv().is_err());
        rejected.poll();
        assert_eq!(backend.counts(), (0, 0, 0, 0));
        asr.assert_no_job();
        println!(
            "app-local engine: {diagnostic} rejected at save and before permission, device or ASR"
        );
    };
    assert_engine_rejected("Unsupported sherpa-onnx ABI: 2.0.1 (requires 1.12.37)");
    install_fixture_sherpa(temp.path(), &ort, &sherpa, LEGACY_RUNTIME_VERSION, true);
    assert_engine_rejected("Missing runtime symbol: SherpaOnnxDecodeOfflineStream");
    install_fixture_sherpa(temp.path(), &ort, &sherpa, LEGACY_RUNTIME_VERSION, false);
    fs::OpenOptions::new()
        .write(true)
        .open(&ort)
        .unwrap()
        .set_len(8)
        .unwrap();
    assert_engine_rejected("ONNX Runtime:");
}

#[cfg(debug_assertions)]
#[test]
fn app_rules_manual_scene_repaste_guards_the_main_thread() {
    verify_manual_scene_history_and_repaste(false);
}

#[cfg(debug_assertions)]
pub(super) fn verify_manual_scene_history_and_repaste(on_main: bool) {
    use crate::{
        controller::recent_dictations::fixture::{SyntheticDictations, SyntheticTarget},
        dictation_dictionary::parse_rules,
        dictation_output::OutputRules,
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
    assert!(
        c.search_recent_dictations(5, "voiceport", Some(HistoryDelivery::Confirmed))
            .is_empty()
    );

    assert_eq!(objc2::MainThreadMarker::new().is_some(), on_main);
    let original_receipt = c.gui_dictation_receipt().unwrap();
    assert_eq!(receiver.borrow().writes, 0);
    let repaste = c.repaste_latest_with(|delivery| {
        delivery.begin_fixture(Box::new(SyntheticTarget(receiver.clone())))
    });
    println!(
        "manual-scene repaste: os_main_thread={on_main}, response={repaste:?}, writes={}",
        receiver.borrow().writes
    );
    if on_main {
        assert!(repaste.error.is_none(), "{repaste:?}");
        assert_eq!(repaste.data["delivery"]["state"], "confirmed");
        assert_eq!(receiver.borrow().snapshot.text, format!("A{FINAL}B"));
        assert_eq!(receiver.borrow().writes, 1);
        let duplicate = c.repaste_latest_with(|_| panic!("duplicate must not capture a target"));
        assert_eq!(duplicate.error.unwrap().code, "possible_duplicate");
        assert_eq!(receiver.borrow().writes, 1);
    } else {
        assert_eq!(repaste.error.unwrap().code, "paste_blocked");
        assert_eq!(repaste.data["failure_stage"], "mainThread");
        assert_eq!(receiver.borrow().snapshot.text, "A|B");
        assert_eq!(receiver.borrow().writes, 0);
    }
    assert_eq!(repaste.data["generation"], generation);
    assert_eq!(c.repaste_receipt().as_ref(), Some(&repaste.data));
    assert_eq!(c.gui_dictation_receipt().unwrap(), original_receipt);
    assert_eq!(
        c.search_recent_dictations(5, "voiceport", Some(HistoryDelivery::NotSent))[0].id,
        entries[0].id
    );

    let (next, _) = rig.begin_with_rule_target(true, false, Some(&editor));
    assert_eq!(rig.controller.active_app_rule(), Some("场景「绑定」"));
    rig.finish(next, RAW);
    assert_eq!(rig.controller.transcript, RAW);
}
