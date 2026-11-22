//! Clicks through the real settings window (headless, via egui's test harness) against a real
//! background blocker: create a block, add sites and a program, start it, lock it, unlock it by
//! typing the random words, and check that pasting them is refused.

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
            .env("PROGRAMPRISON_DISCORD_CLIENT_ID", "") // never show test blocks on a real Discord
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

    eprintln!("• lock tab: random words (30 words)");
    h.get_by_label_contains("Lock").click();
    h.run_steps(3);
    h.get_by_label("Random words").click();
    h.run_steps(2);
    h.get_by_label("Save lock").click();
    h.run_steps(3);
    assert_eq!(h.state().block().unwrap().lock, "random");

    eprintln!("• start for 30 minutes from the dialog");
    h.get_by_label("Start").click();
    h.run_steps(3);
    let len = |h: &Harness<'static, PrisonApp>| format!("{} {}", h.get_by_label("h").value().unwrap_or_default(), h.get_by_label("min").value().unwrap_or_default());
    assert_eq!(len(&h), "0 h 0 min", "starts at zero");
    h.get_by_label("+5m").click();
    h.run_steps(2);
    h.get_by_label("+5m").click();
    h.run_steps(2);
    h.get_by_label("-5m").click();
    h.run_steps(2);
    assert_eq!(len(&h), "0 h 5 min", "+5m +5m -5m");
    h.get_by_label("-5m").click();
    h.run_steps(2);
    h.get_by_label("-5m").click();
    h.run_steps(2);
    assert_eq!(len(&h), "0 h 0 min", "never below zero");
    for _ in 0..13 {
        h.get_by_label("+5m").click();
        h.run_steps(2);
    }
    assert_eq!(len(&h), "1 h 5 min", "minutes roll over into hours");
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

    eprintln!("• the stop dialog: typing-race style, words lock in one at a time");
    h.get_by_label("Stop").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("type these 30 words").is_some());
    assert!(h.query_by_label("Stop block").is_none(), "no Stop block button: it finishes by itself");
    assert!(h.query_by_label("Close").is_some(), "the dialog can still be closed");
    let words: Vec<String> = h
        .get_all_by_role(egui::accesskit::Role::Label)
        .filter_map(|n| n.accesskit_node().value())
        .map(|v| v.to_string())
        .find(|t| t.split(' ').count() == 30 && t.chars().all(|c| c.is_ascii_lowercase() || c == ' '))
        .expect("word chain on screen")
        .split(' ')
        .map(String::from)
        .collect();
    let progress = |h: &Harness<'static, PrisonApp>| h.query_by_label_contains(" / 30 words").map(|n| n.accesskit_node().value().unwrap_or_default().to_string()).unwrap_or_default();
    let box_value = |h: &Harness<'static, PrisonApp>| h.get_by_label("Unlock text").value().unwrap_or_default().to_string();
    let type_in = |h: &mut Harness<'static, PrisonApp>, t: &str| {
        h.get_by_label("Unlock text").type_text(t);
        h.run_steps(3);
    };

    // Paste goes nowhere.
    h.event(egui::Event::Paste(words[..3].join(" ")));
    h.run_steps(2);
    assert_eq!(box_value(&h), "", "paste ignored");

    // A wrong word can't be locked in: it stays in the box, progress doesn't move.
    type_in(&mut h, "zzzz ");
    assert_eq!(progress(&h), "0 / 30 words");
    assert_eq!(box_value(&h), "zzzz", "wrong word kept so it can be fixed");
    for _ in 0..4 {
        h.key_press(egui::Key::Backspace);
        h.run_steps(1);
    }
    assert_eq!(box_value(&h), "");

    // Right words lock in one by one; capitals don't matter.
    for w in &words[..3] {
        type_in(&mut h, &format!("{} ", w.to_uppercase()));
    }
    assert_eq!(progress(&h), "3 / 30 words");
    assert_eq!(box_value(&h), "", "the box clears after each word");

    // Closing keeps the progress: reopening carries on from word 4 with the same words.
    h.get_by_label("Close").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains(" / 30 words").is_none(), "dialog closed");
    h.get_by_label("Stop").click();
    h.run_steps(3);
    assert_eq!(progress(&h), "3 / 30 words", "progress kept after closing");

    // The rest, then the last word with no space: the block stops by itself.
    for w in &words[3..29] {
        type_in(&mut h, &format!("{w} "));
    }
    assert_eq!(progress(&h), "29 / 30 words");
    type_in(&mut h, &words[29]);
    h.run_steps(3);
    assert!(!h.state().block().unwrap().status.active, "stopped after the last word");
    assert!(h.query_by_label_contains(" / 30 words").is_none(), "dialog gone");

    eprintln!("• schedule tab renders and the preset adds hours");
    h.get_by_label_contains("Schedule").click();
    h.run_steps(3);
    h.get_by_label("Add weekdays 9–5").click();
    h.run_steps(3);
    assert_eq!(h.state().block().unwrap().schedule.iter().filter(|&&x| x).count(), 40);

    eprintln!("• settings open and close");
    h.get_by_label("Settings").click();
    h.run_steps(3);
    assert!(h.query_by_label("Start Program Prison when I log in").is_some());
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(h.query_by_label("Start Program Prison when I log in").is_none());
    eprintln!("all window checks passed");
}
