//! Same-UID Unix sockets, private directory, exclusive bind, bounded frames and queues.
#[cfg(test)]
mod tests;
use crate::{MAX_FRAME, Request, Response, TIMEOUT, VERSION};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Instant,
};
fn failure(code: &str) -> Response {
    Response::error(
        code,
        match code {
            "app_not_running" => {
                "App is not running at this socket. Explicitly launch the intended bundle."
            }
            "unsafe_socket" => "Socket or private directory failed ownership/type/mode checks.",
            "busy" => "Control connection or queue limit reached.",
            "timeout" => "Control deadline elapsed. Inspect state before retrying any mutation.",
            _ => "Local control protocol failed. No automatic retry was performed.",
        },
    )
}
pub fn default_path() -> PathBuf {
    // /tmp is a symlink on macOS. Use the real system directory.
    let root = if cfg!(target_os = "macos") {
        "/private/tmp"
    } else {
        "/tmp"
    };
    Path::new(root)
        .join(format!("voiceport-control-{}", unsafe { libc::geteuid() }))
        .join("control.sock")
}
pub fn path_from_env() -> Result<PathBuf, Response> {
    let path =
        std::env::var_os("VOICEPORT_CONTROL_SOCKET").map_or_else(default_path, PathBuf::from);
    validate_path(&path)?;
    Ok(path)
}
fn validate_path(path: &Path) -> Result<(), Response> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
        || path.as_os_str().len() > 100
    {
        return Err(failure("unsafe_socket"));
    }
    Ok(())
}
fn private_parent(path: &Path, create: bool) -> Result<(), Response> {
    validate_path(path)?;
    let parent = path.parent().ok_or_else(|| failure("unsafe_socket"))?;
    // Refuse all linked ancestors; only the immediate private directory may be created.
    for ancestor in parent.ancestors().skip(1) {
        let m = fs::symlink_metadata(ancestor).map_err(|_| failure("unsafe_socket"))?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(failure("unsafe_socket"));
        }
    }
    if create {
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(_) => return Err(failure("unsafe_socket")),
        }
    }
    let m = fs::symlink_metadata(parent).map_err(|e| {
        failure(if e.kind() == io::ErrorKind::NotFound {
            "app_not_running"
        } else {
            "unsafe_socket"
        })
    })?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o700 {
        return Err(failure("unsafe_socket"));
    }
    Ok(())
}
fn socket_identity(path: &Path) -> Result<(u64, u64), Response> {
    let m = fs::symlink_metadata(path).map_err(|e| {
        failure(if e.kind() == io::ErrorKind::NotFound {
            "app_not_running"
        } else {
            "unsafe_socket"
        })
    })?;
    if !m.file_type().is_socket()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o777 != 0o600
    {
        return Err(failure("unsafe_socket"));
    }
    Ok((m.dev(), m.ino()))
}
fn peer(stream: &UnixStream) -> Result<(), Response> {
    #[cfg(target_os = "macos")]
    let uid = {
        let (mut uid, mut gid) = (0, 0);
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(failure("unsafe_socket"));
        }
        uid
    };
    #[cfg(target_os = "linux")]
    let uid = {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of_val(&cred) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut size,
            )
        } != 0
        {
            return Err(failure("unsafe_socket"));
        }
        cred.uid
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = stream;
        return Err(failure("unsupported"));
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if uid != unsafe { libc::geteuid() } {
        return Err(failure("unsafe_socket"));
    }
    Ok(())
}
fn wait(stream: &UnixStream, deadline: Instant, events: libc::c_short) -> Result<(), Response> {
    stream
        .set_nonblocking(true)
        .map_err(|_| failure("protocol_error"))?;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| failure("timeout"))?;
        let millis = remaining
            .as_millis()
            .saturating_add(1)
            .min(i32::MAX as u128) as i32;
        let mut fd = libc::pollfd {
            fd: stream.as_raw_fd(),
            events,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut fd, 1, millis) };
        if rc > 0 {
            if fd.revents & libc::POLLNVAL != 0 {
                return Err(failure("protocol_error"));
            }
            return Ok(());
        }
        if rc < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(failure("protocol_error"));
        }
    }
}
fn read_exact(
    stream: &mut UnixStream,
    mut bytes: &mut [u8],
    deadline: Instant,
) -> Result<(), Response> {
    while !bytes.is_empty() {
        wait(stream, deadline, libc::POLLIN)?;
        match stream.read(bytes) {
            Ok(0) => return Err(failure("protocol_error")),
            Ok(n) => bytes = &mut bytes[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) => {}
            Err(e) => {
                return Err(failure(
                    if matches!(
                        e.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) {
                        "timeout"
                    } else {
                        "protocol_error"
                    },
                ));
            }
        }
    }
    Ok(())
}
fn read_frame<T: DeserializeOwned>(
    stream: &mut UnixStream,
    deadline: Instant,
) -> Result<T, Response> {
    let mut size = [0; 4];
    read_exact(stream, &mut size, deadline)?;
    let size = u32::from_be_bytes(size) as usize;
    if size == 0 || size > MAX_FRAME {
        return Err(failure("frame_too_large"));
    }
    let mut bytes = vec![0; size];
    read_exact(stream, &mut bytes, deadline)?;
    serde_json::from_slice(&bytes).map_err(|_| failure("invalid_request"))
}
fn write_frame(
    stream: &mut UnixStream,
    value: &impl Serialize,
    deadline: Instant,
) -> Result<(), Response> {
    let bytes = serde_json::to_vec(value).map_err(|_| failure("protocol_error"))?;
    if bytes.len() > MAX_FRAME {
        return Err(failure("frame_too_large"));
    }
    let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
    frame.extend(bytes);
    let mut remaining = frame.as_slice();
    while !remaining.is_empty() {
        wait(stream, deadline, libc::POLLOUT)?;
        let n = match stream.write(remaining) {
            Ok(n) => n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(_) => return Err(failure("protocol_error")),
        };
        if n == 0 {
            return Err(failure("protocol_error"));
        }
        remaining = &remaining[n..];
    }
    Ok(())
}
pub fn call(path: &Path, action: crate::Action) -> Result<Response, Response> {
    private_parent(path, false)?;
    let identity = socket_identity(path)?;
    let deadline = Instant::now() + TIMEOUT;
    // Nonblocking connect avoids an unbounded full-listen-backlog wait.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(failure("protocol_error"));
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(failure("protocol_error"));
    }
    stream
        .set_nonblocking(true)
        .map_err(|_| failure("protocol_error"))?;
    connect(&stream, path)?;
    stream
        .set_nonblocking(false)
        .map_err(|_| failure("protocol_error"))?;
    peer(&stream)?;
    if socket_identity(path)? != identity {
        return Err(failure("unsafe_socket"));
    }
    let mut stream = stream;
    write_frame(
        &mut stream,
        &Request {
            schema_version: VERSION,
            action,
        },
        deadline,
    )?;
    let reply: Response = read_frame(&mut stream, deadline)?;
    if reply.schema_version != VERSION
        || !matches!(reply.status.as_str(), "ok" | "error")
        || (reply.status == "ok") != reply.error.is_none()
    {
        return Err(failure("protocol_error"));
    }
    Ok(reply)
}
fn connect(stream: &UnixStream, path: &Path) -> Result<(), Response> {
    use std::os::unix::ffi::OsStrExt;
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as _;
    let bytes = path.as_os_str().as_bytes();
    if bytes.len() >= address.sun_path.len() {
        return Err(failure("unsafe_socket"));
    }
    for (dst, src) in address.sun_path.iter_mut().zip(bytes) {
        *dst = *src as _;
    }
    let rc = unsafe {
        libc::connect(
            stream.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as _,
        )
    };
    if rc != 0 {
        return Err(failure(match io::Error::last_os_error().raw_os_error() {
            Some(libc::ECONNREFUSED | libc::ENOENT) => "app_not_running",
            Some(libc::EAGAIN | libc::EINPROGRESS) => "busy",
            _ => "protocol_error",
        }));
    }
    Ok(())
}
pub struct Pending {
    pub request: Request,
    pub deadline: Instant,
    reply: SyncSender<Response>,
}
impl Pending {
    pub fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }
    pub fn respond(self, response: Response) {
        let response = if serde_json::to_vec(&response).is_ok_and(|v| v.len() <= MAX_FRAME) {
            response
        } else {
            failure("frame_too_large")
        };
        let _ = self.reply.try_send(response);
    }
}
pub struct Server {
    requests: Receiver<Pending>,
    stop: Arc<AtomicBool>,
    _guard: SocketGuard,
}
struct SocketGuard {
    path: PathBuf,
    identity: (u64, u64),
}
impl Drop for SocketGuard {
    fn drop(&mut self) {
        if socket_identity(&self.path).ok() == Some(self.identity) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
impl Server {
    pub fn bind(path: &Path) -> Result<Self, Response> {
        private_parent(path, true)?;
        // Existing sockets, including stale/unknown ones, are never removed.
        if fs::symlink_metadata(path).is_ok() {
            return Err(failure("socket_in_use"));
        }
        let listener = UnixListener::bind(path).map_err(|_| failure("socket_in_use"))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| failure("unsafe_socket"))?;
        let guard = SocketGuard {
            path: path.into(),
            identity: socket_identity(path)?,
        };
        listener
            .set_nonblocking(true)
            .map_err(|_| failure("protocol_error"))?;
        let (tx, requests) = mpsc::sync_channel(4);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        std::thread::Builder::new()
            .name("voiceport-control-listener".into())
            .spawn(move || {
                let active = Arc::new(AtomicUsize::new(0));
                while !stopping.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            if active.load(Ordering::Acquire) >= 4 {
                                let _ = write_frame(
                                    &mut stream,
                                    &failure("busy"),
                                    Instant::now() + std::time::Duration::from_millis(10),
                                );
                                continue;
                            }
                            active.fetch_add(1, Ordering::AcqRel);
                            let active = active.clone();
                            let tx = tx.clone();
                            let count = active.clone();
                            if std::thread::Builder::new()
                                .name("voiceport-control-connection".into())
                                .spawn(move || {
                                    let deadline = Instant::now() + TIMEOUT;
                                    let result = (|| {
                                        peer(&stream)?;
                                        let request: Request = read_frame(&mut stream, deadline)?;
                                        if request.schema_version != VERSION {
                                            return Err(failure("version_mismatch"));
                                        }
                                        let (reply, response) = mpsc::sync_channel(1);
                                        tx.try_send(Pending {
                                            request,
                                            deadline,
                                            reply,
                                        })
                                        .map_err(|_| failure("busy"))?;
                                        response
                                            .recv_timeout(
                                                deadline.saturating_duration_since(Instant::now()),
                                            )
                                            .map_err(|_| failure("timeout"))
                                    })()
                                    .unwrap_or_else(|e| e);
                                    let _ = write_frame(&mut stream, &result, deadline);
                                    active.fetch_sub(1, Ordering::AcqRel);
                                })
                                .is_err()
                            {
                                count.fetch_sub(1, Ordering::AcqRel);
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(10))
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|_| failure("protocol_error"))?;
        Ok(Self {
            requests,
            stop,
            _guard: guard,
        })
    }
    pub fn next(&self) -> Option<Pending> {
        self.requests.try_recv().ok()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
