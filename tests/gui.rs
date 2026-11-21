//! Clicks through the real settings window (headless, via egui's test harness) against a real
//! background blocker: create a block, add sites and a program, start it, lock it, unlock it by
//! typing the random text, and check that pasting the text is refused.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use programprison::gui::{Conn, PrisonApp, Tab};
use programprison::ipc::{Client, Request};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_programprison");

fn start_service() -> (Child, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("pp-gui-{}-{}", std::process::id(), programprison::engine::now_ms()));
    std::fs::create_dir_all(dir.join("data")).unwrap();
    let hosts = dir.join("hosts");
    std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
    unsafe {
        std::env::set_var("PROGRAMPRISON_DATA", dir.join("data"));
    }
    let child = Command::new(BIN)
        .arg("--service")
        .env("PROGRAMPRISON_DATA", dir.join("data"))
        .env("PROGRAMPRISON_HOSTS", &hosts)
        .env("PROGRAMPRISON_TICK_MS", "300")
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(10);
    while Client::connect().is_none() {
        assert!(Instant::now() < end, "service didn't start");
        std::thread::sleep(Duration::from_millis(100));
    }
    (child, hosts)
}

fn harness() -> Harness<'static, PrisonApp> {
    let client = std::sync::Arc::new(Client::connect().unwrap());
    let conn: Conn = std::sync::Arc::new(move |r: &Request| client.send(r));
    let app = PrisonApp::new(conn);
    let mut h = Harness::builder().with_size([1040.0, 720.0]).build_ui_state(|ui, app: &mut PrisonApp| app.draw(ui), app);
    programprison::gui::setup_style(&h.ctx);
    h.run_steps(3);
    h
}

#[test]
fn click_through_the_window() {
    let (mut svc, hosts) = start_service();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&hosts)));
    let _ = svc.kill();
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}

fn run(hosts: &std::path::Path) {
    let mut h = harness();

    eprintln!("• empty state, create a block");
    h.get_by_label("Create your first block").click();
    h.run_steps(3);
    assert_eq!(h.state().state.as_ref().unwrap().blocks.len(), 1);
    assert!(h.state().block().is_some());

    eprintln!("• add a website by typing and pressing Add");
    let input = h.get_by_label("Website to block");
    input.focus();
    h.run_steps(1);
    h.get_by_label("Website to block").type_text("https://www.reddit.com/r/all");
    h.run_steps(1);
    h.get_by_label("Add").click();
    h.run_steps(3);
    assert_eq!(h.state().block().unwrap().sites, vec!["reddit.com"]);

    eprintln!("• quick-add a preset");
    h.get_by_label("+ Video & streaming").click();
    h.run_steps(3);
    assert!(h.state().block().unwrap().sites.contains(&"youtube.com".to_string()));
    assert!(h.query_by_label("Video & streaming added").is_some(), "preset shows as added");

    eprintln!("• programs tab: add one, refuse a system program");
    h.get_by_label_contains("Programs").click();
    h.run_steps(3);
    assert_eq!(h.state().tab, Tab::Apps);
    h.get_by_label("Program to block").focus();
    h.run_steps(1);
    h.get_by_label("Program to block").type_text("Steam.exe");
    h.run_steps(1);
    h.get_by_label("Add").click();
    h.run_steps(3);
    assert_eq!(h.state().block().unwrap().apps[0].name, "steam");
    h.get_by_label("Program to block").focus();
    h.run_steps(1);
    h.get_by_label("Program to block").type_text("explorer.exe");
    h.run_steps(1);
    h.get_by_label("Add").click();
    h.run_steps(3);
    assert_eq!(h.state().block().unwrap().apps.len(), 1);
    assert!(h.state().toast.as_ref().is_some_and(|t| t.1 && t.0.contains("can't be blocked")));

    eprintln!("• lock tab: random text (200 characters)");
    h.get_by_label_contains("Lock").click();
    h.run_steps(3);
    h.get_by_label("Random text").click();
    h.run_steps(2);
    h.get_by_label("Save lock").click();
    h.run_steps(3);
    assert_eq!(h.state().block().unwrap().lock, "random");

    eprintln!("• start for 30 minutes from the dialog");
    h.get_by_label("Start").click();
    h.run_steps(3);
    h.get_by_label("30 min").click();
    h.run_steps(2);
    h.get_by_label("Start block").click();
    h.run_steps(3);
    let b = h.state().block().unwrap().clone();
    assert!(b.status.active && b.status.locked, "{:?}", b.status);
    let left = b.status.until.unwrap() - programprison::engine::now_ms();
    assert!((29 * 60_000..=30 * 60_000).contains(&left), "{left}");
    let end = Instant::now() + Duration::from_secs(5);
    while !std::fs::read_to_string(hosts).unwrap().contains("0.0.0.0 reddit.com") {
        assert!(Instant::now() < end, "hosts file not written");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(h.query_by_label_contains("Locked. You can add").is_some(), "locked banner shown");

    eprintln!("• the stop dialog: pasting is refused, a wrong text is refused, the right text stops it");
    h.get_by_label("Stop").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("type these 200 characters").is_some());
    // Paste goes nowhere.
    h.get_by_label("Unlock text").focus();
    h.run_steps(1);
    h.event(egui::Event::Paste("abcdefghijklmnopqrst".into()));
    h.run_steps(2);
    assert_eq!(h.get_by_label("Unlock text").value().unwrap_or_default(), "", "paste ignored");
    h.get_by_label("Unlock text").type_text("wrongwrongwrongwrong");
    h.run_steps(1);
    h.get_by_label("Stop block").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("doesn't match").is_some(), "mismatch error shown");
    assert!(h.state().block().unwrap().status.active, "still on");
    // The new challenge text is the monospace label; spaces between groups are ignored.
    // The text to type is the label made only of letters and digits in groups of ten.
    let challenge: String = h
        .get_all_by_role(egui::accesskit::Role::Label)
        .filter_map(|n| n.accesskit_node().value())
        .map(|v| v.chars().filter(|c| !c.is_whitespace()).collect::<String>())
        .find(|t| t.len() == 200 && t.chars().all(|c| c.is_ascii_alphanumeric()))
        .expect("challenge text on screen");
    assert_eq!(challenge.len(), 200);
    h.get_by_label("Unlock text").focus();
    h.run_steps(1);
    h.get_by_label("Unlock text").type_text(&challenge);
    h.run_steps(1);
    h.get_by_label("Stop block").click();
    h.run_steps(3);
    assert!(!h.state().block().unwrap().status.active, "stopped");

    eprintln!("• schedule tab renders and the preset adds hours");
    h.get_by_label_contains("Schedule").click();
    h.run_steps(3);
    h.get_by_label("Add weekdays 9–5").click();
    h.run_steps(3);
    assert_eq!(h.state().block().unwrap().schedule.iter().filter(|&&x| x).count(), 40);

    eprintln!("• settings open and close");
    h.get_by_label("Settings").click();
    h.run_steps(3);
    assert!(h.query_by_label("Start ProgramPrison when I log in").is_some());
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(h.query_by_label("Start ProgramPrison when I log in").is_none());
    eprintln!("all window checks passed");
}
