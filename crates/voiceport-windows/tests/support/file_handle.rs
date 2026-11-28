use std::fs::File;

#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, RawHandle};

#[cfg(unix)]
pub type Handle = RawFd;
#[cfg(windows)]
pub type Handle = RawHandle;

#[cfg(unix)]
unsafe extern "C" {
    fn fcntl(fd: RawFd, cmd: i32, ...) -> i32;
}

#[cfg(unix)]
pub fn handle(file: &File) -> RawFd {
    file.as_raw_fd()
}

#[cfg(windows)]
pub fn handle(file: &File) -> RawHandle {
    file.as_raw_handle()
}

#[cfg(unix)]
pub fn is_open(handle: RawFd) -> bool {
    unsafe { fcntl(handle, 1) >= 0 } // F_GETFD
}

#[cfg(windows)]
pub fn is_open(handle: RawHandle) -> bool {
    use windows_sys::Win32::Foundation::GetHandleInformation;

    let mut flags = 0;
    unsafe { GetHandleInformation(handle, &mut flags) != 0 }
}
