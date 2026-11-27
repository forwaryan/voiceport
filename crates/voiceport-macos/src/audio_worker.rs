//! One native audio operation at a time. AppKit owns only commands and plain
//! replies. A blocked framework call keeps the slot until native teardown ends.
use super::{EventSender, Recorder, Recovery, devices};
use crate::{local_audio, storage::Recording};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
    },
    time::{Duration, Instant},
};

pub(crate) const WAIT: Duration = Duration::from_secs(5);
const AUDIO_POLL: Duration = Duration::from_millis(100);
pub(crate) const AUDIO_BUSY: &str =
    "系统音频仍在处理，暂不能打开麦克风。若一直无法恢复，请退出并重新打开 VoicePort。";
pub(crate) const START_TIMEOUT: &str = "等待麦克风超时，本轮未开始录音。";
pub(crate) const RECOVERY_TIMEOUT: &str = "等待麦克风恢复超时，本轮音频未提交转写。";
pub(crate) const STOP_TIMEOUT: &str = "等待麦克风停止超时，本轮音频未提交转写。";
const CANCELLED: &str = "本轮音频操作已取消。";

#[derive(Default)]
pub(crate) struct Lane {
    reserved: Arc<AtomicBool>,
    #[cfg(debug_assertions)]
    pub(crate) fixture: Option<devices::fixture::Backend>,
}

struct Reservation(Arc<AtomicBool>);
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Lane {
    pub fn busy(&self) -> bool {
        self.reserved.load(Ordering::Acquire)
    }

    pub fn microphone_authorized(&self) -> bool {
        #[cfg(debug_assertions)]
        if let Some(fixture) = &self.fixture {
            return fixture.authorized();
        }
        Recorder::check_microphone().is_ok()
    }

    pub(super) fn spawn(&self, run: impl FnOnce() + Send + 'static) -> Result<(), String> {
        if self.reserved.swap(true, Ordering::AcqRel) {
            return Err(AUDIO_BUSY.into());
        }
        let reservation = Reservation(self.reserved.clone());
        #[cfg(debug_assertions)]
        let fixture = self.fixture.clone();
        std::thread::Builder::new()
            .name("voiceport-audio".into())
            .spawn(move || {
                let _reservation = reservation;
                #[cfg(debug_assertions)]
                devices::fixture::set_current(fixture);
                objc2::rc::autoreleasepool(|_| run());
            })
            .map(|_| ()) // Dropping the handle never joins the native thread.
            .map_err(|_| "无法启动音频线程，请重试。".into())
    }

    pub fn inventory(&self) -> Result<Inventory, String> {
        self.query(devices::inputs)
    }

    pub fn input_snapshot(&self) -> Result<Inventory<InputSnapshot>, String> {
        self.query(|| {
            Ok(InputSnapshot {
                observed: Instant::now(),
                devices: devices::inputs(),
                system: devices::system_input_preview(),
                permission: Recorder::microphone_permission_status(),
            })
        })
    }

    fn query<T: Send + 'static>(
        &self,
        read: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<Inventory<T>, String> {
        let deadline = Instant::now() + WAIT;
        let (tx, result) = mpsc::sync_channel(1);
        self.spawn(move || {
            let _ = tx.send(read());
        })?;
        Ok(Inventory { result, deadline })
    }
}

/// Observations for the input-device page. Never used by capture or test admission.
pub(crate) struct InputSnapshot {
    pub observed: Instant,
    pub devices: Result<Vec<devices::Device>, String>,
    pub system: Result<devices::Device, String>,
    pub permission: &'static str,
}

pub(crate) struct Inventory<T = Vec<devices::Device>> {
    result: Receiver<Result<T, String>>,
    deadline: Instant,
}

impl<T> Inventory<T> {
    pub fn poll(&self) -> Option<Result<T, String>> {
        if Instant::now() >= self.deadline {
            return Some(Err("等待读取系统音频设备超时。".into()));
        }
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("音频设备读取已结束。".into())),
        }
    }
}

pub(crate) struct Start {
    pub generation: u64,
    pub events: EventSender,
    pub speech: bool,
    pub language: String,
    pub preferred_uid: Option<String>,
    pub local: Option<local_audio::Input>,
}

pub(crate) enum Reply {
    Prepared,
    Started {
        preview: bool,
        lease: Arc<Recording>,
        route: String,
    },
    Recovering,
    Route(String),
    Stopped {
        result: Result<(), String>,
        diagnostics: Option<super::CaptureDiagnostics>,
    },
    Interrupted {
        reason: String,
        diagnostics: Option<super::CaptureDiagnostics>,
    },
    Failed(String),
}

enum Command {
    Commit,
    Stop,
}

// Stop immediately closes tap admission while preserving the admitted audio
// for finalization. Cancel closes it without producing a usable recording.
#[derive(Clone, Default)]
pub(super) struct Activity(Arc<AtomicU8>);
impl Activity {
    const ACCEPTING: u8 = 0;
    const STOPPING: u8 = 1;
    const CANCELLED: u8 = 2;
    const INTERRUPTED: u8 = 3;

    fn close_interrupted(&self) -> Result<(), String> {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state != Self::CANCELLED).then_some(Self::INTERRUPTED)
            })
            .map(|_| ())
            .map_err(|_| CANCELLED.into())
    }

    pub fn accepting(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::ACCEPTING
    }

    fn stopping(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::STOPPING
    }

    fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == Self::CANCELLED
    }
}

pub(crate) struct Capture {
    pub generation: u64,
    pub deadline: Instant,
    pub replies: Receiver<Reply>,
    commands: SyncSender<Command>,
    activity: Activity,
    // Only a validated Stop or interrupted reply can transfer this path.
    pending_recording: Arc<Mutex<Option<Arc<Recording>>>>,
    checking: Arc<AtomicU64>,
    input_lost: Arc<AtomicBool>,
    level: Arc<AtomicU8>,
}

impl Capture {
    pub fn begin(lane: &Lane, start: Start) -> Result<Self, String> {
        let (commands, command_rx) = mpsc::sync_channel(1);
        let (reply_tx, replies) = mpsc::channel();
        let activity = Activity::default();
        let pending_recording = Arc::new(Mutex::new(None));
        let checking = Arc::new(AtomicU64::new(0));
        let input_lost = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU8::new(0));
        let generation = start.generation;
        let deadline = Instant::now() + WAIT;
        let worker = Worker {
            commands: command_rx,
            replies: reply_tx,
            activity: activity.clone(),
            checking: checking.clone(),
            input_lost: input_lost.clone(),
            level: level.clone(),
        };
        let worker_recording = pending_recording.clone();
        lane.spawn(move || {
            let result = worker.run(start, deadline, &worker_recording);
            if let Err(error) = result
                && !worker.activity.cancelled()
            {
                let _ = worker.replies.send(Reply::Failed(error));
            }
        })?;
        Ok(Self {
            generation,
            deadline,
            replies,
            commands,
            activity,
            pending_recording,
            checking,
            input_lost,
            level,
        })
    }

    pub fn commit(&self) -> Result<(), String> {
        self.commands
            .try_send(Command::Commit)
            .map_err(|_| "麦克风准备已结束，请重新开始。".into())
    }

    pub fn stop(&self) -> Result<(), String> {
        match self.activity.0.compare_exchange(
            Activity::ACCEPTING,
            Activity::STOPPING,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {}
            Err(Activity::INTERRUPTED) => return Ok(()), // Consume the single pending terminal reply.
            Err(_) => return Err(CANCELLED.into()),
        }
        match self.commands.try_send(Command::Stop) {
            Ok(()) => Ok(()),
            Err(_) if self.activity.0.load(Ordering::Acquire) == Activity::INTERRUPTED => Ok(()),
            Err(_) => Err("录音线程已结束，请重新开始。".into()),
        }
    }

    pub fn take_finalized(&self) -> Result<Arc<Recording>, String> {
        self.pending_recording
            .lock()
            .expect("capture recording custody")
            .take()
            .ok_or_else(|| "录音已不可用。".into())
    }

    pub fn input_lost(&self) -> bool {
        self.input_lost.load(Ordering::Acquire)
    }

    pub fn checking(&self) -> Option<u64> {
        let check = self.checking.load(Ordering::Acquire);
        (check % 2 == 1).then_some(check)
    }

    pub fn level(&self) -> Option<u8> {
        self.level.load(Ordering::Relaxed).checked_sub(1)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.activity
            .0
            .store(Activity::CANCELLED, Ordering::Release);
        let recording = self
            .pending_recording
            .lock()
            .expect("capture recording custody")
            .take();
        if let Some(recording) = recording
            && let Err(error) = recording.discard_unfinished()
        {
            eprintln!("无法撤销未完成的录音缓存：{error}");
        }
    }
}

struct Worker {
    commands: Receiver<Command>,
    replies: mpsc::Sender<Reply>,
    activity: Activity,
    checking: Arc<AtomicU64>,
    input_lost: Arc<AtomicBool>,
    level: Arc<AtomicU8>,
}

impl Worker {
    fn finish_interruption(
        &self,
        mut recorder: Recorder,
        reason: String,
        deadline: Instant,
    ) -> Result<(), String> {
        self.activity.close_interrupted()?;
        let result = objc2::rc::autoreleasepool(|_| recorder.finalize_interrupted());
        let diagnostics = recorder.capture_diagnostics();
        drop(recorder);
        if self.activity.cancelled() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(RECOVERY_TIMEOUT.into());
        }
        result.map_err(|error| format!("{reason} {error} 请重新开始。"))?;
        self.replies
            .send(Reply::Interrupted {
                reason,
                diagnostics,
            })
            .map_err(|_| CANCELLED.into())
    }

    fn check(&self, deadline: Instant) -> Result<(), String> {
        if !self.activity.accepting() {
            Err(CANCELLED.into())
        } else if Instant::now() >= deadline {
            Err(START_TIMEOUT.into())
        } else {
            Ok(())
        }
    }

    fn run(
        &self,
        start: Start,
        deadline: Instant,
        pending_recording: &Mutex<Option<Arc<Recording>>>,
    ) -> Result<(), String> {
        let push = |reply| self.replies.send(reply).map_err(|_| CANCELLED.to_owned());
        let check = || self.check(deadline);
        check()?;
        let timer = super::StageTimer::start(Instant::now);
        let (route, input) = objc2::rc::autoreleasepool(|_| {
            Recorder::prepare_input(start.preferred_uid.as_deref(), &check)
        })?;
        let commit = || {
            push(Reply::Prepared)?;
            match self
                .commands
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Command::Commit) => check(),
                Err(RecvTimeoutError::Timeout) => Err(START_TIMEOUT.into()),
                _ => Err(CANCELLED.into()),
            }
        };
        let preferred_uid = start.preferred_uid.clone();
        let mut recorder = Recorder::new();
        let preview = objc2::rc::autoreleasepool(|_| {
            recorder.start_prepared(
                super::PreparedCapture {
                    route,
                    input,
                    started: timer,
                },
                start,
                self.activity.clone(),
                &check,
                commit,
                || {
                    let store = super::capture_storage().map_err(|error| error.to_string())?;
                    // The slot covers only local cache creation and registration.
                    // Quit can wait for those syscalls without waiting for AVFoundation.
                    let mut pending = pending_recording.lock().expect("capture recording custody");
                    check()?;
                    let recording = Arc::new(store.create_recording().map_err(|e| e.to_string())?);
                    *pending = Some(recording.clone());
                    Ok(recording)
                },
            )
        })?;
        push(Reply::Started {
            preview,
            lease: recorder.lease().expect("started recorder owns its lease"),
            route: recorder.route().unwrap().summary(),
        })?;
        loop {
            match self.commands.recv_timeout(AUDIO_POLL) {
                Ok(Command::Stop) if !self.activity.cancelled() => {
                    let deadline = Instant::now() + WAIT;
                    let loss = objc2::rc::autoreleasepool(|_| recorder.interrupted_at_stop());
                    if loss.is_some() {
                        self.input_lost.store(true, Ordering::Release);
                    }
                    if loss == Some(true) {
                        return self.finish_interruption(
                            recorder,
                            "麦克风在结束录音前已断开。".into(),
                            deadline,
                        );
                    }
                    let result = if loss.is_some() {
                        Err("无法确认结束录音时的麦克风；未转写本轮音频，请重新开始。".into())
                    } else {
                        objc2::rc::autoreleasepool(|_| recorder.finalize_capture()).map(|_| ())
                    };
                    let diagnostics = recorder.capture_diagnostics();
                    drop(recorder);
                    if self.activity.cancelled() {
                        return Ok(());
                    }
                    return push(Reply::Stopped {
                        result,
                        diagnostics,
                    });
                }
                Err(RecvTimeoutError::Timeout) => {}
                _ => return Ok(()),
            }
            if self.activity.cancelled() {
                return Ok(());
            }
            if self.activity.stopping() {
                continue;
            }
            self.checking.fetch_add(1, Ordering::Release);
            let deadline = Instant::now() + WAIT;
            let recover = || {
                if self.activity.cancelled() {
                    Err(CANCELLED.into())
                } else if self.activity.stopping() {
                    Err("麦克风在结束录音前已断开；未转写本轮音频，请重新开始。".into())
                } else if Instant::now() >= deadline {
                    Err(RECOVERY_TIMEOUT.into())
                } else {
                    Ok(())
                }
            };
            let recovered = objc2::rc::autoreleasepool(|_| {
                recorder.recover_input(
                    preferred_uid.as_deref(),
                    &recover,
                    || {
                        self.input_lost.store(true, Ordering::Release);
                        push(Reply::Recovering)
                    },
                    || self.activity.stopping(),
                )
            });
            if Instant::now() >= deadline && !self.activity.stopping() {
                return Err(RECOVERY_TIMEOUT.into());
            }
            match recovered? {
                Recovery::Unchanged => {}
                Recovery::Recovered => {
                    // Do not finalize a replacement that raced Stop or Cancel.
                    recover()?;
                    self.input_lost.store(false, Ordering::Release);
                    push(Reply::Route(recorder.route().unwrap().summary()))?;
                }
                Recovery::Interrupted(reason) => {
                    return self.finish_interruption(recorder, reason, deadline);
                }
            }
            self.checking.fetch_add(1, Ordering::Release);
            self.level.store(
                recorder.live_level().map_or(0, |n| n + 1),
                Ordering::Relaxed,
            );
        }
    }
}
