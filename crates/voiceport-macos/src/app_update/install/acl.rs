//! Darwin extended ACL inspection on the directory that will be used.
use std::io;
use std::os::fd::RawFd;
use std::ptr::NonNull;

// ABI and constants from the macOS SDK's sys/acl.h.
const ACL_TYPE_EXTENDED: libc::c_uint = 0x100;
const ACL_FIRST_ENTRY: libc::c_int = 0;
const ACL_NEXT_ENTRY: libc::c_int = -1;
const ACL_EXTENDED_ALLOW: libc::c_uint = 1;
const ACL_EXTENDED_DENY: libc::c_uint = 2;

unsafe extern "C" {
    fn acl_get_fd_np(fd: RawFd, acl_type: libc::c_uint) -> *mut libc::c_void;
    fn acl_valid(acl: *mut libc::c_void) -> libc::c_int;
    fn acl_get_entry(
        acl: *mut libc::c_void,
        entry_id: libc::c_int,
        entry: *mut *mut libc::c_void,
    ) -> libc::c_int;
    fn acl_get_tag_type(entry: *mut libc::c_void, tag: *mut libc::c_uint) -> libc::c_int;
    fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
}

struct Acl(NonNull<libc::c_void>);

impl Drop for Acl {
    fn drop(&mut self) {
        unsafe { acl_free(self.0.as_ptr()) };
    }
}

impl Acl {
    fn validate(&self) -> io::Result<()> {
        if unsafe { acl_valid(self.0.as_ptr()) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn entry(&self, entry_id: libc::c_int) -> io::Result<Option<NonNull<libc::c_void>>> {
        let mut entry = std::ptr::null_mut();
        if unsafe { acl_get_entry(self.0.as_ptr(), entry_id, &mut entry) } == 0 {
            return NonNull::new(entry)
                .map(Some)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing ACL entry"));
        }
        let error = io::Error::last_os_error();
        // On a valid Darwin ACL, iteration past the last entry yields EINVAL.
        if error.raw_os_error() == Some(libc::EINVAL) {
            Ok(None)
        } else {
            Err(error)
        }
    }

    fn has_only_denials(&self) -> io::Result<bool> {
        self.validate()?;
        let mut entry_id = ACL_FIRST_ENTRY;
        while let Some(entry) = self.entry(entry_id)? {
            let mut tag = 0;
            if unsafe { acl_get_tag_type(entry.as_ptr(), &mut tag) } != 0 {
                return Err(io::Error::last_os_error());
            }
            match tag {
                ACL_EXTENDED_DENY => entry_id = ACL_NEXT_ENTRY,
                ACL_EXTENDED_ALLOW => return Ok(false),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "unknown ACL tag",
                    ));
                }
            }
        }
        Ok(true)
    }
}

fn read_acl(fd: RawFd) -> io::Result<Option<Acl>> {
    if let Some(acl) = NonNull::new(unsafe { acl_get_fd_np(fd, ACL_TYPE_EXTENDED) }) {
        return Ok(Some(Acl(acl)));
    }
    let error = io::Error::last_os_error();
    // For this open descriptor, Darwin uses ENOENT for an absent ACL.
    if error.raw_os_error() == Some(libc::ENOENT) {
        Ok(None)
    } else {
        Err(error)
    }
}

pub(super) fn has_no_acl_grants(fd: RawFd) -> io::Result<bool> {
    match read_acl(fd)? {
        Some(acl) => acl.has_only_denials(),
        None => Ok(true),
    }
}

pub(crate) fn has_no_extended_acl(fd: RawFd) -> io::Result<bool> {
    let Some(acl) = read_acl(fd)? else {
        return Ok(true);
    };
    acl.validate()?;
    Ok(acl.entry(ACL_FIRST_ENTRY)?.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn acl_from_text(text: *const libc::c_char) -> *mut libc::c_void;
    }

    fn memory_acl(text: &CStr) -> Acl {
        Acl(NonNull::new(unsafe { acl_from_text(text.as_ptr()) }).expect("valid ACL fixture"))
    }

    #[test]
    fn absent_and_deny_only_acls_are_eligible() {
        let directory = tempfile::tempdir_in("/private/tmp").unwrap();
        let descriptor = super::super::open_directory(directory.path()).unwrap();
        assert!(has_no_acl_grants(descriptor.as_raw_fd()).unwrap());
        let acl = memory_acl(
            c"!#acl 1\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:deny:delete\n",
        );
        assert!(acl.has_only_denials().unwrap());
    }

    #[test]
    fn a_grant_after_a_deny_still_disqualifies_the_acl() {
        let acl = memory_acl(c"!#acl 1\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:deny:delete\ngroup:ABCDEFAB-CDEF-ABCD-EFAB-CDEF0000000C:everyone:12:allow:write\n");
        assert!(!acl.has_only_denials().unwrap());
    }

    #[test]
    fn unreadable_acl_is_not_an_absent_acl() {
        assert_eq!(
            has_no_acl_grants(-1).unwrap_err().raw_os_error(),
            Some(libc::EBADF)
        );
    }
}
