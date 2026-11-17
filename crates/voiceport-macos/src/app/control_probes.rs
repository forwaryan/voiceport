//! CLI ownership and content-free reports for the shared independent probe engine.
use crate::{
    controller::Controller,
    local_models::{Entry, SessionSource},
    source_tests::Status,
};
use serde_json::json;
use voiceport_control::Response;

pub(super) struct ProbeOperation {
    pub id: String,
    generation: u64,
    entries: Vec<Entry>,
    language: String,
    cancelled: bool,
    result: Option<Response>,
}
impl ProbeOperation {
    pub fn start(
        c: &mut Controller,
        source: Option<String>,
        language: String,
        accept_upload: bool,
    ) -> Result<Self, Response> {
        if !voiceport_core::language::supported(&language) {
            return Err(Response::error("invalid_request", "Unsupported language."));
        }
        let batch = source.is_none();
        let entries: Vec<_> = match source {
            Some(id) => vec![
                c.catalog
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .cloned()
                    .ok_or_else(|| {
                        Response::error("not_found", "Source does not exist in this instance.")
                    })?,
            ],
            None => c.catalog.entries.clone(),
        };
        if entries.is_empty() {
            return Err(Response::error(
                "invalid_config",
                "No sources are available to inspect.",
            ));
        }
        if !accept_upload {
            let destinations: Vec<_> = entries.iter().map(|entry| {
                let mut value = json!({"source":entry.id,"name":entry.title,"available":entry.source.is_ok()});
                match &entry.source {
                    Ok(SessionSource::ChatGpt) => { value["kind"] = json!("chatgpt"); }
                    Ok(SessionSource::Local(_)) => { value["kind"] = json!("local"); }
                    Ok(SessionSource::Plugin(p)) => {
                        value["kind"] = json!("plugin");
                        value["plugin"] = json!(p.manifest.id);
                        value["plugin_version"] = json!(p.manifest.version);
                        // A valid program can have large argv shared by 128 sources.
                        // Keep the batch preview bounded; inspect exact destinations
                        // and programs through sources show / plugins show before acceptance.
                        if !batch {
                            value["endpoint"] = json!(p.source.config.get("endpoint"));
                            value["program"] = json!(p.manifest.program);
                            value["origin"] = json!(p.manifest.origin);
                        }
                    }
                    Err(_) => { value["kind"] = json!("unavailable"); }
                }
                value
            }).collect();
            let mut response = Response::error(
                "confirmation_required",
                "Explicit --accept-upload is required to test synthetic silence; cloud sources may upload audio and adapters execute with current-user privileges.",
            );
            response.data = json!({"sources":destinations,"batch":batch,"details":if batch {Some("Inspect sources show ID and plugins show PLUGIN for each configured destination and program before accepting all sources.")}else{None},"language":if language.is_empty(){"auto"}else{&language},"sample":"one_second_silence","microphone":false,"delivery":"none"});
            return Err(response);
        }
        let ids: Vec<_> = entries.iter().map(|entry| entry.id.clone()).collect();
        c.start_source_tests_with_language(&ids, language.clone())
            .map_err(|_| {
                Response::error(
                    "busy",
                    "Source tests could not start; inspect status before another action.",
                )
            })?;
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            generation: c.source_tests.generation(),
            entries,
            language,
            cancelled: false,
            result: None,
        })
    }
    pub fn active_id(&self) -> Option<&str> {
        self.result.is_none().then_some(self.id.as_str())
    }
    pub fn settle(&mut self, c: &Controller) {
        if self.result.is_some() {
            return;
        }
        if c.source_tests.generation() != self.generation {
            self.result = Some(Response::error(
                "not_found",
                "Source probe was superseded by another test run.",
            ));
        } else if !c.source_tests.busy() {
            self.result = Some(self.report(c));
        }
    }
    pub fn result(&mut self, c: &Controller) -> Response {
        self.settle(c);
        self.result.clone().unwrap_or_else(|| self.report(c))
    }
    pub fn cancel(&mut self, c: &mut Controller) -> Response {
        self.settle(c);
        if self.result.is_some() {
            return Response::error("invalid_state", "Source probe is no longer active.");
        }
        self.cancelled = true;
        c.source_tests.cancel();
        self.result(c)
    }
    fn report(&self, c: &Controller) -> Response {
        let busy = c.source_tests.busy();
        let cleanup_pending = c.source_tests.cleanup_pending();
        let pending = busy && (self.cancelled || cleanup_pending);
        let rows: Vec<_> = self.entries.iter().map(|entry| {
            let result = c.source_tests.row(entry);
            json!({
                "source":entry.id,
                "status":result.map_or("unavailable", |r| match r.status {
                    Status::Pending => "pending", Status::Running => "running", Status::Success => "success",
                    Status::Failed => "failed", Status::TimedOut => "timed_out", Status::Cancelled => "cancelled", Status::CleanupPending => "cleanup_pending",
                }),
                "elapsed_ms":result.and_then(|r| r.elapsed).map(|elapsed| elapsed.as_millis() as u64),
                "completed_at_unix_ms":result.and_then(|r| r.completed_at).and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|elapsed| elapsed.as_millis() as u64),
            })
        }).collect();
        let all_passed = !busy && rows.iter().all(|row| row["status"] == "success");
        Response::ok(
            json!({"operation":self.id,"state":if pending {"stopping"} else if busy {"running"} else {"completed"},"busy":busy,"pending":pending,"warning":if cleanup_pending {Some("cleanup_pending")}else{None},"language":if self.language.is_empty(){"auto"}else{&self.language},"all_passed":all_passed,"results":rows,"microphone":false,"delivery":"none"}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        local_models::{CHATGPT, Model, ModelKind},
        source_tests::SourceTests,
    };
    use std::{
        sync::{Arc, Mutex, mpsc},
        time::{Duration, Instant},
    };

    fn controller() -> Controller {
        let mut c = Controller::new();
        c.catalog.entries = vec![
            Entry {
                id: CHATGPT.into(),
                title: "Cloud".into(),
                detail: String::new(),
                source: Ok(SessionSource::ChatGpt),
            },
            Entry {
                id: "local".into(),
                title: "Local".into(),
                detail: String::new(),
                source: Ok(SessionSource::Local(Model {
                    kind: ModelKind::Whisper,
                    directory: "never-opened".into(),
                    runtime: "never-opened".into(),
                    encoder: "never-opened".into(),
                    decoder: "never-opened".into(),
                    tokens: "never-opened".into(),
                    joiner: None,
                })),
            },
            Entry {
                id: "disabled".into(),
                title: "Disabled".into(),
                detail: String::new(),
                source: Err("PRIVATE_CONFIGURATION".into()),
            },
        ];
        c.transcript = "PRIVATE_TRANSCRIPT".into();
        c.language = "zh".into();
        c
    }
    fn finish(c: &mut Controller, op: &mut ProbeOperation) -> Response {
        let deadline = Instant::now() + Duration::from_secs(5);
        while c.source_tests.busy() && Instant::now() < deadline {
            c.source_tests.poll();
            std::thread::yield_now();
        }
        assert!(!c.source_tests.busy());
        op.result(c)
    }
    #[test]
    fn consent_and_validation_do_not_dispatch_and_reports_preserve_dictation() {
        let mut c = controller();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        c.source_tests =
            SourceTests::with_executor(Duration::from_secs(5), move |source, language, _, _| {
                seen.lock().unwrap().push((source, language.to_owned()));
                Ok(())
            });
        assert_eq!(
            ProbeOperation::start(&mut c, None, "en".into(), false)
                .err()
                .unwrap()
                .exit_code(),
            5
        );
        assert_eq!(
            ProbeOperation::start(&mut c, Some("missing".into()), String::new(), true)
                .err()
                .unwrap()
                .exit_code(),
            6
        );
        assert_eq!(
            ProbeOperation::start(&mut c, None, "invalid".into(), true)
                .err()
                .unwrap()
                .exit_code(),
            2
        );
        assert!(calls.lock().unwrap().is_empty());
        let generation = c.state.generation();
        let mut op = ProbeOperation::start(&mut c, None, "en".into(), true).unwrap();
        let report = finish(&mut c, &mut op);
        assert_eq!(report.data["state"], "completed");
        assert_eq!(report.data["all_passed"], false);
        assert_eq!(report.data["language"], "en");
        assert_eq!(report.data["results"][0]["status"], "success");
        assert!(report.data["results"][0]["elapsed_ms"].is_number());
        assert!(report.data["results"][0]["completed_at_unix_ms"].is_number());
        assert_eq!(report.data["results"][2]["status"], "failed");
        assert_eq!(calls.lock().unwrap().len(), 2);
        assert!(
            calls
                .lock()
                .unwrap()
                .iter()
                .all(|(_, language)| language == "en")
        );
        assert_eq!(c.transcript, "PRIVATE_TRANSCRIPT");
        assert_eq!(c.language, "zh");
        assert_eq!(c.state.generation(), generation);
        assert!(!serde_json::to_string(&report).unwrap().contains("PRIVATE_"));
        // A later GUI probe cannot replace the completed CLI report.
        c.start_source_tests(&[CHATGPT.into()]).unwrap();
        assert_eq!(op.result(&c).data, report.data);
        c.source_tests.cancel();
        let _ = finish(&mut c, &mut op);
    }
    #[test]
    fn batch_confirmation_fits_the_wire_with_many_sources_sharing_large_argv() {
        use crate::plugins::{Adapter, Capabilities, Manifest, Program, Source, Store};
        let manifest = Manifest {
            schema_version: 1,
            protocol_version: 1,
            id: "lab.large-program".into(),
            version: "1".into(),
            name: "Synthetic adapter".into(),
            origin: "Synthetic fixture; never executed".into(),
            adapter: Adapter::Jsonl,
            capabilities: Capabilities {
                batch: true,
                streaming: false,
                system_preview: false,
            },
            fields: vec![],
            program: Some(Program {
                executable: "/usr/bin/true".into(),
                args: vec!["x".repeat(4096)],
            }),
        };
        let store = Store {
            schema_version: 1,
            plugins: vec![manifest],
            sources: (0..128)
                .map(|index| Source {
                    id: format!("{index:03}{}", "s".repeat(117)),
                    name: "n".repeat(128),
                    plugin_id: "lab.large-program".into(),
                    plugin_version: "1".into(),
                    enabled: true,
                    config: Default::default(),
                    credential_refs: Default::default(),
                })
                .collect(),
        };
        store.validate().unwrap();
        assert!(serde_json::to_vec(&store).unwrap().len() < crate::plugins::MAX_CONFIG as usize);
        let mut c = controller();
        c.catalog.add_configured(&store);
        c.source_tests = SourceTests::with_executor(Duration::from_secs(5), |_, _, _, _| {
            panic!("Confirmation must not execute an adapter")
        });
        let response = ProbeOperation::start(&mut c, None, String::new(), false)
            .err()
            .unwrap();
        assert_eq!(response.exit_code(), 5);
        assert!(serde_json::to_vec(&response).unwrap().len() < voiceport_control::MAX_FRAME);
        assert_eq!(response.data["batch"], true);
        assert!(
            response.data["details"]
                .as_str()
                .unwrap()
                .contains("plugins show")
        );
        let rows = response.data["sources"].as_array().unwrap();
        assert_eq!(
            rows.iter().filter(|row| row["kind"] == "plugin").count(),
            128
        );
        assert!(rows.iter().all(|row| row.get("program").is_none()));
        let single = ProbeOperation::start(
            &mut c,
            Some(store.sources[0].id.clone()),
            String::new(),
            false,
        )
        .err()
        .unwrap();
        assert_eq!(
            single.data["sources"][0]["program"]["args"][0]
                .as_str()
                .unwrap()
                .len(),
            4096
        );
        assert_eq!(c.source_tests.generation(), 0);
        assert!(!c.source_tests.busy());
    }
    #[test]
    fn cancellation_waits_for_exit_and_cannot_cancel_a_later_gui_run() {
        let mut c = controller();
        let (started_tx, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        c.source_tests =
            SourceTests::with_executor(Duration::from_secs(5), move |_, _, token, _| {
                started_tx.send(token.clone()).unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                Ok(())
            });
        let mut op =
            ProbeOperation::start(&mut c, Some(CHATGPT.into()), String::new(), true).unwrap();
        let token = started.recv_timeout(Duration::from_secs(5)).unwrap();
        let response = op.cancel(&mut c);
        assert_eq!(response.data["state"], "stopping");
        assert_eq!(response.data["busy"], true);
        assert!(token.is_cancelled());
        assert!(c.start_source_tests(&[CHATGPT.into()]).is_err());
        release.send(()).unwrap();
        assert_eq!(
            finish(&mut c, &mut op).data["results"][0]["status"],
            "cancelled"
        );
        assert_eq!(op.cancel(&mut c).error.unwrap().code, "invalid_state");
        // Deliberately leave another completed CLI run uncollected before GUI starts.
        c.source_tests = SourceTests::with_executor(Duration::from_secs(5), |_, _, _, _| Ok(()));
        let mut stale =
            ProbeOperation::start(&mut c, Some(CHATGPT.into()), String::new(), true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while c.source_tests.busy() && Instant::now() < deadline {
            c.source_tests.poll();
            std::thread::yield_now();
        }
        c.start_source_tests(&[CHATGPT.into()]).unwrap();
        assert_eq!(stale.cancel(&mut c).error.unwrap().code, "invalid_state");
        assert_eq!(stale.result(&c).error.unwrap().code, "not_found");
        c.source_tests.cancel();
        let _ = finish(&mut c, &mut stale);
    }
}
