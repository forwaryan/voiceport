//! Descriptor-relative private recording storage. No operation runs at construction.
//!
//! Only this Rust-specific namespace is inspected. Unknown entries, invalid markers,
//! future timestamps, symlinks, hard links, and sessions with a held lease are retained.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const NAMESPACE: &str = "io.voiceport.rust-recordings-v1";
pub const RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const APPLICATION: &str = "io.voiceport.rust-recording-storage";
const OWNER: &str = "owner.json";
const SESSION: &str = "session.json";
const LEASE: &str = "lease";
const AUDIO: &str = "dictation.m4a";
const DIRECTORY_FLAGS: i32 =
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CleanupReport {
    pub removed: usize,
    pub active: usize,
    pub retained: usize,
    pub failed: usize,
    pub root_unavailable: bool,
}

/// An inert path configuration. The parent directory must already exist.
#[derive(Debug, Clone)]
pub struct Storage {
    root: PathBuf,
}

impl Storage {
    /// Configure isolated storage without inspecting or creating any filesystem entry.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn create_recording(&self) -> io::Result<Recording> {
        self.create_recording_at(SystemTime::now())
    }

    /// Explicit clock injection for deterministic, isolated storage tests.
    pub fn create_recording_at(&self, now: SystemTime) -> io::Result<Recording> {
        let created_at = seconds(now)?;
        let root = self.open_root(true)?;
        let name = Uuid::new_v4().to_string();
        require(root.still_linked())?;
        let cname = cstring(name.as_bytes())?;
        if unsafe { libc::mkdirat(root.directory.as_raw_fd(), cname.as_ptr(), 0o700) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let directory = open_at(root.directory.as_raw_fd(), &name, DIRECTORY_FLAGS)?;
        // A transaction rolls back only exact files it created, never recursively.
        let mut made = BTreeMap::new();
        let result = (|| {
            require(private_directory(&info(directory.as_raw_fd())?))?;
            let lease = create_file(directory.as_raw_fd(), LEASE, &[])?;
            made.insert(LEASE.to_owned(), info(lease.as_raw_fd())?);
            lock(lease.as_raw_fd())?;
            let marker = SessionMarker {
                application: APPLICATION.into(),
                version: 1,
                owner_uid: uid(),
                kind: "session".into(),
                root_id: root.id.clone(),
                session_id: name.clone(),
                created_at,
            };
            let marker_file = create_file(
                directory.as_raw_fd(),
                SESSION,
                &serde_json::to_vec(&marker)?,
            )?;
            made.insert(SESSION.to_owned(), info(marker_file.as_raw_fd())?);
            let audio = create_file(directory.as_raw_fd(), AUDIO, &[])?;
            made.insert(AUDIO.to_owned(), info(audio.as_raw_fd())?);
            require(
                root.still_linked()
                    && matches_directory(root.directory.as_raw_fd(), &name, directory.as_raw_fd()),
            )?;
            Ok((lease, audio))
        })();
        match result {
            Ok((lease, audio)) => Ok(Recording {
                path: self.root.join(&name).join(AUDIO),
                root,
                directory,
                lease,
                marker: made[SESSION],
                name,
                audio: Mutex::new(AudioOutput {
                    file: audio,
                    prepared: false,
                }),
            }),
            Err(error) => {
                remove_files(&root, directory.as_raw_fd(), &name, &made);
                Err(error)
            }
        }
    }

    /// Inspect only this store. This never creates a missing root.
    pub fn cleanup_expired_at(&self, now: SystemTime) -> CleanupReport {
        let mut report = CleanupReport::default();
        let Ok(now) = seconds(now) else {
            report.root_unavailable = true;
            return report;
        };
        let Ok(root) = self.open_root(false) else {
            report.root_unavailable = true;
            return report;
        };
        let entries = (|| {
            require(
                not_future(&info(root.directory.as_raw_fd())?, now)
                    && not_future(&root.marker, now),
            )?;
            names(root.directory.as_raw_fd())
        })();
        let Ok(entries) = entries else {
            report.root_unavailable = true;
            return report;
        };
        for name in entries.into_iter().filter(|name| name != OWNER) {
            if !identifier(&name) || !root.still_linked() {
                report.retained += 1;
                continue;
            }
            let Ok(directory) = open_at(root.directory.as_raw_fd(), &name, DIRECTORY_FLAGS) else {
                report.retained += 1;
                continue;
            };
            let lease = (|| {
                require(private_directory(&info(directory.as_raw_fd())?))?;
                let before = info_at(directory.as_raw_fd(), LEASE)?;
                require(private_file(&before) && before.st_size == 0 && not_future(&before, now))?;
                let lease = open_at(
                    directory.as_raw_fd(),
                    LEASE,
                    libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )?;
                require(same_file(&before, &info(lease.as_raw_fd())?))?;
                Ok::<_, io::Error>(lease)
            })();
            let Ok(lease) = lease else {
                report.retained += 1;
                continue;
            };
            if let Err(error) = lock(lease.as_raw_fd()) {
                if matches!(error.raw_os_error(), Some(libc::EWOULDBLOCK)) {
                    report.active += 1
                } else {
                    report.failed += 1
                }
                continue;
            }
            let Ok(files) = inspect(
                &root,
                directory.as_raw_fd(),
                lease.as_raw_fd(),
                &name,
                now,
                true,
            ) else {
                report.retained += 1;
                continue;
            };
            if remove_files(&root, directory.as_raw_fd(), &name, &files) {
                report.removed += 1
            } else {
                report.failed += 1
            }
        }
        report
    }

    fn open_root(&self, create: bool) -> io::Result<Root> {
        require(
            self.root.is_absolute()
                && self
                    .root
                    .components()
                    .all(|c| matches!(c, Component::RootDir | Component::Normal(_))),
        )?;
        // Path::components normalizes '.', so reject it in the original bytes too.
        require(
            !self
                .root
                .as_os_str()
                .as_bytes()
                .split(|b| *b == b'/')
                .any(|part| part == b"." || part == b".."),
        )?;
        let parent_path = self.root.parent().ok_or_else(unavailable)?;
        let name = self
            .root
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(unavailable)?
            .to_owned();
        let parent_path = cstring(parent_path.as_os_str().as_bytes())?;
        let parent = owned_fd(unsafe {
            libc::open(
                parent_path.as_ptr(),
                (DIRECTORY_FLAGS & !libc::O_NOFOLLOW) | libc::O_NOFOLLOW_ANY,
            )
        })?;
        let cname = cstring(name.as_bytes())?;
        let made = if create {
            let result = unsafe { libc::mkdirat(parent.as_raw_fd(), cname.as_ptr(), 0o700) };
            if result != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
                return Err(io::Error::last_os_error());
            }
            result == 0
        } else {
            false
        };
        let directory = open_at(parent.as_raw_fd(), &name, DIRECTORY_FLAGS)?;
        require(private_directory(&info(directory.as_raw_fd())?))?;
        if made {
            let marker = RootMarker {
                application: APPLICATION.into(),
                version: 1,
                owner_uid: uid(),
                kind: "root".into(),
                root_id: Uuid::new_v4().to_string(),
            };
            // If initialization fails, leave an unrecognized root intact rather than
            // adopt or delete content another process may have added.
            create_file(directory.as_raw_fd(), OWNER, &serde_json::to_vec(&marker)?)?;
        }
        let (marker, marker_info): (RootMarker, _) = read_marker(directory.as_raw_fd(), OWNER)?;
        require(
            marker.application == APPLICATION
                && marker.version == 1
                && marker.owner_uid == uid()
                && marker.kind == "root"
                && identifier(&marker.root_id),
        )?;
        let root = Root {
            parent,
            directory,
            name,
            id: marker.root_id,
            marker: marker_info,
        };
        require(root.still_linked())?;
        Ok(root)
    }
}

/// Owns the directory descriptors and exclusive flock until the final Arc drops.
/// The network worker must retain an Arc while reading; dropping the recorder's
/// reference alone cannot remove an in-flight recording.
#[derive(Debug)]
pub struct Recording {
    path: PathBuf,
    root: Root,
    directory: File,
    lease: File,
    marker: libc::stat,
    name: String,
    audio: Mutex<AudioOutput>,
}

#[derive(Debug)]
struct AudioOutput {
    file: File,
    prepared: bool,
}

impl Recording {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-secure AVFoundation's prepared output before installing the tap. The
    /// initializer may replace the empty placeholder, so retain its actual inode.
    pub(crate) fn secure_audio_file(&self) -> io::Result<()> {
        require(
            self.root.still_linked()
                && matches_directory(
                    self.root.directory.as_raw_fd(),
                    &self.name,
                    self.directory.as_raw_fd(),
                ),
        )?;
        let file = open_at(
            self.directory.as_raw_fd(),
            AUDIO,
            libc::O_RDWR | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )?;
        require(regular_owned(&info(file.as_raw_fd())?))?;
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let stat = info(file.as_raw_fd())?;
        require(private_file(&stat) && matches_at(self.directory.as_raw_fd(), AUDIO, &stat))?;
        let mut output = self.audio.lock().map_err(|_| unavailable())?;
        require(matches_directory(
            self.root.directory.as_raw_fd(),
            &self.name,
            self.directory.as_raw_fd(),
        ))?;
        *output = AudioOutput {
            file,
            prepared: true,
        };
        Ok(())
    }

    /// After the encoder closes, reject replaced, linked, empty or unprotected
    /// output. This must pass before stop() returns an uploadable Arc.
    pub(crate) fn validate_finalized(&self) -> io::Result<()> {
        // Cancellation may unlink this inode even while sync_all is running.
        // Hold the mutex only while taking a reference to the descriptor.
        let audio = self
            .audio
            .lock()
            .map_err(|_| unavailable())?
            .file
            .try_clone()?;
        let stat = info(audio.as_raw_fd())?;
        require(
            self.root.still_linked()
                && private_file(&stat)
                && stat.st_size > 0
                && matches_directory(
                    self.root.directory.as_raw_fd(),
                    &self.name,
                    self.directory.as_raw_fd(),
                )
                && matches_at(self.directory.as_raw_fd(), AUDIO, &stat),
        )?;
        audio.sync_all()
    }

    /// Revoke this exact capture while the encoder may still hold the file open.
    /// Renaming the directory first also prevents the native initializer from
    /// reopening or recreating the original URL after cancellation.
    pub(crate) fn discard_unfinished(&self) -> io::Result<()> {
        let output = self.audio.lock().map_err(|_| unavailable())?;
        let parent = self.root.directory.as_raw_fd();
        let directory = self.directory.as_raw_fd();
        let output_inode = info(output.file.as_raw_fd())?;
        let check_audio = || match info_at(directory, AUDIO) {
            Ok(stat) => {
                require(if output.prepared {
                    private_file(&stat)
                        && stat.st_dev == output_inode.st_dev
                        && stat.st_ino == output_inode.st_ino
                } else {
                    // Until secure_audio_file, the initializer owns this slot
                    // and may replace the original empty file with a new inode.
                    regular_owned(&stat)
                })?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        };
        let lease = info(self.lease.as_raw_fd())?;
        require(
            self.root.still_linked()
                && matches_directory(parent, &self.name, directory)
                && matches_at(directory, SESSION, &self.marker)
                && matches_at(directory, LEASE, &lease)
                && names(directory)?
                    .iter()
                    .all(|name| [SESSION, LEASE, AUDIO].contains(&name.as_str())),
        )?;
        check_audio()?;
        let discarded = format!("{}.discarding", self.name);
        let from = cstring(self.name.as_bytes())?;
        let to = cstring(discarded.as_bytes())?;
        if unsafe {
            libc::renameatx_np(
                parent,
                from.as_ptr(),
                parent,
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        require(matches_directory(parent, &discarded, directory))?;
        require(matches_at(directory, SESSION, &self.marker))?;
        unlink_at(directory, SESSION, 0)?;
        require(matches_at(directory, LEASE, &lease))?;
        unlink_at(directory, LEASE, 0)?;
        // A lookup already in progress at rename may still finish once in this
        // directory. Unlink the reserved output again if it beat the first rmdir.
        for _ in 0..2 {
            if check_audio()? {
                match unlink_at(directory, AUDIO, 0) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
            }
            require(matches_directory(parent, &discarded, directory))?;
            match unlink_at(parent, &discarded, libc::AT_REMOVEDIR) {
                Err(error) if error.raw_os_error() == Some(libc::ENOTEMPTY) => continue,
                result => return result,
            }
        }
        Err(unavailable())
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        // No recursive deletion and no unlink through an unverified path. Unknown
        // data causes retention; a later launch may inspect it conservatively.
        // A name replaced after preparation is unknown content, even if its
        // replacement happens to be an owned regular file with mode 0600.
        let original_audio = self
            .audio
            .lock()
            .ok()
            .and_then(|output| info(output.file.as_raw_fd()).ok());
        if !original_audio.is_some_and(|stat| matches_at(self.directory.as_raw_fd(), AUDIO, &stat))
        {
            return;
        }
        let Ok(now) = seconds(SystemTime::now()) else {
            return;
        };
        if let Ok(files) = inspect(
            &self.root,
            self.directory.as_raw_fd(),
            self.lease.as_raw_fd(),
            &self.name,
            now,
            false,
        ) {
            remove_files(&self.root, self.directory.as_raw_fd(), &self.name, &files);
        }
    }
}

/// Resolve the native user home only when an explicit operation requests it.
pub fn default_storage() -> io::Result<Storage> {
    let home = objc2_foundation::NSHomeDirectory().to_string();
    require(!home.is_empty())?;
    let namespace = if voiceport_core::test_build::active() {
        "io.voiceport.test-rust-recordings-v1"
    } else {
        NAMESPACE
    };
    Ok(Storage::at(
        PathBuf::from(home).join("Library/Caches").join(namespace),
    ))
}

/// Call only during a normal launch, after diagnostic/smoke argument handling.
pub fn cleanup_expired() -> CleanupReport {
    default_storage()
        .map(|store| store.cleanup_expired_at(SystemTime::now()))
        .unwrap_or(CleanupReport {
            root_unavailable: true,
            ..CleanupReport::default()
        })
}

#[derive(Debug)]
struct Root {
    parent: File,
    directory: File,
    name: String,
    id: String,
    marker: libc::stat,
}

impl Root {
    fn still_linked(&self) -> bool {
        matches_directory(
            self.parent.as_raw_fd(),
            &self.name,
            self.directory.as_raw_fd(),
        ) && info(self.directory.as_raw_fd()).is_ok_and(|s| private_directory(&s))
            && matches_at(self.directory.as_raw_fd(), OWNER, &self.marker)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RootMarker {
    application: String,
    version: u32,
    owner_uid: u32,
    kind: String,
    root_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionMarker {
    application: String,
    version: u32,
    owner_uid: u32,
    kind: String,
    root_id: String,
    session_id: String,
    created_at: f64,
}

fn inspect(
    root: &Root,
    directory: RawFd,
    lease: RawFd,
    name: &str,
    now: f64,
    expired: bool,
) -> io::Result<BTreeMap<String, libc::stat>> {
    let directory_info = info(directory)?;
    require(
        root.still_linked()
            && private_directory(&directory_info)
            && not_future(&directory_info, now)
            && matches_directory(root.directory.as_raw_fd(), name, directory),
    )?;
    let entries: BTreeSet<_> = names(directory)?.into_iter().collect();
    require(
        entries.contains(SESSION)
            && entries.contains(LEASE)
            && entries
                .iter()
                .all(|n| [SESSION, LEASE, AUDIO].contains(&n.as_str())),
    )?;
    let mut files = BTreeMap::new();
    for name in entries {
        let stat = info_at(directory, &name)?;
        require(private_file(&stat) && not_future(&stat, now))?;
        files.insert(name, stat);
    }
    let lease_info = files.get(LEASE).ok_or_else(unavailable)?;
    require(lease_info.st_size == 0 && same_file(lease_info, &info(lease)?))?;
    let (marker, stat): (SessionMarker, _) = read_marker(directory, SESSION)?;
    require(
        files.get(SESSION).is_some_and(|s| same_file(s, &stat))
            && marker.application == APPLICATION
            && marker.version == 1
            && marker.kind == "session"
            && marker.owner_uid == uid()
            && marker.root_id == root.id
            && marker.session_id == name
            && identifier(name)
            && marker.created_at.is_finite()
            && marker.created_at >= 0.0
            && marker.created_at <= now
            && (!expired || now - marker.created_at > RETENTION.as_secs_f64()),
    )?;
    Ok(files)
}

fn remove_files(
    root: &Root,
    directory: RawFd,
    name: &str,
    files: &BTreeMap<String, libc::stat>,
) -> bool {
    let expected: BTreeSet<_> = files.keys().cloned().collect();
    if !root.still_linked()
        || !matches_directory(root.directory.as_raw_fd(), name, directory)
        || !names(directory).is_ok_and(|n| n.into_iter().collect::<BTreeSet<_>>() == expected)
        || !files.iter().all(|(n, s)| matches_at(directory, n, s))
    {
        return false;
    }
    // Descriptor-relative unlink never follows a symlink; recheck each inode.
    // As in the Swift store, same-UID hostile mutation is not atomically excluded.
    for file in [AUDIO, SESSION, LEASE] {
        let Some(stat) = files.get(file) else {
            continue;
        };
        if !root.still_linked()
            || !matches_directory(root.directory.as_raw_fd(), name, directory)
            || !matches_at(directory, file, stat)
            || unlink_at(directory, file, 0).is_err()
        {
            return false;
        }
    }
    matches_directory(root.directory.as_raw_fd(), name, directory)
        && unlink_at(root.directory.as_raw_fd(), name, libc::AT_REMOVEDIR).is_ok()
}

fn uid() -> u32 {
    unsafe { libc::geteuid() }
}
fn unavailable() -> io::Error {
    io::Error::other("无法安全访问本用户的录音缓存。")
}
fn require(value: bool) -> io::Result<()> {
    if value { Ok(()) } else { Err(unavailable()) }
}
fn seconds(time: SystemTime) -> io::Result<f64> {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .map_err(|_| unavailable())
}
fn identifier(name: &str) -> bool {
    Uuid::parse_str(name).is_ok_and(|id| id.to_string() == name)
}
fn cstring(value: &[u8]) -> io::Result<CString> {
    CString::new(value).map_err(|_| unavailable())
}
fn owned_fd(fd: RawFd) -> io::Result<File> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
fn open_at(directory: RawFd, name: &str, flags: i32) -> io::Result<File> {
    let name = cstring(name.as_bytes())?;
    owned_fd(unsafe { libc::openat(directory, name.as_ptr(), flags) })
}
fn unlink_at(directory: RawFd, name: &str, flags: i32) -> io::Result<()> {
    let name = cstring(name.as_bytes())?;
    if unsafe { libc::unlinkat(directory, name.as_ptr(), flags) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
fn lock(fd: RawFd) -> io::Result<()> {
    if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
fn info(fd: RawFd) -> io::Result<libc::stat> {
    let mut stat = std::mem::MaybeUninit::uninit();
    if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { stat.assume_init() })
}
fn info_at(directory: RawFd, name: &str) -> io::Result<libc::stat> {
    let name = cstring(name.as_bytes())?;
    let mut stat = std::mem::MaybeUninit::uninit();
    if unsafe {
        libc::fstatat(
            directory,
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { stat.assume_init() })
}
fn private_directory(s: &libc::stat) -> bool {
    s.st_mode & libc::S_IFMT == libc::S_IFDIR && s.st_uid == uid() && s.st_mode & 0o7777 == 0o700
}
fn regular_owned(s: &libc::stat) -> bool {
    s.st_mode & libc::S_IFMT == libc::S_IFREG && s.st_uid == uid() && s.st_nlink == 1
}
fn private_file(s: &libc::stat) -> bool {
    regular_owned(s) && s.st_mode & 0o7777 == 0o600
}
fn not_future(s: &libc::stat, now: f64) -> bool {
    s.st_mtime as f64 + s.st_mtime_nsec as f64 / 1_000_000_000.0 <= now
}
fn same_file(a: &libc::stat, b: &libc::stat) -> bool {
    a.st_dev == b.st_dev
        && a.st_ino == b.st_ino
        && a.st_mode == b.st_mode
        && a.st_uid == b.st_uid
        && a.st_nlink == b.st_nlink
        && a.st_size == b.st_size
        && a.st_mtime == b.st_mtime
        && a.st_mtime_nsec == b.st_mtime_nsec
}
fn matches_at(directory: RawFd, name: &str, expected: &libc::stat) -> bool {
    info_at(directory, name).is_ok_and(|s| same_file(&s, expected))
}
fn matches_directory(parent: RawFd, name: &str, directory: RawFd) -> bool {
    match (info_at(parent, name), info(directory)) {
        (Ok(a), Ok(b)) => {
            a.st_mode & libc::S_IFMT == libc::S_IFDIR
                && a.st_dev == b.st_dev
                && a.st_ino == b.st_ino
        }
        _ => false,
    }
}
fn names(directory: RawFd) -> io::Result<Vec<String>> {
    let listing = open_at(directory, ".", DIRECTORY_FLAGS)?;
    use std::os::fd::IntoRawFd;
    let fd = listing.into_raw_fd();
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        unsafe { libc::close(fd) };
        return Err(io::Error::last_os_error());
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            unsafe { libc::closedir(self.0) };
        }
    }
    let stream = Directory(stream);
    let mut output = Vec::new();
    loop {
        unsafe { *libc::__error() = 0 };
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error);
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let name = name.to_str().map_err(|_| unavailable())?;
        if name != "." && name != ".." {
            output.push(name.to_owned())
        }
        require(output.len() <= 1024)?;
    }
    Ok(output)
}
fn create_file(directory: RawFd, name: &str, data: &[u8]) -> io::Result<File> {
    let cname = cstring(name.as_bytes())?;
    let mut file = owned_fd(unsafe {
        libc::openat(
            directory,
            cname.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    })?;
    let result = (|| {
        if unsafe { libc::fchmod(file.as_raw_fd(), 0o600) } != 0 {
            return Err(io::Error::last_os_error());
        }
        file.write_all(data)?;
        require(private_file(&info(file.as_raw_fd())?))
    })();
    if let Err(error) = result {
        if info(file.as_raw_fd()).is_ok_and(|s| matches_at(directory, name, &s)) {
            let _ = unlink_at(directory, name, 0);
        }
        return Err(error);
    }
    Ok(file)
}
fn read_marker<T: serde::de::DeserializeOwned>(
    directory: RawFd,
    name: &str,
) -> io::Result<(T, libc::stat)> {
    let mut file = open_at(
        directory,
        name,
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
    )?;
    let before = info(file.as_raw_fd())?;
    require(private_file(&before) && before.st_size > 0 && before.st_size <= 4096)?;
    let mut bytes = vec![0; before.st_size as usize];
    file.read_exact(&mut bytes)?;
    require(same_file(&before, &info(file.as_raw_fd())?) && matches_at(directory, name, &before))?;
    Ok((serde_json::from_slice(&bytes)?, before))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_store() -> (tempfile::TempDir, Storage) {
        let folder = tempfile::tempdir_in("/tmp").unwrap();
        let store = Storage::at(folder.path().canonicalize().unwrap().join(NAMESPACE));
        (folder, store)
    }

    #[test]
    fn discard_revokes_the_path_without_waiting_for_an_open_writer() {
        let (_folder, store) = isolated_store();
        let recording = store.create_recording().unwrap();
        let other = store.create_recording().unwrap();
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(recording.path())
            .unwrap();
        recording.secure_audio_file().unwrap();
        writer.write_all(b"synthetic PCM").unwrap();
        recording.discard_unfinished().unwrap();
        writer
            .write_all(b"late write to the unlinked inode")
            .unwrap();
        drop(writer);
        assert!(recording.validate_finalized().is_err());
        assert!(!recording.path().parent().unwrap().exists());
        assert!(other.path().exists());
        assert_eq!(std::fs::read_dir(store.path()).unwrap().count(), 2);
    }

    #[test]
    fn discard_accepts_the_initializer_output_but_preserves_a_secured_replacement() {
        let (_folder, store) = isolated_store();
        let unprepared = store.create_recording().unwrap();
        let replace_audio = |recording: &Recording| {
            std::fs::remove_file(recording.path()).unwrap();
            create_file(
                recording.directory.as_raw_fd(),
                AUDIO,
                b"synthetic replacement",
            )
            .unwrap();
        };
        replace_audio(&unprepared);
        unprepared.discard_unfinished().unwrap();
        assert!(!unprepared.path().parent().unwrap().exists());

        let prepared = store.create_recording().unwrap();
        prepared.secure_audio_file().unwrap();
        replace_audio(&prepared);
        assert!(prepared.discard_unfinished().is_err());
        let retained = prepared.path().to_owned();
        drop(prepared);
        assert_eq!(std::fs::read(retained).unwrap(), b"synthetic replacement");
    }
}
