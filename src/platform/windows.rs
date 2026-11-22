//! Windows: everything runs on one thread around the Windows message loop.
//!
//! - A timer runs the enforcement pass every few seconds.
//! - Windows tells us when a covered program's windows move, resize, minimize, close or come
//!   to the front (WinEvent hooks), so covers follow without any polling.
//! - Covers are plain native windows painted with GDI.
//! - The tray icon lives on a hidden window on the same thread.

use crate::engine;
use crate::ipc::{self, CoverInfo, Request, Response};
use crate::procs::Proc;
use crate::service::{self, Service};
use crate::util;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{w, BOOL, PCWSTR, PWSTR};

const WM_APP_ENFORCE: u32 = WM_APP + 1; // state changed: enforce now
const WM_APP_EVENTS: u32 = WM_APP + 2; // window events queued
const WM_APP_TRAY: u32 = WM_APP + 3; // tray icon clicked
const WM_APP_SNAPSHOT: u32 = WM_APP + 4; // tests: copy cover positions into COVER_SNAP
static COVER_SNAP: Mutex<Option<Vec<CoverInfo>>> = Mutex::new(None);
const TIMER_TICK: usize = 1;
const TIMER_COUNTDOWN: usize = 2;
const TIMER_RETRY: usize = 3; // work that arrived while busy, tried again shortly
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const EVENT_SYSTEM_MOVESIZEEND: u32 = 0x000B;
const EVENT_SYSTEM_MINIMIZEEND: u32 = 0x0017;
const EVENT_OBJECT_CREATE: u32 = 0x8000;
const EVENT_OBJECT_REORDER: u32 = 0x8004;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
const EVENT_OBJECT_CLOAKED: u32 = 0x8017;
const EVENT_OBJECT_UNCLOAKED: u32 = 0x8018;
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const WINEVENT_SKIPOWNPROCESS: u32 = 2;

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

fn hwnd_of(v: isize) -> HWND {
    HWND(v as *mut _)
}
fn key(h: HWND) -> isize {
    h.0 as isize
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn post(msg: u32) {
    let h = MAIN_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(hwnd_of(h)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

// ---------- processes ----------

pub fn list_processes() -> Vec<Proc> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(Proc { pid: e.th32ProcessID, names: vec![String::from_utf16_lossy(&e.szExeFile[..len])] });
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

pub fn kill_process(pid: u32) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) else { return false };
        let ok = TerminateProcess(h, 1).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

fn image_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..size as usize]).rsplit('\\').next().unwrap_or("").to_string())
    }
}

pub fn set_run_at_login(on: bool, command: &str) {
    unsafe {
        let sub = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
        if on {
            let v = wide(command);
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, sub, w!("ProgramPrison"), REG_SZ.0, Some(v.as_ptr().cast()), (v.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, sub, w!("ProgramPrison"));
        }
    }
}

// ---------- windows of other programs ----------

fn window_pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

/// Visible bounds in physical pixels (without the invisible resize border of Windows 10/11).
fn window_rect(h: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut r as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32).is_ok() {
            return Some(r);
        }
        GetWindowRect(h, &mut r).ok().map(|_| r)
    }
}

fn is_cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, (&mut v as *mut u32).cast(), 4).is_ok() && v != 0 }
}

fn ex_style(h: HWND) -> u32 {
    unsafe { GetWindowLongW(h, GWL_EXSTYLE) as u32 }
}

/// A window worth covering: visible, not minimized or cloaked, a real size.
fn coverable(h: HWND) -> Option<RECT> {
    unsafe {
        if !IsWindow(Some(h)).as_bool() || !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() || is_cloaked(h) {
            return None;
        }
        let owner = GetWindow(h, GW_OWNER).unwrap_or_default();
        if ex_style(h) & WS_EX_TOOLWINDOW.0 != 0 && owner.0.is_null() {
            return None;
        }
    }
    window_rect(h).filter(|r| r.right - r.left >= 40 && r.bottom - r.top >= 30)
}

unsafe extern "system" fn collect_windows(h: HWND, lp: LPARAM) -> BOOL {
    let v = unsafe { &mut *(lp.0 as *mut (u32, Vec<HWND>)) };
    if window_pid(h) == v.0 && coverable(h).is_some() {
        v.1.push(h);
    }
    BOOL(1)
}

fn windows_of(pid: u32) -> Vec<HWND> {
    let mut v: (u32, Vec<HWND>) = (pid, Vec::new());
    unsafe {
        let _ = EnumWindows(Some(collect_windows), LPARAM(&mut v as *mut _ as isize));
    }
    v.1
}

// ---------- window events ----------

thread_local! {
    static EVENTS: RefCell<VecDeque<(u32, isize)>> = const { RefCell::new(VecDeque::new()) };
    static EVENTS_POSTED: Cell<bool> = const { Cell::new(false) };
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    /// What each cover window shows, read while painting.
    static LABELS: RefCell<HashMap<isize, Label>> = RefCell::new(HashMap::new());
}

/// Called by Windows for hooked events. Only queues: the work happens in the message loop.
unsafe extern "system" fn on_win_event(_h: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, id_child: i32, _t: u32, _time: u32) {
    if id_object != 0 || id_child != 0 || hwnd.0.is_null() {
        return; // only whole windows, not carets, cursors or controls
    }
    EVENTS.with(|q| q.borrow_mut().push_back((event, key(hwnd))));
    if !EVENTS_POSTED.with(|p| p.replace(true)) {
        post(WM_APP_EVENTS);
    }
}

fn hook(pid: u32, min: u32, max: u32) -> Option<HWINEVENTHOOK> {
    let h = unsafe { SetWinEventHook(min, max, None, Some(on_win_event), pid, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS) };
    (!h.0.is_null()).then_some(h)
}

// ---------- covers ----------

#[derive(Clone, Debug, Default)]
struct Label {
    app: String,
    block: String,
    until: Option<i64>,
}

struct Target {
    label: Label,
    hooks: Vec<HWINEVENTHOOK>,
}

struct Cover {
    hwnd: HWND,
    pid: u32,
}

#[derive(Default)]
struct Covers {
    targets: HashMap<u32, Target>,
    covers: HashMap<isize, Cover>, // keyed by the covered window
    foreground_hook: Option<HWINEVENTHOOK>,
    pending: Vec<u32>,
}

impl Covers {
    fn watch_foreground(&mut self, on: bool) {
        match (on, self.foreground_hook) {
            (true, None) => self.foreground_hook = hook(0, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND),
            (false, Some(h)) => {
                unsafe {
                    let _ = UnhookWinEvent(h);
                }
                self.foreground_hook = None;
            }
            _ => {}
        }
    }

    fn set_targets(&mut self, targets: HashMap<u32, Label>) {
        let gone: Vec<u32> = self.targets.keys().filter(|p| !targets.contains_key(p)).copied().collect();
        for pid in gone {
            self.remove_target(pid);
        }
        for (pid, label) in targets {
            if let Some(t) = self.targets.get_mut(&pid) {
                t.label = label.clone();
                for (_, c) in self.covers.iter().filter(|(_, c)| c.pid == pid) {
                    LABELS.with(|l| l.borrow_mut().insert(key(c.hwnd), label.clone()));
                }
            } else {
                self.add_target(pid, label);
            }
        }
    }

    fn add_target(&mut self, pid: u32, label: Label) {
        if self.targets.contains_key(&pid) {
            return;
        }
        let hooks = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND),
            (EVENT_OBJECT_CREATE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ]
        .iter()
        .filter_map(|&(a, b)| hook(pid, a, b))
        .collect::<Vec<_>>();
        util::debug(format!("cover target {pid} ({}), {} hooks", label.app, hooks.len()));
        self.targets.insert(pid, Target { label, hooks });
        self.refresh(pid);
        let fg = unsafe { GetForegroundWindow() };
        if self.covers.contains_key(&key(fg)) {
            self.raise(fg);
        }
    }

    fn remove_target(&mut self, pid: u32) {
        if let Some(t) = self.targets.remove(&pid) {
            for h in t.hooks {
                unsafe {
                    let _ = UnhookWinEvent(h);
                }
            }
        }
        let gone: Vec<isize> = self.covers.iter().filter(|(_, c)| c.pid == pid).map(|(k, _)| *k).collect();
        for k in gone {
            self.remove_cover(k);
        }
    }

    fn clear(&mut self) {
        let pids: Vec<u32> = self.targets.keys().copied().collect();
        for p in pids {
            self.remove_target(p);
        }
        self.watch_foreground(false);
    }

    fn on_event(&mut self, event: u32, target: HWND) {
        let k = key(target);
        let pid = self.covers.get(&k).map(|c| c.pid).unwrap_or_else(|| window_pid(target));
        if event == EVENT_SYSTEM_FOREGROUND {
            if self.targets.contains_key(&pid) {
                self.raise(target);
            }
            return; // foreground of other programs is handled by the caller
        }
        if !self.targets.contains_key(&pid) {
            return;
        }
        match event {
            EVENT_OBJECT_LOCATIONCHANGE | EVENT_SYSTEM_MOVESIZEEND | EVENT_OBJECT_REORDER if self.covers.contains_key(&k) => self.position(target),
            _ => {
                if !self.pending.contains(&pid) {
                    self.pending.push(pid);
                }
            }
        }
    }

    fn flush_pending(&mut self) {
        for pid in std::mem::take(&mut self.pending) {
            self.refresh(pid);
        }
    }

    /// Makes the covers of one program match its windows.
    fn refresh(&mut self, pid: u32) {
        let Some(label) = self.targets.get(&pid).map(|t| t.label.clone()) else { return };
        let wins = windows_of(pid);
        let keep: Vec<isize> = wins.iter().map(|h| key(*h)).collect();
        let gone: Vec<isize> = self.covers.iter().filter(|(k, c)| c.pid == pid && !keep.contains(k)).map(|(k, _)| *k).collect();
        for k in gone {
            self.remove_cover(k);
        }
        for h in wins {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.covers.entry(key(h))
                && let Some(c) = create_cover(pid, &label)
            {
                util::debug(format!("cover created for window {} of {pid}", key(h)));
                slot.insert(c);
            }
            self.position(h);
        }
    }

    /// Puts the cover exactly over its window and directly above it in the z-order.
    fn position(&mut self, target: HWND) {
        let Some(c) = self.covers.get(&key(target)) else { return };
        let Some(r) = coverable(target) else {
            let pid = c.pid;
            if !self.pending.contains(&pid) {
                self.pending.push(pid);
            }
            post(WM_APP_EVENTS);
            return;
        };
        unsafe {
            let topmost = ex_style(target) & WS_EX_TOPMOST.0 != 0;
            let prev = GetWindow(target, GW_HWNDPREV).unwrap_or_default();
            let (after, mut flags) = if topmost {
                (HWND_TOPMOST, SWP_NOACTIVATE)
            } else if prev == c.hwnd {
                (HWND::default(), SWP_NOACTIVATE | SWP_NOZORDER) // already right above it
            } else if prev.0.is_null() {
                (HWND_TOP, SWP_NOACTIVATE)
            } else {
                (prev, SWP_NOACTIVATE)
            };
            if !topmost && ex_style(c.hwnd) & WS_EX_TOPMOST.0 != 0 {
                let _ = SetWindowPos(c.hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                flags &= !SWP_NOZORDER;
            }
            let _ = SetWindowPos(c.hwnd, Some(after), r.left, r.top, r.right - r.left, r.bottom - r.top, flags | SWP_SHOWWINDOW);
        }
    }

    /// The covered program was brought to the front: give the keyboard to the cover. Windows
    /// refuses that to background programs at times; then the program is minimized instead.
    /// Async only, so a frozen blocked program can never freeze Program Prison.
    fn raise(&mut self, target: HWND) {
        let Some(c) = self.covers.get(&key(target)) else {
            let pid = window_pid(target);
            if !self.pending.contains(&pid) {
                self.pending.push(pid);
            }
            post(WM_APP_EVENTS);
            return;
        };
        let cover = c.hwnd;
        self.position(target);
        unsafe {
            let _ = BringWindowToTop(cover);
            let ok = SetForegroundWindow(cover).as_bool() && GetForegroundWindow() == cover;
            if !ok {
                let _ = ShowWindowAsync(target, SW_MINIMIZE);
            }
            util::debug(format!("raise {}: {}", key(target), if ok { "focused" } else { "minimized" }));
        }
    }

    fn remove_cover(&mut self, k: isize) {
        if let Some(c) = self.covers.remove(&k) {
            LABELS.with(|l| l.borrow_mut().remove(&key(c.hwnd)));
            unsafe {
                let _ = DestroyWindow(c.hwnd);
            }
        }
    }

    fn snapshot(&self) -> Vec<CoverInfo> {
        self.covers
            .iter()
            .map(|(k, c)| {
                let mut r = RECT::default();
                unsafe {
                    let _ = GetWindowRect(c.hwnd, &mut r);
                }
                CoverInfo {
                    target: *k as i64,
                    pid: c.pid,
                    x: r.left,
                    y: r.top,
                    width: r.right - r.left,
                    height: r.bottom - r.top,
                    visible: unsafe { IsWindowVisible(c.hwnd).as_bool() },
                }
            })
            .collect()
    }
}

fn hinstance() -> HINSTANCE {
    unsafe { GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default() }
}

fn create_cover(pid: u32, label: &Label) -> Option<Cover> {
    unsafe {
        let h = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("ProgramPrisonCover"),
            w!("Blocked by Program Prison"),
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(hinstance()),
            None,
        )
        .ok()?;
        LABELS.with(|l| l.borrow_mut().insert(key(h), label.clone()));
        SetTimer(Some(h), TIMER_COUNTDOWN, 1000, None);
        Some(Cover { hwnd: h, pid })
    }
}

fn fmt_left(ms: i64) -> String {
    let s = (ms.max(0) + 999) / 1000;
    let (d, h, m, sec) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60, s % 60);
    if d > 0 {
        format!("{d} d {h} h left")
    } else if h > 0 {
        format!("{h} h {m:02} min left")
    } else {
        format!("{m}:{sec:02} left")
    }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16)
}

unsafe fn draw_text(hdc: HDC, text: &str, size: i32, weight: i32, color: COLORREF, rect: &mut RECT) {
    unsafe {
        let font = CreateFontW(-size, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI"));
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        SetTextColor(hdc, color);
        let mut t: Vec<u16> = text.encode_utf16().collect();
        DrawTextW(hdc, &mut t, rect, DT_CENTER | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
        SelectObject(hdc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
}

/// Brightness across a steel bar, 0..1 from one edge to the other: a cylinder lit from the
/// upper left, so each bar has a bright highlight and dark edges.
fn steel(t: f32) -> COLORREF {
    let shade = (0.5 + 0.5 * ((t - 0.32) * std::f32::consts::PI * 1.25).cos()).clamp(0.0, 1.0).powf(1.4);
    let mix = |lo: f32, hi: f32| (lo + (hi - lo) * shade) as u8;
    rgb(mix(58.0, 228.0), mix(61.0, 232.0), mix(65.0, 236.0))
}

/// Fills a rectangle with one color, without creating a brush each time.
unsafe fn fill(hdc: HDC, l: i32, t: i32, r: i32, b: i32, c: COLORREF) {
    unsafe {
        SetDCBrushColor(hdc, c);
        FillRect(hdc, &RECT { left: l, top: t, right: r, bottom: b }, HBRUSH(GetStockObject(DC_BRUSH).0));
    }
}

/// The cell: a dark wall with steel bars running top to bottom, each casting a shadow.
unsafe fn paint_bars(hdc: HDC, w: i32, h: i32, px: impl Fn(i32) -> i32) {
    unsafe {
        // the wall: a little lighter just above the middle
        let wall = |y: i32| -> (u8, u8, u8) {
            let t = (1.0 - ((y as f32 / h.max(1) as f32) - 0.45).abs() * 1.2).max(0.0);
            let c = 0x1a as f32 + 16.0 * t;
            (c as u8, (c - 2.0) as u8, (c - 5.0) as u8)
        };
        let step = 4;
        let mut y = 0;
        while y < h {
            let (r, g, b) = wall(y);
            fill(hdc, 0, y, w, y + step, rgb(r, g, b));
            y += step;
        }
        let bar_w = px(18);
        let gap = px(74);
        let (top, bottom) = (0, h);
        let shadow_w = px(10);
        let darker = |(r, g, b): (u8, u8, u8), a: f32| rgb((r as f32 * (1.0 - a)) as u8, (g as f32 * (1.0 - a)) as u8, (b as f32 * (1.0 - a)) as u8);
        // Painted back to front: shadows on the wall, then the bars.
        let bars: Vec<i32> = (0..).map(|i| px(40) + i * gap).take_while(|&x| x < w).collect();
        for s in 0..shadow_w {
            let a = 0.4 * (1.0 - s as f32 / shadow_w as f32);
            let c = darker(wall(h / 2), a);
            for &x in &bars {
                fill(hdc, x + bar_w + s, top, x + bar_w + s + 1, bottom, c);
            }
        }
        for i in 0..bar_w {
            let c = steel(i as f32 / (bar_w - 1).max(1) as f32);
            for &x in &bars {
                fill(hdc, x + i, top, x + i + 1, bottom, c);
            }
        }
    }
}

unsafe extern "system" fn cover_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let screen = BeginPaint(h, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(h, &mut rc);
                let (w, ht) = (rc.right - rc.left, rc.bottom - rc.top);
                // Draw offscreen, then copy in one go, so the countdown never flickers.
                let hdc = CreateCompatibleDC(Some(screen));
                let bmp = CreateCompatibleBitmap(screen, w.max(1), ht.max(1));
                let old_bmp = SelectObject(hdc, HGDIOBJ(bmp.0));
                let dpi = GetDpiForWindow(h).max(96) as i32;
                let px = |v: i32| (v * dpi / 96).max(1);
                paint_bars(hdc, w, ht, px);

                // The text sits on a dark plate so the bars never get in the way of reading it.
                let label = LABELS.with(|l| l.borrow().get(&key(h)).cloned()).unwrap_or_default();
                let small = ht < px(200);
                let mid = ht / 2;
                let (pw, ph) = ((w - px(24)).min(px(460)), if small { (ht - px(8)).min(px(70)) } else { px(170) });
                let plate = RECT { left: (w - pw) / 2, top: mid - ph / 2, right: (w + pw) / 2, bottom: mid + ph / 2 };
                let pen = CreatePen(PS_SOLID, px(2), rgb(0x6b, 0x4a, 0x35));
                let old_pen = SelectObject(hdc, HGDIOBJ(pen.0));
                let old_brush = SelectObject(hdc, GetStockObject(DC_BRUSH));
                SetDCBrushColor(hdc, rgb(0x1d, 0x1b, 0x18));
                let _ = RoundRect(hdc, plate.left, plate.top, plate.right, plate.bottom, px(28), px(28));
                SelectObject(hdc, old_brush);
                SelectObject(hdc, old_pen);
                let _ = DeleteObject(HGDIOBJ(pen.0));

                SetBkMode(hdc, TRANSPARENT);
                let (l, r) = (plate.left + px(12), plate.right - px(12));
                let mut r1 = RECT { left: l, top: mid - px(if small { 22 } else { 50 }), right: r, bottom: mid - px(if small { 2 } else { 16 }) };
                draw_text(hdc, &format!("{} is blocked", label.app), px(if small { 15 } else { 24 }), 700, rgb(0xef, 0xe9, 0xdf), &mut r1);
                if !small {
                    let mut r2 = RECT { left: l, top: mid - px(10), right: r, bottom: mid + px(14) };
                    draw_text(hdc, &format!("by “{}”", label.block), px(15), 400, rgb(0xb9, 0xb0, 0xa2), &mut r2);
                }
                let left = match label.until {
                    Some(t) => fmt_left(t - engine::now_ms()),
                    None => "Blocked on a schedule".into(),
                };
                let mut r3 = RECT { left: l, top: mid + px(if small { 2 } else { 24 }), right: r, bottom: mid + px(if small { 22 } else { 52 }) };
                draw_text(hdc, &left, px(16), 700, rgb(0xf0, 0xa3, 0x5e), &mut r3);

                let _ = BitBlt(screen, 0, 0, w, ht, Some(hdc), 0, 0, SRCCOPY);
                SelectObject(hdc, old_bmp);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(hdc);
                let _ = EndPaint(h, &ps);
                LRESULT(0)
            }
            WM_TIMER if wp.0 == TIMER_COUNTDOWN => {
                let _ = InvalidateRect(Some(h), None, false);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_CLOSE => LRESULT(0), // Alt+F4 on a cover does nothing
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

// ---------- the main thread ----------

struct Ui {
    svc: Arc<Mutex<Service>>,
    covers: Covers,
    hwnd: HWND,
    tray_tip: String,
    tray_icon: HICON,
    /// What the last enforcement pass decided, for programs that come to the front in between.
    close: Vec<String>,
    cover: Vec<String>,
    labels: HashMap<String, Label>,
}

impl Ui {
    fn enforce(&mut self, force: bool) {
        let (work, locked, tip) = {
            let mut s = self.svc.lock().unwrap();
            if s.quit_requested && !s.any_locked() {
                unsafe { PostQuitMessage(0) };
                return;
            }
            s.check_clock();
            let work = s.enforce(force);
            let now = engine::now_ms();
            let active: Vec<String> = s.state.blocks.iter().filter(|b| b.is_active(now)).map(|b| b.name.clone()).collect();
            let tip = if active.is_empty() { "Program Prison: no blocks on".to_string() } else { format!("Program Prison: {} on", active.join(", ")) };
            (work, s.any_locked(), tip)
        };
        let _ = locked;
        self.close = work.apps.close.clone();
        self.cover = work.apps.cover.clone();
        self.labels = work.labels.iter().map(|(app, block, until)| (app.clone(), Label { app: app.clone(), block: block.clone(), until: *until })).collect();
        let watching = !self.close.is_empty() || !self.cover.is_empty();
        self.covers.watch_foreground(watching);
        if watching {
            let list = list_processes();
            self.svc.lock().unwrap().close_programs(&list, &self.close);
            let me = std::process::id();
            let mut targets = HashMap::new();
            for p in &list {
                if p.pid != me && engine::app_matches(&p.names, &self.cover) {
                    targets.insert(p.pid, self.label_for(&p.names));
                }
            }
            self.covers.set_targets(targets);
        } else {
            self.covers.set_targets(HashMap::new());
        }
        self.covers.flush_pending();
        if tip != self.tray_tip {
            self.tray_tip = tip;
            self.update_tray(NIM_MODIFY);
        }
        let _ = force;
    }

    fn label_for(&self, names: &[String]) -> Label {
        for (app, l) in &self.labels {
            if engine::app_matches(names, std::slice::from_ref(app)) {
                let mut l = l.clone();
                l.app = names[0].trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
                return l;
            }
        }
        Label { app: names[0].clone(), ..Default::default() }
    }

    /// A program that wasn't running at the last check came to the front.
    fn on_foreground(&mut self, h: HWND) {
        let pid = window_pid(h);
        if pid == 0 || pid == std::process::id() || self.covers.targets.contains_key(&pid) {
            return;
        }
        let Some(name) = image_name(pid) else { return };
        let names = vec![name];
        if engine::app_matches(&names, &self.close) {
            if kill_process(pid) {
                util::debug(format!("closed {} on focus", names[0]));
                let mut s = self.svc.lock().unwrap();
                engine::record_closed(&mut s.state, engine::now_ms(), 1);
                s.save();
            }
        } else if engine::app_matches(&names, &self.cover) {
            let label = self.label_for(&names);
            self.covers.add_target(pid, label);
        }
    }

    fn drain_events(&mut self) {
        EVENTS_POSTED.with(|p| p.set(false));
        let events: Vec<(u32, isize)> = EVENTS.with(|q| q.borrow_mut().drain(..).collect());
        for (ev, h) in events {
            let h = hwnd_of(h);
            if ev == EVENT_SYSTEM_FOREGROUND && !self.covers.targets.contains_key(&window_pid(h)) {
                self.on_foreground(h);
            } else {
                self.covers.on_event(ev, h);
            }
        }
        self.covers.flush_pending();
    }

    fn update_tray(&self, op: NOTIFY_ICON_MESSAGE) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.tray_icon,
            ..Default::default()
        };
        for (i, c) in self.tray_tip.encode_utf16().take(127).enumerate() {
            nid.szTip[i] = c;
        }
        unsafe {
            let _ = Shell_NotifyIconW(op, &nid);
        }
    }

}

/// The tray icon's menu. Runs with nothing borrowed: while a menu is open Windows keeps
/// delivering timer and window messages, and they must be able to do their work.
fn show_tray_menu(hwnd: HWND) {
    let locked = UI.with(|u| u.try_borrow().ok().and_then(|g| g.as_ref().map(|ui| ui.svc.lock().unwrap().any_locked()))).unwrap_or(true);
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Open Program Prison"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let quit = if locked { w!("Quit (a locked block is on)") } else { w!("Quit") };
        let _ = AppendMenuW(menu, if locked { MF_STRING | MF_GRAYED } else { MF_STRING }, MENU_QUIT, quit);
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd); // so the menu closes when clicking elsewhere
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, Some(0), hwnd, None);
        let _ = DestroyMenu(menu);
        // Required after a tray menu, or it can stay stuck open (Microsoft KB 135788).
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        match cmd.0 as usize {
            MENU_OPEN => super::spawn_window(),
            MENU_QUIT if !locked => {
                with_ui(|ui| ui.svc.lock().unwrap().quit_requested = true);
                PostQuitMessage(0);
            }
            _ => {}
        }
    }
}

static RETRY_ENFORCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RETRY_EVENTS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Busy right now (re-entered from inside other work): try again in 50 ms rather than
/// re-posting straight away, which would spin.
fn retry_later(flag: &std::sync::atomic::AtomicBool) {
    flag.store(true, Ordering::SeqCst);
    let h = MAIN_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            SetTimer(Some(hwnd_of(h)), TIMER_RETRY, 50, None);
        }
    }
}

fn with_ui(f: impl FnOnce(&mut Ui)) -> bool {
    UI.with(|u| match u.try_borrow_mut() {
        Ok(mut g) => {
            if let Some(ui) = g.as_mut() {
                f(ui);
            }
            true
        }
        Err(_) => false, // re-entered while busy: try again from the message loop
    })
}

static TASKBAR_CREATED: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn main_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp.0 == TIMER_TICK => {
                if !with_ui(|ui| ui.enforce(false)) {
                    retry_later(&RETRY_ENFORCE);
                }
                LRESULT(0)
            }
            WM_TIMER if wp.0 == TIMER_RETRY => {
                let _ = KillTimer(Some(h), TIMER_RETRY);
                if RETRY_ENFORCE.swap(false, Ordering::SeqCst) && !with_ui(|ui| ui.enforce(true)) {
                    retry_later(&RETRY_ENFORCE);
                }
                if RETRY_EVENTS.swap(false, Ordering::SeqCst) && !with_ui(|ui| ui.drain_events()) {
                    retry_later(&RETRY_EVENTS);
                }
                LRESULT(0)
            }
            WM_APP_ENFORCE => {
                if !with_ui(|ui| ui.enforce(true)) {
                    retry_later(&RETRY_ENFORCE);
                }
                LRESULT(0)
            }
            WM_APP_EVENTS => {
                if !with_ui(|ui| ui.drain_events()) {
                    retry_later(&RETRY_EVENTS);
                }
                LRESULT(0)
            }
            WM_APP_SNAPSHOT => {
                with_ui(|ui| *COVER_SNAP.lock().unwrap() = Some(ui.covers.snapshot()));
                LRESULT(0)
            }
            WM_APP_TRAY => {
                match (lp.0 as u32) & 0xffff {
                    WM_LBUTTONUP => super::spawn_window(),
                    WM_RBUTTONUP | WM_CONTEXTMENU => show_tray_menu(h),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_POWERBROADCAST => {
                const PBT_APMSUSPEND: usize = 0x4;
                const PBT_APMRESUMEAUTOMATIC: usize = 0x12;
                with_ui(|ui| {
                    let mut s = ui.svc.lock().unwrap();
                    if wp.0 == PBT_APMSUSPEND {
                        s.asleep = true;
                    } else if wp.0 == PBT_APMRESUMEAUTOMATIC {
                        s.resumed();
                    }
                });
                LRESULT(1)
            }
            WM_QUERYENDSESSION => LRESULT(1), // never block shutdown or logoff
            WM_ENDSESSION => {
                with_ui(|ui| ui.svc.lock().unwrap().save());
                LRESULT(0)
            }
            m if m as isize == TASKBAR_CREATED.load(Ordering::SeqCst) && m != 0 => {
                with_ui(|ui| ui.update_tray(NIM_ADD)); // Explorer restarted: put the icon back
                LRESULT(0)
            }
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }
}

fn app_icon() -> HICON {
    unsafe {
        // Icon resource 1 is embedded by build.rs; fall back to the stock icon.
        LoadIconW(Some(hinstance()), PCWSTR(1 as _)).or_else(|_| LoadIconW(None, IDI_APPLICATION)).unwrap_or_default()
    }
}

pub fn run_service() {
    if service::already_running() {
        return;
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let inst = hinstance();
        let icon = app_icon();
        let wc = WNDCLASSW { lpfnWndProc: Some(main_proc), hInstance: inst, lpszClassName: w!("ProgramPrisonMain"), hIcon: icon, ..Default::default() };
        RegisterClassW(&wc);
        let cc = WNDCLASSW {
            lpfnWndProc: Some(cover_proc),
            hInstance: inst,
            lpszClassName: w!("ProgramPrisonCover"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: icon,
            ..Default::default()
        };
        RegisterClassW(&cc);
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")) as isize, Ordering::SeqCst);
        // A hidden top-level window (not message-only, so it gets power and taskbar broadcasts).
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("ProgramPrisonMain"), w!("Program Prison"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, Some(inst), None) else {
            util::debug("could not create the main window");
            return;
        };
        MAIN_HWND.store(key(hwnd), Ordering::SeqCst);

        let svc = Arc::new(Mutex::new(Service::new(true)));
        let s2 = svc.clone();
        if let Err(e) = ipc::serve(move |req| {
            if let Request::Covers = req {
                return covers_snapshot();
            }
            let (resp, enforce) = service::dispatch(&s2, req);
            if enforce {
                post(WM_APP_ENFORCE);
            }
            resp
        }) {
            util::log(format!("blocker could not start: {e}"));
            return;
        }
        crate::autostart::apply(svc.lock().unwrap().state.settings.start_at_login);

        let ui = Ui {
            svc: svc.clone(),
            covers: Covers::default(),
            hwnd,
            tray_tip: "Program Prison".into(),
            tray_icon: icon,
            close: vec![],
            cover: vec![],
            labels: HashMap::new(),
        };
        ui.update_tray(NIM_ADD);
        UI.with(|u| *u.borrow_mut() = Some(ui));
        let tick = std::env::var("PROGRAMPRISON_TICK_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000u32);
        SetTimer(Some(hwnd), TIMER_TICK, tick, None);
        post(WM_APP_ENFORCE);
        util::log("blocker started");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        UI.with(|u| {
            if let Some(mut ui) = u.borrow_mut().take() {
                ui.covers.clear();
                ui.update_tray(NIM_DELETE);
                ui.svc.lock().unwrap().save();
            }
        });
        let _ = std::fs::remove_file(ipc::endpoint_path());
    }
}

/// Covers live on the main thread; ask it for their positions (tests only).
fn covers_snapshot() -> Response {
    let h = MAIN_HWND.load(Ordering::SeqCst);
    if h == 0 {
        return Response::err("not ready");
    }
    unsafe {
        // SendMessage runs the handler on the main thread and waits for it.
        let _ = SendMessageTimeoutW(hwnd_of(h), WM_APP_SNAPSHOT, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 3000, None);
    }
    let covers = COVER_SNAP.lock().unwrap().take();
    match covers {
        Some(c) => Response { ok: true, covers: Some(c), ..Default::default() },
        None => Response::err("main thread busy"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws the cover's bars offscreen and checks real pixels: bright steel, a dark wall, and
    /// shadows darker than the wall.
    #[test]
    fn cover_bars_stand_out() {
        unsafe {
            let screen = GetDC(None);
            let hdc = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, 600, 400);
            let old = SelectObject(hdc, HGDIOBJ(bmp.0));
            paint_bars(hdc, 600, 400, |v| v); // 96 dpi
            let lum = |x: i32, y: i32| {
                let c = GetPixel(hdc, x, y).0;
                ((c & 0xff) + ((c >> 8) & 0xff) + ((c >> 16) & 0xff)) / 3
            };
            let highlight = lum(40 + 6, 200); // a third of the way across the first bar
            let wall = lum(40 + 18 + 30, 200); // between two bars
            let shadow = lum(40 + 18 + 1, 200); // just right of a bar
            let top = lum(40 + 6, 1); // bars run the full height: bright at the very top...
            let bottom = lum(40 + 6, 398); // ...and the very bottom
            let no_crossbar = lum(40 + 18 + 30, 10); // no crossbar near the top: just wall
            SelectObject(hdc, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(hdc);
            ReleaseDC(None, screen);
            assert!(highlight > 180, "bar highlight {highlight}");
            assert!(wall < 60, "wall {wall}");
            assert!(highlight - wall > 140, "contrast {highlight} vs {wall}");
            assert!(shadow < wall, "shadow {shadow} darker than wall {wall}");
            assert!(top > 180 && bottom > 180, "bars reach top {top} and bottom {bottom}");
            assert!(no_crossbar < 60, "no crossbar: {no_crossbar}");
        }
    }
}
