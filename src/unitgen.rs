//! Builds unit files from structured form fields — never from raw text.
//!
//! The root helper (`daemonhall-unitctl`) installs what this renders into
//! /etc/systemd/system without a password prompt, so the renderer is the
//! security boundary: every field is validated, `User=` is forced to the
//! requesting user, `ExecStart=` must be an absolute path to an existing
//! executable (which rules out systemd's `+`/`!`/`-`/`@`/`:` prefixes,
//! including `+` = "run as root"), and `%` is escaped so systemd specifiers
//! can't smuggle anything in. Hardening follows the blackbox service
//! template (`~/Templates/devops/New Service.service`).

use serde::{Deserialize, Serialize};
use std::path::Path;

/// First line of every unit DaemonHall writes; `delete` and the UI use it
/// to tell DaemonHall-made units from hand-written ones.
pub const MARKER: &str = "# Managed by DaemonHall (cybercore-tech/DaemonHall)";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    System,
    User,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Restart {
    No,
    OnFailure,
    Always,
}

impl Restart {
    fn as_str(self) -> &'static str {
        match self {
            Restart::No => "no",
            Restart::OnFailure => "on-failure",
            Restart::Always => "always",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TimerSpec {
    /// `OnCalendar=` expression, e.g. `daily`, `*-*-* 03:00:00`, `hourly`.
    pub on_calendar: String,
    #[serde(default)]
    pub persistent: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServiceSpec {
    pub scope: Scope,
    /// Base name, without `.service`.
    pub name: String,
    pub description: String,
    /// Absolute path to the executable, then its arguments.
    pub exec: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
    #[serde(default)]
    pub environment: Vec<(String, String)>,
    pub restart: Restart,
    #[serde(default = "default_restart_sec")]
    pub restart_sec: u32,
    /// Apply the template's sandboxing (ProtectSystem=strict etc.).
    #[serde(default = "yes")]
    pub hardened: bool,
    /// Extra writable paths when hardened (ProtectHome=read-only otherwise).
    #[serde(default)]
    pub writable_paths: Vec<String>,
    #[serde(default)]
    pub after_network: bool,
    /// Run on a schedule instead of as a long-running daemon.
    #[serde(default)]
    pub timer: Option<TimerSpec>,
    /// Enable and start right after installing.
    #[serde(default = "yes")]
    pub start_now: bool,
}

fn default_restart_sec() -> u32 {
    5
}
fn yes() -> bool {
    true
}

pub struct Rendered {
    pub service_name: String,
    pub service: String,
    pub timer_name: Option<String>,
    pub timer: Option<String>,
}

pub fn valid_unit_base(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || *c == b'_')
}

/// A full unit file name we're willing to act on: `[A-Za-z0-9_.-]+` with a
/// `.service` or `.timer` suffix, no template `@`, no path separators.
pub fn valid_unit_name(name: &str) -> bool {
    let Some(base) = name
        .strip_suffix(".service")
        .or_else(|| name.strip_suffix(".timer"))
    else {
        return false;
    };
    !base.is_empty()
        && base.len() <= 128
        && base
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && !base.starts_with('.')
}

fn clean_line(field: &str, v: &str, max: usize) -> Result<String, String> {
    if v.len() > max {
        return Err(format!("{field} is longer than {max} characters"));
    }
    if v.chars().any(|c| c.is_control()) {
        return Err(format!(
            "{field} can't contain newlines or control characters"
        ));
    }
    Ok(v.trim().to_string())
}

/// systemd specifier escaping: a literal `%` is `%%`.
fn esc_pct(v: &str) -> String {
    v.replace('%', "%%")
}

/// One `ExecStart=` word: quote when needed, escape `\` and `"`, and `%`.
fn exec_word(w: &str) -> String {
    let e = esc_pct(w);
    if !e.is_empty()
        && e.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/._-=:,+@".contains(&c))
    {
        e
    } else {
        format!("\"{}\"", e.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

fn abs_path(field: &str, p: &str) -> Result<String, String> {
    let p = clean_line(field, p, 512)?;
    if !p.starts_with('/') || p.contains("/../") || p.ends_with("/..") {
        return Err(format!("{field} must be an absolute path"));
    }
    if p.chars().any(char::is_whitespace) {
        return Err(format!("{field} can't contain spaces"));
    }
    Ok(p)
}

/// Validates `spec` and renders the unit file(s) to install for `user`
/// (the account the service runs as; ignored for user-scope units).
pub fn render(spec: &ServiceSpec, user: &str) -> Result<Rendered, String> {
    if !valid_unit_base(&spec.name) {
        return Err(
            "name: lowercase letters, digits, - and _, starting with a letter (max 64)".into(),
        );
    }
    let description = clean_line("description", &spec.description, 200)?;
    if description.is_empty() {
        return Err("description is required".into());
    }

    let exec = abs_path("program", &spec.exec)?;
    let meta = std::fs::metadata(&exec).map_err(|_| format!("program {exec} doesn't exist"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
            return Err(format!("program {exec} isn't an executable file"));
        }
    }
    let mut exec_line = exec_word(&exec);
    for a in &spec.args {
        let a = clean_line("argument", a, 512)?;
        if a.is_empty() {
            continue;
        }
        exec_line.push(' ');
        exec_line.push_str(&exec_word(&a));
    }

    let workdir = match &spec.working_directory {
        Some(w) if !w.trim().is_empty() => {
            let w = abs_path("working directory", w)?;
            if !Path::new(&w).is_dir() {
                return Err(format!("working directory {w} doesn't exist"));
            }
            Some(w)
        }
        _ => None,
    };

    let mut env_lines = Vec::new();
    for (k, v) in &spec.environment {
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        if !(k
            .bytes()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase() || c == b'_')
            && k.bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'))
        {
            return Err(format!("environment name {k:?}: use A-Z, 0-9 and _"));
        }
        let v = clean_line("environment value", v, 1024)?;
        env_lines.push(format!(
            "Environment=\"{}={}\"",
            k,
            esc_pct(&v).replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }

    let mut writable = Vec::new();
    for p in &spec.writable_paths {
        if p.trim().is_empty() {
            continue;
        }
        writable.push(esc_pct(&abs_path("writable path", p)?));
    }

    if !(1..=3600).contains(&spec.restart_sec) {
        return Err("restart delay must be 1–3600 seconds".into());
    }

    let timer = match &spec.timer {
        Some(t) => {
            let cal = clean_line("schedule", &t.on_calendar, 120)?;
            if cal.is_empty() {
                return Err("schedule (OnCalendar) is required for a timer".into());
            }
            Some((cal, t.persistent))
        }
        None => None,
    };

    let user_scope = spec.scope == Scope::User;
    let mut s = String::new();
    s.push_str(MARKER);
    s.push('\n');
    s.push_str("[Unit]\n");
    s.push_str(&format!("Description={}\n", esc_pct(&description)));
    if spec.after_network {
        if user_scope {
            s.push_str("After=network-online.target\n");
        } else {
            s.push_str("After=network-online.target\nWants=network-online.target\n");
        }
    }
    s.push_str("\n[Service]\n");
    s.push_str(if timer.is_some() {
        "Type=oneshot\n"
    } else {
        "Type=simple\n"
    });
    if !user_scope {
        if !user
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || user.is_empty()
        {
            return Err("invalid user".into());
        }
        s.push_str(&format!("User={user}\nGroup={user}\n"));
    }
    if let Some(w) = &workdir {
        s.push_str(&format!("WorkingDirectory={}\n", esc_pct(w)));
    }
    for e in &env_lines {
        s.push_str(e);
        s.push('\n');
    }
    s.push_str(&format!("ExecStart={exec_line}\n"));
    if timer.is_none() && spec.restart != Restart::No {
        s.push_str(&format!(
            "Restart={}\nRestartSec={}\n",
            spec.restart.as_str(),
            spec.restart_sec
        ));
    }
    if spec.hardened {
        s.push_str(
            "\n# Hardening (blackbox convention): ProtectSystem=strict does NOT make /home\n",
        );
        s.push_str(
            "# read-only here, so it's paired with ProtectHome=read-only + ReadWritePaths.\n",
        );
        s.push_str("NoNewPrivileges=true\nProtectSystem=strict\nProtectHome=read-only\n");
        if !writable.is_empty() {
            s.push_str(&format!("ReadWritePaths={}\n", writable.join(" ")));
        }
        s.push_str("PrivateTmp=true\n");
        if !user_scope {
            // Kernel/cgroup protections need privileges the user manager
            // doesn't have; they'd make a user unit fail to start.
            s.push_str("ProtectKernelTunables=true\nProtectKernelModules=true\nProtectControlGroups=true\n");
            s.push_str("RestrictSUIDSGID=true\nLockPersonality=true\nCapabilityBoundingSet=\n");
        }
    } else if !user_scope {
        // Even unhardened, a DaemonHall unit never gains privileges.
        s.push_str("NoNewPrivileges=true\n");
    }

    let service_name = format!("{}.service", spec.name);
    let (timer_name, timer_text) = match timer {
        Some((cal, persistent)) => {
            s.push('\n');
            let mut t = String::new();
            t.push_str(MARKER);
            t.push('\n');
            t.push_str(&format!(
                "[Unit]\nDescription={} (schedule)\n\n",
                esc_pct(&description)
            ));
            t.push_str(&format!("[Timer]\nOnCalendar={}\n", esc_pct(&cal)));
            if persistent {
                t.push_str("Persistent=true\n");
            }
            t.push_str(&format!(
                "Unit={service_name}\n\n[Install]\nWantedBy=timers.target\n"
            ));
            (Some(format!("{}.timer", spec.name)), Some(t))
        }
        None => {
            s.push_str(&format!(
                "\n[Install]\nWantedBy={}\n",
                if user_scope {
                    "default.target"
                } else {
                    "multi-user.target"
                }
            ));
            (None, None)
        }
    };
    Ok(Rendered {
        service_name,
        service: s,
        timer_name,
        timer: timer_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpec {
        ServiceSpec {
            scope: Scope::System,
            name: "demo-svc".into(),
            description: "Demo 100% service".into(),
            exec: "/bin/sh".into(),
            args: vec!["-c".into(), "echo hi; sleep 1".into()],
            working_directory: Some("/tmp".into()),
            environment: vec![("RUST_LOG".into(), "info".into())],
            restart: Restart::OnFailure,
            restart_sec: 5,
            hardened: true,
            writable_paths: vec!["/tmp".into()],
            after_network: true,
            timer: None,
            start_now: true,
        }
    }

    #[test]
    fn renders_hardened_system_unit_as_user() {
        let r = render(&spec(), "raven").unwrap();
        assert!(r.service.starts_with(MARKER));
        assert!(r.service.contains("User=raven\nGroup=raven\n"));
        assert!(r.service.contains("Description=Demo 100%% service"));
        assert!(
            r.service
                .contains("ExecStart=/bin/sh -c \"echo hi; sleep 1\"")
        );
        assert!(r.service.contains("ProtectSystem=strict"));
        assert!(r.service.contains("WantedBy=multi-user.target"));
        assert!(r.timer.is_none());
    }

    #[test]
    fn rejects_prefixed_or_relative_exec() {
        for bad in ["+/bin/sh", "-/bin/sh", "!/bin/sh", "sh", "/bin/../bin/sh x"] {
            let mut s = spec();
            s.exec = bad.into();
            assert!(render(&s, "raven").is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_newline_injection() {
        let mut s = spec();
        s.description = "x\nExecStartPre=+/bin/sh".into();
        assert!(render(&s, "raven").is_err());
        let mut s = spec();
        s.args = vec!["a\n[Service]".into()];
        assert!(render(&s, "raven").is_err());
        let mut s = spec();
        s.environment = vec![("A".into(), "b\nUser=root".into())];
        assert!(render(&s, "raven").is_err());
    }

    #[test]
    fn rejects_bad_names() {
        for bad in ["", "Upper", "1abc", "a/b", "a.b", "a b", "../x"] {
            let mut s = spec();
            s.name = bad.into();
            assert!(render(&s, "raven").is_err(), "{bad}");
        }
        assert!(valid_unit_name("wraithflow.service"));
        assert!(valid_unit_name("sigilward-check.timer"));
        assert!(!valid_unit_name("getty@.service"));
        assert!(!valid_unit_name("../x.service"));
        assert!(!valid_unit_name("x.socket"));
    }

    #[test]
    fn timer_makes_oneshot_and_timer_unit() {
        let mut s = spec();
        s.timer = Some(TimerSpec {
            on_calendar: "daily".into(),
            persistent: true,
        });
        let r = render(&s, "raven").unwrap();
        assert!(r.service.contains("Type=oneshot"));
        assert!(!r.service.contains("Restart=on-failure"));
        assert!(!r.service.contains("[Install]"));
        let t = r.timer.unwrap();
        assert!(t.contains("OnCalendar=daily\nPersistent=true\nUnit=demo-svc.service"));
        assert_eq!(r.timer_name.as_deref(), Some("demo-svc.timer"));
    }

    #[test]
    fn user_scope_has_no_user_line_or_kernel_protections() {
        let mut s = spec();
        s.scope = Scope::User;
        let r = render(&s, "raven").unwrap();
        assert!(!r.service.contains("User="));
        assert!(!r.service.contains("ProtectKernelModules"));
        assert!(r.service.contains("WantedBy=default.target"));
    }
}
