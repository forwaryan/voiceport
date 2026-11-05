//! A request has two owners: UI and covered cleanup work. Only explicit
//! request disposal or a completed OS join can relinquish cleanup ownership.
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
/// The actual returned thread covers supplied work, including execution in
/// its TLS destructors. On Err no work may have started or be retained.
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
}
struct RequestOwner {
    job: Option<Arc<Job>>,
    permit: Option<Arc<Permit>>,
    shared: Arc<Shared>,
    safe: bool,
}
impl Drop for RequestOwner {
    fn drop(&mut self) {
        if self.safe {
            // Release our actual ticket before acknowledging. An unclaimed request can
            // only reach this branch when its queue/closure owner truly disposes it.
            drop(self.job.take());
            drop(self.permit.take());
            self.shared.stopped.store(true, Ordering::Release);
        } else {
            // Unknown OS-join failure: retain the single ticket/permit indefinitely.
            // Never substitute a closure flag, timeout or cancellation for stop proof.
            Box::leak(Box::new((self.job.take(), self.permit.take())));
        }
    }
}
struct Work {
    owner: RequestOwner,
    service: Arc<dyn Service>,
    spawner: Arc<dyn Spawner>,
}
struct CleanupReceiver {
    receiver: mpsc::Receiver<Work>,
    accepting: Arc<AtomicBool>,
}
impl CleanupReceiver {
    fn recv(&self) -> Result<Work, mpsc::RecvError> {
        self.receiver.recv()
    }
}
impl Drop for CleanupReceiver {
    fn drop(&mut self) {
        self.accepting.store(false, Ordering::Release);
    }
}
struct Reaper {
    sender: mpsc::SyncSender<Work>,
    accepting: Arc<AtomicBool>,
    _thread: JoinHandle<()>,
}
static REAPER: OnceLock<Reaper> = OnceLock::new();
fn reaper(spawner: &dyn Spawner) -> io::Result<&'static Reaper> {
    if let Some(r) = REAPER.get() {
        if !r.accepting.load(Ordering::Acquire) {
            return Err(io::Error::other("cleanup unavailable"));
        }
        return Ok(r);
    }
    let (sender, receiver) = mpsc::sync_channel::<Work>(1);
    let accepting = Arc::new(AtomicBool::new(true));
    let queue = CleanupReceiver {
        receiver,
        accepting: accepting.clone(),
    };
    let thread = spawner.spawn(Box::new(move || {
        while let Ok(mut work) = queue.recv() {
            let shared = work.owner.shared.clone();
            let job = work.owner.job.as_ref().unwrap().clone();
            let output = shared.clone();
            let service = work.service;
            work.owner.safe = false;
            let spawned = catch_unwind(AssertUnwindSafe(|| {
                work.spawner.spawn(Box::new(move || {
                    let answer = catch_unwind(AssertUnwindSafe(|| {
                        if job.cancelled() {
                            Err(ServiceFailure)
                        } else {
                            service.run(&job)
                        }
                    }))
                    .unwrap_or(Err(ServiceFailure));
                    *output.result.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(answer.and_then(|text| {
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
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        let result = handle.join(); // The sole join, always on the covered cleanup thread.
                        work.owner.safe = true;
                        if result.is_err() {
                            *shared.result.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(Err(ServiceFailure));
                        }
                        drop(result);
                    }));
                }
                _ => work.owner.safe = true, // trusted spawn failure: no worker exists
            }
            // Owner drops after actual join (or proven no-start). UI may already be
            // gone; our Arc keeps File/permit until here and then releases both.
        }
    }))?;
    assert!(
        REAPER
            .set(Reaper {
                sender,
                accepting,
                _thread: thread
            })
            .is_ok()
    );
    Ok(REAPER.get().unwrap())
}
struct Running {
    job: Arc<Job>,
    shared: Arc<Shared>,
    _permit: Arc<Permit>,
}
impl Running {
    fn stopped(&self) -> bool {
        self.shared.stopped.load(Ordering::Acquire)
    }
    fn finish(self) -> (Job, Answer) {
        let result = self
            .shared
            .result
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap_or(Err(ServiceFailure));
        let job =
            Arc::try_unwrap(self.job).unwrap_or_else(|_| panic!("worker ownership invariant"));
        (job, result)
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
        if self.active.is_some()
            || BUSY
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(job);
        }
        let permit = Arc::new(Permit);
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
        });
        let work = Work {
            owner: RequestOwner {
                job: Some(job.clone()),
                permit: Some(permit.clone()),
                shared: shared.clone(),
                safe: true,
            },
            service: self.service.clone(),
            spawner: self.spawner.clone(),
        };
        // Returned command is explicitly disposed on send failure; its owner
        // acknowledges only after dropping the extra ticket, never before.
        if let Err(error) = reaper.sender.try_send(work) {
            drop(error);
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
            drop(r);
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
