//! CLI ownership around the App's file transcription service.
use super::{Response, err, missing};
use crate::{
    controller::Controller,
    file_transcription::{self as files, ExportFormat, Input, Plan, Status},
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use voiceport_control::{FileFormat, FileRequest};

pub(super) struct Prepared {
    pub input: Input,
    pub plan: Plan,
}

struct Proposal {
    id: String,
    target: files::Target,
    state: ProposalState,
}

enum ProposalState {
    Planned(Box<Prepared>),
    Cancelled,
    Rejected(Response),
}

#[derive(Default)]
pub(super) struct FileControl {
    proposal: Option<Proposal>,
    started: VecDeque<(String, String)>,
}

impl FileControl {
    pub(super) fn clear_proposal(&mut self) {
        self.proposal = None;
    }

    pub(super) fn prepare(&mut self, prepared: Box<Prepared>) -> Response {
        let proposal = Proposal {
            id: uuid::Uuid::new_v4().to_string(),
            target: prepared.plan.target.clone(),
            state: ProposalState::Planned(prepared),
        };
        let reply = proposal.status();
        self.proposal = Some(proposal);
        reply
    }

    pub(super) fn dispatch(
        &mut self,
        c: &mut Controller,
        request: FileRequest,
        locked: bool,
    ) -> Response {
        match request {
            FileRequest::Sources { language } => {
                if !voiceport_core::language::supported(&language) {
                    return err("invalid_request", "Unsupported speech language code.");
                }
                Response::ok(json!({"sources":files::sources(&c.catalog, &language)}))
            }
            FileRequest::Status { operation } => self.status(c, &operation),
            FileRequest::Result { operation } => {
                let Some(id) = self.job_id(&operation) else {
                    return self.status(c, &operation);
                };
                match c.file_jobs.result(id) {
                    Ok((status, text)) => {
                        let mut data = serialize_status(status);
                        if let Some(text) = text {
                            data["text"] = json!(text);
                            data["empty"] = json!(text.is_empty());
                        }
                        if matches!(status.state, files::State::Failed | files::State::Cancelled)
                            && let Some(error) = &status.error
                        {
                            let mut reply = error_response(error);
                            reply.data = data;
                            return reply;
                        }
                        Response::ok(data)
                    }
                    Err(e) => error_response(&e),
                }
            }
            FileRequest::Cancel { operation } => {
                if let Some(id) = self.job_id(&operation) {
                    return match c.file_jobs.cancel(id) {
                        Ok(status) => Response::ok(serialize_status(&status)),
                        Err(e) => error_response(&e),
                    };
                }
                let Some(proposal) = self
                    .proposal
                    .as_mut()
                    .filter(|proposal| proposal.id == operation)
                else {
                    return missing();
                };
                if matches!(proposal.state, ProposalState::Planned(_)) {
                    proposal.state = ProposalState::Cancelled;
                }
                proposal.status()
            }
            FileRequest::Start { operation, confirm } => self.start(c, &operation, confirm, locked),
            FileRequest::Export {
                operation,
                path,
                format,
            } => {
                let Some(id) = self.job_id(&operation) else {
                    return missing();
                };
                if locked || c.busy() {
                    return super::busy();
                }
                let format = match format {
                    FileFormat::Text => ExportFormat::Text,
                    FileFormat::Markdown => ExportFormat::Markdown,
                };
                match c.file_jobs.export(id, &path, format) {
                    Ok(()) => Response::ok(
                        json!({"operation":id,"exported":true,"format":format.extension(),"delivery":"none"}),
                    ),
                    Err(e) => error_response(&e),
                }
            }
            FileRequest::Plan { .. } => {
                unreachable!("The bounded background worker opens the file")
            }
        }
    }

    fn status(&self, c: &Controller, operation: &str) -> Response {
        if let Some(id) = self.job_id(operation) {
            return match c.file_jobs.status(id) {
                Ok(status) => Response::ok(serialize_status(status)),
                Err(e) => error_response(&e),
            };
        }
        self.proposal
            .as_ref()
            .filter(|proposal| proposal.id == operation)
            .map_or_else(missing, Proposal::status)
    }

    fn start(
        &mut self,
        c: &mut Controller,
        operation: &str,
        confirm: bool,
        locked: bool,
    ) -> Response {
        if self.job_id(operation).is_some() {
            return self.status(c, operation);
        }
        let Some(proposal) = self
            .proposal
            .as_mut()
            .filter(|proposal| proposal.id == operation)
        else {
            return missing();
        };
        if !matches!(proposal.state, ProposalState::Planned(_)) {
            return proposal.status();
        }
        if !confirm {
            let mut reply = err(
                "confirmation_required",
                "Review the prepared file and destination, then use --confirm.",
            );
            reply.data = proposal.status().data;
            return reply;
        }
        if locked || c.busy() {
            return super::busy();
        }
        let ProposalState::Planned(prepared) =
            std::mem::replace(&mut proposal.state, ProposalState::Cancelled)
        else {
            unreachable!()
        };
        match c.start_prepared_file(prepared.input, prepared.plan, true) {
            Ok(status) => {
                // The Controller retains four completed jobs plus one active job.
                self.started
                    .push_front((operation.into(), status.operation.clone()));
                self.started.truncate(5);
                self.proposal = None;
                Response::ok(serialize_status(&status))
            }
            Err(error) => {
                let mut reply = error_response(&error);
                reply.data = proposal.data("failed");
                proposal.state = ProposalState::Rejected(reply.clone());
                reply
            }
        }
    }

    fn job_id(&self, operation: &str) -> Option<&str> {
        self.started
            .iter()
            .find(|(proposal, job)| proposal == operation || job == operation)
            .map(|(_, job)| job.as_str())
    }
}

impl Proposal {
    fn data(&self, state: &str) -> Value {
        json!({"operation":self.id,"state":state,"target":self.target,"delivery":"none"})
    }

    fn status(&self) -> Response {
        match &self.state {
            ProposalState::Planned(_) => Response::ok(self.data("planned")),
            ProposalState::Cancelled => Response::ok(self.data("cancelled")),
            ProposalState::Rejected(response) => response.clone(),
        }
    }
}

fn serialize_status(status: &Status) -> Value {
    serde_json::to_value(status).expect("file status is serializable")
}

pub(super) fn error_response(error: &files::Error) -> Response {
    let mut reply = err(error.code, error.message);
    if let Some(target) = &error.target {
        reply.data = json!({"target":target});
    }
    reply
}
