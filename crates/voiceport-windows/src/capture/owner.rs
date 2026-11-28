//! Lazy, bounded process owner for explicit capture requests.
//! Caller methods only change atomics, wake threads, or try a short result lock.
//! Only the supervisor joins; only release PLUS join permits a stopped result.
use super::{CleanupStatus, Error, Phase, Result, Session};
use crate::{
    Recording,
    executor::{OsSpawner, Spawner},
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle, Thread},
    time::Duration,
};
const TICK: Duration = Duration::from_millis(10);
const STOP: u8 = 1;
const CANCEL: u8 = 2;
const RECOVER: u8 = 4;

/// Trusted injection seam, not native storage/security evidence. Create the
/// thread-bound session INSIDE create. Command values and their destructors must
/// be inert/bounded (no backend, output, File, Root or Recording captured here).
/// A factory error must completely dispose partial resources before returning.
/// The owner treats factory panic as unknown, retaining process admission.
pub trait Factory: Send + 'static {
    type Session: CaptureSession;
    fn create(self) -> Result<Self::Session>;
}
/// Trusted synchronous owner seam. No Send requirement: all methods AND complete
/// destruction occur on the creating capture thread. Released must mean lawful
/// native release; cleanup_status must distinguish interrupted and retry stages.
pub trait CaptureSession: 'static {
    fn begin(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<Phase>;
    fn cancel(&mut self) -> Result<Phase>;
    fn poll(&mut self) -> Result<Phase>;
    fn phase(&self) -> Phase;
    fn error(&self) -> Option<Error>;
    fn cleanup_status(&self) -> CleanupStatus;
    fn take_recording(&mut self) -> Option<Recording>;
}
macro_rules! session_adapter {
    ($ty:ty) => {
        fn begin(&mut self) -> Result<()> {
            <$ty>::begin(self)
        }
        fn stop(&mut self) -> Result<Phase> {
            <$ty>::stop(self)
        }
        fn cancel(&mut self) -> Result<Phase> {
            <$ty>::cancel(self)
        }
        fn poll(&mut self) -> Result<Phase> {
            <$ty>::poll(self)
        }
        fn phase(&self) -> Phase {
            <$ty>::phase(self)
        }
        fn error(&self) -> Option<Error> {
            <$ty>::error(self)
        }
        fn cleanup_status(&self) -> CleanupStatus {
            <$ty>::cleanup_status(self)
        }
        fn take_recording(&mut self) -> Option<Recording> {
            <$ty>::take_recording(self)
        }
    };
}
impl<B: super::Backend + 'static, O: super::Output + 'static> CaptureSession for Session<B, O> {
    session_adapter!(Session<B, O>);
}
#[cfg(windows)]
impl CaptureSession for super::NativeSession {
    session_adapter!(super::NativeSession);
}

/// Inert native command. Root discovery, secure prepare and activation happen
/// only after explicit start, on the capture thread, under the private budget.
#[derive(Default)]
pub struct NativeFactory {
    language: Option<String>,
}
impl NativeFactory {
    pub fn new(language: Option<String>) -> Result<Self> {
        if language.as_ref().is_some_and(|s| s.len() > 64) {
            return Err(Error::Format);
        }
        Ok(Self { language })
    }
}
#[cfg(windows)]
impl Factory for NativeFactory {
    type Session = super::NativeSession;
    fn create(self) -> Result<Self::Session> {
        let root = crate::storage::Storage
            .open_or_create()
            .map_err(|_| Error::Write)?;
        super::NativeCapture.prepare(&root, self.language)
    }
}
#[cfg(not(windows))]
impl Factory for NativeFactory {
    type Session = Unsupported;
    fn create(self) -> Result<Self::Session> {
        let _ = self.language;
        Err(Error::Unsupported)
    }
}
#[cfg(not(windows))]
pub struct Unsupported;
#[cfg(not(windows))]
impl CaptureSession for Unsupported {
    fn begin(&mut self) -> Result<()> {
        Err(Error::Unsupported)
    }
    fn stop(&mut self) -> Result<Phase> {
        Err(Error::Unsupported)
    }
    fn cancel(&mut self) -> Result<Phase> {
        Err(Error::Unsupported)
    }
    fn poll(&mut self) -> Result<Phase> {
        Err(Error::Unsupported)
    }
    fn phase(&self) -> Phase {
        Phase::Released
    }
    fn error(&self) -> Option<Error> {
        Some(Error::Unsupported)
    }
    fn cleanup_status(&self) -> CleanupStatus {
        CleanupStatus::Released
    }
    fn take_recording(&mut self) -> Option<Recording> {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Idle,
    Starting,
    Running,
    Stopping,
    RecoveryRequired,
    Interrupted,
    Joining,
    Ready,
    Stopped,
    Unavailable,
}
fn status(n: u8) -> Status {
    match n {
        0 => Status::Idle,
        1 => Status::Starting,
        2 => Status::Running,
        3 => Status::Stopping,
        4 => Status::RecoveryRequired,
        5 => Status::Interrupted,
        6 => Status::Joining,
        7 => Status::Ready,
        8 => Status::Stopped,
        _ => Status::Unavailable,
    }
}
struct Shared {
    generation: u64,
    commands: AtomicU8,
    state: AtomicU8,
    invalid: AtomicBool,
    claimed: AtomicBool,
    release: AtomicBool,
    answer: Mutex<Option<Result<Recording>>>,
    worker: OnceLock<Thread>,
    supervisor: Thread,
    accepting: Arc<AtomicBool>,
    #[cfg(test)]
    recovery_parks: AtomicU64,
}
impl Shared {
    fn set(&self, state: Status) {
        self.state.store(state as u8, Ordering::Release);
    }
    fn wake(&self) {
        if let Some(t) = self.worker.get() {
            t.unpark();
        }
        self.supervisor.unpark();
    }
}
/// Non-cloneable UI handle. Construction and default do not initialize anything.
#[derive(Default)]
pub struct CaptureOwner {
    active: Option<Arc<Shared>>,
    closed: bool,
}
impl CaptureOwner {
    pub fn start<F: Factory>(&mut self, factory: F) -> Result<u64> {
        let domain = {
            let mut slot = DOMAIN.try_lock().map_err(|_| Error::Busy)?;
            slot.get_or_insert_with(|| Arc::new(Domain::default()))
                .clone()
        };
        self.start_in(&domain, factory, Arc::new(OsSpawner), Arc::new(OsSpawner))
    }
    fn start_in<F: Factory>(
        &mut self,
        domain: &Arc<Domain>,
        factory: F,
        supervisor: Arc<dyn Spawner>,
        worker: Arc<dyn Spawner>,
    ) -> Result<u64> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self
            .active
            .as_ref()
            .is_some_and(|s| status(s.state.load(Ordering::Acquire)) != Status::Stopped)
        {
            return Err(Error::Busy);
        }
        if domain
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::Busy);
        }
        let generation =
            match domain
                .generation
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            {
                Ok(n) => n + 1,
                Err(_) => {
                    domain.busy.store(false, Ordering::Release);
                    return Err(Error::Exhausted);
                }
            };
        let mut infrastructure = match domain.infrastructure.try_lock() {
            Ok(g) => g,
            Err(_) => {
                domain.busy.store(false, Ordering::Release);
                return Err(Error::Busy);
            }
        };
        if matches!(*infrastructure, Infrastructure::Cold) {
            let (sender, receiver) = mpsc::sync_channel(1);
            let accepting = Arc::new(AtomicBool::new(true));
            let guard = SupervisorReceiver {
                receiver,
                accepting: accepting.clone(),
            };
            // Only inert infrastructure captured. On Err Spawner guarantees no
            // started/retained work; on panic the outcome is unknown forever.
            let spawned = catch_unwind(AssertUnwindSafe(|| {
                supervisor.spawn(Box::new(move || supervise(guard)))
            }));
            match spawned {
                Ok(Ok(handle)) => {
                    *infrastructure = Infrastructure::Live(Runtime {
                        sender,
                        accepting,
                        handle,
                    })
                }
                Ok(Err(_)) => *infrastructure = Infrastructure::Failed,
                Err(p) => {
                    std::mem::forget(p);
                    *infrastructure = Infrastructure::Lost;
                }
            }
        }
        let runtime = match &*infrastructure {
            Infrastructure::Live(r) if r.accepting.load(Ordering::Acquire) => r,
            Infrastructure::Failed => {
                domain.busy.store(false, Ordering::Release);
                return Err(Error::Start);
            }
            _ => return Err(Error::Cleanup),
        };
        let shared = Arc::new(Shared {
            generation,
            commands: AtomicU8::new(0),
            state: AtomicU8::new(Status::Starting as u8),
            invalid: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
            release: AtomicBool::new(false),
            answer: Mutex::new(None),
            worker: OnceLock::new(),
            #[cfg(test)]
            recovery_parks: AtomicU64::new(0),
            supervisor: runtime.handle.thread().clone(),
            accepting: runtime.accepting.clone(),
        });
        let output = shared.clone();
        let work = Work {
            shared: shared.clone(),
            domain: domain.clone(),
            spawner: worker,
            run: Box::new(move || capture(factory, output)),
        };
        if let Err(e) = runtime.sender.try_send(work) {
            // Infrastructure loss/fullness is not a stop proof. Retain even the
            // inert command rather than dispose arbitrary trusted-seam fields on UI.
            std::mem::forget(e);
            shared.set(Status::Unavailable);
        }
        self.active = Some(shared);
        Ok(generation)
    }
    pub fn generation(&self) -> Option<u64> {
        self.active.as_ref().map(|s| s.generation)
    }
    /// Static snapshots never drive cleanup and never retry a release error.
    pub fn poll(&self) -> Status {
        let Some(s) = &self.active else {
            return Status::Idle;
        };
        let current = status(s.state.load(Ordering::Acquire));
        if current == Status::Stopped {
            return current;
        }
        if !s.accepting.load(Ordering::Acquire) {
            return Status::Unavailable;
        }
        if s.invalid.load(Ordering::Acquire) && current == Status::Ready {
            return Status::Stopping;
        }
        current
    }
    pub fn stop(&mut self) {
        if let Some(s) = &self.active {
            s.commands.fetch_or(STOP, Ordering::AcqRel);
            s.wake();
        }
    }
    pub fn cancel(&mut self) {
        if let Some(s) = &self.active {
            s.invalid.store(true, Ordering::Release); // invalidate before consumption
            s.commands.fetch_or(CANCEL, Ordering::AcqRel);
            s.wake();
        }
    }
    pub fn exit(&mut self) {
        self.closed = true;
        self.cancel();
    }
    /// NEW explicit authorization after observing a returned release failure.
    /// Ordinary Stop/Cancel/Drop and background ticks cannot set this command.
    pub fn recover(&mut self) -> Result<()> {
        let s = self.active.as_ref().ok_or(Error::Closed)?;
        if !s.accepting.load(Ordering::Acquire) {
            return Err(Error::Cleanup);
        }
        if s.state
            .compare_exchange(
                Status::RecoveryRequired as u8,
                Status::Stopping as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(Error::Cleanup);
        }
        s.commands.fetch_or(RECOVER, Ordering::AcqRel);
        s.wake();
        Ok(())
    }
    /// At most once, generation checked, and only AFTER genuine off-caller join.
    /// The claimed finalized Recording and its exact guards become caller owned.
    pub fn take_result(&mut self, generation: u64) -> Option<Result<Recording>> {
        let s = self.active.as_ref()?;
        if s.generation != generation
            || self.poll() != Status::Ready
            || s.invalid.load(Ordering::Acquire)
        {
            return None;
        }
        let mut slot = s.answer.try_lock().ok()?;
        if s.invalid.load(Ordering::Acquire) {
            return None;
        }
        let answer = slot.take()?;
        s.claimed.store(true, Ordering::Release);
        s.supervisor.unpark();
        Some(answer)
    }
    /// Only a proven no-start initialization failure can be explicitly reset.
    /// Lost/unknown supervisor or worker ownership can never use this recovery.
    pub fn recover_infrastructure() -> Result<()> {
        let domain = DOMAIN.try_lock().map_err(|_| Error::Busy)?.clone();
        domain.map_or(Ok(()), |d| d.recover_infrastructure())
    }
}
impl Drop for CaptureOwner {
    fn drop(&mut self) {
        self.cancel();
    }
}
#[derive(Default)]
struct Domain {
    busy: AtomicBool,
    generation: AtomicU64,
    infrastructure: Mutex<Infrastructure>,
}
impl Domain {
    fn recover_infrastructure(&self) -> Result<()> {
        if self.busy.load(Ordering::Acquire) {
            return Err(Error::Busy);
        }
        let mut i = self.infrastructure.try_lock().map_err(|_| Error::Busy)?;
        if matches!(*i, Infrastructure::Failed) {
            *i = Infrastructure::Cold;
            Ok(())
        } else {
            Err(Error::Cleanup)
        }
    }
}
static DOMAIN: Mutex<Option<Arc<Domain>>> = Mutex::new(None);
#[derive(Default)]
enum Infrastructure {
    #[default]
    Cold,
    Live(Runtime),
    Failed,
    Lost,
}
struct Runtime {
    sender: mpsc::SyncSender<Work>,
    accepting: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}
struct SupervisorReceiver {
    receiver: mpsc::Receiver<Work>,
    accepting: Arc<AtomicBool>,
}
impl Drop for SupervisorReceiver {
    fn drop(&mut self) {
        self.accepting.store(false, Ordering::Release);
    }
}
struct Work {
    shared: Arc<Shared>,
    domain: Arc<Domain>,
    spawner: Arc<dyn Spawner>,
    run: Box<dyn FnOnce() + Send>,
}
// Losing a supervisor or panicking join path cannot detach/drop ownership.
struct JoinOwner {
    handle: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    safe: bool,
}
impl Drop for JoinOwner {
    fn drop(&mut self) {
        if !self.safe {
            std::mem::forget(self.handle.take());
            let _ = Arc::into_raw(self.shared.clone());
        }
    }
}
fn supervise(queue: SupervisorReceiver) {
    while let Ok(work) = queue.receiver.recv() {
        let shared = work.shared.clone();
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| complete(work))) {
            std::mem::forget(p);
            shared.set(Status::Interrupted);
            let _ = Arc::into_raw(shared); // retains any unclaimed outcome and busy domain
        }
    }
}
fn complete(work: Work) {
    let Work {
        shared,
        domain,
        spawner,
        run,
    } = work;
    let mut owner = JoinOwner {
        handle: None,
        shared: shared.clone(),
        safe: false,
    };
    let spawned = catch_unwind(AssertUnwindSafe(|| spawner.spawn(run)));
    match spawned {
        Ok(Ok(handle)) => {
            owner.handle = Some(handle);
            // A body flag is not enough: the OS join covers full TLS teardown.
            let joined = owner.handle.take().expect("owned join handle").join();
            if let Err(p) = joined {
                std::mem::forget(p);
                shared.set(Status::Interrupted);
                return;
            }
            if !shared.release.load(Ordering::Acquire) {
                shared.set(Status::Interrupted);
                return;
            }
        }
        Ok(Err(_)) => {
            *shared.answer.lock().unwrap_or_else(|p| p.into_inner()) = Some(Err(Error::Start));
        }
        Err(p) => {
            std::mem::forget(p);
            shared.set(Status::Interrupted);
            return;
        }
    }
    owner.safe = true;
    shared.set(Status::Ready);
    while !shared.claimed.load(Ordering::Acquire) && !shared.invalid.load(Ordering::Acquire) {
        thread::park_timeout(TICK);
    }
    // Mutex never held across user destructors. If disposal panics, the outer
    // supervisor catch retains capacity and never admits another owner.
    let answer = shared
        .answer
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    drop(answer);
    shared.set(Status::Stopped);
    domain.busy.store(false, Ordering::Release);
}
// If anything unwinds, retain the thread-bound session without running implicit
// cleanup. The original Session also protects its own remaining fields.
struct SessionSlot<S>(Option<Box<S>>);
impl<S> Drop for SessionSlot<S> {
    fn drop(&mut self) {
        if let Some(s) = self.0.take() {
            let _ = Box::leak(s);
        }
    }
}
fn capture<F: Factory>(factory: F, shared: Arc<Shared>) {
    let _ = shared.worker.set(thread::current());
    let outcome = catch_unwind(AssertUnwindSafe(|| drive(factory, &shared)));
    match outcome {
        Ok(Some(answer)) => {
            *shared.answer.lock().unwrap_or_else(|p| p.into_inner()) = Some(answer);
            shared.set(Status::Joining);
            shared.release.store(true, Ordering::Release);
        }
        Ok(None) => shared.set(Status::Interrupted),
        Err(p) => {
            std::mem::forget(p);
            shared.set(Status::Interrupted);
        }
    }
}
fn drive<F: Factory>(factory: F, shared: &Shared) -> Option<Result<Recording>> {
    if shared.invalid.load(Ordering::Acquire) {
        drop(factory);
        return Some(Err(Error::Closed));
    }
    let session = match factory.create() {
        Ok(s) => s,
        Err(e) => return Some(Err(e)),
    };
    let mut slot = SessionSlot(Some(Box::new(session)));
    let s = slot.0.as_mut().expect("created session");
    let mut cancelled = shared.invalid.load(Ordering::Acquire);
    if cancelled {
        let _ = s.cancel();
    } else {
        let _ = s.begin();
    }
    let mut stop_sent = false;
    let mut parked = false;
    loop {
        match s.cleanup_status() {
            CleanupStatus::Released => break,
            CleanupStatus::Interrupted => {
                shared.set(Status::Interrupted);
                return None;
            }
            CleanupStatus::RecoveryRequired => {
                if !parked {
                    shared.set(Status::RecoveryRequired);
                    parked = true;
                }
                // Coalesced new explicit recovery is consumed BEFORE a call.
                // A second unknown stage needs another new authorization.
                if shared.commands.fetch_and(!RECOVER, Ordering::AcqRel) & RECOVER != 0 {
                    shared.set(Status::Stopping);
                    parked = false;
                    let _ = s.poll();
                } else {
                    #[cfg(test)]
                    shared.recovery_parks.fetch_add(1, Ordering::Release);
                    thread::park_timeout(TICK);
                }
                continue;
            }
            CleanupStatus::Progress => {}
        }
        if shared.invalid.load(Ordering::Acquire) && !cancelled {
            cancelled = true;
            let _ = s.cancel();
        } else if shared.commands.load(Ordering::Acquire) & STOP != 0
            && !stop_sent
            && s.phase() == Phase::Running
        {
            stop_sent = true;
            let _ = s.stop();
        } else {
            let _ = s.poll();
        }
        shared.set(if s.phase() == Phase::Running {
            Status::Running
        } else {
            Status::Stopping
        });
        if s.cleanup_status() == CleanupStatus::Progress {
            thread::park_timeout(TICK);
        }
    }
    let answer = if shared.invalid.load(Ordering::Acquire) {
        Err(Error::Closed)
    } else if let Some(e) = s.error() {
        Err(e)
    } else {
        s.take_recording().ok_or(Error::Empty)
    };
    // Includes complete field destructors. Only after normal return may capture
    // acknowledge release; publication still awaits the supervisor's real join.
    drop(slot.0.take());
    Some(answer)
}

#[cfg(test)]
#[path = "../../tests/capture/owner.rs"]
mod tests;
