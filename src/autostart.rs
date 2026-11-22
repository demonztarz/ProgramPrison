//! Starting the background blocker at login.

/// Development builds and test runs never register themselves.
fn enabled() -> bool {
    !cfg!(debug_assertions) && std::env::var_os("PROGRAMPRISON_DATA").is_none()
}

fn exe() -> Option<String> {
    // An AppImage runs from a temporary mount; register the AppImage file itself.
    std::env::var("APPIMAGE").ok().or_else(|| std::env::current_exe().ok().map(|p| p.display().to_string()))
}

/// Removes the start-at-login entry, whatever build made it (used when uninstalling).
pub fn remove() {
    if std::env::var_os("PROGRAMPRISON_DATA").is_some() {
        return; // test runs never touch the real entry
    }
    imp::apply(false, "");
}

pub fn apply(on: bool) {
    if !enabled() {
        return;
    }
    let Some(exe) = exe() else { return };
    imp::apply(on, &exe);
}

#[cfg(windows)]
mod imp {
    pub fn apply(on: bool, exe: &str) {
        crate::platform::windows::set_run_at_login(on, &format!("\"{exe}\" --service"));
    }
}

#[cfg(target_os = "macos")]
mod imp {
    // Starts at login only. Deliberately not restarted after a force-quit: that stays
    // available as an emergency way out (Activity Monitor → Force Quit).
    pub fn apply(on: bool, exe: &str) {
        let Some(home) = std::env::var_os("HOME") else { return };
        let dir = std::path::Path::new(&home).join("Library/LaunchAgents");
        let file = dir.join("app.programprison.plist");
        if on {
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>app.programprison</string>
<key>ProgramArguments</key><array><string>{exe}</string><string>--service</string></array>
<key>RunAtLoad</key><true/>
</dict></plist>
"#
            );
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&file, plist);
        } else {
            let _ = std::fs::remove_file(&file);
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub fn apply(on: bool, exe: &str) {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")));
        let Some(base) = base else { return };
        let dir = base.join("autostart");
        let file = dir.join("programprison.desktop");
        if on {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&file, format!("[Desktop Entry]\nType=Application\nName=Program Prison\nExec=\"{exe}\" --service\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n"));
        } else {
            let _ = std::fs::remove_file(&file);
        }
    }
}
