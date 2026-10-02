//! daemonhall-unitctl — DaemonHall's root helper.
//!
//! Installed root-owned at /usr/local/bin and run through `pkexec`; a polkit
//! rule (written by `sync`) lets the DaemonHall user run it without a
//! password. Because of that it trusts nothing it's given:
//!
//! - the user is taken from `PKEXEC_UID` (or `SUDO_UID` when an admin runs
//!   it by hand), never from arguments;
//! - every unit it touches must be on that user's watch list AND be a
//!   hand-installed regular file in /etc/systemd/system (see `allow.rs`);
//! - new units are rendered by `unitgen` from structured JSON on stdin,
//!   which forces `User=` to that user and validates every field;
//! - it only ever writes /etc/systemd/system/<validated name>, the polkit
//!   rule file and backups under /var/lib/daemonhall — never anything in
//!   the user's home.
//!
//! Usage: sync | daemon-reload | enable <unit> | disable <unit> |
//!        create (ServiceSpec JSON on stdin) | delete <unit>
//! Prints one JSON line: {"ok": bool, "message": "..."}.

use daemonhall::allow::{self, RULES_PATH, SYSTEM_DIR};
use daemonhall::unitgen::{self, Scope, ServiceSpec};
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const SYSTEMCTL: &str = "/usr/bin/systemctl";
const ANALYZE: &str = "/usr/bin/systemd-analyze";
const BACKUP_DIR: &str = "/var/lib/daemonhall/backups";

struct Caller {
    user: String,
    home: PathBuf,
}

fn main() {
    let result = run();
    let (ok, message) = match result {
        Ok(m) => (true, m),
        Err(m) => (false, m),
    };
    println!("{}", serde_json::json!({ "ok": ok, "message": message }));
    std::process::exit(if ok { 0 } else { 1 });
}

fn run() -> Result<String, String> {
    if euid() != 0 {
        return Err("daemonhall-unitctl must run as root (via pkexec)".into());
    }
    let caller = caller()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let unit_arg = || -> Result<String, String> {
        let u = args.get(1).cloned().ok_or("missing unit name")?;
        if !unitgen::valid_unit_name(&u) {
            return Err(format!("invalid unit name {u:?}"));
        }
        Ok(u)
    };
    match cmd {
        "sync" => sync(&caller),
        "daemon-reload" => systemctl(&["daemon-reload"]).map(|_| "daemon reloaded".into()),
        "enable" => {
            let u = managed(&caller, &unit_arg()?)?;
            systemctl(&["enable", &u])?;
            Ok(format!("enabled {u}"))
        }
        "disable" => {
            let u = managed(&caller, &unit_arg()?)?;
            systemctl(&["disable", &u])?;
            Ok(format!("disabled {u}"))
        }
        "create" => create(&caller),
        "delete" => delete(&caller, &unit_arg()?),
        _ => Err(
            "usage: sync | daemon-reload | enable <unit> | disable <unit> | create | delete <unit>"
                .into(),
        ),
    }
}

fn euid() -> u32 {
    // /proc/self/status avoids pulling in libc just for geteuid().
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(2).map(str::to_string))
        })
        .and_then(|v| v.parse().ok())
        .unwrap_or(u32::MAX)
}

/// The invoking (non-root) user: pkexec sets PKEXEC_UID; sudo sets SUDO_UID.
fn caller() -> Result<Caller, String> {
    let uid = std::env::var("PKEXEC_UID")
        .or_else(|_| std::env::var("SUDO_UID"))
        .map_err(|_| "run me through pkexec (no PKEXEC_UID)".to_string())?;
    let uid: u32 = uid.parse().map_err(|_| "bad PKEXEC_UID".to_string())?;
    if uid == 0 {
        return Err("refusing to act for root".into());
    }
    let passwd = std::fs::read_to_string("/etc/passwd").map_err(|e| e.to_string())?;
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 6 && f[2].parse::<u32>().ok() == Some(uid) {
            return Ok(Caller {
                user: f[0].to_string(),
                home: PathBuf::from(f[5]),
            });
        }
    }
    Err(format!("no passwd entry for uid {uid}"))
}

fn systemctl(args: &[&str]) -> Result<String, String> {
    let out = Command::new(SYSTEMCTL)
        .args(args)
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "systemctl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn managed(caller: &Caller, unit: &str) -> Result<String, String> {
    if allow::managed_system_units(&caller.home)
        .iter()
        .any(|u| u == unit)
    {
        Ok(unit.to_string())
    } else {
        Err(format!(
            "{unit} isn't a DaemonHall-managed unit (watched + hand-installed in {SYSTEM_DIR})"
        ))
    }
}

fn write_root_file(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("daemonhall-tmp");
    std::fs::write(&tmp, content).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| format!("install {}: {e}", path.display()))
}

fn sync(caller: &Caller) -> Result<String, String> {
    let units = allow::managed_system_units(&caller.home);
    write_root_file(
        Path::new(RULES_PATH),
        &allow::polkit_rules(&caller.user, &units),
    )?;
    Ok(format!("polkit rule updated: {} units", units.len()))
}

fn create(caller: &Caller) -> Result<String, String> {
    let mut input = String::new();
    std::io::stdin()
        .take(64 * 1024)
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    let spec: ServiceSpec =
        serde_json::from_str(&input).map_err(|e| format!("bad request: {e}"))?;
    if spec.scope != Scope::System {
        return Err("the helper only installs system units; user units need no root".into());
    }
    let r = unitgen::render(&spec, &caller.user)?;

    let mut files = vec![(r.service_name.clone(), r.service.clone())];
    if let (Some(n), Some(t)) = (&r.timer_name, &r.timer) {
        files.push((n.clone(), t.clone()));
    }
    for (name, _) in &files {
        if std::fs::symlink_metadata(Path::new(SYSTEM_DIR).join(name)).is_ok()
            || Path::new("/usr/lib/systemd/system").join(name).exists()
        {
            return Err(format!("{name} already exists"));
        }
    }

    // Verify in a private scratch dir before anything touches /etc.
    let scratch = PathBuf::from(format!("/run/daemonhall-verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir(&scratch).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700)).ok();
    let mut paths = Vec::new();
    for (name, text) in &files {
        let p = scratch.join(name);
        std::fs::write(&p, text).map_err(|e| e.to_string())?;
        paths.push(p);
    }
    let verify = Command::new(ANALYZE)
        .arg("verify")
        .args(&paths)
        .env("SYSTEMD_LOG_COLOR", "0")
        .output();
    let _ = std::fs::remove_dir_all(&scratch);
    let verify = verify.map_err(|e| format!("systemd-analyze: {e}"))?;
    if !verify.status.success() {
        return Err(format!(
            "systemd-analyze verify rejected the unit: {}",
            String::from_utf8_lossy(&verify.stderr).trim()
        ));
    }

    for (name, text) in &files {
        write_root_file(&Path::new(SYSTEM_DIR).join(name), text)?;
    }
    systemctl(&["daemon-reload"])?;

    // The web server (running as the user) has already put these names on
    // the watch list, so this sync includes them. The helper never writes
    // inside the user's home: a root process writing into a directory the
    // user controls is a symlink-race waiting to happen.
    sync(caller)?;

    let primary = r.timer_name.clone().unwrap_or(r.service_name.clone());
    if spec.start_now {
        systemctl(&["enable", "--now", &primary])?;
        Ok(format!("installed and started {primary}"))
    } else {
        Ok(format!("installed {primary} (not started)"))
    }
}

fn delete(caller: &Caller, unit: &str) -> Result<String, String> {
    let unit = managed(caller, unit)?;
    let mut targets = vec![unit.clone()];
    // A service and its same-named timer go together.
    let base = unit.trim_end_matches(".service").trim_end_matches(".timer");
    for sibling in [format!("{base}.timer"), format!("{base}.service")] {
        if sibling != unit && allow::is_admin_unit_file(&sibling) {
            targets.push(sibling);
        }
    }

    std::fs::create_dir_all(BACKUP_DIR).map_err(|e| e.to_string())?;
    std::fs::set_permissions(
        "/var/lib/daemonhall",
        std::fs::Permissions::from_mode(0o755),
    )
    .ok();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for t in &targets {
        let src = Path::new(SYSTEM_DIR).join(t);
        std::fs::copy(&src, Path::new(BACKUP_DIR).join(format!("{t}.{stamp}")))
            .map_err(|e| format!("backup {t}: {e}"))?;
    }
    for t in &targets {
        let _ = systemctl(&["disable", "--now", t]);
        std::fs::remove_file(Path::new(SYSTEM_DIR).join(t))
            .map_err(|e| format!("remove {t}: {e}"))?;
    }
    systemctl(&["daemon-reload"])?;
    for t in &targets {
        let _ = systemctl(&["reset-failed", t]);
    }
    sync(caller)?;
    Ok(format!(
        "removed {} (backup in {BACKUP_DIR}, suffix .{stamp})",
        targets.join(" + ")
    ))
}
