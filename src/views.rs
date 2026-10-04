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

pub fn page(token: &str) -> String {
    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<meta name="dh-token" content="{token}">
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
      </div>
    </div>
    <a class="btn-ghost" href="http://127.0.0.1:8761/" target="_blank" rel="noopener" title="Open the shared Cybercore Theme Studio">Theme Studio ↗</a>
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

</body>
</html>"##
    )
}
