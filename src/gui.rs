//! The settings window (egui). It only runs while open: closing it leaves the background
//! blocker running. Every change goes through the blocker, which enforces the lock rules.

use crate::engine::{self, App, AppMode, Edit, LockRequest, PublicBlock, PRESETS, SLOTS};
use crate::ipc::{Challenge, Client, PublicState, Request, Response};
use eframe::egui::{self, Color32, CornerRadius, Event, Id, Modal, RichText, Sense, Stroke, Vec2};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Sends a request to the blocker. A function so tests can plug in their own.
pub type Conn = Arc<dyn Fn(&Request) -> Result<Response, String> + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Sites,
    Apps,
    Schedule,
    Lock,
}

struct StartDialog {
    hours: u32,
    minutes: u32,
    until_mode: bool,
    until_text: String,
    error: String,
}

struct StopDialog {
    challenge: Challenge,
    answer: String,
    error: String,
}

struct LockForm {
    for_key: String,
    kind: String,
    length: u32,
    password: String,
}

pub struct PrisonApp {
    conn: Conn,
    pub state: Option<PublicState>,
    last_poll: Option<Instant>,
    pub selected: Option<String>,
    pub tab: Tab,
    site_input: String,
    app_input: String,
    name_input: (String, String), // (block id, text)
    running: Vec<String>,
    running_loaded: Option<Instant>,
    start: Option<StartDialog>,
    stop: Option<StopDialog>,
    settings_open: bool,
    confirm_delete: bool,
    confirm_locked: bool,
    lock_form: LockForm,
    pub toast: Option<(String, bool, Instant)>,
    paint: Option<(bool, Vec<bool>)>,
    setup: Option<mpsc::Receiver<Response>>,
    pub offline: Option<String>,
    shown_requests: Option<u32>,
}

// ---------- colors ----------

const ACCENT: Color32 = Color32::from_rgb(0xc9, 0x54, 0x1f);
const ON: Color32 = Color32::from_rgb(0x2f, 0x7d, 0x4f);
const DANGER: Color32 = Color32::from_rgb(0xb3, 0x26, 0x1e);

fn muted(ui: &egui::Ui) -> Color32 {
    ui.visuals().weak_text_color()
}

fn soft(ui: &egui::Ui, c: Color32) -> Color32 {
    if ui.visuals().dark_mode { c.gamma_multiply(0.25) } else { c.gamma_multiply(0.14) }
}

// ---------- formatting ----------

fn fmt_left(ms: i64) -> String {
    let s = (ms.max(0) + 999) / 1000;
    let (d, h, m, sec) = (s / 86400, (s % 86400) / 3600, (s % 3600) / 60, s % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

fn fmt_time(t: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(d) = Local.timestamp_millis_opt(t).single() else { return String::new() };
    if d.date_naive() == Local::now().date_naive() { d.format("%H:%M").to_string() } else { d.format("%a %d %b %H:%M").to_string() }
}

/// (color, text) for a block's status line.
fn status_text(b: &PublicBlock, now: i64) -> (Color32, String) {
    let s = &b.status;
    if s.active {
        let left = if s.forever { "always on".to_string() } else { format!("{} left", fmt_left(s.until.unwrap_or(now) - now)) };
        if s.locked { (ACCENT, format!("Locked · {left}")) } else { (ON, format!("On · {left}")) }
    } else if let Some(u) = s.unlocked_until {
        (Color32::GRAY, format!("Off · skipping schedule until {}", fmt_time(u)))
    } else if s.scheduled {
        (Color32::GRAY, "Off · scheduled".into())
    } else {
        (Color32::GRAY, "Off".into())
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

impl PrisonApp {
    pub fn new(conn: Conn) -> PrisonApp {
        let mut app = PrisonApp {
            conn,
            state: None,
            last_poll: None,
            selected: None,
            tab: Tab::Sites,
            site_input: String::new(),
            app_input: String::new(),
            name_input: (String::new(), String::new()),
            running: vec![],
            running_loaded: None,
            start: None,
            stop: None,
            settings_open: false,
            confirm_delete: false,
            confirm_locked: false,
            lock_form: LockForm { for_key: String::new(), kind: "none".into(), length: 200, password: String::new() },
            toast: None,
            paint: None,
            setup: None,
            offline: None,
            shown_requests: None,
        };
        app.poll();
        app
    }

    fn send(&mut self, req: Request) -> Response {
        match (self.conn)(&req) {
            Ok(r) => {
                self.offline = None;
                if let Some(s) = &r.state {
                    self.set_state(s.clone());
                }
                if let Some(e) = &r.error {
                    self.toast(e.clone(), true);
                }
                r
            }
            Err(e) => {
                self.offline = Some(e.clone());
                Response::err(e)
            }
        }
    }

    fn poll(&mut self) {
        self.last_poll = Some(Instant::now());
        match (self.conn)(&Request::GetState) {
            Ok(r) => {
                self.offline = None;
                if let Some(s) = r.state {
                    self.set_state(s);
                }
            }
            Err(e) => self.offline = Some(e),
        }
    }

    fn set_state(&mut self, s: PublicState) {
        if !s.blocks.iter().any(|b| Some(&b.id) == self.selected.as_ref()) {
            self.selected = s.blocks.first().map(|b| b.id.clone());
        }
        self.state = Some(s);
    }

    fn toast(&mut self, msg: impl Into<String>, bad: bool) {
        self.toast = Some((msg.into(), bad, Instant::now()));
    }

    pub fn block(&self) -> Option<&PublicBlock> {
        let s = self.state.as_ref()?;
        s.blocks.iter().find(|b| Some(&b.id) == self.selected.as_ref())
    }

    fn update_block(&mut self, edit: Edit) -> Response {
        let Some(id) = self.selected.clone() else { return Response::err("no block") };
        self.send(Request::UpdateBlock { id, edit })
    }

    // ---------- actions ----------

    pub fn new_block(&mut self) {
        let r = self.send(Request::CreateBlock { name: String::new() });
        if let Some(id) = r.id {
            self.selected = Some(id);
            self.tab = Tab::Sites;
        }
    }

    pub fn add_sites(&mut self, text: &str) {
        let Some(b) = self.block() else { return };
        let raw: Vec<String> = text.split([' ', ',', '\n', '\t']).filter(|s| !s.is_empty()).map(String::from).collect();
        if raw.is_empty() {
            return;
        }
        let mut sites = b.sites.clone();
        sites.extend(raw.iter().cloned());
        let r = self.update_block(Edit { sites: Some(sites), ..Default::default() });
        if r.ok {
            let bad: Vec<String> = raw.into_iter().filter(|x| engine::normalize_domain(x).is_none()).collect();
            if !bad.is_empty() {
                self.toast(format!("Not a website address: {}", bad.join(", ")), true);
            }
            self.site_input = bad.join(" ");
        }
    }

    pub fn add_app(&mut self, name: &str) {
        let Some(b) = self.block() else { return };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if let Err(e) = engine::normalize_app(&name) {
            self.toast(e, true);
            return;
        }
        let mut apps = b.apps.clone();
        apps.push(App { name, mode: AppMode::Close });
        if self.update_block(Edit { apps: Some(apps), ..Default::default() }).ok {
            self.app_input.clear();
        }
    }

    fn load_running(&mut self) {
        if self.running_loaded.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return;
        }
        self.running_loaded = Some(Instant::now());
        if let Ok(Response { apps: Some(a), .. }) = (self.conn)(&Request::RunningApps) {
            self.running = a;
        }
    }

    fn open_stop(&mut self) {
        let Some(id) = self.selected.clone() else { return };
        if let Some(c) = self.send(Request::Challenge { id }).challenge {
            let error = self.stop.take().map(|s| s.error).unwrap_or_default();
            self.stop = Some(StopDialog { challenge: c, answer: String::new(), error });
        }
    }

    fn submit_stop(&mut self) {
        let (Some(id), Some(d)) = (self.selected.clone(), self.stop.as_ref()) else { return };
        let answer = d.answer.clone();
        let random = matches!(d.challenge, Challenge::Random { .. });
        let r = (self.conn)(&Request::StopBlock { id, answer }).unwrap_or_else(Response::err);
        if let Some(s) = r.state.clone() {
            self.set_state(s);
        }
        if r.ok {
            self.stop = None;
            self.toast("Block stopped.", false);
        } else {
            let err = r.error.unwrap_or_default();
            if let Some(d) = self.stop.as_mut() {
                d.error = err;
                d.answer.clear();
            }
            if random {
                self.open_stop(); // a new text after every wrong try
            }
        }
    }

    fn submit_start(&mut self) {
        let (Some(id), Some(d)) = (self.selected.clone(), self.start.as_ref()) else { return };
        let ends = match start_end(d) {
            Ok(t) => t,
            Err(e) => {
                self.start.as_mut().unwrap().error = e;
                return;
            }
        };
        let r = (self.conn)(&Request::StartBlock { id, ends_at: ends }).unwrap_or_else(Response::err);
        if let Some(s) = r.state.clone() {
            self.set_state(s);
        }
        if r.ok {
            self.start = None;
            self.toast("Block started.", false);
        } else if let Some(d) = self.start.as_mut() {
            d.error = r.error.unwrap_or_default();
        }
    }

    fn save_lock(&mut self) {
        let Some(id) = self.selected.clone() else { return };
        let f = &self.lock_form;
        let req = LockRequest { kind: f.kind.clone(), length: f.length, password: f.password.clone() };
        if self.send(Request::SetLock { id, lock: req }).ok {
            self.lock_form.password.clear();
            self.lock_form.for_key.clear();
            self.toast("Lock saved.", false);
        }
    }

    fn start_setup(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let conn = self.conn.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = conn(&Request::SetupHosts).unwrap_or_else(Response::err);
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.setup = Some(rx);
    }
}

fn start_end(d: &StartDialog) -> Result<i64, String> {
    if d.until_mode {
        use chrono::{Local, NaiveDateTime, TimeZone};
        let t = NaiveDateTime::parse_from_str(d.until_text.trim(), "%Y-%m-%d %H:%M").map_err(|_| "Write the end time as YYYY-MM-DD HH:MM.".to_string())?;
        Local.from_local_datetime(&t).earliest().map(|x| x.timestamp_millis()).ok_or_else(|| "That time doesn't exist here.".to_string())
    } else {
        Ok(engine::now_ms() + (d.hours as i64 * 60 + d.minutes as i64) * 60_000)
    }
}

// ---------- drawing ----------

impl eframe::App for PrisonApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl PrisonApp {
    /// The whole window. Public so tests can drive it.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.last_poll.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) && self.paint.is_none() {
            self.poll();
        }
        if let Some(rx) = &self.setup
            && let Ok(r) = rx.try_recv() {
                self.setup = None;
                if let Some(s) = r.state.clone() {
                    self.set_state(s);
                }
                if r.ok {
                    self.toast("Website blocking is ready.", false)
                } else {
                    self.toast(r.error.unwrap_or_default(), true)
                }
            }
        // Someone opened ProgramPrison again (or clicked the tray icon): come to the front.
        if let Some(n) = self.state.as_ref().map(|s| s.show_requests) {
            if self.shown_requests.is_some_and(|seen| n > seen) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            self.shown_requests = Some(n);
        }
        // Counting down needs a redraw once a second; nothing more while idle.
        ctx.request_repaint_after(Duration::from_secs(1));

        egui::Panel::left("sidebar").exact_size(232.0).resizable(false).show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.add_space(6.0);
                self.main(ui);
            });
        });
        self.dialogs(&ctx);
        self.draw_toast(&ctx);
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
            paint_logo(ui.painter(), r);
            ui.label(RichText::new("ProgramPrison").strong().size(17.0));
        });
        ui.add_space(14.0);
        ui.label(RichText::new("BLOCKS").small().color(muted(ui)));
        ui.add_space(4.0);
        let now = engine::now_ms();
        let blocks = self.state.as_ref().map(|s| s.blocks.clone()).unwrap_or_default();
        for b in &blocks {
            let selected = Some(&b.id) == self.selected.as_ref();
            let (color, text) = status_text(b, now);
            let resp = egui::Frame::new()
                .fill(if selected { ui.visuals().extreme_bg_color } else { Color32::TRANSPARENT })
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 7))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(9.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.5, color);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&b.name).strong());
                            ui.label(RichText::new(text).small().color(muted(ui)));
                        });
                    });
                })
                .response
                .interact(Sense::click());
            if resp.clicked() {
                self.selected = Some(b.id.clone());
            }
        }
        ui.add_space(6.0);
        if ui.add(egui::Button::new("+ New block").frame(false)).clicked() {
            self.new_block();
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(12.0);
            if ui.add(egui::Button::new("Settings").frame(false)).clicked() {
                self.settings_open = true;
            }
            if let Some(s) = &self.state
                && s.closed_today > 0 {
                    ui.label(RichText::new(format!("{} closed today", plural(s.closed_today as usize, "program"))).small().color(muted(ui)));
                }
        });
    }

    fn main(&mut self, ui: &mut egui::Ui) {
        if let Some(e) = &self.offline {
            banner(ui, DANGER, &format!("Can't reach the ProgramPrison blocker ({e}). Restart ProgramPrison."));
        }
        let Some(state) = self.state.clone() else { return };
        let needs_sites = state.blocks.iter().any(|b| !b.sites.is_empty());
        if state.hosts.needs_setup || (!state.hosts.writable && needs_sites) {
            let mut clicked = false;
            egui::Frame::new().fill(soft(ui, ACCENT)).stroke(Stroke::new(1.0, ACCENT)).corner_radius(CornerRadius::same(10)).inner_margin(12).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Website blocking needs a one-time permission to edit the system hosts file.");
                    if self.setup.is_some() {
                        ui.spinner();
                        ui.label("Waiting for permission…");
                    } else {
                        clicked = ui.add(primary("Set up website blocking")).clicked();
                    }
                });
            });
            if clicked {
                self.start_setup(ui.ctx());
            }
            ui.add_space(12.0);
        }
        let Some(b) = self.block().cloned() else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.18);
                let (r, _) = ui.allocate_exact_size(Vec2::splat(84.0), Sense::hover());
                paint_logo(ui.painter(), r);
                ui.add_space(12.0);
                ui.label(RichText::new("Lock away distractions").size(22.0).strong());
                ui.add_space(4.0);
                ui.label(RichText::new("A block is a list of websites and programs to keep out of reach.\nRun it on a timer or a weekly schedule, and lock it so you can't back out.").color(muted(ui)));
                ui.add_space(16.0);
                if ui.add(primary("Create your first block")).clicked() {
                    self.new_block();
                }
            });
            return;
        };
        let now = engine::now_ms();
        let locked = b.status.locked;

        // header
        ui.horizontal(|ui| {
            if self.name_input.0 != b.id {
                self.name_input = (b.id.clone(), b.name.clone());
            }
            let w = (self.name_input.1.chars().count() as f32 * 12.5 + 24.0).clamp(120.0, 360.0);
            let r = ui.add(egui::TextEdit::singleline(&mut self.name_input.1).font(egui::TextStyle::Heading).desired_width(w).frame(egui::Frame::NONE).id_salt("name"));
            named(&r, "Block name");
            if r.lost_focus() && self.name_input.1.trim() != b.name {
                let name = self.name_input.1.clone();
                self.update_block(Edit { name: Some(name), ..Default::default() });
            }
            let (color, text) = status_text(&b, now);
            egui::Frame::new().fill(soft(ui, color)).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(9, 3)).show(ui, |ui| {
                ui.label(RichText::new(text).small().strong().color(color));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!locked, egui::Button::new(RichText::new("Delete").color(DANGER)).frame(false)).clicked() {
                    self.confirm_delete = true;
                }
                if b.status.active && ui.add_enabled(b.lock != "locked" || !locked, egui::Button::new("Stop")).clicked() {
                    self.stop = None;
                    self.open_stop();
                }
                if ui.add(primary(if b.status.active { "Extend" } else { "Start" })).clicked() {
                    self.start = Some(StartDialog { hours: 1, minutes: 0, until_mode: false, until_text: fmt_input(now + engine::HOUR), error: String::new() });
                }
            });
        });
        if locked {
            ui.add_space(8.0);
            let how = match b.lock.as_str() {
                "locked" => "Nothing can be removed or stopped until it ends.",
                "password" => "Removing anything or stopping needs the password.",
                _ => "Removing anything or stopping needs the unlock text.",
            };
            banner(ui, ACCENT, &format!("Locked. You can add websites, programs and hours. {how}"));
        }

        // tabs
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let hours = b.schedule.iter().filter(|&&x| x).count();
            for (tab, label) in [
                (Tab::Sites, format!("Websites {}", count(b.sites.len()))),
                (Tab::Apps, format!("Programs {}", count(b.apps.len()))),
                (Tab::Schedule, format!("Schedule {}", if hours > 0 { format!("{hours} h") } else { String::new() })),
                (Tab::Lock, format!("Lock {}", if b.lock == "none" { "" } else { &b.lock })),
            ] {
                let sel = self.tab == tab;
                let color = if sel { ui.visuals().strong_text_color() } else { muted(ui) };
                let r = ui.add(egui::Button::new(RichText::new(label.trim()).strong().color(color)).frame(false));
                if sel {
                    ui.painter().hline(r.rect.x_range(), r.rect.bottom() + 3.0, Stroke::new(2.0, ACCENT));
                }
                if r.clicked() {
                    self.tab = tab;
                    if tab == Tab::Apps {
                        self.load_running();
                    }
                }
            }
        });
        ui.separator();
        ui.add_space(8.0);
        ui.set_max_width(780.0);
        match self.tab {
            Tab::Sites => self.sites_tab(ui, &b),
            Tab::Apps => self.apps_tab(ui, &b, state.cover_supported),
            Tab::Schedule => self.schedule_tab(ui, &b),
            Tab::Lock => self.lock_tab(ui, &b),
        }
    }

    fn sites_tab(&mut self, ui: &mut egui::Ui, b: &PublicBlock) {
        let mut submit = false;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.site_input).hint_text("example.com, or paste a link").desired_width(ui.available_width() - 70.0).id_salt("site"));
            named(&r, "Website to block");
            submit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            submit |= ui.button("Add").clicked();
        });
        if submit {
            let t = self.site_input.clone();
            self.add_sites(&t);
        }
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Quick add").small().color(muted(ui)));
            for p in PRESETS {
                let missing: Vec<String> = p.sites.iter().filter(|s| !b.sites.iter().any(|x| x == *s)).map(|s| s.to_string()).collect();
                let label = if missing.is_empty() { format!("{} added", p.name) } else { format!("+ {}", p.name) };
                let btn = egui::Button::new(RichText::new(label).small()).corner_radius(CornerRadius::same(12));
                if ui.add_enabled(!missing.is_empty(), btn).on_hover_text(p.sites.join(", ")).clicked() {
                    let mut sites = b.sites.clone();
                    sites.extend(missing);
                    self.update_block(Edit { sites: Some(sites), ..Default::default() });
                }
            }
        });
        ui.add_space(8.0);
        let mut remove = None;
        list_frame(ui, |ui| {
            if b.sites.is_empty() {
                empty_row(ui, "No websites yet.");
            }
            for (i, s) in b.sites.iter().enumerate() {
                row(ui, i, |ui| {
                    ui.label(s);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!b.status.locked, egui::Button::new("×").frame(false)).on_hover_text(format!("Remove {s}")).clicked() {
                            remove = Some(s.clone());
                        }
                    });
                });
            }
        });
        if let Some(s) = remove {
            self.update_block(Edit { sites: Some(b.sites.iter().filter(|x| **x != s).cloned().collect()), ..Default::default() });
        }
        hint(ui, "Blocking example.com also blocks www., m. and mobile. versions. Other subdomains need adding separately. Works in every browser.");
    }

    fn apps_tab(&mut self, ui: &mut egui::Ui, b: &PublicBlock, cover_supported: bool) {
        let mut submit = false;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.app_input).hint_text("Program name, e.g. steam.exe or Discord").desired_width(ui.available_width() - 70.0).id_salt("app"));
            named(&r, "Program to block");
            submit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            submit |= ui.button("Add").clicked();
        });
        if submit {
            let t = self.app_input.clone();
            self.add_app(&t);
        }
        ui.horizontal(|ui| {
            let mut picked = None;
            egui::ComboBox::from_id_salt("running").selected_text("Or pick a program that's running now…").width(ui.available_width() - 80.0).show_ui(ui, |ui| {
                for n in &self.running {
                    if ui.selectable_label(false, n).clicked() {
                        picked = Some(n.clone());
                    }
                }
            });
            if ui.button("Refresh").clicked() {
                self.running_loaded = None;
                self.load_running();
            }
            if let Some(n) = picked {
                self.add_app(&n);
            }
        });
        ui.add_space(8.0);
        let mut new_apps: Option<Vec<App>> = None;
        list_frame(ui, |ui| {
            if b.apps.is_empty() {
                empty_row(ui, "No programs yet.");
            }
            for (i, a) in b.apps.iter().enumerate() {
                row(ui, i, |ui| {
                    ui.label(&a.name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!b.status.locked, egui::Button::new("×").frame(false)).on_hover_text(format!("Remove {}", a.name)).clicked() {
                            new_apps = Some(b.apps.iter().filter(|x| x.name != a.name).cloned().collect());
                        }
                        let mut mode = a.mode;
                        let cover_label = if cover_supported { "Cover its windows" } else { "Cover (Windows only)" };
                        let shown = if mode == AppMode::Close { "Close it" } else { cover_label };
                        egui::ComboBox::from_id_salt(("mode", &a.name)).selected_text(shown).width(170.0).show_ui(ui, |ui| {
                            ui.selectable_value(&mut mode, AppMode::Close, "Close it");
                            let can_cover = cover_supported && !(b.status.locked && a.mode == AppMode::Close);
                            ui.add_enabled_ui(can_cover, |ui| {
                                ui.selectable_value(&mut mode, AppMode::Cover, cover_label);
                            });
                        });
                        if mode != a.mode {
                            new_apps = Some(b.apps.iter().map(|x| if x.name == a.name { App { name: x.name.clone(), mode } } else { x.clone() }).collect());
                        }
                    });
                });
            }
        });
        if let Some(apps) = new_apps {
            self.update_block(Edit { apps: Some(apps), ..Default::default() });
        }
        hint(
            ui,
            if cover_supported {
                "Close it: the program is closed whenever it starts. Cover its windows: an overlay sits exactly on top of each of its windows and follows them; the program keeps running underneath."
            } else {
                "While the block is on, these programs are closed as soon as they start."
            },
        );
    }

    fn schedule_tab(&mut self, ui: &mut egui::Ui, b: &PublicBlock) {
        hint(ui, "Click or drag across the hours this block should switch on by itself, every week.");
        ui.add_space(8.0);
        const DAYS: [(&str, usize); 7] = [("Mon", 1), ("Tue", 2), ("Wed", 3), ("Thu", 4), ("Fri", 5), ("Sat", 6), ("Sun", 0)];
        let label_w = 40.0;
        let gap = 3.0;
        let width = ui.available_width().min(780.0);
        let cell_w = ((width - label_w) / 24.0 - gap).max(8.0);
        let cell_h = 24.0;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 18.0 + 7.0 * (cell_h + gap)), Sense::click_and_drag());
        let p = ui.painter_at(rect);
        let font = egui::FontId::proportional(11.0);
        for h in (0..24).step_by(3) {
            let x = rect.left() + label_w + h as f32 * (cell_w + gap);
            let t = match h {
                0 => "12a".to_string(),
                12 => "12p".to_string(),
                h if h < 12 => format!("{h}a"),
                h => format!("{}p", h - 12),
            };
            p.text(egui::pos2(x, rect.top()), egui::Align2::LEFT_TOP, t, font.clone(), muted(ui));
        }
        let cell_at = |pos: egui::Pos2| -> Option<usize> {
            let col = ((pos.x - rect.left() - label_w) / (cell_w + gap)).floor();
            let rowi = ((pos.y - rect.top() - 18.0) / (cell_h + gap)).floor();
            if (0.0..24.0).contains(&col) && (0.0..7.0).contains(&rowi) { Some(DAYS[rowi as usize].1 * 24 + col as usize) } else { None }
        };
        // painting with the mouse: the first cell decides whether this stroke adds or removes
        if self.paint.is_none() && (resp.drag_started() || resp.clicked() || resp.is_pointer_button_down_on())
            && let Some(slot) = resp.interact_pointer_pos().and_then(cell_at) {
                let value = !b.schedule[slot];
                if !value && b.status.locked {
                    if resp.clicked() || resp.drag_started() {
                        self.toast("The block is locked: hours can be added but not removed.", true);
                    }
                } else {
                    self.paint = Some((value, b.schedule.clone()));
                }
            }
        if let (Some((value, sched)), Some(pos)) = (self.paint.as_mut(), resp.interact_pointer_pos())
            && let Some(slot) = cell_at(pos) {
                sched[slot] = *value;
            }
        let released = self.paint.is_some() && !ui.input(|i| i.pointer.any_down());
        let shown = self.paint.as_ref().map(|(_, s)| s.clone()).unwrap_or_else(|| b.schedule.clone());
        let cur = engine::slot_of(engine::now_ms());
        let empty_fill = if ui.visuals().dark_mode { Color32::from_gray(52) } else { Color32::from_rgb(0xe6, 0xe3, 0xdd) };
        for (ri, (label, day)) in DAYS.iter().enumerate() {
            let y = rect.top() + 18.0 + ri as f32 * (cell_h + gap);
            p.text(egui::pos2(rect.left(), y + cell_h / 2.0), egui::Align2::LEFT_CENTER, *label, egui::FontId::proportional(12.0), muted(ui));
            for h in 0..24 {
                let slot = day * 24 + h;
                let x = rect.left() + label_w + h as f32 * (cell_w + gap);
                let r = egui::Rect::from_min_size(egui::pos2(x, y), Vec2::new(cell_w, cell_h));
                p.rect_filled(r, CornerRadius::same(4), if shown[slot] { ACCENT } else { empty_fill });
                if slot == cur {
                    p.rect_stroke(r, CornerRadius::same(4), Stroke::new(2.0, ui.visuals().strong_text_color()), egui::StrokeKind::Inside);
                }
            }
        }
        if released {
            let (_, sched) = self.paint.take().unwrap();
            if sched != b.schedule {
                self.update_block(Edit { schedule: Some(sched), ..Default::default() });
            }
        }
        ui.add_space(10.0);
        let mut new_sched = None;
        ui.horizontal(|ui| {
            let add = |f: &dyn Fn(usize, usize) -> bool| -> Vec<bool> { (0..SLOTS).map(|i| b.schedule[i] || f(i / 24, i % 24)).collect() };
            if ui.button("Add weekdays 9–5").clicked() {
                new_sched = Some(add(&|d, h| (1..=5).contains(&d) && (9..17).contains(&h)));
            }
            if ui.button("Add evenings 9pm–7am").clicked() {
                new_sched = Some(add(&|_, h| !(7..21).contains(&h)));
            }
            if ui.add_enabled(!b.status.locked, egui::Button::new("Clear")).clicked() {
                new_sched = Some(vec![false; SLOTS]);
            }
        });
        if let Some(s) = new_sched {
            self.update_block(Edit { schedule: Some(s), ..Default::default() });
        }
    }

    fn lock_tab(&mut self, ui: &mut egui::Ui, b: &PublicBlock) {
        let key = format!("{}|{}|{}|{}", b.id, b.lock, b.random_length, b.status.locked);
        if self.lock_form.for_key != key {
            self.lock_form = LockForm { for_key: key, kind: b.lock.clone(), length: if b.random_length > 0 { b.random_length } else { 200 }, password: String::new() };
        }
        let locked = b.status.locked;
        hint(ui, "What it takes to stop this block early. It applies to timed blocks and scheduled hours alike.");
        ui.add_space(8.0);
        let options: [(&str, &str, &str); 4] = [
            ("none", "No lock", "Stop whenever you like."),
            ("random", "Random text", "To stop early, type out random characters by hand. Copy and paste are switched off."),
            ("password", "Password", "Ask someone you trust to type a password you don't know."),
            ("locked", "Locked", "No way out until time runs out. You can't stop it, quit ProgramPrison, delete the block or remove anything from it."),
        ];
        for (kind, title, desc) in options {
            let enabled = !locked || kind == "locked" || kind == b.lock;
            let sel = self.lock_form.kind == kind;
            ui.add_enabled_ui(enabled, |ui| {
                let stroke = if sel { Stroke::new(1.5, ACCENT) } else { ui.visuals().widgets.noninteractive.bg_stroke };
                let r = egui::Frame::new().stroke(stroke).corner_radius(CornerRadius::same(10)).inner_margin(12).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.lock_form.kind, kind.to_string(), "");
                        ui.vertical(|ui| {
                            ui.label(RichText::new(title).strong());
                            ui.label(RichText::new(desc).color(muted(ui)));
                            if kind == "random" && sel {
                                ui.horizontal(|ui| {
                                    ui.label("Characters:");
                                    ui.add(egui::DragValue::new(&mut self.lock_form.length).range(20..=2000).speed(10));
                                });
                            }
                            if kind == "password" && sel && !locked {
                                let hint_text = if b.lock == "password" { "Leave empty to keep the current password" } else { "New password" };
                                ui.add(egui::TextEdit::singleline(&mut self.lock_form.password).password(true).hint_text(hint_text).desired_width(280.0).id_salt("pw"));
                            }
                        });
                    });
                });
                if enabled && r.response.interact(Sense::click()).clicked() {
                    self.lock_form.kind = kind.to_string();
                }
            });
            ui.add_space(6.0);
        }
        ui.add_space(6.0);
        if ui.add(primary("Save lock")).clicked() {
            if self.lock_form.kind == "locked" && !locked {
                self.confirm_locked = true;
            } else {
                self.save_lock();
            }
        }
    }

    // ---------- dialogs ----------

    fn dialogs(&mut self, ctx: &egui::Context) {
        let name = self.block().map(|b| b.name.clone()).unwrap_or_default();

        if self.start.is_some() {
            let b = self.block().cloned();
            let mut close = false;
            let mut submit = false;
            Modal::new(Id::unique("start")).show(ctx, |ui| {
                ui.set_width(440.0);
                let d = self.start.as_mut().unwrap();
                ui.heading(format!("Start “{name}”"));
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, m) in [("15 min", 15), ("30 min", 30), ("1 hour", 60), ("2 hours", 120), ("4 hours", 240), ("8 hours", 480), ("1 day", 1440)] {
                        if ui.button(label).clicked() {
                            d.hours = m / 60;
                            d.minutes = m % 60;
                            d.until_mode = false;
                        }
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.until_mode, false, "For");
                    ui.add(egui::DragValue::new(&mut d.hours).range(0..=8784).suffix(" h"));
                    ui.add(egui::DragValue::new(&mut d.minutes).range(0..=59).suffix(" min"));
                });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut d.until_mode, true, "Until");
                    let r = ui.add(egui::TextEdit::singleline(&mut d.until_text).desired_width(160.0).id_salt("until"));
                    if r.changed() {
                        d.until_mode = true;
                    }
                    ui.label(RichText::new("YYYY-MM-DD HH:MM").small().color(muted(ui)));
                });
                ui.add_space(8.0);
                if let Some(b) = &b {
                    let what = match (b.sites.len(), b.apps.len()) {
                        (0, 0) => "nothing yet (add websites or programs first)".to_string(),
                        (s, 0) => plural(s, "website"),
                        (0, a) => plural(a, "program"),
                        (s, a) => format!("{} and {}", plural(s, "website"), plural(a, "program")),
                    };
                    let lock = match b.lock.as_str() {
                        "none" => "No lock: you can stop it any time.".to_string(),
                        "random" => format!("Stopping early means typing {} random characters.", b.random_length),
                        "password" => "Stopping early needs the password.".to_string(),
                        _ => "Locked: it can't be stopped early at all.".to_string(),
                    };
                    let summary = match start_end(d) {
                        Ok(t) if t > engine::now_ms() => format!("Blocks {what} until {} ({}). {lock}", fmt_time(t), fmt_left(t - engine::now_ms())),
                        _ => "Pick when the block should end.".into(),
                    };
                    egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(CornerRadius::same(8)).inner_margin(10).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(summary);
                    });
                }
                if !d.error.is_empty() {
                    ui.colored_label(DANGER, &d.error);
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    submit = ui.add(primary("Start block")).clicked();
                    close = ui.button("Cancel").clicked();
                });
            });
            if close {
                self.start = None;
            } else if submit {
                self.submit_start();
            }
        }

        if self.stop.is_some() {
            let mut close = false;
            let mut submit = false;
            // No pasting the unlock text.
            if matches!(self.stop.as_ref().unwrap().challenge, Challenge::Random { .. }) {
                ctx.input_mut(|i| i.events.retain(|e| !matches!(e, Event::Paste(_) | Event::Copy | Event::Cut)));
            }
            Modal::new(Id::unique("stop")).show(ctx, |ui| {
                ui.set_width(460.0);
                let d = self.stop.as_mut().unwrap();
                ui.heading(format!("Stop “{name}”?"));
                ui.add_space(8.0);
                let mut can_submit = true;
                match &d.challenge {
                    Challenge::None => {
                        ui.label("The websites and programs in this block will be allowed again.");
                    }
                    Challenge::Locked { until } => {
                        ui.label(match until {
                            Some(t) => format!("This block is locked until {}. It can't be stopped early.", fmt_time(*t)),
                            None => "This block is locked. It can't be stopped early.".into(),
                        });
                        can_submit = false;
                    }
                    Challenge::Password => {
                        ui.label("Enter the unlock password.");
                        let r = ui.add(egui::TextEdit::singleline(&mut d.answer).password(true).desired_width(f32::INFINITY).id_salt("answer"));
                        named(&r, "Unlock password");
                        submit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    }
                    Challenge::Random { text } => {
                        ui.label(format!("To stop early, type these {} characters exactly. Upper and lower case count.", text.len()));
                        let spaced: String = text.chars().collect::<Vec<_>>().chunks(10).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ");
                        egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(CornerRadius::same(8)).inner_margin(10).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                                ui.add(egui::Label::new(RichText::new(spaced).monospace().size(14.0)).selectable(false).wrap());
                            });
                        });
                        let r = ui.add(egui::TextEdit::multiline(&mut d.answer).font(egui::TextStyle::Monospace).desired_rows(4).desired_width(f32::INFINITY).hint_text("Type the text above exactly").id_salt("answer"));
                        named(&r, "Unlock text");
                        let typed = d.answer.chars().filter(|c| !c.is_whitespace()).count();
                        ui.label(RichText::new(format!("{typed} / {}", text.len())).small().color(muted(ui)));
                    }
                }
                if !d.error.is_empty() {
                    ui.colored_label(DANGER, &d.error);
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if can_submit {
                        submit |= ui.add(egui::Button::new(RichText::new("Stop block").color(Color32::WHITE)).fill(DANGER)).clicked();
                    }
                    close = ui.button(if can_submit { "Cancel" } else { "OK" }).clicked();
                });
            });
            if close {
                self.stop = None;
            } else if submit {
                self.submit_stop();
            }
        }

        if self.confirm_delete {
            let mut answer = None;
            Modal::new(Id::unique("delete")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading(format!("Delete “{name}”?"));
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("Delete").color(Color32::WHITE)).fill(DANGER)).clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
            if let Some(yes) = answer {
                self.confirm_delete = false;
                if yes
                    && let Some(id) = self.selected.clone()
                        && self.send(Request::DeleteBlock { id }).ok {
                            self.toast("Block deleted.", false);
                        }
            }
        }

        if self.confirm_locked {
            let mut answer = None;
            Modal::new(Id::unique("confirm_locked")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading("Use “Locked”?");
                ui.add_space(6.0);
                ui.label("With Locked there is no way to stop this block early, not even by quitting or restarting the computer.");
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(primary("Use Locked")).clicked() {
                        answer = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        answer = Some(false);
                    }
                });
            });
            if let Some(yes) = answer {
                self.confirm_locked = false;
                if yes {
                    self.save_lock();
                }
            }
        }

        if self.settings_open {
            let Some(state) = self.state.clone() else { return };
            let mut close = false;
            let mut setup = false;
            let mut quit = false;
            let mut login_change = None;
            let busy = self.setup.is_some();
            Modal::new(Id::unique("settings")).show(ctx, |ui| {
                ui.set_width(440.0);
                ui.heading("Settings");
                ui.add_space(10.0);
                let mut login = state.settings.start_at_login;
                if ui.checkbox(&mut login, "Start ProgramPrison when I log in").changed() {
                    login_change = Some(login);
                }
                ui.label(RichText::new("Keep this on so blocks still apply after a restart.").small().color(muted(ui)));
                ui.add_space(12.0);
                ui.label(RichText::new("Website blocking").strong());
                if state.hosts.writable {
                    ui.label(format!("Ready. ProgramPrison can edit {}.", state.hosts.path));
                } else {
                    ui.label(format!("Not set up. ProgramPrison needs permission to edit {}. You'll be asked for an administrator password once.", state.hosts.path));
                    if busy {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Waiting for permission…");
                        });
                    } else {
                        setup = ui.button("Set up website blocking").clicked();
                    }
                }
                ui.add_space(12.0);
                ui.label(RichText::new("Background blocker").strong());
                ui.label(RichText::new("Closing this window keeps blocks running. To stop ProgramPrison completely:").color(muted(ui)));
                quit = ui.button("Quit ProgramPrison").clicked();
                ui.add_space(10.0);
                ui.label(RichText::new(format!("ProgramPrison {}", state.version)).small().color(muted(ui)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.add(primary("Done")).clicked();
                });
            });
            if let Some(v) = login_change {
                self.send(Request::UpdateSettings { start_at_login: v });
            }
            if setup {
                self.start_setup(ctx);
            }
            if quit && self.send(Request::Quit).ok {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if close {
                self.settings_open = false;
            }
        }
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, bad, at)) = self.toast.clone() else { return };
        let ttl = if bad { 5.0 } else { 2.5 };
        if at.elapsed().as_secs_f32() > ttl {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(300));
        egui::Area::new(Id::unique("toast")).anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -20.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
            let fill = if bad { DANGER } else { Color32::from_rgb(0x1d, 0x1b, 0x18) };
            egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(16, 9)).show(ui, |ui| {
                ui.label(RichText::new(msg).color(Color32::WHITE));
            });
        });
    }
}

// ---------- small widgets ----------

/// Gives a text box an accessible name (read by screen readers, and used by the tests).
fn named(r: &egui::Response, name: &str) {
    r.ctx.accesskit_node_builder(r.id, |b| b.set_label(name));
}


fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).strong().color(Color32::WHITE)).fill(ACCENT)
}

fn count(n: usize) -> String {
    if n > 0 { n.to_string() } else { String::new() }
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).small().color(muted(ui)));
}

fn banner(ui: &mut egui::Ui, color: Color32, text: &str) {
    egui::Frame::new().fill(soft(ui, color)).corner_radius(CornerRadius::same(8)).inner_margin(10).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(text);
    });
}

fn list_frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn row(ui: &mut egui::Ui, i: usize, add: impl FnOnce(&mut egui::Ui)) {
    if i > 0 {
        ui.separator();
    }
    ui.horizontal(|ui| {
        ui.set_min_height(26.0);
        add(ui);
    });
}

fn empty_row(ui: &mut egui::Ui, text: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        ui.label(RichText::new(text).color(muted(ui)));
        ui.add_space(10.0);
    });
}

fn fmt_input(t: i64) -> String {
    use chrono::{Local, TimeZone};
    Local.timestamp_millis_opt(t).single().map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

/// The app icon: cell bars on an orange tile.
fn paint_logo(p: &egui::Painter, r: egui::Rect) {
    p.rect_filled(r, CornerRadius::same((r.width() * 0.22) as u8), ACCENT);
    let s = r.width();
    let at = |x: f32, y: f32| egui::pos2(r.left() + x * s, r.top() + y * s);
    let bar = Color32::from_rgb(0xff, 0xf7, 0xee);
    p.rect_filled(egui::Rect::from_min_max(at(0.22, 0.24), at(0.78, 0.30)), 0.0, bar);
    p.rect_filled(egui::Rect::from_min_max(at(0.22, 0.70), at(0.78, 0.76)), 0.0, bar);
    for cx in [0.305, 0.435, 0.565, 0.695] {
        p.rect_filled(egui::Rect::from_min_max(at(cx - 0.032, 0.24), at(cx + 0.032, 0.76)), 0.0, bar);
    }
}

/// The same icon as RGBA pixels, for the window and taskbar.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let ss = 4;
    let inside = |x: f32, y: f32| -> Option<[f32; 3]> {
        let (x0, y0, x1, y1, rad) = (0.04, 0.04, 0.96, 0.96, 0.2);
        if x < x0 || x > x1 || y < y0 || y > y1 {
            return None;
        }
        let cx = x.clamp(x0 + rad, x1 - rad);
        let cy = y.clamp(y0 + rad, y1 - rad);
        if (x - cx).powi(2) + (y - cy).powi(2) > rad * rad {
            return None;
        }
        let rail = (0.22..=0.78).contains(&x) && ((0.24..=0.30).contains(&y) || (0.70..=0.76).contains(&y));
        let bars = (0.24..=0.76).contains(&y) && [0.305f32, 0.435, 0.565, 0.695].iter().any(|c| (x - c).abs() <= 0.032);
        Some(if rail || bars { [255.0, 247.0, 238.0] } else { [226.0 - 30.0 * y, 108.0 - 28.0 * y, 46.0 - 12.0 * y] })
    };
    for y in 0..size {
        for x in 0..size {
            let (mut acc, mut n) = ([0f32; 3], 0u32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let fx = (x as f32 + (sx as f32 + 0.5) / ss as f32) / size as f32;
                    let fy = (y as f32 + (sy as f32 + 0.5) / ss as f32) / size as f32;
                    if let Some(c) = inside(fx, fy) {
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                        n += 1;
                    }
                }
            }
            let i = ((y * size + x) * 4) as usize;
            if n > 0 {
                for k in 0..3 {
                    px[i + k] = (acc[k] / n as f32) as u8;
                }
            }
            px[i + 3] = (n * 255 / (ss * ss)) as u8;
        }
    }
    px
}

pub fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::System);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        ctx.style_mut_of(theme, |s| {
            use egui::{FontFamily, FontId, TextStyle};
            s.text_styles = [
                (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(14.5, FontFamily::Proportional)),
                (TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(13.5, FontFamily::Monospace)),
            ]
            .into();
            s.spacing.item_spacing = Vec2::new(8.0, 6.0);
            s.spacing.button_padding = Vec2::new(12.0, 5.0);
            s.visuals.selection.bg_fill = ACCENT.gamma_multiply(0.6);
            s.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
            s.visuals.hyperlink_color = ACCENT;
            for w in [&mut s.visuals.widgets.inactive, &mut s.visuals.widgets.hovered, &mut s.visuals.widgets.active] {
                w.corner_radius = CornerRadius::same(7);
            }
            if theme == egui::Theme::Light {
                s.visuals.panel_fill = Color32::from_rgb(0xf6, 0xf5, 0xf2);
                s.visuals.window_fill = Color32::WHITE;
                s.visuals.extreme_bg_color = Color32::WHITE;
                s.visuals.faint_bg_color = Color32::from_rgb(0xf0, 0xee, 0xea);
            }
        });
    }
}

/// Held while a settings window is open. The system releases it if the window crashes.
fn window_lock() -> Option<std::fs::File> {
    let dir = crate::util::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("window.lock")).ok()?;
    f.try_lock().ok()?;
    Some(f)
}

/// Connects to the background blocker, starting it if it isn't running.
pub fn connect_or_start() -> Option<Client> {
    if let Some(c) = Client::connect() {
        return Some(c);
    }
    let exe = std::env::current_exe().ok()?;
    let _ = crate::util::command(&exe.display().to_string()).arg("--service").spawn();
    let end = Instant::now() + Duration::from_secs(6);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(c) = Client::connect() {
            return Some(c);
        }
    }
    None
}

pub fn run() {
    let Some(client) = connect_or_start() else {
        crate::util::debug("window: could not reach or start the blocker");
        return;
    };
    // Only one settings window: if one is open already, bring it forward and stop here.
    let _window_lock = match window_lock() {
        Some(lock) => lock,
        None => {
            let _ = client.send(&Request::ShowWindow);
            return;
        }
    };
    let client = Arc::new(client);
    let conn: Conn = Arc::new(move |r: &Request| client.send(r));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ProgramPrison")
            .with_app_id("programprison")
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([760.0, 540.0])
            .with_icon(egui::IconData { rgba: icon_rgba(64), width: 64, height: 64 }),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "ProgramPrison",
        options,
        Box::new(move |cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(PrisonApp::new(conn)))
        }),
    );
}
