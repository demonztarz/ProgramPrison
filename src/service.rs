//! The background blocker: holds the state, answers the window, and enforces blocks.
//! The platform module drives it: a plain loop on macOS/Linux, the Windows message loop on Windows.

use crate::engine::{self, Block, BlockedApps, State};
use crate::ipc::{Challenge, HostsStatus, PublicState, Request, Response};
use crate::{hosts, procs, util};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

pub struct Service {
    pub state: State,
    state_path: PathBuf,
    hosts_path: PathBuf,
    hosts_status: HostsStatus,
    last_domains: Option<Vec<String>>,
    last_hosts_check: Option<Instant>,
    challenges: HashMap<String, String>,
    pub cover_supported: bool,
    last_wall: i64,
    last_mono: i64,
    pub asleep: bool,
    pub quit_requested: bool,
    show_requests: u32,
}

/// What the platform loop should do with programs after an enforcement pass.
pub struct AppWork {
    pub apps: BlockedApps,
    /// For covers: (program name, block name, end time) of each covered program.
    pub labels: Vec<(String, String, Option<i64>)>,
}

impl Service {
    pub fn new(cover_supported: bool) -> Service {
        let dir = util::data_dir();
        let state_path = dir.join("state.json");
        let state = std::fs::read_to_string(&state_path).map(|t| State::from_json(&t)).unwrap_or_default();
        let hosts_path = hosts::hosts_path();
        Service {
            state,
            state_path,
            hosts_status: HostsStatus { path: hosts_path.display().to_string(), writable: true, ..Default::default() },
            hosts_path,
            last_domains: None,
            last_hosts_check: None,
            challenges: HashMap::new(),
            cover_supported,
            last_wall: engine::now_ms(),
            last_mono: mono_ms(),
            asleep: false,
            quit_requested: false,
            show_requests: 0,
        }
    }

    pub fn save(&self) {
        if let Some(dir) = self.state_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.state_path.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(&self.state).unwrap()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.state_path);
        }
    }

    pub fn any_locked(&self) -> bool {
        let now = engine::now_ms();
        self.state.blocks.iter().any(|b| b.is_locked(now))
    }

    /// Detects the clock being moved forward. Call on every tick.
    pub fn check_clock(&mut self) {
        let wall = engine::now_ms();
        let mono = mono_ms();
        if !self.asleep {
            let elapsed = mono - self.last_mono;
            let jump = engine::adjust_for_clock_jump(&mut self.state.blocks, self.last_wall, wall, elapsed);
            if jump > 0 {
                util::debug(format!("clock jumped forward {jump} ms; sessions extended"));
                self.save();
            }
        }
        self.last_wall = wall;
        self.last_mono = mono;
    }

    /// After sleep, real time and wall time both moved on; don't mistake that for a clock change.
    pub fn resumed(&mut self) {
        self.asleep = false;
        self.last_wall = engine::now_ms();
        self.last_mono = mono_ms();
    }

    /// Updates the hosts file (when needed) and says which programs to close or cover.
    pub fn enforce(&mut self, force: bool) -> AppWork {
        let now = engine::now_ms();
        if engine::expire_sessions(&mut self.state.blocks, now) {
            self.save();
        }
        let active: Vec<&Block> = self.state.blocks.iter().filter(|b| b.is_active(now)).collect();
        let domains = engine::blocked_domains(active.iter().copied());
        // Re-check the file now and then even when nothing changed, to undo hand edits.
        let stale = self.last_hosts_check.is_none_or(|t| t.elapsed().as_secs() >= 10);
        if force || stale || self.last_domains.as_ref() != Some(&domains) {
            self.last_hosts_check = Some(Instant::now());
            let r = hosts::sync(&self.hosts_path, &domains);
            if r == hosts::Sync::Changed {
                util::debug(format!("hosts file updated: {} names", domains.len()));
            }
            let ok = matches!(r, hosts::Sync::Changed | hosts::Sync::Unchanged);
            self.hosts_status.needs_setup = matches!(r, hosts::Sync::NeedsSetup(_));
            self.hosts_status.error = match r {
                hosts::Sync::NeedsSetup(e) | hosts::Sync::Failed(e) => Some(e),
                _ => None,
            };
            self.last_domains = if ok { Some(domains) } else { None };
        }
        let apps = engine::blocked_apps(active.iter().copied(), self.cover_supported);
        let labels = apps
            .cover
            .iter()
            .map(|name| {
                let b = active.iter().find(|b| b.apps.iter().any(|a| &a.name == name));
                let until = b.and_then(|b| b.active_until(now)).filter(|&t| t != i64::MAX);
                (name.clone(), b.map(|b| b.name.clone()).unwrap_or_default(), until)
            })
            .collect();
        AppWork { apps, labels }
    }

    /// Closes running programs on the close list. Returns how many were closed.
    pub fn close_programs(&mut self, list: &[procs::Proc], close: &[String]) -> u32 {
        if close.is_empty() {
            return 0;
        }
        let mut n = 0;
        for p in list {
            if engine::app_matches(&p.names, close) && procs::kill(p.pid) {
                util::debug(format!("closed {} ({})", p.names.join("/"), p.pid));
                n += 1;
            }
        }
        if n > 0 {
            engine::record_closed(&mut self.state, engine::now_ms(), n);
            self.save();
        }
        n
    }

    pub fn public_state(&self) -> PublicState {
        let now = engine::now_ms();
        PublicState {
            now,
            blocks: self.state.blocks.iter().map(|b| b.public(now)).collect(),
            settings: self.state.settings.clone(),
            closed_today: self.state.apps_closed.get(&engine::day_key(now)).copied().unwrap_or(0),
            hosts: HostsStatus { writable: hosts::can_write(&self.hosts_path), ..self.hosts_status.clone() },
            platform: std::env::consts::OS.into(),
            cover_supported: self.cover_supported,
            version: env!("CARGO_PKG_VERSION").into(),
            show_requests: self.show_requests,
        }
    }

    fn changed(&mut self) -> Response {
        self.save();
        self.enforce(true);
        Response { ok: true, state: Some(self.public_state()), ..Default::default() }
    }

    fn fail(&self, e: impl Into<String>) -> Response {
        Response { ok: false, error: Some(e.into()), state: Some(self.public_state()), ..Default::default() }
    }

    fn block_mut(&mut self, id: &str) -> Option<&mut Block> {
        self.state.blocks.iter_mut().find(|b| b.id == id)
    }

    /// Answers one request. Returns the response and whether enforcement should run now.
    pub fn handle(&mut self, req: Request) -> (Response, bool) {
        let now = engine::now_ms();
        let gone = "That block no longer exists.";
        let r = match req {
            Request::Ping => return (Response::ok(), false),
            Request::GetState => return (Response { ok: true, state: Some(self.public_state()), ..Default::default() }, false),
            Request::RunningApps => return (Response { ok: true, apps: Some(procs::running_names()), ..Default::default() }, false),
            Request::Covers => return (Response::ok(), false), // answered by the platform loop
            Request::ShowWindow => {
                self.show_requests += 1;
                return (Response::ok(), false);
            }
            Request::CreateBlock { name } => {
                let name = if name.trim().is_empty() { format!("Block {}", self.state.blocks.len() + 1) } else { name };
                let b = Block::new(&name);
                let id = b.id.clone();
                self.state.blocks.push(b);
                let mut r = self.changed();
                r.id = Some(id);
                r
            }
            Request::UpdateBlock { id, edit } => match self.block_mut(&id) {
                None => self.fail(gone),
                Some(old) => {
                    let next = engine::apply_edit(old, &edit);
                    match engine::check_edit(old, &next, now) {
                        Err(e) => self.fail(e),
                        Ok(()) => {
                            *old = next;
                            self.changed()
                        }
                    }
                }
            },
            Request::DeleteBlock { id } => match self.state.blocks.iter().position(|b| b.id == id) {
                None => self.fail(gone),
                Some(i) if self.state.blocks[i].is_locked(now) => {
                    let n = self.state.blocks[i].name.clone();
                    self.fail(format!("“{n}” is locked and can't be deleted until it ends."))
                }
                Some(i) => {
                    self.state.blocks.remove(i);
                    self.changed()
                }
            },
            Request::StartBlock { id, ends_at } => match self.block_mut(&id) {
                None => self.fail(gone),
                Some(b) => match b.start(now, ends_at) {
                    Err(e) => self.fail(e),
                    Ok(()) => self.changed(),
                },
            },
            Request::SetLock { id, lock } => match self.block_mut(&id) {
                None => self.fail(gone),
                Some(b) => match engine::build_lock(b, &lock, b.is_locked(now)) {
                    Err(e) => self.fail(e),
                    Ok(l) => {
                        b.lock = l;
                        self.challenges.remove(&id);
                        self.changed()
                    }
                },
            },
            Request::Challenge { id } => {
                let Some(b) = self.state.blocks.iter().find(|b| b.id == id) else { return (self.fail(gone), false) };
                let c = if !b.is_active(now) {
                    Challenge::None
                } else {
                    match &b.lock {
                        engine::Lock::None => Challenge::None,
                        engine::Lock::Locked => Challenge::Locked { until: b.active_until(now).filter(|&t| t != i64::MAX) },
                        engine::Lock::Password { .. } => Challenge::Password,
                        engine::Lock::Random { length } => {
                            let text = engine::random_text(*length as usize);
                            self.challenges.insert(id.clone(), text.clone());
                            Challenge::Random { text }
                        }
                    }
                };
                return (Response { ok: true, challenge: Some(c), ..Default::default() }, false);
            }
            Request::StopBlock { id, answer } => {
                let want = self.challenges.get(&id).cloned();
                match self.block_mut(&id) {
                    None => self.fail(gone),
                    Some(b) => {
                        let err = if b.is_active(now) {
                            match &b.lock {
                                engine::Lock::None => None,
                                engine::Lock::Locked => Some(format!("“{}” is locked until it ends.", b.name)),
                                l @ engine::Lock::Password { .. } => (!engine::check_password(l, &answer)).then(|| "Wrong password.".to_string()),
                                engine::Lock::Random { .. } => {
                                    let typed: String = answer.chars().filter(|c| !c.is_whitespace()).collect();
                                    (want.as_deref() != Some(typed.as_str()))
                                        .then(|| "The text doesn't match. Check every character; upper and lower case count.".to_string())
                                }
                            }
                        } else {
                            None
                        };
                        match err {
                            Some(e) => self.fail(e),
                            None => {
                                b.stop(now);
                                self.challenges.remove(&id);
                                self.changed()
                            }
                        }
                    }
                }
            }
            Request::UpdateSettings { start_at_login } => {
                if !start_at_login && self.any_locked() {
                    self.fail("Start at login can't be turned off while a locked block is running.")
                } else {
                    self.state.settings.start_at_login = start_at_login;
                    crate::autostart::apply(start_at_login);
                    self.changed()
                }
            }
            Request::SetupHosts => {
                // dispatch() asks for permission before taking the lock; this only refreshes.
                let r: Result<(), String> = Ok(());
                self.last_domains = None;
                let mut out = self.changed();
                if let Err(e) = r {
                    out.ok = false;
                    out.error = Some(e);
                }
                out
            }
            Request::Quit => {
                if self.any_locked() {
                    self.fail("A locked block is running, so ProgramPrison can't be closed until it ends.")
                } else {
                    self.quit_requested = true;
                    Response::ok()
                }
            }
        };
        let enforce = r.ok;
        (r, enforce)
    }
}

/// True if another copy of the background blocker is already answering.
pub fn already_running() -> bool {
    crate::ipc::Client::connect().is_some_and(|c| c.send(&Request::Ping).is_ok_and(|r| r.ok))
}

/// Milliseconds of real time, including time asleep (so sleeping isn't a "clock jump").
pub fn mono_ms() -> i64 {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        #[cfg(target_os = "linux")]
        let id = libc::CLOCK_BOOTTIME;
        #[cfg(target_os = "macos")]
        let id = libc::CLOCK_MONOTONIC; // counts during sleep on macOS
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(id, &mut ts) };
        ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000
    }
    #[cfg(windows)]
    {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_millis() as i64
    }
}

/// Handles a request from the window. Asking for the administrator password can take minutes,
/// so it happens before the state is locked. Returns the response and whether to enforce now.
pub fn dispatch(svc: &std::sync::Mutex<Service>, req: Request) -> (Response, bool) {
    if let Request::SetupHosts = req {
        let r = hosts::grant_access();
        let mut s = svc.lock().unwrap();
        let (mut resp, _) = s.handle(Request::SetupHosts);
        if let Err(e) = r {
            resp.ok = false;
            resp.error = Some(e);
        }
        return (resp, true);
    }
    svc.lock().unwrap().handle(req)
}
