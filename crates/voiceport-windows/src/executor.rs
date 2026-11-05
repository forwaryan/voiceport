//! One service worker plus one process-owned join thread; UI never joins.
use crate::Job;
use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};
static BUSY: AtomicBool = AtomicBool::new(false);
#[derive(Debug, Clone, Copy)]
pub struct ServiceFailure;
pub trait Service: Send + Sync + 'static {
    fn run(&self, job: &Job) -> Result<String, ServiceFailure>;
}
/// Trusted spawners return a handle covering all execution; on Err no work
/// may have started or be retained. Cleanup spawners execute the supplied loop.
pub trait Spawner: Send + Sync + 'static {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>>;
}
pub struct OsSpawner;
impl Spawner for OsSpawner {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>> {
        thread::Builder::new()
            .name("voiceport-owned-thread".into())
            .spawn(work)
    }
}
struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
type Answer = Result<String, ServiceFailure>;
struct Shared {
    result: Mutex<Option<Answer>>,
    stopped: AtomicBool,
    claimed: AtomicBool,
}
struct Work {
    job: Arc<Job>,
    shared: Arc<Shared>,
    service: Arc<dyn Service>,
    spawner: Arc<dyn Spawner>,
}
struct Reaper {
    sender: mpsc::SyncSender<Work>,
    thread: JoinHandle<()>,
}
static REAPER: OnceLock<Reaper> = OnceLock::new();
fn reaper(spawner: &dyn Spawner) -> io::Result<&'static Reaper> {
    if let Some(r) = REAPER.get() {
        if r.thread.is_finished() {
            return Err(io::Error::other("cleanup unavailable"));
        }
        return Ok(r);
    }
    let (sender, receiver) = mpsc::sync_channel::<Work>(1);
    let thread = spawner.spawn(Box::new(move || {
        while let Ok(work) = receiver.recv() {
            let shared = work.shared.clone();
            shared.claimed.store(true, Ordering::Release);
            let job = work.job;
            let output = shared.clone();
            let service = work.service;
            let spawned = catch_unwind(AssertUnwindSafe(|| {
                work.spawner.spawn(Box::new(move || {
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        if job.cancelled() {
                            Err(ServiceFailure)
                        } else {
                            service.run(&job)
                        }
                    }))
                    .unwrap_or(Err(ServiceFailure));
                    *output.result.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(result.and_then(|text| {
                            if text.len() <= 1_000_000 {
                                Ok(text)
                            } else {
                                Err(ServiceFailure)
                            }
                        }));
                }))
            }));
            match spawned {
                Ok(Ok(handle)) => {
                    // This is the ONLY join, off the UI thread, including TLS.
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        let result = handle.join();
                        if result.is_err() {
                            *shared.result.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(Err(ServiceFailure));
                        }
                        shared.stopped.store(true, Ordering::Release);
                        drop(result);
                    }));
                }
                _ => shared.stopped.store(true, Ordering::Release), // no worker started
            }
        }
    }))?;
    // Process-wide permit serializes initialization. The one idle cleanup thread
    // persists for the process; no per-request cleanup threads accumulate.
    assert!(REAPER.set(Reaper { sender, thread }).is_ok());
    Ok(REAPER.get().unwrap())
}
struct Running {
    job: Arc<Job>,
    shared: Arc<Shared>,
    _permit: Permit,
}
impl Running {
    fn stopped(&self) -> bool {
        if self.shared.stopped.load(Ordering::Acquire) {
            return true;
        }
        // If an injected cleanup loop returned without claiming this request, no
        // service worker could have started. Never use this to replace the reaper.
        REAPER.get().is_some_and(|r| r.thread.is_finished())
            && !self.shared.claimed.load(Ordering::Acquire)
    }
    fn finish(self) -> (Job, Answer) {
        let answer = self
            .shared
            .result
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap_or(Err(ServiceFailure));
        let job =
            Arc::try_unwrap(self.job).unwrap_or_else(|_| panic!("worker ownership invariant"));
        (job, answer)
    }
}
struct Orphans {
    receiver: mpsc::Receiver<Running>,
    waiting: Option<Running>,
}
type OrphanOwner = (mpsc::SyncSender<Running>, Mutex<Orphans>);
static ORPHAN: OnceLock<OrphanOwner> = OnceLock::new();
fn orphans() -> &'static OrphanOwner {
    ORPHAN.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel(1);
        (
            sender,
            Mutex::new(Orphans {
                receiver,
                waiting: None,
            }),
        )
    })
}
fn reap_orphan() {
    if let Ok(mut o) = orphans().1.try_lock() {
        if o.waiting.is_none() {
            o.waiting = o.receiver.try_recv().ok();
        }
        if o.waiting.as_ref().is_some_and(Running::stopped) {
            drop(o.waiting.take().unwrap().finish());
        }
    }
}
pub struct Executor {
    service: Arc<dyn Service>,
    spawner: Arc<dyn Spawner>,
    cleanup: Arc<dyn Spawner>,
    active: Option<Running>,
}
impl Executor {
    pub fn new(service: Arc<dyn Service>) -> Self {
        Self::with_spawner(service, Arc::new(OsSpawner))
    }
    pub fn with_spawner(service: Arc<dyn Service>, spawner: Arc<dyn Spawner>) -> Self {
        Self::with_spawners(service, spawner, Arc::new(OsSpawner))
    }
    pub fn with_spawners(
        service: Arc<dyn Service>,
        spawner: Arc<dyn Spawner>,
        cleanup: Arc<dyn Spawner>,
    ) -> Self {
        Self {
            service,
            spawner,
            cleanup,
            active: None,
        }
    }
    pub fn idle(&self) -> bool {
        self.active.is_none()
    }
    pub fn submit(&mut self, job: Job) -> Result<(), Job> {
        reap_orphan();
        if self.active.is_some()
            || BUSY
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(job);
        }
        let permit = Permit;
        let reaper = match reaper(self.cleanup.as_ref()) {
            Ok(r) => r,
            Err(_) => {
                drop(permit);
                return Err(job);
            }
        };
        let job = Arc::new(job);
        let shared = Arc::new(Shared {
            result: Mutex::new(None),
            stopped: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
        });
        let work = Work {
            job: job.clone(),
            shared: shared.clone(),
            service: self.service.clone(),
            spawner: self.spawner.clone(),
        };
        if let Err(error) = reaper.sender.try_send(work) {
            drop(error);
            shared.stopped.store(true, Ordering::Release);
        }
        self.active = Some(Running {
            job,
            shared,
            _permit: permit,
        });
        Ok(())
    }
    pub fn poll(&mut self) -> Option<(Job, Answer)> {
        if !self.active.as_ref()?.stopped() {
            return None;
        }
        Some(self.active.take().unwrap().finish())
    }
}
impl Drop for Executor {
    fn drop(&mut self) {
        if let Some(r) = self.active.take() {
            r.job.cancellation.cancel();
            if let Err(error) = orphans().0.try_send(r) {
                let (mpsc::TrySendError::Full(r) | mpsc::TrySendError::Disconnected(r)) = error;
                Box::leak(Box::new(r));
            }
        }
    }
}
#[cfg(feature = "service")]
pub trait RuntimeFactory: Send + Sync {
    fn build(&self) -> io::Result<tokio::runtime::Runtime>;
}
#[cfg(feature = "service")]
struct Runtime;
#[cfg(feature = "service")]
impl RuntimeFactory for Runtime {
    fn build(&self) -> io::Result<tokio::runtime::Runtime> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    }
}
#[cfg(feature = "service")]
pub struct CoreService {
    pub runtime: Arc<dyn RuntimeFactory>,
    pub auth: Option<std::path::PathBuf>,
}
#[cfg(feature = "service")]
impl Default for CoreService {
    fn default() -> Self {
        Self {
            runtime: Arc::new(Runtime),
            auth: None,
        }
    }
}
#[cfg(feature = "service")]
impl Service for CoreService {
    fn run(&self, job: &Job) -> Result<String, ServiceFailure> {
        if job.cancelled() {
            return Err(ServiceFailure);
        }
        let runtime = self.runtime.build().map_err(|_| ServiceFailure)?;
        let auth = self
            .auth
            .clone()
            .map(Ok)
            .unwrap_or_else(voiceport_core::auth::default_path)
            .map_err(|_| ServiceFailure)?;
        runtime
            .block_on(job.transcribe(auth))
            .map_err(|_| ServiceFailure)
    }
}
