// DaemonHall front end. Hash-routed views over the JSON API:
//   #/              the Hall: every watched unit as a card
//   #/unit/<id>     one unit: overview, live logs, activity, unit file
//   #/activity      timeline across all units
//   #/timers        schedules
//   #/manage        watch list, new service, maintenance, notifications
// The DOM is built with h() (textContent, never innerHTML with data), so
// unit names, descriptions and log lines can't inject markup.
"use strict";

const TOKEN = document.querySelector('meta[name="dh-token"]').content;
const POLL_MS = 3000;

const view = document.getElementById("view");
let snap = null;           // last /api/units
let meta = null;           // /api/meta
let pollTimer = null;
let currentRoute = null;
let renderCurrent = null;  // re-render hook for the active view on new data
let teardown = null;       // cleanup for the active view (EventSource etc.)
const cpuHistory = new Map(); // unit id -> recent CPU % samples

// ── helpers ────────────────────────────────────────────────────────────
function h(tag, attrs, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === "class") el.className = v;
    else if (k === "text") el.textContent = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "style") el.setAttribute("style", v);
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) {
    if (kid == null || kid === false) continue;
    el.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  return el;
}

async function api(method, path, body) {
  const opts = { method, headers: {} };
  if (method !== "GET") opts.headers["X-DaemonHall-Token"] = TOKEN;
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const r = await fetch(path, opts);
  const ct = r.headers.get("content-type") || "";
  const data = ct.includes("json") ? await r.json() : await r.text();
  if (!r.ok) throw new Error((data && data.message) || data || r.statusText);
  return data;
}

function toast(msg, kind = "ok", ms = 5000) {
  const t = h("div", { class: "toast " + kind, role: kind === "err" ? "alert" : "status" }, msg);
  document.getElementById("toasts").append(t);
  setTimeout(() => t.remove(), kind === "err" ? Math.max(ms, 9000) : ms);
}

const enc = (id) => encodeURIComponent(id);

function fmtBytes(n) {
  if (n == null) return "—";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (n >= 10 || i === 0 ? Math.round(n) : n.toFixed(1)) + " " + u[i];
}
function fmtDur(sec) {
  if (sec == null || sec < 0) return "—";
  const d = Math.floor(sec / 86400), hh = Math.floor(sec % 86400 / 3600), m = Math.floor(sec % 3600 / 60), s = Math.floor(sec % 60);
  if (d) return `${d}d ${hh}h`;
  if (hh) return `${hh}h ${m}m`;
  if (m) return `${m}m ${s}s`;
  return `${s}s`;
}
const nowSec = () => Date.now() / 1000;
function rel(ts) {
  if (!ts) return "—";
  const d = ts - nowSec();
  return d >= 0 ? "in " + fmtDur(d) : fmtDur(-d) + " ago";
}
function absTime(sec) {
  if (!sec) return "—";
  return new Date(sec * 1000).toLocaleString([], { year: "numeric", month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

/// up | busy | idle | warn | down — drives colours everywhere.
function stateOf(u) {
  if (u.load_state === "not-found" || u.active_state === "failed") return "down";
  if (u.needs_attention) return "warn";
  if (u.active_state === "activating" || u.active_state === "deactivating" || u.active_state === "reloading") return "busy";
  if (u.active_state === "active") return "up";
  return "idle";
}
const badge = (u) => h("span", { class: "badge b-" + stateOf(u), text: timerDriven(u) && stateOf(u) === "idle" ? "on timer" : u.status_label });

function displayName(u) { return u.name.replace(/\.service$/, ""); }
/// A service started by a timer: being inactive between runs is normal.
const timerDriven = (u) => u.kind === "service" && u.triggered_by.some((t) => t.endsWith(".timer"));

// ── shared Cybercore theme catalog ─────────────────────────────────────
let themeCatalog = [];
let themeAppearance = "dark";

async function applyTheme(id, persist = false) {
  const theme = themeCatalog.find((entry) => entry.id === id);
  if (!theme) return;
  if (persist) await api("POST", "/api/cybergrid/active/" + enc(id), {});
  const css = await api("GET", `/api/cybergrid/css/${enc(id)}?appearance=${themeAppearance}`);
  document.getElementById("theme-vars").textContent = css;
  document.getElementById("theme-picker-label").textContent = theme.name;
  document.querySelectorAll("#theme-dropdown .theme-item").forEach((el) => el.classList.toggle("selected", el.dataset.value === id));
}
function closeDropdowns() {
  document.getElementById("theme-dropdown").hidden = true;
  document.getElementById("theme-picker-btn").setAttribute("aria-expanded", "false");
}
async function setupTheme() {
  const btn = document.getElementById("theme-picker-btn");
  const dd = document.getElementById("theme-dropdown");
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    const open = dd.hidden;
    closeDropdowns();
    if (!open) return;
    const r = btn.getBoundingClientRect();
    dd.style.top = r.bottom + 6 + "px";
    dd.style.left = Math.max(8, Math.min(r.left, innerWidth - 238)) + "px";
    dd.hidden = false;
    btn.setAttribute("aria-expanded", "true");
    const sel = dd.querySelector(".theme-item.selected");
    if (sel) sel.scrollIntoView({ block: "center" });
  });
  dd.addEventListener("click", (e) => {
    const item = e.target.closest(".theme-item");
    if (item) { applyTheme(item.dataset.value, true).catch((error) => toast(error.message, "err")); closeDropdowns(); }
  });
  dd.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target.classList.contains("theme-item")) { applyTheme(e.target.dataset.value, true).catch((error) => toast(error.message, "err")); closeDropdowns(); btn.focus(); }
  });
  document.addEventListener("click", (e) => { if (!e.target.closest(".theme-dropdown-wrap")) closeDropdowns(); });
  const catalog = await api("GET", "/api/cybergrid/themes");
  themeCatalog = catalog.themes;
  themeAppearance = catalog.appearance;
  const groups = new Map();
  for (const theme of themeCatalog) {
    const family = theme.family || "Other";
    if (!groups.has(family)) groups.set(family, []);
    groups.get(family).push(theme);
  }
  dd.replaceChildren(...[...groups].flatMap(([family, themes]) => [
    h("div", { class: "theme-group-label", text: family }),
    ...themes.map((theme) => h("div", {
      class: "theme-item",
      "data-value": theme.id,
      tabindex: "0",
      role: "option",
      text: theme.name,
    })),
  ]));
  await applyTheme(catalog.active);
  let lastCatalog = themeCatalog.map((theme) => theme.id).join("\0");
  let lastActive = catalog.active;
  let lastAppearance = catalog.appearance;
  const syncSharedTheme = async () => {
    if (document.hidden) return;
    try {
      const current = await api("GET", "/api/cybergrid/themes");
      const ids = current.themes.map((theme) => theme.id).join("\0");
      if (ids !== lastCatalog) { location.reload(); return; }
      if (current.active !== lastActive || current.appearance !== lastAppearance) {
        lastActive = current.active;
        lastAppearance = current.appearance;
        themeAppearance = current.appearance;
        themeCatalog = current.themes;
        await applyTheme(current.active);
      }
    } catch (error) { console.debug("Shared theme refresh deferred", error); }
  };
  const themeEvents = new EventSource("/api/cybergrid/events");
  themeEvents.addEventListener("theme-change", syncSharedTheme);
  document.addEventListener("visibilitychange", syncSharedTheme);
}

// ── modal ──────────────────────────────────────────────────────────────
const modal = document.getElementById("modal");
let lastFocus = null;
function openModal(title, body, actions) {
  lastFocus = document.activeElement;
  document.getElementById("modal-title").textContent = title;
  const b = document.getElementById("modal-body");
  b.replaceChildren(h("div", { class: "modal-pad" }, body), actions ? h("div", { class: "viewer-actions" }, actions) : null);
  modal.hidden = false;
  const first = b.querySelector("input, textarea, button");
  if (first) first.focus();
}
function closeModal() { modal.hidden = true; if (lastFocus) lastFocus.focus(); }
modal.addEventListener("click", (e) => { if (e.target === modal || e.target.closest("[data-close-modal]")) closeModal(); });
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  closeDropdowns();
  if (!modal.hidden) closeModal();
});

// ── actions ────────────────────────────────────────────────────────────
async function act(btn, method, path, body, okMsg) {
  if (btn) btn.classList.add("busy");
  try {
    const r = await api(method, path, body);
    toast(okMsg || r.message || "done", "ok");
    await refresh();
    return r;
  } catch (e) {
    toast(e.message, "err");
    return null;
  } finally {
    if (btn) btn.classList.remove("busy");
  }
}
const unitAction = (btn, u, verb) => act(btn, "POST", `/api/unit/${enc(u.id)}/action/${verb}`);

function confirmDelete(u) {
  const input = h("input", { class: "text-input", style: "width:100%", placeholder: u.name, "aria-label": "Type the unit name to confirm" });
  const go = h("button", { class: "danger", disabled: true, text: "Delete for good" });
  input.addEventListener("input", () => { go.disabled = input.value.trim() !== u.name; });
  go.addEventListener("click", async () => {
    const r = await act(go, "POST", `/api/unit/${enc(u.id)}/delete`);
    if (r) { closeModal(); location.hash = "#/"; }
  });
  openModal("Delete " + u.id, [
    h("p", {}, "This stops and disables ", h("b", { text: u.name }), ", removes its unit file",
      u.kind === "service" ? " (and a same-named .timer, if any)" : " (and its same-named .service)",
      ", and reloads the manager. A backup copy is kept."),
    u.scope === "system" ? h("p", { class: "muted", text: "Backup: /var/lib/daemonhall/backups" }) : h("p", { class: "muted", text: "Backup: ~/.local/state/daemonhall/backups" }),
    h("p", {}, "Type ", h("code", { text: u.name }), " to confirm:"), input,
  ], [h("button", { "data-close-modal": true, text: "Cancel" }), go]);
}

function controlButtons(u, small) {
  const dis = !u.controllable;
  const why = dis ? "Not controllable: system units must be hand-installed in /etc/systemd/system and on the watch list" : null;
  const b = (label, verb, cls) => h("button", { class: cls, disabled: dis, title: why, onclick: (e) => { e.preventDefault(); unitAction(e.currentTarget, u, verb); } }, label);
  const out = [];
  if (u.kind === "timer") {
    out.push(b("Run now", "run-now", "primary"));
    if (!small) out.push(u.active_state === "active" ? b("Stop timer", "stop") : b("Start timer", "start"));
  } else if (timerDriven(u) && u.active_state !== "active") {
    out.push(b("Run now", "start", "primary"));
  } else if (u.active_state === "active" || u.active_state === "activating" || u.active_state === "reloading") {
    out.push(b("Restart", "restart"), b("Stop", "stop", "danger"));
    if (!small) out.push(b("Reload", "reload"));
  } else {
    out.push(b("Start", "start", "primary"));
  }
  if (!small) out.push(u.unit_file_state === "enabled" ? b("Disable", "disable") : b("Enable", "enable"));
  return out;
}

// ── views ──────────────────────────────────────────────────────────────
const hallState = { q: "", scope: "all", filter: "all" };

function viewHall() {
  const units = snap.units;
  const count = (f) => units.filter(f).length;
  const tiles = [
    ["all", "Total", units.length, "var(--purple)"],
    ["up", "Running", count((u) => stateOf(u) === "up" && u.kind === "service"), "var(--acid)"],
    ["timers", "Timers armed", count((u) => u.kind === "timer" && u.active_state === "active"), "var(--cyan)"],
    ["attention", "Need attention", count((u) => u.needs_attention), "var(--orange)"],
    ["down", "Failed", count((u) => stateOf(u) === "down"), "var(--red)"],
    ["idle", "Stopped", count((u) => stateOf(u) === "idle" && u.kind === "service" && !timerDriven(u)), "var(--muted)"],
  ];
  const match = (u) => {
    const q = hallState.q.toLowerCase();
    if (q && !(u.id.toLowerCase().includes(q) || u.description.toLowerCase().includes(q))) return false;
    if (hallState.scope !== "all" && u.scope !== hallState.scope) return false;
    switch (hallState.filter) {
      case "up": return stateOf(u) === "up" && u.kind === "service";
      case "timers": return u.kind === "timer";
      case "attention": return u.needs_attention;
      case "down": return stateOf(u) === "down";
      case "idle": return stateOf(u) === "idle" && u.kind === "service" && !timerDriven(u);
      default: return true;
    }
  };

  const search = h("input", { class: "text-input", type: "search", placeholder: "Search units and descriptions…", value: hallState.q, "aria-label": "Search units" });
  const scopeSel = h("select", { "aria-label": "Scope" },
    ...[["all", "All scopes"], ["system", "System"], ["user", "User"]].map(([v, l]) => h("option", { value: v, selected: hallState.scope === v }, l)));
  const gridHost = h("div");
  const tileRow = h("div", { class: "tiles" });

  function drawTiles() {
    tileRow.replaceChildren(...tiles.map(([key, label, n, c]) =>
      h("button", { class: "tile", style: `--tc:${c}`, "aria-pressed": String(hallState.filter === key), onclick: () => { hallState.filter = hallState.filter === key ? "all" : key; drawTiles(); drawGrid(); } },
        h("div", { class: "tile-n", text: n }), h("div", { class: "tile-l", text: label }))));
  }
  function card(u) {
    const st = stateOf(u);
    const stats = [];
    if (u.kind === "timer") {
      stats.push(h("span", {}, "NEXT ", h("b", { text: rel(u.next_run) })), h("span", {}, "LAST ", h("b", { text: rel(u.last_run) })));
    } else {
      if (timerDriven(u) && u.active_state !== "active") {
        stats.push(h("span", {}, "ON ", h("b", { text: u.triggered_by.find((t) => t.endsWith(".timer")) })));
        if (u.inactive_since) stats.push(h("span", {}, "LAST RUN ", h("b", { text: fmtDur(nowSec() - u.inactive_since) + " ago" })));
      } else if (u.active_state === "active") stats.push(h("span", {}, "UP ", h("b", { text: fmtDur(nowSec() - (u.active_since || nowSec())) })));
      else if (u.inactive_since) stats.push(h("span", {}, "DOWN ", h("b", { text: fmtDur(nowSec() - u.inactive_since) })));
      if (u.cpu_pct != null) stats.push(h("span", {}, "CPU ", h("b", { text: u.cpu_pct.toFixed(1) + "%" })));
      if (u.memory != null) stats.push(h("span", {}, "MEM ", h("b", { text: fmtBytes(u.memory) })));
      if (u.n_restarts) stats.push(h("span", {}, "RESTARTS ", h("b", { text: u.n_restarts })));
    }
    return h("article", { class: "unit-card state-" + st },
      h("div", { class: "uc-head" },
        h("span", { class: "uc-dot", "aria-hidden": "true" }),
        h("a", { class: "uc-name", href: "#/unit/" + enc(u.id), title: u.id, text: displayName(u) }),
        u.scope === "user" ? h("span", { class: "uc-scope", text: "user" }) : null,
        u.kind === "timer" ? h("span", { class: "uc-scope", text: "timer" }) : null,
        badge(u)),
      h("p", { class: "uc-desc", text: u.description || "—" }),
      h("div", { class: "uc-stats" }, stats),
      h("div", { class: "uc-actions" }, controlButtons(u, true),
        h("a", { class: "btn-ghost", href: "#/unit/" + enc(u.id) + "/logs", style: "padding:0.25rem 0.55rem;font-size:0.7rem" }, "Logs")));
  }
  function drawGrid() {
    const shown = units.filter(match);
    const order = { down: 0, warn: 1, busy: 2, up: 3, idle: 4 };
    shown.sort((a, b) => order[stateOf(a)] - order[stateOf(b)] || a.name.localeCompare(b.name));
    const groups = [
      ["Needs attention", shown.filter((u) => u.needs_attention || stateOf(u) === "down")],
      ["System services", shown.filter((u) => u.scope === "system" && u.kind === "service" && !(u.needs_attention || stateOf(u) === "down"))],
      ["User services", shown.filter((u) => u.scope === "user" && u.kind === "service" && !(u.needs_attention || stateOf(u) === "down"))],
      ["Timers", shown.filter((u) => u.kind === "timer" && !(u.needs_attention || stateOf(u) === "down"))],
    ];
    const parts = groups.filter(([, list]) => list.length).map(([title, list]) => [
      h("h2", { class: "section-title" }, title, h("span", { class: "count", text: "(" + list.length + ")" })),
      h("div", { class: "grid" }, list.map(card)),
    ]);
    gridHost.replaceChildren(...(parts.length ? parts.flat() : [h("div", { class: "empty-state", text: "No units match." })]));
  }
  search.addEventListener("input", () => { hallState.q = search.value; drawGrid(); });
  scopeSel.addEventListener("change", () => { hallState.scope = scopeSel.value; drawGrid(); });

  const attention = count((u) => u.needs_attention || stateOf(u) === "down");
  view.replaceChildren(
    h("section", { class: "hall-head" },
      h("img", { src: "/static/daemon.svg", alt: "" }),
      h("div", {},
        h("h1", {}, "THE ", h("span", { text: "HALL" })),
        h("p", { text: attention ? `${attention} of ${units.length} daemons need you.` : `All ${units.length} daemons accounted for.` }))),
    tileRow,
    h("div", { class: "toolbar" }, search, scopeSel, h("a", { class: "btn-ghost", href: "#/manage" }, "+ New service")),
    gridHost);
  drawTiles();
  drawGrid();
  renderCurrent = () => {
    // Refresh numbers in place; keep the search box (and its focus) as is.
    tiles.forEach((t) => {
      const f = { all: () => true, up: (u) => stateOf(u) === "up" && u.kind === "service", timers: (u) => u.kind === "timer" && u.active_state === "active", attention: (u) => u.needs_attention, down: (u) => stateOf(u) === "down", idle: (u) => stateOf(u) === "idle" && u.kind === "service" && !timerDriven(u) }[t[0]];
      t[2] = snap.units.filter(f).length;
    });
    units.length = 0;
    units.push(...snap.units);
    drawTiles();
    drawGrid();
  };
}

function sparkline(values) {
  const w = 300, hgt = 42, max = Math.max(5, ...values);
  const pts = values.map((v, i) => `${(i / Math.max(1, values.length - 1)) * w},${hgt - (v / max) * (hgt - 4) - 2}`).join(" ");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", `0 0 ${w} ${hgt}`);
  svg.setAttribute("preserveAspectRatio", "none");
  svg.setAttribute("class", "spark");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", "CPU history");
  const line = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
  line.setAttribute("points", pts);
  line.setAttribute("fill", "none");
  line.setAttribute("stroke", "var(--pink)");
  line.setAttribute("stroke-width", "1.5");
  svg.append(line);
  return svg;
}

async function viewUnit(id, tab) {
  tab = tab || "overview";
  const tabs = [["overview", "Overview"], ["logs", "Live logs"], ["activity", "Activity"], ["file", "Unit file"]];
  let detail;
  try { detail = await api("GET", "/api/unit/" + enc(id)); } catch (e) {
    view.replaceChildren(h("div", { class: "empty-state" }, "Not a watched unit: " + id + " ", h("a", { href: "#/manage", text: "add it to the watch list" })));
    return;
  }
  const head = h("div");
  const body = h("div");

  function drawHead(u) {
    head.replaceChildren(
      h("div", { class: "crumbs" }, h("a", { href: "#/", text: "Hall" }), " / ", u.scope, " / ", u.name),
      h("div", { class: "detail-head" },
        h("div", {},
          h("h1", {}, h("span", { class: "uc-dot", style: `--sc:var(--${{ up: "acid", busy: "cyan", idle: "muted", warn: "orange", down: "red" }[stateOf(u)]})` }),
            u.name, u.scope === "user" ? h("span", { class: "uc-scope", text: "user" }) : null,
            u.made_here ? h("span", { class: "badge b-made", text: "made in the hall" }) : null, badge(u)),
          h("p", { text: u.description })),
        h("div", { class: "actions" }, controlButtons(u, false),
          h("button", { onclick: (e) => act(e.currentTarget, "POST", "/api/watch", { unit: u.id, add: false }).then((r) => { if (r) location.hash = "#/"; }) }, "Unwatch"),
          u.controllable ? h("button", { class: "danger", onclick: () => confirmDelete(u) }, "Delete") : null)),
      h("div", { class: "tabs", role: "tablist" }, tabs.map(([k, l]) =>
        h("a", { class: "tab btn-ghost", role: "tab", "aria-selected": String(k === tab), href: `#/unit/${enc(id)}/${k}`, text: l }))));
  }

  function overview(u, ports) {
    const hist = cpuHistory.get(u.id) || [];
    const isSvc = u.kind === "service";
    const res = h("div", { class: "window" },
      h("div", { class: "titlebar" }, h("span", { class: "dot dot-a" }), h("span", { class: "dot dot-b" }), h("span", { class: "dot dot-c" }), h("span", { class: "titlebar-text", text: "vitals" })),
      h("div", { class: "window-body" },
        h("dl", { class: "kv" },
          kv("State", `${u.active_state} (${u.sub_state})`),
          isSvc ? kv(u.active_state === "active" ? "Up for" : "Down for", u.active_state === "active" ? fmtDur(nowSec() - (u.active_since || nowSec())) : (u.inactive_since ? fmtDur(nowSec() - u.inactive_since) : "—")) : null,
          isSvc ? kv("CPU", u.cpu_pct != null ? u.cpu_pct.toFixed(1) + "% of a core" : "—") : null,
          isSvc ? kv("Memory", fmtBytes(u.memory)) : null,
          isSvc ? kv("Tasks", u.tasks ?? "—") : null,
          isSvc ? kv("Main PID", u.main_pid || "—") : null,
          isSvc ? kv("Restarts", u.n_restarts ?? "—") : null,
          isSvc ? kv("Listening", ports.length ? ports.join(", ") : "none visible (ports of other users' processes need root)") : null,
          !isSvc ? kv("Next run", u.next_run ? `${rel(u.next_run)} · ${absTime(u.next_run)}` : "—") : null,
          !isSvc ? kv("Last run", u.last_run ? `${rel(u.last_run)} · ${absTime(u.last_run)}` : "never") : null),
        isSvc && hist.length > 1 ? sparkline(hist) : null));
    const props = h("div", { class: "window" },
      h("div", { class: "titlebar" }, h("span", { class: "dot dot-a" }), h("span", { class: "dot dot-b" }), h("span", { class: "dot dot-c" }), h("span", { class: "titlebar-text", text: "properties" })),
      h("div", { class: "window-body" },
        h("dl", { class: "kv" },
          kv("Unit file", u.fragment_path || "—"),
          kv("Enabled", u.unit_file_state || "—"),
          kv("Last result", u.result || "—"),
          u.exit_status != null && isSvc ? kv("Exit status", u.exit_status) : null,
          isSvc ? kv("Type", u.service_type || "—") : null,
          isSvc ? kv("Restart policy", u.restart_policy || "—") : null,
          u.scope === "system" && isSvc ? kv("Runs as", u.run_as || "root") : null,
          u.triggers.length ? kv("Triggers", u.triggers.join(" ")) : null,
          u.triggered_by.length ? kv("Triggered by", u.triggered_by.join(" ")) : null,
          kv("Control", u.controllable ? "yes (watched)" : "read-only: not a hand-installed unit"))));
    const deps = h("div", { class: "window", style: "margin-top:1rem" },
      h("div", { class: "titlebar" }, h("span", { class: "dot dot-a" }), h("span", { class: "dot dot-b" }), h("span", { class: "dot dot-c" }), h("span", { class: "titlebar-text", text: "dependencies" })),
      h("div", { class: "window-body" }, h("dl", { class: "kv" },
        ...[["Requires", u.requires], ["Wants", u.wants], ["After", u.after], ["Wanted by", u.wanted_by]].map(([k, list]) =>
          h("div", {}, h("dt", { text: k }), h("dd", {}, list.length ? h("div", { class: "deps" }, list.slice(0, 40).map((d) => h("span", { text: d }))) : "—"))))));
    return [h("div", { class: "split" }, res, props), deps];
  }

  function logs(u) {
    const out = h("div", { class: "log-view", role: "log", "aria-live": "off", tabindex: "0" });
    const filter = h("input", { class: "text-input", type: "search", placeholder: "Filter lines…", "aria-label": "Filter log lines", style: "flex:1;min-width:180px" });
    const prio = h("select", { "aria-label": "Minimum priority" }, h("option", { value: "7" }, "All levels"), h("option", { value: "4" }, "Warnings +"), h("option", { value: "3" }, "Errors only"));
    const dot = h("span", { class: "live-dot", title: "live" });
    const pauseBtn = h("button", { text: "Pause" });
    const follow = h("label", { class: "check" }, h("input", { type: "checkbox", checked: true }), "Follow");
    let paused = false, lines = [];
    const keep = 3000;
    const render = (l) => {
      const q = filter.value.toLowerCase();
      if (l.prio > Number(prio.value)) return null;
      if (q && !l.msg.toLowerCase().includes(q)) return null;
      const t = new Date(l.ts / 1000).toLocaleTimeString([], { hour12: false }) ;
      const row = h("span", { class: "ll p" + l.prio }, h("span", { class: "t", text: t }));
      if (q) {
        const i = l.msg.toLowerCase().indexOf(q);
        row.append(l.msg.slice(0, i), h("mark", { text: l.msg.slice(i, i + q.length) }), l.msg.slice(i + q.length));
      } else row.append(l.msg);
      return row;
    };
    const redraw = () => { out.replaceChildren(...lines.map(render).filter(Boolean)); if (follow.firstChild.checked) out.scrollTop = out.scrollHeight; };
    filter.addEventListener("input", redraw);
    prio.addEventListener("change", redraw);
    pauseBtn.addEventListener("click", () => { paused = !paused; pauseBtn.textContent = paused ? "Resume" : "Pause"; dot.classList.toggle("off", paused); if (!paused) redraw(); });
    const es = new EventSource(`/api/logs?unit=${enc(u.id)}&lines=300`);
    es.onmessage = (ev) => {
      const l = JSON.parse(ev.data);
      lines.push(l);
      if (lines.length > keep) lines = lines.slice(-keep);
      if (paused) return;
      const row = render(l);
      if (row) {
        out.append(row);
        while (out.childNodes.length > keep) out.firstChild.remove();
        if (follow.firstChild.checked) out.scrollTop = out.scrollHeight;
      }
    };
    es.onerror = () => { dot.classList.add("off"); };
    teardown = () => es.close();
    return [h("div", { class: "log-tools" }, dot, filter, prio, follow, pauseBtn,
      h("button", { onclick: () => { lines = []; out.replaceChildren(); } }, "Clear")), out];
  }

  async function unitFile(u) {
    if (u.scope === "user") {
      const ta = h("textarea", { rows: "24", spellcheck: "false", "aria-label": "Unit file" });
      ta.value = detail.file;
      const save = h("button", { class: "primary", text: "Verify + save" });
      save.addEventListener("click", () => act(save, "POST", `/api/unit/${enc(u.id)}/file`, { text: ta.value }));
      return [h("p", { class: "muted" }, "Your own file (", h("code", { text: u.fragment_path }), "). Saving runs ", h("code", { text: "systemd-analyze --user verify" }), " first, keeps a backup and reloads the user manager."), ta, h("div", { class: "row-actions" }, save)];
    }
    return [
      h("p", { class: "muted" }, "System unit files are root's. Editing opens ", h("code", { text: "sudoedit" }), " in a terminal (asks for your password) and reloads systemd after."),
      h("pre", { class: "preview", text: detail.file }),
      h("div", { class: "row-actions" }, h("button", { onclick: (e) => act(e.currentTarget, "POST", `/api/unit/${enc(u.id)}/edit-in-terminal`) }, "Edit in terminal")),
    ];
  }

  async function activityTab(u) {
    const list = await api("GET", `/api/activity?unit=${enc(u.id)}&limit=150`);
    return timeline(list, false);
  }

  async function drawBody() {
    if (teardown) { teardown(); teardown = null; }
    const u = detail.unit;
    let content;
    if (tab === "logs") content = logs(u);
    else if (tab === "file") content = await unitFile(u);
    else if (tab === "activity") content = await activityTab(u);
    else content = overview(u, detail.ports);
    body.replaceChildren(...content);
  }

  drawHead(detail.unit);
  view.replaceChildren(head, body);
  await drawBody();
  renderCurrent = async () => {
    const fresh = snap.units.find((x) => x.id === id);
    if (!fresh) return;
    detail.unit = fresh;
    drawHead(fresh);
    if (tab === "overview") {
      try { detail = await api("GET", "/api/unit/" + enc(id)); } catch { return; }
      body.replaceChildren(...overview(detail.unit, detail.ports));
    }
  };
}
function kv(k, v) { return h("div", {}, h("dt", { text: k }), h("dd", { text: String(v) })); }

/// Folds runs of the same unit + kind + message (a 30-second timer, say)
/// into one entry with a count and a time span.
function fold(list) {
  const out = [];
  for (const e of list) {
    const prev = out[out.length - 1];
    if (prev && prev.unit === e.unit && prev.kind === e.kind && prev.message === e.message && prev.source === e.source && prev.ok === e.ok) {
      prev.count += 1;
      prev.oldest = e.ts;
    } else out.push(Object.assign({ count: 1, oldest: e.ts }, e));
  }
  return out;
}

function timeline(list, showUnit) {
  if (!list.length) return [h("div", { class: "empty-state", text: "Nothing recorded yet." })];
  const items = [];
  let lastDay = "";
  for (const e of fold(list)) {
    const d = new Date(e.ts / 1000);
    const day = d.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" });
    if (day !== lastDay) { items.push(h("li", { class: "day", text: day })); lastDay = day; }
    const kind = e.source === "daemonhall" && e.ok === false ? "failed" : e.kind;
    items.push(h("li", { class: "ev-" + kind },
      h("span", { class: "when", text: d.toLocaleTimeString([], { hour12: false }) }),
      h("span", { class: "what", text: e.source === "daemonhall" ? (e.ok === false ? e.kind + " ✗" : e.kind) : e.kind }),
      h("span", { class: "msg" },
        showUnit ? [h("a", { href: "#/unit/" + enc(e.unit), text: e.unit }), " "] : null,
        e.message,
        e.count > 1 ? h("span", { class: "src", style: "color:var(--cyan)", text: `×${e.count} since ${new Date(e.oldest / 1000).toLocaleTimeString([], { hour12: false })}` }) : null,
        h("span", { class: "src", text: e.source === "daemonhall" ? "via hall" : "" }))));
  }
  return [h("ul", { class: "timeline" }, items)];
}

async function viewActivity() {
  const unitSel = h("select", { "aria-label": "Unit" }, h("option", { value: "" }, "All units"),
    ...snap.units.map((u) => h("option", { value: u.id, text: u.id })));
  const kinds = ["started", "stopped", "restarted", "failed", "auto-restart", "finished", "exited", "alert", "action"];
  const kindSel = h("select", { "aria-label": "Event kind" }, h("option", { value: "" }, "All events"), ...kinds.map((k) => h("option", { value: k, text: k })));
  const q = h("input", { class: "text-input", type: "search", placeholder: "Search messages…", "aria-label": "Search events" });
  const routine = h("label", { class: "check" }, h("input", { type: "checkbox" }), "Show routine timer runs");
  const host = h("div");
  let data = [];
  const draw = () => {
    const s = q.value.toLowerCase();
    const shown = data.filter((e) => (!kindSel.value || e.kind === kindSel.value) && (!s || (e.unit + " " + e.message).toLowerCase().includes(s))
      && (routine.firstChild.checked || kindSel.value === "finished" || unitSel.value || e.kind !== "finished"));
    host.replaceChildren(...timeline(shown, !unitSel.value));
  };
  const load = async () => {
    const path = "/api/activity?limit=400" + (unitSel.value ? "&unit=" + enc(unitSel.value) : "");
    try { data = await api("GET", path); } catch (e) { toast(e.message, "err"); }
    draw();
  };
  unitSel.addEventListener("change", load);
  kindSel.addEventListener("change", draw);
  q.addEventListener("input", draw);
  routine.firstChild.addEventListener("change", draw);
  view.replaceChildren(
    h("section", { class: "hall-head" }, h("img", { src: "/static/daemon.svg", alt: "" }),
      h("div", {}, h("h1", {}, "THE ", h("span", { text: "CHRONICLE" })), h("p", { text: "Every start, stop, crash and restart of your daemons, from systemd's own journal, plus what was done from here." }))),
    h("div", { class: "toolbar" }, q, unitSel, kindSel, routine, h("button", { onclick: load }, "Refresh")),
    host);
  await load();
  const t = setInterval(load, 15000);
  teardown = () => clearInterval(t);
  renderCurrent = null;
}

function viewTimers() {
  const draw = () => {
    const timers = snap.units.filter((u) => u.kind === "timer");
    const byId = new Map(snap.units.map((u) => [u.id, u]));
    const rows = timers.sort((a, b) => (a.next_run || 9e12) - (b.next_run || 9e12)).map((t) => {
      const svcName = t.triggers.find((x) => x.endsWith(".service")) || "";
      const svc = byId.get((t.scope === "user" ? "user:" : "") + svcName);
      return h("tr", {},
        h("td", {}, h("a", { href: "#/unit/" + enc(t.id), text: t.id })),
        h("td", {}, badge(t)),
        h("td", { text: t.next_run ? rel(t.next_run) : "—", title: absTime(t.next_run) }),
        h("td", { text: t.last_run ? rel(t.last_run) : "never", title: absTime(t.last_run) }),
        h("td", {}, svc ? h("a", { href: "#/unit/" + enc(svc.id), text: svcName }) : svcName || "—"),
        h("td", {}, svc ? h("span", { class: "badge b-" + (svc.result && svc.result !== "success" ? "down" : "up"), text: svc.result || "—" }) : "—"),
        h("td", { text: t.unit_file_state }),
        h("td", {}, h("div", { class: "uc-actions" }, controlButtons(t, false))));
    });
    view.replaceChildren(
      h("section", { class: "hall-head" }, h("img", { src: "/static/daemon.svg", alt: "" }),
        h("div", {}, h("h1", {}, "THE ", h("span", { text: "CLOCKWORK" })), h("p", { text: `${timers.length} timers. Run any of them now, or switch them off.` }))),
      timers.length ? h("div", { class: "table-wrap" }, h("table", { class: "res-table" },
        h("thead", {}, h("tr", {}, ["Timer", "State", "Next", "Last", "Runs", "Last result", "Enabled", ""].map((x) => h("th", { text: x })))),
        h("tbody", {}, rows))) : h("div", { class: "empty-state", text: "No watched timers." }));
  };
  draw();
  renderCurrent = draw;
}

function viewManage() {
  // Watch list picker
  const pickHost = h("div", {}, h("div", { class: "loading", text: "listing unit files…" }));
  const pickQ = h("input", { class: "text-input", type: "search", placeholder: "Find any unit file…", "aria-label": "Find unit files" });
  const pickScope = h("select", { "aria-label": "Scope" }, h("option", { value: "" }, "All"), h("option", { value: "system" }, "System"), h("option", { value: "user" }, "User"));
  const onlyWatched = h("label", { class: "check" }, h("input", { type: "checkbox" }), "Watched only");
  let files = [];
  const drawPick = () => {
    const q = pickQ.value.toLowerCase();
    const list = files.filter((f) => (!q || f.id.toLowerCase().includes(q)) && (!pickScope.value || f.scope === pickScope.value) && (!onlyWatched.firstChild.checked || f.watched));
    pickHost.replaceChildren(h("div", { class: "table-wrap", style: "max-height:420px;overflow:auto" }, h("table", { class: "res-table" },
      h("thead", {}, h("tr", {}, ["Unit", "Scope", "File state", ""].map((x) => h("th", { text: x })))),
      h("tbody", {}, list.slice(0, 400).map((f) => h("tr", {},
        h("td", {}, f.watched ? h("a", { href: "#/unit/" + enc(f.id), text: f.id }) : f.id),
        h("td", { text: f.scope }), h("td", { text: f.state }),
        h("td", {}, h("button", { class: f.watched ? "" : "primary", onclick: async (e) => {
          const r = await act(e.currentTarget, "POST", "/api/watch", { unit: f.id, add: !f.watched });
          if (r) { f.watched = !f.watched; drawPick(); }
        } }, f.watched ? "Unwatch" : "Watch"))))))),
      list.length > 400 ? h("p", { class: "muted small", text: `showing 400 of ${list.length}; narrow the search` }) : null);
  };
  pickQ.addEventListener("input", drawPick);
  pickScope.addEventListener("change", drawPick);
  onlyWatched.firstChild.addEventListener("change", drawPick);
  api("GET", "/api/unit-files").then((f) => { files = f; drawPick(); }).catch((e) => pickHost.replaceChildren(h("div", { class: "error-box", text: e.message })));

  // New service form
  const f = {};
  const field = (label, el, hint, wide) => h("div", { class: "field" + (wide ? " wide" : "") }, h("label", { class: "field-label", for: el.id, text: label }), el, hint ? h("span", { class: "hint", text: hint }) : null);
  const input = (id, attrs) => (f[id] = h("input", Object.assign({ id: "nf-" + id, class: "text-input" }, attrs)));
  const scopeSeg = h("div", { class: "seg", role: "radiogroup", "aria-label": "Scope" },
    ...[["user", "User (no root)"], ["system", "System (boot, no login)"]].map(([v, l], i) =>
      h("label", {}, h("input", { type: "radio", name: "nf-scope", value: v, checked: i === 0 }), h("span", { text: l }))));
  const restart = (f.restart = h("select", { id: "nf-restart" }, h("option", { value: "on-failure" }, "on failure"), h("option", { value: "always" }, "always"), h("option", { value: "no" }, "never")));
  const env = (f.env = h("textarea", { id: "nf-env", rows: "3", placeholder: "RUST_LOG=info\nPORT=8080", spellcheck: "false" }));
  const writable = (f.writable = h("textarea", { id: "nf-writable", rows: "2", placeholder: "/home/raven/.local/share/myapp", spellcheck: "false" }));
  const checks = {};
  const check = (k, label, on) => h("label", { class: "check" }, (checks[k] = h("input", { type: "checkbox", checked: on })), label);
  const timerOn = check("timer", "Run on a schedule (timer) instead of as a daemon", false);
  const cal = input("cal", { placeholder: "daily · hourly · Mon *-*-* 03:00:00", disabled: true });
  const persistent = check("persistent", "Catch up missed runs (Persistent)", true);
  checks.timer.addEventListener("change", () => { cal.disabled = !checks.timer.checked; restart.disabled = checks.timer.checked; schedulePreview(); });
  const previewBox = h("pre", { class: "preview", text: "Fill in the form: the unit file appears here." });
  const errBox = h("div", { class: "error-box", hidden: true });
  const createBtn = h("button", { class: "primary", text: "Summon it" });

  const spec = () => ({
    scope: scopeSeg.querySelector("input:checked").value,
    name: f.name.value.trim(),
    description: f.desc.value.trim(),
    exec: f.exec.value.trim(),
    args: splitArgs(f.args.value),
    working_directory: f.wd.value.trim() || null,
    environment: env.value.split("\n").map((l) => l.trim()).filter(Boolean).map((l) => { const i = l.indexOf("="); return i < 0 ? [l, ""] : [l.slice(0, i), l.slice(i + 1)]; }),
    restart: restart.value,
    restart_sec: Number(f.rsec.value) || 5,
    hardened: checks.hardened.checked,
    writable_paths: writable.value.split("\n").map((l) => l.trim()).filter(Boolean),
    after_network: checks.net.checked,
    timer: checks.timer.checked ? { on_calendar: cal.value.trim(), persistent: checks.persistent.checked } : null,
    start_now: checks.start.checked,
  });
  let pt = null;
  const schedulePreview = () => { clearTimeout(pt); pt = setTimeout(doPreview, 350); };
  async function doPreview() {
    const s = spec();
    if (!s.name && !s.exec) return;
    try {
      const r = await api("POST", "/api/services/preview", s);
      if (r.ok) {
        errBox.hidden = true;
        previewBox.textContent = `# ${r.service_name}\n${r.service}` + (r.timer ? `\n# ${r.timer_name}\n${r.timer}` : "");
      } else { errBox.hidden = false; errBox.textContent = r.message; }
    } catch (e) { errBox.hidden = false; errBox.textContent = e.message; }
  }
  createBtn.addEventListener("click", async () => {
    const s = spec();
    const r = await act(createBtn, "POST", "/api/services", s);
    if (r) location.hash = "#/unit/" + enc((s.scope === "user" ? "user:" : "") + s.name + (s.timer ? ".timer" : ".service"));
  });

  const form = h("div", { class: "form-grid" },
    h("div", { class: "field wide" }, h("span", { class: "field-label", text: "Scope" }), scopeSeg,
      h("span", { class: "hint", text: "User services run in your session and need no root. System services start at boot; DaemonHall's helper installs them, always running as you." })),
    field("Name", input("name", { placeholder: "my-daemon", autocomplete: "off" }), "lowercase, digits, - and _"),
    field("Description", input("desc", { placeholder: "What it does" })),
    field("Program", input("exec", { placeholder: "/home/raven/.local/bin/my-daemon" }), "absolute path to the executable", true),
    field("Arguments", input("args", { placeholder: "--port 9000 \"quoted arg\"" }), null, true),
    field("Working directory", input("wd", { placeholder: "(optional)" })),
    field("Restart", restart),
    field("Restart delay (s)", input("rsec", { type: "number", min: "1", max: "3600", value: "5" })),
    field("Environment (one per line)", env, null, true),
    h("div", { class: "field wide" }, check("hardened", "Hardened (ProtectSystem=strict, ProtectHome=read-only, …)", true), check("net", "Wait for network", false), check("start", "Enable and start right away", true)),
    field("Writable paths when hardened (one per line)", writable, "ProtectHome=read-only makes $HOME read-only; list what it writes to", true),
    h("div", { class: "field wide" }, timerOn), field("Schedule (OnCalendar)", cal, "checked with systemd-analyze before install"),
    h("div", { class: "field" }, h("span", { class: "field-label", text: " " }), persistent));
  form.addEventListener("input", schedulePreview);
  form.addEventListener("change", schedulePreview);

  // Maintenance
  const settingsHost = h("div");
  api("GET", "/api/settings").then((s) => {
    const sw = (k, label) => {
      const cb = h("input", { type: "checkbox", checked: s[k] });
      cb.addEventListener("change", () => { s[k] = cb.checked; act(null, "POST", "/api/settings", s); });
      return h("label", { class: "check" }, cb, label);
    };
    settingsHost.replaceChildren(sw("notify_failures", "Desktop notification when a daemon fails or needs attention"),
      sw("notify_recoveries", "…and when it recovers"), sw("notify_flapping", "…and when it flaps (3+ restarts in 10 min)"));
  });
  const win = (title, ...kids) => h("section", { class: "window", style: "margin-bottom:1rem" },
    h("div", { class: "titlebar" }, h("span", { class: "dot dot-a" }), h("span", { class: "dot dot-b" }), h("span", { class: "dot dot-c" }), h("span", { class: "titlebar-text", text: title })),
    h("div", { class: "window-body" }, ...kids));

  view.replaceChildren(
    h("section", { class: "hall-head" }, h("img", { src: "/static/daemon.svg", alt: "" }),
      h("div", {}, h("h1", {}, "THE ", h("span", { text: "SUMMONING ROOM" })), h("p", { text: "Summon new daemons, choose which ones the hall watches, and keep the place tidy." }))),
    win("summon a new service",
      h("div", { class: "cols-2" }, h("div", {}, form, h("div", { class: "row-actions" }, createBtn)),
        h("div", {}, h("p", { class: "field-label", text: "Unit file preview" }), errBox, previewBox))),
    win("watch list (shared with cyberwatch)", h("div", { class: "toolbar" }, pickQ, pickScope, onlyWatched), pickHost),
    win("maintenance",
      h("div", { class: "row-actions" },
        h("button", { onclick: (e) => act(e.currentTarget, "POST", "/api/daemon-reload", { scope: "user" }) }, "Reload user manager"),
        h("button", { onclick: (e) => act(e.currentTarget, "POST", "/api/daemon-reload", { scope: "system" }) }, "Reload system manager"),
        h("button", { onclick: (e) => act(e.currentTarget, "POST", "/api/polkit-sync") }, "Re-sync polkit rule")),
      h("p", { class: "muted small" }, "Watch list: ", h("code", { text: meta ? meta.config : "~/.config/cyberwatch/config.json" }),
        " · helper ", h("b", { text: snap.helper_installed ? "installed" : "missing" }), " · polkit rule ", h("b", { text: snap.rule_installed ? "installed" : "missing" })),
      settingsHost));
  renderCurrent = null;
}

/// Shell-ish argument split: spaces separate, "double" or 'single' quotes group.
function splitArgs(s) {
  const out = [];
  let cur = "", q = null, any = false;
  for (const c of s) {
    if (q) { if (c === q) q = null; else cur += c; }
    else if (c === '"' || c === "'") { q = c; any = true; }
    else if (/\s/.test(c)) { if (cur || any) { out.push(cur); cur = ""; any = false; } }
    else cur += c;
  }
  if (cur || any) out.push(cur);
  return out;
}

// ── data + routing ─────────────────────────────────────────────────────
async function refresh() {
  try {
    snap = await api("GET", "/api/units");
  } catch {
    return;
  }
  for (const u of snap.units) {
    if (u.cpu_pct == null) continue;
    const arr = cpuHistory.get(u.id) || [];
    arr.push(u.cpu_pct);
    if (arr.length > 120) arr.shift();
    cpuHistory.set(u.id, arr);
  }
  const banner = document.getElementById("banner");
  if (!snap.helper_installed || !snap.rule_installed) {
    banner.hidden = false;
    banner.replaceChildren(h("b", { text: "Privileged helper not installed. " }),
      "User services work fully; for system services run ", h("code", { text: "sudo ./packaging/install-root.sh" }),
      " from the DaemonHall checkout once (installs the root helper + a polkit rule scoped to your watched units).");
  } else banner.hidden = true;
  const attention = snap.units.filter((u) => u.needs_attention || stateOf(u) === "down").length;
  document.title = (attention ? `(${attention}) ` : "") + "DAEMONHALL";
  if (renderCurrent) await renderCurrent();
}

async function route() {
  if (teardown) { teardown(); teardown = null; }
  renderCurrent = null;
  const hash = location.hash.replace(/^#/, "") || "/";
  const parts = hash.split("/").filter(Boolean);
  const section = parts[0] || "hall";
  document.querySelectorAll("[data-nav]").forEach((a) => a.classList.toggle("active", a.dataset.nav === (section === "unit" ? "hall" : section)));
  if (!snap) await refresh();
  if (!snap) { view.replaceChildren(h("div", { class: "error-box", text: "Can't reach the DaemonHall server." })); return; }
  currentRoute = hash;
  if (section === "unit") await viewUnit(decodeURIComponent(parts[1] || ""), parts[2]);
  else if (section === "activity") await viewActivity();
  else if (section === "timers") viewTimers();
  else if (section === "manage") viewManage();
  else viewHall();
  view.focus({ preventScroll: true });
}

function startPolling() {
  clearInterval(pollTimer);
  pollTimer = setInterval(() => { if (!document.hidden) refresh(); }, POLL_MS);
}
document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });

window.addEventListener("hashchange", route);
setupTheme().catch((error) => toast("Could not load shared themes: " + error.message, "err"));
api("GET", "/api/meta").then((m) => { meta = m; document.getElementById("brand-host").textContent = "@" + m.host; }).catch(() => {});
route();
startPolling();
