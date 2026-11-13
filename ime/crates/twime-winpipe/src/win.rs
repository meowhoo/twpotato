//! Windows named-pipe plumbing shared by the engine, the TSF shell and the tools.
//!
//! Server: first instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`, DACL granting only the current user,
//! `PIPE_REJECT_REMOTE_CLIENTS`, client identity (user SID and logon SID) verified by impersonation, frames limited to
//! 64 KB. Client: overlapped I/O with a hard timeout, server identity checked by process image path.
//!
//! Written from the Microsoft documentation only.

#![cfg(windows)]

use std::{
    io,
    sync::{Mutex, atomic::{AtomicBool, Ordering}},
};

use twime_proto::{MAX_FRAME, frame, frame_len};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_PIPE_BUSY,
            ERROR_PIPE_CONNECTED, ERROR_NO_DATA, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree,
            WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Security::{
            Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW},
            GetTokenInformation, PSECURITY_DESCRIPTOR, RevertToSelf, SECURITY_ATTRIBUTES, TOKEN_GROUPS, TOKEN_QUERY,
            TOKEN_USER, TokenGroups, TokenUser, SID_AND_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING,
            PIPE_ACCESS_DUPLEX, ReadFile, WriteFile, SECURITY_SQOS_PRESENT, SECURITY_IDENTIFICATION,
        },
        System::{
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
                GetNamedPipeServerProcessId, ImpersonateNamedPipeClient, NAMED_PIPE_MODE, PIPE_READMODE_MESSAGE,
                PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, SetNamedPipeHandleState,
                WaitNamedPipeW,
            },
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
            Threading::{
                CreateEventW, GetCurrentProcess, GetCurrentThread, OpenProcess, OpenProcessToken, OpenThreadToken,
                PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
                ResetEvent, WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

const BUF: usize = MAX_FRAME + 16;

fn os(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Closes the handle on drop.
pub struct Owned(pub HANDLE);
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Identity

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// SID string of the token user.
    pub user: String,
    /// SID string of the logon-session group (S-1-5-5-x-y), empty if the token has none.
    pub logon: String,
}

fn sid_string(sid: windows::Win32::Security::PSID) -> io::Result<String> {
    unsafe {
        let mut p = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut p).map_err(os)?;
        let s = p.to_string().map_err(|e| io::Error::other(e.to_string()));
        let _ = LocalFree(Some(HLOCAL(p.0 as _)));
        s
    }
}

fn token_info(token: HANDLE, class: windows::Win32::Security::TOKEN_INFORMATION_CLASS) -> io::Result<Vec<u8>> {
    unsafe {
        let mut len = 0u32;
        let _ = GetTokenInformation(token, class, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize + 8];
        GetTokenInformation(token, class, Some(buf.as_mut_ptr() as _), buf.len() as u32, &mut len).map_err(os)?;
        Ok(buf)
    }
}

fn identity_of_token(token: HANDLE) -> io::Result<Identity> {
    unsafe {
        let b = token_info(token, TokenUser)?;
        let user = sid_string((*(b.as_ptr() as *const TOKEN_USER)).User.Sid)?;
        let g = token_info(token, TokenGroups)?;
        let groups = &*(g.as_ptr() as *const TOKEN_GROUPS);
        let arr: &[SID_AND_ATTRIBUTES] = std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
        let mut logon = String::new();
        for a in arr {
            if a.Attributes & 0xC000_0000 == 0xC000_0000 {
                logon = sid_string(a.Sid)?; // SE_GROUP_LOGON_ID
                break;
            }
        }
        Ok(Identity { user, logon })
    }
}

/// Identity of the current process token.
pub fn current_identity() -> io::Result<Identity> {
    unsafe {
        let mut t = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t).map_err(os)?;
        let t = Owned(t);
        identity_of_token(t.0)
    }
}

pub fn current_session_id() -> u32 {
    unsafe {
        let mut s = 0u32;
        let _ = windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(windows::Win32::System::Threading::GetCurrentProcessId(), &mut s);
        s
    }
}

/// `name` as a full pipe path (a bare name gets the pipe-namespace prefix).
pub fn qualify(name: &str) -> String {
    const PREFIX: &str = "\\\\.\\pipe\\";
    if name.starts_with("\\\\") { name.to_string() } else { format!("{PREFIX}{name}") }
}

/// Pipe name for this user and Windows session.
pub fn pipe_name(id: &Identity) -> String {
    format!(r"\\.\pipe\twpotato-{}-{}", id.user, current_session_id())
}

/// Pre-rename pipe name, for installers locating an older engine.
pub fn legacy_pipe_name(id: &Identity) -> String {
    format!(r"\\.\pipe\twime-{}-{}", id.user, current_session_id())
}

/// Security facts read from one process token. Callers must handle query failure conservatively.
pub struct TokenSecurity {
    pub integrity_rid: u32,
    pub elevated: bool,
    pub user: String,
    pub session_id: u32,
}

pub fn current_token_security() -> io::Result<TokenSecurity> {
    use windows::Win32::Security::{GetSidSubAuthority, GetSidSubAuthorityCount, TOKEN_MANDATORY_LABEL,
        TOKEN_ELEVATION, TokenIntegrityLevel, TokenElevation, TokenSessionId};
    // Fixed-size classes reject padded lengths; usize storage also aligns the returned structures.
    fn info(token: HANDLE, class: windows::Win32::Security::TOKEN_INFORMATION_CLASS) -> io::Result<Vec<usize>> {
        unsafe {
            let mut len = 0;
            let _ = GetTokenInformation(token, class, None, 0, &mut len);
            let mut buf = vec![0usize; (len as usize).div_ceil(size_of::<usize>())];
            GetTokenInformation(token, class, Some(buf.as_mut_ptr().cast()), len, &mut len).map_err(os)?;
            Ok(buf)
        }
    }
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(os)?;
        let token = Owned(token);
        let label = info(token.0, TokenIntegrityLevel)?;
        let sid = (*(label.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 { return Err(io::Error::other("integrity SID has no RID")); }
        let integrity_rid = *GetSidSubAuthority(sid, u32::from(count - 1));
        let elevation = info(token.0, TokenElevation)?;
        let elevated = (*(elevation.as_ptr() as *const TOKEN_ELEVATION)).TokenIsElevated != 0;
        let user = info(token.0, TokenUser)?;
        let user = sid_string((*(user.as_ptr() as *const TOKEN_USER)).User.Sid)?;
        let session = info(token.0, TokenSessionId)?;
        let session_id = *(session.as_ptr() as *const u32);
        Ok(TokenSecurity { integrity_rid, elevated, user, session_id })
    }
}

/// Full image path of a process, if it can be opened.
pub fn image_path_of_pid(pid: u32) -> io::Result<String> {
    unsafe {
        let h = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(os)?);
        let mut buf = vec![0u16; 1024];
        let mut n = buf.len() as u32;
        QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).map_err(os)?;
        Ok(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// Working set and private usage in bytes of a process.
pub fn memory_of_pid(pid: u32) -> io::Result<(u64, u64)> {
    unsafe {
        let h = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(os)?);
        let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(h.0, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, c.cb).map_err(os)?;
        Ok((c.WorkingSetSize as u64, c.PrivateUsage as u64))
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Server

pub struct PipeServer {
    name: String,
    id: Identity,
    first: AtomicBool,
    next: Mutex<Option<Owned>>,
}

/// One connected client (server side, blocking I/O).
pub struct ServerConn {
    h: Owned,
    pub client_pid: u32,
    expect: Identity,
    verified: AtomicBool,
}

impl PipeServer {
    /// Creates the first instance. Fails if another process already owns the name (`FILE_FLAG_FIRST_PIPE_INSTANCE`).
    pub fn new() -> io::Result<PipeServer> {
        Self::with_name(None)
    }

    /// `name` overrides the default per-user name (tests / tools).
    pub fn with_name(name: Option<String>) -> io::Result<PipeServer> {
        let id = current_identity()?;
        let name = name.map(|n| qualify(&n)).unwrap_or_else(|| pipe_name(&id));
        let s = PipeServer { name, id, first: AtomicBool::new(true), next: Mutex::new(None) };
        let inst = s.create_instance()?;
        *s.next.lock().unwrap() = Some(inst);
        Ok(s)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn create_instance(&self) -> io::Result<Owned> {
        unsafe {
            let sddl = wide(&format!("D:P(A;;GA;;;{})", self.id.user));
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), 1, &mut sd, None).map_err(os)?;
            let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
            let mut flags = PIPE_ACCESS_DUPLEX;
            if self.first.swap(false, Ordering::SeqCst) {
                flags |= FILE_FLAG_FIRST_PIPE_INSTANCE;
            }
            let n = wide(&self.name);
            let h = CreateNamedPipeW(
                PCWSTR(n.as_ptr()),
                flags,
                NAMED_PIPE_MODE(PIPE_TYPE_MESSAGE.0 | PIPE_READMODE_MESSAGE.0 | PIPE_WAIT.0 | PIPE_REJECT_REMOTE_CLIENTS.0),
                PIPE_UNLIMITED_INSTANCES,
                BUF as u32,
                BUF as u32,
                0,
                Some(&sa),
            );
            let err = io::Error::last_os_error();
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            if h == INVALID_HANDLE_VALUE {
                // The first-instance failure must be reported as such; a later failure is retried by the caller.
                self.first.store(false, Ordering::SeqCst);
                return Err(err);
            }
            Ok(Owned(h))
        }
    }

    /// Waits for the next client whose identity matches ours. Always keeps a fresh instance ready for the next one.
    pub fn accept(&self) -> io::Result<ServerConn> {
        loop {
            let inst = match self.next.lock().unwrap().take() {
                Some(i) => i,
                None => self.create_instance()?,
            };
            let connected = unsafe {
                match ConnectNamedPipe(inst.0, None) {
                    Ok(()) => true,
                    Err(e) => (e.code().0 & 0xFFFF) as u32 == ERROR_PIPE_CONNECTED.0,
                }
            };
            // Re-arm before handling this client so a new connect never finds the name without a free instance.
            if let Ok(n) = self.create_instance() {
                *self.next.lock().unwrap() = Some(n);
            }
            if !connected {
                continue;
            }
            let mut pid = 0u32;
            if unsafe { GetNamedPipeClientProcessId(inst.0, &mut pid) }.is_err() {
                unsafe {
                    let _ = DisconnectNamedPipe(inst.0);
                }
                continue;
            }
            // The identity check needs data to have been read first (ImpersonateNamedPipeClient), so it runs in
            // `ServerConn::read` on the first message.
            return Ok(ServerConn { h: inst, client_pid: pid, expect: self.id.clone(), verified: AtomicBool::new(false) });
        }
    }
}

/// Fails unless the pipe's client has the same user SID and logon SID as `expect`. Only valid after a read.
fn verify_pipe_client(h: HANDLE, expect: &Identity) -> io::Result<()> {
    unsafe {
        ImpersonateNamedPipeClient(h).map_err(os)?;
        let mut t = HANDLE::default();
        let r = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut t).map_err(os);
        let ident = r.and_then(|_| {
            let t = Owned(t);
            identity_of_token(t.0)
        });
        let _ = RevertToSelf();
        if ident? == *expect { Ok(()) } else { Err(io::Error::new(io::ErrorKind::PermissionDenied, "client identity differs")) }
    }
}

impl ServerConn {
    /// Reads one frame payload; `Ok(None)` when the client disconnected.
    pub fn read(&self) -> io::Result<Option<Vec<u8>>> {
        let mut buf = vec![0u8; BUF];
        let mut n = 0u32;
        let r = unsafe { ReadFile(self.h.0, Some(&mut buf), Some(&mut n), None) };
        match r {
            Ok(()) => {}
            Err(e) => {
                let c = (e.code().0 & 0xFFFF) as u32;
                if c == ERROR_BROKEN_PIPE.0 || c == ERROR_NO_DATA.0 {
                    return Ok(None);
                }
                if c == ERROR_MORE_DATA.0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
                }
                return Err(os(e));
            }
        }
        if !self.verified.load(Ordering::Relaxed) {
            verify_pipe_client(self.h.0, &self.expect)?;
            self.verified.store(true, Ordering::Relaxed);
        }
        unpack(&buf[..n as usize]).map(Some)
    }

    pub fn write(&self, payload: &[u8]) -> io::Result<()> {
        let f = frame(payload).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "payload too large"))?;
        let mut n = 0u32;
        unsafe { WriteFile(self.h.0, Some(&f), Some(&mut n), None).map_err(os) }
    }

    pub fn disconnect(&self) {
        unsafe {
            let _ = DisconnectNamedPipe(self.h.0);
        }
    }
}

fn unpack(msg: &[u8]) -> io::Result<Vec<u8>> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "bad frame");
    if msg.len() < 4 {
        return Err(bad());
    }
    let n = frame_len([msg[0], msg[1], msg[2], msg[3]]).ok_or_else(bad)?;
    if msg.len() != 4 + n {
        return Err(bad());
    }
    Ok(msg[4..].to_vec())
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
    h: Owned,
    ev: Owned,
    pub server_pid: u32,
}

impl Client {
    /// Connects to the pipe. If `expected_image` is given, the server process image path must equal it
    /// (case-insensitive). Waits up to `wait_ms` while all instances are busy; a missing pipe fails at once.
    pub fn connect(name: &str, expected_image: Option<&str>, wait_ms: u32) -> io::Result<Client> {
        unsafe {
            let n = wide(&qualify(name));
            let h = loop {
                match CreateFileW(
                    PCWSTR(n.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    None,
                ) {
                    Ok(h) => break h,
                    Err(e) => {
                        let c = (e.code().0 & 0xFFFF) as u32;
                        if c == ERROR_PIPE_BUSY.0 && wait_ms > 0 {
                            if !WaitNamedPipeW(PCWSTR(n.as_ptr()), wait_ms).as_bool() {
                                return Err(io::Error::from(io::ErrorKind::TimedOut));
                            }
                            continue;
                        }
                        if c == ERROR_FILE_NOT_FOUND.0 {
                            return Err(io::Error::from(io::ErrorKind::NotFound));
                        }
                        return Err(os(e));
                    }
                }
            };
            let h = Owned(h);
            let mode = PIPE_READMODE_MESSAGE;
            SetNamedPipeHandleState(h.0, Some(&mode), None, None).map_err(os)?;
            let mut pid = 0u32;
            GetNamedPipeServerProcessId(h.0, &mut pid).map_err(os)?;
            if let Some(want) = expected_image {
                let got = image_path_of_pid(pid)?;
                if !got.eq_ignore_ascii_case(want) {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "server image path differs"));
                }
            }
            let ev = Owned(CreateEventW(None, true, false, PCWSTR::null()).map_err(os)?);
            Ok(Client { h, ev, server_pid: pid })
        }
    }

    /// Sends one frame and waits for one reply frame, at most `timeout_ms` in total. After `Timeout` or `Broken`
    /// the connection must be dropped.
    pub fn call(&self, payload: &[u8], timeout_ms: u32) -> Result<Vec<u8>, CallError> {
        self.call_with_written(payload, timeout_ms, |_| {})
    }

    /// Like `call`, with notification after the complete request frame has been written.
    /// The timestamp is captured after write completion, before invoking the callback.
    pub fn call_with_written(&self, payload: &[u8], timeout_ms: u32, written: impl FnOnce(std::time::Instant)) -> Result<Vec<u8>, CallError> {
        let f = frame(payload).ok_or_else(|| CallError::Broken(io::Error::new(io::ErrorKind::InvalidInput, "payload too large")))?;
        let start = std::time::Instant::now();
        unsafe {
            self.io(|ov| WriteFile(self.h.0, Some(&f), None, Some(ov)), timeout_ms).map(|_| ())?;
            written(std::time::Instant::now());
            let mut buf = vec![0u8; BUF];
            let left = timeout_ms.saturating_sub(start.elapsed().as_millis() as u32);
            let n = self.io(|ov| ReadFile(self.h.0, Some(&mut buf), None, Some(ov)), left)?;
            unpack(&buf[..n as usize]).map_err(CallError::Broken)
        }
    }

    unsafe fn io(
        &self,
        op: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
        timeout_ms: u32,
    ) -> Result<u32, CallError> {
        unsafe {
            let _ = ResetEvent(self.ev.0);
            let mut ov = OVERLAPPED { hEvent: self.ev.0, ..Default::default() };
            if let Err(e) = op(&mut ov) {
                if (e.code().0 & 0xFFFF) as u32 != ERROR_IO_PENDING.0 {
                    return Err(CallError::Broken(os(e)));
                }
                let w = WaitForSingleObject(self.ev.0, timeout_ms);
                if w == WAIT_TIMEOUT {
                    let _ = CancelIoEx(self.h.0, Some(&ov));
                    // The cancelled operation must finish before `ov` goes out of scope.
                    let mut n = 0u32;
                    let _ = GetOverlappedResult(self.h.0, &ov, &mut n, true);
                    return Err(CallError::Timeout);
                }
                if w != WAIT_OBJECT_0 {
                    let _ = CancelIoEx(self.h.0, Some(&ov));
                    let mut n = 0u32;
                    let _ = GetOverlappedResult(self.h.0, &ov, &mut n, true);
                    return Err(CallError::Broken(io::Error::other("wait failed")));
                }
            }
            let mut n = 0u32;
            GetOverlappedResult(self.h.0, &ov, &mut n, false).map_err(|e| CallError::Broken(os(e)))?;
            Ok(n)
        }
    }
}

impl Client {
    /// False once the server end is gone (cheap check, no data is consumed).
    pub fn alive(&self) -> bool {
        unsafe { windows::Win32::System::Pipes::PeekNamedPipe(self.h.0, None, 0, None, None, None).is_ok() }
    }
}

/// Logon session id of the current token, `hhhhhhhh:llllllll` (TOKEN_STATISTICS.AuthenticationId).
pub fn logon_session_id() -> io::Result<String> {
    use windows::Win32::Security::{TOKEN_STATISTICS, TokenStatistics};
    unsafe {
        let mut t = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t).map_err(os)?;
        let t = Owned(t);
        let b = token_info(t.0, TokenStatistics)?;
        let st = &*(b.as_ptr() as *const TOKEN_STATISTICS);
        Ok(format!("{:08x}:{:08x}", st.AuthenticationId.HighPart as u32, st.AuthenticationId.LowPart))
    }
}
