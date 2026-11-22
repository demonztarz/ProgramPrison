//! Discord Rich Presence: while a block is on, your Discord profile shows
//! "Playing Program Prison · Locked in: Deep work · 1:23:45 left".
//!
//! Talks to the Discord desktop app over its local connection (a named pipe on Windows, a
//! socket elsewhere). Runs on its own thread: if Discord is closed, slow or frozen, blocking is
//! never affected. Discord counts the time down itself, so an update is only sent when what's
//! shown changes.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The Discord application whose name ("Program Prison") appears on profiles. Created at
/// https://discord.com/developers/applications. PROGRAMPRISON_DISCORD_CLIENT_ID overrides it.
pub const CLIENT_ID: &str = "1573937020118761514";

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;
const RETRY: Duration = Duration::from_secs(30); // Discord may be started after Program Prison
const SETTLE: Duration = Duration::from_millis(1500); // Discord rate-limits updates; send bursts once

pub fn client_id() -> String {
    std::env::var("PROGRAMPRISON_DISCORD_CLIENT_ID").unwrap_or_else(|_| CLIENT_ID.to_string()).trim().to_string()
}

fn valid_id(id: &str) -> bool {
    (5..=30).contains(&id.len()) && id.chars().all(|c| c.is_ascii_digit())
}

/// Builds the frame Discord expects: op code and length (little-endian), then JSON.
pub fn encode(op: u32, payload: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(payload).unwrap_or_default();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn read_frame(r: &mut impl Read) -> std::io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let op = u32::from_le_bytes(head[0..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok((op, serde_json::from_slice(&body).unwrap_or(Value::Null)))
}

// ---------- the connection ----------

trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

/// Where the Discord desktop app listens: discord-ipc-0 to 9.
pub fn endpoints() -> Vec<std::path::PathBuf> {
    let ids = 0..10;
    if cfg!(windows) {
        return ids.map(|i| std::path::PathBuf::from(format!(r"\\.\pipe\discord-ipc-{i}"))).collect();
    }
    let base = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(std::env::var_os)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "/tmp".into());
    // the regular app, then the Flatpak and Snap versions
    let dirs = [base.clone(), base.join("app/com.discordapp.Discord"), base.join(".flatpak/com.discordapp.Discord/xdg-run"), base.join("snap.discord")];
    dirs.iter().flat_map(|d| ids.clone().map(move |i| d.join(format!("discord-ipc-{i}")))).collect()
}

fn open(path: &std::path::Path) -> std::io::Result<Box<dyn Stream>> {
    #[cfg(unix)]
    {
        let s = std::os::unix::net::UnixStream::connect(path)?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        s.set_write_timeout(Some(Duration::from_secs(10)))?;
        Ok(Box::new(s))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(std::fs::OpenOptions::new().read(true).write(true).open(path)?))
    }
}

/// Requests and replies one at a time (Discord answers every request), so one handle is
/// enough on every system.
struct Conn {
    s: Box<dyn Stream>,
}

impl Conn {
    fn connect(id: &str) -> Result<(Conn, String), String> {
        let mut errors = Vec::new();
        for p in endpoints() {
            let s = match open(&p) {
                Ok(s) => s,
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        errors.push(e.to_string());
                    }
                    continue;
                }
            };
            let mut c = Conn { s };
            c.s.write_all(&encode(OP_HANDSHAKE, &json!({ "v": 1, "client_id": id }))).map_err(|e| e.to_string())?;
            loop {
                let (op, msg) = c.read()?;
                match op {
                    OP_FRAME if msg["evt"] == "READY" => {
                        let user = msg["data"]["user"]["username"].as_str().unwrap_or("").to_string();
                        return Ok((c, user));
                    }
                    OP_CLOSE => return Err(format!("Discord refused the connection: {} (code {}).", msg["message"].as_str().unwrap_or("no reason given"), msg["code"])),
                    _ => {}
                }
            }
        }
        Err(if errors.is_empty() {
            "Discord isn't running (the Discord desktop app is needed; the browser version can't show statuses).".into()
        } else {
            format!("Discord was found but couldn't be reached ({}). If one of Program Prison and Discord runs as administrator and the other doesn't, run both normally.", errors.join("; "))
        })
    }

    fn read(&mut self) -> Result<(u32, Value), String> {
        loop {
            let (op, msg) = read_frame(&mut self.s).map_err(|e| format!("Discord closed the connection ({e})."))?;
            if op == OP_PING {
                self.s.write_all(&encode(OP_PONG, &msg)).map_err(|e| e.to_string())?;
                continue;
            }
            return Ok((op, msg));
        }
    }

    fn set_activity(&mut self, activity: &Option<Value>) -> Result<(), String> {
        let nonce = format!("{}-{}", crate::engine::now_ms(), std::process::id());
        let req = json!({ "cmd": "SET_ACTIVITY", "args": { "pid": std::process::id(), "activity": activity }, "nonce": nonce });
        self.s.write_all(&encode(OP_FRAME, &req)).map_err(|e| format!("Discord closed the connection ({e})."))?;
        loop {
            let (op, msg) = self.read()?;
            if op == OP_CLOSE {
                return Err("Discord closed the connection.".into());
            }
            if msg["nonce"] == nonce || msg["cmd"] == "SET_ACTIVITY" {
                if msg["evt"] == "ERROR" {
                    return Err(format!("Discord rejected the status: {}", msg["data"]["message"].as_str().unwrap_or("unknown error")));
                }
                return Ok(());
            }
        }
    }
}

// ---------- the thread ----------

/// Shows and clears the status from a background thread.
pub struct Presence {
    tx: Sender<Option<Value>>,
    status: Arc<Mutex<String>>,
    last: Option<Option<Value>>,
}

impl Presence {
    pub fn start() -> Presence {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(String::new()));
        let id = client_id();
        *status.lock().unwrap() = if valid_id(&id) { "Not connected yet.".into() } else { "Not available in this build (no Discord application ID).".into() };
        if valid_id(&id) {
            let st = status.clone();
            std::thread::spawn(move || run(id, rx, st));
        }
        Presence { tx, status, last: None }
    }

    /// What to show, or None to show nothing. Repeats are ignored.
    pub fn set(&mut self, activity: Option<Value>) {
        if self.last.as_ref() == Some(&activity) {
            return;
        }
        self.last = Some(activity.clone());
        let _ = self.tx.send(activity);
    }

    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

fn run(id: String, rx: Receiver<Option<Value>>, status: Arc<Mutex<String>>) {
    let note = |s: String| {
        crate::util::debug(format!("discord: {s}"));
        *status.lock().unwrap() = s;
    };
    let mut want: Option<Value> = None;
    let mut sent: Option<Option<Value>> = None;
    let mut conn: Option<Conn> = None;
    let mut next_try = Instant::now();
    loop {
        // Wait for a change; while something should be shown but Discord isn't connected, wake
        // up to try again.
        let timeout = if want.is_some() && conn.is_none() { next_try.saturating_duration_since(Instant::now()) } else { Duration::from_secs(3600) };
        match rx.recv_timeout(timeout) {
            Ok(a) => {
                want = a;
                // let a burst of changes settle, keep only the last
                let until = Instant::now() + SETTLE;
                while let Ok(a) = rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                    want = a;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if want.is_none() && conn.is_none() {
            continue; // nothing to show and not connected: nothing to do
        }
        if conn.is_none() {
            if Instant::now() < next_try {
                continue;
            }
            match Conn::connect(&id) {
                Ok((c, user)) => {
                    note(if user.is_empty() { "Connected to Discord.".into() } else { format!("Connected to Discord as {user}.") });
                    conn = Some(c);
                    sent = None;
                }
                Err(e) => {
                    note(format!("{e} Trying again every 30 seconds."));
                    next_try = Instant::now() + RETRY;
                    continue;
                }
            }
        }
        if sent.as_ref() == Some(&want) {
            continue;
        }
        match conn.as_mut().unwrap().set_activity(&want) {
            Ok(()) => {
                note(match &want {
                    Some(a) => format!("Showing on Discord: {}.", a["details"].as_str().unwrap_or("")),
                    None => "Connected; nothing to show while no block is on.".into(),
                });
                sent = Some(want.clone());
            }
            Err(e) => {
                note(e);
                conn = None;
                next_try = Instant::now() + Duration::from_secs(2);
            }
        }
        // Nothing left to show: let Discord go until the next block.
        if want.is_none() {
            conn = None;
        }
    }
}

/// What a running block looks like on Discord. `blocks`: (name, locked, end time) of each block
/// that's on; None when no block is on.
pub fn activity(blocks: &[(String, bool, Option<i64>)], sites: usize, apps: usize) -> Option<Value> {
    if blocks.is_empty() {
        return None;
    }
    let (name, locked, _) = &blocks[0];
    let mut details = if *locked { format!("Locked in: {name}") } else { format!("Focusing: {name}") };
    if blocks.len() > 1 {
        details.push_str(&format!(" + {} more", blocks.len() - 1));
    }
    let what = match (sites, apps) {
        (0, 0) => "Blocking distractions".to_string(),
        (s, 0) => format!("Blocking {s} website{}", if s == 1 { "" } else { "s" }),
        (0, a) => format!("Blocking {a} program{}", if a == 1 { "" } else { "s" }),
        (s, a) => format!("Blocking {s} website{} and {a} program{}", if s == 1 { "" } else { "s" }, if a == 1 { "" } else { "s" }),
    };
    let mut act = json!({
        "details": details,
        "state": what,
        "assets": { "large_image": "https://raw.githubusercontent.com/demonztarz/ProgramPrison/HEAD/assets/logo-1024.png", "large_text": "Program Prison" },
        "buttons": [{ "label": "Get Program Prison", "url": "https://github.com/demonztarz/ProgramPrison" }],
        "instance": false,
    });
    // The block that ends soonest decides the countdown; on a schedule with no end, none.
    if let Some(end) = blocks.iter().filter_map(|b| b.2).min() {
        act["timestamps"] = json!({ "end": end });
    }
    Some(act)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let f = encode(OP_FRAME, &json!({"a": 1}));
        assert_eq!(&f[0..4], &1u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().unwrap()) as usize, f.len() - 8);
        let (op, v) = read_frame(&mut &f[..]).unwrap();
        assert_eq!(op, OP_FRAME);
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn activity_text() {
        assert_eq!(activity(&[], 3, 1), None);
        let a = activity(&[("Deep work".into(), true, Some(5000)), ("Evenings".into(), false, Some(3000))], 5, 2).unwrap();
        assert_eq!(a["details"], "Locked in: Deep work + 1 more");
        assert_eq!(a["state"], "Blocking 5 websites and 2 programs");
        assert_eq!(a["timestamps"]["end"], 3000, "soonest end");
        let b = activity(&[("Work".into(), false, None)], 1, 0).unwrap();
        assert_eq!(b["details"], "Focusing: Work");
        assert_eq!(b["state"], "Blocking 1 website");
        assert!(b.get("timestamps").is_none(), "no countdown without an end");
    }

    #[test]
    fn ids() {
        assert!(valid_id("1569344330315862127"));
        assert!(!valid_id(""));
        assert!(!valid_id("abc123"));
    }
}
