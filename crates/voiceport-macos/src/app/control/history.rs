use super::{Controller, Delegate, Response, busy, err, missing};
use serde_json::json;
use voiceport_control::HistoryRequest;

impl Delegate {
    pub(super) fn set_control_history(&self, enabled: bool, confirm: bool) -> Response {
        if !enabled && !confirm {
            return err(
                "confirmation_required",
                "Confirm disabling and clearing recent dictations with --confirm.",
            );
        }
        if self.set_recent_dictations_enabled(enabled) {
            Response::ok(json!({"enabled":enabled}))
        } else {
            busy()
        }
    }
}

pub(super) fn dispatch(c: &mut Controller, request: HistoryRequest) -> Response {
    let enabled = c.recent_dictations_enabled();
    match request {
        HistoryRequest::List { limit } if (1..=100).contains(&limit) => Response::ok(json!({
            "enabled":enabled, "entries":c.list_recent_dictations(usize::from(limit)),
        })),
        HistoryRequest::List { .. } => err(
            "invalid_request",
            "History limit must be between 1 and 100.",
        ),
        HistoryRequest::Show { id } => {
            if let Some(text) = c.read_recent_dictation(&id) {
                Response::ok(json!({"id":id,"text":text}))
            } else if c.recent_dictation_metadata(&id).is_some() {
                err(
                    "text_unavailable",
                    "Only metadata is retained for this entry.",
                )
            } else {
                missing()
            }
        }
        HistoryRequest::Clear { confirm: true } => {
            Response::ok(json!({"enabled":enabled,"removed":c.clear_recent_dictations()}))
        }
        HistoryRequest::Clear { confirm: false } => err(
            "confirmation_required",
            "Confirm clearing recent dictations with --confirm.",
        ),
        HistoryRequest::Settings { enabled: None, .. } => Response::ok(json!({"enabled":enabled})),
        HistoryRequest::Settings {
            enabled: Some(_), ..
        } => {
            unreachable!("Delegate owns preference commits")
        }
    }
}
