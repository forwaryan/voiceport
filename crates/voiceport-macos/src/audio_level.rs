//! User-initiated, RAM-only input level. No AAC, speech service, clipboard,
//! credentials, or transcription. Native input belongs to the audio worker.
use super::{
    MAXIMUM_TAP_FRAMES, Recorder,
    devices::{OpenInput, Route},
    pcm_samples, validate_format,
    worker::{Lane, WAIT},
};
use objc2_avf_audio::AVAudioPCMBuffer;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
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

struct LevelTest {
    input: OpenInput,
    route: Route,
    level: Arc<Mutex<Level>>,
    until: Instant,
}

impl LevelTest {
    fn start(
        preferred: Option<&str>,
        cancelled: Arc<AtomicBool>,
        check: &impl Fn() -> Result<(), String>,
    ) -> Result<Self, String> {
        if Recorder::check_microphone().is_err() {
            return Err("请先使用设置中的「检查麦克风与语音权限」，再测试输入电平。".into());
        }
        let (route, mut input) = OpenInput::select(preferred, check, |format| {
            validate_format(format).map_err(str::to_owned)
        })?;
        input.verify_start(&route, check)?;
        let level = Arc::new(Mutex::new(Level::default()));
        let output = level.clone();
        input.attach(
            &|| {
                Recorder::check_microphone()?;
                check()
            },
            move || !cancelled.load(Ordering::Acquire),
            move |buffer| {
                if let Ok(mut level) = output.try_lock() {
                    level.observe(buffer, Instant::now());
                }
            },
        )?;
        Ok(Self {
            input,
            route,
            level,
            until: Instant::now() + DURATION,
        })
    }

    fn status(&mut self, now: Instant) -> Result<String, &'static str> {
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

pub(crate) struct Job {
    pub id: String,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<Option<(Instant, String)>>>,
    result: Receiver<String>,
    deadline: Instant,
    last_update: Option<Instant>,
}

impl Job {
    pub fn start(lane: &Lane, preferred: Option<String>) -> Result<Self, String> {
        let (done, result) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(None));
        let snapshot = status.clone();
        let cancel = cancelled.clone();
        let deadline = Instant::now() + WAIT;
        lane.spawn(move || {
            let check = || {
                if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
                    Err("麦克风测试准备已取消或超时。".into())
                } else {
                    Ok(())
                }
            };
            let run = || {
                let mut input = LevelTest::start(preferred.as_deref(), cancel.clone(), &check)?;
                while !cancel.load(Ordering::Acquire) {
                    let now = Instant::now();
                    let text =
                        objc2::rc::autoreleasepool(|_| input.status(now)).map_err(str::to_owned)?;
                    if let Ok(mut slot) = snapshot.lock() {
                        *slot = Some((now, text));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(())
            };
            if let Err(message) = run() {
                let _ = done.send(message);
            }
        })?;
        Ok(Self {
            id: uuid::Uuid::new_v4().to_string(),
            cancel: cancelled,
            status,
            result,
            deadline,
            last_update: None,
        })
    }

    pub fn measuring(&self) -> bool {
        self.last_update.is_some()
    }

    pub fn display_is_stale(&self, now: Instant) -> bool {
        self.last_update
            .is_some_and(|updated| now.saturating_duration_since(updated) > STALE)
    }

    pub fn status(&mut self, now: Instant) -> Result<Option<String>, String> {
        match self.result.try_recv() {
            Ok(message) => return Err(message),
            Err(TryRecvError::Disconnected) => return Err("麦克风测试已结束。".into()),
            Err(TryRecvError::Empty) => {}
        }
        if let Ok(slot) = self.status.try_lock()
            && let Some((updated, text)) = slot.as_ref()
        {
            self.deadline = *updated + WAIT;
            if now < self.deadline {
                self.last_update = Some(*updated);
                return Ok(Some(text.clone()));
            }
        }
        if now >= self.deadline {
            return Err("等待麦克风测试超时。".into());
        }
        Ok(None)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
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
