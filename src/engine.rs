//! All blocking rules: blocks, schedules, sessions, locks, domains and the hosts file text.
//! No operating-system calls here, so everything is unit tested.

use chrono::{Datelike, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const HOUR: i64 = 3_600_000;
pub const DAY: i64 = 24 * HOUR;
pub const SLOTS: usize = 7 * 24; // index = weekday (0 = Sunday) * 24 + hour, local time
const MAX_SESSION: i64 = 366 * DAY;
const CLOCK_JUMP_TOLERANCE: i64 = 2 * 60_000;

pub const MARK_BEGIN: &str = "# >>> ProgramPrison: managed section, edits here are undone >>>";
pub const MARK_END: &str = "# <<< ProgramPrison <<<";

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------- data ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppMode {
    #[default]
    Close,
    Cover,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    #[serde(default)]
    pub mode: AppMode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Lock {
    None,
    Random { length: u32 },
    Password { salt: String, hash: String },
    Locked,
}

impl Lock {
    pub fn kind(&self) -> &'static str {
        match self {
            Lock::None => "none",
            Lock::Random { .. } => "random",
            Lock::Password { .. } => "password",
            Lock::Locked => "locked",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub started_at: i64,
    pub ends_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub apps: Vec<App>,
    #[serde(default = "empty_schedule", with = "schedule_bits")]
    pub schedule: Vec<bool>,
    #[serde(default = "no_lock")]
    pub lock: Lock,
    #[serde(default)]
    pub session: Option<Session>,
    #[serde(default)]
    pub unlocked_until: i64,
}

fn empty_schedule() -> Vec<bool> {
    vec![false; SLOTS]
}
fn no_lock() -> Lock {
    Lock::None
}

/// The schedule is stored as a 168-character string of 0/1, one per hour of the week.
mod schedule_bits {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[bool], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.iter().map(|&b| if b { '1' } else { '0' }).collect::<String>())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<bool>, D::Error> {
        let s = String::deserialize(d)?;
        let mut v: Vec<bool> = s.chars().map(|c| c == '1').collect();
        v.resize(super::SLOTS, false);
        Ok(v)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub start_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { start_at_login: true }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default)]
    pub blocks: Vec<Block>,
    #[serde(default)]
    pub settings: Settings,
    /// Programs closed per local day ("2026-11-21" -> count), last 60 days.
    #[serde(default)]
    pub apps_closed: std::collections::BTreeMap<String, u32>,
}

impl State {
    /// Reads a state file, repairing anything invalid instead of failing.
    pub fn from_json(text: &str) -> State {
        let mut s: State = serde_json::from_str(text).unwrap_or_default();
        for b in &mut s.blocks {
            b.repair();
        }
        s
    }
}

pub fn new_id() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Block {
    pub fn new(name: &str) -> Block {
        let name = clean_name(name);
        Block {
            id: new_id(),
            name: if name.is_empty() { "New block".into() } else { name },
            sites: vec![],
            apps: vec![],
            schedule: empty_schedule(),
            lock: Lock::None,
            session: None,
            unlocked_until: 0,
        }
    }

    fn repair(&mut self) {
        if self.id.is_empty() {
            self.id = new_id();
        }
        self.name = clean_name(&self.name);
        if self.name.is_empty() {
            self.name = "Block".into();
        }
        self.sites = clean_sites(&self.sites);
        self.apps = clean_apps(&self.apps);
        self.schedule.resize(SLOTS, false);
        if let Lock::Random { length } = self.lock
            && !(20..=2000).contains(&length) {
                self.lock = Lock::Random { length: length.clamp(20, 2000) };
            }
        if let Lock::Password { salt, hash } = &self.lock
            && (salt.is_empty() || hash.is_empty()) {
                self.lock = Lock::Locked; // never turn a broken password lock into no lock
            }
    }
}

pub fn clean_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(60).collect()
}

// ---------- time ----------

fn local(t: i64) -> chrono::DateTime<Local> {
    Local.timestamp_millis_opt(t).single().unwrap_or_else(|| Local.timestamp_millis_opt(t + HOUR).earliest().unwrap())
}

pub fn slot_of(t: i64) -> usize {
    let d = local(t);
    d.weekday().num_days_from_sunday() as usize * 24 + d.hour() as usize
}

/// Start of the next local hour. Works for zones with half-hour offsets, since minutes are local.
pub fn next_hour(t: i64) -> i64 {
    let d = local(t);
    let into_hour = (d.minute() as i64 * 60 + d.second() as i64) * 1000 + (t.rem_euclid(1000));
    t - into_hour + HOUR
}

impl Block {
    pub fn scheduled_at(&self, t: i64) -> bool {
        self.schedule.get(slot_of(t)).copied().unwrap_or(false)
    }

    fn session_at(&self, t: i64) -> bool {
        matches!(self.session, Some(s) if t >= s.started_at && t < s.ends_at)
    }

    fn raw_active(&self, t: i64) -> bool {
        self.session_at(t) || self.scheduled_at(t)
    }

    pub fn is_active(&self, now: i64) -> bool {
        now >= self.unlocked_until && self.raw_active(now)
    }

    pub fn is_locked(&self, now: i64) -> bool {
        self.is_active(now) && self.lock != Lock::None
    }

    /// When the current run of activity (session and schedule together) ends.
    /// `Some(i64::MAX)` when the whole week is scheduled; `None` when not active.
    pub fn active_until(&self, now: i64) -> Option<i64> {
        if !self.raw_active(now) {
            return None;
        }
        let mut end = now;
        for _ in 0..SLOTS + 2 {
            if self.session_at(end) {
                end = self.session.unwrap().ends_at;
            } else if self.scheduled_at(end) {
                end = next_hour(end);
            } else {
                return Some(end);
            }
        }
        Some(i64::MAX)
    }

    pub fn start(&mut self, now: i64, ends_at: i64) -> Result<(), String> {
        if self.sites.is_empty() && self.apps.is_empty() {
            return Err("Add at least one website or program before starting the block.".into());
        }
        if ends_at <= now + 30_000 {
            return Err("Pick an end time at least a minute from now.".into());
        }
        if ends_at - now > MAX_SESSION {
            return Err("A block can run for a year at most.".into());
        }
        let current = if self.session_at(now) { self.session.unwrap().ends_at } else { 0 };
        self.session = Some(Session { started_at: now, ends_at: current.max(ends_at) });
        self.unlocked_until = 0;
        Ok(())
    }

    /// Ends the timed session; if scheduled hours are running too, skips them until they end.
    pub fn stop(&mut self, now: i64) {
        self.session = None;
        self.unlocked_until = match self.active_until(now) {
            None => 0,
            Some(i64::MAX) => now + 7 * DAY,
            Some(t) => t,
        };
    }
}

pub fn expire_sessions(blocks: &mut [Block], now: i64) -> bool {
    let mut changed = false;
    for b in blocks {
        if matches!(b.session, Some(s) if now >= s.ends_at) {
            b.session = None;
            changed = true;
        }
        if b.unlocked_until != 0 && now >= b.unlocked_until {
            b.unlocked_until = 0;
            changed = true;
        }
    }
    changed
}

/// If the wall clock moved forward much further than real time passed, someone changed the
/// clock to skip a block: push running sessions back by the same amount. Returns the jump.
pub fn adjust_for_clock_jump(blocks: &mut [Block], prev_wall: i64, wall: i64, elapsed_mono: i64) -> i64 {
    let jump = wall - prev_wall - elapsed_mono;
    if jump <= CLOCK_JUMP_TOLERANCE {
        return 0;
    }
    for b in blocks {
        if let Some(s) = &mut b.session
            && s.ends_at > prev_wall && s.started_at <= prev_wall {
                s.ends_at += jump;
            }
    }
    jump
}

// ---------- websites ----------

pub fn normalize_domain(input: &str) -> Option<String> {
    let mut s = input.trim().to_ascii_lowercase();
    if let Some(i) = s.find("://")
        && s[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) {
            s = s[i + 3..].to_string();
        }
    let s = s.split(['/', '?', '#', ' ', '\t']).next().unwrap_or("");
    let s = s.rsplit('@').next().unwrap_or("");
    let s = match s.rfind(':') {
        Some(i) if s[i + 1..].chars().all(|c| c.is_ascii_digit()) => &s[..i],
        _ => s,
    };
    let mut s = s.trim_end_matches('.');
    while let Some(rest) = s.strip_prefix("*.") {
        s = rest;
    }
    let s = s.strip_prefix("www.").unwrap_or(s);
    if s.is_empty() || s.len() > 253 {
        return None;
    }
    let labels: Vec<&str> = s.split('.').collect();
    if labels.len() < 2 {
        return None;
    }
    for l in &labels {
        let ok = !l.is_empty()
            && l.len() <= 63
            && l.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !l.starts_with('-')
            && !l.ends_with('-');
        if !ok {
            return None;
        }
    }
    let tld = labels.last().unwrap();
    if !tld.starts_with(|c: char| c.is_ascii_lowercase()) {
        return None; // rejects IP addresses
    }
    Some(s.to_string())
}

pub fn clean_sites(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in list {
        if let Some(d) = normalize_domain(s)
            && !out.contains(&d) {
                out.push(d);
            }
    }
    out
}

/// Hostnames that belong to the same site but aren't simple prefixes of it.
fn related(d: &str) -> &'static [&'static str] {
    match d {
        "youtube.com" => &["youtu.be", "www.youtu.be", "music.youtube.com", "youtube-nocookie.com", "www.youtube-nocookie.com"],
        "reddit.com" => &["old.reddit.com", "new.reddit.com", "np.reddit.com", "i.reddit.com", "redd.it", "www.redd.it"],
        "twitter.com" => &["x.com", "www.x.com", "mobile.x.com"],
        "x.com" => &["twitter.com", "www.twitter.com", "mobile.twitter.com"],
        "facebook.com" => &["web.facebook.com", "fb.com", "www.fb.com"],
        "instagram.com" => &["i.instagram.com"],
        "tiktok.com" => &["vm.tiktok.com"],
        "twitch.tv" => &["clips.twitch.tv"],
        "discord.com" => &["discordapp.com", "www.discordapp.com", "discord.gg"],
        _ => &[],
    }
}

pub fn expand_domain(d: &str) -> Vec<String> {
    let mut out = vec![d.to_string(), format!("www.{d}")];
    if d.split('.').count() <= 3 {
        out.push(format!("m.{d}"));
        out.push(format!("mobile.{d}"));
    }
    for r in related(d) {
        if !out.iter().any(|x| x == r) {
            out.push(r.to_string());
        }
    }
    out
}

pub fn blocked_domains<'a>(active: impl IntoIterator<Item = &'a Block>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for b in active {
        for s in &b.sites {
            set.extend(expand_domain(s));
        }
    }
    set.into_iter().collect()
}

/// Returns hosts file content with our section replaced (or removed when `domains` is empty).
pub fn apply_hosts_section(content: &str, domains: &[String]) -> String {
    let eol = if content.contains("\r\n") { "\r\n" } else { "\n" };
    let mut base = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find(MARK_BEGIN) {
        base.push_str(&rest[..start]);
        match rest[start..].find(MARK_END) {
            Some(e) => {
                let mut after = start + e + MARK_END.len();
                let tail = &rest[after..];
                let trimmed = tail.trim_start_matches([' ', '\t']);
                after += tail.len() - trimmed.len();
                if rest[after..].starts_with("\r\n") {
                    after += 2;
                } else if rest[after..].starts_with('\n') {
                    after += 1;
                }
                rest = &rest[after..];
            }
            None => {
                rest = "";
            }
        }
    }
    base.push_str(rest);
    if domains.is_empty() {
        return base;
    }
    if !base.is_empty() && !base.ends_with('\n') {
        base.push_str(eol);
    }
    base.push_str(MARK_BEGIN);
    base.push_str(eol);
    for d in domains {
        base.push_str(&format!("0.0.0.0 {d}{eol}:: {d}{eol}"));
    }
    base.push_str(MARK_END);
    base.push_str(eol);
    base
}

pub struct Preset {
    pub name: &'static str,
    pub sites: &'static [&'static str],
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Social media", sites: &["facebook.com", "instagram.com", "twitter.com", "x.com", "tiktok.com", "reddit.com", "snapchat.com", "threads.net", "bsky.app", "linkedin.com", "pinterest.com", "tumblr.com"] },
    Preset { name: "Video & streaming", sites: &["youtube.com", "netflix.com", "twitch.tv", "hulu.com", "disneyplus.com", "primevideo.com", "max.com", "kick.com", "vimeo.com"] },
    Preset { name: "News", sites: &["news.google.com", "cnn.com", "bbc.com", "bbc.co.uk", "nytimes.com", "theguardian.com", "foxnews.com", "news.ycombinator.com", "reuters.com"] },
    Preset { name: "Shopping", sites: &["amazon.com", "ebay.com", "aliexpress.com", "temu.com", "shein.com", "etsy.com", "walmart.com"] },
    Preset { name: "Gaming", sites: &["store.steampowered.com", "steamcommunity.com", "epicgames.com", "roblox.com", "chess.com", "miniclip.com", "poki.com"] },
];

// ---------- programs ----------

/// Processes that must never be closed: closing them would break the desktop or this app.
const PROTECTED: &[&str] = &[
    "explorer", "winlogon", "csrss", "smss", "wininit", "services", "lsass", "svchost", "dwm", "system", "idle",
    "conhost", "fontdrvhost", "sihost", "ctfmon", "registry", "runtimebroker", "searchhost", "startmenuexperiencehost",
    "shellexperiencehost", "textinputhost", "launchd", "kernel_task", "windowserver", "loginwindow", "finder", "dock",
    "systemuiserver", "systemd", "init", "xorg", "xwayland", "gnome-shell", "kwin_x11", "kwin_wayland", "plasmashell",
    "sddm", "gdm", "sh", "bash", "zsh", "ps",
];

pub fn app_key(name: &str) -> String {
    let base = name.trim().rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
    let base = base.strip_suffix(".exe").or_else(|| base.strip_suffix(".app")).unwrap_or(&base);
    base.trim().to_string()
}

pub fn is_protected(key: &str) -> bool {
    PROTECTED.contains(&key) || key.starts_with("programprison")
}

pub fn normalize_app(input: &str) -> Result<String, String> {
    let key = app_key(input);
    if key.is_empty() {
        return Err("Type the name of a program.".into());
    }
    if key.len() > 100 {
        return Err("That program name is too long.".into());
    }
    if is_protected(&key) {
        return Err(format!("“{key}” is part of the system or of ProgramPrison and can't be blocked."));
    }
    Ok(key)
}

pub fn clean_apps(list: &[App]) -> Vec<App> {
    let mut out: Vec<App> = Vec::new();
    for a in list {
        if let Ok(name) = normalize_app(&a.name)
            && !out.iter().any(|x| x.name == name) {
                out.push(App { name, mode: a.mode });
            }
    }
    out
}

/// `names`: every name a running process goes by (exe name, macOS bundle name...).
pub fn app_matches(names: &[String], apps: &[String]) -> bool {
    names.iter().any(|n| {
        let k = app_key(n);
        !k.is_empty()
            && !is_protected(&k)
            && apps.iter().any(|a| *a == k || (k.len() == 15 && a.len() > 15 && a.starts_with(&k)))
    })
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BlockedApps {
    pub close: Vec<String>,
    pub cover: Vec<String>,
}

/// Program names to close / cover. Close wins when blocks disagree. Where covering isn't
/// supported everything is closed.
pub fn blocked_apps<'a>(active: impl IntoIterator<Item = &'a Block>, cover_supported: bool) -> BlockedApps {
    let mut close = BTreeSet::new();
    let mut cover = BTreeSet::new();
    for b in active {
        for a in &b.apps {
            if a.mode == AppMode::Cover && cover_supported {
                cover.insert(a.name.clone());
            } else {
                close.insert(a.name.clone());
            }
        }
    }
    for n in &close {
        cover.remove(n);
    }
    BlockedApps { close: close.into_iter().collect(), cover: cover.into_iter().collect() }
}

// ---------- edits and locks ----------

/// The parts of a block the window may change directly.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Edit {
    pub name: Option<String>,
    pub sites: Option<Vec<String>>,
    pub apps: Option<Vec<App>>,
    pub schedule: Option<Vec<bool>>,
}

pub fn apply_edit(old: &Block, e: &Edit) -> Block {
    let mut next = old.clone();
    if let Some(n) = &e.name {
        let n = clean_name(n);
        if !n.is_empty() {
            next.name = n;
        }
    }
    if let Some(s) = &e.sites {
        next.sites = clean_sites(s);
    }
    if let Some(a) = &e.apps {
        next.apps = clean_apps(a);
    }
    if let Some(s) = &e.schedule {
        next.schedule = (0..SLOTS).map(|i| s.get(i).copied().unwrap_or(false)).collect();
    }
    next
}

/// While a block is locked you may add to it but not take anything away.
pub fn check_edit(old: &Block, next: &Block, now: i64) -> Result<(), String> {
    if !old.is_locked(now) {
        return Ok(());
    }
    let name = &old.name;
    if let Some(s) = old.sites.iter().find(|s| !next.sites.contains(s)) {
        return Err(format!("“{name}” is locked: you can't remove {s} until it ends."));
    }
    for a in &old.apps {
        match next.apps.iter().find(|x| x.name == a.name) {
            None => return Err(format!("“{name}” is locked: you can't remove {} until it ends.", a.name)),
            Some(n) if a.mode == AppMode::Close && n.mode == AppMode::Cover => {
                return Err(format!("“{name}” is locked: {} can't be switched from closing to covering until it ends.", a.name));
            }
            _ => {}
        }
    }
    if old.schedule.iter().zip(&next.schedule).any(|(&o, &n)| o && !n) {
        return Err(format!("“{name}” is locked: you can add scheduled hours but not remove them."));
    }
    Ok(())
}

/// What the window asks for when it sets a lock.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockRequest {
    pub kind: String,
    #[serde(default)]
    pub length: u32,
    #[serde(default)]
    pub password: String,
}

pub fn build_lock(old: &Block, req: &LockRequest, locked: bool) -> Result<Lock, String> {
    if locked {
        let same = req.kind == old.lock.kind()
            && match old.lock {
                Lock::Random { length } => req.length >= length,
                Lock::Password { .. } => req.password.is_empty(),
                _ => true,
            };
        if !same && req.kind != "locked" {
            return Err("The block is locked: the lock can only be made stronger (Locked) until it ends.".into());
        }
    }
    match req.kind.as_str() {
        "none" => Ok(Lock::None),
        "locked" => Ok(Lock::Locked),
        "random" => {
            if (20..=2000).contains(&req.length) {
                Ok(Lock::Random { length: req.length })
            } else {
                Err("Random text length must be between 20 and 2000 characters.".into())
            }
        }
        "password" => {
            if req.password.is_empty() {
                return match &old.lock {
                    l @ Lock::Password { .. } => Ok(l.clone()),
                    _ => Err("Type a password for the lock.".into()),
                };
            }
            if req.password.chars().count() < 4 {
                return Err("Use a password of at least 4 characters.".into());
            }
            let (salt, hash) = hash_password(&req.password);
            Ok(Lock::Password { salt, hash })
        }
        _ => Err("Unknown lock type.".into()),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

fn scrypt_hash(pw: &str, salt: &[u8]) -> [u8; 32] {
    let params = scrypt::Params::new(14, 8, 1).expect("valid scrypt params");
    let mut out = [0u8; 32];
    scrypt::scrypt(pw.as_bytes(), salt, &params, &mut out).expect("32-byte output");
    out
}

pub fn hash_password(pw: &str) -> (String, String) {
    let mut salt = [0u8; 16];
    let _ = getrandom::fill(&mut salt);
    let hash = scrypt_hash(pw, &salt);
    (hex(&salt), hex(&hash))
}

pub fn check_password(lock: &Lock, pw: &str) -> bool {
    let Lock::Password { salt, hash } = lock else { return false };
    let (Some(salt), Some(want)) = (unhex(salt), unhex(hash)) else { return false };
    let got = scrypt_hash(pw, &salt);
    // constant-time comparison
    want.len() == got.len() && want.iter().zip(got.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

const RANDOM_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

pub fn random_text(n: usize) -> String {
    let mut out = String::with_capacity(n);
    let mut buf = [0u8; 64];
    let limit = 256 - (256 % RANDOM_ALPHABET.len()); // rejection sampling keeps it uniform
    while out.len() < n {
        let _ = getrandom::fill(&mut buf);
        for &b in &buf {
            if (b as usize) < limit && out.len() < n {
                out.push(RANDOM_ALPHABET[b as usize % RANDOM_ALPHABET.len()] as char);
            }
        }
    }
    out
}

// ---------- what the window sees ----------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub active: bool,
    pub locked: bool,
    /// End of the current run; None while off or when on forever.
    pub until: Option<i64>,
    pub forever: bool,
    pub scheduled: bool,
    pub unlocked_until: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicBlock {
    pub id: String,
    pub name: String,
    pub sites: Vec<String>,
    pub apps: Vec<App>,
    pub schedule: Vec<bool>,
    /// "none", "random", "password" or "locked"
    pub lock: String,
    pub random_length: u32,
    pub status: Status,
}

impl Block {
    /// Never includes the password hash.
    pub fn public(&self, now: i64) -> PublicBlock {
        let active = self.is_active(now);
        let until = if active { self.active_until(now) } else { None };
        PublicBlock {
            id: self.id.clone(),
            name: self.name.clone(),
            sites: self.sites.clone(),
            apps: self.apps.clone(),
            schedule: self.schedule.clone(),
            lock: self.lock.kind().into(),
            random_length: if let Lock::Random { length } = self.lock { length } else { 0 },
            status: Status {
                active,
                locked: active && self.lock != Lock::None,
                until: until.filter(|&t| t != i64::MAX),
                forever: until == Some(i64::MAX),
                scheduled: self.schedule.iter().any(|&b| b),
                unlocked_until: if !active && self.unlocked_until > now { Some(self.unlocked_until) } else { None },
            },
        }
    }
}

pub fn day_key(t: i64) -> String {
    let d = local(t);
    format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
}

pub fn record_closed(state: &mut State, now: i64, n: u32) {
    *state.apps_closed.entry(day_key(now)).or_insert(0) += n;
    while state.apps_closed.len() > 60 {
        let first = state.apps_closed.keys().next().unwrap().clone();
        state.apps_closed.remove(&first);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().timestamp_millis()
    }
    // Wednesday 2026-11-18 10:30 local
    fn wed_1030() -> i64 {
        at(2026, 11, 18, 10, 30)
    }
    fn block() -> Block {
        let mut b = Block::new("Test");
        b.sites = vec!["reddit.com".into()];
        b.apps = vec![App { name: "steam".into(), mode: AppMode::Close }];
        b
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn normalize_domain_accepts_links() {
        assert_eq!(normalize_domain("https://www.Reddit.com/r/all?x=1").as_deref(), Some("reddit.com"));
        assert_eq!(normalize_domain("  youtube.com:443 ").as_deref(), Some("youtube.com"));
        assert_eq!(normalize_domain("*.example.co.uk").as_deref(), Some("example.co.uk"));
        assert_eq!(normalize_domain("news.ycombinator.com/").as_deref(), Some("news.ycombinator.com"));
        assert_eq!(normalize_domain("user@site.org").as_deref(), Some("site.org"));
        assert_eq!(normalize_domain("xn--80ak6aa92e.com").as_deref(), Some("xn--80ak6aa92e.com"));
        for bad in ["", "localhost", "not a site", "192.168.0.1", "a..b.com", "-x.com", "foo.", "exa_mple.com"] {
            assert_eq!(normalize_domain(bad), None, "{bad}");
        }
    }

    #[test]
    fn expand_domain_covers_prefixes_and_aliases() {
        let yt = expand_domain("youtube.com");
        for h in ["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be"] {
            assert!(yt.contains(&h.to_string()), "{h}");
        }
        assert!(expand_domain("twitter.com").contains(&"x.com".to_string()));
        assert!(!expand_domain("a.b.c.example.com").contains(&"m.a.b.c.example.com".to_string()));
    }

    #[test]
    fn hosts_section_roundtrip() {
        let original = "127.0.0.1 localhost\n::1 localhost\n";
        let a = apply_hosts_section(original, &s(&["reddit.com", "www.reddit.com"]));
        assert!(a.starts_with(original));
        assert!(a.contains("0.0.0.0 reddit.com\n:: reddit.com\n"));
        assert_eq!(apply_hosts_section(&a, &s(&["reddit.com", "www.reddit.com"])), a);
        let b = apply_hosts_section(&a, &s(&["x.com"]));
        assert!(!b.contains("reddit"));
        assert_eq!(b.matches(MARK_BEGIN).count(), 1);
        assert_eq!(apply_hosts_section(&b, &[]), original);
        let win = "127.0.0.1 localhost\r\n# comment";
        let w = apply_hosts_section(win, &s(&["x.com"]));
        assert!(w.contains("\r\n0.0.0.0 x.com\r\n"));
        assert_eq!(apply_hosts_section(&w, &[]), format!("{win}\r\n"));
        assert_eq!(apply_hosts_section("", &[]), "");
        // an unterminated section (hand-damaged file) is dropped rather than duplicated
        let damaged = format!("127.0.0.1 localhost\n{MARK_BEGIN}\n0.0.0.0 a.com\n");
        assert_eq!(apply_hosts_section(&damaged, &[]), "127.0.0.1 localhost\n");
    }

    #[test]
    fn apps_rules() {
        assert_eq!(normalize_app("C:\\Games\\Steam\\Steam.EXE").unwrap(), "steam");
        assert_eq!(normalize_app("/Applications/Discord.app").unwrap(), "discord");
        assert!(normalize_app("explorer.exe").is_err());
        assert!(normalize_app("ProgramPrison.exe").is_err());
        assert!(normalize_app("   ").is_err());
        assert!(app_matches(&s(&["Steam.exe"]), &s(&["steam"])));
        assert!(!app_matches(&s(&["steamwebhelper.exe"]), &s(&["steam"])));
        assert!(app_matches(&s(&["/usr/bin/x", "Discord"]), &s(&["discord"])));
        assert!(app_matches(&s(&["averyveryverylo"]), &s(&["averyveryverylongname"])));
        assert!(!app_matches(&s(&["explorer.exe"]), &s(&["explorer"])));
    }

    #[test]
    fn blocked_apps_close_wins_and_fallback() {
        let mut a = block();
        a.apps = vec![App { name: "steam".into(), mode: AppMode::Cover }, App { name: "discord".into(), mode: AppMode::Cover }];
        let b = block();
        assert_eq!(blocked_apps([&a, &b], true), BlockedApps { close: s(&["steam"]), cover: s(&["discord"]) });
        assert_eq!(blocked_apps([&a], false), BlockedApps { close: s(&["discord", "steam"]), cover: vec![] });
    }

    #[test]
    fn sessions() {
        let now = wed_1030();
        let mut b = block();
        assert!(!b.is_active(now));
        b.start(now, now + 60 * MIN).unwrap();
        assert!(b.is_active(now + 59 * MIN));
        assert_eq!(b.active_until(now), Some(now + 60 * MIN));
        b.start(now + MIN, now + 10 * MIN).unwrap();
        assert_eq!(b.session.unwrap().ends_at, now + 60 * MIN, "never shortened");
        assert!(!b.is_active(now + 60 * MIN));
        assert!(expire_sessions(std::slice::from_mut(&mut b), now + 61 * MIN));
        assert_eq!(b.session, None);
        let mut empty = Block::new("e");
        assert!(empty.start(now, now + HOUR).is_err());
        assert!(b.start(now, now - MIN).is_err());
        assert!(b.start(now, now + 400 * DAY).is_err());
    }

    #[test]
    fn schedule() {
        let now = wed_1030();
        let mut b = block();
        let wed = 3 * 24;
        b.schedule[wed + 10] = true;
        b.schedule[wed + 11] = true;
        assert!(b.is_active(now));
        assert_eq!(b.active_until(now), Some(at(2026, 11, 18, 12, 0)));
        assert!(!b.is_active(at(2026, 11, 18, 12, 0)));
        b.schedule[wed + 13] = true;
        b.start(now, at(2026, 11, 18, 13, 15)).unwrap();
        assert_eq!(b.active_until(now), Some(at(2026, 11, 18, 14, 0)));
        b.schedule = vec![true; SLOTS];
        assert_eq!(b.active_until(now), Some(i64::MAX));
    }

    #[test]
    fn stop() {
        let now = wed_1030();
        let mut b = block();
        b.schedule[3 * 24 + 10] = true;
        b.schedule[3 * 24 + 15] = true;
        b.stop(now);
        assert!(!b.is_active(now + MIN));
        assert!(b.is_active(at(2026, 11, 18, 15, 5)), "next scheduled run still happens");
        b.start(now + 2 * MIN, now + 20 * MIN).unwrap();
        assert!(b.is_active(now + 3 * MIN), "starting clears the stop");
        let mut t = block();
        t.start(now, now + HOUR).unwrap();
        t.stop(now + MIN);
        assert_eq!(t.unlocked_until, 0);
        assert!(!t.is_active(now + 2 * MIN));
    }

    #[test]
    fn locked_edits() {
        let now = wed_1030();
        let mut b = block();
        b.lock = Lock::Random { length: 100 };
        b.schedule[3 * 24 + 10] = true;
        assert!(b.is_locked(now));
        let e = |e: Edit| check_edit(&b, &apply_edit(&b, &e), now);
        assert!(e(Edit { sites: Some(vec![]), ..Default::default() }).is_err());
        assert!(e(Edit { apps: Some(vec![]), ..Default::default() }).is_err());
        assert!(e(Edit { apps: Some(vec![App { name: "steam".into(), mode: AppMode::Cover }]), ..Default::default() }).is_err());
        assert!(e(Edit { schedule: Some(vec![false; SLOTS]), ..Default::default() }).is_err());
        assert!(e(Edit { sites: Some(s(&["reddit.com", "x.com"])), name: Some("Renamed".into()), ..Default::default() }).is_ok());
        assert!(e(Edit { schedule: Some(vec![true; SLOTS]), ..Default::default() }).is_ok());
        let mut off = block();
        off.lock = Lock::Locked;
        assert!(check_edit(&off, &apply_edit(&off, &Edit { sites: Some(vec![]), ..Default::default() }), now).is_ok());
    }

    #[test]
    fn locks() {
        let mut b = block();
        let req = |kind: &str, length: u32, password: &str| LockRequest { kind: kind.into(), length, password: password.into() };
        assert_eq!(build_lock(&b, &req("random", 300, ""), false), Ok(Lock::Random { length: 300 }));
        assert!(build_lock(&b, &req("random", 5, ""), false).is_err());
        assert!(build_lock(&b, &req("password", 0, ""), false).is_err());
        let pw = build_lock(&b, &req("password", 0, "hunter22"), false).unwrap();
        assert!(check_password(&pw, "hunter22"));
        assert!(!check_password(&pw, "hunter23"));
        assert!(!check_password(&pw, ""));
        assert!(!serde_json::to_string(&pw).unwrap().contains("hunter22"));
        b.lock = pw.clone();
        assert_eq!(build_lock(&b, &req("password", 0, ""), false), Ok(pw), "empty keeps the password");
        assert!(build_lock(&b, &req("none", 0, ""), true).is_err());
        assert!(build_lock(&b, &req("password", 0, "new1"), true).is_err());
        assert_eq!(build_lock(&b, &req("locked", 0, ""), true), Ok(Lock::Locked));
        b.lock = Lock::Random { length: 200 };
        assert!(build_lock(&b, &req("random", 50, ""), true).is_err());
        assert!(build_lock(&b, &req("random", 400, ""), true).is_ok());
    }

    #[test]
    fn clock_jump() {
        let now = wed_1030();
        let mut b = block();
        b.start(now, now + HOUR).unwrap();
        let mut later = block();
        later.session = Some(Session { started_at: now + 2 * HOUR, ends_at: now + 3 * HOUR });
        let mut v = vec![b, later];
        let jump = adjust_for_clock_jump(&mut v, now + MIN, now + MIN + 5 * HOUR, 3000);
        assert!(jump > 4 * HOUR);
        assert_eq!(v[0].session.unwrap().ends_at, now + HOUR + jump);
        assert_eq!(v[1].session.unwrap().ends_at, now + 3 * HOUR);
        assert_eq!(adjust_for_clock_jump(&mut v, 1000, 4000, 3000), 0);
        assert_eq!(adjust_for_clock_jump(&mut v, 1000, 61000, 3000), 0);
        assert_eq!(adjust_for_clock_jump(&mut v, 10 * HOUR, 1000, 3000), 0);
    }

    #[test]
    fn state_repair_and_roundtrip() {
        assert_eq!(State::from_json("garbage"), State::default());
        let st = State::from_json(r#"{"blocks":[{"id":"","name":" x ","sites":["https://Reddit.com","junk!"],"apps":[{"name":"steam.exe"},{"name":"explorer"}],"schedule":"1","lock":{"type":"password","salt":"","hash":""}}]}"#);
        let b = &st.blocks[0];
        assert!(!b.id.is_empty());
        assert_eq!(b.name, "x");
        assert_eq!(b.sites, s(&["reddit.com"]));
        assert_eq!(b.apps, vec![App { name: "steam".into(), mode: AppMode::Close }]);
        assert_eq!(b.schedule.len(), SLOTS);
        assert!(b.schedule[0]);
        assert_eq!(b.lock, Lock::Locked, "broken password lock becomes Locked, not open");
        let text = serde_json::to_string(&st).unwrap();
        assert_eq!(State::from_json(&text), st);
    }

    #[test]
    fn public_block_hides_hash() {
        let mut b = block();
        let (salt, hash) = hash_password("abcd");
        b.lock = Lock::Password { salt, hash: hash.clone() };
        let p = b.public(wed_1030());
        assert_eq!(p.lock, "password");
        assert!(!serde_json::to_string(&p).unwrap().contains(&hash));
    }

    #[test]
    fn random_text_alphabet() {
        let t = random_text(500);
        assert_eq!(t.len(), 500);
        assert!(t.bytes().all(|c| RANDOM_ALPHABET.contains(&c)));
        assert!(!t.contains(['0', 'O', '1', 'l', 'I']));
    }

    #[test]
    fn next_hour_is_on_the_hour() {
        let t = at(2026, 11, 18, 10, 30) + 12_345;
        assert_eq!(next_hour(t), at(2026, 11, 18, 11, 0));
    }
}
