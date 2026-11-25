//! Content-free evidence for the latest GUI dictation and its delivery transport.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiDictationReceipt {
    /// The initial Controller generation, scoped to this App process.
    pub session: u64,
    /// The current transcription attempt. An explicit recording retry advances it.
    pub generation: u64,
    pub automatic: bool,
    pub transcription: TranscriptionState,
    pub delivery: DeliveryReceipt,
    pub method: Option<String>,
    pub failure_stage: Option<String>,
    pub block_reason: Option<DictationBlockReason>,
}

/// Diagnostics for the same GUI session and attempt as the delivery receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuiDictationDiagnostics {
    pub session: u64,
    pub generation: u64,
    pub capture: Option<CaptureSummary>,
    pub timings: DictationTimings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureSummary {
    /// The generation that actually captured audio. A retry keeps this generation.
    pub generation: u64,
    pub signal: CaptureSignal,
    pub callbacks: Option<u64>,
    pub observed_frames: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSignal {
    NoCallbacks,
    NoFrames,
    DigitalSilence,
    LowEnergy,
    Signal,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictationTimings {
    pub capture_first_frame_ms: Option<u64>,
    pub preparation_ms: Option<u64>,
    pub transcription_ms: Option<u64>,
    pub delivery_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionState {
    NotStarted,
    Pending,
    Completed,
    Empty,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictationBlockReason {
    PanelOnly,
    RetryPanelOnly,
    StartRejected,
    SourceUnavailable,
    UnsupportedLanguage,
    MicrophoneDenied,
    CaptureFailed,
    TranscriptionFailed,
    EmptyTranscript,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// Counts write operations, including preview corrections and restoration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryReceipt {
    pub state: DeliveryState,
    pub submitted_operations: usize,
    pub confirmed_operations: usize,
    pub stopped: bool,
}

impl DeliveryReceipt {
    pub fn waiting(&mut self) {
        self.state = if self.submitted_operations == 0 {
            DeliveryState::Waiting
        } else {
            DeliveryState::PartiallySent
        };
    }

    /// Call before crossing a potentially mutating boundary. An error from
    /// that boundary cannot prove that no side effect occurred.
    pub fn attempting(&mut self) {
        self.state = DeliveryState::AwaitingConfirmation;
    }

    pub fn submitted(&mut self, acknowledged: bool) {
        self.submitted_operations += 1;
        self.state = if acknowledged {
            DeliveryState::AwaitingConfirmation
        } else {
            DeliveryState::Submitted
        };
    }

    pub fn confirmed(&mut self) {
        self.confirmed_operations += 1;
        self.state = DeliveryState::PartiallySent;
    }

    pub fn finish(&mut self) {
        if self.submitted_operations > 0
            && self.confirmed_operations == self.submitted_operations
            && self.state != DeliveryState::Uncertain
        {
            self.state = DeliveryState::Confirmed;
        }
    }

    pub fn stop(&mut self) {
        self.state = match self.state {
            DeliveryState::AwaitingConfirmation => DeliveryState::Uncertain,
            DeliveryState::Waiting => DeliveryState::NotSent,
            state => state,
        };
        self.stopped = true;
    }
}
