'use strict';
// Direct Windows API calls (through koffi), used for a cheap process list and for covering windows.
// Only loaded on Windows.

const koffi = require('koffi');

const user32 = koffi.load('user32.dll');
const kernel32 = koffi.load('kernel32.dll');
const dwmapi = koffi.load('dwmapi.dll');

// Window handles are kept as plain numbers (they fit in 32 bits even on 64-bit Windows).
const HWND = 'intptr_t';

const RECT = koffi.struct('RECT', { left: 'int32_t', top: 'int32_t', right: 'int32_t', bottom: 'int32_t' });
const PROCESSENTRY32W = koffi.struct('PROCESSENTRY32W', {
  dwSize: 'uint32_t',
  cntUsage: 'uint32_t',
  th32ProcessID: 'uint32_t',
  th32DefaultHeapID: 'uintptr_t',
  th32ModuleID: 'uint32_t',
  cntThreads: 'uint32_t',
  th32ParentProcessID: 'uint32_t',
  pcPriClassBase: 'int32_t',
  dwFlags: 'uint32_t',
  szExeFile: koffi.array('char16_t', 260, 'String'),
});

const EnumWindowsProc = koffi.proto('bool __stdcall EnumWindowsProc(intptr_t hwnd, intptr_t lParam)');
const WinEventProc = koffi.proto('void __stdcall WinEventProc(intptr_t hook, uint32_t event, intptr_t hwnd, int32_t idObject, int32_t idChild, uint32_t thread, uint32_t time)');

const EnumWindows = user32.func('bool __stdcall EnumWindows(EnumWindowsProc *cb, intptr_t lParam)');
const GetWindowThreadProcessId = user32.func('uint32_t __stdcall GetWindowThreadProcessId(intptr_t hwnd, _Out_ uint32_t *pid)');
const IsWindowVisible = user32.func('bool __stdcall IsWindowVisible(intptr_t hwnd)');
const IsWindow = user32.func('bool __stdcall IsWindow(intptr_t hwnd)');
const IsIconic = user32.func('bool __stdcall IsIconic(intptr_t hwnd)');
const GetWindowRect = user32.func('bool __stdcall GetWindowRect(intptr_t hwnd, _Out_ RECT *rect)');
const GetWindowLongW = user32.func('int32_t __stdcall GetWindowLongW(intptr_t hwnd, int32_t index)');
const GetWindow = user32.func('intptr_t __stdcall GetWindow(intptr_t hwnd, uint32_t cmd)');
const GetForegroundWindow = user32.func('intptr_t __stdcall GetForegroundWindow()');
const SetForegroundWindow = user32.func('bool __stdcall SetForegroundWindow(intptr_t hwnd)');
const BringWindowToTop = user32.func('bool __stdcall BringWindowToTop(intptr_t hwnd)');
const SetWindowPos = user32.func('bool __stdcall SetWindowPos(intptr_t hwnd, intptr_t after, int x, int y, int cx, int cy, uint32_t flags)');
const AttachThreadInput = user32.func('bool __stdcall AttachThreadInput(uint32_t idAttach, uint32_t idAttachTo, bool attach)');
const SetWinEventHook = user32.func('intptr_t __stdcall SetWinEventHook(uint32_t eventMin, uint32_t eventMax, intptr_t hmod, WinEventProc *cb, uint32_t idProcess, uint32_t idThread, uint32_t flags)');
const UnhookWinEvent = user32.func('bool __stdcall UnhookWinEvent(intptr_t hook)');
const DwmGetWindowAttribute = dwmapi.func('int32_t __stdcall DwmGetWindowAttribute(intptr_t hwnd, uint32_t attr, void *value, uint32_t size)');

const CreateToolhelp32Snapshot = kernel32.func('intptr_t __stdcall CreateToolhelp32Snapshot(uint32_t flags, uint32_t pid)');
const Process32FirstW = kernel32.func('bool __stdcall Process32FirstW(intptr_t snap, _Inout_ PROCESSENTRY32W *entry)');
const Process32NextW = kernel32.func('bool __stdcall Process32NextW(intptr_t snap, _Inout_ PROCESSENTRY32W *entry)');
const CloseHandle = kernel32.func('bool __stdcall CloseHandle(intptr_t h)');
const OpenProcess = kernel32.func('intptr_t __stdcall OpenProcess(uint32_t access, bool inherit, uint32_t pid)');
const QueryFullProcessImageNameW = kernel32.func('bool __stdcall QueryFullProcessImageNameW(intptr_t proc, uint32_t flags, void *name, _Inout_ uint32_t *size)');
const GetCurrentThreadId = kernel32.func('uint32_t __stdcall GetCurrentThreadId()');

const TH32CS_SNAPPROCESS = 0x2;
const INVALID_HANDLE = -1;
const PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
const GW_OWNER = 4;
const GW_HWNDPREV = 3;
const GWL_EXSTYLE = -20;
const WS_EX_TOPMOST = 0x8;
const WS_EX_TOOLWINDOW = 0x80;
const DWMWA_EXTENDED_FRAME_BOUNDS = 9;
const DWMWA_CLOAKED = 14;
const HWND_TOP = 0;
const SWP_NOSIZE = 0x1;
const SWP_NOMOVE = 0x2;
const SWP_NOACTIVATE = 0x10;
const WINEVENT_OUTOFCONTEXT = 0x0;
const WINEVENT_SKIPOWNPROCESS = 0x2;

const EVENT = {
  FOREGROUND: 0x0003,
  MOVESIZEEND: 0x000b,
  MINIMIZESTART: 0x0016,
  MINIMIZEEND: 0x0017,
  CREATE: 0x8000,
  DESTROY: 0x8001,
  SHOW: 0x8002,
  HIDE: 0x8003,
  REORDER: 0x8004,
  LOCATIONCHANGE: 0x800b,
  CLOAKED: 0x8017,
  UNCLOAKED: 0x8018,
};

function listProcesses() {
  const snap = Number(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0));
  if (snap === INVALID_HANDLE || snap === 0) return [];
  const out = [];
  try {
    const entry = { dwSize: koffi.sizeof(PROCESSENTRY32W) };
    let ok = Process32FirstW(snap, entry);
    while (ok) {
      out.push({ pid: entry.th32ProcessID, names: [entry.szExeFile] });
      ok = Process32NextW(snap, entry);
    }
  } finally {
    CloseHandle(snap);
  }
  return out;
}

function processImageName(pid) {
  const h = Number(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid));
  if (!h) return null;
  try {
    const buf = Buffer.alloc(1040);
    const size = [520];
    if (!QueryFullProcessImageNameW(h, 0, buf, size)) return null;
    return buf.toString('utf16le', 0, size[0] * 2);
  } finally {
    CloseHandle(h);
  }
}

function windowPid(hwnd) {
  const pid = [0];
  GetWindowThreadProcessId(hwnd, pid);
  return pid[0];
}

function isCloaked(hwnd) {
  const buf = Buffer.alloc(4);
  return DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, buf, 4) === 0 && buf.readUInt32LE(0) !== 0;
}

// Visible bounds in physical pixels (without the invisible resize border Windows 10/11 adds).
function windowRect(hwnd) {
  const buf = Buffer.alloc(16);
  if (DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, buf, 16) === 0) {
    return { x: buf.readInt32LE(0), y: buf.readInt32LE(4), width: buf.readInt32LE(8) - buf.readInt32LE(0), height: buf.readInt32LE(12) - buf.readInt32LE(4) };
  }
  const r = {};
  if (!GetWindowRect(hwnd, r)) return null;
  return { x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top };
}

// A window worth covering: visible, not minimized or cloaked, a real size.
function coverable(hwnd) {
  if (!IsWindowVisible(hwnd) || IsIconic(hwnd) || isCloaked(hwnd)) return false;
  const ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
  if (ex & WS_EX_TOOLWINDOW && !GetWindow(hwnd, GW_OWNER)) return false;
  const r = windowRect(hwnd);
  return !!r && r.width >= 40 && r.height >= 30;
}

// Top-level windows of these processes that should be covered: [{ hwnd, pid, rect, topmost }].
function windowsOf(pids) {
  const found = [];
  if (!pids.size) return found;
  EnumWindows((hwnd) => {
    hwnd = Number(hwnd);
    const pid = windowPid(hwnd);
    if (pids.has(pid) && coverable(hwnd)) {
      found.push({ hwnd, pid, rect: windowRect(hwnd), topmost: !!(GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST) });
    }
    return true;
  }, 0);
  return found;
}

function windowInfo(hwnd) {
  if (!IsWindow(hwnd) || !coverable(hwnd)) return null;
  return { hwnd, pid: windowPid(hwnd), rect: windowRect(hwnd), topmost: !!(GetWindowLongW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST) };
}

// Puts `cover` directly above `target` in the z-order without activating it, so other programs
// can still be dragged over both.
function placeAbove(cover, target) {
  const prev = Number(GetWindow(target, GW_HWNDPREV));
  if (prev === cover) return; // already there; avoids a reorder event ping-pong
  const after = prev || HWND_TOP;
  SetWindowPos(cover, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
}

// Windows only lets the program that has the keyboard take the foreground; joining its input
// queue for a moment is the usual way round that.
function forceForeground(hwnd) {
  const fg = Number(GetForegroundWindow());
  if (fg === hwnd) return true;
  const fgThread = fg ? GetWindowThreadProcessId(fg, [0]) : 0;
  const me = GetCurrentThreadId();
  const attach = fgThread && fgThread !== me;
  if (attach) AttachThreadInput(me, fgThread, true);
  BringWindowToTop(hwnd);
  const ok = SetForegroundWindow(hwnd);
  if (attach) AttachThreadInput(me, fgThread, false);
  return ok;
}

// Calls onEvent(event, hwnd) for top-level window events. pid 0 = every process.
// Returns a function that removes the hook.
function hookEvents(pid, eventMin, eventMax, onEvent) {
  const cb = koffi.register((_hook, event, hwnd, idObject, idChild) => {
    if (idObject !== 0 || idChild !== 0) return; // only the window itself, not carets, cursors, controls
    try { onEvent(event, Number(hwnd)); } catch (e) { console.error(e); }
  }, koffi.pointer(WinEventProc));
  const hook = Number(SetWinEventHook(eventMin, eventMax, 0, cb, pid, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS));
  if (!hook) {
    koffi.unregister(cb);
    return null;
  }
  return () => {
    UnhookWinEvent(hook);
    koffi.unregister(cb);
  };
}

function foregroundWindow() {
  return Number(GetForegroundWindow());
}

module.exports = { EVENT, listProcesses, processImageName, windowPid, windowsOf, windowInfo, placeAbove, forceForeground, hookEvents, foregroundWindow };
