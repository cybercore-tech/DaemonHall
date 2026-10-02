//! Activity history and live logs, straight from the journal.
//!
//! History comes from the service manager's own structured messages
//! (started / stopped / failed / auto-restart, keyed by MESSAGE_ID), so it
//! covers everything that happened before DaemonHall was running too, and
//! is merged with DaemonHall's audit log of what was done from the UI.

use axum::response::sse::Event;
use cyberwatch::unit::{Scope, Unit};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    /// Microseconds since the epoch.
    pub ts: u64,
    pub unit: String,
    /// started | stopped | restarted | failed | auto-restart | exited |
    /// finished | action (DaemonHall) | alert (DaemonHall monitor)
    pub kind: String,
    pub message: String,
    /// "systemd" or "daemonhall".
    pub source: String,
    #[serde(default)]
    pub ok: Option<bool>,
}

fn classify(d: &Value) -> Option<(&'static str, String)> {
    let id = d.get("MESSAGE_ID")?.as_str()?;
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let job_type = s("JOB_TYPE");
    let job_result = s("JOB_RESULT");
    Some(match id {
        // Job finished (start/restart/oneshot run).
        "39f53479d3a045ac8e11786248231fbf" => match (job_type.as_str(), job_result.as_str()) {
            (_, r) if !r.is_empty() && r != "done" => ("failed", format!("{job_type} job {r}")),
            ("restart", _) => ("restarted", "restarted".into()),
            ("start", _) if s("MESSAGE").starts_with("Finished") => {
                ("finished", "run finished".into())
            }
            _ => ("started", "started".into()),
        },
        "9d1aaa27d60140bd96365438aad20286" => match job_type.as_str() {
            "restart" => return None, // the restart's "started" half says it
            _ => ("stopped", "stopped".into()),
        },
        "be02cf6855d2428ba40df7e9d022f03d" => ("failed", "failed to start".into()),
        "d9b373ed55a64feb8242e02dbe79a49c" => ("failed", format!("failed ({})", s("UNIT_RESULT"))),
        "5eb03494b6584870a536b337290809b3" => (
            "auto-restart",
            format!("scheduled restart #{}", s("N_RESTARTS")),
        ),
        "98e322203f7a4ed290d09fe03c09fe15" => {
            let code = s("EXIT_CODE");
            let status = s("EXIT_STATUS");
            if code == "exited" && status == "0" {
                return None;
            }
            ("exited", format!("process {code} status={status}"))
        }
        _ => return None,
    })
}

/// systemd's lifecycle events for `units`, newest first.
pub async fn history(units: &[Unit], limit: usize) -> Vec<ActivityEvent> {
    let mut events = Vec::new();
    for scope in [Scope::System, Scope::User] {
        let names: Vec<&Unit> = units.iter().filter(|u| u.scope == scope).collect();
        if names.is_empty() {
            continue;
        }
        let mut cmd = Command::new("journalctl");
        cmd.args([
            "-o",
            "json",
            "--no-pager",
            "-r",
            "-n",
            &(limit * 6).to_string(),
        ]);
        match scope {
            Scope::System => {
                cmd.arg("_PID=1");
                for u in &names {
                    cmd.arg(format!("UNIT={}", u.name));
                }
            }
            Scope::User => {
                cmd.args(["--user", "_SYSTEMD_USER_UNIT=init.scope"]);
                for u in &names {
                    cmd.arg(format!("USER_UNIT={}", u.name));
                }
            }
        }
        let Ok(out) = cmd.output().await else {
            continue;
        };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Ok(d) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some((kind, message)) = classify(&d) else {
                continue;
            };
            let name = d
                .get(if scope == Scope::User {
                    "USER_UNIT"
                } else {
                    "UNIT"
                })
                .and_then(Value::as_str)
                .unwrap_or("");
            let ts = d
                .get("__REALTIME_TIMESTAMP")
                .and_then(Value::as_str)
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let unit = if scope == Scope::User {
                Unit::user(name)
            } else {
                Unit::system(name)
            };
            events.push(ActivityEvent {
                ts,
                unit: unit.id(),
                kind: kind.into(),
                message,
                source: "systemd".into(),
                ok: None,
            });
        }
    }
    events
}

// ── audit log: what was done from DaemonHall ──────────────────────────

pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"))
        .join("daemonhall")
}

fn audit_path() -> PathBuf {
    state_dir().join("activity.jsonl")
}

pub fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

pub fn record(unit: &str, kind: &str, message: &str, ok: bool) {
    let ev = ActivityEvent {
        ts: now_us(),
        unit: unit.into(),
        kind: kind.into(),
        message: message.into(),
        source: "daemonhall".into(),
        ok: Some(ok),
    };
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = audit_path();
    // Keep the log bounded: past 2 MB, keep the newest half.
    if std::fs::metadata(&path)
        .map(|m| m.len() > 2_000_000)
        .unwrap_or(false)
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        let lines: Vec<&str> = text.lines().collect();
        let keep = lines[lines.len() / 2..].join("\n");
        let _ = std::fs::write(&path, keep + "\n");
    }
    if let Ok(line) = serde_json::to_string(&ev) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

pub fn audit(filter: Option<&str>) -> Vec<ActivityEvent> {
    std::fs::read_to_string(audit_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<ActivityEvent>(l).ok())
        .filter(|e| filter.is_none_or(|u| e.unit == u))
        .collect()
}

// ── live logs ──────────────────────────────────────────────────────────

#[derive(Serialize)]
struct LogLine {
    ts: u64,
    prio: u8,
    pid: String,
    ident: String,
    msg: String,
}

/// Drops ANSI colour/cursor sequences: real ESC [ … final-byte, and the
/// literal text `\x1b[…m` / `\u001b[…m` some programs write into the journal.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let seq_start = if b[i] == 0x1b && b.get(i + 1) == Some(&b'[') {
            Some(i + 2)
        } else if s[i..].starts_with("\\x1b[") {
            Some(i + 5)
        } else if s[i..].starts_with("\\u001b[") {
            Some(i + 7)
        } else {
            None
        };
        if let Some(mut j) = seq_start {
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b';' || b[j] == b'?') {
                j += 1;
            }
            if j < b.len() && b[j].is_ascii_alphabetic() {
                i = j + 1;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn log_line(raw: &str) -> Option<String> {
    let d: Value = serde_json::from_str(raw).ok()?;
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    // MESSAGE is an array of bytes when it isn't valid UTF-8.
    let msg = match d.get("MESSAGE") {
        Some(Value::String(m)) => m.clone(),
        Some(Value::Array(bytes)) => String::from_utf8_lossy(
            &bytes
                .iter()
                .filter_map(|b| b.as_u64().map(|v| v as u8))
                .collect::<Vec<_>>(),
        )
        .into_owned(),
        _ => String::new(),
    };
    serde_json::to_string(&LogLine {
        ts: s("__REALTIME_TIMESTAMP").parse().unwrap_or(0),
        prio: s("PRIORITY").parse().unwrap_or(6),
        pid: s("_PID"),
        ident: s("SYSLOG_IDENTIFIER"),
        msg: strip_ansi(&msg),
    })
    .ok()
}

/// Follows a unit's journal: the last `lines` entries, then live. The
/// `journalctl` child is killed when the browser disconnects (the stream,
/// which owns it, is dropped).
pub fn follow(
    unit: &Unit,
    lines: u32,
) -> std::io::Result<impl Stream<Item = Result<Event, std::convert::Infallible>> + use<>> {
    let mut cmd = Command::new("journalctl");
    cmd.args(unit.journal_args())
        .args(["-f", "-o", "json", "--no-pager", "-n", &lines.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout"))?;
    let reader = BufReader::new(stdout).lines();
    Ok(futures_util::stream::unfold(
        (child, reader),
        |(child, mut reader)| async move {
            loop {
                match reader.next_line().await {
                    Ok(Some(raw)) => {
                        if let Some(json) = log_line(&raw) {
                            return Some((Ok(Event::default().data(json)), (child, reader)));
                        }
                    }
                    _ => return None,
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_lifecycle_messages() {
        let started = json!({"MESSAGE_ID":"39f53479d3a045ac8e11786248231fbf","JOB_TYPE":"start","JOB_RESULT":"done","MESSAGE":"Started X"});
        assert_eq!(classify(&started).unwrap().0, "started");
        let finished = json!({"MESSAGE_ID":"39f53479d3a045ac8e11786248231fbf","JOB_TYPE":"start","JOB_RESULT":"done","MESSAGE":"Finished X"});
        assert_eq!(classify(&finished).unwrap().0, "finished");
        let restarted = json!({"MESSAGE_ID":"39f53479d3a045ac8e11786248231fbf","JOB_TYPE":"restart","JOB_RESULT":"done"});
        assert_eq!(classify(&restarted).unwrap().0, "restarted");
        let failed =
            json!({"MESSAGE_ID":"d9b373ed55a64feb8242e02dbe79a49c","UNIT_RESULT":"exit-code"});
        assert_eq!(classify(&failed).unwrap().1, "failed (exit-code)");
        let clean_exit = json!({"MESSAGE_ID":"98e322203f7a4ed290d09fe03c09fe15","EXIT_CODE":"exited","EXIT_STATUS":"0"});
        assert!(classify(&clean_exit).is_none());
        assert!(classify(&json!({"MESSAGE":"plain log"})).is_none());
    }

    #[test]
    fn strips_real_and_literal_ansi() {
        assert_eq!(strip_ansi("\u{1b}[34m[STATS]\u{1b}[0m ok"), "[STATS] ok");
        assert_eq!(
            strip_ansi("INFO \\x1b[34m[STATS]\\x1b[0m x"),
            "INFO [STATS] x"
        );
        assert_eq!(strip_ansi("plain é text"), "plain é text");
    }

    #[test]
    fn log_line_handles_byte_array_messages() {
        let raw = r#"{"__REALTIME_TIMESTAMP":"5","PRIORITY":"3","MESSAGE":[104,105]}"#;
        let v: Value = serde_json::from_str(&log_line(raw).unwrap()).unwrap();
        assert_eq!(v["msg"], "hi");
        assert_eq!(v["prio"], 3);
    }
}
