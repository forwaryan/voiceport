//! Exercise production start admission and completion with a synthetic worker.
#[test]
fn dictation_workflow_frozen_gui_profile_preserves_original_and_keeps_cli_literal() {
    use super::*;
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
            .prepare_start_with(SessionSource::ChatGpt, "zh".into(), false, gui, |_| {
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
                result: Ok(ORIGINAL.into()),
            })
            .unwrap();
        c.poll();
        assert_eq!(c.state.phase, Phase::Ready);
        if gui {
            assert_eq!(c.transcript, "北京的VoicePort🙂\n第二行");
            assert_eq!(c.raw_transcript.as_deref(), Some(ORIGINAL));
        } else {
            assert_eq!(c.transcript, ORIGINAL);
            assert!(c.raw_transcript.is_none());
        }
    }
}
