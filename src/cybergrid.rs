//! CYBERGRID palette bridge — the same module as cyberdeck-hub's and
//! dockspace's `cybergrid.rs`. Exposes the shared `cybercore` crate's 72
//! named colour themes over HTTP so the picker can list and apply them.
//!
//! - `GET /api/cybergrid/themes`      — every theme + its 11 colour roles
//! - `GET /api/cybergrid/css/:name`   — a ready `:root{ --bg:#… }` block

use axum::{Json, extract::Path, http::header, response::IntoResponse};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// `family name -> slugs`, read straight off cybercore's real
/// `schema/themes/<family>/<slug>.json` folder layout (path dep, so this
/// is a stable relative path at compile time via `CARGO_MANIFEST_DIR`).
/// Not derivable from the slug alone: most families share a `<family>-
/// <word>` naming convention, but the `default` family is a grab-bag of
/// well-known theme names (`dracula`, `nord`, ...) plus a curated
/// re-export of a few themes that live under other families too — a
/// slug-prefix heuristic would misfile or duplicate those.
pub fn theme_families() -> BTreeMap<String, Vec<String>> {
    let mut families = BTreeMap::new();
    let Some(rd) = themes_dir().and_then(|d| std::fs::read_dir(d).ok()) else {
        // No schema folder on this machine: still offer every embedded
        // theme, as one group, rather than an empty menu.
        let schema = cybercore::schema::load();
        families.insert(
            "all".to_string(),
            schema.theme_names().map(str::to_string).collect(),
        );
        return families;
    };
    for family_entry in rd.filter_map(|e| e.ok()) {
        if !family_entry.path().is_dir() {
            continue;
        }
        let family = family_entry.file_name().to_string_lossy().into_owned();
        let mut slugs: Vec<String> = std::fs::read_dir(family_entry.path())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        name.strip_suffix(".json").map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default();
        slugs.sort();
        families.insert(family, slugs);
    }
    families
}

/// cybercore's `schema/themes` folder: next to this checkout in
/// `~/.sysops`, in the Devspace layout (`core/DaemonHall` +
/// `framework/cybercore`), then the `~/.sysops/cybercore` link.
fn themes_dir() -> Option<std::path::PathBuf> {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut candidates = vec![
        manifest.join("../cybercore/schema/themes"),
        manifest.join("../../framework/cybercore/schema/themes"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(std::path::Path::new(&home).join(".sysops/cybercore/schema/themes"));
    }
    candidates.into_iter().find(|d| d.is_dir())
}

fn role_map(name: &str, p: &cybercore::schema::Palette) -> Value {
    json!({
        "name": name,
        "bg": p.bg,
        "white": p.white,
        "acid_green": p.acid_green,
        "hot_pink": p.hot_pink,
        "purple": p.purple,
        "cyan": p.cyan,
        "orange": p.orange,
        "red": p.red,
        "panel": p.panel,
        "line": p.line,
        "muted": p.muted,
    })
}

pub async fn list_themes() -> impl IntoResponse {
    let s = cybercore::schema::load();
    let themes: Vec<Value> = s
        .theme_names()
        .filter_map(|n| s.theme(n).map(|p| role_map(n, p)))
        .collect();
    Json(json!({ "active": s.active, "themes": themes }))
}

pub async fn theme_css(Path(name): Path<String>) -> impl IntoResponse {
    let s = cybercore::schema::load();
    let p = s.theme(&name).unwrap_or_else(|| s.active_theme());
    let css = format!(
        ":root{{\
--bg:#{bg};--fg:#{white};\
--acid:#{acid};--pink:#{pink};--purple:#{purple};--cyan:#{cyan};\
--orange:#{orange};--red:#{red};\
--panel:#{panel};--line:#{line};--muted:#{muted};\
}}\n",
        bg = p.bg,
        white = p.white,
        acid = p.acid_green,
        pink = p.hot_pink,
        purple = p.purple,
        cyan = p.cyan,
        orange = p.orange,
        red = p.red,
        panel = p.panel,
        line = p.line,
        muted = p.muted,
    );
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], css)
}
