//! Legacy waveIn, event/poll only: no Rust callbacks and no unsafe Send/Sync.
//! Off-UI, single-thread owner. Two initially queued fixed buffers cover the
//! ENTIRE maximum plus an overflow sentinel; no refill race can silently lose
//! samples within an accepted <=180s recording. It is not a streaming/resampler
//! backend. Driver/OS promises remain an actual Windows-runtime acceptance gate.
use super::*;
use crate::storage::{Pending, Root};
use std::{cell::UnsafeCell, marker::PhantomData, rc::Rc, sync::OnceLock};
use windows_sys::Win32::{Foundation::*, Media::Audio::*, System::Threading::*};
const HEADER_BYTES: u32 = std::mem::size_of::<WAVEHDR>() as u32;
#[repr(C, align(8))]
struct Header(UnsafeCell<WAVEHDR>);
const _: () = assert!(std::mem::offset_of!(WAVEHDR, dwFlags) % 4 == 0);
struct Buffer {
    header: Box<Header>,
    data: Box<[UnsafeCell<i16>]>,
    prepared: bool,
    queued: bool,
    offset: usize,
}
impl Buffer {
    fn new(n: usize, id: usize) -> Self {
        let data = (0..n)
            .map(|_| UnsafeCell::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let h = WAVEHDR {
            lpData: data.as_ptr().cast_mut().cast(),
            dwBufferLength: (n * 2) as u32,
            dwUser: id,
            ..Default::default()
        };
        Self {
            header: Box::new(Header(UnsafeCell::new(h))),
            data,
            prepared: false,
            queued: false,
            offset: 0,
        }
    }
    fn ptr(&self) -> *mut WAVEHDR {
        self.header.0.get()
    }
    fn flags(&self) -> u32 {
        // Header wrapper guarantees this field's alignment. WinMM documents
        // polling DONE; volatile avoids caching a driver-written completion bit.
        unsafe { std::ptr::read_volatile(std::ptr::addr_of!((*self.ptr()).dwFlags)) }
    }
}
#[derive(Default)]
struct WaveIn {
    device: HWAVEIN,
    event: HANDLE,
    buffers: Vec<Buffer>,
    id: u64,
    device_id: u32,
    sequence: u64,
    reset: bool,
    _thread: PhantomData<Rc<()>>,
}
pub struct NativeSession(Session<WaveIn, Pending>);
impl NativeSession {
    pub fn begin(&mut self) -> Result<()> {
        self.0.begin()
    }
    pub fn stop(&mut self) -> Result<Phase> {
        self.0.stop()
    }
    pub fn cancel(&mut self) -> Result<Phase> {
        self.0.cancel()
    }
    pub fn poll(&mut self) -> Result<Phase> {
        self.0.poll()
    }
    pub fn phase(&self) -> Phase {
        self.0.phase()
    }
    pub fn error(&self) -> Option<Error> {
        self.0.error()
    }
    pub fn cleanup_status(&self) -> CleanupStatus {
        self.0.cleanup_status()
    }
    pub fn take_recording(&mut self) -> Option<Recording> {
        self.0.take_recording()
    }
}
impl Output for Pending {
    fn append(&mut self, s: &[i16]) -> Result<()> {
        Pending::append(self, s).map_err(|_| Error::Write)
    }
    fn finish(self, l: Option<String>) -> Result<Recording> {
        self.finalize(l).map_err(|_| Error::Finalize)
    }
}
impl NativeCapture {
    /// Explicit storage IO on a positively held native Root. Device stays inert
    /// until begin, called only on its eventual dedicated owner thread.
    pub fn prepare(&self, root: &Root, language: Option<String>) -> Result<NativeSession> {
        static BUDGET: OnceLock<Budget> = OnceLock::new();
        let permit = BUDGET.get_or_init(Budget::default).acquire()?;
        let output = root.create().map_err(|_| Error::Write)?;
        Session::owned(WaveIn::default(), output, language, permit).map(NativeSession)
    }
}
fn format() -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: 1,
        nChannels: 1,
        nSamplesPerSec: 16_000,
        nAvgBytesPerSec: 32_000,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}
impl Backend for WaveIn {
    fn open(&mut self, id: u64, f: Format) -> Result<Device> {
        if !self.device.is_null() || !self.event.is_null() || f != PCM {
            return Err(Error::Open);
        }
        let user = usize::try_from(id).map_err(|_| Error::Exhausted)?;
        self.id = id;
        // Explicit calls only. No names/capability enumeration; exact PCM query
        // plus DIRECT forbids ACM conversion. Actual selected handle ID is bound.
        unsafe {
            if waveInOpen(
                std::ptr::null_mut(),
                WAVE_MAPPER,
                &format(),
                0,
                0,
                WAVE_FORMAT_DIRECT_QUERY,
            ) != 0
            {
                return Err(Error::Format);
            }
            self.event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
            if self.event.is_null() {
                return Err(Error::Open);
            }
            if waveInOpen(
                &mut self.device,
                WAVE_MAPPER,
                &format(),
                self.event as usize,
                0,
                CALLBACK_EVENT | WAVE_FORMAT_DIRECT,
            ) != 0
            {
                return Err(Error::Open);
            }
            if self.device.is_null()
                || waveInGetID(
                    self.device,
                    std::ptr::addr_of_mut!(self.device_id).cast_const(),
                ) != 0
            {
                return Err(Error::Open);
            }
        }
        self.buffers = vec![
            Buffer::new(MAX_SAMPLES, user),
            Buffer::new(FRAME_SAMPLES, user),
        ];
        Ok(Device {
            id: self.device_id,
            format: PCM,
        })
    }
    fn prepare(&mut self) -> Result<()> {
        if self.device.is_null() {
            return Err(Error::Prepare);
        }
        for b in &mut self.buffers {
            unsafe {
                if waveInPrepareHeader(self.device, b.ptr(), HEADER_BYTES) != 0 {
                    return Err(Error::Prepare);
                }
                b.prepared = true;
                // Mark potentially queued before call: even a failed add cannot
                // justify freeing an ambiguously accepted pointer before reset.
                b.queued = true;
                if waveInAddBuffer(self.device, b.ptr(), HEADER_BYTES) != 0 {
                    return Err(Error::Prepare);
                }
            }
        }
        Ok(())
    }
    fn start(&mut self) -> Result<()> {
        if self.device.is_null()
            || self.buffers.len() != 2
            || self.buffers.iter().any(|b| !b.queued)
        {
            return Err(Error::Start);
        }
        if unsafe { waveInStart(self.device) } != 0 {
            return Err(Error::Start);
        }
        Ok(())
    }
    fn read(&mut self, draining: bool, out: &mut Frame) -> Result<Read> {
        if self.device.is_null() {
            return Ok(Read::End);
        }
        if draining && !self.reset {
            return Err(Error::Read);
        }
        if !draining {
            let event = unsafe { WaitForSingleObject(self.event, 0) };
            if event != WAIT_OBJECT_0 && event != WAIT_TIMEOUT {
                return Err(Error::Read);
            }
            // The second fixed buffer is overflow-only. Once completed, audio
            // may already have been discarded by WinMM; NEVER accept that run.
            if self
                .buffers
                .get(1)
                .is_some_and(|b| b.flags() & WHDR_DONE != 0)
            {
                return Err(Error::Overflow);
            }
        }
        for b in &mut self.buffers {
            if !b.queued {
                continue;
            }
            let flags = b.flags();
            if flags & WHDR_DONE == 0 {
                // A failed AddBuffer may leave an unqueued prepared block.
                // Reset plus missing DONE is never audio success; independent
                // unprepare/close calls can still prove pointer release.
                if draining && flags & WHDR_INQUEUE == 0 {
                    return Err(Error::InvalidEvent);
                }
                return Ok(Read::Idle);
            }
            if flags & WHDR_INQUEUE != 0
                || flags & WHDR_PREPARED == 0
                || flags & !(WHDR_DONE | WHDR_PREPARED) != 0
            {
                return Err(Error::InvalidEvent);
            }
            // Only after DONE: WinMM promises not to write this unrequeued block.
            // Fields are packed: copy by value, never borrow packed field refs.
            let h = unsafe { std::ptr::read_unaligned(b.ptr()) };
            if h.dwUser != self.id as usize
                || h.lpData != b.data.as_ptr().cast_mut().cast()
                || h.dwBufferLength as usize != b.data.len() * 2
                || h.dwBytesRecorded > h.dwBufferLength
                || h.dwBytesRecorded % 2 != 0
            {
                return Err(Error::InvalidEvent);
            }
            let n = h.dwBytesRecorded as usize / 2;
            if b.offset > n {
                return Err(Error::InvalidEvent);
            }
            if b.offset == n {
                b.queued = false;
                continue;
            }
            let count = (n - b.offset).min(FRAME_SAMPLES);
            for i in 0..count {
                out.samples[i] = unsafe { *b.data[b.offset + i].get() };
            }
            b.offset += count;
            self.sequence = self.sequence.checked_add(1).ok_or(Error::Exhausted)?;
            out.session = self.id;
            out.device = self.device_id;
            out.sequence = self.sequence;
            out.format = PCM;
            out.bytes = count * 2;
            out.done = true;
            return Ok(Read::Data);
        }
        Ok(if draining { Read::End } else { Read::Idle })
    }
    fn stop(&mut self) -> Result<()> {
        if !self.device.is_null() && unsafe { waveInStop(self.device) } != 0 {
            return Err(Error::Stop);
        }
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        if !self.device.is_null() && unsafe { waveInReset(self.device) } != 0 {
            return Err(Error::Reset);
        }
        self.reset = true;
        Ok(())
    }
    fn unprepare(&mut self) -> Result<()> {
        for b in &mut self.buffers {
            if !b.prepared {
                continue;
            }
            if b.flags() & WHDR_INQUEUE != 0 {
                return Err(Error::Unprepare);
            }
            if unsafe { waveInUnprepareHeader(self.device, b.ptr(), HEADER_BYTES) } != 0 {
                return Err(Error::Unprepare);
            }
            b.prepared = false;
            b.queued = false;
        }
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        if self.buffers.iter().any(|b| b.prepared || b.queued) {
            return Err(Error::Close);
        }
        if !self.device.is_null() {
            if unsafe { waveInClose(self.device) } != 0 {
                return Err(Error::Close);
            }
            self.device = std::ptr::null_mut();
        }
        if !self.event.is_null() {
            if unsafe { CloseHandle(self.event) } == 0 {
                return Err(Error::Close);
            }
            self.event = std::ptr::null_mut();
        }
        Ok(())
    }
}
impl Drop for WaveIn {
    fn drop(&mut self) {
        // Defensive backstop for an embedding caller violating Backend lifetime:
        // never free driver pointers if a native close was not confirmed. The
        // Session native budget remains the normal bounded unknown-owner latch.
        if !self.device.is_null() {
            std::mem::forget(std::mem::take(&mut self.buffers));
        }
    }
}
