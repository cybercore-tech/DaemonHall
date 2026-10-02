/* Cybercore project-site kit — theme engine + interactions.
 *
 * Themes come from the org site's schema at /data (same origin on
 * cybercore-tech.github.io), so every project site shares one registry:
 *   /data/cybergrid.json            families → theme names
 *   /data/themes/<family>/<name>.json  11-slot palettes
 * Each site also ships SIGNATURE MASHUPS (see SITE.mashups in index.html):
 * a "base" palette (surfaces/text) blended with an "accent" palette (neons),
 * merged live from the same registry.
 */
(() => {
  const root = document.documentElement;
  const SITE = window.SITE || { mashups: [] };
  const DATA = (SITE.dataRoot || '/data').replace(/\/$/, '');
  const STORE = 'cybercore-theme';
  const FALLBACK = { bg: '09080f', white: 'f0f0f2', acid_green: '39ff33', hot_pink: 'ff147f', purple: '8a22e2', cyan: '00e5ff', orange: 'ff9000', red: 'f1142d', panel: '16151f', line: '32303e', muted: '807f8b' };
  const VARS = { bg: '--bg', white: '--white', acid_green: '--acid', hot_pink: '--pink', purple: '--purple', cyan: '--cyan', orange: '--orange', red: '--red', panel: '--panel', line: '--line', muted: '--muted' };
  const SURFACE = ['bg', 'white', 'panel', 'line', 'muted'];
  const LABELS = { mashup: 'SIGNATURE MASHUPS', 'cybercore-tech': 'CYBERCORE TECH / MASTER MIX', cyberdyne: 'CYBERDYNE', cyberpunk: 'CYBERPUNK', default: 'DEFAULT / CLASSICS', dystopian: 'DYSTOPIAN', neosynth: 'NEOSYNTH', synthwave: 'SYNTHWAVE' };
  const display = s => s.replaceAll('-', ' ').toUpperCase();
  const $ = s => document.querySelector(s);

  let families = {};
  const familyOf = name => Object.keys(families).find(f => families[f].includes(name));
  const cache = {};

  async function palette(name) {
    if (cache[name]) return cache[name];
    const family = familyOf(name);
    if (!family) throw new Error(`unknown theme ${name}`);
    const res = await fetch(`${DATA}/themes/${family}/${name}.json`);
    if (!res.ok) throw new Error(`${name}: HTTP ${res.status}`);
    return (cache[name] = await res.json());
  }

  async function resolve(name) {
    const mash = SITE.mashups.find(m => m.id === name);
    if (!mash) return palette(name);
    const [base, accent] = await Promise.all([palette(mash.base), palette(mash.accent)]);
    const out = { ...accent };
    SURFACE.forEach(k => { out[k] = base[k]; });
    return out;
  }

  function paint(name, p) {
    Object.entries(VARS).forEach(([k, v]) => p[k] && root.style.setProperty(v, `#${p[k]}`));
    root.dataset.theme = name;
    const mash = SITE.mashups.find(m => m.id === name);
    const label = mash ? mash.label : display(name);
    document.querySelectorAll('[data-active-theme]').forEach(el => { el.textContent = label; });
    const recipe = $('#themeRecipe');
    if (recipe) recipe.textContent = mash ? `${display(mash.base)} × ${display(mash.accent)}` : 'FAMILY PALETTE';
    const sw = $('#swatches');
    if (sw) sw.innerHTML = ['acid_green', 'hot_pink', 'purple', 'cyan', 'orange', 'red'].map(k => `<i title="${k}" style="background:#${p[k]}"></i>`).join('');
    syncMenu(name);
  }

  async function apply(name, { persist = true } = {}) {
    try { paint(name, await resolve(name)); } catch (e) { console.warn('theme', e); paint('cybercore-tech', FALLBACK); return; }
    if (persist) { try { localStorage.setItem(STORE, name); } catch {} history.replaceState(null, '', `?theme=${name}${location.hash}`); }
  }

  /* ---------- dark categorized theme menu ---------- */
  const button = $('#themeButton');
  const menu = $('#themeMenu');
  function closeMenu() { if (!menu) return; menu.hidden = true; button.setAttribute('aria-expanded', 'false'); }
  function syncMenu(name) {
    if (!menu) return;
    const mash = SITE.mashups.find(m => m.id === name);
    button.textContent = mash ? mash.label : display(name);
    menu.querySelectorAll('.theme-option').forEach(o => o.setAttribute('aria-selected', String(o.dataset.value === name)));
  }
  function buildMenu() {
    if (!menu) return;
    menu.innerHTML = '';
    const groups = [['mashup', SITE.mashups.map(m => ({ value: m.id, text: m.label }))]]
      .concat(Object.entries(families).map(([f, names]) => [f, names.map(n => ({ value: n, text: display(n) }))]));
    groups.forEach(([family, items]) => {
      if (!items.length) return;
      const h = document.createElement('div');
      h.className = 'theme-group-label';
      h.textContent = LABELS[family] || family.toUpperCase();
      menu.append(h);
      items.forEach(item => {
        const b = document.createElement('button');
        b.type = 'button'; b.className = 'theme-option'; b.setAttribute('role', 'option');
        b.dataset.value = item.value; b.textContent = item.text;
        b.addEventListener('click', () => { apply(item.value); closeMenu(); button.focus(); });
        menu.append(b);
      });
    });
    const count = $('#themeCount');
    if (count) count.textContent = `${SITE.mashups.length + Object.values(families).flat().length} THEMES`;
  }
  if (button && menu) {
    button.addEventListener('click', () => {
      const opening = menu.hidden; menu.hidden = !opening; button.setAttribute('aria-expanded', String(opening));
      // Scroll only the menu to the current theme; scrollIntoView would also
      // scroll the page (and move targets under the pointer mid-click).
      const current = opening && menu.querySelector('[aria-selected="true"]');
      if (current) menu.scrollTop = current.offsetTop - menu.clientHeight / 2 + current.offsetHeight / 2;
    });
    document.addEventListener('pointerdown', e => { if (!e.target.closest('.theme-picker')) closeMenu(); });
    document.addEventListener('keydown', e => { if (e.key === 'Escape' && !menu.hidden) { closeMenu(); button.focus(); } });
  }

  async function boot() {
    try {
      const schema = await (await fetch(`${DATA}/cybergrid.json`)).json();
      families = schema.families || {};
    } catch (e) {
      console.warn('theme registry offline', e);
      families = {};
      if (button) button.textContent = 'REGISTRY OFFLINE';
      paint('cybercore-tech', FALLBACK);
      return;
    }
    buildMenu();
    const known = n => n && (SITE.mashups.some(m => m.id === n) || familyOf(n));
    const requested = new URLSearchParams(location.search).get('theme');
    let saved = null; try { saved = localStorage.getItem(STORE); } catch {}
    const start = [requested, saved, SITE.defaultTheme, 'cybercore-tech'].find(known);
    apply(start, { persist: Boolean(requested) });
  }

  /* ---------- ambience ---------- */
  const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
  window.addEventListener('pointermove', e => { root.style.setProperty('--x', `${e.clientX}px`); root.style.setProperty('--y', `${e.clientY}px`); }, { passive: true });
  if (!still) {
    const brand = $('.brand');
    if (brand) { let phase = 0; setInterval(() => { phase = (phase + 1 + Math.floor(Math.random() * 5)) % 6; brand.dataset.brandPhase = phase; }, 4700); }
    // Type-on terminal lines.
    document.querySelectorAll('[data-type]').forEach(block => {
      const lines = [...block.children]; lines.forEach(l => { l.style.visibility = 'hidden'; });
      const io = new IntersectionObserver(entries => {
        if (!entries.some(e => e.isIntersecting)) return; io.disconnect();
        lines.forEach((l, i) => setTimeout(() => { l.style.visibility = 'visible'; l.classList.add('typed'); }, 160 * i));
      }, { threshold: 0.3 });
      io.observe(block);
    });
    // Reveal-on-scroll.
    const ro = new IntersectionObserver(es => es.forEach(e => { if (e.isIntersecting) { e.target.classList.add('in'); ro.unobserve(e.target); } }), { threshold: 0.12 });
    document.querySelectorAll('.reveal').forEach(el => ro.observe(el));
  } else {
    document.querySelectorAll('.reveal').forEach(el => el.classList.add('in'));
  }
  // Copy buttons on code blocks.
  document.querySelectorAll('pre[data-copy]').forEach(pre => {
    const b = document.createElement('button'); b.className = 'copy'; b.type = 'button'; b.textContent = 'COPY';
    b.addEventListener('click', async () => { try { await navigator.clipboard.writeText(pre.querySelector('code').innerText); b.textContent = 'COPIED'; setTimeout(() => { b.textContent = 'COPY'; }, 1400); } catch { b.textContent = 'SELECT + COPY'; } });
    pre.append(b);
  });
  const year = $('#year'); if (year) year.textContent = new Date().getFullYear();

  boot();
})();
