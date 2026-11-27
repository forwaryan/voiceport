use super::{Control, Response, err, missing};
use crate::{
    app::Delegate, controller::Controller, dictation_dictionary::Rule,
    dictionary_exchange as exchange,
};
use serde_json::json;
use voiceport_control::{DictionaryImportMode, DictionaryRequest};

const STAGE: &str = "post_transcription";
const PREVIEW_PAGE: usize = 16;

pub(super) struct ProposedImport {
    operation: String,
    current: Vec<Rule>,
    plan: exchange::ImportPlan,
    mode: DictionaryImportMode,
}

impl ProposedImport {
    fn preview(&self, offset: usize) -> Response {
        let total = self.plan.rules.len();
        if offset > total {
            return err("invalid_arguments", "Preview offset is out of range.");
        }
        let end = (offset + PREVIEW_PAGE).min(total);
        Response::ok(json!({
            "stage": STAGE,
            "operation": self.operation,
            "state": "planned",
            "mode": self.mode,
            "existing": self.current.len(),
            "imported": self.plan.imported,
            "skipped_duplicates": self.plan.skipped_duplicates,
            "result_count": total,
            "offset": offset,
            "next_offset": (end < total).then_some(end),
            "rules": &self.plan.rules[offset..end],
        }))
    }
}

impl Delegate {
    pub(super) fn control_dictionary_exchange(
        &self,
        control: &mut Control,
        c: &mut Controller,
        request: DictionaryRequest,
    ) -> Response {
        use DictionaryRequest::*;
        let pending = &mut control.dictionary_import;
        match request {
            Import { path, mode } => {
                let plan = exchange::read(&path)
                    .and_then(|rules| exchange::ImportPlan::new(&c.workflows.rules, rules, mode));
                let plan = match plan {
                    Ok(plan) => plan,
                    Err(message) => return err("invalid_config", &message),
                };
                let proposed = ProposedImport {
                    operation: uuid::Uuid::new_v4().to_string(),
                    current: c.workflows.rules.clone(),
                    plan,
                    mode,
                };
                let response = proposed.preview(0);
                *pending = Some(proposed);
                response
            }
            ImportPreview { operation, offset } => pending
                .as_ref()
                .filter(|plan| plan.operation == operation)
                .map_or_else(missing, |plan| plan.preview(offset)),
            ImportApply { operation, confirm } => {
                if !confirm {
                    return err(
                        "confirmation_required",
                        "Review the import and pass --confirm.",
                    );
                }
                let Some(proposed) = pending.as_ref().filter(|plan| plan.operation == operation)
                else {
                    return missing();
                };
                if proposed.current != c.workflows.rules {
                    *pending = None;
                    return err(
                        "invalid_state",
                        "The dictionary changed. Plan the import again.",
                    );
                }
                let changed = proposed.plan.rules != c.workflows.rules;
                let count = proposed.plan.rules.len();
                if changed {
                    let mut next = c.workflows.clone();
                    next.rules = proposed.plan.rules.clone();
                    if self.commit_workflows(c, next).is_err() {
                        return err("invalid_config", "Could not save dictionary settings.");
                    }
                }
                *pending = None;
                Response::ok(json!({
                    "stage": STAGE, "state": "applied", "changed": changed,
                    "rule_count": count, "enabled": c.workflows.dictionary_enabled,
                }))
            }
            ImportCancel { operation } => {
                if pending
                    .as_ref()
                    .is_none_or(|plan| plan.operation != operation)
                {
                    return missing();
                }
                *pending = None;
                Response::ok(json!({"stage": STAGE, "state": "cancelled"}))
            }
            Export { path } => match exchange::write(&path, &c.workflows.rules) {
                Ok(()) => Response::ok(json!({
                    "stage": STAGE, "format": "csv", "rule_count": c.workflows.rules.len(),
                })),
                Err(message) => err("invalid_config", &message),
            },
            Show | Set { .. } | Preview { .. } => {
                err("invalid_request", "Invalid exchange request.")
            }
        }
    }
}
