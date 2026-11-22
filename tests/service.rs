//! Starts the real background blocker and drives it the way the window does: blocks a website
//! and a program, locks, unlocks, the quit guard, and a forced restart.
//! Uses a scratch hosts file and data folder, never the real ones.
//! On Windows, Notepad is covered instead of closed, and the cover must follow it when moved.

use programprison::engine::{App, AppMode, Edit, LockRequest};
use programprison::ipc::{Challenge, Client, PublicState, Request, Response};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_programprison");

struct Env {
    dir: PathBuf,
    hosts: PathBuf,
    log: PathBuf,
}

impl Env {
    fn new() -> Env {
        let dir = std::env::temp_dir().join(format!("pp-e2e-{}-{}", std::process::id(), programprison::engine::now_ms()));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let hosts = dir.join("hosts");
        std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
        let log = dir.join("debug.log");
        // The client in this process finds the service through the same data folder.
        unsafe {
            std::env::set_var("PROGRAMPRISON_DATA", dir.join("data"));
        }
        Env { dir, hosts, log }
    }

    fn start(&self) -> Child {
        let _ = std::fs::remove_file(self.dir.join("data").join("service.json"));
        let child = Command::new(BIN)
            .arg("--service")
            .env("PROGRAMPRISON_DATA", self.dir.join("data"))
            .env("PROGRAMPRISON_HOSTS", &self.hosts)
            .env("PROGRAMPRISON_DEBUG", &self.log)
            .env("PROGRAMPRISON_TICK_MS", "300")
            .env("PROGRAMPRISON_DISCORD_CLIENT_ID", "") // never show test blocks on a real Discord
            .spawn()
            .expect("start service");
        wait_for("service to answer", 10, || Client::connect().is_some());
        child
    }

    fn hosts(&self) -> String {
        std::fs::read_to_string(&self.hosts).unwrap()
    }

    fn dump_log(&self) {
        if let Ok(t) = std::fs::read_to_string(&self.log) {
            let lines: Vec<&str> = t.lines().collect();
            eprintln!("--- service log (last 60 lines) ---\n{}", lines[lines.len().saturating_sub(60)..].join("\n"));
        }
    }
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn call(req: Request) -> Response {
    Client::connect().expect("service running").send(&req).expect("request sent")
}

fn ok(req: Request) -> Response {
    let r = call(req);
    assert!(r.ok, "request failed: {:?}", r.error);
    r
}

fn state() -> PublicState {
    ok(Request::GetState).state.unwrap()
}

fn step(name: &str) {
    eprintln!("• {name}");
}

/// A long-running program with a chosen name that Program Prison should block.
fn start_program(dir: &Path) -> (Child, &'static str) {
    if cfg!(windows) {
        (Command::new("notepad.exe").spawn().unwrap(), "notepad")
    } else {
        let exe = dir.join("timewaster");
        // A script's process takes the script's name (renamed coreutils binaries may refuse to run).
        if cfg!(target_os = "linux") {
            std::fs::write(&exe, "#!/bin/sh\nwhile :; do sleep 1; done\n").unwrap();
        } else {
            std::fs::copy("/bin/sleep", &exe).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (Command::new(&exe).arg("120").spawn().unwrap(), "timewaster")
    }
}

#[test]
fn end_to_end() {
    let env = Env::new();
    let mut svc = env.start();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&env, &mut svc)));
    if result.is_err() {
        env.dump_log();
    }
    let _ = svc.kill();
    if cfg!(windows) {
        let _ = Command::new("taskkill").args(["/F", "/IM", "notepad.exe"]).output();
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

fn run(env: &Env, svc: &mut Child) {
    step("a second copy refuses to start while one is running");
    let second = Command::new(BIN).arg("--service").env("PROGRAMPRISON_DATA", env.dir.join("data")).env("PROGRAMPRISON_HOSTS", &env.hosts).status().unwrap();
    assert!(second.success());
    assert!(Client::connect().is_some(), "the first copy still answers");

    step("opening the window again asks the open one to come to the front");
    let before = state().show_requests;
    ok(Request::ShowWindow);
    assert_eq!(state().show_requests, before + 1);

    step("create a block and add websites (links are cleaned, junk dropped)");
    let id = ok(Request::CreateBlock { name: "Deep work".into() }).id.unwrap();
    let r = ok(Request::UpdateBlock { id: id.clone(), edit: Edit { sites: Some(vec!["https://www.reddit.com/r/all".into(), "not a site".into(), "youtube.com".into()]), ..Default::default() } });
    assert_eq!(r.state.unwrap().blocks[0].sites, vec!["reddit.com", "youtube.com"]);

    step("add a program; system programs are dropped");
    let (mut prog, prog_name) = start_program(&env.dir);
    let mode = if cfg!(windows) { AppMode::Cover } else { AppMode::Close };
    let r = ok(Request::UpdateBlock {
        id: id.clone(),
        edit: Edit { apps: Some(vec![App { name: format!("{prog_name}.exe"), mode }, App { name: "explorer.exe".into(), mode: AppMode::Close }]), ..Default::default() },
    });
    assert_eq!(r.state.unwrap().blocks[0].apps, vec![App { name: prog_name.into(), mode }]);
    assert_eq!(state().cover_supported, cfg!(windows));

    step("start for 30 minutes: hosts file written");
    std::thread::sleep(Duration::from_millis(if cfg!(windows) { 1500 } else { 300 }));
    let now = programprison::engine::now_ms();
    ok(Request::StartBlock { id: id.clone(), ends_at: now + 30 * 60_000 });
    wait_for("hosts entries", 5, || env.hosts().contains("0.0.0.0 reddit.com") && env.hosts().contains("0.0.0.0 youtu.be"));
    let s = state();
    assert!(s.blocks[0].status.active && !s.blocks[0].status.locked);

    if cfg!(windows) {
        step("Notepad is covered, not closed, and the cover follows it");
        #[cfg(windows)]
        win::check_cover(&mut prog);
    } else {
        step("the program is closed");
        wait_for("program closed", 5, || prog.try_wait().unwrap().is_some());
        wait_for("closed count", 3, || state().closed_today >= 1);
    }

    step("hand edits to the hosts file are undone");
    std::fs::write(&env.hosts, "127.0.0.1 localhost\n").unwrap();
    wait_for("hosts section restored", 15, || env.hosts().contains("0.0.0.0 reddit.com"));

    step("random-text lock: loosening refused, adding allowed, typing the text stops it");
    ok(Request::SetLock { id: id.clone(), lock: LockRequest { kind: "random".into(), length: 20, password: String::new() } });
    assert!(state().blocks[0].status.locked);
    let r = call(Request::UpdateBlock { id: id.clone(), edit: Edit { sites: Some(vec![]), ..Default::default() } });
    assert!(!r.ok && r.error.unwrap().contains("locked"));
    ok(Request::UpdateBlock { id: id.clone(), edit: Edit { sites: Some(vec!["reddit.com".into(), "youtube.com".into(), "x.com".into()]), ..Default::default() } });
    assert!(!call(Request::DeleteBlock { id: id.clone() }).ok);
    assert!(!call(Request::SetLock { id: id.clone(), lock: LockRequest { kind: "none".into(), length: 0, password: String::new() } }).ok);
    assert!(!call(Request::Quit).ok, "quitting refused while locked");
    let Some(Challenge::Random { text, .. }) = ok(Request::Challenge { id: id.clone() }).challenge else { panic!("expected a random-text challenge") };
    // Asking again gives the same chain, with the typing progress the window saved.
    ok(Request::WordProgress { id: id.clone(), done: 7 });
    let Some(Challenge::Random { text: again, done }) = ok(Request::Challenge { id: id.clone() }).challenge else { panic!() };
    assert_eq!(again, text, "same chain until it's typed");
    assert_eq!(done, 7, "progress kept");
    assert_eq!(text.split(' ').count(), 20, "20 words");
    let r = call(Request::StopBlock { id: id.clone(), answer: "wrong".into() });
    assert!(!r.ok && r.error.unwrap().contains("don't match"));
    let Some(Challenge::Random { text, .. }) = ok(Request::Challenge { id: id.clone() }).challenge else { panic!() };
    // Missing a word is refused; capitals and extra spaces are fine.
    let short: Vec<&str> = text.split(' ').skip(1).collect();
    assert!(!call(Request::StopBlock { id: id.clone(), answer: short.join(" ") }).ok);
    let Some(Challenge::Random { text, .. }) = ok(Request::Challenge { id: id.clone() }).challenge else { panic!() };
    ok(Request::StopBlock { id: id.clone(), answer: text.to_uppercase().replace(' ', "  \n ") });
    assert!(!state().blocks[0].status.active);
    wait_for("hosts section removed", 5, || !env.hosts().contains("reddit"));
    #[cfg(windows)]
    wait_for("covers removed", 5, || call(Request::Covers).covers.is_some_and(|c| c.is_empty()));

    step("schedule: painted hours switch the block on");
    let mut sched = vec![true; 168];
    ok(Request::UpdateBlock { id: id.clone(), edit: Edit { schedule: Some(sched.clone()), ..Default::default() } });
    let s = state();
    assert!(s.blocks[0].status.active && s.blocks[0].status.forever);
    wait_for("hosts entries from schedule", 5, || env.hosts().contains("0.0.0.0 x.com"));
    sched.iter_mut().for_each(|v| *v = false);
    let r = call(Request::UpdateBlock { id: id.clone(), edit: Edit { schedule: Some(sched.clone()), ..Default::default() } });
    assert!(!r.ok, "random lock: scheduled hours can't be removed while on");
    let Some(Challenge::Random { text, .. }) = ok(Request::Challenge { id: id.clone() }).challenge else { panic!() };
    ok(Request::StopBlock { id: id.clone(), answer: text });
    assert!(state().blocks[0].status.unlocked_until.is_some(), "skipping the scheduled run");
    ok(Request::UpdateBlock { id: id.clone(), edit: Edit { schedule: Some(sched), ..Default::default() } });

    step("password lock: wrong password refused, right one stops it");
    ok(Request::SetLock { id: id.clone(), lock: LockRequest { kind: "password".into(), length: 0, password: "letmeout".into() } });
    let saved = std::fs::read_to_string(env.dir.join("data").join("state.json")).unwrap();
    assert!(!saved.contains("letmeout"), "password not stored in plain text");
    ok(Request::StartBlock { id: id.clone(), ends_at: programprison::engine::now_ms() + 15 * 60_000 });
    assert_eq!(ok(Request::Challenge { id: id.clone() }).challenge, Some(Challenge::Password));
    assert!(!call(Request::StopBlock { id: id.clone(), answer: "nope".into() }).ok);
    ok(Request::StopBlock { id: id.clone(), answer: "letmeout".into() });

    step("Locked: no stop, no delete, no quitting");
    ok(Request::SetLock { id: id.clone(), lock: LockRequest { kind: "locked".into(), length: 0, password: String::new() } });
    ok(Request::StartBlock { id: id.clone(), ends_at: programprison::engine::now_ms() + 60 * 60_000 });
    assert!(matches!(ok(Request::Challenge { id: id.clone() }).challenge, Some(Challenge::Locked { .. })));
    assert!(!call(Request::StopBlock { id: id.clone(), answer: String::new() }).ok);
    assert!(!call(Request::DeleteBlock { id: id.clone() }).ok);
    assert!(!call(Request::Quit).ok);
    assert!(!call(Request::UpdateSettings { start_at_login: Some(false), discord: None }).ok);
    std::thread::sleep(Duration::from_millis(500));
    assert!(svc.try_wait().unwrap().is_none(), "still running");

    step("a forced restart keeps the state and blocking resumes");
    svc.kill().unwrap();
    svc.wait().unwrap();
    std::fs::write(&env.hosts, "127.0.0.1 localhost\n").unwrap();
    *svc = env.start();
    let s = state();
    assert_eq!(s.blocks.len(), 1);
    assert!(s.blocks[0].status.locked);
    wait_for("hosts restored after restart", 5, || env.hosts().contains("0.0.0.0 reddit.com"));

    let _ = prog.kill();
    eprintln!("all end-to-end checks passed");
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, MoveWindow};

    fn rect(target: i64) -> RECT {
        let h = HWND(target as isize as *mut _);
        let mut r = RECT::default();
        unsafe {
            if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut r as *mut RECT).cast(), 16).is_err() {
                let _ = GetWindowRect(h, &mut r);
            }
        }
        let _ = LPARAM(0);
        r
    }

    fn matches(c: &programprison::ipc::CoverInfo) -> bool {
        let r = rect(c.target);
        (c.x - r.left).abs() <= 2 && (c.y - r.top).abs() <= 2 && (c.width - (r.right - r.left)).abs() <= 2 && (c.height - (r.bottom - r.top)).abs() <= 2
    }

    pub fn check_cover(prog: &mut Child) {
        let mut covers = vec![];
        wait_for("a visible cover over Notepad", 15, || {
            covers = call(Request::Covers).covers.unwrap_or_default();
            covers.iter().any(|c| c.visible && matches(c))
        });
        assert!(prog.try_wait().unwrap().is_none(), "Notepad is covered, not closed");
        let target = covers.iter().find(|c| c.visible).unwrap().target;
        let r = rect(target);
        unsafe {
            MoveWindow(HWND(target as isize as *mut _), r.left + 120, r.top + 80, 700, 500, true).unwrap();
        }
        wait_for("the cover to follow the moved window", 5, || {
            call(Request::Covers).covers.unwrap_or_default().iter().any(|c| c.target == target && c.visible && matches(c) && c.width > 600)
        });
        eprintln!("  cover follows: {:?}", call(Request::Covers).covers);
    }
}
