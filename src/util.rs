//! Paths, the debug log, and running helper commands without flashing console windows.

use std::path::PathBuf;
use std::process::Command;

/// Where state.json and service.json live. PROGRAMPRISON_DATA overrides it (used by tests).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PROGRAMPRISON_DATA") {
        return PathBuf::from(d);
    }
    let home = || std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home)
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local").join("share"))
    };
    base.join("ProgramPrison")
}

/// PROGRAMPRISON_DEBUG=<file>: append a line per event. Written straight away so it survives a crash.
pub fn debug(msg: impl AsRef<str>) {
    if let Some(path) = std::env::var_os("PROGRAMPRISON_DEBUG") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{} {}", crate::engine::now_ms(), msg.as_ref());
        }
    }
}

/// A command that never opens a console window on Windows.
pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

/// Runs a command in the background and forgets about it (the thread reaps it).
pub fn run_detached(mut c: Command) {
    std::thread::spawn(move || {
        let _ = c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    });
}
