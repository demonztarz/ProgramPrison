//! Listing and closing processes. Read straight from the system: no helper programs are run.

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    /// Every name the process goes by: executable name, macOS bundle name...
    pub names: Vec<String>,
}

pub fn list() -> Vec<Proc> {
    imp::list()
}

pub fn kill(pid: u32) -> bool {
    if pid == std::process::id() || pid <= 4 {
        return false;
    }
    imp::kill(pid)
}

/// Programs offered in the "pick a running program" list.
pub fn running_names() -> Vec<String> {
    let mut names: Vec<String> = list()
        .into_iter()
        .filter_map(|p| p.names.into_iter().last())
        .filter(|n| crate::engine::normalize_app(n).is_ok())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Proc;

    pub fn list() -> Vec<Proc> {
        let mut out = Vec::new();
        let Ok(dir) = std::fs::read_dir("/proc") else { return out };
        for e in dir.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let mut names = Vec::new();
            if let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) {
                names.push(comm.trim_end().to_string());
            }
            // comm is cut at 15 characters; the executable's file name isn't (own processes only).
            if let Ok(exe) = std::fs::read_link(e.path().join("exe"))
                && let Some(n) = exe.file_name().and_then(|n| n.to_str()) {
                    let n = n.trim_end_matches(" (deleted)").to_string();
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
            if !names.is_empty() {
                out.push(Proc { pid, names });
            }
        }
        out
    }

    pub fn kill(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Proc;

    pub fn list() -> Vec<Proc> {
        let mut out = Vec::new();
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return out;
        }
        let mut pids = vec![0i32; n as usize + 64];
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
        let mut buf = vec![0u8; 4096];
        for &pid in pids.iter().take(n.max(0) as usize) {
            if pid <= 0 {
                continue;
            }
            let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            if len <= 0 {
                continue;
            }
            let path = String::from_utf8_lossy(&buf[..len as usize]).into_owned();
            let mut names = vec![path.rsplit('/').next().unwrap_or("").to_string()];
            // /Applications/Steam.app/Contents/MacOS/steam_osx -> also "Steam"
            if let Some(i) = path.find(".app/") {
                if let Some(bundle) = path[..i].rsplit('/').next() {
                    names.push(bundle.to_string());
                }
            }
            out.push(Proc { pid: pid as u32, names });
        }
        out
    }

    pub fn kill(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 }
    }
}

#[cfg(windows)]
mod imp {
    use super::Proc;
    pub fn list() -> Vec<Proc> {
        crate::platform::windows::list_processes()
    }
    pub fn kill(pid: u32) -> bool {
        crate::platform::windows::kill_process(pid)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lists_this_process() {
        let me = std::process::id();
        let list = super::list();
        assert!(list.len() > 1);
        assert!(list.iter().any(|p| p.pid == me), "own pid listed");
    }

    #[cfg(unix)]
    #[test]
    fn closes_by_name() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pp-procs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("timewaster");
        // A script's process takes the script's name (copying /bin/sleep fails with multi-call coreutils).
        if cfg!(target_os = "linux") {
            std::fs::write(&exe, "#!/bin/sh\nwhile :; do sleep 1; done\n").unwrap();
        } else {
            std::fs::copy("/bin/sleep", &exe).unwrap();
        }
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = std::process::Command::new(&exe).arg("30").spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let apps = vec!["timewaster".to_string()];
        let found: Vec<_> = super::list().into_iter().filter(|p| crate::engine::app_matches(&p.names, &apps)).collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(super::kill(found[0].pid));
        let status = child.wait().unwrap();
        assert!(!status.success());
    }
}
