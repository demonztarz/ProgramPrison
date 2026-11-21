'use strict';
const { app, BrowserWindow, Tray, Menu, ipcMain, nativeImage, powerMonitor, dialog, Notification } = require('electron');
const fs = require('fs');
const os = require('os');
const path = require('path');
const { performance } = require('perf_hooks');
const engine = require('./engine');
const enforcer = require('./enforcer');

// Tests point the app at a scratch hosts file and data folder.
const HOSTS_PATH = process.env.PROGRAMPRISON_HOSTS || enforcer.HOSTS_PATH;
if (process.env.PROGRAMPRISON_DATA) app.setPath('userData', process.env.PROGRAMPRISON_DATA);
const TICK_MS = Number(process.env.PROGRAMPRISON_TICK_MS) || 3000;
const startHidden = process.argv.includes('--hidden');
// --self-test=<file>: write what works on this machine to <file> and exit (used to check builds).
const selfTestArg = process.argv.find((a) => a.startsWith('--self-test='));

if (!app.requestSingleInstanceLock()) {
  app.quit();
  return;
}

let win = null;
let tray = null;
let state = engine.defaultState();
let hostsStatus = { ok: true, writable: true };
let lastDomainsKey = null;
let lastHostsCheck = 0;
let checkingApps = false; // a program check is running
let covers = null; // CoverManager, Windows only
let allowQuit = false;
let asleep = false;
let lastWall = Date.now();
let lastMono = performance.now();
const challenges = new Map(); // block id -> random text the user must type

const statePath = () => path.join(app.getPath('userData'), 'state.json');

function loadState() {
  try {
    state = engine.repairState(JSON.parse(fs.readFileSync(statePath(), 'utf8')));
  } catch {
    state = engine.defaultState();
  }
}

function saveState() {
  const file = statePath();
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const tmp = file + '.tmp';
  fs.writeFileSync(tmp, JSON.stringify(state, null, 1));
  fs.renameSync(tmp, file);
}

function anyLocked(now = Date.now()) {
  return state.blocks.some((b) => engine.isLocked(b, now));
}

function publicState() {
  const now = Date.now();
  const writable = enforcer.canWriteHosts(HOSTS_PATH);
  return {
    now,
    blocks: state.blocks.map((b) => engine.publicBlock(b, now)),
    settings: { ...state.settings },
    closedToday: state.stats.appsClosed[engine.dayKey(now)] || 0,
    presets: engine.PRESETS,
    hosts: { ...hostsStatus, writable, path: HOSTS_PATH },
    platform: process.platform,
    coverSupported: !!covers,
    version: app.getVersion(),
  };
}

// The window counts down by itself; it only needs fresh state when it's on screen.
function broadcast(force) {
  if (win && !win.isDestroyed() && (force || win.isVisible())) win.webContents.send('state', publicState());
  updateTray();
}

// ---------- enforcement ----------

function enforce(force) {
  const now = Date.now();
  if (engine.expireSessions(state.blocks, now)) saveState();
  const active = state.blocks.filter((b) => engine.isActive(b, now));

  const domains = engine.blockedDomains(active);
  const key = domains.join(' ');
  // Re-check the file now and then even when nothing changed, to undo hand edits.
  if (force || key !== lastDomainsKey || now - lastHostsCheck > 10000) {
    lastHostsCheck = now;
    const r = enforcer.syncHosts(domains, HOSTS_PATH);
    hostsStatus = { ok: r.ok, needsSetup: !!r.needsSetup, error: r.ok ? null : r.error, blocking: r.ok ? domains.length : 0 };
    if (r.ok) lastDomainsKey = key;
    else lastDomainsKey = null;
  }

  if (!checkingApps) {
    checkingApps = true;
    checkApps(active, now).catch((e) => console.error('app check failed:', e)).finally(() => { checkingApps = false; });
  }
}

// Which block a covered program belongs to, for the text on the cover.
function coverInfo(names, active, now) {
  const b = active.find((x) => x.apps.some((a) => a.mode === 'cover' && engine.appMatches(names, [a.name])));
  const until = b ? engine.activeUntil(b, now) : null;
  return { app: names[0].replace(/\.exe$/i, ''), block: b ? b.name : 'ProgramPrison', until: Number.isFinite(until) ? until : null };
}

async function checkApps(active, now) {
  const { close, cover } = engine.blockedApps(active, !!covers);
  if (covers) covers.watchForeground(close.length + cover.length > 0);
  if (!close.length && !cover.length) {
    if (covers) covers.setTargets(new Map());
    return;
  }
  const procs = await enforcer.listProcesses();
  const closed = await enforcer.closeMatching(close, procs);
  if (covers) {
    const targets = new Map();
    for (const p of procs) {
      if (p.pid !== process.pid && engine.appMatches(p.names, cover)) targets.set(p.pid, coverInfo(p.names, active, now));
    }
    covers.setTargets(targets);
  }
  if (closed.length) reportClosed(closed);
}

function reportClosed(closed) {
  engine.recordClosed(state, Date.now(), closed.length);
  saveState();
  notify('Blocked', `${[...new Set(closed)].join(', ')} ${closed.length === 1 ? 'was' : 'were'} closed by ProgramPrison.`);
  broadcast(true);
}

// A program just came to the front that wasn't seen at the last check: deal with it right away.
function onForegroundPid(pid) {
  if (pid === process.pid) return;
  const name = enforcer.win32.processImageName(pid);
  if (!name) return;
  const now = Date.now();
  const active = state.blocks.filter((b) => engine.isActive(b, now));
  const { close, cover } = engine.blockedApps(active, true);
  const names = [name.split('\\').pop()];
  if (engine.appMatches(names, close)) {
    enforcer.killPid(pid).then((ok) => { if (ok) reportClosed(names); });
  } else if (engine.appMatches(names, cover)) {
    covers.addTarget(pid, coverInfo(names, active, now));
  }
}

function tick() {
  const wall = Date.now();
  const mono = performance.now();
  if (!asleep) {
    const jump = engine.adjustForClockJump(state.blocks, lastWall, wall, mono - lastMono);
    if (jump) saveState();
  }
  lastWall = wall;
  lastMono = mono;
  enforce(false);
  broadcast();
}

let lastNotice = 0;
function notify(title, body) {
  if (Date.now() - lastNotice < 5000 || !Notification.isSupported()) return;
  lastNotice = Date.now();
  new Notification({ title, body, silent: true }).show();
}

// ---------- start at login ----------

function applyLoginItem() {
  if (!app.isPackaged) return; // don't register a development copy
  const on = state.settings.startAtLogin;
  if (process.platform === 'linux') {
    const dir = path.join(os.homedir(), '.config', 'autostart');
    const file = path.join(dir, 'programprison.desktop');
    try {
      if (on) {
        const exe = process.env.APPIMAGE || process.execPath;
        fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(file, `[Desktop Entry]\nType=Application\nName=ProgramPrison\nExec="${exe}" --hidden\nX-GNOME-Autostart-enabled=true\n`);
      } else if (fs.existsSync(file)) fs.unlinkSync(file);
    } catch { /* not fatal */ }
    return;
  }
  const exe = process.env.PORTABLE_EXECUTABLE_FILE || process.execPath;
  app.setLoginItemSettings({ openAtLogin: on, openAsHidden: true, path: exe, args: ['--hidden'] });
}

// ---------- window and tray ----------

function iconPath(name) {
  return path.join(__dirname, 'assets', name);
}

function createWindow() {
  win = new BrowserWindow({
    width: 1040,
    height: 720,
    minWidth: 760,
    minHeight: 540,
    show: false,
    title: 'ProgramPrison',
    icon: iconPath('icon-256.png'),
    backgroundColor: '#f6f5f2',
    autoHideMenuBar: true,
    webPreferences: { preload: path.join(__dirname, 'preload.js'), contextIsolation: true, nodeIntegration: false, sandbox: true },
  });
  win.removeMenu();
  win.loadFile(path.join(__dirname, 'ui', 'index.html'));
  win.once('ready-to-show', () => { if (!startHidden) win.show(); });
  win.on('show', () => broadcast(true));
  // Closing the window keeps the blocker running in the tray.
  win.on('close', (e) => {
    if (allowQuit) return;
    e.preventDefault();
    win.hide();
    if (!state.settings.hideNoticeShown) {
      state.settings.hideNoticeShown = true;
      saveState();
      notify('ProgramPrison is still running', 'Blocks keep working in the background. Use the tray icon to open it again or quit.');
    }
  });
  win.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  win.webContents.on('will-navigate', (e) => e.preventDefault());
}

function showWindow() {
  if (!win || win.isDestroyed()) createWindow();
  if (win.isMinimized()) win.restore();
  win.show();
  win.focus();
}

function tryQuit() {
  if (anyLocked()) {
    showWindow();
    dialog.showMessageBox(win, { type: 'info', title: 'ProgramPrison', message: "A locked block is running, so ProgramPrison can't be closed until it ends." });
    return;
  }
  allowQuit = true;
  app.quit();
}

function updateTray() {
  if (!tray) return;
  const now = Date.now();
  const active = state.blocks.filter((b) => engine.isActive(b, now));
  tray.setToolTip(active.length ? `ProgramPrison: ${active.map((b) => b.name).join(', ')} on` : 'ProgramPrison: no blocks on');
  const locked = anyLocked(now);
  const key = `${active.length}|${locked}`;
  if (tray._key === key) return;
  tray._key = key;
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: active.length ? `${active.length} block${active.length > 1 ? 's' : ''} on` : 'No blocks on', enabled: false },
    { type: 'separator' },
    { label: 'Open ProgramPrison', click: showWindow },
    { label: locked ? 'Quit (locked)' : 'Quit', enabled: !locked, click: tryQuit },
  ]));
}

function createTray() {
  const img = nativeImage.createFromPath(iconPath(process.platform === 'darwin' ? 'tray-mac.png' : 'tray.png'));
  if (process.platform === 'darwin') img.setTemplateImage(true);
  tray = new Tray(img);
  tray.on('click', showWindow);
  updateTray();
}

// ---------- IPC ----------

function findBlock(id) {
  return state.blocks.find((b) => b.id === id);
}

function changed() {
  saveState();
  enforce(true);
  return { ok: true, state: publicState() };
}

function fail(error) {
  return { ok: false, error, state: publicState() };
}

const handlers = {
  'state:get': () => publicState(),

  'block:create': (_e, name) => {
    const b = engine.newBlock(name || `Block ${state.blocks.length + 1}`);
    state.blocks.push(b);
    const r = changed();
    r.id = b.id;
    return r;
  },

  'block:update': (_e, input) => {
    const old = input && findBlock(input.id);
    if (!old) return fail('That block no longer exists.');
    const next = engine.applyEdit(old, input);
    const err = engine.checkEdit(old, next, Date.now());
    if (err) return fail(err);
    Object.assign(old, next);
    return changed();
  },

  'block:delete': (_e, id) => {
    const b = findBlock(id);
    if (!b) return fail('That block no longer exists.');
    if (engine.isLocked(b, Date.now())) return fail(`“${b.name}” is locked and can't be deleted until it ends.`);
    state.blocks = state.blocks.filter((x) => x !== b);
    return changed();
  },

  'block:start': (_e, { id, endsAt }) => {
    const b = findBlock(id);
    if (!b) return fail('That block no longer exists.');
    const err = engine.startSession(b, Date.now(), Number(endsAt));
    if (err) return fail(err);
    return changed();
  },

  'block:setLock': (_e, input) => {
    const b = input && findBlock(input.id);
    if (!b) return fail('That block no longer exists.');
    const r = engine.buildLock(b, input, Date.now(), engine.isLocked(b, Date.now()));
    if (r.error) return fail(r.error);
    b.lock = r.lock;
    challenges.delete(b.id);
    return changed();
  },

  // What the user has to do to stop a block early.
  'block:challenge': (_e, id) => {
    const b = findBlock(id);
    if (!b) return { type: 'gone' };
    const now = Date.now();
    if (!engine.isActive(b, now) || b.lock.type === 'none') return { type: 'none' };
    if (b.lock.type === 'locked') return { type: 'locked', until: engine.activeUntil(b, now) };
    if (b.lock.type === 'password') return { type: 'password' };
    const text = engine.randomText(b.lock.length);
    challenges.set(b.id, text);
    return { type: 'random', text };
  },

  'block:stop': (_e, { id, answer }) => {
    const b = findBlock(id);
    if (!b) return fail('That block no longer exists.');
    const now = Date.now();
    if (engine.isActive(b, now) && b.lock.type !== 'none') {
      if (b.lock.type === 'locked') return fail(`“${b.name}” is locked until it ends.`);
      if (b.lock.type === 'password' && !engine.checkPassword(b.lock, answer)) return fail('Wrong password.');
      if (b.lock.type === 'random') {
        const want = challenges.get(b.id);
        if (!want || String(answer || '').replace(/\s+/g, '') !== want) return fail("The text doesn't match. Check every character, upper and lower case count.");
      }
    }
    challenges.delete(b.id);
    engine.stopBlock(b, now);
    return changed();
  },

  'settings:update': (_e, input) => {
    if (input && typeof input.startAtLogin === 'boolean') {
      if (!input.startAtLogin && anyLocked()) return fail("Start at login can't be turned off while a locked block is running.");
      state.settings.startAtLogin = input.startAtLogin;
      applyLoginItem();
    }
    return changed();
  },

  'hosts:setup': async () => {
    const r = await enforcer.grantHostsAccess();
    lastDomainsKey = null;
    const out = changed();
    if (!r.ok) { out.ok = false; out.error = r.error; }
    return out;
  },

  'apps:running': () => enforcer.runningAppNames(),
};

for (const [ch, fn] of Object.entries(handlers)) ipcMain.handle(ch, fn);

// ---------- lifecycle ----------

app.on('second-instance', showWindow);

app.on('before-quit', (e) => {
  if (allowQuit) return;
  if (anyLocked()) {
    e.preventDefault();
    showWindow();
    return;
  }
  allowQuit = true;
});

// Never stand in the way of the computer shutting down or logging out.
powerMonitor.on('shutdown', () => { allowQuit = true; saveState(); });
powerMonitor.on('suspend', () => { asleep = true; });
powerMonitor.on('resume', () => {
  asleep = false;
  lastWall = Date.now();
  lastMono = performance.now();
  enforce(true);
});

app.on('window-all-closed', () => { /* keep running in the tray */ });
app.on('activate', showWindow);

async function selfTest(file) {
  const out = { version: app.getVersion(), platform: process.platform, packaged: app.isPackaged };
  try {
    out.processes = (await enforcer.listProcesses()).length;
    out.nativeProcessList = !!enforcer.win32;
    if (enforcer.win32) {
      require('./covers');
      out.foreground = enforcer.win32.foregroundWindow() > 0;
      out.ownWindowsFound = enforcer.win32.windowsOf(new Set([process.pid])).length >= 0;
    }
    out.ok = out.processes > 0;
  } catch (e) {
    out.ok = false;
    out.error = String(e && e.stack || e);
  }
  fs.writeFileSync(file, JSON.stringify(out, null, 2));
  allowQuit = true;
  app.exit(0);
}

app.whenReady().then(() => {
  if (selfTestArg) { selfTest(selfTestArg.slice('--self-test='.length)); return; }
  if (process.platform === 'darwin' && app.dock) app.dock.setIcon(iconPath('icon-256.png'));
  loadState();
  applyLoginItem();
  createWindow();
  win.on('session-end', () => { allowQuit = true; saveState(); }); // Windows logoff/shutdown
  createTray();
  if (enforcer.win32) {
    try {
      covers = new (require('./covers').CoverManager)(enforcer.win32, { onForegroundPid });
    } catch (e) {
      console.error('Covering windows is unavailable:', e);
    }
  }
  if (process.env.PROGRAMPRISON_TEST) {
    global.__prison = { covers: () => (covers ? covers.snapshot() : null), state: () => state, enforce: () => enforce(true), coverSupported: () => !!covers };
  }
  enforce(true);
  setInterval(tick, TICK_MS);
});
