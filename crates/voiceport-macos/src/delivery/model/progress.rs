//! Content-free evidence shared by every delivery transport. Counts are write
//! operations (including AX preview corrections/restoration), never characters.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryState {
    #[default]
    NotSent,
    Waiting,
    AwaitingConfirmation,
    PartiallySent,
    Confirmed,
    Submitted,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryReceipt {
    pub state: DeliveryState,
    pub submitted_operations: usize,
    pub confirmed_operations: usize,
    pub stopped: bool,
}

impl DeliveryReceipt {
    pub(super) fn waiting(&mut self) {
        self.state = if self.submitted_operations == 0 {
            DeliveryState::Waiting
        } else {
            DeliveryState::PartiallySent
        };
    }

    // Call before crossing a potentially mutating boundary. An error from that
    // boundary cannot prove that no side effect occurred.
    pub(super) fn attempting(&mut self) {
        self.state = DeliveryState::AwaitingConfirmation;
    }

    pub(super) fn submitted(&mut self, acknowledged: bool) {
        self.submitted_operations += 1;
        self.state = if acknowledged {
            DeliveryState::AwaitingConfirmation
        } else {
            DeliveryState::Submitted
        };
    }

    pub(super) fn confirmed(&mut self) {
        self.confirmed_operations += 1;
        self.state = DeliveryState::PartiallySent;
    }

    pub(super) fn finish(&mut self) {
        if self.submitted_operations > 0
            && self.confirmed_operations == self.submitted_operations
            && self.state != DeliveryState::Uncertain
        {
            self.state = DeliveryState::Confirmed;
        }
    }

    pub(super) fn stop(&mut self) {
        if self.state == DeliveryState::AwaitingConfirmation {
            self.state = DeliveryState::Uncertain;
        }
        self.stopped = true;
    }
}
