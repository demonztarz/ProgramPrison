'use strict';
// Core blocking logic. No Electron or OS calls here, so all of it can be unit tested.

const crypto = require('crypto');

const HOUR = 3600000;
const DAY = 24 * HOUR;
const SLOTS = 7 * 24; // one slot per hour of the week, index = jsDay * 24 + hour (jsDay 0 = Sunday)
const MAX_SESSION = 366 * DAY;
const CLOCK_JUMP_TOLERANCE = 2 * 60000;

const MARK_BEGIN = '# >>> ProgramPrison: managed section, edits here are undone >>>';
const MARK_END = '# <<< ProgramPrison <<<';

// Hostnames that belong to the same site but are not simple prefixes of it.
const RELATED = {
  'youtube.com': ['youtu.be', 'www.youtu.be', 'music.youtube.com', 'youtube-nocookie.com', 'www.youtube-nocookie.com'],
  'reddit.com': ['old.reddit.com', 'new.reddit.com', 'np.reddit.com', 'i.reddit.com', 'redd.it', 'www.redd.it'],
  'twitter.com': ['x.com', 'www.x.com', 'mobile.x.com'],
  'x.com': ['twitter.com', 'www.twitter.com', 'mobile.twitter.com'],
  'facebook.com': ['web.facebook.com', 'fb.com', 'www.fb.com'],
  'instagram.com': ['i.instagram.com'],
  'tiktok.com': ['vm.tiktok.com'],
  'twitch.tv': ['clips.twitch.tv'],
  'discord.com': ['discordapp.com', 'www.discordapp.com', 'discord.gg'],
};

const PRESETS = [
  { name: 'Social media', sites: ['facebook.com', 'instagram.com', 'twitter.com', 'x.com', 'tiktok.com', 'reddit.com', 'snapchat.com', 'threads.net', 'bsky.app', 'linkedin.com', 'pinterest.com', 'tumblr.com'] },
  { name: 'Video & streaming', sites: ['youtube.com', 'netflix.com', 'twitch.tv', 'hulu.com', 'disneyplus.com', 'primevideo.com', 'max.com', 'kick.com', 'vimeo.com'] },
  { name: 'News', sites: ['news.google.com', 'cnn.com', 'bbc.com', 'bbc.co.uk', 'nytimes.com', 'theguardian.com', 'foxnews.com', 'news.ycombinator.com', 'reuters.com'] },
  { name: 'Shopping', sites: ['amazon.com', 'ebay.com', 'aliexpress.com', 'temu.com', 'shein.com', 'etsy.com', 'walmart.com'] },
  { name: 'Gaming', sites: ['store.steampowered.com', 'steamcommunity.com', 'epicgames.com', 'roblox.com', 'chess.com', 'miniclip.com', 'poki.com'] },
];

// Processes that must never be closed: closing them would break the desktop or this app itself.
const PROTECTED_APPS = new Set([
  'explorer', 'winlogon', 'csrss', 'smss', 'wininit', 'services', 'lsass', 'svchost', 'dwm', 'system', 'idle',
  'conhost', 'fontdrvhost', 'sihost', 'ctfmon', 'registry', 'runtimebroker', 'searchhost', 'startmenuexperiencehost',
  'launchd', 'kernel_task', 'windowserver', 'loginwindow', 'finder', 'dock', 'systemuiserver',
  'systemd', 'init', 'xorg', 'xwayland', 'gnome-shell', 'kwin_x11', 'kwin_wayland', 'plasmashell', 'sddm', 'gdm',
  'programprison', 'electron', 'ps', 'tasklist', 'taskkill',
]);

function defaultState() {
  return { version: 1, blocks: [], settings: { startAtLogin: true }, stats: { appsClosed: {} } };
}

function newBlock(name) {
  return {
    id: crypto.randomUUID(),
    name: cleanName(name) || 'New block',
    sites: [],
    apps: [],
    schedule: new Array(SLOTS).fill(false),
    lock: { type: 'none' },
    session: null,
    unlockedUntil: 0,
  };
}

// Fills in anything missing from a block read from disk, so a hand-edited or old state file loads.
function repairBlock(b) {
  const block = newBlock(b && b.name);
  if (!b || typeof b !== 'object') return block;
  if (typeof b.id === 'string' && b.id) block.id = b.id;
  block.sites = uniq((Array.isArray(b.sites) ? b.sites : []).map(normalizeDomain).filter(Boolean));
  block.apps = cleanApps(Array.isArray(b.apps) ? b.apps : []);
  if (Array.isArray(b.schedule)) block.schedule = block.schedule.map((_, i) => b.schedule[i] === true);
  if (b.lock && ['none', 'random', 'password', 'locked'].includes(b.lock.type)) {
    block.lock = { ...b.lock };
    if (block.lock.type === 'password' && !(b.lock.salt && b.lock.hash)) block.lock = { type: 'locked' };
  }
  if (b.session && Number.isFinite(b.session.endsAt)) {
    block.session = { startedAt: Number(b.session.startedAt) || 0, endsAt: b.session.endsAt };
  }
  block.unlockedUntil = Number(b.unlockedUntil) || 0;
  return block;
}

function repairState(s) {
  const state = defaultState();
  if (!s || typeof s !== 'object') return state;
  if (Array.isArray(s.blocks)) state.blocks = s.blocks.map(repairBlock);
  if (s.settings && typeof s.settings.startAtLogin === 'boolean') state.settings.startAtLogin = s.settings.startAtLogin;
  if (s.stats && s.stats.appsClosed && typeof s.stats.appsClosed === 'object') state.stats.appsClosed = { ...s.stats.appsClosed };
  return state;
}

// Programs are { name, mode }: mode 'close' ends the program, 'cover' hides its windows behind
// an overlay (Windows only; elsewhere covering falls back to closing).
function cleanApps(list) {
  const out = [];
  const seen = new Set();
  for (const a of list) {
    const raw = typeof a === 'string' ? { name: a } : a || {};
    const name = normalizeApp(raw.name).value;
    if (!name || seen.has(name)) continue;
    seen.add(name);
    out.push({ name, mode: raw.mode === 'cover' ? 'cover' : 'close' });
  }
  return out;
}

function cleanName(name) {
  return String(name == null ? '' : name).replace(/\s+/g, ' ').trim().slice(0, 60);
}

function uniq(list) {
  return [...new Set(list)];
}

// ---------- time ----------

function slotOf(t) {
  const d = new Date(t);
  return d.getDay() * 24 + d.getHours();
}

function nextHour(t) {
  const d = new Date(t);
  d.setMinutes(60, 0, 0);
  return d.getTime();
}

function scheduledAt(block, t) {
  return block.schedule[slotOf(t)] === true;
}

function sessionAt(block, t) {
  return !!block.session && t >= (block.session.startedAt || 0) && t < block.session.endsAt;
}

// Active ignoring any unlock.
function rawActive(block, t) {
  return sessionAt(block, t) || scheduledAt(block, t);
}

function isActive(block, now) {
  return now >= (block.unlockedUntil || 0) && rawActive(block, now);
}

function isLocked(block, now) {
  return isActive(block, now) && block.lock.type !== 'none';
}

// When the current run of activity (session and schedule together) ends. Infinity when the
// schedule covers every hour of the week; null when the block is not active.
function activeUntil(block, now) {
  if (!rawActive(block, now)) return null;
  let end = now;
  for (let i = 0; i < SLOTS + 2; i++) {
    if (sessionAt(block, end)) end = block.session.endsAt;
    else if (scheduledAt(block, end)) end = nextHour(end);
    else return end;
  }
  return Infinity;
}

function startSession(block, now, endsAt) {
  if (!block.sites.length && !block.apps.length) return 'Add at least one website or app before starting the block.';
  if (!Number.isFinite(endsAt) || endsAt <= now + 30000) return 'Pick an end time at least a minute from now.';
  if (endsAt - now > MAX_SESSION) return 'A block can run for a year at most.';
  const current = sessionAt(block, now) ? block.session.endsAt : 0;
  block.session = { startedAt: now, endsAt: Math.max(current, Math.round(endsAt)) };
  block.unlockedUntil = 0;
  return null;
}

// Ends the timed session; if scheduled hours are running too, skips them until they end.
function stopBlock(block, now) {
  block.session = null;
  const until = activeUntil(block, now);
  block.unlockedUntil = until == null ? 0 : until === Infinity ? now + 7 * DAY : until;
}

function expireSessions(blocks, now) {
  let changed = false;
  for (const b of blocks) {
    if (b.session && now >= b.session.endsAt) { b.session = null; changed = true; }
    if (b.unlockedUntil && now >= b.unlockedUntil) { b.unlockedUntil = 0; changed = true; }
  }
  return changed;
}

// If the wall clock moved forward much further than real time passed, someone changed the clock
// to skip a block: push running sessions back by the same amount. Returns the jump in ms (0 = none).
function adjustForClockJump(blocks, prevWall, wall, elapsedMono) {
  const jump = wall - prevWall - elapsedMono;
  if (!(jump > CLOCK_JUMP_TOLERANCE)) return 0;
  for (const b of blocks) {
    if (b.session && b.session.endsAt > prevWall && b.session.startedAt <= prevWall) b.session.endsAt += jump;
  }
  return jump;
}

// ---------- websites ----------

function normalizeDomain(input) {
  let s = String(input == null ? '' : input).trim().toLowerCase();
  s = s.replace(/^[a-z][a-z0-9+.-]*:\/\//, '');
  s = s.split(/[/?#\s]/)[0];
  s = s.replace(/^.*@/, '').replace(/:\d*$/, '');
  s = s.replace(/^(\*\.)+/, '').replace(/^www\./, '').replace(/\.$/, '');
  if (!/^(?=.{1,253}$)([a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(s)) return null;
  return s;
}

function expandDomain(d) {
  const out = [d, 'www.' + d];
  // Mobile prefixes only make sense on a registrable domain, not on a deep subdomain.
  if (d.split('.').length <= 3) out.push('m.' + d, 'mobile.' + d);
  return uniq(out.concat(RELATED[d] || []));
}

function blockedDomains(activeBlocks) {
  const set = new Set();
  for (const b of activeBlocks) for (const s of b.sites) for (const h of expandDomain(s)) set.add(h);
  return [...set].sort();
}

// Program names each active block wants closed / covered. Close wins when two blocks disagree.
// coverSupported: false on systems where covering isn't available, so everything is closed.
function blockedApps(activeBlocks, coverSupported) {
  const close = new Set();
  const cover = new Set();
  for (const b of activeBlocks) for (const a of b.apps) (a.mode === 'cover' && coverSupported ? cover : close).add(a.name);
  for (const n of close) cover.delete(n);
  return { close: [...close].sort(), cover: [...cover].sort() };
}

function escapeRegExp(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

// Returns hosts file content with our section replaced (or removed when domains is empty).
function applyHostsSection(content, domains) {
  const eol = content.includes('\r\n') ? '\r\n' : '\n';
  const re = new RegExp(escapeRegExp(MARK_BEGIN) + '[\\s\\S]*?' + escapeRegExp(MARK_END) + '[ \\t]*(\\r?\\n)?', 'g');
  let base = content.replace(re, '');
  if (!domains.length) return base;
  if (base.length && !base.endsWith('\n')) base += eol;
  const lines = [MARK_BEGIN];
  for (const d of domains) lines.push('0.0.0.0 ' + d, ':: ' + d);
  lines.push(MARK_END);
  return base + lines.join(eol) + eol;
}

// ---------- apps ----------

function appKey(name) {
  let s = String(name == null ? '' : name).trim();
  s = s.split(/[\\/]/).pop().toLowerCase();
  s = s.replace(/\.(exe|app)$/, '').trim();
  return s;
}

function normalizeApp(input) {
  const key = appKey(input);
  if (!key) return { error: 'Type the name of a program.' };
  if (key.length > 100) return { error: 'That program name is too long.' };
  if (PROTECTED_APPS.has(key) || key.startsWith('programprison')) {
    return { error: `“${key}” is part of the system or of ProgramPrison and can't be blocked.` };
  }
  return { value: key };
}

// names: every name a running process goes by (exe name, macOS bundle name...).
function appMatches(names, apps) {
  for (const n of names) {
    const k = appKey(n);
    if (!k || PROTECTED_APPS.has(k)) continue;
    for (const a of apps) {
      if (k === a) return true;
      // Linux truncates process names to 15 characters.
      if (k.length === 15 && a.length > 15 && a.startsWith(k)) return true;
    }
  }
  return false;
}

// ---------- edits and locks ----------

// While a block is locked you may add to it but not take anything away.
function checkEdit(old, next, now) {
  if (!isLocked(old, now)) return null;
  const goneSite = old.sites.find((s) => !next.sites.includes(s));
  if (goneSite) return `“${old.name}” is locked: you can't remove ${goneSite} until it ends.`;
  for (const a of old.apps) {
    const n = next.apps.find((x) => x.name === a.name);
    if (!n) return `“${old.name}” is locked: you can't remove ${a.name} until it ends.`;
    if (a.mode === 'close' && n.mode === 'cover') return `“${old.name}” is locked: ${a.name} can't be switched from closing to covering until it ends.`;
  }
  if (old.schedule.some((on, i) => on && !next.schedule[i])) return `“${old.name}” is locked: you can add scheduled hours but not remove them.`;
  return null;
}

// Builds a block from what the window sent: only name, sites, apps and schedule can change this way.
function applyEdit(old, input) {
  const next = { ...old };
  if (input.name !== undefined) next.name = cleanName(input.name) || old.name;
  if (Array.isArray(input.sites)) next.sites = uniq(input.sites.map(normalizeDomain).filter(Boolean));
  if (Array.isArray(input.apps)) next.apps = cleanApps(input.apps);
  if (Array.isArray(input.schedule)) next.schedule = old.schedule.map((_, i) => input.schedule[i] === true);
  return next;
}

function hashPassword(pw) {
  const salt = crypto.randomBytes(16).toString('hex');
  const hash = crypto.scryptSync(String(pw), salt, 32).toString('hex');
  return { salt, hash };
}

function checkPassword(lock, pw) {
  if (!lock || !lock.salt || !lock.hash) return false;
  const h = crypto.scryptSync(String(pw == null ? '' : pw), lock.salt, 32);
  const want = Buffer.from(lock.hash, 'hex');
  return want.length === h.length && crypto.timingSafeEqual(h, want);
}

// Returns { lock } or { error }.
function buildLock(old, input, now, locked) {
  const type = input && input.type;
  if (!['none', 'random', 'password', 'locked'].includes(type)) return { error: 'Unknown lock type.' };
  if (locked) {
    const same = type === old.lock.type && (type !== 'random' || Number(input.length) >= old.lock.length) && !(type === 'password' && input.password);
    if (!same && type !== 'locked') return { error: 'The block is locked: the lock can only be made stronger (Locked) until it ends.' };
  }
  if (type === 'none' || type === 'locked') return { lock: { type } };
  if (type === 'random') {
    const length = Math.round(Number(input.length));
    if (!(length >= 20 && length <= 2000)) return { error: 'Random text length must be between 20 and 2000 characters.' };
    return { lock: { type, length } };
  }
  const pw = input.password == null ? '' : String(input.password);
  if (!pw) {
    if (old.lock.type === 'password') return { lock: old.lock };
    return { error: 'Type a password for the lock.' };
  }
  if (pw.length < 4) return { error: 'Use a password of at least 4 characters.' };
  return { lock: { type, ...hashPassword(pw) } };
}

const RANDOM_ALPHABET = 'abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789';

function randomText(n) {
  let s = '';
  for (let i = 0; i < n; i++) s += RANDOM_ALPHABET[crypto.randomInt(RANDOM_ALPHABET.length)];
  return s;
}

// What the window gets: never the password hash.
function publicBlock(b, now) {
  const until = activeUntil(b, now);
  const active = isActive(b, now);
  return {
    id: b.id,
    name: b.name,
    sites: b.sites.slice(),
    apps: b.apps.map((a) => ({ ...a })),
    schedule: b.schedule.slice(),
    lock: b.lock.type === 'random' ? { type: 'random', length: b.lock.length } : { type: b.lock.type },
    status: {
      active,
      locked: active && b.lock.type !== 'none',
      until: active && until !== Infinity ? until : null,
      forever: active && until === Infinity,
      scheduled: b.schedule.some(Boolean),
      unlockedUntil: !active && b.unlockedUntil > now ? b.unlockedUntil : null,
    },
  };
}

function dayKey(t) {
  const d = new Date(t);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

function recordClosed(state, now, n) {
  const k = dayKey(now);
  state.stats.appsClosed[k] = (state.stats.appsClosed[k] || 0) + n;
  const keys = Object.keys(state.stats.appsClosed).sort();
  while (keys.length > 60) delete state.stats.appsClosed[keys.shift()];
}

module.exports = {
  HOUR, DAY, SLOTS, MARK_BEGIN, MARK_END, PRESETS, PROTECTED_APPS,
  defaultState, newBlock, repairState, repairBlock, cleanName,
  slotOf, scheduledAt, isActive, isLocked, activeUntil, startSession, stopBlock, expireSessions, adjustForClockJump,
  normalizeDomain, expandDomain, blockedDomains, blockedApps, cleanApps, applyHostsSection,
  appKey, normalizeApp, appMatches,
  checkEdit, applyEdit, buildLock, hashPassword, checkPassword, randomText, publicBlock, dayKey, recordClosed,
};
