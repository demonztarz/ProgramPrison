//! Pretends to be the Discord app (a fake Discord socket) and checks what the background
//! blocker shows: a status while a block is on, nothing once it's stopped or switched off.
#![cfg(unix)]

use programprison::discord::encode;
use programprison::ipc::{Client, Request};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_programprison");

fn read_frame(s: &mut UnixStream) -> Option<(u32, Value)> {
    let mut head = [0u8; 8];
    s.read_exact(&mut head).ok()?;
    let op = u32::from_le_bytes(head[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).ok()?;
    Some((op, serde_json::from_slice(&body).unwrap()))
}

/// A fake Discord: answers the handshake and every SET_ACTIVITY, and reports what it got.
fn fake_discord(listener: UnixListener, tx: mpsc::Sender<Value>) {
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut s) = conn else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                let Some((0, hello)) = read_frame(&mut s) else { return };
                let _ = tx.send(json!({ "handshake": hello }));
                let ready = json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "user": { "username": "tester" } } });
                s.write_all(&encode(1, &ready)).unwrap();
                while let Some((op, msg)) = read_frame(&mut s) {
                    if op != 1 {
                        break;
                    }
                    let reply = json!({ "cmd": "SET_ACTIVITY", "nonce": msg["nonce"], "data": msg["args"]["activity"] });
                    s.write_all(&encode(1, &reply)).unwrap();
                    let _ = tx.send(json!({ "activity": msg["args"]["activity"] }));
                }
                let _ = tx.send(json!({ "closed": true }));
            });
        }
    });
}

fn next(rx: &mpsc::Receiver<Value>, what: &str, f: impl Fn(&Value) -> bool) -> Value {
    let end = Instant::now() + Duration::from_secs(15);
    while Instant::now() < end {
        if let Ok(v) = rx.recv_timeout(Duration::from_millis(200))
            && f(&v)
        {
            return v;
        }
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn shows_running_blocks_on_discord() {
    let dir = std::env::temp_dir().join(format!("pp-discord-{}-{}", std::process::id(), programprison::engine::now_ms()));
    std::fs::create_dir_all(dir.join("data")).unwrap();
    let hosts = dir.join("hosts");
    std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
    let listener = UnixListener::bind(dir.join("discord-ipc-0")).unwrap();
    let (tx, rx) = mpsc::channel();
    fake_discord(listener, tx);
    unsafe {
        std::env::set_var("PROGRAMPRISON_DATA", dir.join("data"));
    }
    let mut svc = Command::new(BIN)
        .arg("--service")
        .env("PROGRAMPRISON_DATA", dir.join("data"))
        .env("PROGRAMPRISON_HOSTS", &hosts)
        .env("PROGRAMPRISON_TICK_MS", "300")
        .env("XDG_RUNTIME_DIR", &dir)
        .env("PROGRAMPRISON_DISCORD_CLIENT_ID", "123456789012345678")
        .spawn()
        .unwrap();
    let result = std::panic::catch_unwind(|| {
        let end = Instant::now() + Duration::from_secs(10);
        while Client::connect().is_none() {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(100));
        }
        let call = |r: Request| Client::connect().unwrap().send(&r).unwrap();

        eprintln!("• connects as soon as Discord is running, before any block");
        // (so a block that later cuts off discord.com can't stop Discord recognising it)
        let hello = next(&rx, "handshake", |v| v.get("handshake").is_some());
        assert_eq!(hello["handshake"]["client_id"], "123456789012345678");
        next(&rx, "empty status", |v| v.get("activity").is_some_and(|a| a.is_null()));
        let status = call(Request::GetState).state.unwrap().discord_status;
        assert!(status.contains("ready to show your next block"), "{status}");

        eprintln!("• a block shows up over that same connection, and ending it keeps the connection");
        let id = call(Request::CreateBlock { name: "Deep work".into() }).id.unwrap();
        call(Request::UpdateBlock { id: id.clone(), edit: programprison::engine::Edit { sites: Some(vec!["reddit.com".into(), "x.com".into()]), ..Default::default() } });
        call(Request::StartBlock { id: id.clone(), ends_at: programprison::engine::now_ms() + 20 * 60_000, minutes: None });
        let a = next(&rx, "activity", |v| v.get("activity").is_some_and(|a| !a.is_null()))["activity"].clone();
        assert_eq!(a["details"], "Focusing: Deep work");
        call(Request::StopBlock { id: id.clone(), answer: String::new() });
        next(&rx, "status cleared", |v| v.get("activity").is_some_and(|a| a.is_null()));
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if let Ok(v) = rx.recv_timeout(Duration::from_millis(200)) {
                assert!(v.get("handshake").is_none() && v.get("closed").is_none(), "stayed connected: {v}");
            }
        }

        eprintln!("• a locked block shows as Locked in, with Discord counting down to the end");
        call(Request::SetLock { id: id.clone(), lock: programprison::engine::LockRequest { kind: "locked".into(), length: 0, password: String::new() } });
        let ends = programprison::engine::now_ms() + 45 * 60_000;
        call(Request::StartBlock { id: id.clone(), ends_at: ends, minutes: None });
        let a = next(&rx, "activity", |v| v.get("activity").is_some_and(|a| !a.is_null()))["activity"].clone();
        assert_eq!(a["details"], "Locked in: Deep work");
        assert_eq!(a["state"], "Blocking 2 websites · 45 min left");
        assert_eq!(a["timestamps"]["end"], ends);
        assert_eq!(a["type"], 5);
        assert_eq!(a["assets"]["large_text"], "Program Prison");
        let status = call(Request::GetState).state.unwrap().discord_status;
        assert!(status.contains("Showing on Discord"), "{status}");

        eprintln!("• switching it off clears the status and disconnects");
        call(Request::UpdateSettings { start_at_login: None, discord: Some(false) });
        next(&rx, "status cleared", |v| v.get("activity").is_some_and(|a| a.is_null()));
        next(&rx, "disconnected", |v| v.get("closed").is_some());
        assert_eq!(call(Request::GetState).state.unwrap().discord_status, "Off.");

        eprintln!("• switching it back on reconnects and shows it again");
        call(Request::UpdateSettings { start_at_login: None, discord: Some(true) });
        next(&rx, "handshake again", |v| v.get("handshake").is_some());
        next(&rx, "activity again", |v| v.get("activity").is_some_and(|a| a["details"] == "Locked in: Deep work"));
    });
    let _ = svc.kill();
    let _ = svc.wait();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
    eprintln!("all Discord checks passed");
}
