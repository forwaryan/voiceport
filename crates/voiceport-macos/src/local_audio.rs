//! Bounded transport and continuous fractional-rate conversion, without native I/O.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
};
pub const RATE: usize = 16_000;
pub const MAX_SAMPLES: usize = RATE * 180;
pub const MAX_PACKET_FRAMES: usize = 8192;
const QUEUE_PACKETS: usize = 128;
#[derive(Debug)]
pub struct Packet {
    pub rate: f64,
    pub channels: usize,
    pub samples: Vec<f32>,
}
impl Packet {
    pub fn validate(&self) -> Result<(), String> {
        if !self.rate.is_finite()
            || !(8_000.0..=192_000.0).contains(&self.rate)
            || !(1..=8).contains(&self.channels)
            || self.samples.is_empty()
            || !self.samples.len().is_multiple_of(self.channels)
            || self.samples.len() / self.channels > MAX_PACKET_FRAMES
            || self.samples.iter().any(|s| !s.is_finite())
        {
            return Err("本地转写收到无效或不支持的 PCM 音频。".into());
        }
        Ok(())
    }
}
pub struct Input {
    sender: SyncSender<Packet>,
    finished: Arc<AtomicBool>,
    fault: Arc<AtomicU8>,
}
pub struct Incoming {
    pub receiver: Receiver<Packet>,
    finished: Arc<AtomicBool>,
    fault: Arc<AtomicU8>,
}
pub fn channel() -> (Input, Incoming) {
    let (sender, receiver) = mpsc::sync_channel(QUEUE_PACKETS);
    let finished = Arc::new(AtomicBool::new(false));
    let fault = Arc::new(AtomicU8::new(0));
    (
        Input {
            sender,
            finished: finished.clone(),
            fault: fault.clone(),
        },
        Incoming {
            receiver,
            finished,
            fault,
        },
    )
}
impl Input {
    pub fn send(&self, packet: Packet) -> Result<(), String> {
        packet.validate()?;
        match self.sender.try_send(packet) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.fault.store(1, Ordering::Release);
                Err("本地转写音频队列已满，请停止并重试保留的录音。".into())
            }
            Err(TrySendError::Disconnected(_)) => {
                self.fault.store(2, Ordering::Release);
                Err("本地转写线程已断开，请重试保留的录音。".into())
            }
        }
    }
    pub fn finish(self) {
        self.finished.store(true, Ordering::Release);
    }
}
impl Incoming {
    pub fn check(&self) -> Result<(), String> {
        match self.fault.load(Ordering::Acquire) {
            0 => Ok(()),
            1 => Err("本地转写音频队列溢出，录音需手动重试。".into()),
            _ => Err("本地音频工作线程已断开。".into()),
        }
    }
    pub fn completed(&self) -> Result<(), String> {
        self.check()?;
        if self.finished.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err("本地音频通道意外断开，录音未完整结束。".into())
        }
    }
}
#[derive(Default)]
pub struct Resampler {
    rate: Option<f64>,
    pending: Vec<f32>,
    position: f64,
    output_samples: usize,
}
impl Resampler {
    pub fn push(&mut self, packet: Packet) -> Result<Vec<f32>, String> {
        packet.validate()?;
        if self.rate.is_some_and(|rate| rate != packet.rate) {
            return Err("录音期间麦克风采样率发生变化，请重新录音。".into());
        }
        self.rate = Some(packet.rate);
        self.pending
            .extend(packet.samples.chunks_exact(packet.channels).map(|frame| {
                frame.iter().map(|s| *s as f64).sum::<f64>() as f32 / packet.channels as f32
            }));
        self.convert(false)
    }
    pub fn finish(&mut self) -> Result<Vec<f32>, String> {
        self.convert(true)
    }
    fn convert(&mut self, finish: bool) -> Result<Vec<f32>, String> {
        let Some(rate) = self.rate else {
            return Ok(vec![]);
        };
        let step = rate / RATE as f64;
        let mut output = Vec::new();
        while self.position + if finish { 0.0 } else { 1.0 } < self.pending.len() as f64 {
            if self.output_samples >= MAX_SAMPLES {
                return Err("本地转写音频超过 180 秒。".into());
            }
            let index = self.position.floor() as usize;
            let a = self.pending[index];
            let b = self.pending.get(index + 1).copied().unwrap_or(a);
            let value = a + (b - a) * (self.position - index as f64) as f32;
            if !value.is_finite() {
                return Err("PCM 混音或重采样产生无效数值。".into());
            }
            output.push(value);
            self.output_samples += 1;
            self.position += step;
        }
        let consumed = (self.position.floor() as usize).min(self.pending.len());
        self.pending.drain(..consumed);
        self.position -= consumed as f64;
        if finish {
            self.pending.clear();
        }
        Ok(output)
    }
}
#[derive(Default)]
pub struct Transcript {
    committed: String,
}
impl Transcript {
    pub fn current(&self, partial: &str) -> String {
        let text = partial.trim();
        match (self.committed.is_empty(), text.is_empty()) {
            (_, true) => self.committed.clone(),
            (true, false) => text.into(),
            (false, false) => format!("{} {text}", self.committed),
        }
    }
    pub fn commit(&mut self, text: &str) {
        self.committed = self.current(text);
    }
}
pub fn quiet_split(samples: &[f32]) -> usize {
    let limit = (30 * RATE).min(samples.len());
    if limit < 30 * RATE {
        return limit;
    }
    let window = RATE / 10;
    ((limit - 5 * RATE)..=limit - window)
        .step_by(window / 2)
        .min_by(|a, b| {
            let energy = |start: usize| {
                samples[start..start + window]
                    .iter()
                    .map(|s| (*s as f64).powi(2))
                    .sum::<f64>()
            };
            energy(*a).total_cmp(&energy(*b))
        })
        .map_or(limit, |index| index + window / 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(rate: f64, channels: usize, frames: usize) -> Packet {
        Packet {
            rate,
            channels,
            samples: vec![0.25; frames * channels],
        }
    }
    #[test]
    fn fractional_stereo_resampling_is_continuous_and_flushes_tail() {
        let mut split = Resampler::default();
        let mut values = vec![];
        for _ in 0..441 {
            values.extend(split.push(packet(44_100.5, 2, 100)).unwrap());
        }
        let before = values.len();
        values.extend(split.finish().unwrap());
        let mut whole = Resampler::default();
        let mut other = vec![];
        for _ in 0..7 {
            other.extend(whole.push(packet(44_100.5, 2, 6300)).unwrap());
        }
        other.extend(whole.finish().unwrap());
        assert!(values.len() >= before);
        assert_eq!(values.len(), 16000);
        assert_eq!(values, other);
        let mut tail = Resampler::default();
        assert_eq!(tail.push(packet(16000., 1, 5)).unwrap().len(), 4);
        assert_eq!(tail.finish().unwrap().len(), 1);
        assert!(values.iter().all(|v| (*v - 0.25).abs() < 1e-6));
    }
    #[test]
    fn rejects_nonfinite_unsupported_and_changing_pcm_before_native_calls() {
        for rate in [f64::NAN, f64::INFINITY, 0., 7999., 192001.] {
            assert!(packet(rate, 1, 1).validate().is_err());
        }
        for channels in [0, 9] {
            assert!(packet(16000., channels, 1).validate().is_err());
        }
        for sample in [f32::NAN, f32::INFINITY] {
            assert!(
                Packet {
                    rate: 16000.,
                    channels: 1,
                    samples: vec![sample]
                }
                .validate()
                .is_err()
            );
        }
        let mut r = Resampler::default();
        r.push(packet(48000., 1, 3)).unwrap();
        assert!(r.push(packet(44100., 1, 3)).is_err());
    }
    #[test]
    fn stop_drains_every_queued_packet_and_disconnect_is_explicit() {
        let (input, incoming) = channel();
        for _ in 0..8 {
            input.send(packet(16000., 1, 5)).unwrap();
        }
        input.finish();
        assert_eq!(
            incoming
                .receiver
                .iter()
                .map(|p| p.samples.len())
                .sum::<usize>(),
            40
        );
        incoming.completed().unwrap();
        let (input, incoming) = channel();
        drop(input);
        assert!(incoming.completed().is_err());
    }
    #[test]
    fn overflow_is_latched_even_after_stop_and_receiver_failure_is_explicit() {
        let (input, incoming) = channel();
        for _ in 0..QUEUE_PACKETS {
            input.send(packet(16000., 1, 1)).unwrap();
        }
        assert!(input.send(packet(16000., 1, 1)).is_err());
        input.finish();
        assert!(incoming.completed().is_err());
        let (input, incoming) = channel();
        drop(incoming);
        assert!(input.send(packet(16000., 1, 1)).is_err());
    }
    #[test]
    fn endpoint_reset_accumulates_full_text_without_repeating_current_partial() {
        let mut t = Transcript::default();
        assert_eq!(t.current("hello"), "hello");
        assert_eq!(t.current("hello world"), "hello world");
        t.commit("hello world");
        assert_eq!(t.current(""), "hello world");
        assert_eq!(t.current("你好"), "hello world 你好");
        t.commit("你好");
        assert_eq!(t.current("tail"), "hello world 你好 tail");
    }
    #[test]
    fn normalized_audio_has_an_independent_hard_duration_limit() {
        let mut r = Resampler {
            output_samples: MAX_SAMPLES,
            ..Resampler::default()
        };
        assert!(r.push(packet(16000., 1, 2)).unwrap_err().contains("180"));
    }
    #[test]
    fn quiet_chunks_are_bounded_and_cover_all_samples() {
        let samples = vec![0.01; MAX_SAMPLES];
        let mut count = 0;
        while count < samples.len() {
            let size = quiet_split(&samples[count..]);
            assert!(size > 0 && size <= 30 * RATE);
            count += size;
        }
        assert_eq!(count, MAX_SAMPLES);
    }
}
