//! Per-capture observations and independent monotonic stage durations.

use std::time::{Duration, Instant};

/// Signal level at the input tap, before any channel mixing or normalization
/// beyond converting PCM to full-scale units. A level does not indicate speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum InputSignal {
    NoCallbacks,
    NoFrames,
    DigitalSilence,
    LowEnergy,
    Signal,
    #[default]
    Unknown,
}

/// A snapshot contains elapsed durations only. Overlapping stages can each
/// have a duration, and stages lacking either boundary are absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct StageTimings {
    /// From native capture preparation to observation of the first valid input frame.
    pub(crate) capture_first_frame: Option<Duration>,
    /// Input or model preparation, when its boundaries are observable.
    pub(crate) preparation: Option<Duration>,
    /// From the start of the recognition attempt to its observed outcome.
    pub(crate) transcription: Option<Duration>,
    /// From the attempted final delivery to its observed outcome.
    pub(crate) delivery: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StageTimer(Instant);

impl StageTimer {
    pub(crate) fn start(now: impl FnOnce() -> Instant) -> Self {
        Self(now())
    }

    pub(crate) fn finish(self, now: impl FnOnce() -> Instant) -> Option<Duration> {
        now().checked_duration_since(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct CaptureDiagnostics {
    pub(crate) signal: InputSignal,
    /// Callbacks received while the sink accepted input, including empty buffers.
    pub(crate) callbacks: Option<u64>,
    /// Frames successfully inspected within the recording limit, before encoding.
    pub(crate) observed_frames: Option<u64>,
    pub(crate) timings: StageTimings,
}

/// A diagnostic peak boundary of -60 dBFS. Each channel is tested before mixing.
/// Even a single nonzero sample below the boundary is distinct from digital zero.
const SIGNAL_PEAK: f64 = 0.001;

pub(super) struct CaptureObservation {
    started: StageTimer,
    first_frame: Option<Duration>,
    callbacks: u64,
    observed_frames: u64,
    signal: InputSignal,
    live_level: Option<(Instant, u8)>,
}

impl CaptureObservation {
    pub(super) fn new(started: StageTimer) -> Self {
        Self {
            started,
            first_frame: None,
            callbacks: 0,
            observed_frames: 0,
            signal: InputSignal::NoCallbacks,
            live_level: None,
        }
    }

    pub(super) fn callback(&mut self) {
        self.callbacks = self.callbacks.saturating_add(1);
        self.signal = self.signal.max(InputSignal::NoFrames);
    }

    pub(super) fn unavailable(&mut self) {
        self.signal = InputSignal::Unknown;
        self.live_level = None;
    }

    pub(super) fn live_level(&self, now: Instant) -> Option<u8> {
        self.live_level.and_then(|(observed, level)| {
            (now.checked_duration_since(observed)? < Duration::from_millis(300)).then_some(level)
        })
    }

    /// Observe the bounds-checked PCM prefix once. Only a short-lived, coarse
    /// pre-mix level (not PCM, a device name, or a speech decision) is retained.
    pub(super) fn observe(
        &mut self,
        frames: usize,
        channels: usize,
        sample: impl Fn(usize, usize) -> f64,
        now: impl FnOnce() -> Instant,
    ) {
        let observed = now();
        let mut peak = 0.0_f64;
        for frame in 0..frames {
            for channel in 0..channels {
                let sample = sample(frame, channel);
                if !sample.is_finite() {
                    self.unavailable();
                    return;
                }
                peak = peak.max(sample.abs());
            }
        }
        if self.observed_frames == 0 {
            self.first_frame = self.started.finish(|| observed);
        }
        let level = [0.005, 0.02, 0.08, 0.32]
            .into_iter()
            .take_while(|threshold| peak >= *threshold)
            .count() as u8;
        self.live_level = Some((observed, level));
        self.observed_frames += frames as u64;
        let signal = if peak >= SIGNAL_PEAK {
            InputSignal::Signal
        } else if peak > 0.0 {
            InputSignal::LowEnergy
        } else {
            InputSignal::DigitalSilence
        };
        self.signal = self.signal.max(signal);
    }

    pub(super) fn snapshot(&self) -> CaptureDiagnostics {
        CaptureDiagnostics {
            signal: self.signal,
            callbacks: Some(self.callbacks),
            observed_frames: Some(self.observed_frames),
            timings: StageTimings {
                capture_first_frame: self.first_frame,
                ..StageTimings::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_overlay_meter_is_coarse_recent_and_pre_mix() {
        use super::{CaptureObservation, Duration, Instant, StageTimer};
        let now = Instant::now();
        let mut observation = CaptureObservation::new(StageTimer::start(|| now));
        assert_eq!(observation.live_level(now), None);
        observation.observe(8, 2, |_, ch| if ch == 0 { 0.1 } else { -0.1 }, || now);
        assert_eq!(observation.live_level(now), Some(3));
        assert_eq!(
            observation.live_level(now + Duration::from_millis(300)),
            None
        );
        observation.observe(8, 2, |_, _| 0., || now);
        assert_eq!(observation.live_level(now), Some(0));
        observation.observe(8, 2, |_, _| f64::NAN, || now);
        assert_eq!(observation.live_level(now), None);
    }

    #[test]
    fn stage_timers_measure_overlap_independently_with_a_substitute_clock() {
        use super::{Duration, Instant, StageTimer, StageTimings};

        let epoch = Instant::now();
        let at = |ms| epoch + Duration::from_millis(ms);
        let preparation = StageTimer::start(|| at(3));
        let transcription = StageTimer::start(|| at(7));
        let timings = StageTimings {
            preparation: preparation.finish(|| at(11)),
            transcription: transcription.finish(|| at(23)),
            ..StageTimings::default()
        };
        assert_eq!(timings.preparation, Some(Duration::from_millis(8)));
        assert_eq!(timings.transcription, Some(Duration::from_millis(16)));
        assert_eq!(timings.delivery, None);
        assert_eq!(timings.capture_first_frame, None);
        assert_eq!(StageTimer::start(|| at(23)).finish(|| at(22)), None);
    }

    #[test]
    fn first_frame_is_timed_once_after_empty_and_unobservable_buffers() {
        use super::{CaptureObservation, Duration, InputSignal, Instant, StageTimer};

        let started = Instant::now();
        let mut observation = CaptureObservation::new(StageTimer::start(|| started));
        observation.callback(); // Empty buffer.
        observation.callback();
        observation.observe(2, 1, |_, _| f64::NAN, || started);
        assert_eq!(observation.snapshot().timings.capture_first_frame, None);
        observation.callback();
        observation.observe(2, 1, |_, _| 0.0, || started + Duration::from_millis(5));
        let later = started + Duration::from_millis(310);
        assert_eq!(observation.live_level(later), None);
        observation.callback();
        observation.observe(2, 1, |_, _| 0.04, || later);
        assert_eq!(observation.live_level(later), Some(2));
        let snapshot = observation.snapshot();
        assert_eq!(snapshot.signal, InputSignal::Unknown);
        assert_eq!(snapshot.callbacks, Some(4));
        assert_eq!(snapshot.observed_frames, Some(4));
        assert_eq!(
            snapshot.timings.capture_first_frame,
            Some(Duration::from_millis(5))
        );
    }
}
