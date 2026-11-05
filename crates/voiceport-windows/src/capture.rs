//! Explicit off-UI capture ownership. No device/storage IO at construction.
//! Native facade accepts only the reviewed private Root. Backend/Output are
//! trusted embedding seams: lifetime contracts, never native security evidence.
use crate::{Recording, wav::MAX_SAMPLES};
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
pub const FRAME_SAMPLES: usize = 4096;
pub const MAX_DRAIN_PACKETS: usize = MAX_SAMPLES.div_ceil(FRAME_SAMPLES) + 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
    pub bits: u16,
}
pub const PCM: Format = Format {
    rate: 16_000,
    channels: 1,
    bits: 16,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Busy,
    Exhausted,
    Closed,
    Open,
    Format,
    Prepare,
    Start,
    Read,
    InvalidEvent,
    Overflow,
    Write,
    Empty,
    Stop,
    Reset,
    Unprepare,
    Close,
    Finalize,
    Cleanup,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "Capture unsupported",
            Self::Busy => "Capture ownership unavailable",
            Self::Exhausted => "Capture identity exhausted",
            Self::Closed => "Capture session inactive",
            Self::Open => "Capture device unavailable",
            Self::Format => "Capture format unsupported",
            Self::Prepare => "Capture preparation failed",
            Self::Start => "Capture could not start",
            Self::Read => "Capture data unavailable",
            Self::InvalidEvent => "Capture data invalid",
            Self::Overflow => "Capture limit exceeded",
            Self::Write => "Capture output failed",
            Self::Empty => "Capture is empty",
            Self::Stop => "Capture stop failed",
            Self::Reset => "Capture release not confirmed",
            Self::Unprepare => "Capture buffer release not confirmed",
            Self::Close => "Capture device close not confirmed",
            Self::Finalize => "Capture could not finalize",
            Self::Cleanup => "Capture cleanup did not complete",
        })
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: u32,
    pub format: Format,
}
pub struct Frame {
    pub session: u64,
    pub device: u32,
    pub sequence: u64,
    pub format: Format,
    pub bytes: usize,
    pub done: bool,
    pub samples: [i16; FRAME_SAMPLES],
}
impl Default for Frame {
    fn default() -> Self {
        Self {
            session: 0,
            device: 0,
            sequence: 0,
            format: PCM,
            bytes: 0,
            done: false,
            samples: [0; FRAME_SAMPLES],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    Idle,
    Data,
    End,
}
/// Trusted owner backend. All calls (including Drop) may block: NEVER run on UI.
/// open Err must still retain any partially opened device in self. reset success
/// is a request acknowledgement, not permission to free headers. End only after
/// every queued buffer is returned. unprepare/close Ok must establish actual
/// release, not a timeout/result/Weak/is_finished observation. No deferred work
/// may use self after successful close and completed Drop. No callback panics.
pub trait Backend {
    fn open(&mut self, session: u64, format: Format) -> Result<Device>;
    fn prepare(&mut self) -> Result<()>;
    fn start(&mut self) -> Result<()>;
    fn read(&mut self, draining: bool, frame: &mut Frame) -> Result<Read>;
    fn stop(&mut self) -> Result<()>;
    fn reset(&mut self) -> Result<()>;
    fn unprepare(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}
/// The caller establishes secure output before begin. Implementations must
/// preserve the exact held object/guards on finish. Synthetic implementors prove
/// lifetime only; native production exclusively uses storage::Pending below.
pub trait Output {
    fn append(&mut self, samples: &[i16]) -> Result<()>;
    fn finish(self, language: Option<String>) -> Result<Recording>;
}
/// One bounded owner per budget. Native facade uses one process-wide budget;
/// isolated budgets are for independently owned synthetic backend contexts only.
#[derive(Clone, Default)]
pub struct Budget(Arc<AtomicBool>);
impl Budget {
    pub fn occupied(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
struct Permit(Arc<AtomicBool>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
impl Budget {
    fn acquire(&self) -> Result<Permit> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::Busy)?;
        Ok(Permit(self.0.clone()))
    }
}
static GENERATION: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Prepared,
    Running,
    Stopping,
    Released,
}
/// Read-only control-plane observation; never proves complete destruction/join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupStatus {
    Progress,
    RecoveryRequired,
    Interrupted,
    Released,
}
#[derive(Clone, Copy)]
enum Cleanup {
    Stop,
    Reset,
    Drain,
    Unprepare,
    Close,
    Done,
}
struct Owner<B: Backend, O: Output> {
    backend: Option<B>,
    output: Option<O>,
    permit: Option<Permit>,
    id: u64,
    device: Option<Device>,
    language: Option<String>,
    phase: Phase,
    cleanup: Cleanup,
    keep: bool,
    error: Option<Error>,
    samples: usize,
    sequence: u64,
    drained: usize,
    frame: Box<Frame>,
    unconfirmed: bool,
    // A returned release error needs a new explicit cleanup call. Unlike an
    // interrupted call, successful explicit recovery can clear this condition.
    explicit_recovery_required: bool,
    more_returned_data: bool,
}
// Field-level unwind protection: Session::drop itself may panic in caller
// backend/output code. A normal Box field would then destroy unconfirmed guards.
// Keep the whole allocation (including its addresses) until normal release.
struct OwnerSlot<B: Backend, O: Output>(Option<Box<Owner<B, O>>>);
impl<B: Backend, O: Output> OwnerSlot<B, O> {
    fn as_ref(&self) -> Option<&Owner<B, O>> {
        self.0.as_deref()
    }
    fn as_mut(&mut self) -> Option<&mut Owner<B, O>> {
        self.0.as_deref_mut()
    }
}
impl<B: Backend, O: Output> Drop for OwnerSlot<B, O> {
    fn drop(&mut self) {
        if let Some(owner) = self.0.take() {
            if owner.phase == Phase::Released && !owner.unconfirmed {
                drop(owner);
            } else {
                let _ = Box::leak(owner);
            }
        }
    }
}
// A panic propagates normally, but cannot reset this latch. Even if the caller
// catches it, later methods cannot retry/confirm the interrupted operation.
fn confirmed_call<T>(unconfirmed: &mut bool, call: impl FnOnce() -> T) -> T {
    *unconfirmed = true;
    let result = call();
    *unconfirmed = false;
    result
}
/// Synchronous off-UI session; not connected to native tray actions. Each poll
/// copies at most one running packet or four drain packets. Unknown cleanup
/// retains self; Drop deliberately leaks at most the occupied native owner, with
/// its process permit, rather than freeing driver pointers or allowing reuse.
pub struct Session<B: Backend, O: Output> {
    owner: OwnerSlot<B, O>,
    result: Option<Recording>,
}
impl<B: Backend, O: Output> Session<B, O> {
    pub fn with_backend(
        backend: B,
        output: O,
        language: Option<String>,
        budget: &Budget,
    ) -> Result<Self> {
        Self::owned(backend, output, language, budget.acquire()?)
    }
    fn owned(backend: B, output: O, language: Option<String>, permit: Permit) -> Result<Self> {
        let id = GENERATION
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| Error::Exhausted)?
            + 1;
        Ok(Self {
            owner: OwnerSlot(Some(Box::new(Owner {
                backend: Some(backend),
                output: Some(output),
                permit: Some(permit),
                id,
                device: None,
                language,
                phase: Phase::Prepared,
                cleanup: Cleanup::Stop,
                keep: false,
                error: None,
                samples: 0,
                sequence: 0,
                drained: 0,
                frame: Box::default(),
                unconfirmed: false,
                explicit_recovery_required: false,
                more_returned_data: false,
            }))),
            result: None,
        })
    }
    pub fn phase(&self) -> Phase {
        self.owner.as_ref().map_or(Phase::Released, |o| o.phase)
    }
    pub fn session_id(&self) -> Option<u64> {
        self.owner.as_ref().map(|o| o.id)
    }
    pub fn error(&self) -> Option<Error> {
        self.owner.as_ref().and_then(|o| o.error)
    }
    pub fn cleanup_status(&self) -> CleanupStatus {
        match self.owner.as_ref() {
            Some(o) if o.unconfirmed => CleanupStatus::Interrupted,
            Some(o) if o.explicit_recovery_required => CleanupStatus::RecoveryRequired,
            Some(o) if o.phase != Phase::Released => CleanupStatus::Progress,
            _ => CleanupStatus::Released,
        }
    }
    pub fn begin(&mut self) -> Result<()> {
        let o = self.owner.as_mut().ok_or(Error::Closed)?;
        if o.phase != Phase::Prepared {
            return Err(Error::Closed);
        }
        // Mark before any backend call: failure/panic cannot permit re-entry.
        o.phase = Phase::Running;
        o.confirmed()?;
        let r = confirmed_call(&mut o.unconfirmed, || {
            let b = o.backend.as_mut().ok_or(Error::Closed)?;
            let device = b.open(o.id, PCM).map_err(|_| Error::Open)?;
            o.device = Some(device);
            if device.format != PCM {
                return Err(Error::Format);
            }
            b.prepare().map_err(|_| Error::Prepare)?;
            b.start().map_err(|_| Error::Start)
        });
        if let Err(e) = r {
            o.fail(e);
            let _ = self.poll();
            return Err(e);
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<Phase> {
        let o = self.owner.as_mut().ok_or(Error::Closed)?;
        o.confirmed()?;
        if o.phase == Phase::Running {
            o.phase = Phase::Stopping;
            o.keep = true;
        } else if o.phase == Phase::Prepared {
            o.fail(Error::Empty);
        }
        self.poll()
    }
    pub fn cancel(&mut self) -> Result<Phase> {
        let o = self.owner.as_mut().ok_or(Error::Closed)?;
        o.keep = false; // invalidate before any stop/reset or queued data
        if o.phase != Phase::Released {
            o.phase = Phase::Stopping;
        }
        self.result = None;
        self.poll()
    }
    pub fn take_recording(&mut self) -> Option<Recording> {
        self.result.take()
    }
    pub fn poll(&mut self) -> Result<Phase> {
        let o = self.owner.as_mut().ok_or(Error::Closed)?;
        o.confirmed()?;
        match o.phase {
            Phase::Prepared | Phase::Released => return Ok(o.phase),
            Phase::Running => {
                match confirmed_call(&mut o.unconfirmed, || {
                    o.backend
                        .as_mut()
                        .ok_or(Error::Closed)?
                        .read(false, &mut o.frame)
                }) {
                    Ok(Read::Idle) => return Ok(Phase::Running),
                    Ok(Read::Data) => {
                        if let Err(e) = o.accept_frame() {
                            o.fail(e);
                        }
                    }
                    Ok(Read::End) => o.fail(Error::InvalidEvent),
                    Err(e) => o.fail(e),
                }
                if o.phase == Phase::Running && o.samples == MAX_SAMPLES {
                    o.phase = Phase::Stopping;
                    o.keep = true;
                }
                if o.phase == Phase::Running {
                    return Ok(o.phase);
                }
            }
            Phase::Stopping => {}
        }
        self.cleanup()
    }
    fn cleanup(&mut self) -> Result<Phase> {
        let o = self.owner.as_mut().ok_or(Error::Closed)?;
        o.confirmed()?;
        o.more_returned_data = false;
        if matches!(o.cleanup, Cleanup::Stop) {
            if confirmed_call(&mut o.unconfirmed, || {
                o.backend.as_mut().ok_or(Error::Closed)?.stop()
            })
            .is_err()
            {
                o.fail(Error::Stop);
            }
            o.cleanup = Cleanup::Reset;
        }
        if matches!(o.cleanup, Cleanup::Reset) {
            if confirmed_call(&mut o.unconfirmed, || {
                o.backend.as_mut().ok_or(Error::Closed)?.reset()
            })
            .is_err()
            {
                o.explicit_recovery_required = true;
                o.fail(Error::Reset);
                return Err(Error::Reset);
            }
            o.explicit_recovery_required = false;
            o.cleanup = Cleanup::Drain;
        }
        if matches!(o.cleanup, Cleanup::Drain) {
            for _ in 0..4 {
                match confirmed_call(&mut o.unconfirmed, || {
                    o.backend
                        .as_mut()
                        .ok_or(Error::Closed)?
                        .read(true, &mut o.frame)
                }) {
                    Ok(Read::End) => {
                        o.cleanup = Cleanup::Unprepare;
                        break;
                    }
                    Ok(Read::Idle) => return Ok(Phase::Stopping),
                    Ok(Read::Data) => {
                        o.drained += 1;
                        if o.drained > MAX_DRAIN_PACKETS {
                            o.fail(Error::Overflow);
                            o.cleanup = Cleanup::Unprepare;
                            break;
                        }
                        if let Err(e) = o.accept_frame() {
                            o.fail(e);
                        }
                    }
                    Err(e) => {
                        o.fail(e);
                        o.cleanup = Cleanup::Unprepare;
                        break;
                    }
                }
            }
            if matches!(o.cleanup, Cleanup::Drain) {
                o.more_returned_data = true; // four returned packets; not Idle
                return Ok(Phase::Stopping);
            }
        }
        if matches!(o.cleanup, Cleanup::Unprepare) {
            if confirmed_call(&mut o.unconfirmed, || {
                o.backend.as_mut().ok_or(Error::Closed)?.unprepare()
            })
            .is_err()
            {
                o.explicit_recovery_required = true;
                o.fail(Error::Unprepare);
                return Err(Error::Unprepare);
            }
            o.explicit_recovery_required = false;
            o.cleanup = Cleanup::Close;
        }
        if matches!(o.cleanup, Cleanup::Close) {
            if confirmed_call(&mut o.unconfirmed, || {
                o.backend.as_mut().ok_or(Error::Closed)?.close()
            })
            .is_err()
            {
                o.explicit_recovery_required = true;
                o.fail(Error::Close);
                return Err(Error::Close);
            }
            o.explicit_recovery_required = false;
            // This synchronous off-UI drop must RETURN before output/permit are
            // released. Weak expiry/close-result publication is not that proof.
            confirmed_call(&mut o.unconfirmed, || drop(o.backend.take()));
            o.cleanup = Cleanup::Done;
            let output = o.output.take().ok_or(Error::Closed)?;
            if o.keep && o.error.is_none() && o.samples != 0 {
                match confirmed_call(&mut o.unconfirmed, || output.finish(o.language.take())) {
                    Ok(r) => self.result = Some(r),
                    Err(_) => o.fail(Error::Finalize),
                }
            } else {
                if o.keep && o.error.is_none() {
                    o.fail(Error::Empty);
                }
                confirmed_call(&mut o.unconfirmed, || drop(output));
            }
            o.phase = Phase::Released;
            // Retain the last static error, but return process capacity only
            // after backend AND output cleanup bodies completed.
            drop(o.permit.take());
        }
        // Closed owner retains only static status, never an old release permit.
        match o.error {
            Some(e) => Err(e),
            None => Ok(o.phase),
        }
    }
}
impl<B: Backend, O: Output> Owner<B, O> {
    fn confirmed(&mut self) -> Result<()> {
        if self.unconfirmed {
            self.fail(Error::Cleanup);
            Err(Error::Cleanup)
        } else {
            Ok(())
        }
    }
    fn fail(&mut self, e: Error) {
        self.error.get_or_insert(e);
        self.keep = false;
        self.phase = Phase::Stopping;
    }
    fn accept_frame(&mut self) -> Result<()> {
        let f = &self.frame;
        if f.session != self.id
            || Some(f.device) != self.device.map(|d| d.id)
            || f.format != PCM
            || !f.done
            || f.bytes == 0
            || f.bytes > FRAME_SAMPLES * 2
            || !f.bytes.is_multiple_of(2)
            || self.sequence.checked_add(1) != Some(f.sequence)
        {
            return Err(Error::InvalidEvent);
        }
        self.sequence = f.sequence;
        let n = f.bytes / 2;
        if n > MAX_SAMPLES - self.samples {
            return Err(Error::Overflow);
        }
        self.samples += n;
        if self.error.is_none() && (self.phase == Phase::Running || self.keep) {
            confirmed_call(&mut self.unconfirmed, || {
                self.output
                    .as_mut()
                    .ok_or(Error::Closed)?
                    .append(&f.samples[..n])
            })
            .map_err(|_| Error::Write)?;
        }
        Ok(())
    }
}
impl<B: Backend, O: Output> Drop for Session<B, O> {
    fn drop(&mut self) {
        if self.phase() == Phase::Released || std::thread::panicking() {
            return;
        }
        // Do not turn disposal into an explicit retry of a release operation
        // that already returned an error. The first error alone is not proof of
        // this state: it may instead describe open/read/output or an old stage
        // that a subsequent explicit call has successfully recovered.
        if self
            .owner
            .as_ref()
            .is_some_and(|o| o.explicit_recovery_required)
        {
            return;
        }
        let _ = self.cancel();
        // The off-UI destructor may drain ALL bounded already-returned data.
        // Poll remains four packets. Never spin on Idle or retry an unknown
        // reset/unprepare/close. The global packet ceiling also bounds a bad
        // backend; the extra pass allows End after a full final quota.
        for _ in 0..=MAX_DRAIN_PACKETS.div_ceil(4) {
            if !self.owner.as_ref().is_some_and(|o| {
                o.phase == Phase::Stopping && !o.unconfirmed && o.more_returned_data
            }) {
                break;
            }
            let _ = self.poll();
        }
        // OwnerSlot's Drop, including unwind from ANY call above, retains an
        // unconfirmed owner/permit. Normal released owners are actually freed.
    }
}
#[derive(Default)]
pub struct NativeCapture;
#[cfg(not(windows))]
pub struct NativeSession;
#[cfg(not(windows))]
impl NativeCapture {
    pub fn prepare(&self, _: &crate::storage::Root, _: Option<String>) -> Result<NativeSession> {
        Err(Error::Unsupported)
    }
}
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub use native::NativeSession;
pub mod owner;
