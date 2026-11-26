use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(500);
const STALE_AFTER: Duration = Duration::from_secs(2);

/// A single latest-value slot for this attempt, shared by the worker and menu.
#[derive(Clone, Default)]
pub(crate) struct DownloadProgress(Arc<Mutex<DownloadSnapshot>>);

#[derive(Clone, Copy, Default)]
pub(crate) struct DownloadSnapshot {
    pub(crate) received: u64,
    pub(crate) verifying: bool,
    total: Option<u64>,
    sample: Option<(Instant, u64)>,
    last_chunk: Option<Instant>,
    rate: Option<f64>,
}

impl DownloadProgress {
    pub(crate) fn set_total(&self, total: Option<u64>) {
        self.0.lock().unwrap().total = total.filter(|&total| total > 0);
    }

    pub(crate) fn received(&self, bytes: u64) {
        self.received_at(bytes, Instant::now());
    }

    fn received_at(&self, bytes: u64, now: Instant) {
        let mut slot = self.0.lock().unwrap();
        if bytes <= slot.received {
            return;
        }
        if slot
            .last_chunk
            .is_none_or(|last| now.duration_since(last) >= STALE_AFTER)
        {
            slot.sample = Some((now, bytes));
            slot.rate = None;
        } else if let Some((at, received)) = slot.sample {
            let elapsed = now.duration_since(at);
            if elapsed >= SAMPLE_INTERVAL {
                slot.rate = Some((bytes - received) as f64 / elapsed.as_secs_f64());
                slot.sample = Some((now, bytes));
            }
        }
        slot.received = bytes;
        slot.last_chunk = Some(now);
    }

    pub(crate) fn verifying(&self) {
        self.0.lock().unwrap().verifying = true;
    }

    pub(crate) fn snapshot(&self) -> DownloadSnapshot {
        *self.0.lock().unwrap()
    }
}

impl DownloadSnapshot {
    pub(crate) fn percent(self) -> Option<u64> {
        let total = self.total?;
        if self.verifying {
            return (self.received == total).then_some(100);
        }
        // The transport still needs to finish successfully, even when all
        // expected bytes have arrived (for example, before gh exits).
        (self.received <= total).then(|| (self.received * 100 / total).min(99))
    }

    pub(crate) fn rate(self, now: Instant) -> Option<f64> {
        if self.verifying || now.duration_since(self.last_chunk?) >= STALE_AFTER {
            return None;
        }
        self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_uses_intervals_and_expires_across_a_stall() {
        let progress = DownloadProgress::default();
        let start = Instant::now();
        progress.received_at(60, start);
        progress.received_at(100, start);
        assert_eq!(progress.snapshot().rate(start), None);
        let second = start + SAMPLE_INTERVAL;
        progress.received_at(600, second);
        assert_eq!(progress.snapshot().rate(second), Some(1080.0));
        let third = second + SAMPLE_INTERVAL;
        progress.received_at(800, third);
        assert_eq!(progress.snapshot().rate(third), Some(400.0));
        let resume = third + STALE_AFTER;
        assert_eq!(progress.snapshot().rate(resume), None);
        progress.received_at(1200, resume);
        assert_eq!(progress.snapshot().rate(resume), None);
        let next = resume + SAMPLE_INTERVAL;
        progress.received_at(1400, next);
        assert_eq!(progress.snapshot().rate(next), Some(400.0));
    }

    #[test]
    fn even_a_fast_download_reaches_100_only_after_successful_transfer() {
        let progress = DownloadProgress::default();
        progress.set_total(Some(10));
        assert_eq!(progress.snapshot().percent(), Some(0));
        progress.received(10);
        assert_eq!(progress.snapshot().percent(), Some(99));
        assert_eq!(progress.snapshot().rate(Instant::now()), None);
        progress.verifying();
        assert_eq!(progress.snapshot().percent(), Some(100));
        assert_eq!(progress.snapshot().rate(Instant::now()), None);
        for total in [None, Some(0), Some(1)] {
            let unknown = DownloadProgress::default();
            unknown.set_total(total);
            unknown.received(2);
            assert_eq!(unknown.snapshot().percent(), None);
            unknown.verifying();
            assert_eq!(unknown.snapshot().percent(), None);
        }
    }
}
