#[cfg(all(target_os = "macos", target_arch = "aarch64", debug_assertions))]
mod fixture {
    use core_foundation::{
        base::{CFType, TCFType},
        boolean::CFBoolean,
        dictionary::CFDictionary,
        number::CFNumber,
        string::CFString,
    };
    use std::ffi::CStr;
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SessionGetInfo(session: u32, actual: *mut u32, attributes: *mut u32) -> i32;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> core_foundation::dictionary::CFDictionaryRef;
    }

    fn sysctl(name: &CStr, buffer: &mut [u8]) -> Option<usize> {
        let mut count = buffer.len();
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut count,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0 && count <= buffer.len()).then_some(count)
    }

    fn attest(boot_id: &str, session_id: &str) -> Result<(), &'static str> {
        let mut vm = [0; 4];
        if sysctl(c"kern.hv_vmm_present", &mut vm) != Some(4) || i32::from_ne_bytes(vm) != 1 {
            return Err("isolated virtual guest required");
        }
        let mut boot = [0; 128];
        let length = sysctl(c"kern.bootsessionuuid", &mut boot).ok_or("missing guest boot ID")?;
        if CStr::from_bytes_with_nul(&boot[..length]).map(CStr::to_bytes) != Ok(boot_id.as_bytes())
        {
            return Err("guest boot ID mismatch");
        }
        let uid = unsafe { libc::geteuid() };
        let mut asid = 0;
        let mut attributes = 0;
        // AuthSession.h: callerSecuritySession, sessionHasGraphicAccess, sessionIsRemote.
        if uid == 0
            || uid != unsafe { libc::getuid() }
            || unsafe { SessionGetInfo(u32::MAX, &mut asid, &mut attributes) } != 0
            || session_id.parse() != Ok(asid)
            || attributes & 0x10 == 0
            || attributes & 0x1000 != 0
        {
            return Err("active local graphic audit session required");
        }
        let session = unsafe { CGSessionCopyCurrentDictionary() };
        if session.is_null() {
            return Err("Aqua session dictionary unavailable");
        }
        let session = unsafe { CFDictionary::<CFString, CFType>::wrap_under_create_rule(session) };
        let flag = |key| {
            session
                .find(CFString::new(key))
                .and_then(|v| v.downcast::<CFBoolean>())
                .map(bool::from)
        };
        let console_uid = session
            .find(CFString::new("kCGSSessionUserIDKey"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|v| v.to_i64());
        if console_uid != Some(i64::from(uid))
            || flag("kCGSSessionOnConsoleKey") != Some(true)
            || flag("kCGSessionLoginDoneKey") != Some(true)
            || session
                .find(CFString::new("CGSSessionScreenIsLocked"))
                .is_some()
                && flag("CGSSessionScreenIsLocked") != Some(false)
        {
            return Err("unlocked foreground Aqua console required");
        }
        let bundle = objc2_foundation::NSBundle::mainBundle();
        if bundle
            .bundleIdentifier()
            .map(|id| id.to_string())
            .as_deref()
            != Some("org.voiceport.item45-update-menu-fixture")
        {
            return Err("dedicated fixture bundle required");
        }
        let front = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication();
        if !front.is_some_and(|app| {
            app.isActive()
                && app.activationPolicy() == objc2_app_kit::NSApplicationActivationPolicy::Regular
                && app.bundleIdentifier().is_some_and(|id| {
                    !matches!(
                        id.to_string().as_str(),
                        "" | "com.apple.loginwindow"
                            | "com.apple.SecurityAgent"
                            | "com.apple.ScreenSaver.Engine"
                            | "com.apple.screensaver"
                    )
                })
        }) {
            return Err("no active regular synthetic Aqua desktop");
        }
        Ok(())
    }

    fn lease(path: &Path) -> std::io::Result<()> {
        // The launch instance may be outside the opener process group. Bound its lifetime independently.
        unsafe {
            libc::alarm(65);
        }
        let mut socket = UnixStream::connect(path)?;
        socket.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut byte = [0];
        socket.read_exact(&mut byte)?;
        if byte != [1] {
            return Err(std::io::Error::other("invalid supervisor acknowledgement"));
        }
        socket.set_read_timeout(None)?;
        std::thread::spawn(move || {
            let _ = socket.read_exact(&mut byte);
            unsafe {
                // Keep loss of the supervisor distinguishable from a fixture assertion.
                libc::_exit(74);
            }
        });
        Ok(())
    }

    fn exercise(presentation: Option<&Path>, output: &Path) -> Result<(), String> {
        let home = tempfile::Builder::new()
            .prefix("vp45-home.")
            .tempdir_in("/private/tmp")
            .map_err(|e| e.to_string())?;
        let applications = home.path().join("Applications");
        std::fs::create_dir_all(applications.join("VoicePort.app")).map_err(|e| e.to_string())?;
        objc2::rc::autoreleasepool(|_| {
            voiceport_macos::item45_acceptance(home.path(), presentation, output)
        })?;
        let entries = std::fs::read_dir(&applications)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if entries.len() != 1
            || entries[0].file_name() != "VoicePort.app"
            || std::fs::read_dir(entries[0].path())
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
        {
            return Err("unexpected change to synthetic install target".into());
        }
        home.close().map_err(|e| e.to_string())?;
        println!("synthetic target unchanged and removed");
        Ok(())
    }

    pub fn main() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let result = match args.as_slice() {
            [mode] if mode == "--detached" => exercise(None, Path::new("/private/tmp")),
            [mode, boot, session] if mode == "--attest-aqua" => {
                if let Err(error) = attest(boot, session) {
                    return refuse(error);
                }
                println!("isolated Aqua console and fixture bundle verified");
                return 0;
            }
            [mode, boot, session, library, output, socket] if mode == "--aqua" => {
                // Subscribe the supervisor before the fast refusal path, so it sees the true exit.
                if lease(Path::new(socket)).is_err() {
                    return refuse("private launch supervisor required");
                }
                if let Err(error) = attest(boot, session) {
                    return refuse(error);
                }
                println!("isolated Aqua console and fixture bundle verified");
                exercise(Some(Path::new(library)), Path::new(output))
            }
            _ => return refuse("explicit fixture mode required"),
        };
        match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("item45 fixture failed: {error}");
                1
            }
        }
    }

    fn refuse(error: &str) -> i32 {
        eprintln!("item45 refused before NSApplication: {error}");
        78
    }
}

fn main() {
    #[cfg(all(target_os = "macos", target_arch = "aarch64", debug_assertions))]
    std::process::exit(fixture::main());
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64", debug_assertions)))]
    std::process::exit(78);
}
