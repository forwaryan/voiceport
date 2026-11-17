//! Main-thread IPC dispatch. Native Controller is never moved to a socket thread.
use super::*;
use crate::{
    local_models::SessionSource,
    plugins::{FieldKind, Manifest, Source, Store},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::mpsc::{self, Receiver},
};
use voiceport_control::{
    Action, Response,
    transport::{Pending, Server},
};

#[derive(Default)]
pub(super) struct Control {
    pub server: Option<Server>,
    background: Option<Receiver<Completed>>,
    operation: Option<Operation>,
    probe: Option<super::control_probes::ProbeOperation>,
}
impl Control {
    pub(super) fn preparing(&self) -> bool {
        self.background.is_some()
    }
}
struct Operation {
    id: String,
    generation: u64,
    cancelled: bool,
    plugin: bool,
    sample: bool,
    result: Option<Response>,
}
impl Operation {
    fn retry_available(&self, c: &Controller) -> bool {
        !self.cancelled
            && !self.sample
            && self.generation == c.state.generation()
            && c.can_retry_recording()
    }
    fn settle(&mut self, c: &Controller) {
        if self.result.is_some() || c.busy() {
            return;
        }
        self.result = Some(if c.state.generation() != self.generation {
            err("not_found", "Operation was superseded by another session.")
        } else if self.cancelled && c.state.phase == Phase::Idle {
            err(
                "cancelled",
                "Operation cancelled; Controller released its active job.",
            )
        } else if c.state.phase == Phase::Ready {
            Response::ok(
                json!({"operation":self.id,"state":"ready","text":c.transcript,"empty":c.transcript.is_empty(),"delivery":"none"}),
            )
        } else {
            let mut response = err(
                if self.plugin {
                    "plugin_error"
                } else {
                    "transcription_error"
                },
                "Operation failed. No service response body or credentials are included.",
            );
            response.data = json!({"operation":self.id,"state":"failed","retry_available":self.retry_available(c),"delivery":"none"});
            response
        });
    }
    fn result(&self, c: &Controller) -> Response {
        if let Some(mut response) = self.result.clone() {
            // A cached error may survive a later GUI session; never advertise
            // its new recording as retryable with this old operation ID.
            if response.data.get("retry_available").is_some() {
                response.data["retry_available"] = self.retry_available(c).into();
            }
            response
        } else if self.generation != c.state.generation() {
            missing()
        } else {
            Response::ok(
                json!({"operation":self.id,"state":phase(c.state.phase),"busy":c.busy(),"pending":c.state.phase==Phase::Stopping,"warning":if c.state.phase==Phase::Stopping {Some("cancellation_pending")} else {None}}),
            )
        }
    }
    fn retry(&mut self, requested: &str, c: &mut Controller) -> Response {
        if self.id != requested {
            return missing();
        }
        if !self.retry_available(c) {
            return err(
                "invalid_state",
                "Only the retained failed recording of this CLI operation can be retried.",
            );
        }
        c.retry();
        if self.generation == c.state.generation() {
            return err("invalid_state", "Recording could not be retried.");
        }
        self.id = uuid::Uuid::new_v4().to_string();
        self.generation = c.state.generation();
        self.result = None;
        Response::ok(json!({"operation":self.id,"state":phase(c.state.phase),"delivery":"none"}))
    }
}
struct Completed {
    pending: Pending,
    outcome: Outcome,
}
enum Outcome {
    Reply(Response, bool),
    Pinned(SessionSource, String, bool),
}
fn err(code: &str, message: &str) -> Response {
    Response::error(code, message)
}
fn busy() -> Response {
    err(
        "busy",
        "App is busy; inspect status before any further mutation.",
    )
}
fn missing() -> Response {
    err(
        "not_found",
        "The requested ID does not exist in this instance.",
    )
}
impl Delegate {
    pub(super) fn start_control(&self) -> Result<(), Response> {
        let path = voiceport_control::transport::path_from_env()?;
        self.ivars().control.borrow_mut().server = Some(Server::bind(&path)?);
        Ok(())
    }
    pub(super) fn set_source(&self, c: &mut Controller, id: String) {
        c.source_id = id;
        unsafe {
            NSUserDefaults::standardUserDefaults().setObject_forKey(
                Some(&NSString::from_str(&c.source_id)),
                ns_string!("transcriptionSource"),
            );
        }
        NSUserDefaults::standardUserDefaults().synchronize();
    }
    pub(super) fn poll_control(&self) {
        // First settle an owned operation, before a later GUI session can expose a new transcript.
        let mut control = self.ivars().control.borrow_mut();
        if let Some(operation) = &mut control.operation
            && let Some(c) = self.ivars().controller.borrow().as_ref()
        {
            operation.settle(c);
        }
        if let Some(probe) = &mut control.probe
            && let Some(c) = self.ivars().controller.borrow().as_ref()
        {
            probe.settle(c);
        }
        let completion = control
            .background
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(completed) = completion {
            control.background = None;
            self.ivars().managing_sources.set(false);
            let response = match completed.outcome {
                Outcome::Reply(response, changed) => {
                    if changed {
                        self.with_controller(|c| c.refresh_catalog());
                    }
                    response
                }
                Outcome::Pinned(source, language, sample) => {
                    if completed.pending.expired() {
                        err(
                            "timeout",
                            "Request expired before Controller dispatch; no recording or upload started.",
                        )
                    } else {
                        let mut reply = busy();
                        self.with_controller(|c| {
                            if c.busy() || self.ivars().quitting.get() || self.ivars().pending_hotkey.borrow().is_some() { return; }
                            let plugin = matches!(source, SessionSource::Plugin(_));
                            if sample { c.test_pinned(source, language); }
                            else { c.start_pinned(source, language, false); }
                            let id = uuid::Uuid::new_v4().to_string();
                            control.operation = Some(Operation { id:id.clone(), generation:c.state.generation(), cancelled:false, plugin, sample, result:None });
                            reply = Response::ok(json!({"operation":id,"state":phase(c.state.phase),"delivery":"none"}));
                        });
                        reply
                    }
                }
            };
            completed.pending.respond(response);
        }
        // Process at most four bounded requests per UI tick.
        for _ in 0..4 {
            let Some(pending) = control.server.as_ref().and_then(Server::next) else {
                break;
            };
            if pending.expired() {
                pending.respond(err("timeout", "Request expired before dispatch."));
                continue;
            }
            self.dispatch_control(&mut control, pending);
        }
    }
    fn dispatch_control(&self, control: &mut Control, mut pending: Pending) {
        let action = std::mem::replace(&mut pending.request.action, Action::Status);
        let mut slot = self.ivars().controller.borrow_mut();
        let Some(c) = slot.as_mut() else {
            pending.respond(busy());
            return;
        };
        let locked = self.ivars().quitting.get()
            || self.ivars().permission_rx.borrow().is_some()
            || self.ivars().pending_hotkey.borrow().is_some()
            || self.settings_locked()
            || control.background.is_some();
        let response = match action {
            Action::Status => {
                let (microphone, speech) = crate::audio::Recorder::permission_status();
                Response::ok(
                    json!({"state":phase(c.state.phase),"busy":c.busy()||locked,"pending":c.state.phase==Phase::Stopping,"warning":if c.state.phase==Phase::Stopping {Some("cancellation_pending")} else {None},"discovering":c.discovering(),"source":c.source_id,"language":c.language,"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id(),"bundle_path":NSBundle::mainBundle().bundlePath().to_string(),"socket":voiceport_control::transport::path_from_env().ok(),"permissions":{"microphone":microphone,"speech":speech,"accessibility":Delivery::trusted()},"input":{"automatic":c.automatic,"method":c.input_method(),"failure_stage":c.input_failure_stage(),"start":self.ivars().last_start_trace.borrow().as_ref()},"operation":control.operation.as_ref().filter(|o|o.result.is_none()).map(|o|&o.id),"source_probe":{"operation":control.probe.as_ref().and_then(|probe|probe.active_id()),"busy":c.source_tests.busy(),"cleanup_pending":c.source_tests.cleanup_pending()}}),
                )
            }
            Action::SourcesProbeResult { operation } => match &mut control.probe {
                Some(probe) if probe.id == operation => probe.result(c),
                _ => missing(),
            },
            Action::SourcesProbeCancel { operation } => match &mut control.probe {
                Some(probe) if probe.id == operation => probe.cancel(c),
                _ => missing(),
            },
            Action::Result { operation } => match &control.operation {
                Some(op) if op.id == operation => op.result(c),
                _ => missing(),
            },
            Action::Cancel { operation } => {
                return self.finish_control_operation(control, pending, operation, c, false);
            }
            Action::DictationStop { operation } => {
                return self.finish_control_operation(control, pending, operation, c, true);
            }
            Action::ShowSettings => {
                drop(slot);
                self.show_window();
                self.select_page(true);
                pending.respond(Response::ok(json!({"shown":true,"page":"settings"})));
                return;
            }
            Action::Show => {
                drop(slot);
                self.show_window();
                pending.respond(Response::ok(json!({"shown":true})));
                return;
            }
            Action::Quit => {
                if locked || c.busy() {
                    busy()
                } else {
                    drop(slot);
                    pending.respond(Response::ok(json!({"quitting":true})));
                    // Defer termination outside the control borrow and let the IO worker
                    // publish its response before AppKit's process exit.
                    self.ivars().cli_quit_at.set(Some(
                        std::time::Instant::now() + std::time::Duration::from_millis(300),
                    ));
                    self.ivars().quitting.set(true);
                    return;
                }
            }
            _ if locked || c.busy() || c.discovering() => busy(),
            Action::DictationRetry { operation } => match &mut control.operation {
                Some(op) => op.retry(&operation, c),
                None => missing(),
            },
            Action::SourcesSelect { id } => {
                if !c.catalog.entries.iter().any(|e| e.id == id) {
                    missing()
                } else if c.catalog.snapshot(&id).is_err() {
                    err(
                        "invalid_config",
                        "Source is disabled or unavailable; selection was not changed.",
                    )
                } else {
                    self.set_source(c, id);
                    Response::ok(json!({"source":c.source_id}))
                }
            }
            Action::SourcesProbe {
                source,
                language,
                accept_upload,
            } => {
                match super::control_probes::ProbeOperation::start(
                    c,
                    source,
                    language,
                    accept_upload,
                ) {
                    Ok(mut probe) => {
                        let response = probe.result(c);
                        control.probe = Some(probe);
                        response
                    }
                    Err(response) => response,
                }
            }
            Action::Test {
                source,
                accept_upload,
            } => {
                let snapshot = c.catalog.snapshot(&source);
                match snapshot {
                    Ok(SessionSource::Plugin(p)) if !accept_upload => {
                        let mut response = err(
                            "confirmation_required",
                            "Explicit --accept-upload is required to send the one-second synthetic silence to this destination.",
                        );
                        response.data = json!({"source":source,"endpoint":p.source.config.get("endpoint"),"program":p.manifest.program,"origin":p.manifest.origin,"sample":"one_second_silence","microphone":false,"delivery":"none"});
                        response
                    }
                    Ok(source @ SessionSource::Plugin(_)) => {
                        drop(slot);
                        self.prepare_control(control, pending, source, String::new(), true);
                        return;
                    }
                    Ok(_) => err(
                        "unsupported",
                        "Synthetic source test supports configured plugin instances only.",
                    ),
                    Err(_) => err(
                        "invalid_config",
                        "Source is missing, disabled or unavailable.",
                    ),
                }
            }
            Action::DictationStart { source, language } => {
                if !voiceport_core::language::supported(&language) {
                    err("invalid_request", "Unsupported language.")
                } else if crate::audio::Recorder::permission_status().0 != "authorized" {
                    err(
                        "permission_denied",
                        "Microphone permission must already be granted in the App before CLI recording; no permission prompt was requested.",
                    )
                } else {
                    match c.catalog.snapshot(&source) {
                        Ok(source) => {
                            drop(slot);
                            self.prepare_control(control, pending, source, language, false);
                            return;
                        }
                        Err(_) => err(
                            "invalid_config",
                            "Source is missing, disabled or unavailable.",
                        ),
                    }
                }
            }
            action => {
                let catalog: Vec<_> = c
                    .catalog
                    .entries
                    .iter()
                    .map(|e| json!({"id":e.id,"name":e.title,"available":e.source.is_ok()}))
                    .collect();
                drop(slot);
                self.background_control(control, pending, move || manage(action, catalog));
                return;
            }
        };
        pending.respond(response);
    }
    fn finish_control_operation(
        &self,
        control: &mut Control,
        pending: Pending,
        operation: String,
        c: &mut Controller,
        stop: bool,
    ) {
        let response = if let Some(op) = &mut control.operation {
            if op.id != operation {
                missing()
            } else if op.result.is_some() || op.generation != c.state.generation() {
                err("invalid_state", "Operation is no longer active.")
            } else if stop && c.state.phase != Phase::Recording {
                err(
                    "invalid_state",
                    "Only a recording can be stopped; inspect status or cancel.",
                )
            } else {
                if stop {
                    c.stop();
                } else {
                    op.cancelled = true;
                    c.cancel();
                    op.generation = c.state.generation();
                }
                Response::ok(
                    json!({"operation":op.id,"state":phase(c.state.phase),"busy":c.busy(),"pending":c.state.phase==Phase::Stopping,"warning":if c.state.phase==Phase::Stopping {Some("cancellation_pending")} else {None}}),
                )
            }
        } else {
            missing()
        };
        pending.respond(response);
    }
    fn prepare_control(
        &self,
        control: &mut Control,
        pending: Pending,
        source: SessionSource,
        language: String,
        sample: bool,
    ) {
        self.background_control(control, pending, move || match source.pin() {
            Ok(source) => Outcome::Pinned(source, language, sample),
            Err(_) => Outcome::Reply(
                err(
                    "credential_error",
                    "Cannot pin the source credentials; no upload or recording started.",
                ),
                false,
            ),
        });
    }
    fn background_control(
        &self,
        control: &mut Control,
        pending: Pending,
        action: impl FnOnce() -> Outcome + Send + 'static,
    ) {
        self.ivars().managing_sources.set(true);
        let (tx, rx) = mpsc::sync_channel(1);
        control.background = Some(rx);
        let spawned = std::thread::Builder::new()
            .name("voiceport-control-config".into())
            .spawn(move || {
                let outcome = if pending.expired() {
                    Outcome::Reply(
                        err("timeout", "Request expired before background work."),
                        false,
                    )
                } else {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).unwrap_or_else(
                        |_| {
                            Outcome::Reply(
                                err("internal_error", "Configuration worker failed."),
                                false,
                            )
                        },
                    )
                };
                let _ = tx.send(Completed { pending, outcome });
            });
        if spawned.is_err() {
            control.background = None;
            self.ivars().managing_sources.set(false);
        }
    }
}
fn phase(p: Phase) -> &'static str {
    match p {
        Phase::Idle => "idle",
        Phase::Permissions => "permissions",
        Phase::Recording => "recording",
        Phase::Transcribing => "transcribing",
        Phase::Delivering => "delivering",
        Phase::Stopping => "stopping",
        Phase::Ready => "ready",
        Phase::Failed => "failed",
    }
}
fn manage(action: Action, catalog: Vec<Value>) -> Outcome {
    let mut changed = false;
    let result = (|| -> Result<Value, Response> {
        let path = crate::plugins::default_path()
            .map_err(|_| err("invalid_config", "Source path is invalid."))?;
        let mut store = Store::load(&path)
            .map_err(|_| err("invalid_config", "Source store cannot be loaded."))?;
        let config_error = |_| {
            err(
                "invalid_config",
                "Configuration validation or persistence failed; inspect configuration before retrying. No secrets or service body are included.",
            )
        };
        match action {
            Action::SourcesList => Ok(json!({"sources":store.sources,"catalog":catalog})),
            Action::SourcesShow { id } => store
                .sources
                .iter()
                .find(|s| s.id == id)
                .map(|s| json!(s))
                .ok_or_else(missing),
            Action::PluginsList => Ok(json!({"plugins":store.descriptors()})),
            Action::PluginsShow { id } => store
                .descriptors()
                .into_iter()
                .find(|p| p.id == id)
                .map(|p| json!(p))
                .ok_or_else(missing),
            Action::PluginsImport {
                manifest,
                accept_program,
            } => {
                let p: Manifest = serde_json::from_value(manifest)
                    .map_err(|_| err("invalid_config", "Manifest fields are invalid."))?;
                p.validate_external()
                    .and_then(|_| p.validate_program())
                    .map_err(config_error)?;
                if !accept_program {
                    let mut response = err(
                        "confirmation_required",
                        "Importing a local program requires --accept-program after reviewing its program, argv, origin and user-level execution risks. Import never executes it.",
                    );
                    response.data = json!({"manifest":p});
                    return Err(response);
                }
                store.import(p.clone()).map_err(config_error)?;
                changed = true; // Rename may have committed even if directory fsync fails.
                store.save(&path).map_err(config_error)?;
                Ok(json!({"imported":p,"executed":false}))
            }
            Action::SourcesSave {
                id,
                update,
                plugin,
                name,
                enabled,
                config,
                secrets,
            } => {
                let index = store.sources.iter().position(|s| s.id == id);
                if update && index.is_none() {
                    return Err(missing());
                }
                if !update && index.is_some() {
                    return Err(err("invalid_config", "Source ID already exists."));
                }
                let mut source = if let Some(i) = index {
                    store.sources[i].clone()
                } else {
                    let plugin = store
                        .plugin(
                            plugin
                                .as_deref()
                                .ok_or_else(|| err("invalid_request", "Plugin is required."))?,
                        )
                        .map_err(config_error)?;
                    Source {
                        id,
                        name: String::new(),
                        plugin_id: plugin.id,
                        plugin_version: plugin.version,
                        enabled: false,
                        config: BTreeMap::new(),
                        credential_refs: BTreeMap::new(),
                    }
                };
                if let Some(name) = name {
                    source.name = name;
                }
                if let Some(enabled) = enabled {
                    source.enabled = enabled;
                }
                for (key, value) in config {
                    if value.is_empty() {
                        source.config.remove(&key);
                    } else {
                        source.config.insert(key, value);
                    }
                }
                let descriptor = store.plugin(&source.plugin_id).map_err(config_error)?;
                let mut pending = vec![];
                for (field, value) in secrets {
                    if value.is_empty()
                        || value.len() > 8192
                        || !descriptor
                            .fields
                            .iter()
                            .any(|f| f.id == field && f.kind == FieldKind::Secret)
                    {
                        return Err(err(
                            "invalid_config",
                            "Secret input must name a declared secret field and respect the size bound.",
                        ));
                    }
                    source
                        .credential_refs
                        .insert(field.clone(), "pending-key-reference".into());
                    pending.push((field, value));
                }
                changed = true;
                crate::source_service::save_source(
                    &mut store,
                    &path,
                    index,
                    source.clone(),
                    pending,
                )
                .map_err(config_error)?;
                Ok(json!({"source":store.sources.iter().find(|s|s.id==source.id)}))
            }
            Action::SourcesRemove { id } => {
                if !store.sources.iter().any(|s| s.id == id) {
                    return Err(missing());
                }
                changed = true;
                crate::source_service::remove(&mut store, &path, &id).map_err(config_error)?;
                Ok(json!({"removed":id,"selection_fallback":false}))
            }
            Action::SourcesSecret { id, field, secret } => {
                let i = store
                    .sources
                    .iter()
                    .position(|s| s.id == id)
                    .ok_or_else(missing)?;
                let mut source = store.sources[i].clone();
                let plugin = store.plugin(&source.plugin_id).map_err(config_error)?;
                if !plugin
                    .fields
                    .iter()
                    .any(|f| f.id == field && f.kind == FieldKind::Secret)
                {
                    return Err(err("invalid_config", "Field is not a declared secret."));
                }
                let pending = if let Some(value) = secret {
                    if value.is_empty() || value.len() > 8192 {
                        return Err(err("invalid_request", "Secret size is invalid."));
                    }
                    source
                        .credential_refs
                        .insert(field.clone(), "pending-key-reference".into());
                    vec![(field, value)]
                } else {
                    source.credential_refs.remove(&field);
                    vec![]
                };
                changed = true;
                crate::source_service::save_source(&mut store, &path, Some(i), source, pending)
                    .map_err(config_error)?;
                Ok(json!({"source":id,"secret_updated":true}))
            }
            _ => Err(err("invalid_request", "Unknown management request.")),
        }
    })();
    Outcome::Reply(result.map(Response::ok).unwrap_or_else(|e| e), changed)
}

#[cfg(test)]
mod operation_tests {
    use super::*;
    #[test]
    fn retry_owns_only_the_matching_failed_recording_and_gets_a_new_id() {
        crate::controller::tests::check_control_retry(|c, generation| {
            let mut op = Operation {
                id: "failed-cli".into(),
                generation,
                cancelled: false,
                plugin: true,
                sample: false,
                result: None,
            };
            op.settle(c);
            let failure = op.result(c);
            assert_eq!(failure.error.unwrap().code, "plugin_error");
            assert_eq!(failure.data["retry_available"], true);
            assert!(
                !serde_json::to_string(&failure.data)
                    .unwrap()
                    .contains("text")
            );
            assert_eq!(op.retry("other-id", c).error.unwrap().code, "not_found");
            for reason in ["sample", "cancelled", "superseded"] {
                op.sample = reason == "sample";
                op.cancelled = reason == "cancelled";
                op.generation = generation + u64::from(reason == "superseded");
                assert_eq!(op.result(c).data["retry_available"], false);
                assert_eq!(
                    op.retry("failed-cli", c).error.unwrap().code,
                    "invalid_state"
                );
                assert_eq!(c.state.generation(), generation);
            }
            op.sample = false;
            op.cancelled = false;
            op.generation = generation;
            let response = op.retry("failed-cli", c);
            assert!(response.error.is_none());
            assert_ne!(response.data["operation"], "failed-cli");
            assert_eq!(response.data["delivery"], "none");
            assert!(op.result.is_none());
            assert_eq!(op.generation, c.state.generation());
            assert_eq!(op.retry("failed-cli", c).error.unwrap().code, "not_found");
            let current = op.id.clone();
            assert_eq!(op.retry(&current, c).error.unwrap().code, "invalid_state");
        });
    }
    #[test]
    fn superseded_uncached_operation_never_reports_the_gui_state_or_text() {
        let mut c = Controller::new();
        let mut op = Operation {
            id: "old-cli".into(),
            generation: c.state.generation(),
            cancelled: false,
            plugin: false,
            sample: false,
            result: None,
        };
        c.state.begin().unwrap();
        c.transcript = "PRIVATE_GUI_TEXT".into();
        let response = op.result(&c);
        assert_eq!(response.error.unwrap().code, "not_found");
        assert_eq!(
            op.retry("old-cli", &mut c).error.unwrap().code,
            "invalid_state"
        );
        assert!(!op.retry_available(&c));
    }
}
