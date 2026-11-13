#![cfg(unix)]
//! The Unix socket transport: frames in both directions, the single-instance guard, a stale socket file, timeouts.

use std::time::{Duration, Instant};

use twime_winpipe::{CallError, Client, PipeServer};

fn name(tag: &str) -> String {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    dir.join(format!("u-{tag}-{}.sock", std::process::id())).display().to_string()
}

/// A child process started while another thread holds a server keeps a copy of that server's lock file open from its
/// creation until it runs its own program; if the server is dropped in that moment, the lock is still held and the
/// next server with the same name is refused. In this test program one test starts processes and one counts on a
/// dropped server's name being free at once: they take turns. (The engine itself starts no processes.)
static NO_SPAWN_WHILE_A_NAME_IS_REUSED: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Echo server: answers every frame with the same payload; `slow` delays each answer.
fn echo(server: PipeServer, slow: Duration) {
    std::thread::spawn(move || {
        loop {
            let Ok(conn) = server.accept() else { break };
            std::thread::spawn(move || {
                while let Ok(Some(p)) = conn.read() {
                    std::thread::sleep(slow);
                    if conn.write(&p).is_err() {
                        break;
                    }
                }
            });
        }
    });
}

#[test]
fn frames_round_trip_including_empty_and_largest() {
    let n = name("echo");
    echo(PipeServer::with_name(Some(n.clone())).unwrap(), Duration::ZERO);
    let c = Client::connect(&n, None, 0).unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(c.server_pid, std::process::id());
    for len in [0usize, 1, 4, 1000, twime_proto::MAX_FRAME] {
        let p: Vec<u8> = (0..len).map(|i| i as u8).collect();
        assert_eq!(c.call(&p, 2000).unwrap(), p);
    }
    assert!(c.alive());
    assert!(matches!(c.call(&vec![0; twime_proto::MAX_FRAME + 1], 2000), Err(CallError::Broken(_))));
}

#[test]
fn second_server_is_refused_and_a_stale_socket_file_is_replaced() {
    let _turn = NO_SPAWN_WHILE_A_NAME_IS_REUSED.lock().unwrap_or_else(|e| e.into_inner());
    let n = name("single");
    let first = PipeServer::with_name(Some(n.clone())).unwrap();
    assert_eq!(PipeServer::with_name(Some(n.clone())).err().unwrap().kind(), std::io::ErrorKind::AddrInUse);
    // A crashed engine leaves the socket file behind (no Drop): simulate it by forgetting the listener's cleanup.
    drop(first);
    std::os::unix::net::UnixListener::bind(&n).map(std::mem::forget).unwrap();
    assert!(std::path::Path::new(&n).exists());
    echo(PipeServer::with_name(Some(n.clone())).unwrap(), Duration::ZERO);
    let c = Client::connect(&n, None, 0).unwrap();
    assert_eq!(c.call(b"x", 2000).unwrap(), b"x");
}

#[test]
fn missing_socket_fails_at_once_and_a_slow_reply_times_out() {
    let t = Instant::now();
    assert_eq!(Client::connect(&name("missing"), None, 5000).err().unwrap().kind(), std::io::ErrorKind::NotFound);
    assert!(t.elapsed() < Duration::from_secs(1));
    let n = name("slow");
    echo(PipeServer::with_name(Some(n.clone())).unwrap(), Duration::from_millis(600));
    let c = Client::connect(&n, None, 0).unwrap();
    let t = Instant::now();
    assert!(matches!(c.call(b"x", 100), Err(CallError::Timeout)));
    assert!(t.elapsed() < Duration::from_millis(500));
}

#[test]
fn client_notices_a_server_that_went_away() {
    let n = name("gone");
    let server = PipeServer::with_name(Some(n.clone())).unwrap();
    let c = Client::connect(&n, None, 0).unwrap();
    let conn = server.accept().unwrap();
    assert!(c.alive());
    drop(conn);
    drop(server);
    assert!(!c.alive());
    assert!(matches!(c.call(b"x", 500), Err(CallError::Broken(_))));
}

#[test]
fn a_frame_arriving_byte_by_byte_is_read_whole_and_an_oversized_header_ends_the_connection() {
    use std::io::{Read, Write};
    let n = name("frag");
    echo(PipeServer::with_name(Some(n.clone())).unwrap(), Duration::ZERO);
    let mut s = std::os::unix::net::UnixStream::connect(&n).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    for b in twime_proto::frame(b"hello").unwrap() {
        s.write_all(&[b]).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut back = [0u8; 9];
    s.read_exact(&mut back).unwrap();
    assert_eq!(&back[4..], b"hello");
    s.write_all(&((twime_proto::MAX_FRAME + 1) as u32).to_le_bytes()).unwrap();
    assert_eq!(s.read(&mut back).unwrap(), 0); // the server gave the connection up
}

#[test]
fn a_path_that_is_not_a_socket_is_left_alone() {
    let n = name("file");
    std::fs::write(&n, b"keep").unwrap();
    assert_eq!(PipeServer::with_name(Some(n.clone())).err().unwrap().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&n).unwrap(), b"keep");
}

/// The only test that changes the environment of this process (bare names; the others use explicit paths).
#[cfg(not(target_os = "macos"))]
#[test]
fn the_default_directory_must_be_private_and_a_relative_runtime_dir_is_ignored() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("u-rt-{}", std::process::id()));
    let dir = base.join("twpotato");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", &base) };
    assert_eq!(twime_winpipe::qualify("engine").err().unwrap().kind(), std::io::ErrorKind::PermissionDenied);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(twime_winpipe::qualify("engine").unwrap(), dir.join("engine.sock"));
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", "relative/dir") };
    assert!(twime_winpipe::qualify("engine").unwrap().starts_with(std::env::temp_dir()));
    unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
}

/// macOS: the socket directory is `$HOME/Library/Caches/TwPotato`, whatever `XDG_RUNTIME_DIR` says; it must be private;
/// a home directory too long for a socket address is an error, not another place. `HOME` is process-wide, so each case
/// runs in a process of its own (this test binary run again).
#[cfg(target_os = "macos")]
#[test]
fn macos_socket_directory_follows_home_and_a_path_too_long_is_refused() {
    use std::{io::ErrorKind, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};
    const VAR: &str = "TWIME_TEST_MACOS_HOME_CASE";
    if let Ok(case) = std::env::var(VAR) {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let dir = home.join("Library/Caches/TwPotato");
        match case.as_str() {
            "directory" => {
                assert_eq!(twime_winpipe::qualify("engine").unwrap(), dir.join("engine.sock"));
                assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
                assert_eq!(twime_winpipe::socket_path().unwrap(), dir.join("engine.sock"));
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
                assert_eq!(twime_winpipe::qualify("engine").err().unwrap().kind(), ErrorKind::PermissionDenied);
            }
            "long" => {
                // The directory itself is fine; only the address does not fit (104 bytes with the final zero).
                assert!(twime_winpipe::qualify("engine").unwrap().as_os_str().len() >= 104);
                assert_eq!(PipeServer::with_name(Some("engine".into())).err().unwrap().kind(), ErrorKind::InvalidInput);
                assert_eq!(Client::connect("engine", None, 0).err().unwrap().kind(), ErrorKind::InvalidInput);
                assert_eq!(Client::connect_nowait("engine").err().unwrap().kind(), ErrorKind::InvalidInput);
            }
            "no-caches" => assert_eq!(twime_winpipe::qualify("engine").err().unwrap().kind(), ErrorKind::NotFound),
            other => panic!("{other}"),
        }
        return;
    }
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("u-home-{}", std::process::id()));
    let _turn = NO_SPAWN_WHILE_A_NAME_IS_REUSED.lock().unwrap_or_else(|e| e.into_inner());
    for (case, home) in [("directory", base.join("d")), ("long", base.join("l".repeat(100))), ("no-caches", base.join("n"))] {
        std::fs::create_dir_all(if case == "no-caches" { home.clone() } else { home.join("Library/Caches") }).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "macos_socket_directory_follows_home_and_a_path_too_long_is_refused", "--test-threads", "1"])
            .env(VAR, case)
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", base.join("xdg"))
            .status()
            .unwrap();
        assert!(status.success(), "case {case}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// Several processes ask for the same socket at once (this test binary run again, four times): exactly one gets it and
/// the others are refused, as we are while the winner lives. Once the winner is killed (SIGKILL: no cleanup runs, the
/// socket file stays) the next server takes over.
#[test]
fn processes_racing_for_one_socket_leave_one_winner_and_a_killed_winner_is_replaced() {
    use std::process::{Command, Stdio};
    const VAR: &str = "TWIME_TEST_RACE_SOCKET";
    const REFUSED: i32 = 3;
    if let Ok(n) = std::env::var(VAR) {
        match PipeServer::with_name(Some(n)) {
            Ok(server) => echo(server, Duration::ZERO),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => std::process::exit(REFUSED),
            Err(e) => panic!("{e}"),
        }
        std::thread::sleep(Duration::from_secs(120)); // the parent kills us long before
        return;
    }
    /// Whatever fails below, no child is left holding the socket.
    struct Reap(Vec<std::process::Child>);
    impl Drop for Reap {
        fn drop(&mut self) {
            for c in &mut self.0 {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
    let n = name("race");
    let mut children = Reap(Vec::new());
    let turn = NO_SPAWN_WHILE_A_NAME_IS_REUSED.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..4 {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "processes_racing_for_one_socket_leave_one_winner_and_a_killed_winner_is_replaced", "--test-threads", "1"])
            .env(VAR, &n)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        children.0.push(child);
    }
    drop(turn);
    // The three losers exit with REFUSED; the winner keeps serving.
    let until = Instant::now() + Duration::from_secs(60);
    let mut refused = 0;
    while children.0.len() > 1 {
        assert!(Instant::now() < until, "{} processes still hold or wait for the socket", children.0.len());
        let mut i = 0;
        while i < children.0.len() {
            match children.0[i].try_wait().unwrap() {
                Some(status) => {
                    assert_eq!(status.code(), Some(REFUSED));
                    refused += 1;
                    children.0.swap_remove(i);
                }
                None => i += 1,
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(refused, 3);
    let winner = &mut children.0[0];
    let c = loop {
        if let Ok(c) = Client::connect(&n, None, 0) { break c }
        assert!(winner.try_wait().unwrap().is_none(), "the winner exited");
        assert!(Instant::now() < until, "the winner never served");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(c.server_pid, if cfg!(target_os = "linux") { winner.id() } else { 0 });
    assert_eq!(c.call(b"x", 2000).unwrap(), b"x");
    assert_eq!(PipeServer::with_name(Some(n.clone())).err().unwrap().kind(), std::io::ErrorKind::AddrInUse);
    winner.kill().unwrap(); // SIGKILL
    winner.wait().unwrap();
    assert!(std::path::Path::new(&n).exists(), "a killed server leaves its socket file");
    assert!(!c.alive());
    echo(PipeServer::with_name(Some(n.clone())).unwrap(), Duration::ZERO);
    assert_eq!(Client::connect(&n, None, 0).unwrap().call(b"y", 2000).unwrap(), b"y");
}

#[test]
fn connect_nowait_works_like_connect() {
    let n = name("nowait");
    assert_eq!(Client::connect_nowait(&n).err().unwrap().kind(), std::io::ErrorKind::NotFound, "no socket");
    echo(PipeServer::with_name(Some(n.clone())).unwrap(), Duration::ZERO);
    let c = Client::connect_nowait(&n).unwrap();
    assert_eq!(c.server_pid, if cfg!(target_os = "linux") { std::process::id() } else { 0 });
    assert_eq!(c.call(b"x", 2000).unwrap(), b"x");
}

/// Linux: a full queue answers `WouldBlock` at once. (Other systems may refuse such a connection instead; not asserted.)
#[cfg(target_os = "linux")]
#[test]
fn connect_nowait_never_waits_for_a_full_queue() {
    use std::os::unix::{io::AsRawFd, net::UnixListener};
    // A listener nobody accepts from, with room for one waiting connection.
    let full = name("nowait-full");
    let listener = UnixListener::bind(&full).unwrap();
    assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);
    let mut held = vec![];
    let t = Instant::now();
    let refused = loop {
        match Client::connect_nowait(&full) {
            Ok(c) => held.push(c),
            Err(e) => break e,
        }
        assert!(held.len() < 100, "the queue never filled");
    };
    assert_eq!(refused.kind(), std::io::ErrorKind::WouldBlock);
    assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
    drop(listener);
    let _ = std::fs::remove_file(&full);
}
