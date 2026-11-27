//! Own the CLI operation ID and App admission while the production model-library job runs.
use super::{Response, busy, err, missing};
use crate::{
    controller::Controller,
    model_library::{self, Definition, Installation, Job, MODELS, Operation, Paths, Progress},
};
use serde_json::{Value, json};
use voiceport_control::{ModelMutation, ModelPreview, ModelsRequest};

#[derive(Default)]
pub(super) struct ModelControl(Option<Receipt>);

struct Receipt {
    id: String,
    target: Value,
    state: State,
}

enum State {
    Planned {
        paths: Paths,
        definition: &'static Definition,
        operation: Operation,
    },
    Running {
        job: Job,
        cancel_requested: bool,
    },
    Ready,
    Failed,
    Cancelled,
}

impl ModelControl {
    pub(super) fn dispatch(
        &mut self,
        c: &mut Controller,
        request: ModelsRequest,
        locked: bool,
    ) -> Response {
        match request {
            ModelsRequest::Details { id } => details(c, &id),
            ModelsRequest::Recommend { language, preview } => {
                recommendation(c, language.as_deref(), preview)
            }
            ModelsRequest::Plan { mutation } => {
                if locked || c.busy() {
                    return busy();
                }
                match proposal(c, mutation) {
                    Ok(receipt) => {
                        let result = receipt.response();
                        self.0 = Some(receipt);
                        result
                    }
                    Err(reply) => reply,
                }
            }
            ModelsRequest::Start { operation, confirm } => {
                let Some(receipt) = self.find(&operation) else {
                    return missing();
                };
                if !matches!(receipt.state, State::Planned { .. }) {
                    return receipt.response();
                }
                if !confirm {
                    let mut reply = err(
                        "confirmation_required",
                        "Review the model proposal, then confirm its operation ID.",
                    );
                    reply.data = receipt.response().data;
                    return reply;
                }
                if locked || c.busy() {
                    return busy();
                }
                let State::Planned {
                    paths,
                    definition,
                    operation,
                } = std::mem::replace(&mut receipt.state, State::Failed)
                else {
                    unreachable!()
                };
                if matches!(operation, Operation::Remove { .. }) && c.source_id == definition.id {
                    return err(
                        "invalid_state",
                        "Select another source before removing this model.",
                    );
                }
                if let Ok(job) = Job::start(paths, definition, operation) {
                    receipt.state = State::Running {
                        job,
                        cancel_requested: false,
                    };
                }
                receipt.response()
            }
            ModelsRequest::Status { operation } => self
                .find(&operation)
                .map_or_else(missing, |receipt| receipt.response()),
            ModelsRequest::Cancel { operation } => {
                let Some(receipt) = self.find(&operation) else {
                    return missing();
                };
                receipt.cancel();
                receipt.response()
            }
        }
    }

    pub(super) fn running(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|receipt| matches!(receipt.state, State::Running { .. }))
    }

    pub(super) fn poll(&mut self, quitting: bool) -> bool {
        let Some(receipt) = &mut self.0 else {
            return false;
        };
        if quitting {
            receipt.cancel();
        }
        if let State::Running { job, .. } = &receipt.state
            && let Some(result) = job.poll()
        {
            receipt.state = match result {
                Ok(_) => State::Ready,
                Err(error) if error == model_library::CANCELLED => State::Cancelled,
                Err(_) => State::Failed,
            };
            return true;
        }
        false
    }

    fn find(&mut self, id: &str) -> Option<&mut Receipt> {
        self.0.as_mut().filter(|receipt| receipt.id == id)
    }
}

impl Receipt {
    fn cancel(&mut self) {
        match &mut self.state {
            State::Planned { .. } => self.state = State::Cancelled,
            State::Running {
                job,
                cancel_requested,
            } if !*cancel_requested => {
                job.cancel();
                *cancel_requested = true;
            }
            _ => {}
        }
    }

    fn response(&self) -> Response {
        let (state, progress) = match &self.state {
            State::Planned { .. } => ("planned", Value::Null),
            State::Running {
                job,
                cancel_requested,
            } => (
                if *cancel_requested {
                    "cancelling"
                } else {
                    "working"
                },
                match job.progress() {
                    Progress::Stage(_) => Value::Null,
                    Progress::Transfer {
                        completed, total, ..
                    } => json!({"completed_bytes":completed,"total_bytes":total}),
                },
            ),
            State::Ready => ("ready", Value::Null),
            State::Failed => ("failed", Value::Null),
            State::Cancelled => ("cancelled", Value::Null),
        };
        let data =
            json!({"operation":self.id,"target":self.target,"state":state,"progress":progress});
        if state == "failed" {
            let mut response = err(
                "model_operation_failed",
                "Model operation did not complete successfully. Inspect the model list before trying a new operation.",
            );
            response.data = data;
            response
        } else {
            Response::ok(data)
        }
    }
}

fn proposal(c: &Controller, mutation: ModelMutation) -> Result<Receipt, Response> {
    if std::env::consts::ARCH != "aarch64" {
        return Err(err(
            "unsupported",
            "Local model installation requires Apple Silicon.",
        ));
    }
    let paths = Paths::system().map_err(|_| {
        err(
            "invalid_config",
            "The App's model library path is unavailable.",
        )
    })?;
    let (kind, id, operation) = match mutation {
        ModelMutation::InstallEngine => ("install_engine", None, Operation::Engine),
        ModelMutation::Download { id } => ("download", Some(id), Operation::Download),
        ModelMutation::Update { id } => {
            let definition = definition(&id).ok_or_else(missing)?;
            if !matches!(
                paths.installation(definition),
                Installation::Current | Installation::Legacy
            ) {
                return Err(err(
                    "invalid_state",
                    "An update requires an existing usable VoicePort-owned copy.",
                ));
            }
            ("update", Some(id), Operation::Download)
        }
        ModelMutation::Import { id, directory } => {
            let definition = definition(&id).ok_or_else(missing)?;
            if !directory.is_absolute()
                || !directory.symlink_metadata().is_ok_and(|info| info.is_dir())
            {
                return Err(err(
                    "invalid_arguments",
                    "Supply an absolute path to a local model directory.",
                ));
            }
            if paths.installation(definition) != Installation::Missing
                || c.catalog.snapshot(&id).is_ok()
            {
                return Err(err(
                    "invalid_state",
                    "This model already has an installed or reusable copy.",
                ));
            }
            ("import", Some(id), Operation::Import(directory))
        }
        ModelMutation::Remove { id } => {
            if c.source_id == id {
                return Err(err(
                    "invalid_state",
                    "Select another source before removing this model.",
                ));
            }
            let definition = definition(&id).ok_or_else(missing)?;
            let identity = model_library::removal_identity(&paths, definition)
                .map_err(|_| err("not_found", "No removable VoicePort-owned copy exists."))?;
            ("remove", Some(id), Operation::Remove { identity })
        }
    };
    let definition = match id.as_deref() {
        Some(id) => self::definition(id).ok_or_else(missing)?,
        None => MODELS.first().expect("fixed library is nonempty"),
    };
    if kind == "download" && c.catalog.snapshot(definition.id).is_ok() {
        return Err(err(
            "invalid_state",
            "This model is already available; use update for an installed VoicePort copy.",
        ));
    }
    let target = json!({
        "kind":kind,"model":id,"may_download":kind != "remove",
        "model_bytes":id.as_ref().map(|_| definition.bytes()),"engine_bytes":model_library::runtime_bytes(),
    });
    Ok(Receipt {
        id: uuid::Uuid::new_v4().to_string(),
        target,
        state: State::Planned {
            paths,
            definition,
            operation,
        },
    })
}

fn definition(id: &str) -> Option<&'static Definition> {
    MODELS.iter().find(|model| model.id == id)
}

fn recommendation(c: &Controller, language: Option<&str>, preview: ModelPreview) -> Response {
    let language = language.unwrap_or(&c.language);
    if !voiceport_core::language::supported(language) {
        return err(
            "invalid_arguments",
            "Supply a supported speech language code.",
        );
    }
    let model = model_library::recommend(language, preview).map(|model| {
        json!({
            "id":model.id,"name":model.name,"languages":model.languages,"bytes":model.bytes(),
            "preview":model.preview(),"available":c.catalog.snapshot(model.id).is_ok(),
            "selected":c.source_id == model.id,
        })
    });
    Response::ok(json!({
        "language":if language.is_empty() { "auto" } else { language },
        "preview":preview,"discovering":c.discovering(),
        "reason":if model.is_some() { "smallest_compatible_download" } else { "no_compatible_model" },
        "recommendation":model,"accuracy":"unmeasured","performance":"unmeasured",
    }))
}

fn details(c: &Controller, id: &str) -> Response {
    let Some(model) = definition(id) else {
        return missing();
    };
    let installation = Paths::system()
        .ok()
        .map(|paths| match paths.installation(model) {
            Installation::Missing => "missing",
            Installation::Current => "current",
            Installation::Legacy => "legacy",
            Installation::Incompatible => "incompatible",
        });
    Response::ok(json!({
        "id":model.id,"name":model.name,"languages":model.languages,"bytes":model.bytes(),
        "available":c.catalog.snapshot(model.id).is_ok(),"selected":c.source_id == model.id,
        "voiceport_installation":installation,
    }))
}

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::*;
    use crate::local_models::{CHATGPT, Catalog, PARAFORMER, WHISPER};
    use std::fs;

    #[test]
    fn recommendation_is_read_only_with_missing_and_available_models() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let runtime = root.join("runtime");
        let directory = root.join("paraformer-bilingual-zh-en");
        let mut c = Controller::new();
        c.language = "zh".into();
        c.automatic = false;
        let mut control = ModelControl::default();
        let mut query = |c: &mut Controller, language: Option<&str>, preview| {
            let reply = control.dispatch(
                c,
                ModelsRequest::Recommend {
                    language: language.map(str::to_owned),
                    preview,
                },
                true,
            );
            assert!(reply.error.is_none());
            reply.data
        };

        let default = query(&mut c, None, ModelPreview::Any);
        assert_eq!(default["language"], "zh");
        assert_eq!(default["recommendation"]["id"], WHISPER);
        assert_eq!(default["recommendation"]["preview"], "after_stop");
        assert_eq!(default["accuracy"], "unmeasured");
        assert_eq!(default["performance"], "unmeasured");
        let missing = query(&mut c, Some("en"), ModelPreview::Live);
        assert_eq!(missing["language"], "en");
        assert_eq!(missing["reason"], "smallest_compatible_download");
        assert_eq!(missing["recommendation"]["id"], PARAFORMER);
        assert_eq!(missing["recommendation"]["available"], false);
        assert_eq!(missing["recommendation"]["selected"], false);
        let unsupported = query(&mut c, Some("yue"), ModelPreview::Live);
        assert_eq!(unsupported["reason"], "no_compatible_model");
        assert!(unsupported["recommendation"].is_null());
        assert_eq!(fs::read_dir(root).unwrap().count(), 0);

        fs::create_dir(&runtime).unwrap();
        fs::write(runtime.join("package.json"), br#"{"version":"2.0.1"}"#).unwrap();
        for filename in ["libsherpa-onnx-c-api.dylib", "libonnxruntime.dylib"] {
            fs::write(
                runtime.join(filename),
                [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1],
            )
            .unwrap();
        }
        fs::create_dir(&directory).unwrap();
        let d = definition(PARAFORMER).unwrap();
        for asset in d.assets {
            fs::write(directory.join(asset.name), b"synthetic fixture").unwrap();
        }
        c.catalog = Catalog::discover(root, &runtime, CHATGPT, std::env::consts::ARCH);
        let before = c.catalog.clone();
        let available = query(&mut c, None, ModelPreview::Live);
        assert_eq!(available["recommendation"]["id"], PARAFORMER);
        assert_eq!(available["recommendation"]["bytes"], d.bytes());
        assert_eq!(available["recommendation"]["available"], true);
        assert_eq!(available["recommendation"]["selected"], false);
        assert_eq!(c.source_id, CHATGPT);
        c.source_id = PARAFORMER.into();
        assert_eq!(
            query(&mut c, Some("en"), ModelPreview::Live)["recommendation"]["selected"],
            true
        );
        assert_eq!(c.source_id, PARAFORMER);
        assert_eq!(c.language, "zh");
        assert!(!c.automatic);
        assert!(!c.busy() && !c.discovering());
        assert_eq!(c.catalog, before);
        assert!(control.0.is_none());
        assert_eq!(fs::read_dir(root).unwrap().count(), 2);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), d.assets.len());
        for asset in d.assets {
            assert_eq!(
                fs::read(directory.join(asset.name)).unwrap(),
                b"synthetic fixture"
            );
        }
    }
}
