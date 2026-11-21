#[cfg(windows)]
pub mod windows;
#[cfg(unix)]
pub mod unix;

/// Runs the background blocker until it's told to quit.
pub fn run_service() {
    #[cfg(windows)]
    windows::run_service();
    #[cfg(unix)]
    unix::run_service();
}

/// Opens the settings window of an already-running or newly started blocker.
pub fn spawn_window() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = crate::util::command(&exe.display().to_string()).spawn();
    }
}
