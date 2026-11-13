//! Unix domain socket plumbing with the same surface as the Windows named-pipe module (`PipeServer`, `ServerConn`,
//! `Client`), so the engine and the tools compile unchanged. Supported systems: Linux and macOS.
//!
//! Server: one stream socket in a directory only the user can enter (`$XDG_RUNTIME_DIR/twpotato`, or a 0700 directory
//! under the temporary directory when that variable is unset; on macOS always `~/Library/Caches/TwPotato`), a held lock file as the single-instance guard, and the
//! peer user id of every connection compared with ours. Frames are the same bytes as on the pipe (4-byte length, then
//! the payload, at most 64 KB); a stream has no message boundaries, so the length header is what delimits them.

use std::{
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::Mutex,
    time::{Duration, Instant},
};

use twime_proto::{frame, frame_len};

fn uid() -> u32 {
    unsafe { libc::getuid() }
}

/// Why this process must not serve as the engine: it runs as root, or with an effective user other than the real one
/// (the counterpart of the Windows integrity-level rule; no development exception).
pub fn privilege_refusal() -> Option<&'static str> {
    let (real, effective) = unsafe { (libc::getuid(), libc::geteuid()) };
    if real == 0 || effective == 0 {
        Some("running as root is not allowed")
    } else if real != effective {
        Some("real and effective user differ")
    } else {
        None
    }
}

/// (user id, process id) of the other end of a connected socket. The process id is 0 where the system cannot tell.
fn peer(s: &UnixStream) -> io::Result<(u32, u32)> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        let mut c: libc::ucred = std::mem::zeroed();
        let mut n = size_of::<libc::ucred>() as libc::socklen_t;
        if libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut c as *mut _ as *mut _, &mut n) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((c.uid, c.pid as u32))
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let (mut u, mut g) = (0, 0);
        if libc::getpeereid(s.as_raw_fd(), &mut u, &mut g) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((u, 0))
    }
}

/// Directory of the socket: created 0700 when missing; refused unless it is ours and closed to everybody else.
/// A relative `XDG_RUNTIME_DIR` is invalid by the XDG rules and ignored.
/// macOS: one fixed place under the home directory. The temporary directory and `XDG_RUNTIME_DIR` can differ between
/// the input method process and an engine started some other way, and the two would not find each other.
fn runtime_dir() -> io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let dir = match std::env::var_os("HOME").map(PathBuf::from).filter(|v| v.is_absolute()) {
        Some(home) => home.join("Library/Caches/TwPotato"),
        None => return Err(io::Error::new(io::ErrorKind::InvalidInput, "HOME is not an absolute path")),
    };
    #[cfg(not(target_os = "macos"))]
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|v| v.is_absolute()) {
        Some(d) => d.join("twpotato"),
        None => std::env::temp_dir().join(format!("twpotato-{}", uid())),
    };
    match fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let m = fs::symlink_metadata(&dir)?;
    if !m.is_dir() || m.uid() != uid() || m.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "socket directory is not private to this user"));
    }
    Ok(dir)
}

/// `name` as a socket path: a bare name becomes `<runtime dir>/<name>.sock` (the product path; the directory is checked).
/// A name containing `/` is used as it is, for tests and tools only: the caller answers for that directory.
pub fn qualify(name: &str) -> io::Result<PathBuf> {
    if name.contains('/') { Ok(PathBuf::from(name)) } else { Ok(runtime_dir()?.join(format!("{name}.sock"))) }
}

/// Default socket of this user's engine.
pub fn socket_path() -> io::Result<PathBuf> {
    qualify("engine")
}

/// Stands in for the Windows logon session id in `running.lock` (same `hhhhhhhh:llllllll` shape): the user id.
pub fn logon_session_id() -> io::Result<String> {
    Ok(format!("{:08x}:{:08x}", uid(), 0))
}

/// Resident and private (anonymous + swap) bytes of a process, from `/proc` (Linux only).
pub fn memory_of_pid(pid: u32) -> io::Result<(u64, u64)> {
    let t = fs::read_to_string(format!("/proc/{pid}/status"))?;
    let kb = |key: &str| t.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok()).unwrap_or(0);
    Ok((kb("VmRSS:") * 1024, (kb("RssAnon:") + kb("VmSwap:")) * 1024))
}

fn read_frame(mut s: &UnixStream) -> io::Result<Option<Vec<u8>>> {
    let mut h = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match s.read(&mut h[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "bad frame")),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    let n = frame_len(h).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf)?;
    Ok(Some(buf))
}

fn write_frame(mut s: &UnixStream, payload: &[u8]) -> io::Result<()> {
    let f = frame(payload).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "payload too large"))?;
    s.write_all(&f)
}

// ---------------------------------------------------------------------------------------------------------------
// Server

pub struct PipeServer {
    name: String,
    path: PathBuf,
    listener: UnixListener,
    /// Held (exclusive, advisory) for the life of the server: the single-instance guard.
    _lock: File,
}

/// One connected client (server side, blocking I/O).
pub struct ServerConn {
    s: UnixStream,
    pub client_pid: u32,
}

impl PipeServer {
    pub fn new() -> io::Result<PipeServer> {
        Self::with_name(None)
    }

    /// `name` overrides the default per-user socket (tests / tools). Fails if another process serves the same name.
    pub fn with_name(name: Option<String>) -> io::Result<PipeServer> {
        let path = match name {
            Some(n) => qualify(&n)?,
            None => socket_path()?,
        };
        let mut lock_path = path.clone().into_os_string();
        lock_path.push(".lock");
        let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).mode(0o600).open(&lock_path)?;
        lock.try_lock().map_err(|_| io::Error::new(io::ErrorKind::AddrInUse, "another engine holds the socket lock"))?;
        // We hold the lock, so a socket file still there belongs to an engine that is gone. Anything else is not ours.
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_socket() => fs::remove_file(&path)?,
            Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the socket path exists and is not a socket")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(PipeServer { name: path.display().to_string(), path, listener, _lock: lock })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Waits for the next client running as our user; anybody else is dropped.
    pub fn accept(&self) -> io::Result<ServerConn> {
        loop {
            let s = match self.listener.accept() {
                Ok((s, _)) => s,
                // A connection that was reset before we took it is not a failure of the listener.
                Err(e) if matches!(e.kind(), io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted) => continue,
                Err(e) => return Err(e),
            };
            // A client that stops reading must not hold its connection thread forever.
            let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
            match peer(&s) {
                Ok((u, pid)) if u == uid() => return Ok(ServerConn { s, client_pid: pid }),
                _ => continue,
            }
        }
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl ServerConn {
    /// Reads one frame payload; `Ok(None)` when the client disconnected.
    pub fn read(&self) -> io::Result<Option<Vec<u8>>> {
        read_frame(&self.s)
    }

    pub fn write(&self, payload: &[u8]) -> io::Result<()> {
        write_frame(&self.s, payload)
    }

    pub fn disconnect(&self) {
        let _ = self.s.shutdown(std::net::Shutdown::Both);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Client

#[derive(Debug)]
pub enum CallError {
    /// No reply within the time limit; the connection is no longer usable.
    Timeout,
    /// The server went away or sent garbage.
    Broken(io::Error),
}

pub struct Client {
    s: UnixStream,
    /// One request and its reply at a time: concurrent callers would interleave frames.
    busy: Mutex<()>,
    /// 0 where the system cannot tell (macOS).
    pub server_pid: u32,
}

fn call_error(e: io::Error) -> CallError {
    if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) { CallError::Timeout } else { CallError::Broken(e) }
}

impl Client {
    /// Connects to the socket; a missing socket fails at once. The server must run as our user. If `expected_image`
    /// is given, the server executable must equal it (Linux only, `/proc/<pid>/exe`; unsupported elsewhere).
    /// `_wait_ms` is unused: a listening socket queues connections and the engine accepts on its own thread, so the
    /// connect returns at once unless more than the listen backlog of connections are pending.
    pub fn connect(name: &str, expected_image: Option<&str>, _wait_ms: u32) -> io::Result<Client> {
        let s = UnixStream::connect(qualify(name)?).map_err(|e| {
            if e.kind() == io::ErrorKind::ConnectionRefused { io::Error::from(io::ErrorKind::NotFound) } else { e }
        })?;
        let (u, pid) = peer(&s)?;
        if u != uid() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "server user differs"));
        }
        if let Some(want) = expected_image {
            #[cfg(target_os = "linux")]
            if fs::read_link(format!("/proc/{pid}/exe"))? != std::path::Path::new(want) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "server image path differs"));
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = want;
                return Err(io::Error::new(io::ErrorKind::Unsupported, "the server image cannot be checked on this system"));
            }
        }
        Ok(Client { s, busy: Mutex::new(()), server_pid: pid })
    }

    /// Like `connect` without an image check, but never waits. For callers that must not block (the front-end shells'
    /// key path). When the server's queue of connections not yet accepted is full (a server that is stopped or stuck),
    /// Linux makes `connect` wait until the server accepts again; this fails at once instead, with `WouldBlock` on Linux.
    /// Other systems may answer a full queue differently (macOS can refuse the connection, which reads `NotFound` here):
    /// only "returns at once" holds everywhere.
    pub fn connect_nowait(name: &str) -> io::Result<Client> {
        use std::os::unix::{ffi::OsStrExt, io::FromRawFd};
        let path = qualify(name)?;
        let bytes = path.as_os_str().as_bytes();
        let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        if bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path too long"));
        }
        for (d, s) in addr.sun_path.iter_mut().zip(bytes) {
            *d = *s as libc::c_char;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // Owns the descriptor from here on: closed on every early return.
        let s = unsafe { UnixStream::from_raw_fd(fd) };
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            // As the standard library does for its own sockets there: a write to a closed peer must not raise SIGPIPE.
            #[cfg(target_vendor = "apple")]
            {
                let on: libc::c_int = 1;
                libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, &on as *const _ as *const _, size_of::<libc::c_int>() as libc::socklen_t);
            }
            s.set_nonblocking(true)?;
        }
        if unsafe { libc::connect(fd, &addr as *const _ as *const libc::sockaddr, size_of::<libc::sockaddr_un>() as libc::socklen_t) } != 0 {
            let e = io::Error::last_os_error();
            return Err(match e.raw_os_error() {
                Some(libc::ECONNREFUSED) => io::Error::from(io::ErrorKind::NotFound),
                // A stream socket in the file system connects at once or not at all; "in progress" is treated the same.
                Some(libc::EINPROGRESS) => io::Error::from(io::ErrorKind::WouldBlock),
                _ => e,
            });
        }
        s.set_nonblocking(false)?;
        let (u, pid) = peer(&s)?;
        if u != uid() {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "server user differs"));
        }
        Ok(Client { s, busy: Mutex::new(()), server_pid: pid })
    }

    /// Sends one frame and waits for one reply frame, at most `timeout_ms` in total. After `Timeout` or `Broken`
    /// the connection must be dropped.
    pub fn call(&self, payload: &[u8], timeout_ms: u32) -> Result<Vec<u8>, CallError> {
        self.call_with_written(payload, timeout_ms, |_| {})
    }

    /// Like `call`, with notification after the complete request frame has been written.
    pub fn call_with_written(&self, payload: &[u8], timeout_ms: u32, written: impl FnOnce(Instant)) -> Result<Vec<u8>, CallError> {
        let _one = self.busy.lock().unwrap_or_else(|p| p.into_inner());
        let end = Instant::now() + Duration::from_millis(u64::from(timeout_ms));
        let left = || end.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()).ok_or(CallError::Timeout);
        let f = frame(payload).ok_or_else(|| CallError::Broken(io::Error::new(io::ErrorKind::InvalidInput, "payload too large")))?;
        // One deadline for the request and the reply: the timeout is set again before every write and read.
        let mut rest = &f[..];
        while !rest.is_empty() {
            self.s.set_write_timeout(Some(left()?)).map_err(CallError::Broken)?;
            match (&self.s).write(rest) {
                Ok(0) => return Err(CallError::Broken(io::Error::from(io::ErrorKind::WriteZero))),
                Ok(n) => rest = &rest[n..],
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(call_error(e)),
            }
        }
        written(Instant::now());
        let mut buf = Vec::new();
        let mut need = 4;
        let mut header = true;
        loop {
            while buf.len() < need {
                self.s.set_read_timeout(Some(left()?)).map_err(CallError::Broken)?;
                let mut chunk = vec![0u8; need - buf.len()];
                match (&self.s).read(&mut chunk) {
                    Ok(0) => return Err(CallError::Broken(io::Error::from(io::ErrorKind::UnexpectedEof))),
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(call_error(e)),
                }
            }
            if !header {
                return Ok(buf);
            }
            need = frame_len([buf[0], buf[1], buf[2], buf[3]]).ok_or_else(|| CallError::Broken(io::Error::new(io::ErrorKind::InvalidData, "bad frame")))?;
            buf.clear();
            header = false;
            if need == 0 {
                return Ok(buf);
            }
        }
    }

    /// False once the server end is seen gone (no data is consumed). A hint about the transport at this instant, not a
    /// promise that the next call succeeds.
    pub fn alive(&self) -> bool {
        let mut b = [0u8; 1];
        loop {
            let r = unsafe { libc::recv(self.s.as_raw_fd(), b.as_mut_ptr().cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
            if r >= 0 {
                return r > 0;
            }
            match io::Error::last_os_error().kind() {
                io::ErrorKind::Interrupted => {}
                k => return k == io::ErrorKind::WouldBlock,
            }
        }
    }
}
