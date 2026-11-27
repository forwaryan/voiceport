//! Process-local, opt-in history of final GUI dictations.

use super::{BlockReason, Controller, TranscriptionState, VisibleTranscript};
use objc2_foundation::{NSString, NSStringCompareOptions};
use serde::Serialize;
use std::{collections::VecDeque, time::SystemTime};
use voiceport_control::{DeliveryReceipt, DeliveryState, HistoryDelivery};

#[cfg(debug_assertions)]
#[path = "recent_dictations/fixture.rs"]
pub(crate) mod fixture;

pub(crate) const MAX_ENTRIES: usize = 5;
pub(crate) const MAX_ENTRY_BYTES: usize = 16 * 1024;
const MAX_TOTAL_BYTES: usize = 48 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecentDictationMetadata {
    pub id: String,
    pub created_at_unix_ms: u64,
    pub text_available: bool,
    pub delivery: DeliveryReceipt,
    pub method: Option<String>,
    pub failure_stage: Option<String>,
}

impl RecentDictationMetadata {
    /// An explicit failure requires evidence that the target write never started.
    pub(crate) fn delivery_filter(&self) -> HistoryDelivery {
        let receipt = self.delivery;
        let failed = self.failure_stage.is_some();
        match receipt.state {
            DeliveryState::Confirmed
                if !receipt.stopped
                    && !failed
                    && receipt.submitted_operations > 0
                    && receipt.confirmed_operations == receipt.submitted_operations =>
            {
                HistoryDelivery::Confirmed
            }
            DeliveryState::Waiting | DeliveryState::AwaitingConfirmation
                if !receipt.stopped && !failed =>
            {
                HistoryDelivery::Pending
            }
            DeliveryState::NotSent | DeliveryState::Waiting
                if receipt.submitted_operations == 0 && receipt.confirmed_operations == 0 =>
            {
                if failed {
                    HistoryDelivery::Failed
                } else {
                    HistoryDelivery::NotSent
                }
            }
            _ => HistoryDelivery::Uncertain,
        }
    }
}

struct Entry {
    generation: u64,
    metadata: RecentDictationMetadata,
    text: Option<String>,
}

#[derive(Default)]
pub(super) struct RecentDictations {
    enabled: bool,
    candidate: Option<u64>,
    entries: VecDeque<Entry>,
    #[cfg(any(test, debug_assertions))]
    fixture_focus_safe: Option<bool>,
}

impl RecentDictations {
    fn clear(&mut self) -> usize {
        self.candidate = None;
        let removed = self.entries.len();
        self.entries.clear();
        removed
    }

    fn begin(&mut self, generation: u64, focus_safe: bool) {
        self.candidate = (self.enabled && focus_safe).then_some(generation);
    }

    fn record(&mut self, generation: u64, text: &str, receipt: &super::GuiDictationReceipt) {
        if self.candidate != Some(generation) {
            return;
        }
        self.candidate = None;
        if text.trim().is_empty() {
            return;
        }
        let text = (text.len() <= MAX_ENTRY_BYTES).then(|| text.to_owned());
        let created_at_unix_ms = SystemTime::UNIX_EPOCH
            .elapsed()
            .ok()
            .and_then(|elapsed| elapsed.as_millis().try_into().ok())
            .unwrap_or_default();
        self.entries.push_front(Entry {
            generation,
            metadata: RecentDictationMetadata {
                id: uuid::Uuid::new_v4().to_string(),
                created_at_unix_ms,
                text_available: text.is_some(),
                delivery: receipt.delivery,
                method: receipt.method.clone(),
                failure_stage: receipt.failure_stage.clone(),
            },
            text,
        });
        let mut bytes = 0;
        let keep = self
            .entries
            .iter()
            .take(MAX_ENTRIES)
            .take_while(|entry| {
                bytes += entry.text.as_ref().map_or(0, String::len);
                bytes <= MAX_TOTAL_BYTES
            })
            .count();
        self.entries.truncate(keep);
    }
}

impl Controller {
    pub(crate) fn recent_dictations_enabled(&self) -> bool {
        self.recent_dictations.enabled
    }

    pub(crate) fn set_recent_dictations_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.clear_recent_dictations();
        }
        self.recent_dictations.enabled = enabled;
    }

    pub(crate) fn list_recent_dictations(&self, limit: usize) -> Vec<RecentDictationMetadata> {
        self.search_recent_dictations(limit, "", None)
    }

    pub(crate) fn has_recent_dictations(&self) -> bool {
        !self.recent_dictations.entries.is_empty()
    }

    pub(crate) fn search_recent_dictations(
        &self,
        limit: usize,
        query: &str,
        delivery: Option<HistoryDelivery>,
    ) -> Vec<RecentDictationMetadata> {
        // Fold both sides rather than lowercasing in context: a word-final Σ
        // and an isolated Σ otherwise become different lowercase characters.
        let fold = |text: &str| {
            NSString::from_str(text)
                .stringByFoldingWithOptions_locale(
                    NSStringCompareOptions::CaseInsensitiveSearch,
                    None,
                )
                .to_string()
        };
        let query = query.trim();
        let query = if query.is_empty() {
            String::new()
        } else {
            fold(query)
        };
        self.recent_dictations
            .entries
            .iter()
            .filter(|entry| {
                delivery.is_none_or(|filter| entry.metadata.delivery_filter() == filter)
            })
            .filter(|entry| {
                query.is_empty()
                    || entry
                        .text
                        .as_deref()
                        .is_some_and(|text| fold(text).contains(&query))
            })
            .take(limit)
            .map(|entry| entry.metadata.clone())
            .collect()
    }

    pub(crate) fn recent_dictation_metadata(&self, id: &str) -> Option<&RecentDictationMetadata> {
        self.recent_dictation_entry(id).map(|entry| &entry.metadata)
    }

    /// Explicit lookup in this process. A missing, cleared, evicted, or oversized entry has no text.
    pub(crate) fn read_recent_dictation(&self, id: &str) -> Option<&str> {
        self.recent_dictation_entry(id)?.text.as_deref()
    }

    fn recent_dictation_entry(&self, id: &str) -> Option<&Entry> {
        self.recent_dictations
            .entries
            .iter()
            .find(|entry| entry.metadata.id == id)
    }

    pub(crate) fn can_clear_recent_dictations(&self) -> bool {
        let history = &self.recent_dictations;
        history.candidate.is_some() || !history.entries.is_empty()
    }

    pub(crate) fn clear_recent_dictations(&mut self) -> usize {
        self.recent_dictations.clear()
    }

    fn history_focus_safe(&self) -> bool {
        #[cfg(any(test, debug_assertions))]
        if let Some(focus_safe) = self.recent_dictations.fixture_focus_safe {
            return focus_safe;
        }
        crate::delivery::Delivery::history_focus_safe()
    }

    fn history_secure_input_active(&self) -> bool {
        #[cfg(any(test, debug_assertions))]
        if let Some(focus_safe) = self.recent_dictations.fixture_focus_safe {
            return !focus_safe;
        }
        crate::delivery::Delivery::secure_input()
    }

    pub(super) fn begin_recent_dictation(&mut self, generation: u64) {
        let safe = self.recent_dictations.enabled && self.history_focus_safe();
        self.recent_dictations.begin(generation, safe);
    }

    pub(super) fn retry_recent_dictation(&mut self, previous: u64, generation: u64) {
        if self.recent_dictations.candidate == Some(previous) {
            self.begin_recent_dictation(generation);
        }
    }

    pub(super) fn discard_recent_dictation_candidate(&mut self) {
        self.recent_dictations.candidate = None;
    }

    pub(super) fn observe_recent_dictation(&mut self) {
        if !self.recent_dictations.enabled {
            return;
        }
        let Some(generation) = self.gui_receipt.as_ref().map(|receipt| receipt.generation) else {
            return;
        };
        if self.recent_dictations.candidate == Some(generation)
            && self.history_secure_input_active()
        {
            self.discard_recent_dictation_candidate();
        }
        let candidate =
            self.recent_dictations.candidate == Some(generation) && self.state.accepts(generation);
        let existing = self
            .recent_dictations
            .entries
            .iter()
            .position(|entry| entry.generation == generation);
        if !candidate && existing.is_none() {
            return;
        }
        let Some(mut receipt) = self
            .gui_dictation_receipt()
            .filter(|r| r.generation == generation)
        else {
            return;
        };
        if matches!(
            receipt.failure_stage.as_deref(),
            Some("secureInput" | "sensitive")
        ) {
            self.discard_recent_dictation_candidate();
            if let Some(index) = existing {
                self.recent_dictations.entries.remove(index);
            }
            return;
        }
        if receipt.block_reason == Some(BlockReason::RetryPanelOnly) {
            // The recovery card carries the original attempt's delivery evidence.
            // History associates the receipt with the generation that produced its text.
            receipt.delivery = DeliveryReceipt::default();
            receipt.method = None;
            receipt.failure_stage = None;
        }
        if let Some(index) = existing {
            let metadata = &mut self.recent_dictations.entries[index].metadata;
            metadata.delivery = receipt.delivery;
            metadata.method = receipt.method;
            metadata.failure_stage = receipt.failure_stage;
            return;
        }
        if !matches!(self.visible_transcript, Some(VisibleTranscript::Final(g)) if g == generation)
            || receipt.transcription != TranscriptionState::Completed
            || self.delivery.preparing_input()
        {
            return;
        }
        if self.history_focus_safe() {
            self.recent_dictations
                .record(generation, &self.transcript, &receipt);
        } else {
            self.discard_recent_dictation_candidate();
        }
    }
}
