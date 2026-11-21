// No console window on Windows: both the blocker and the settings window are GUI programs.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--service") {
        programprison::platform::run_service();
    } else if let Some(out) = args.iter().find_map(|a| a.strip_prefix("--self-test=")) {
        self_test(out);
    } else {
        programprison::gui::run();
    }
}

/// Writes what works on this machine to a file (used to check release builds).
fn self_test(out: &str) {
    let procs = programprison::procs::list();
    let report = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "processes": procs.len(),
        "ownProcessFound": procs.iter().any(|p| p.pid == std::process::id()),
        "hostsPath": programprison::hosts::hosts_path().display().to_string(),
        "ok": procs.len() > 1 && procs.iter().any(|p| p.pid == std::process::id()),
    });
    let _ = std::fs::write(out, serde_json::to_string_pretty(&report).unwrap());
}
