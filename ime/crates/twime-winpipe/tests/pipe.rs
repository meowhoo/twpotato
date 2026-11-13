#![cfg(windows)]

use std::time::{Duration, Instant};

use twime_winpipe::{CallError, Client, PipeServer, qualify};

fn name(tag: &str) -> String {
    format!("twime-test-{tag}-{}", std::process::id())
}

#[test]
fn echo_round_trip_and_second_server_is_refused() {
    let n = name("echo");
    let server = PipeServer::with_name(Some(n.clone())).unwrap();
    // The name is owned by the first instance: a second server must fail (FILE_FLAG_FIRST_PIPE_INSTANCE).
    assert!(PipeServer::with_name(Some(n.clone())).is_err());
    std::thread::spawn(move || {
        while let Ok(conn) = server.accept() {
            std::thread::spawn(move || {
                while let Ok(Some(p)) = conn.read() {
                    if conn.write(&p).is_err() {
                        break;
                    }
                }
            });
        }
    });
    let c = Client::connect(&n, None, 200).unwrap();
    for len in [0usize, 1, 100, 60_000] {
        let msg = vec![7u8; len];
        assert_eq!(c.call(&msg, 2000).unwrap(), msg);
    }
    // Over the frame limit is refused before sending.
    assert!(matches!(c.call(&vec![0u8; 70_000], 2000), Err(CallError::Broken(_))));
    // A second client works while the first one is connected.
    let c2 = Client::connect(&n, None, 200).unwrap();
    assert_eq!(c2.call(b"x", 2000).unwrap(), b"x");
    // The image path check: our own path is not the server's... it is (same process), a different path must fail.
    let me = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    assert!(Client::connect(&n, Some(&me), 200).is_ok());
    assert!(Client::connect(&n, Some(r"C:\Windows\System32\notepad.exe"), 200).is_err());
    assert_eq!(qualify("a"), r"\\.\pipe\a");
}

#[test]
fn silent_server_times_out_and_missing_pipe_is_reported() {
    let n = name("silent");
    let server = PipeServer::with_name(Some(n.clone())).unwrap();
    std::thread::spawn(move || {
        if let Ok(conn) = server.accept() {
            let _ = conn.read(); // takes the request and never answers
            std::thread::sleep(Duration::from_secs(5));
        }
    });
    let c = Client::connect(&n, None, 200).unwrap();
    let t = Instant::now();
    assert!(matches!(c.call(b"ping", 30), Err(CallError::Timeout)));
    assert!(t.elapsed() < Duration::from_millis(500));
    let e = Client::connect(&name("nobody"), None, 0).err().unwrap();
    assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn write_notification_precedes_reply_and_invalid_frames_do_not_notify() {
    let n = name("written");
    let server = PipeServer::with_name(Some(n.clone())).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let conn = server.accept().unwrap();
        assert_eq!(conn.read().unwrap().unwrap(), b"request");
        // No reply until the write-completion callback has run.
        let written = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(written <= Instant::now());
        conn.write(b"reply").unwrap();
    });
    let client = Client::connect(&n, None, 200).unwrap();
    let before = Instant::now();
    let reply = client.call_with_written(b"request", 3000, |written| {
        assert!(written >= before);
        tx.send(written).unwrap();
    }).unwrap();
    assert_eq!(reply, b"reply");
    worker.join().unwrap();
    let mut notified = false;
    assert!(matches!(client.call_with_written(&vec![0; 70_000], 3000, |_| notified = true), Err(CallError::Broken(_))));
    assert!(!notified);
}
