//! Things DaemonHall does to units. Blocking (spawns systemctl & co.), so
//! handlers run these on `spawn_blocking`.
//!
//! System units: start/stop/restart go straight to systemd, allowed without
//! a password by the polkit rule for watched units only; enable/disable,
//! create, delete and daemon-reload go through the root helper via pkexec.
//! User units need no privileges at all.

use crate::journal;
use cyberwatch::unit::{Scope, Unit};
use daemonhall::unitgen::{self, Scope as Scope_, ServiceSpec};
use daemonhall::{HELPER_PATH, allow};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub type OpResult = Result<String, String>;

fn run(cmd: &mut Command) -> OpResult {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() {
            format!("exited with {}", out.status)
        } else {
            err
        })
    }
}

fn systemctl(unit: &Unit) -> Command {
    let mut c = Command::new("systemctl");
    c.args(unit.systemctl_scope_args());
    if unit.scope == Scope::System {
        // Never block on an interactive prompt there's no one to answer.
        c.arg("--no-ask-password");
    }
    c
}

fn explain_denied(e: String) -> String {
    if e.contains("Interactive authentication required") || e.contains("Access denied") {
        format!(
            "{e}\n\nsystemd refused: this unit isn't covered by DaemonHall's polkit rule. \
             Install it with packaging/install-root.sh, or add the unit to the watch list \
             (it must be a hand-installed file in /etc/systemd/system)."
        )
    } else {
        e
    }
}

pub fn helper_installed() -> bool {
    Path::new(HELPER_PATH).is_file()
}

pub fn rule_installed() -> bool {
    Path::new(allow::RULES_PATH).is_file()
}

/// Runs the root helper through pkexec and returns its message.
pub fn helper(args: &[&str], stdin: Option<String>) -> OpResult {
    if !helper_installed() {
        return Err(format!(
            "{HELPER_PATH} isn't installed. Run packaging/install-root.sh (with sudo) once."
        ));
    }
    let mut cmd = Command::new("pkexec");
    cmd.arg(HELPER_PATH)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("pkexec: {e}"))?;
    if let Some(input) = stdin {
        use std::io::Write;
        if let Some(mut s) = child.stdin.take() {
            s.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    match text
        .lines()
        .last()
        .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
    {
        Some(v) => {
            let msg = v["message"].as_str().unwrap_or("").to_string();
            if v["ok"].as_bool() == Some(true) {
                Ok(msg)
            } else {
                Err(msg)
            }
        }
        None => Err(format!(
            "helper failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// start | stop | restart | reload
pub fn control(unit: &Unit, verb: &str) -> OpResult {
    if !matches!(verb, "start" | "stop" | "restart" | "reload") {
        return Err(format!("unknown action {verb}"));
    }
    run(systemctl(unit).arg(verb).arg(&unit.name))
        .map(|_| format!("{verb} {}", unit.name))
        .map_err(explain_denied)
}

pub fn set_enabled(unit: &Unit, enable: bool) -> OpResult {
    let verb = if enable { "enable" } else { "disable" };
    match unit.scope {
        Scope::User => run(Command::new("systemctl").args(["--user", verb, &unit.name]))
            .map(|_| format!("{verb}d {}", unit.name)),
        Scope::System => helper(&[verb, &unit.name], None),
    }
}

/// Starts the service a timer triggers, now, without waiting for the timer.
pub fn run_now(unit: &Unit, triggers: &[String]) -> OpResult {
    let target = triggers
        .iter()
        .find(|t| t.ends_with(".service"))
        .ok_or("this timer doesn't trigger a service")?;
    let svc = Unit {
        scope: unit.scope,
        name: target.clone(),
    };
    run(systemctl(&svc)
        .arg("start")
        .arg("--no-block")
        .arg(&svc.name))
    .map(|_| format!("started {target} now"))
    .map_err(explain_denied)
}

pub fn daemon_reload(scope: Scope) -> OpResult {
    match scope {
        Scope::User => run(Command::new("systemctl").args(["--user", "daemon-reload"]))
            .map(|_| "user manager reloaded".into()),
        Scope::System => helper(&["daemon-reload"], None),
    }
}

// ── watch list ─────────────────────────────────────────────────────────

fn load_cfg() -> Result<cyberwatch::config::Config, String> {
    cyberwatch::config::load_or_init().map_err(|e| e.to_string())
}

fn save_cfg(cfg: &cyberwatch::config::Config) -> Result<(), String> {
    cyberwatch::config::save(cfg).map_err(|e| e.to_string())
}

/// Adds or removes a unit from cyberwatch's watch list (shared with the TUI
/// and bar widget), then refreshes the polkit rule for system units.
pub fn watch(id: &str, add: bool) -> OpResult {
    let unit = Unit::parse(id);
    if !unitgen::valid_unit_name(&unit.name) {
        return Err("invalid unit name".into());
    }
    let mut cfg = load_cfg()?;
    if add {
        cfg.ignore.retain(|i| i != id);
        save_cfg(&cfg)?;
        if !cyberwatch::discover::discover_units(&cfg)
            .iter()
            .any(|u| u == id)
        {
            cfg.extra_units.push(id.to_string());
            save_cfg(&cfg)?;
        }
    } else {
        cfg.extra_units.retain(|i| i != id);
        save_cfg(&cfg)?;
        if cyberwatch::discover::discover_units(&cfg)
            .iter()
            .any(|u| u == id)
        {
            cfg.ignore.push(id.to_string());
            save_cfg(&cfg)?;
        }
    }
    let mut msg = format!("{} {id}", if add { "watching" } else { "stopped watching" });
    if unit.scope == Scope::System && helper_installed() {
        match helper(&["sync"], None) {
            Ok(m) => msg.push_str(&format!(" · {m}")),
            Err(e) => msg.push_str(&format!(" · polkit rule not updated: {e}")),
        }
    }
    Ok(msg)
}

// ── create / delete ────────────────────────────────────────────────────

fn user_unit_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("systemd/user")
}

fn current_user() -> String {
    std::env::var("USER").unwrap_or_default()
}

pub fn create(spec: &ServiceSpec) -> OpResult {
    match spec.scope {
        Scope_::User => create_user(spec),
        Scope_::System => create_system(spec),
    }
}

fn create_user(spec: &ServiceSpec) -> OpResult {
    let r = unitgen::render(spec, &current_user())?;
    let dir = user_unit_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut files = vec![(r.service_name.clone(), r.service.clone())];
    if let (Some(n), Some(t)) = (&r.timer_name, &r.timer) {
        files.push((n.clone(), t.clone()));
    }
    for (name, _) in &files {
        if dir.join(name).exists() {
            return Err(format!("{name} already exists in {}", dir.display()));
        }
    }
    // Verify a copy first, in a private scratch dir.
    let scratch = journal::state_dir().join(format!("verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for (name, text) in &files {
        let p = scratch.join(name);
        std::fs::write(&p, text).map_err(|e| e.to_string())?;
        paths.push(p);
    }
    let verify = run(Command::new("systemd-analyze")
        .arg("--user")
        .arg("verify")
        .args(&paths));
    let _ = std::fs::remove_dir_all(&scratch);
    verify.map_err(|e| format!("systemd-analyze verify rejected the unit: {e}"))?;

    for (name, text) in &files {
        std::fs::write(dir.join(name), text).map_err(|e| e.to_string())?;
    }
    run(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
    let primary = r.timer_name.clone().unwrap_or(r.service_name.clone());
    if spec.start_now {
        run(Command::new("systemctl").args(["--user", "enable", "--now", &primary]))?;
        Ok(format!("created and started user:{primary}"))
    } else {
        Ok(format!("created user:{primary}"))
    }
}

fn create_system(spec: &ServiceSpec) -> OpResult {
    // Validate here too, for a quick, friendly error before pkexec.
    let r = unitgen::render(spec, &current_user())?;
    let mut names = vec![r.service_name.clone()];
    if let Some(t) = &r.timer_name {
        names.push(t.clone());
    }
    // Put the names on the watch list first (as this user — the root helper
    // never writes in $HOME), so the helper's polkit sync includes them.
    let mut cfg = load_cfg()?;
    let before = cfg.clone();
    for n in &names {
        if !cfg.extra_units.contains(n) {
            cfg.extra_units.push(n.clone());
        }
        cfg.ignore.retain(|i| i != n);
    }
    save_cfg(&cfg)?;
    let json = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    match helper(&["create"], Some(json)) {
        Ok(m) => Ok(m),
        Err(e) => {
            let _ = save_cfg(&before);
            Err(e)
        }
    }
}

pub fn delete(unit: &Unit) -> OpResult {
    match unit.scope {
        Scope::System => {
            let msg = helper(&["delete", &unit.name], None)?;
            if let Ok(mut cfg) = load_cfg() {
                let base = unit.base().to_string();
                cfg.extra_units.retain(|i| {
                    let u = Unit::parse(i);
                    !(u.scope == Scope::System && u.base() == base)
                });
                let _ = save_cfg(&cfg);
            }
            Ok(msg)
        }
        Scope::User => delete_user(unit),
    }
}

fn delete_user(unit: &Unit) -> OpResult {
    let dir = user_unit_dir();
    let base = unit.base().to_string();
    let mut targets = Vec::new();
    for n in [format!("{base}.service"), format!("{base}.timer")] {
        if std::fs::symlink_metadata(dir.join(&n)).is_ok() {
            targets.push(n);
        }
    }
    if !targets.contains(&unit.name) {
        return Err(format!("{} isn't a file in {}", unit.name, dir.display()));
    }
    let backups = journal::state_dir().join("backups");
    std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
    let stamp = journal::now_us() / 1_000_000;
    for t in &targets {
        let p = dir.join(t);
        // A symlinked unit (e.g. linked from a repo) is just unlinked; the
        // file it points to stays where it is.
        if !std::fs::symlink_metadata(&p)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            std::fs::copy(&p, backups.join(format!("{t}.{stamp}"))).map_err(|e| e.to_string())?;
        }
    }
    for t in &targets {
        let _ = run(Command::new("systemctl").args(["--user", "disable", "--now", t]));
        std::fs::remove_file(dir.join(t)).map_err(|e| e.to_string())?;
    }
    let _ = run(Command::new("systemctl").args(["--user", "daemon-reload"]));
    for t in &targets {
        let _ = run(Command::new("systemctl").args(["--user", "reset-failed", t]));
    }
    if let Ok(mut cfg) = load_cfg() {
        cfg.extra_units.retain(|i| {
            let u = Unit::parse(i);
            !(u.is_user() && u.base() == base)
        });
        cfg.ignore.retain(|i| {
            let u = Unit::parse(i);
            !(u.is_user() && u.base() == base)
        });
        let _ = save_cfg(&cfg);
    }
    Ok(format!(
        "removed user:{} (backup in {})",
        targets.join(" + "),
        backups.display()
    ))
}

// ── unit files ─────────────────────────────────────────────────────────

pub fn read_unit_file(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    std::fs::read_to_string(path).unwrap_or_else(|e| format!("# can't read {path}: {e}"))
}

/// Saves a user unit file (your own file — no privileges involved) after
/// `systemd-analyze --user verify` accepts it; the old one is backed up.
pub fn save_user_unit_file(unit: &Unit, text: &str) -> OpResult {
    if !unit.is_user() {
        return Err("system unit files are edited with sudoedit (Edit in terminal)".into());
    }
    if text.len() > 64 * 1024 {
        return Err("unit file too large".into());
    }
    let path = user_unit_dir().join(&unit.name);
    let real = std::fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let home = dirs::home_dir().unwrap_or_default();
    if !real.starts_with(&home) {
        return Err("that unit file isn't inside your home directory".into());
    }
    let scratch = journal::state_dir().join(format!("verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let probe = scratch.join(&unit.name);
    std::fs::write(&probe, text).map_err(|e| e.to_string())?;
    let verify = run(Command::new("systemd-analyze")
        .args(["--user", "verify"])
        .arg(&probe));
    let _ = std::fs::remove_dir_all(&scratch);
    verify.map_err(|e| format!("systemd-analyze verify rejected it: {e}"))?;

    let backups = journal::state_dir().join("backups");
    std::fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
    let stamp = journal::now_us() / 1_000_000;
    std::fs::copy(&real, backups.join(format!("{}.{stamp}", unit.name)))
        .map_err(|e| e.to_string())?;
    // Write through to the real file, so a unit symlinked from a repo keeps
    // being that repo's file.
    std::fs::write(&real, text).map_err(|e| e.to_string())?;
    run(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
    Ok(format!(
        "saved {} and reloaded (backup .{stamp})",
        unit.name
    ))
}

/// Opens a terminal running `sudoedit` on a system unit file, then
/// daemon-reload — the password prompt needs a real terminal.
pub fn edit_in_terminal(unit: &Unit, fragment: &str) -> OpResult {
    if unit.is_user() || !fragment.starts_with("/etc/systemd/system/") || fragment.contains("..") {
        return Err("only system unit files in /etc/systemd/system open this way".into());
    }
    let script = format!(
        "sudoedit '{fragment}' && sudo systemctl daemon-reload && echo 'saved + reloaded'; \
         echo; read -rp 'Press Enter to close'"
    );
    let spawned = Command::new("setsid")
        .args([
            "uwsm-app",
            "--",
            "xdg-terminal-exec",
            "--title=DaemonHall edit",
            "-e",
            "bash",
            "-c",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => Ok(format!("opened a terminal editing {fragment}")),
        Err(e) => Err(format!(
            "couldn't open a terminal ({e}). Run: sudoedit {fragment} && sudo systemctl daemon-reload"
        )),
    }
}
