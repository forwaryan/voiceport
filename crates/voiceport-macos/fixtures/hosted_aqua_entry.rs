//! Private Aqua authorization and LaunchServices ownership handshake.
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::CFString,
};
use objc2_foundation::NSBundle;
use std::{
    ffi::CStr,
    io::{self, Read},
    os::unix::{fs::MetadataExt, net::UnixStream},
};

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SessionGetInfo(session: u32, session_id: *mut u32, attributes: *mut u32) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

struct ConsoleSession {
    uid: u32,
    session_id: u32,
}

impl ConsoleSession {
    fn authorize(boot_id: &str, session_id: &str, bundle_id: &str) -> Result<Self, &'static str> {
        let mut vmm = [0; size_of::<libc::c_int>()];
        if read_sysctl(c"kern.hv_vmm_present", &mut vmm) != Some(vmm.len())
            || libc::c_int::from_ne_bytes(vmm) != 1
        {
            return Err("macOS virtual guest was not detected");
        }
        let mut boot = [0; 128];
        let uuid = read_sysctl(c"kern.bootsessionuuid", &mut boot)
            .and_then(|len| CStr::from_bytes_with_nul(&boot[..len]).ok());
        if boot_id.is_empty() || uuid.map(CStr::to_bytes) != Some(boot_id.as_bytes()) {
            return Err("guest boot UUID does not match the authorization");
        }
        let session_id = session_id.parse().map_err(|_| "invalid Aqua session ID")?;
        let console = Self {
            uid: unsafe { libc::geteuid() },
            session_id,
        };
        if !console.is_current() {
            return Err("the caller is outside the authorized active Aqua console session");
        }
        if NSBundle::mainBundle()
            .bundleIdentifier()
            .is_none_or(|id| id.to_string() != bundle_id)
        {
            return Err("the executable is outside the dedicated synthetic fixture bundle");
        }
        Ok(console)
    }

    fn is_current(&self) -> bool {
        // AuthSession.h: callerSecuritySession, sessionHasGraphicAccess and sessionIsRemote.
        const CALLER_SESSION: u32 = u32::MAX;
        const GRAPHIC_ACCESS: u32 = 0x0010;
        const REMOTE: u32 = 0x1000;
        let mut id = 0;
        let mut attributes = 0;
        let status = unsafe { SessionGetInfo(CALLER_SESSION, &mut id, &mut attributes) };
        if self.uid == 0
            || unsafe { libc::getuid() } != self.uid
            || status != 0
            || id != self.session_id
            || attributes & (GRAPHIC_ACCESS | REMOTE) != GRAPHIC_ACCESS
            || std::fs::metadata("/dev/console").map(|m| m.uid()).ok() != Some(self.uid)
        {
            return false;
        }
        let dictionary = unsafe { CGSessionCopyCurrentDictionary() };
        if dictionary.is_null() {
            return false;
        }
        // CGSession.h uses CFString keys. kCGSessionOnConsoleKey expands to
        // kCGSSessionOnConsoleKey. CGSessionCopyCurrentDictionary returns an owned value.
        let dictionary =
            unsafe { CFDictionary::<CFString, CFType>::wrap_under_create_rule(dictionary) };
        let session_uid = dictionary
            .find(CFString::new("kCGSSessionUserIDKey"))
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|number| number.to_i64());
        let is_true = |key| {
            dictionary
                .find(CFString::new(key))
                .and_then(|value| value.downcast::<CFBoolean>())
                .is_some_and(bool::from)
        };
        let unlocked = dictionary
            .find(CFString::new("CGSSessionScreenIsLocked"))
            .is_none_or(|value| {
                value
                    .downcast::<CFBoolean>()
                    .is_some_and(|flag| !bool::from(flag))
            });
        session_uid == Some(i64::from(self.uid))
            && is_true("kCGSSessionOnConsoleKey")
            && is_true("kCGSessionLoginDoneKey")
            && unlocked
    }
}

fn read_sysctl(name: &CStr, buffer: &mut [u8]) -> Option<usize> {
    let mut len = buffer.len();
    // The two read-only queries need no child processes outside the runner's group.
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && len <= buffer.len()).then_some(len)
}

fn refuse(reason: &str) -> ! {
    eprintln!("native-routes: refused before AppKit ({reason})");
    std::process::exit(78);
}

fn watch_launcher(path: &str, seconds: u32) -> io::Result<()> {
    // The app may belong to LaunchServices rather than the wrapper's process
    // group. Give it a hard deadline and exit on loss of this run's socket.
    unsafe {
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        libc::alarm(seconds);
    }
    let mut stream = UnixStream::connect(path)?;
    let mut byte = [0];
    stream.read_exact(&mut byte)?;
    if byte != [1] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid launch acknowledgement",
        ));
    }
    std::thread::Builder::new()
        .spawn(move || {
            let _ = stream.read_exact(&mut byte);
            // Exit only this process, even if its AppKit main thread is blocked.
            unsafe { libc::_exit(1) };
        })
        .map(drop)
}

pub fn enter(boot: &str, aqua: &str, launch: &[String], bundle_id: &str, seconds: u32) {
    let [mode, path] = launch else {
        refuse("owned launch supervisor required")
    };
    if mode != "--launch-socket" || watch_launcher(path, seconds).is_err() {
        refuse("owned launch supervisor unavailable");
    }
    if let Err(reason) = ConsoleSession::authorize(boot, aqua, bundle_id) {
        refuse(reason);
    }
}
