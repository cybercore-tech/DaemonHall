//! HTTP routes and the request guard.
//!
//! DaemonHall can stop system services, so a page on some other site open
//! in the same browser must not be able to drive it. Every request must
//! carry `Host: 127.0.0.1:<port>` or `localhost:<port>` (blocks DNS
//! rebinding); every state-changing request must also carry the per-run
//! token from the page's <meta> tag in `X-DaemonHall-Token` and, when the
//! browser sends one, a matching `Origin`. Another origin can't read the
//! token (no CORS headers are ever sent), so it can't forge either.

use crate::journal::{self, ActivityEvent};
use crate::monitor::{Monitor, Settings};
use crate::{cybergrid, ops, systemd, views};
use axum::extract::{Path, Query, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cyberwatch::unit::{Scope, Unit};
use daemonhall::unitgen::ServiceSpec;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct AppState {
    pub token: String,
    pub port: u16,
    pub monitor: Arc<Monitor>,
}

type Shared = Arc<AppState>;

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/:file", get(asset))
        .route("/static/fonts/:file", get(font))
        .route("/vendor/tokens.css", get(tokens_css))
        .route("/api/cybergrid/themes", get(cybergrid::list_themes))
        .route("/api/cybergrid/css/:name", get(cybergrid::theme_css))
        .route("/api/cybergrid/active/:id", post(cybergrid::select_theme))
        .route(
            "/api/cybergrid/appearance/:mode",
            post(cybergrid::select_appearance),
        )
        .route("/api/meta", get(meta))
        .route("/api/units", get(units))
        .route("/api/unit/:id", get(unit_detail))
        .route("/api/unit/:id/action/:verb", post(unit_action))
        .route("/api/unit/:id/delete", post(unit_delete))
        .route("/api/unit/:id/file", post(unit_file_save))
        .route("/api/unit/:id/edit-in-terminal", post(unit_edit_terminal))
        .route("/api/activity", get(activity))
        .route("/api/logs", get(logs))
        .route("/api/unit-files", get(unit_files))
        .route("/api/watch", post(watch))
        .route("/api/services", post(create_service))
        .route("/api/services/preview", post(preview_service))
        .route("/api/daemon-reload", post(daemon_reload))
        .route("/api/settings", get(get_settings).post(set_settings))
        .route("/api/polkit-sync", post(polkit_sync))
        .layer(middleware::from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

async fn guard(State(s): State<Shared>, req: Request, next: Next) -> Response {
    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| {
            h == format!("127.0.0.1:{}", s.port) || h == format!("localhost:{}", s.port)
        });
    if !host_ok {
        return (StatusCode::MISDIRECTED_REQUEST, "unexpected Host header").into_response();
    }
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        let h = req.headers();
        let origin_ok = match h.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
            None => true,
            Some(o) => {
                o == format!("http://127.0.0.1:{}", s.port)
                    || o == format!("http://localhost:{}", s.port)
            }
        };
        let token_ok = h
            .get("x-daemonhall-token")
            .and_then(|t| t.to_str().ok())
            .is_some_and(|t| constant_time_eq(t.as_bytes(), s.token.as_bytes()));
        if !origin_ok || !token_ok {
            return (StatusCode::FORBIDDEN, "missing or bad DaemonHall token").into_response();
        }
    }
    let mut resp = next.run(req).await;
    let hs = resp.headers_mut();
    hs.insert("x-frame-options", "DENY".parse().unwrap());
    hs.insert("x-content-type-options", "nosniff".parse().unwrap());
    hs.insert("referrer-policy", "no-referrer".parse().unwrap());
    resp
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ── pages and assets ───────────────────────────────────────────────────

async fn index(State(s): State<Shared>) -> Html<String> {
    Html(views::page(&s.token))
}

async fn asset(Path(file): Path<String>) -> Response {
    let (body, ctype): (&'static [u8], &str) = match file.as_str() {
        "ui.css" => (views::UI_CSS.as_bytes(), "text/css; charset=utf-8"),
        "app.js" => (views::APP_JS.as_bytes(), "text/javascript; charset=utf-8"),
        "favicon.svg" => (views::FAVICON.as_bytes(), "image/svg+xml"),
        "daemon.svg" => (views::DAEMON_SVG.as_bytes(), "image/svg+xml"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, ctype),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn font(Path(file): Path<String>) -> Response {
    let body: &'static [u8] = match file.as_str() {
        "jbm-nerd-regular.woff2" => views::FONT_REGULAR,
        "jbm-nerd-bold.woff2" => views::FONT_BOLD,
        "jbm-nerd-italic.woff2" => views::FONT_ITALIC,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "max-age=86400"),
        ],
        body,
    )
        .into_response()
}

async fn tokens_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, cybercore::tokens::CSS_CONTENT_TYPE)],
        cybercore::tokens::CSS,
    )
}

// ── read API ───────────────────────────────────────────────────────────

async fn meta(State(s): State<Shared>) -> Json<Value> {
    let snap = s.monitor.snapshot.read().await;
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "port": s.port,
        "user": std::env::var("USER").unwrap_or_default(),
        "host": std::fs::read_to_string("/etc/hostname").unwrap_or_default().trim(),
        "helper_installed": snap.helper_installed,
        "rule_installed": snap.rule_installed,
        "config": cyberwatch::config::config_path(),
    }))
}

async fn units(State(s): State<Shared>) -> Json<Value> {
    let snap = s.monitor.snapshot.read().await.clone();
    Json(serde_json::to_value(snap).unwrap_or_default())
}

fn find(snap: &crate::monitor::Snapshot, id: &str) -> Option<systemd::UnitInfo> {
    snap.units.iter().find(|u| u.id == id).cloned()
}

async fn unit_detail(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let snap = s.monitor.snapshot.read().await.clone();
    let Some(info) = find(&snap, &id) else {
        return (StatusCode::NOT_FOUND, "not a watched unit").into_response();
    };
    let detail = tokio::task::spawn_blocking(move || {
        let file = ops::read_unit_file(&info.fragment_path);
        let ports_by_pid = systemd::listening_ports();
        let mut ports: BTreeSet<String> = BTreeSet::new();
        for pid in systemd::cgroup_pids(&info.control_group) {
            if let Some(p) = ports_by_pid.get(&pid) {
                ports.extend(p.iter().cloned());
            }
        }
        json!({ "unit": info, "file": file, "ports": ports })
    })
    .await
    .unwrap_or_default();
    Json(detail).into_response()
}

#[derive(Deserialize)]
struct ActivityQuery {
    unit: Option<String>,
    limit: Option<usize>,
}

async fn activity(
    State(s): State<Shared>,
    Query(q): Query<ActivityQuery>,
) -> Json<Vec<ActivityEvent>> {
    let limit = q.limit.unwrap_or(200).clamp(10, 1000);
    let ids: Vec<String> = match &q.unit {
        Some(u) => vec![u.clone()],
        None => s
            .monitor
            .snapshot
            .read()
            .await
            .units
            .iter()
            .map(|u| u.id.clone())
            .collect(),
    };
    let units: Vec<Unit> = ids.iter().map(|i| Unit::parse(i)).collect();
    let mut events = journal::history(&units, limit).await;
    events.extend(journal::audit(q.unit.as_deref()));
    events.sort_by_key(|e| std::cmp::Reverse(e.ts));
    events.truncate(limit);
    Json(events)
}

#[derive(Deserialize)]
struct LogsQuery {
    unit: String,
    lines: Option<u32>,
}

async fn logs(State(s): State<Shared>, Query(q): Query<LogsQuery>) -> Response {
    let watched = s
        .monitor
        .snapshot
        .read()
        .await
        .units
        .iter()
        .any(|u| u.id == q.unit);
    if !watched {
        return (StatusCode::NOT_FOUND, "not a watched unit").into_response();
    }
    match journal::follow(&Unit::parse(&q.unit), q.lines.unwrap_or(200).min(2000)) {
        Ok(stream) => Sse::new(stream)
            .keep_alive(KeepAlive::default())
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn unit_files(State(s): State<Shared>) -> Json<Value> {
    let watched: BTreeSet<String> = s
        .monitor
        .snapshot
        .read()
        .await
        .units
        .iter()
        .map(|u| u.id.clone())
        .collect();
    let list = tokio::task::spawn_blocking(move || systemd::all_unit_files(&watched))
        .await
        .unwrap_or_default();
    Json(serde_json::to_value(list).unwrap_or_default())
}

async fn get_settings(State(s): State<Shared>) -> Json<Settings> {
    Json(s.monitor.settings.read().await.clone())
}

// ── write API ──────────────────────────────────────────────────────────

fn outcome(s: &Shared, unit: &str, kind: &str, r: ops::OpResult) -> Response {
    s.monitor.poke();
    match r {
        Ok(m) => {
            journal::record(unit, kind, &m, true);
            Json(json!({ "ok": true, "message": m })).into_response()
        }
        Err(e) => {
            journal::record(unit, kind, &e, false);
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "ok": false, "message": e })),
            )
                .into_response()
        }
    }
}

async fn blocking<F: FnOnce() -> ops::OpResult + Send + 'static>(f: F) -> ops::OpResult {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(e.to_string()))
}

async fn unit_action(
    State(s): State<Shared>,
    Path((id, verb)): Path<(String, String)>,
) -> Response {
    let snap = s.monitor.snapshot.read().await.clone();
    let Some(info) = find(&snap, &id) else {
        return (StatusCode::NOT_FOUND, "not a watched unit").into_response();
    };
    let unit = Unit::parse(&id);
    let r = match verb.as_str() {
        "start" | "stop" | "restart" | "reload" => {
            blocking(move || ops::control(&unit, &verb)).await
        }
        "enable" => blocking(move || ops::set_enabled(&unit, true)).await,
        "disable" => blocking(move || ops::set_enabled(&unit, false)).await,
        "run-now" => blocking(move || ops::run_now(&unit, &info.triggers)).await,
        _ => Err(format!("unknown action {verb}")),
    };
    outcome(&s, &id, "action", r)
}

async fn unit_delete(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let unit = Unit::parse(&id);
    let r = blocking(move || ops::delete(&unit)).await;
    outcome(&s, &id, "delete", r)
}

#[derive(Deserialize)]
struct FileBody {
    text: String,
}

async fn unit_file_save(
    State(s): State<Shared>,
    Path(id): Path<String>,
    Json(b): Json<FileBody>,
) -> Response {
    let unit = Unit::parse(&id);
    let r = blocking(move || ops::save_user_unit_file(&unit, &b.text)).await;
    outcome(&s, &id, "edit", r)
}

async fn unit_edit_terminal(State(s): State<Shared>, Path(id): Path<String>) -> Response {
    let snap = s.monitor.snapshot.read().await.clone();
    let Some(info) = find(&snap, &id) else {
        return (StatusCode::NOT_FOUND, "not a watched unit").into_response();
    };
    let unit = Unit::parse(&id);
    let r = blocking(move || ops::edit_in_terminal(&unit, &info.fragment_path)).await;
    outcome(&s, &id, "edit", r)
}

#[derive(Deserialize)]
struct WatchBody {
    unit: String,
    add: bool,
}

async fn watch(State(s): State<Shared>, Json(b): Json<WatchBody>) -> Response {
    let id = b.unit.clone();
    let r = blocking(move || ops::watch(&b.unit, b.add)).await;
    outcome(&s, &id, "watch", r)
}

async fn create_service(State(s): State<Shared>, Json(spec): Json<ServiceSpec>) -> Response {
    let label = match spec.scope {
        daemonhall::unitgen::Scope::User => format!("user:{}.service", spec.name),
        daemonhall::unitgen::Scope::System => format!("{}.service", spec.name),
    };
    let r = blocking(move || ops::create(&spec)).await;
    outcome(&s, &label, "create", r)
}

/// Renders (and validates) a New-service form without installing anything.
async fn preview_service(Json(spec): Json<ServiceSpec>) -> Json<Value> {
    let user = std::env::var("USER").unwrap_or_default();
    match daemonhall::unitgen::render(&spec, &user) {
        Ok(r) => Json(json!({
            "ok": true,
            "service_name": r.service_name,
            "service": r.service,
            "timer_name": r.timer_name,
            "timer": r.timer,
        })),
        Err(e) => Json(json!({ "ok": false, "message": e })),
    }
}

#[derive(Deserialize)]
struct ReloadBody {
    scope: String,
}

async fn daemon_reload(State(s): State<Shared>, Json(b): Json<ReloadBody>) -> Response {
    let scope = if b.scope == "user" {
        Scope::User
    } else {
        Scope::System
    };
    let r = blocking(move || ops::daemon_reload(scope)).await;
    outcome(&s, &format!("{}-manager", b.scope), "reload", r)
}

async fn polkit_sync(State(s): State<Shared>) -> Response {
    let r = blocking(|| ops::helper(&["sync"], None)).await;
    outcome(&s, "polkit", "sync", r)
}

async fn set_settings(State(s): State<Shared>, Json(new): Json<Settings>) -> Response {
    match crate::monitor::save_settings(&new) {
        Ok(()) => {
            *s.monitor.settings.write().await = new;
            Json(json!({ "ok": true, "message": "settings saved" })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "message": e })),
        )
            .into_response(),
    }
}
