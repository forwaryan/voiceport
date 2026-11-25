//! User-initiated, RAM-only input level. No AAC, speech service, clipboard,
//! credentials, or transcription. Owned by the controller on the main thread.
use super::{
    MAXIMUM_TAP_FRAMES, Recorder,
    devices::{OpenInput, Route},
    pcm_samples, validate_format,
};
use objc2_avf_audio::AVAudioPCMBuffer;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const DURATION: Duration = Duration::from_secs(10);
const STALE: Duration = Duration::from_millis(500);

#[derive(Default)]
struct Level(Option<(Instant, f64)>);
impl Level {
    fn observe(&mut self, buffer: &AVAudioPCMBuffer, now: Instant) {
        let Ok(pcm) = pcm_samples(buffer, MAXIMUM_TAP_FRAMES as usize) else {
            return;
        };
        if pcm.frames == 0 {
            return;
        }
        let mut peak = 0.0_f64;
        for frame in 0..pcm.frames {
            for channel in 0..pcm.channels {
                let sample = (pcm.sample)(frame, channel);
                if !sample.is_finite() {
                    return;
                }
                peak = peak.max(sample.abs());
            }
        }
        self.0 = Some((now, peak.min(1.0)));
    }

    fn text(&self, now: Instant) -> String {
        match self
            .0
            .filter(|(at, _)| now.saturating_duration_since(*at) <= STALE)
        {
            Some((_, 0.0)) => "峰值：静音（已收到采样）".into(),
            Some((_, peak)) => format!("峰值：{:.0} dBFS", 20.0 * peak.log10()),
            None => "峰值：等待有效采样…".into(),
        }
    }
}

pub(crate) struct LevelTest {
    input: OpenInput,
    route: Route,
    level: Arc<Mutex<Level>>,
    until: Instant,
}

impl LevelTest {
    pub fn start(preferred: Option<&str>) -> Result<Self, String> {
        super::main_thread()?;
        if Recorder::permission_status().0 != "authorized" {
            return Err("请先使用设置中的「检查麦克风与语音权限」，再测试输入电平。".into());
        }
        let (route, input) = OpenInput::select(preferred, |format| {
            validate_format(format).map_err(str::to_owned)
        })?;
        let level = Arc::new(Mutex::new(Level::default()));
        let output = level.clone();
        input.attach(move |buffer| {
            if let Ok(mut level) = output.try_lock() {
                level.observe(buffer, Instant::now());
            }
        });
        Ok(Self {
            input,
            route,
            level,
            until: Instant::now() + DURATION,
        })
    }

    pub fn status(&mut self, now: Instant) -> Result<String, &'static str> {
        if now >= self.until {
            return Err("电平测试已结束。");
        }
        if !self.input.healthy(&self.route, false) {
            return Err("测试期间输入设备已断开，测试已停止。");
        }
        let text = self
            .level
            .lock()
            .map_or_else(|_| "峰值：不可用".into(), |level| level.text(now));
        Ok(format!("{} · {text}", self.route.summary()))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reports_direct_peak_distinguishing_silence_no_frames_and_stale_readings() {
        use super::*;
        let now = Instant::now();
        let (_, buffer) = super::super::tests::synthetic_buffer(48_000., 2, 8);
        let mut level = Level::default();
        assert_eq!(level.text(now), "峰值：等待有效采样…");
        unsafe {
            let data = buffer.floatChannelData();
            assert!(!data.is_null());
            for n in 0..2 {
                std::slice::from_raw_parts_mut((*data.add(n)).as_ptr(), 8).fill(0.0);
            }
        }
        level.observe(&buffer, now);
        assert_eq!(level.text(now), "峰值：静音（已收到采样）");
        // A louder value on the second channel must not be hidden by mixing.
        unsafe {
            *(*buffer.floatChannelData().add(1)).as_ptr().add(4) = -0.5;
        }
        level.observe(&buffer, now);
        assert_eq!(level.text(now), "峰值：-6 dBFS");
        assert_eq!(
            level.text(now + STALE + Duration::from_millis(1)),
            "峰值：等待有效采样…"
        );
    }
}
