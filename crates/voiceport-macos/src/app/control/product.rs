use super::{Control, Pending, Response, busy, err, missing};
use crate::{
    app::{
        Delegate, microphone, model_manager, source_manager::destination::Destination,
        status_overlay, workbench,
    },
    controller::{Controller, VisibleTranscript},
    dictation_dictionary::{Dictionary, format_rules, parse_rules},
    dictation_workflows::Preset,
    local_models::SessionSource,
    model_library::MODELS,
    shortcut::ShortcutMode,
};
use objc2::DefinedClass;
use serde_json::{Value, json};
use voiceport_control::{
    DictionaryRequest, MicrophoneRequest, Page, PreferencesRequest, RecordingMode, ScenesRequest,
    TranscriptionState,
};

pub(super) enum Window {
    Page(Page),
    ApplyScene(String),
}

impl Delegate {
    pub(super) fn show_control_window(&self, window: Window, pending: Pending) {
        if pending.expired() {
            return pending.respond(err("timeout", "Request expired before opening the window."));
        }
        let page = match window {
            Window::ApplyScene(name) => {
                pending.respond(user_action(Page::Scenes, "confirm_scene_destination"));
                self.confirm_named_scene(&name);
                return;
            }
            Window::Page(page) => page,
        };
        let response = match page {
            Page::Result | Page::Settings => {
                self.show_window();
                self.select_page(matches!(page, Page::Settings));
                shown(page)
            }
            Page::Shortcut => {
                pending.respond(user_action(page, "record_and_save_shortcut"));
                self.open_shortcut_recorder();
                return;
            }
            Page::FirstRun | Page::Permissions => {
                self.show_first_run_checks();
                user_action(page, "use_first_run_permission_or_input_controls")
            }
            Page::Models => {
                let existing = self.ivars().model_manager.borrow().clone();
                match existing {
                    Some(manager) => manager.bring_forward(),
                    None => {
                        if model_manager::show(self).is_err() {
                            return pending.respond(err(
                                "invalid_state",
                                "The model manager could not be opened.",
                            ));
                        }
                    }
                }
                user_action(page, "choose_native_model_operation")
            }
            Page::Sources => {
                self.open_source_manager();
                if self.ivars().source_manager.borrow().is_none() {
                    err("invalid_state", "The source manager could not be opened.")
                } else {
                    shown(page)
                }
            }
            Page::SourceTests => {
                crate::app::source_probes::show(self);
                user_action(page, "choose_and_approve_source_test")
            }
            Page::Microphone => {
                self.show_workbench_page(workbench::Page::Device);
                shown(page)
            }
            Page::QuickPanel => {
                self.show_quick_panel();
                shown(page)
            }
            Page::File => {
                self.show_file_transcription();
                user_action(page, "choose_audio_file")
            }
            Page::Dictionary | Page::Scenes => {
                pending.respond(user_action(page, "complete_native_dialog"));
                match page {
                    Page::Dictionary => self.show_dictionary(),
                    Page::Scenes => self.show_presets(),
                    _ => unreachable!(),
                }
                return;
            }
        };
        pending.respond(response);
    }

    pub(super) fn control_preferences(
        &self,
        c: &mut Controller,
        request: PreferencesRequest,
    ) -> Response {
        match request {
            PreferencesRequest::Get => {}
            PreferencesRequest::Language { language } => {
                if !voiceport_core::language::supported(&language) {
                    return err("invalid_request", "Unsupported speech language code.");
                }
                let selected = c.catalog.snapshot(&c.source_id);
                if !selected
                    .as_ref()
                    .map_or(true, |source| source_allows(source, &language))
                {
                    return err(
                        "invalid_config",
                        "The selected source does not support that language.",
                    );
                }
                self.set_language(c, language);
            }
            PreferencesRequest::AutomaticInput { enabled } => self.set_automatic_input(c, enabled),
            PreferencesRequest::StatusOverlay { enabled } => status_overlay::set_enabled(enabled),
            PreferencesRequest::RecordingMode { mode } => self.set_shortcut_mode(match mode {
                RecordingMode::Toggle => ShortcutMode::Toggle,
                RecordingMode::Hold => ShortcutMode::Hold,
            }),
        }
        Response::ok(json!({
            "source": c.source_id, "language": language_code(&c.language),
            "languages": voiceport_core::language::LANGUAGES.iter().map(|(code, label)| json!({"code": language_code(code), "label":label})).collect::<Vec<_>>(),
            "automatic_input": c.automatic, "status_overlay": status_overlay::enabled(),
            "recording_mode": self.ivars().shortcut_mode.get().preference(),
            "shortcut": {"label": self.ivars().shortcut_binding.get().label(), "registration": self.hotkey_registration()},
        }))
    }

    pub(super) fn begin_microphone_inventory(
        &self,
        control: &mut Control,
        c: &Controller,
        pending: Pending,
        select: Option<String>,
    ) {
        if control.microphone_inventory.is_some() || c.audio_input_busy() {
            pending.respond(busy());
            return;
        }
        match c.read_microphones() {
            Ok(query) => {
                control.microphone_inventory = Some(MicrophoneInventory {
                    pending,
                    query,
                    select,
                    generation: c.state.generation(),
                    preferred: c.microphone_uid.clone(),
                })
            }
            Err(_) => pending.respond(err(
                "device_error",
                "Could not enumerate microphone devices.",
            )),
        }
    }

    pub(super) fn poll_microphone_inventory(&self, control: &mut Control) {
        let Some(inventory) = &control.microphone_inventory else {
            return;
        };
        let expired = inventory.pending.expired();
        let result = if expired {
            None
        } else {
            inventory.query.poll()
        };
        if !expired && result.is_none() {
            return;
        }
        let inventory = control.microphone_inventory.take().unwrap();
        let reply = if expired {
            err(
                "timeout",
                "Request expired while reading microphone devices.",
            )
        } else if let Some(Ok(devices)) = result {
            let mut reply = busy();
            self.with_controller(|c| {
                if let Some(uid) = inventory.select {
                    if self.ivars().quitting.get() || c.busy() || self.settings_locked()
                        || control.preparing() || self.ivars().permission_rx.borrow().is_some()
                        || self.ivars().pending_hotkey.borrow().is_some()
                        || !c.state.accepts(inventory.generation) || c.microphone_uid != inventory.preferred
                    {
                        return;
                    }
                    if !devices.iter().any(|device| device.uid == uid) {
                        reply = missing();
                        return;
                    }
                    microphone::set_preference(c, Some(uid));
                    reply = microphone_status(c, control.microphone_operation.as_deref());
                } else {
                    reply = Response::ok(json!({
                        "devices": devices.iter().map(|d| json!({"uid":d.uid,"name":d.name,"label":d.label()})).collect::<Vec<_>>(),
                        "preferred_uid": c.microphone_uid,
                    }));
                }
            });
            reply
        } else {
            err("device_error", "Could not enumerate microphone devices.")
        };
        inventory.pending.respond(reply);
    }

    pub(super) fn control_microphone(
        &self,
        control: &mut Control,
        c: &mut Controller,
        request: MicrophoneRequest,
        locked: bool,
    ) -> Response {
        use MicrophoneRequest::*;
        match request {
            Status => microphone_status(c, control.microphone_operation.as_deref()),
            TestStop { operation } => {
                if control.microphone_operation.as_deref() != Some(&operation) {
                    return missing();
                }
                if c.microphone_test_id() == Some(operation.as_str()) {
                    c.stop_microphone_test();
                }
                microphone_status(c, Some(&operation))
            }
            _ if locked
                || c.busy()
                || c.audio_input_busy()
                || control.microphone_inventory.is_some() =>
            {
                busy()
            }
            Select { uid: None } => {
                microphone::set_preference(c, None);
                microphone_status(c, control.microphone_operation.as_deref())
            }
            List | Select { uid: Some(_) } => {
                unreachable!("inventory requests are dispatched separately")
            }
            TestStart { confirm } => {
                if !confirm {
                    return err(
                        "confirmation_required",
                        "Confirm the local microphone level test.",
                    );
                }
                if c.capture_permission_status().0 != "authorized" {
                    return err(
                        "permission_denied",
                        "Allow VoicePort microphone access in the first-run panel before testing.",
                    );
                }
                c.start_microphone_test();
                let Some(operation) = c.microphone_test_id() else {
                    return err("device_error", "Could not start the microphone level test.");
                };
                control.microphone_operation = Some(operation.into());
                microphone_status(c, Some(operation))
            }
        }
    }

    pub(super) fn control_dictionary(
        &self,
        control: &mut Control,
        c: &mut Controller,
        request: DictionaryRequest,
    ) -> Response {
        match request {
            DictionaryRequest::Show => {}
            DictionaryRequest::Set { rules, enabled } => {
                if rules.len() > 24_000 {
                    return err("invalid_config", "Dictionary exceeds the CLI input limit.");
                }
                let Ok(rules) = parse_rules(&rules) else {
                    return err(
                        "invalid_config",
                        "Invalid dictionary rules. Use one original → replacement pair per line.",
                    );
                };
                let mut next = c.workflows.clone();
                next.rules = rules;
                next.dictionary_enabled = enabled;
                if self.commit_workflows(c, next).is_err() {
                    return err("invalid_config", "Could not save dictionary settings.");
                }
            }
            DictionaryRequest::Preview { text } => {
                if text.len() > 24_000 {
                    return err(
                        "invalid_request",
                        "Dictionary preview exceeds the input limit.",
                    );
                }
                let Ok(dictionary) = Dictionary::compile(&c.workflows.rules) else {
                    return err("invalid_config", "Invalid saved dictionary rules.");
                };
                let result = dictionary.apply(&text);
                if result.len() > 32_000 {
                    return err(
                        "output_too_large",
                        "Dictionary preview exceeds the output limit.",
                    );
                }
                return Response::ok(json!({"text":result, "changed":result != text}));
            }
            request => return self.control_dictionary_exchange(control, c, request),
        }
        let response = Response::ok(json!({
            "enabled":c.workflows.dictionary_enabled,
            "rules":format_rules(&c.workflows.rules),
            "stage":"post_transcription",
        }));
        if serde_json::to_vec(&response)
            .is_ok_and(|data| data.len() <= voiceport_control::MAX_FRAME)
        {
            response
        } else {
            err(
                "frame_too_large",
                "Read the saved rules with dictionary show --offset 0, then follow next_offset and revision.",
            )
        }
    }

    pub(super) fn control_scenes(&self, c: &mut Controller, request: ScenesRequest) -> Response {
        let mut next = c.workflows.clone();
        match request {
            ScenesRequest::List => return Response::ok(json!({"scenes":next.presets})),
            ScenesRequest::Add {
                name,
                source,
                language,
                dictionary_enabled,
            } => {
                let preset = Preset {
                    name,
                    source_id: source,
                    language,
                    dictionary_enabled,
                };
                if preset.source(&c.catalog).is_err() || next.add_preset(preset).is_err() {
                    return err(
                        "invalid_config",
                        "Scene has an invalid name, unavailable source, unsupported language, duplicate name or exceeds the scene limit.",
                    );
                }
            }
            ScenesRequest::Remove { name, confirm } => {
                let Some(index) = next.presets.iter().position(|preset| preset.name == name) else {
                    return missing();
                };
                if !confirm {
                    return err(
                        "confirmation_required",
                        "Confirm removal of the named scene with --confirm.",
                    );
                }
                next.presets.remove(index);
            }
            ScenesRequest::Apply { .. } => unreachable!("handled as a named native confirmation"),
        }
        if self.commit_workflows(c, next).is_err() {
            return err("invalid_config", "Could not save scenes.");
        }
        Response::ok(json!({"scenes":c.workflows.presets}))
    }
}

pub(super) fn capabilities() -> Response {
    Response::ok(json!({
        "control_schema":voiceport_control::VERSION, "version":env!("CARGO_PKG_VERSION"),
        "user_scope":"effective_user", "platform":"macos",
        "app_update":["check","status"],
        "dictionary_exchange":["import","import-preview","import-apply","import-cancel","export"],
        "groups":["app","status","sources","plugins","dictation","preferences","microphone","models","dictionary","scenes","history","file"],
    }))
}

fn shown(page: Page) -> Response {
    Response::ok(json!({"shown":true,"page":page}))
}

fn user_action(page: Page, next: &str) -> Response {
    Response::ok(json!({"page":page,"state":"user_action_required","next":next}))
}

fn language_code(code: &str) -> &str {
    if code.is_empty() { "auto" } else { code }
}

fn source_allows(source: &SessionSource, language: &str) -> bool {
    match source {
        SessionSource::Local(model) => model.kind.supports_language(language),
        _ => true,
    }
}

pub(super) fn destination(c: &Controller, source_id: &str) -> Response {
    if !c.catalog.entries.iter().any(|entry| entry.id == source_id) {
        return missing();
    }
    let Ok(source) = c.catalog.snapshot(source_id) else {
        return err("invalid_config", "The source is disabled or unavailable.");
    };
    let descriptor = source.descriptor();
    let configuration = match &source {
        SessionSource::Plugin(snapshot) => Some(&snapshot.source),
        _ => None,
    };
    let destination = Destination::configured(Some(&descriptor), configuration);
    let (kind, origin) = match &destination {
        Destination::OnDevice => ("on_device", None),
        Destination::ChatGpt => ("chatgpt", None),
        Destination::LoopbackHttp(origin) => ("loopback_http", Some(origin)),
        Destination::RemoteHttp(origin) => ("remote_http", Some(origin)),
        Destination::HttpNotConfigured => ("http_not_configured", None),
        Destination::ExternalProgram => ("external_program", None),
        Destination::Unknown => ("unknown", None),
    };
    Response::ok(json!({
        "source":source_id, "destination":{"kind":kind,"origin":origin},
        "languages":voiceport_core::language::LANGUAGES.iter().filter(|(code, _)| source_allows(&source, code)).map(|(code, _)| language_code(code)).collect::<Vec<_>>(),
        "external_audio_possible": !matches!(destination, Destination::OnDevice),
    }))
}

pub(super) fn models(c: &Controller) -> Response {
    let entries: Vec<Value> = MODELS.iter().map(|model| {
        json!({"id":model.id, "name":model.name, "languages":model.languages, "bytes":model.bytes(),
            "available":c.catalog.snapshot(model.id).is_ok(),"selected":c.source_id == model.id})
    }).collect();
    Response::ok(json!({"models":entries,"discovering":c.discovering()}))
}

pub(super) fn gui_result(c: &Controller, generation: u64, original: bool) -> Response {
    let Some(receipt) = c
        .gui_dictation_receipt()
        .filter(|r| r.generation == generation && c.state.generation() == generation)
    else {
        return missing();
    };
    let text = match receipt.transcription {
        TranscriptionState::Empty => "",
        TranscriptionState::Completed if matches!(c.visible_transcript, Some(VisibleTranscript::Final(current)) if current == generation) => {
            if original {
                c.raw_transcript.as_deref().unwrap_or(&c.transcript)
            } else {
                &c.transcript
            }
        }
        _ => {
            return err(
                "invalid_state",
                "The requested GUI generation has no retained final result.",
            );
        }
    };
    if text.len() > 32_000 {
        return err(
            "output_too_large",
            "Use the native result panel to read this long transcript.",
        );
    }
    Response::ok(
        json!({"generation":generation,"original":original,"text":text,"empty":text.is_empty(),"receipt":receipt}),
    )
}

pub(super) struct MicrophoneInventory {
    pending: Pending,
    query: crate::audio::worker::Inventory,
    select: Option<String>,
    generation: u64,
    preferred: Option<String>,
}

fn microphone_status(c: &Controller, operation: Option<&str>) -> Response {
    let running = c.microphone_test_measuring();
    let owned = c.microphone_test_running() && c.microphone_test_id() == operation;
    Response::ok(json!({
        "preferred_uid":c.microphone_uid, "active_route":c.microphone_route(),
        "permission":c.capture_permission_status().0,
        "test":{
            "operation":operation, "running":owned && running, "pending":owned && !running, "device_in_test":running,
            "detail":c.microphone_test_detail(operation),
            "recorded":false,"uploaded":false
        },
    }))
}
