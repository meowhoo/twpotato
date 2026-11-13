#![cfg(windows)]

use twime_winpipe::{Client, Owned, PipeServer};
use windows::{core::PCWSTR, Win32::{
    Foundation::{ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE},
    Security::{GetTokenInformation, RevertToSelf, SecurityIdentification, TokenImpersonationLevel,
        SECURITY_IMPERSONATION_LEVEL, TOKEN_QUERY},
    Storage::FileSystem::{ReadFile, WriteFile, PIPE_ACCESS_DUPLEX},
    System::{Pipes::{ConnectNamedPipe, CreateNamedPipeW, ImpersonateNamedPipeClient,
        PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_WAIT},
        Threading::{GetCurrentThread, OpenThreadToken}},
}};

fn private_name(suffix: &str) -> String {
    twime_winpipe::qualify(&format!("twpotato-p2-sqos-{}-{suffix}", std::process::id()))
}

#[test]
fn process_security_query_accepts_fixed_size_classes() {
    let token = twime_winpipe::current_token_security().unwrap();
    assert_eq!(token.user, twime_winpipe::current_identity().unwrap().user);
    assert_eq!(token.session_id, twime_winpipe::current_session_id());
}

#[test]
fn identification_client_passes_existing_server_identity_verification() {
    let server = PipeServer::with_name(Some(private_name("identity"))).unwrap();
    let name = server.name().to_owned();
    let worker = std::thread::spawn(move || {
        let conn = server.accept().unwrap();
        // read() runs the production user SID + logon SID verification.
        assert_eq!(conn.read().unwrap().unwrap(), b"identify");
        conn.write(b"verified").unwrap();
    });
    let client = Client::connect(&name, None, 1000).unwrap();
    assert_eq!(client.call(b"identify", 3000).unwrap(), b"verified");
    worker.join().unwrap();
}

#[test]
fn server_impersonation_token_is_identification() {
    // A native test server gives access to its handle without exposing ServerConn internals.
    let name = private_name("level");
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let pipe = unsafe { CreateNamedPipeW(PCWSTR(wide.as_ptr()), PIPE_ACCESS_DUPLEX,
        PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT, 1, 4096, 4096, 0, None) };
    assert_ne!(pipe, INVALID_HANDLE_VALUE);
    let pipe = Owned(pipe);
    let worker = std::thread::spawn(move || unsafe {
        let pipe = pipe; // Capture the Send owner, not its raw HANDLE field.
        if let Err(e) = ConnectNamedPipe(pipe.0, None) {
            assert_eq!((e.code().0 & 0xffff) as u32, ERROR_PIPE_CONNECTED.0);
        }
        let mut buf = [0u8; 4096];
        let mut n = 0;
        ReadFile(pipe.0, Some(&mut buf), Some(&mut n), None).unwrap();
        ImpersonateNamedPipeClient(pipe.0).unwrap();
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) { unsafe { RevertToSelf().unwrap(); } }
        }
        let revert = Revert;
        let mut token = HANDLE::default();
        OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut token).unwrap();
        let token = Owned(token);
        let mut level = SECURITY_IMPERSONATION_LEVEL::default();
        let mut len = 0;
        GetTokenInformation(token.0, TokenImpersonationLevel, Some(&mut level as *mut _ as _),
            size_of::<SECURITY_IMPERSONATION_LEVEL>() as u32, &mut len).unwrap();
        assert_eq!(level, SecurityIdentification);
        drop(revert);
        let reply = twime_proto::frame(b"identification").unwrap();
        WriteFile(pipe.0, Some(&reply), None, None).unwrap();
    });
    let client = Client::connect(&name, None, 1000).unwrap();
    assert_eq!(client.call(b"query", 3000).unwrap(), b"identification");
    worker.join().unwrap();
}

#[test]
fn pipe_names_preserve_sid_and_session_with_distinct_prefixes() {
    let id = twime_winpipe::Identity { user: "S-1-5-21-123".into(), logon: String::new() };
    let session = twime_winpipe::current_session_id();
    assert_eq!(twime_winpipe::pipe_name(&id), format!(r"\\.\pipe\twpotato-S-1-5-21-123-{session}"));
    assert_eq!(twime_winpipe::legacy_pipe_name(&id), format!(r"\\.\pipe\twime-S-1-5-21-123-{session}"));
}
