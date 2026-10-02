//! Background loop: keeps a fresh snapshot for the API (every 2 s, or right
//! after an action), computes CPU %, and raises desktop notifications when
//! a watched unit fails, recovers, or flaps (restarts repeatedly).

use crate::journal;
use crate::systemd::{self, UnitInfo};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Notify, RwLock};

const TICK: Duration = Duration::from_secs(2);
const FLAP_WINDOW: Duration = Duration::from_secs(600);
const FLAP_RESTARTS: u64 = 3;

#[derive(Clone, Serialize, Default)]
pub struct Snapshot {
    /// Unix seconds.
    pub generated: u64,
    pub units: Vec<UnitInfo>,
    pub helper_installed: bool,
    pub rule_installed: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub notify_failures: bool,
    pub notify_recoveries: bool,
    pub notify_flapping: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            notify_failures: true,
            notify_recoveries: true,
            notify_flapping: true,
        }
    }
}

fn settings_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_default()
        .join("daemonhall/settings.json")
}

pub fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_settings(s: &Settings) -> Result<(), String> {
    let p = settings_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, serde_json::to_string_pretty(s).unwrap_or_default())
        .map_err(|e| e.to_string())
}

pub struct Monitor {
    pub snapshot: RwLock<Snapshot>,
    pub wake: Notify,
    pub settings: RwLock<Settings>,
}

impl Monitor {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            snapshot: RwLock::new(Snapshot::default()),
            wake: Notify::new(),
            settings: RwLock::new(load_settings()),
        })
    }

    /// Refresh now (after an action) instead of on the next tick.
    pub fn poke(&self) {
        self.wake.notify_one();
    }
}

struct Seen {
    active_state: String,
    needs_attention: bool,
    n_restarts: Option<u64>,
    cpu: Option<(u64, Instant)>,
    restarts: VecDeque<(Instant, u64)>,
    flap_alerted: Option<Instant>,
}

fn notify(title: &str, body: &str, critical: bool) {
    let _ = std::process::Command::new("notify-send")
        .args([
            "-a",
            "DaemonHall",
            "-u",
            if critical { "critical" } else { "normal" },
        ])
        .args([
            "-i",
            if critical {
                "dialog-warning"
            } else {
                "dialog-information"
            },
        ])
        .arg(title)
        .arg(body)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn collect() -> Snapshot {
    let ids = systemd::watched_ids();
    let home = dirs::home_dir().unwrap_or_default();
    let managed: BTreeSet<String> = daemonhall::allow::managed_system_units(&home)
        .into_iter()
        .collect();
    Snapshot {
        generated: journal::now_us() / 1_000_000,
        units: systemd::snapshot(&ids, &managed),
        helper_installed: crate::ops::helper_installed(),
        rule_installed: crate::ops::rule_installed(),
    }
}

pub async fn run(mon: Arc<Monitor>) {
    let mut seen: HashMap<String, Seen> = HashMap::new();
    let mut first = true;
    loop {
        let mut snap = tokio::task::spawn_blocking(collect)
            .await
            .unwrap_or_default();
        let settings = mon.settings.read().await.clone();
        let now = Instant::now();

        for u in &mut snap.units {
            let prev = seen.remove(&u.id);
            // CPU % of one core since the last sample.
            if let (Some(cpu), Some(Some((pcpu, pt)))) = (u.cpu_nsec, prev.as_ref().map(|p| p.cpu))
            {
                let dt = now.duration_since(pt).as_nanos() as f64;
                if dt > 0.0 && cpu >= pcpu {
                    u.cpu_pct = Some(((cpu - pcpu) as f64 / dt * 1000.0).round() / 10.0);
                }
            }
            let mut entry = Seen {
                active_state: u.active_state.clone(),
                needs_attention: u.needs_attention,
                n_restarts: u.n_restarts,
                cpu: u.cpu_nsec.map(|c| (c, now)),
                restarts: prev
                    .as_ref()
                    .map(|p| p.restarts.clone())
                    .unwrap_or_default(),
                flap_alerted: prev.as_ref().and_then(|p| p.flap_alerted),
            };
            if let (Some(p), false) = (&prev, first) {
                let label = &u.id;
                if u.active_state == "failed" && p.active_state != "failed" {
                    let msg = format!(
                        "{label} failed ({})",
                        if u.result.is_empty() { "?" } else { &u.result }
                    );
                    journal::record(label, "alert", &msg, false);
                    if settings.notify_failures {
                        notify("Service failed", &msg, true);
                    }
                } else if u.needs_attention && !p.needs_attention {
                    let msg = format!("{label} needs attention ({})", u.status_label);
                    journal::record(label, "alert", &msg, false);
                    if settings.notify_failures {
                        notify("Service needs attention", &msg, true);
                    }
                } else if p.active_state == "failed" && u.active_state == "active" {
                    let msg = format!("{label} recovered");
                    journal::record(label, "alert", &msg, true);
                    if settings.notify_recoveries {
                        notify("Service recovered", &msg, false);
                    }
                }
                if let (Some(n), Some(pn)) = (u.n_restarts, p.n_restarts)
                    && n > pn
                {
                    entry.restarts.push_back((now, n - pn));
                }
                while entry
                    .restarts
                    .front()
                    .is_some_and(|(t, _)| now.duration_since(*t) > FLAP_WINDOW)
                {
                    entry.restarts.pop_front();
                }
                let recent: u64 = entry.restarts.iter().map(|(_, d)| d).sum();
                let quiet = entry
                    .flap_alerted
                    .is_none_or(|t| now.duration_since(t) > FLAP_WINDOW);
                if recent >= FLAP_RESTARTS && quiet {
                    entry.flap_alerted = Some(now);
                    let msg = format!("{label} restarted {recent}× in 10 min");
                    journal::record(label, "alert", &format!("flapping: {msg}"), false);
                    if settings.notify_flapping {
                        notify("Service flapping", &msg, true);
                    }
                }
            }
            seen.insert(u.id.clone(), entry);
        }
        first = false;
        *mon.snapshot.write().await = snap;

        tokio::select! {
            _ = tokio::time::sleep(TICK) => {}
            _ = mon.wake.notified() => {}
        }
    }
}
