//! The system hosts file: websites are blocked by pointing them at 0.0.0.0 there.

use crate::{engine, util};
use std::io::Write;
use std::path::PathBuf;

pub fn hosts_path() -> PathBuf {
    if let Some(p) = std::env::var_os("PROGRAMPRISON_HOSTS") {
        return PathBuf::from(p);
    }
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows"));
        root.join("System32").join("drivers").join("etc").join("hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

/// Whether we may write the file, without changing it.
pub fn can_write(path: &std::path::Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

#[derive(Debug, PartialEq, Eq)]
pub enum Sync {
    Unchanged,
    Changed,
    NeedsSetup(String),
    Failed(String),
}

/// Writes our section for these domains. Writes in place (no rename) so the permission
/// granted on the file by `grant_access` survives.
pub fn sync(path: &std::path::Path, domains: &[String]) -> Sync {
    let current = match std::fs::read(path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Sync::Failed(e.to_string()),
    };
    let next = engine::apply_hosts_section(&current, domains);
    if next == current {
        return Sync::Unchanged;
    }
    let res = std::fs::OpenOptions::new().write(true).truncate(true).create(true).open(path).and_then(|mut f| f.write_all(next.as_bytes()));
    match res {
        Ok(()) => {
            flush_dns();
            Sync::Changed
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Sync::NeedsSetup(e.to_string()),
        Err(e) => Sync::Failed(e.to_string()),
    }
}

fn flush_dns() {
    let mut c;
    if cfg!(windows) {
        c = util::command("ipconfig");
        c.arg("/flushdns");
    } else if cfg!(target_os = "macos") {
        c = util::command("dscacheutil");
        c.arg("-flushcache");
    } else {
        c = util::command("resolvectl");
        c.arg("flush-caches");
    }
    util::run_detached(c);
}

fn user_name() -> String {
    std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default()
}

/// One-time step: asks for the administrator password once so this user may edit the hosts
/// file from then on, and Program Prison never has to run as administrator.
pub fn grant_access() -> Result<(), String> {
    let path = hosts_path();
    let p = path.display().to_string();
    let user = user_name();
    let out = if cfg!(windows) {
        let account = match std::env::var("USERDOMAIN") {
            Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
            _ => user.clone(),
        };
        let inner = format!("attrib -r \"{p}\" & icacls \"{p}\" /grant \"{account}:M\"");
        let ps = format!(
            "$p = Start-Process -FilePath cmd.exe -ArgumentList '/c', '{}' -Verb RunAs -Wait -WindowStyle Hidden -PassThru; exit $p.ExitCode",
            inner.replace('\'', "''")
        );
        util::command("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &ps]).output()
    } else if cfg!(target_os = "macos") {
        let shell = format!("/bin/chmod +a 'user:{user} allow write,append' /private/etc/hosts");
        let script = format!("do shell script \"{}\" with administrator privileges", shell.replace('\\', "\\\\").replace('"', "\\\""));
        util::command("osascript").args(["-e", &script]).output()
    } else {
        let has_setfacl = util::command("sh").args(["-c", "command -v setfacl"]).output().map(|o| o.status.success()).unwrap_or(false);
        if !has_setfacl {
            return Err("The setfacl tool is missing. Install the \"acl\" package (for example: sudo apt install acl), then try again.".into());
        }
        util::command("pkexec").args(["setfacl", "-m", &format!("u:{user}:rw"), &p]).output()
    };
    if can_write(&path) {
        return Ok(());
    }
    let detail = match out {
        Ok(o) => String::from_utf8_lossy(if o.stderr.is_empty() { &o.stdout } else { &o.stderr }).lines().next().unwrap_or("").trim().to_string(),
        Err(e) => e.to_string(),
    };
    Err(if detail.is_empty() { "Permission was not granted.".into() } else { format!("Permission was not granted ({detail}).") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pp-hosts-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&d).unwrap();
        d.join("hosts")
    }

    #[test]
    fn sync_in_place() {
        let f = tmp("a");
        std::fs::write(&f, "127.0.0.1 localhost\n").unwrap();
        #[cfg(unix)]
        let ino = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&f).unwrap());
        assert_eq!(sync(&f, &["reddit.com".into()]), Sync::Changed);
        assert!(std::fs::read_to_string(&f).unwrap().contains("0.0.0.0 reddit.com"));
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&f).unwrap()), ino, "written in place");
        assert_eq!(sync(&f, &["reddit.com".into()]), Sync::Unchanged);
        assert_eq!(sync(&f, &[]), Sync::Changed);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "127.0.0.1 localhost\n");
    }

    #[test]
    fn reports_missing_permission() {
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            return; // root can write anything
        }
        let f = tmp("b");
        std::fs::write(&f, "127.0.0.1 localhost\n").unwrap();
        let mut perm = std::fs::metadata(&f).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&f, perm).unwrap();
        assert!(matches!(sync(&f, &["reddit.com".into()]), Sync::NeedsSetup(_)));
        assert!(!can_write(&f));
        assert_eq!(sync(&f, &[]), Sync::Unchanged, "nothing to remove, nothing to write");
    }
}
