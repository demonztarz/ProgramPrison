'use strict';
// Covers the windows of blocked programs with an overlay of the same size (Windows only).
// Event driven: Windows tells us when a covered program's window moves, resizes, minimizes,
// closes or comes to the front, so nothing is polled while a cover is up.

const path = require('path');
const { BrowserWindow, screen } = require('electron');

const SYSTEM_EVENTS = [0x0003, 0x0017]; // foreground .. minimize end
const OBJECT_EVENTS = [0x8000, 0x800b]; // create .. location change
const CLOAK_EVENTS = [0x8017, 0x8018];

class CoverManager {
  constructor(win32, { onForegroundPid } = {}) {
    this.w = win32;
    this.onForegroundPid = onForegroundPid || (() => {});
    this.targets = new Map(); // pid -> { info, unhooks }
    this.covers = new Map(); // target hwnd -> { pid, win, hwnd }
    this.pending = new Set(); // pids waiting for a refresh
    this.refreshQueued = false;
    this.globalUnhook = null;
  }

  // Watches for any program coming to the front, so a blocked one is dealt with the moment it
  // appears instead of at the next process check.
  watchForeground(on) {
    if (on && !this.globalUnhook) {
      this.globalUnhook = this.w.hookEvents(0, 0x0003, 0x0003, (_ev, hwnd) => {
        const pid = this.w.windowPid(hwnd);
        if (this.targets.has(pid)) this.raise(hwnd);
        else if (pid) this.onForegroundPid(pid);
      });
    } else if (!on && this.globalUnhook) {
      this.globalUnhook();
      this.globalUnhook = null;
    }
  }

  // targets: Map pid -> { name, until } for every running process that should be covered.
  setTargets(targets) {
    for (const [pid, t] of this.targets) {
      if (!targets.has(pid)) {
        for (const u of t.unhooks) u();
        this.targets.delete(pid);
        for (const [hwnd, c] of this.covers) if (c.pid === pid) this.removeCover(hwnd);
      }
    }
    for (const [pid, info] of targets) {
      const t = this.targets.get(pid);
      if (t) {
        t.info = info;
        for (const c of this.covers.values()) if (c.pid === pid) this.sendInfo(c);
        continue;
      }
      this.addTarget(pid, info);
    }
  }

  addTarget(pid, info) {
    if (this.targets.has(pid)) return;
    const onEvent = (ev, hwnd) => this.onEvent(pid, ev, hwnd);
    const unhooks = [SYSTEM_EVENTS, OBJECT_EVENTS, CLOAK_EVENTS]
      .map(([a, b]) => this.w.hookEvents(pid, a, b, onEvent))
      .filter(Boolean);
    this.targets.set(pid, { info, unhooks });
    this.refresh(pid);
    const fg = this.w.foregroundWindow();
    if (this.covers.has(fg)) this.raise(fg);
  }

  clear() {
    this.setTargets(new Map());
    this.watchForeground(false);
  }

  onEvent(pid, ev, hwnd) {
    const E = this.w.EVENT;
    const c = this.covers.get(hwnd);
    if (ev === E.LOCATIONCHANGE || ev === E.MOVESIZEEND) {
      if (c) this.position(hwnd, c);
      else this.queueRefresh(pid);
    } else if (ev === E.FOREGROUND) {
      this.raise(hwnd);
    } else if (ev === E.REORDER) {
      if (c) this.w.placeAbove(c.hwnd, hwnd);
    } else if ([E.CREATE, E.SHOW, E.HIDE, E.DESTROY, E.MINIMIZESTART, E.MINIMIZEEND, E.CLOAKED, E.UNCLOAKED].includes(ev)) {
      this.queueRefresh(pid);
    }
  }

  // Several events usually arrive together (a window shows, moves and gets focus): handle them once.
  queueRefresh(pid) {
    this.pending.add(pid);
    if (this.refreshQueued) return;
    this.refreshQueued = true;
    setImmediate(() => {
      this.refreshQueued = false;
      const pids = [...this.pending];
      this.pending.clear();
      for (const p of pids) this.refresh(p);
    });
  }

  // Brings the cover in front when its program is brought to the front, so keys go to the cover.
  raise(hwnd) {
    const c = this.covers.get(hwnd);
    if (!c) {
      const pid = this.w.windowPid(hwnd);
      if (this.targets.has(pid)) this.queueRefresh(pid);
      return;
    }
    this.position(hwnd, c);
    this.w.forceForeground(c.hwnd);
  }

  refresh(pid) {
    if (!this.targets.has(pid)) return;
    const wins = this.w.windowsOf(new Set([pid]));
    const seen = new Set(wins.map((x) => x.hwnd));
    for (const [hwnd, c] of this.covers) if (c.pid === pid && !seen.has(hwnd)) this.removeCover(hwnd);
    for (const info of wins) {
      let c = this.covers.get(info.hwnd);
      if (!c) c = this.createCover(info);
      this.position(info.hwnd, c, info);
    }
  }

  createCover(info) {
    const win = new BrowserWindow({
      show: false,
      frame: false,
      resizable: false,
      movable: false,
      minimizable: false,
      maximizable: false,
      fullscreenable: false,
      skipTaskbar: true,
      hasShadow: false,
      thickFrame: false,
      roundedCorners: false,
      backgroundColor: '#1d1b18',
      title: 'Blocked by ProgramPrison',
      webPreferences: { preload: path.join(__dirname, 'ui', 'cover-preload.js'), contextIsolation: true, nodeIntegration: false, sandbox: true },
    });
    win.removeMenu();
    const buf = win.getNativeWindowHandle();
    const hwnd = buf.length >= 8 ? Number(buf.readBigUInt64LE(0)) : buf.readUInt32LE(0);
    const c = { pid: info.pid, win, hwnd, target: info.hwnd, shown: false, destroying: false };
    win.on('close', (e) => { if (!c.destroying) e.preventDefault(); }); // Alt+F4 on the cover does nothing
    win.webContents.on('did-finish-load', () => this.sendInfo(c));
    win.webContents.on('will-navigate', (e) => e.preventDefault());
    win.loadFile(path.join(__dirname, 'ui', 'cover.html'));
    this.covers.set(info.hwnd, c);
    return c;
  }

  sendInfo(c) {
    const t = this.targets.get(c.pid);
    if (t && !c.win.isDestroyed()) c.win.webContents.send('info', t.info);
  }

  position(hwnd, c, info) {
    info = info || this.w.windowInfo(hwnd);
    if (!info) { this.queueRefresh(c.pid); return; }
    const r = info.rect;
    const dip = process.platform === 'win32' ? screen.screenToDipRect(null, r) : r;
    const b = { x: Math.round(dip.x), y: Math.round(dip.y), width: Math.max(1, Math.round(dip.width)), height: Math.max(1, Math.round(dip.height)) };
    const cur = c.win.getBounds();
    if (cur.x !== b.x || cur.y !== b.y || cur.width !== b.width || cur.height !== b.height) c.win.setBounds(b);
    if (c.win.isAlwaysOnTop() !== info.topmost) c.win.setAlwaysOnTop(info.topmost, 'screen-saver');
    if (!c.shown) { c.win.showInactive(); c.shown = true; }
    this.w.placeAbove(c.hwnd, hwnd);
  }

  removeCover(hwnd) {
    const c = this.covers.get(hwnd);
    if (!c) return;
    this.covers.delete(hwnd);
    c.destroying = true;
    if (!c.win.isDestroyed()) c.win.destroy();
  }

  // For tests and the status line.
  snapshot() {
    return [...this.covers.entries()].map(([target, c]) => ({ target, pid: c.pid, bounds: c.win.isDestroyed() ? null : c.win.getBounds(), visible: !c.win.isDestroyed() && c.win.isVisible() }));
  }
}

module.exports = { CoverManager };
