//! Windows-only candidate foundation; compiler checked, not NTFS-runtime accepted.
//! All names descend from a bounded, audited fixed-NTFS ancestor chain. Retained
//! read handles deny write/delete sharing; every component is opened as an object
//! (OPEN_REPARSE_POINT) and checked by handle. Private root/file DACLs admit only
//! the process user. Same-user, administrator and preexisting privileged handles
//! are outside the threat boundary (see docs/WINDOWS.md). Never scan or delete.
//!
//! Win32 out-pointers live until each call returns. LocalAlloc security results,
//! token handles and CreateFile handles have distinct RAII owners; borrowed SIDs
//! and ACEs never escape the owning security descriptor. Only native handles
//! returned successfully by CreateFile are converted to File. No raw pointer or
//! GUI object crosses threads: Recording retains Arc<Lease> of ordinary Files.
use super::*;
use crate::{Recording, wav::Wav};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle},
    },
    path::{Component, PathBuf},
    sync::Arc,
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Cryptography::*, *},
    Storage::FileSystem::*,
    System::{Com::CoTaskMemFree, Threading::*},
    UI::Shell::*,
};
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
fn handle(file: &File) -> HANDLE {
    file.as_raw_handle()
}
struct Kernel(HANDLE);
impl Drop for Kernel {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Local(*mut core::ffi::c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct Security {
    sid: Vec<u32>,
    _descriptor: Local,
    attributes: SECURITY_ATTRIBUTES,
}
impl Security {
    fn current() -> Result<Self> {
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(Error::Unavailable);
            }
            let _token = Kernel(token);
            let mut size = 0;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut size);
            if size == 0 || size > 4096 {
                return Err(Error::Unavailable);
            }
            let mut buf = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
            if GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), size, &mut size) == 0
            {
                return Err(Error::Unavailable);
            }
            let user = &*buf.as_ptr().cast::<TOKEN_USER>();
            if IsValidSid(user.User.Sid) == 0 {
                return Err(Error::Unavailable);
            }
            let len = GetLengthSid(user.User.Sid);
            if len == 0 || len > 68 {
                return Err(Error::Unavailable);
            }
            let mut sid = vec![0u32; (len as usize).div_ceil(4)];
            if CopySid(len, sid.as_mut_ptr().cast(), user.User.Sid) == 0 {
                return Err(Error::Unavailable);
            }
            let mut text = std::ptr::null_mut();
            if ConvertSidToStringSidW(sid.as_mut_ptr().cast(), &mut text) == 0 {
                return Err(Error::Unavailable);
            }
            let _text = Local(text.cast());
            let mut n = 0;
            while n < 184 && *text.add(n) != 0 {
                n += 1;
            }
            if n == 184 {
                return Err(Error::Unavailable);
            }
            let sidtext = String::from_utf16(std::slice::from_raw_parts(text, n))
                .map_err(|_| Error::Unavailable)?;
            let sddl = wide(std::ffi::OsStr::new(&format!(
                "O:{sidtext}D:P(A;;FA;;;{sidtext})"
            )));
            let mut desc = std::ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut desc,
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(Error::Unavailable);
            }
            Ok(Self {
                sid,
                _descriptor: Local(desc),
                attributes: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: desc,
                    bInheritHandle: 0,
                },
            })
        }
    }
    fn trusted(&self, sid: PSID) -> bool {
        unsafe {
            if sid.is_null() || IsValidSid(sid) == 0 {
                return false;
            }
            if EqualSid(sid, self.sid.as_ptr().cast_mut().cast()) != 0
                || IsWellKnownSid(sid, WinLocalSystemSid) != 0
                || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
            {
                return true;
            }
            let mut text = std::ptr::null_mut();
            if ConvertSidToStringSidW(sid, &mut text) == 0 {
                return false;
            }
            let _text = Local(text.cast());
            let mut n = 0;
            while n < 184 && *text.add(n) != 0 {
                n += 1;
            }
            if n == 184 {
                return false;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(text, n))
                == "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
        }
    }
    fn verify_ancestor(&self, file: &File) -> Result<()> {
        unsafe {
            let mut owner = std::ptr::null_mut();
            let mut dacl = std::ptr::null_mut();
            let mut desc = std::ptr::null_mut();
            if GetSecurityInfo(
                handle(file),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut desc,
            ) != 0
            {
                return Err(Error::UnsafeObject);
            }
            let _desc = Local(desc);
            if !self.trusted(owner) || dacl.is_null() || IsValidAcl(dacl) == 0 {
                return Err(Error::UnsafeObject);
            }
            let mut size: ACL_SIZE_INFORMATION = std::mem::zeroed();
            if GetAclInformation(
                dacl,
                (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of_val(&size) as u32,
                AclSizeInformation,
            ) == 0
                || size.AceCount > 64
            {
                return Err(Error::UnsafeObject);
            }
            for i in 0..size.AceCount {
                let mut ace = std::ptr::null_mut();
                if GetAce(dacl, i, &mut ace) == 0 {
                    return Err(Error::UnsafeObject);
                }
                let head = &*ace.cast::<ACE_HEADER>();
                if head.AceFlags & INHERIT_ONLY_ACE as u8 != 0 {
                    continue;
                }
                if head.AceType == 1 {
                    continue;
                }
                if head.AceType != 0 || (head.AceSize as usize) < 16 {
                    return Err(Error::UnsafeObject);
                }
                let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                let sid: PSID = (&allowed.SidStart as *const u32).cast_mut().cast();
                // SID header is eight bytes; bound its subauthority count before
                // asking Win32 to inspect it. Only a basic, exact-size ACE is accepted.
                let count = *sid.cast::<u8>().add(1) as u32;
                if count > 15 || head.AceSize as u32 != 16 + 4 * count || IsValidSid(sid) == 0 {
                    return Err(Error::UnsafeObject);
                }
                let dangerous = GENERIC_ALL
                    | GENERIC_WRITE
                    | FILE_WRITE_DATA
                    | FILE_WRITE_EA
                    | FILE_WRITE_ATTRIBUTES
                    | FILE_DELETE_CHILD
                    | DELETE
                    | WRITE_DAC
                    | WRITE_OWNER;
                if allowed.Mask & dangerous != 0 && !self.trusted(sid) {
                    return Err(Error::UnsafeObject);
                }
            }
            info(file, true)?;
            Ok(())
        }
    }
    fn verify(&self, file: &File) -> Result<()> {
        unsafe {
            let mut owner = std::ptr::null_mut();
            let mut dacl = std::ptr::null_mut();
            let mut desc = std::ptr::null_mut();
            if GetSecurityInfo(
                handle(file),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut desc,
            ) != 0
            {
                return Err(Error::UnsafeObject);
            }
            let _desc = Local(desc);
            if owner.is_null()
                || IsValidSid(owner) == 0
                || EqualSid(owner, self.sid.as_ptr().cast_mut().cast()) == 0
                || dacl.is_null()
                || IsValidAcl(dacl) == 0
            {
                return Err(Error::UnsafeObject);
            }
            let mut control = 0;
            let mut revision = 0;
            if GetSecurityDescriptorControl(desc, &mut control, &mut revision) == 0
                || control & SE_DACL_PROTECTED == 0
                || control & SE_DACL_PRESENT == 0
            {
                return Err(Error::UnsafeObject);
            }
            let mut info: ACL_SIZE_INFORMATION = std::mem::zeroed();
            if GetAclInformation(
                dacl,
                (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
                AclSizeInformation,
            ) == 0
                || info.AceCount != 1
            {
                return Err(Error::UnsafeObject);
            }
            let mut ace = std::ptr::null_mut();
            if GetAce(dacl, 0, &mut ace) == 0 {
                return Err(Error::UnsafeObject);
            }
            let head = &*ace.cast::<ACE_HEADER>();
            if head.AceSize as u32 != 8 + GetLengthSid(self.sid.as_ptr().cast_mut().cast()) {
                return Err(Error::UnsafeObject);
            }
            let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
            if allowed.Header.AceType != 0 /* ACCESS_ALLOWED_ACE_TYPE, winnt.h */
                || allowed.Header.AceFlags != 0
                || allowed.Mask != FILE_ALL_ACCESS
                || EqualSid(
                    (&allowed.SidStart as *const u32).cast_mut().cast(),
                    self.sid.as_ptr().cast_mut().cast(),
                ) == 0
            {
                return Err(Error::UnsafeObject);
            }
            Ok(())
        }
    }
}
fn info(file: &File, directory: bool) -> Result<BY_HANDLE_FILE_INFORMATION> {
    unsafe {
        let mut i = std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>();
        if GetFileType(handle(file)) != FILE_TYPE_DISK
            || GetFileInformationByHandle(handle(file), &mut i) == 0
            || i.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (i.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || (!directory && i.nNumberOfLinks != 1)
        {
            return Err(Error::UnsafeObject);
        }
        Ok(i)
    }
}
fn open(
    path: &Path,
    create: bool,
    write: bool,
    security: Option<&Security>,
    dir: bool,
) -> Result<File> {
    unsafe {
        let name = wide(path.as_os_str());
        let access = GENERIC_READ | READ_CONTROL | if write { GENERIC_WRITE } else { 0 };
        let h = CreateFileW(
            name.as_ptr(),
            access,
            FILE_SHARE_READ,
            security.map_or(std::ptr::null(), |s| &s.attributes),
            if create { CREATE_NEW } else { OPEN_EXISTING },
            FILE_FLAG_OPEN_REPARSE_POINT
                | if dir {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    FILE_ATTRIBUTE_NORMAL
                },
            std::ptr::null_mut(),
        );
        if h == INVALID_HANDLE_VALUE {
            return Err(if GetLastError() == ERROR_FILE_EXISTS {
                Error::Collision
            } else {
                Error::Unavailable
            });
        }
        let f = File::from_raw_handle(h);
        info(&f, dir)?;
        Ok(f)
    }
}
struct Lease {
    _chain: Vec<File>,
    _marker: File,
}
pub struct Root {
    path: PathBuf,
    lease: Arc<Lease>,
}
pub struct Pending {
    path: PathBuf,
    lease: Arc<Lease>,
    writer: Wav<File>,
    identity: (u32, u32, u32),
    failed: bool,
}
impl Storage {
    pub fn open_or_create(&self) -> Result<Root> {
        unsafe {
            let mut raw = std::ptr::null_mut();
            if SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, std::ptr::null_mut(), &mut raw) < 0 {
                return Err(Error::Unavailable);
            }
            if raw.is_null() {
                return Err(Error::Unavailable);
            }
            let mut n = 0;
            while n < 240 && *raw.add(n) != 0 {
                n += 1;
            }
            let path = if n == 240 {
                Err(Error::Unsupported)
            } else {
                Ok(PathBuf::from(std::ffi::OsString::from_wide(
                    std::slice::from_raw_parts(raw, n),
                )))
            };
            CoTaskMemFree(raw.cast());
            Root::under_owned_parent(&path?)
        }
    }
}
impl Root {
    /// Explicit IO: parent must be positively owned by the embedding caller.
    /// Native owner/ACL/volume/reparse checks still apply; no permission repair.
    /// Production uses KnownFolder LocalAppData. QA only invokes this on Windows
    /// under a newly owned synthetic parent, never actual user data.
    pub fn under_owned_parent(parent: &Path) -> Result<Self> {
        validate_local_path(parent)?;
        let security = Security::current()?;
        let mut chain = Vec::new();
        let mut prefix = PathBuf::new();
        for component in parent.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    prefix.push(component.as_os_str());
                    if matches!(component, Component::RootDir) {
                        let f = open(&prefix, false, false, None, true)?;
                        security.verify_ancestor(&f)?;
                        chain.push(f);
                    }
                }
                Component::Normal(_) => {
                    prefix.push(component.as_os_str());
                    let f = open(&prefix, false, false, None, true)?;
                    security.verify_ancestor(&f)?;
                    chain.push(f);
                }
                _ => return Err(Error::Unsupported),
            }
        }
        unsafe {
            let volume = wide(std::ffi::OsStr::new(
                &parent.to_str().ok_or(Error::Unsupported)?[..3],
            ));
            let mut fs = [0u16; 16];
            if GetDriveTypeW(volume.as_ptr()) != 3 /* DRIVE_FIXED */
                || GetVolumeInformationW(
                    volume.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    fs.as_mut_ptr(),
                    16,
                ) == 0
                || String::from_utf16_lossy(&fs[..4]) != "NTFS"
            {
                return Err(Error::Unsupported);
            }
        }
        let path = parent.join("VoicePort-recordings-v1");
        let created =
            unsafe { CreateDirectoryW(wide(path.as_os_str()).as_ptr(), &security.attributes) != 0 };
        if !created && unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            return Err(Error::Unavailable);
        }
        let root = open(&path, false, false, None, true)?;
        security.verify(&root)?;
        chain.push(root);
        let markerpath = path.join(".voiceport-owner-v1");
        let mut marker = open(&markerpath, created, created, Some(&security), false)?;
        security.verify(&marker)?;
        let mut expected = b"VPWIN1\0".to_vec();
        for x in &security.sid {
            expected.extend(x.to_le_bytes());
        }
        if created {
            marker
                .write_all(&expected)
                .map_err(|_| Error::Unavailable)?;
            marker.sync_all().map_err(|_| Error::Unavailable)?;
        } else {
            if marker.metadata().map_err(|_| Error::Unavailable)?.len() != expected.len() as u64 {
                return Err(Error::UnsafeObject);
            }
            let mut data = vec![0; expected.len()];
            marker
                .read_exact(&mut data)
                .map_err(|_| Error::UnsafeObject)?;
            if data != expected {
                return Err(Error::UnsafeObject);
            }
        }
        Ok(Self {
            path,
            lease: Arc::new(Lease {
                _chain: chain,
                _marker: marker,
            }),
        })
    }
    pub fn create(&self) -> Result<Pending> {
        let security = Security::current()?;
        security.verify(self.lease._chain.last().ok_or(Error::Unavailable)?)?;
        for _ in 0..8 {
            let mut random = [0u8; 16];
            if unsafe {
                BCryptGenRandom(
                    std::ptr::null_mut(),
                    random.as_mut_ptr(),
                    16,
                    BCRYPT_USE_SYSTEM_PREFERRED_RNG,
                )
            } < 0
            {
                return Err(Error::Unavailable);
            }
            let name = format!(
                "{}.wav",
                random
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            let path = self.path.join(name);
            let file = match open(&path, true, true, Some(&security), false) {
                Err(Error::Collision) => continue,
                x => x?,
            };
            security.verify(&file)?;
            let i = info(&file, false)?;
            let identity = (i.dwVolumeSerialNumber, i.nFileIndexHigh, i.nFileIndexLow);
            let writer = Wav::new(file).map_err(|_| Error::InvalidRecording)?;
            return Ok(Pending {
                path,
                lease: self.lease.clone(),
                writer,
                identity,
                failed: false,
            });
        }
        Err(Error::Collision)
    }
}
impl Pending {
    pub fn append(&mut self, samples: &[i16]) -> Result<()> {
        if self.failed {
            return Err(Error::InvalidRecording);
        }
        if self.writer.append(samples).is_err() {
            self.failed = true;
            return Err(Error::InvalidRecording);
        }
        Ok(())
    }
    pub fn finalize(self, language: Option<String>) -> Result<Recording> {
        if self.failed {
            return Err(Error::InvalidRecording);
        }
        let mut file = self.writer.finish().map_err(|_| Error::InvalidRecording)?;
        file.sync_all().map_err(|_| Error::InvalidRecording)?;
        let security = Security::current()?;
        security.verify(&file)?;
        let i = info(&file, false)?;
        if (i.dwVolumeSerialNumber, i.nFileIndexHigh, i.nFileIndexLow) != self.identity {
            return Err(Error::UnsafeObject);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| Error::InvalidRecording)?;
        Ok(Recording::caller_owned_with_guard(
            file, self.path, language, self.lease,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "actual NTFS APIs require authorized Windows runtime, not executed by target check"]
    fn native_atomic_create_never_truncates_existing_owned_file() {
        let d = tempfile::tempdir().unwrap();
        let root = Root::under_owned_parent(d.path()).unwrap();
        let sec = Security::current().unwrap();
        let p = root.path.join("controlled-collision.wav");
        let mut f = open(&p, true, true, Some(&sec), false).unwrap();
        f.write_all(b"synthetic-existing").unwrap();
        assert!(matches!(
            open(&p, true, true, Some(&sec), false),
            Err(Error::Collision)
        ));
        f.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"synthetic-existing");
    }
}
