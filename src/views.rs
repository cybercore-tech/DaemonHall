//! The page shell. Plain string-built HTML (no template engine), like
//! cyberdeck-hub's `views.rs`; everything after first paint is app.js
//! talking to the JSON API. Static assets are compiled into the binary.

pub const UI_CSS: &str = include_str!("../static/ui.css");
pub const APP_JS: &str = include_str!("../static/app.js");
pub const FAVICON: &str = include_str!("../static/favicon.svg");
pub const DAEMON_SVG: &str = include_str!("../static/daemon.svg");
pub const FONT_REGULAR: &[u8] = include_bytes!("../static/fonts/jbm-nerd-regular.woff2");
pub const FONT_BOLD: &[u8] = include_bytes!("../static/fonts/jbm-nerd-bold.woff2");
pub const FONT_ITALIC: &[u8] = include_bytes!("../static/fonts/jbm-nerd-italic.woff2");

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Theme picker options, grouped by CYBERGRID family — the same dropdown
/// as cyberdeck-hub (a custom one: Chromium forces system-blue on native
/// <option> highlights).
fn theme_options() -> (String, String) {
    let schema = cybercore::schema::load();
    let active = schema.active.clone();
    let options = crate::cybergrid::theme_families()
        .iter()
        .filter_map(|(family, slugs)| {
            let items: String = slugs
                .iter()
                .filter(|slug| schema.theme(slug).is_some())
                .map(|slug| {
                    let sel = if **slug == active { " selected" } else { "" };
                    let slug = escape(slug);
                    format!(r#"<div class="theme-item{sel}" data-value="{slug}" role="option">{slug}</div>"#)
                })
                .collect();
            (!items.is_empty()).then(|| {
                format!(r#"<div class="theme-group-label">{}</div>{items}"#, escape(family))
            })
        })
        .collect();
    (active, options)
}

pub fn page(token: &str) -> String {
    let (active, options) = theme_options();
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta name="dh-token" content="{token}">
<meta name="dh-default-theme" content="{active}">
<title>DAEMONHALL</title>
<link rel="icon" type="image/svg+xml" href="/static/favicon.svg">
<link rel="stylesheet" href="/vendor/tokens.css">
<style id="theme-vars"></style>
<link rel="stylesheet" href="/static/ui.css">
<script src="/static/app.js" defer></script>
</head>
<body>
<div class="bg-scanlines" aria-hidden="true"></div>
<div class="bg-glow" aria-hidden="true"></div>
<div class="bg-sigil" aria-hidden="true"></div>
<header class="topbar">
  <a class="brand" href="#/">
    <img src="/static/daemon.svg" alt="" class="brand-daemon" width="34" height="34">
    <span>DAEMON<span class="brand-accent">HALL</span></span>
    <span class="brand-sub" id="brand-host"></span>
  </a>
  <div class="topbar-right">
    <nav class="topbar-nav" aria-label="Sections">
      <a href="#/" class="btn-ghost" data-nav="hall">Hall</a>
      <a href="#/activity" class="btn-ghost" data-nav="activity">Activity</a>
      <a href="#/timers" class="btn-ghost" data-nav="timers">Timers</a>
      <a href="#/manage" class="btn-ghost" data-nav="manage">Manage</a>
    </nav>
    <div class="theme-dropdown-wrap">
      <button type="button" class="btn-ghost" id="theme-picker-btn" aria-haspopup="listbox" aria-expanded="false">
        <span id="theme-picker-label">THEME</span> <span class="caret-down">&#9662;</span>
      </button>
      <div id="theme-dropdown" class="theme-dropdown" role="listbox" hidden>
        {options}
        <div id="custom-theme-group" hidden></div>
      </div>
    </div>
    <button type="button" class="btn-ghost" id="theme-creator-btn" title="Build a custom theme (saved to this browser only)">+ Theme</button>
  </div>
</header>
<div id="banner" class="banner" hidden></div>
<main id="view" tabindex="-1"><div class="loading">summoning daemons&hellip;</div></main>
<div id="toasts" class="toasts" aria-live="polite"></div>

<div id="modal" class="viewer-overlay" hidden>
  <div class="window viewer-window" role="dialog" aria-modal="true" aria-labelledby="modal-title">
    <div class="titlebar">
      <span class="dot dot-a"></span><span class="dot dot-b"></span><span class="dot dot-c"></span>
      <span class="titlebar-text" id="modal-title"></span>
      <button type="button" class="win-close" data-close-modal title="Close">&#10005;</button>
    </div>
    <div class="viewer-body" id="modal-body"></div>
  </div>
</div>

<div id="theme-creator-overlay" class="viewer-overlay" hidden>
  <div class="window viewer-window ct-window" role="dialog" aria-modal="true">
    <div class="titlebar">
      <span class="dot dot-a"></span><span class="dot dot-b"></span><span class="dot dot-c"></span>
      <span class="titlebar-text">custom theme (this browser only)</span>
      <button type="button" class="win-close" data-close-creator title="Close">&#10005;</button>
    </div>
    <div class="viewer-body">
      <div class="ct-layout">
        <div class="ct-controls">
          <p class="muted" style="margin-top:0;">Not added to the shared CYBERGRID palette (that's compiled in) &mdash; a local custom theme saved in this browser only.</p>
          <label class="field-label" for="ct-name">Name</label>
          <input id="ct-name" class="text-input" placeholder="my-custom-theme">
          <div class="row-actions">
            <button type="button" id="ct-import">Import from file&hellip;</button>
            <button type="button" id="ct-export">Export to file</button>
            <button type="button" id="ct-reset">Reset to active</button>
            <input type="file" id="ct-file-input" accept=".json,application/json" hidden>
          </div>
          <p class="muted small">Import/export use the cybercore theme file shape (schema/themes/&lt;family&gt;/&lt;slug&gt;.json).</p>
          <div id="ct-swatches" class="ct-grid"></div>
        </div>
        <div class="ct-preview">
          <p class="field-label">Live preview</p>
          <div class="unit-card state-up" style="margin-bottom:0.9rem;">
            <div class="uc-head"><span class="uc-dot"></span><span class="uc-name">sample.service</span><span class="badge b-up">running</span></div>
            <p class="uc-desc">A unit card looks like this under your theme.</p>
            <div class="uc-stats"><span>CPU <b>0.4%</b></span><span>MEM <b>12 MB</b></span><span>UP <b>3h</b></span></div>
          </div>
          <div class="chips"><span class="chip c-cyan">cyan</span><span class="chip c-purple">purple</span><span class="chip c-orange">orange</span><span class="chip c-red">red</span></div>
        </div>
      </div>
      <div class="viewer-actions">
        <button type="button" id="ct-save">Save</button>
        <button type="button" data-close-creator>Close</button>
      </div>
    </div>
  </div>
</div>
</body>
</html>"##
    )
}
