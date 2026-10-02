//! Reading unit state: one batched `systemctl show` per scope, enriched
//! with resources, timers and cyberwatch's own `needs_attention` verdict.

use cyberwatch::status::{Kind, UnitStatus};
use cyberwatch::unit::{Scope, Unit};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::process::Command;

const PROPS: &str = "Id,Description,LoadState,ActiveState,SubState,UnitFileState,Result,MainPID,\
ActiveEnterTimestamp,InactiveEnterTimestamp,NRestarts,MemoryCurrent,CPUUsageNSec,TasksCurrent,\
FragmentPath,User,Type,Triggers,TriggeredBy,NextElapseUSecRealtime,LastTriggerUSec,ControlGroup,\
ExecMainStatus,Requires,Wants,After,WantedBy,Restart";

#[derive(Clone, Serialize, Default)]
pub struct UnitInfo {
    pub id: String,
    pub name: String,
    pub scope: &'static str,
    pub kind: &'static str,
    pub description: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub unit_file_state: String,
    pub result: String,
    pub status_label: String,
    pub needs_attention: bool,
    pub main_pid: u32,
    /// Unix seconds the unit last became active / inactive.
    pub active_since: Option<u64>,
    pub inactive_since: Option<u64>,
    pub n_restarts: Option<u64>,
    pub memory: Option<u64>,
    pub cpu_nsec: Option<u64>,
    /// Percent of one core since the previous sample.
    pub cpu_pct: Option<f64>,
    pub tasks: Option<u64>,
    pub fragment_path: String,
    pub run_as: String,
    pub service_type: String,
    pub restart_policy: String,
    pub triggers: Vec<String>,
    pub triggered_by: Vec<String>,
    pub next_run: Option<u64>,
    pub last_run: Option<u64>,
    pub exit_status: Option<i64>,
    pub control_group: String,
    pub requires: Vec<String>,
    pub wants: Vec<String>,
    pub after: Vec<String>,
    pub wanted_by: Vec<String>,
    /// DaemonHall may control it (user units always; system units when
    /// hand-installed in /etc/systemd/system — see `allow.rs`).
    pub controllable: bool,
    /// The unit file carries DaemonHall's marker (made by the New form).
    pub made_here: bool,
}

pub fn watched_ids() -> Vec<String> {
    let cfg = cyberwatch::config::load_from(&cyberwatch::config::config_path()).unwrap_or_default();
    cyberwatch::discover::discover_units(&cfg)
}

fn parse_blocks(text: &str) -> Vec<HashMap<String, String>> {
    let mut out = Vec::new();
    let mut cur: HashMap<String, String> = HashMap::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else if let Some((k, v)) = line.split_once('=') {
            cur.insert(k.to_string(), v.to_string());
        }
    }
    out
}

fn show(scope: Scope, names: &[String]) -> Vec<HashMap<String, String>> {
    if names.is_empty() {
        return Vec::new();
    }
    let mut cmd = Command::new("systemctl");
    if scope == Scope::User {
        cmd.arg("--user");
    }
    cmd.args(["show", "--timestamp=unix", "-p", PROPS, "--"])
        .args(names);
    match cmd.output() {
        Ok(out) => parse_blocks(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => Vec::new(),
    }
}

fn num(v: Option<&String>) -> Option<u64> {
    let v = v?.trim();
    let n: u64 = v.parse().ok()?;
    // systemd prints UINT64_MAX for "not available / infinity".
    (n != u64::MAX).then_some(n)
}

fn stamp(v: Option<&String>) -> Option<u64> {
    let v = v?.trim().strip_prefix('@')?;
    let n: u64 = v.parse().ok()?;
    (n > 0).then_some(n)
}

fn list(v: Option<&String>) -> Vec<String> {
    v.map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// `systemctl list-timers` knows the next/last run for every timer, including
/// monotonic ones (OnUnitActiveSec=…) whose NextElapseUSecRealtime is empty.
/// Returns name -> (next, last) in Unix seconds.
fn timer_schedule(scope: Scope) -> HashMap<String, (Option<u64>, Option<u64>)> {
    let mut cmd = Command::new("systemctl");
    if scope == Scope::User {
        cmd.arg("--user");
    }
    cmd.args(["list-timers", "--all", "-o", "json", "--no-pager"]);
    let Ok(out) = cmd.output() else {
        return HashMap::new();
    };
    let Ok(list) = serde_json::from_slice::<Vec<serde_json::Value>>(&out.stdout) else {
        return HashMap::new();
    };
    let secs = |v: &serde_json::Value| v.as_u64().filter(|n| *n > 0).map(|n| n / 1_000_000);
    list.iter()
        .filter_map(|t| {
            let name = t.get("unit")?.as_str()?.to_string();
            Some((name, (secs(&t["next"]), secs(&t["last"]))))
        })
        .collect()
}

/// Everything DaemonHall knows about the watched units, in watch-list order.
pub fn snapshot(ids: &[String], managed_system: &BTreeSet<String>) -> Vec<UnitInfo> {
    let units: Vec<Unit> = ids.iter().map(|i| Unit::parse(i)).collect();
    let mut by_scope: HashMap<Scope, Vec<String>> = HashMap::new();
    for u in &units {
        by_scope.entry(u.scope).or_default().push(u.name.clone());
    }
    let mut fields: HashMap<(Scope, String), HashMap<String, String>> = HashMap::new();
    for (scope, names) in &by_scope {
        for block in show(*scope, names) {
            if let Some(id) = block.get("Id").cloned() {
                fields.insert((*scope, id), block);
            }
        }
    }
    let mut schedules: HashMap<(Scope, String), (Option<u64>, Option<u64>)> = HashMap::new();
    for scope in [Scope::System, Scope::User] {
        if units.iter().any(|u| u.scope == scope && u.is_timer()) {
            for (name, s) in timer_schedule(scope) {
                schedules.insert((scope, name), s);
            }
        }
    }
    let timer_targets: BTreeSet<String> = ids
        .iter()
        .filter(|n| n.ends_with(".timer"))
        .map(|n| n.trim_end_matches(".timer").to_string())
        .collect();

    units
        .iter()
        .map(|u| {
            let empty = HashMap::new();
            let f = fields.get(&(u.scope, u.name.clone())).unwrap_or(&empty);
            let get = |k: &str| f.get(k).cloned().unwrap_or_default();
            let kind = if u.is_timer() {
                Kind::Timer
            } else {
                Kind::Service
            };
            let load = if f.is_empty() {
                "not-found".to_string()
            } else {
                get("LoadState")
            };
            let status = UnitStatus {
                name: u.id(),
                kind,
                load_state: load.clone(),
                active_state: get("ActiveState"),
                sub_state: get("SubState"),
                unit_file_state: get("UnitFileState"),
                result: get("Result"),
                has_timer: timer_targets.contains(u.id().trim_end_matches(".service")),
            };
            let fragment = get("FragmentPath");
            let made_here = std::fs::read_to_string(&fragment)
                .map(|t| t.starts_with(daemonhall::unitgen::MARKER))
                .unwrap_or(false);
            UnitInfo {
                id: u.id(),
                name: u.name.clone(),
                scope: if u.is_user() { "user" } else { "system" },
                kind: if u.is_timer() { "timer" } else { "service" },
                description: get("Description"),
                load_state: load,
                active_state: status.active_state.clone(),
                sub_state: status.sub_state.clone(),
                unit_file_state: status.unit_file_state.clone(),
                result: status.result.clone(),
                status_label: status.status_label().to_string(),
                needs_attention: status.needs_attention(),
                main_pid: num(f.get("MainPID")).unwrap_or(0) as u32,
                active_since: stamp(f.get("ActiveEnterTimestamp")),
                inactive_since: stamp(f.get("InactiveEnterTimestamp")),
                n_restarts: num(f.get("NRestarts")),
                memory: num(f.get("MemoryCurrent")),
                cpu_nsec: num(f.get("CPUUsageNSec")),
                cpu_pct: None,
                tasks: num(f.get("TasksCurrent")),
                fragment_path: fragment,
                run_as: get("User"),
                service_type: get("Type"),
                restart_policy: get("Restart"),
                triggers: list(f.get("Triggers")),
                triggered_by: list(f.get("TriggeredBy")),
                next_run: schedules
                    .get(&(u.scope, u.name.clone()))
                    .and_then(|s| s.0)
                    .or_else(|| stamp(f.get("NextElapseUSecRealtime"))),
                last_run: schedules
                    .get(&(u.scope, u.name.clone()))
                    .and_then(|s| s.1)
                    .or_else(|| stamp(f.get("LastTriggerUSec"))),
                exit_status: f.get("ExecMainStatus").and_then(|v| v.parse().ok()),
                control_group: get("ControlGroup"),
                requires: list(f.get("Requires")),
                wants: list(f.get("Wants")),
                after: list(f.get("After")),
                wanted_by: list(f.get("WantedBy")),
                controllable: u.is_user() || managed_system.contains(&u.name),
                made_here,
            }
        })
        .collect()
}

/// Listening TCP/UDP ports per PID — only for processes this user can see
/// (`ss -p` hides other users' processes without root).
pub fn listening_ports() -> HashMap<u32, Vec<String>> {
    let mut map: HashMap<u32, Vec<String>> = HashMap::new();
    let Ok(out) = Command::new("ss").args(["-ltnupH"]).output() else {
        return map;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        let proto = cols[0];
        let local = cols[4];
        let rest = cols[5..].join(" ");
        let mut s = rest.as_str();
        while let Some(i) = s.find("pid=") {
            let tail = &s[i + 4..];
            let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(pid) = digits.parse() {
                let entry = map.entry(pid).or_default();
                let label = format!("{proto} {local}");
                if !entry.contains(&label) {
                    entry.push(label);
                }
            }
            s = &tail[digits.len()..];
        }
    }
    map
}

/// PIDs in a unit's cgroup (cgroup.procs is world-readable).
pub fn cgroup_pids(control_group: &str) -> Vec<u32> {
    if control_group.is_empty() || control_group.contains("..") {
        return Vec::new();
    }
    std::fs::read_to_string(format!("/sys/fs/cgroup{control_group}/cgroup.procs"))
        .map(|t| t.lines().filter_map(|l| l.trim().parse().ok()).collect())
        .unwrap_or_default()
}

#[derive(Serialize)]
pub struct UnitFile {
    pub id: String,
    pub scope: &'static str,
    pub state: String,
    pub watched: bool,
}

/// Every `.service`/`.timer` unit file both managers know, for the
/// "add to watch list" picker.
pub fn all_unit_files(watched: &BTreeSet<String>) -> Vec<UnitFile> {
    let mut out = Vec::new();
    for (scope, flag) in [(Scope::System, None), (Scope::User, Some("--user"))] {
        let mut cmd = Command::new("systemctl");
        if let Some(f) = flag {
            cmd.arg(f);
        }
        cmd.args([
            "list-unit-files",
            "--type=service,timer",
            "--no-legend",
            "--no-pager",
        ]);
        let Ok(o) = cmd.output() else { continue };
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let mut cols = line.split_whitespace();
            let (Some(name), Some(state)) = (cols.next(), cols.next()) else {
                continue;
            };
            if name.contains('@') {
                continue; // templates
            }
            let id = match scope {
                Scope::System => name.to_string(),
                Scope::User => format!("{}{name}", cyberwatch::unit::USER_PREFIX),
            };
            out.push(UnitFile {
                watched: watched.contains(&id),
                id,
                scope: if scope == Scope::User {
                    "user"
                } else {
                    "system"
                },
                state: state.to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_show_blocks_and_values() {
        let text = "Id=a.service\nMemoryCurrent=18446744073709551615\nActiveEnterTimestamp=@1790823080\n\nId=b.timer\nTriggers=b.service c.service\n";
        let b = parse_blocks(text);
        assert_eq!(b.len(), 2);
        assert_eq!(num(b[0].get("MemoryCurrent")), None);
        assert_eq!(stamp(b[0].get("ActiveEnterTimestamp")), Some(1790823080));
        assert_eq!(list(b[1].get("Triggers")), vec!["b.service", "c.service"]);
        assert_eq!(stamp(Some(&"@0".to_string())), None);
    }
}
