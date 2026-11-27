//! In-process debug fixture. No environment variable can enable this backend.
use super::*;
use crate::storage::Storage;
use objc2::AllocAnyThread;
use std::{
    cell::RefCell,
    collections::VecDeque,
    sync::{Weak, atomic::AtomicU32, mpsc},
};

thread_local! { static BACKEND: RefCell<Option<Backend>> = const { RefCell::new(None) }; }

pub(in crate::audio) fn current() -> Option<Backend> {
    BACKEND.with(|slot| slot.borrow().clone())
}

pub(in crate::audio) fn set_current(backend: Option<Backend>) {
    BACKEND.with(|slot| slot.replace(backend));
}

#[derive(Clone)]
pub(crate) struct Backend(Arc<State>);

struct State {
    pub storage: Storage,
    default: AtomicU32,
    missing: AtomicU32,
    authorized: AtomicBool,
    sample: AtomicU32,
    frames: AtomicU32,
    opens: AtomicU32,
    starts: AtomicU32,
    attached: AtomicU32,
    released: AtomicU32,
    open_gates: Mutex<VecDeque<mpsc::Receiver<bool>>>,
    release_gate: Mutex<Option<(mpsc::Receiver<bool>, mpsc::Sender<()>)>>,
    default_gate: QueryGate,
    final_id_gate: QueryGate,
    default_admission: Mutex<Weak<DefaultAdmission>>,
}

type QueryGate = Mutex<Option<(mpsc::Receiver<()>, mpsc::Sender<()>)>>;

fn block_query(gate: &QueryGate) -> (mpsc::SyncSender<()>, mpsc::Receiver<()>) {
    let (resume, next) = mpsc::sync_channel(1);
    let (entered, observed) = mpsc::channel();
    *gate.lock().unwrap() = Some((next, entered));
    (resume, observed)
}

fn pause_query(gate: &QueryGate) {
    let pending = gate.lock().unwrap().take();
    if let Some((resume, entered)) = pending {
        let _ = entered.send(());
        let _ = resume.recv();
    }
}

impl Backend {
    pub(crate) fn new(storage: Storage) -> Self {
        Self(Arc::new(State {
            storage,
            default: AtomicU32::new(1),
            missing: AtomicU32::new(0),
            authorized: AtomicBool::new(true),
            sample: AtomicU32::new(0.125_f32.to_bits()),
            frames: AtomicU32::new(1024),
            opens: AtomicU32::new(0),
            starts: AtomicU32::new(0),
            attached: AtomicU32::new(0),
            released: AtomicU32::new(0),
            open_gates: Mutex::default(),
            release_gate: Mutex::default(),
            default_gate: Mutex::default(),
            final_id_gate: Mutex::default(),
            default_admission: Mutex::default(),
        }))
    }

    pub(crate) fn block_next_open(&self) -> mpsc::SyncSender<bool> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.0.open_gates.lock().unwrap().push_back(rx);
        tx
    }

    pub(crate) fn block_next_release(&self) -> (mpsc::SyncSender<bool>, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::sync_channel(1);
        let (entered, observed) = mpsc::channel();
        *self.0.release_gate.lock().unwrap() = Some((rx, entered));
        (tx, observed)
    }

    pub(crate) fn block_next_default_query(&self) -> (mpsc::SyncSender<()>, mpsc::Receiver<()>) {
        block_query(&self.0.default_gate)
    }

    pub(crate) fn block_next_final_default_id(&self) -> (mpsc::SyncSender<()>, mpsc::Receiver<()>) {
        block_query(&self.0.final_id_gate)
    }

    pub(super) fn watch_default(&self, admission: &Arc<DefaultAdmission>) {
        *self.0.default_admission.lock().unwrap() = Arc::downgrade(admission);
    }

    pub(super) fn final_default_input_id(&self) -> u32 {
        let id = self.0.default.load(Ordering::Acquire);
        pause_query(&self.0.final_id_gate);
        id
    }

    // The property's new value can be readable before its notification arrives.
    pub(crate) fn notify_default_changed(&self) -> bool {
        let admission = self.0.default_admission.lock().unwrap().upgrade();
        if let Some(admission) = admission {
            admission.invalidate();
            true
        } else {
            false
        }
    }

    pub(crate) fn counts(&self) -> (u32, u32, u32, u32) {
        let read = |n: &AtomicU32| n.load(Ordering::Acquire);
        (
            read(&self.0.opens),
            read(&self.0.starts),
            read(&self.0.attached),
            read(&self.0.released),
        )
    }

    pub(crate) fn set_default(&self, id: u32) {
        self.0.default.store(id, Ordering::Release);
    }

    pub(crate) fn disconnect(&self, id: u32) {
        self.0.missing.store(id, Ordering::Release);
    }

    pub(crate) fn set_samples(&self, frames: u32, sample: f32) {
        assert!(frames <= 1024);
        self.0.sample.store(sample.to_bits(), Ordering::Release);
        self.0.frames.store(frames, Ordering::Release);
    }

    pub(crate) fn set_authorized(&self, allowed: bool) {
        self.0.authorized.store(allowed, Ordering::Release);
    }

    pub(crate) fn authorized(&self) -> bool {
        self.0.authorized.load(Ordering::Acquire)
    }

    pub(in crate::audio) fn storage(&self) -> &Storage {
        &self.0.storage
    }

    fn device(&self, id: u32) -> Option<Device> {
        if id == 0 || self.0.missing.load(Ordering::Acquire) == id {
            return None;
        }
        Some(Device {
            id,
            uid: format!("fixture-{id}"),
            name: format!("Synthetic input {id}"),
        })
    }

    pub(super) fn open<T>(
        &self,
        device: &Device,
        check: &impl Fn() -> Result<(), String>,
        prepare: &impl Fn(&AVAudioFormat) -> Result<T, String>,
    ) -> Result<(OpenInput, T), OpenError> {
        check()?;
        // Synthetic equivalent of inputNode returning after a blocked hardware
        // initialization. The same open/commit guards run before device use.
        let gate = self.0.open_gates.lock().unwrap().pop_front();
        self.0.opens.fetch_add(1, Ordering::Release);
        if gate.is_some_and(|gate| gate.recv() != Ok(true)) {
            return Err(OpenError::Unavailable(
                "Synthetic input initialization failed.".into(),
            ));
        }
        check()?;
        let sample_rate = if device.id == 1 { 48_000.0 } else { 16_000.0 };
        let (format, pcm) = unsafe {
            let format = AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                sample_rate,
                1,
            )
            .unwrap();
            let pcm = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &format,
                1024,
            )
            .unwrap();
            let frames = self.0.frames.load(Ordering::Acquire);
            pcm.setFrameLength(frames);
            let sample = f32::from_bits(self.0.sample.load(Ordering::Acquire));
            std::slice::from_raw_parts_mut((*pcm.floatChannelData()).as_ptr(), frames as usize)
                .fill(sample);
            (format, pcm)
        };
        let prepared = prepare(&format)?;
        check()?;
        self.0.starts.fetch_add(1, Ordering::Release);
        Ok((
            OpenInput {
                native: InputEngine::Synthetic(Input {
                    backend: self.clone(),
                    id: device.id,
                    pcm,
                }),
                format,
                gate: Arc::default(),
                default_watch: None,
                next_check: Instant::now(),
            },
            prepared,
        ))
    }
}

impl Inventory for Backend {
    fn inputs(&self) -> Result<Vec<Device>, String> {
        Ok([1, 2]
            .into_iter()
            .filter_map(|id| self.device(id))
            .collect())
    }

    fn default_candidate(&self) -> Result<Option<Device>, String> {
        // Match HAL: capture the ID before looking up the device details.
        let id = self.0.default.load(Ordering::Acquire);
        pause_query(&self.0.default_gate);
        Ok(self.device(id))
    }
}

pub(super) struct Input {
    pub backend: Backend,
    id: u32,
    pcm: Retained<AVAudioPCMBuffer>,
}

impl Input {
    pub(super) fn healthy(&self) -> bool {
        self.backend.device(self.id).is_some()
    }

    pub(super) fn attached(&self) {
        self.backend.0.attached.fetch_add(1, Ordering::Release);
    }

    pub(super) fn pulse(&self, gate: &TapGate) {
        if self.healthy() {
            gate.consume(&self.pcm);
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        let gate = self.backend.0.release_gate.lock().unwrap().take();
        if let Some((resume, entered)) = gate {
            let _ = entered.send(());
            let _ = resume.recv();
        }
        self.backend.0.released.fetch_add(1, Ordering::Release);
    }
}
