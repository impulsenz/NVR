'use strict';
// ======================================================================
// Core helpers
// ======================================================================
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const enc = encodeURIComponent;
const DAYS = ['Mon','Tue','Wed','Thu','Fri','Sat','Sun'];
const LABEL_GROUP = {person:'person', car:'vehicle', truck:'vehicle', bus:'vehicle', motorcycle:'vehicle', bicycle:'vehicle',
  dog:'animal', cat:'animal', bird:'animal', horse:'animal', cow:'animal', sheep:'animal', bear:'animal'};
// Alert types. Plain recordings aren't listed: they are on the timeline.
const FILTER_TYPES = [['motion','Motion'],['person','Person'],['car','Car'],['truck','Truck'],['bus','Bus'],
  ['motorcycle','Motorcycle'],['bicycle','Bicycle'],['dog','Dog'],['cat','Cat'],['bird','Bird'],['other','Other objects']];

let status = null, settings = null, cameraCfgs = [], sysinfo = null;
let page = 'live';

const fmtBytes = (b) => {
  if (b == null || isNaN(b)) return '–';
  const u = ['B','KB','MB','GB','TB']; let i = 0;
  while (Math.abs(b) >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return b.toFixed(i ? (b < 10 ? 2 : 1) : 0) + ' ' + u[i];
};
const fmtDur = (s) => {
  s = Math.max(0, Math.floor(s)); const d = Math.floor(s / 86400); s %= 86400;
  const h = Math.floor(s / 3600); s %= 3600; const m = Math.floor(s / 60); s %= 60;
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  return m ? `${m}m ${s}s` : `${s}s`;
};
const fmtClock = (secs) => {
  secs = Math.max(0, secs); const h = Math.floor(secs / 3600), m = Math.floor(secs / 60) % 60, s = Math.floor(secs % 60);
  return (h ? h + ':' + String(m).padStart(2, '0') : m) + ':' + String(s).padStart(2, '0');
};

// Date/time formatting with tokens: YYYY YY MMMM MMM MM M DD D dddd ddd HH H hh h mm ss A a
const MONTHS = ['January','February','March','April','May','June','July','August','September','October','November','December'];
const WEEKDAYS = ['Sunday','Monday','Tuesday','Wednesday','Thursday','Friday','Saturday'];
function fmtDate(d, f) {
  d = d instanceof Date ? d : new Date(d);
  f = f || (status && status.display.clock_format) || 'DD/MM/YYYY HH:mm:ss';
  const p = (n) => String(n).padStart(2, '0');
  const h12 = d.getHours() % 12 || 12;
  const map = {
    YYYY: d.getFullYear(), YY: String(d.getFullYear()).slice(2), MMMM: MONTHS[d.getMonth()], MMM: MONTHS[d.getMonth()].slice(0, 3),
    MM: p(d.getMonth() + 1), M: d.getMonth() + 1, DD: p(d.getDate()), D: d.getDate(), dddd: WEEKDAYS[d.getDay()],
    ddd: WEEKDAYS[d.getDay()].slice(0, 3), HH: p(d.getHours()), H: d.getHours(), hh: p(h12), h: h12,
    mm: p(d.getMinutes()), ss: p(d.getSeconds()), A: d.getHours() < 12 ? 'AM' : 'PM', a: d.getHours() < 12 ? 'am' : 'pm',
  };
  return f.replace(/YYYY|YY|MMMM|MMM|MM|M|DD|D|dddd|ddd|HH|H|hh|h|mm|ss|A|a/g, t => map[t]);
}
// Date part / time part of the configured format.
function fmtDay(d) {
  const f = (status && status.display.clock_format) || 'DD/MM/YYYY';
  const datePart = f.replace(/\s*[Hh]{1,2}.*$/, '').trim() || 'DD/MM/YYYY';
  return fmtDate(d, 'ddd ' + datePart);
}
function fmtTime(d) {
  const f = (status && status.display.clock_format) || 'HH:mm:ss';
  const m = f.match(/[Hh]{1,2}.*$/);
  return fmtDate(d, m ? m[0] : 'HH:mm:ss');
}

async function api(path, opts = {}) {
  if (opts.json !== undefined) {
    opts.body = JSON.stringify(opts.json); delete opts.json;
    opts.headers = {'Content-Type': 'application/json'};
  }
  const r = await fetch(path, opts);
  if (!r.ok) {
    let msg = r.statusText;
    try { msg = (await r.json()).error || msg; } catch (e) {}
    throw new Error(msg);
  }
  if (r.status === 204) return null;
  const ct = r.headers.get('content-type') || '';
  return ct.includes('json') ? r.json() : r.text();
}

let toastTimer;
function toast(msg, isError) {
  const t = $('toast'); t.textContent = msg; t.className = isError ? 'error' : ''; t.hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => t.hidden = true, isError ? 7000 : 3500);
}

function fullscreen(el) {
  if (document.fullscreenElement) document.exitFullscreen();
  else (el.requestFullscreen || el.webkitRequestFullscreen || (() => {})).call(el);
}
// Double-click any video picture: full screen; double-click again: back to
// how it was (also inside the full-screen video wall or recordings view,
// which the browser returns to).
document.addEventListener('dblclick', (e) => {
  const s = e.target.closest('.surface');
  if (!s || e.target.closest('button, a, input, select, .rz') || (typeof editing !== 'undefined' && editing)) return;
  e.preventDefault();
  window.getSelection && window.getSelection().removeAllRanges();
  if (document.fullscreenElement === s) document.exitFullscreen().catch(() => {});
  else (s.requestFullscreen || s.webkitRequestFullscreen || (() => {})).call(s);
});
// A single click that isn't the first half of a double-click.
function singleClick(fn, delay = 260) {
  let t = null;
  return (e) => {
    clearTimeout(t);
    if (e.detail > 1) return;
    t = setTimeout(() => fn(e), delay);
  };
}

const COLOR_CSS = {black: '#111', white: '#f5f5f5', grey: '#777', silver: '#c0c4c8', red: '#dc2626', orange: '#f97316', yellow: '#facc15',
  green: '#16a34a', blue: '#2563eb', purple: '#9333ea', pink: '#ec4899', brown: '#8b5a2b', beige: '#d6c7a1'};
// Zoom a whole-picture thumbnail into the recognized object (the box stays the same size).
function zoomStyle(f) {
  if (!f) return '';
  const [x, y, w, h] = f, s = Math.min(3, Math.max(1, 0.55 / Math.max(w, h * 0.9, 0.01)));
  if (s < 1.05) return '';
  return `transform-origin:${((x + w / 2) * 100).toFixed(1)}% ${((y + h / 2) * 100).toFixed(1)}%;transform:scale(${s.toFixed(2)})`;
}
// Boxes around what was detected in an alert picture, for review. The
// picture is the whole camera view, so boxes are placed in % of it.
let showBoxes = store_get('show-boxes', true);
document.body.classList.toggle('no-boxes', !showBoxes);
function boxesHtml(ev, labels) {
  if (!ev || !ev.boxes || !ev.boxes.length) return '';
  return '<div class="bxs">' + ev.boxes.map(b => {
    const motion = b.label === 'motion';
    const tip = motion ? 'motion' : `${b.label} ${Math.round(b.score * 100)}%`;
    return `<div class="bx ${motion ? 'motion' : ''} ${b.y < 0.07 ? 'top' : ''}" title="${esc(tip)}" style="left:${(b.x * 100).toFixed(2)}%;top:${(b.y * 100).toFixed(2)}%;width:${(b.w * 100).toFixed(2)}%;height:${(b.h * 100).toFixed(2)}%">${labels ? `<span>${motion ? labelIcon('motion') + ' motion' : labelIcon(b.label) + ' ' + esc(tip)}</span>` : ''}</div>`;
  }).join('') + '</div>';
}
function labelTags(labels, motion) {
  if (labels && labels.length) return labels.map(l => `<span class="tag ${LABEL_GROUP[l.label] || ''}">${l.color ? esc(l.color) + ' ' : ''}${esc(l.label)} ${Math.round(l.score * 100)}%</span>`).join('');
  return motion ? '<span class="tag motion">motion</span>' : '';
}

// Close popovers when clicking elsewhere.
document.addEventListener('click', (e) => {
  document.querySelectorAll('.popover').forEach(p => {
    if (!p.hidden && !p.parentElement.contains(e.target)) p.hidden = true;
  });
});

// ======================================================================
// Camera state helpers
// ======================================================================
function cam(id) { return status && status.cameras.find(c => c.id === id); }
// Problems connecting to the camera itself (shared connection).
function streamErrors(c) {
  const out = [];
  for (const [k, label] of [['main', 'Main stream'], ['sub', 'Sub-stream']]) {
    const s = c.streams && c.streams[k];
    if (s && s.state === 'retrying' && s.error) out.push(`${label}: ${explainError(s.error)}${s.retry_in ? ` (retrying in ${s.retry_in}s)` : ''}`);
  }
  return out;
}
// Plain-language hints for common camera replies.
function explainError(e) {
  if (/453|Not Enough Bandwidth/i.test(e)) return e + ' — the camera has too many connections open. Close other apps/NVRs viewing it, or restart the camera.';
  if (/401|Unauthorized/i.test(e)) return e + ' — wrong username or password.';
  if (/404|Not Found/i.test(e)) return e + ' — wrong stream path; try another URL format.';
  if (/Connection refused|timed out|No route/i.test(e)) return e + ' — camera not reachable; check the IP address and that RTSP is turned on.';
  return e;
}
function camState(c) {
  if (!c.enabled) return {cls: 'disabled', text: 'Disabled'};
  if (c.streams && c.streams.main && c.streams.main.state === 'retrying') return {cls: 'retrying', text: 'Offline'};
  const rec = c.recorder || c.buffer;
  const st = rec ? rec.state : null;
  if (st === 'connecting') return {cls: 'connecting', text: 'Connecting'};
  if (st === 'retrying') return {cls: 'retrying', text: 'Offline'};
  if (rec && rec.paused) return {cls: 'idle', text: 'Paused'};
  if (c.recorder && c.recorder.state === 'recording') return {cls: 'recording', text: 'REC'};
  if (c.buffer && c.buffer.state === 'recording') {
    return c.detect && c.detect.event_active ? {cls: 'recording', text: 'REC'} : {cls: 'armed', text: 'Armed'};
  }
  if (c.record_mode === 'off') return {cls: 'idle', text: 'Live'};
  if (rec && !rec.scheduled) return {cls: 'idle', text: 'Off schedule'};
  return {cls: 'idle', text: 'Idle'};
}
const pillHtml = (c) => { const s = camState(c); return `<span class="pill st-${s.cls}"><span class="dot"></span>${esc(s.text)}</span>`; };
const MODE_TEXT = {continuous: 'HD 24/7', continuous_sub: 'Sub-stream 24/7', sub_plus_events: 'Sub-stream 24/7 + HD alerts', events: 'HD on alerts only', off: 'Not recording'};

// ======================================================================
// Live video over one WebSocket
// ======================================================================
const Live = {
  ws: null, wanted: [], imgs: new Map(), urls: new Map(), stats: new Map(), connected: false,
  key: (camId, q) => camId + '|' + q,
  connect() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${proto}//${location.host}${location.pathname.replace(/[^/]*$/, '')}api/live/ws`);
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => { this.connected = true; this.send(); };
    ws.onclose = () => { this.connected = false; setTimeout(() => this.connect(), 2000); };
    ws.onmessage = (ev) => this.onFrame(ev.data);
    this.ws = ws;
  },
  onFrame(buf) {
    const b = new Uint8Array(buf);
    dsStats.bytes += b.length;
    const now = performance.now();
    if (now - dsStats.since > 5000) { dsStats.rate = dsStats.bytes * 1000 / (now - dsStats.since); dsStats.bytes = 0; dsStats.since = now; applyDataSaver(); }
    const q = b[0] ? 'main' : 'sub';
    const n = b[1];
    const id = new TextDecoder().decode(b.subarray(2, 2 + n));
    const url = URL.createObjectURL(new Blob([b.subarray(2 + n)], {type: 'image/jpeg'}));
    const k = this.key(id, q);
    const old = this.urls.get(k);
    this.urls.set(k, url);
    if (old) setTimeout(() => URL.revokeObjectURL(old), 2000);
    const st = this.stats.get(k) || {n: 0, t: performance.now(), fps: 0, last: 0};
    st.n++; st.last = performance.now();
    if (st.last - st.t > 2000) { st.fps = st.n * 1000 / (st.last - st.t); st.n = 0; st.t = st.last; }
    this.stats.set(k, st);
    for (const img of this.imgs.get(k) || []) { img.src = url; if (q === 'main') img.classList.remove('sd-fill'); }
    if (q === 'main' && id === focusId && typeof hideFocusLoading === 'function') hideFocusLoading();
    // The HD image also shows sub frames until its first HD frame arrives.
    if (q === 'sub') for (const img of this.imgs.get(this.key(id, 'main')) || []) if (!this.urls.has(this.key(id, 'main'))) { img.src = url; img.classList.add('sd-fill'); }
  },
  fps(id, q) { const s = this.stats.get(this.key(id, q)); return s && performance.now() - s.last < 3000 ? s.fps : 0; },
  // Show a camera in an <img>; returns an unbind function.
  bind(img, id, q) {
    const k = this.key(id, q);
    if (!this.imgs.has(k)) this.imgs.set(k, new Set());
    this.imgs.get(k).add(img);
    const hd = this.urls.get(k), cur = hd || (q === 'main' && this.urls.get(this.key(id, 'sub')));
    // The low-resolution stand-in is stretched to the size the HD picture will have.
    img.classList.toggle('sd-fill', !hd && !!cur);
    if (cur) img.src = cur;
    return () => { const s = this.imgs.get(k); if (s) s.delete(img); };
  },
  setWanted(subs) {
    const j = JSON.stringify(subs);
    if (j === JSON.stringify(this.wanted)) return;
    this.wanted = subs; this.send();
  },
  send() { if (this.connected) this.ws.send(JSON.stringify({subs: document.hidden ? [] : this.wanted})); },
};
document.addEventListener('visibilitychange', () => Live.send());

// Which streams the page needs right now. The dashboard keeps running in the
// background on every page so switching back or opening a camera is instant.
// Which stream a picture shows: Stream 1 (main, full quality) or Stream 2
// (sub-stream). `ctx` is 'grid' (tiles) or 'focus' (an opened camera, or
// one enlarged on the video wall). The viewer can switch each one with the
// "S1/S2" button on the picture; the choice is remembered in this browser.
const streamPref = store_get('stream-pref', {});
function feedQ(c, ctx) {
  const p = streamPref[c.id + '|' + ctx];
  if (p === 'main' || p === 'sub') return p;
  return ctx === 'focus' || (c.view && c.view.grid_quality === 'main') ? 'main' : 'sub';
}
function setFeedQ(c, ctx, q) {
  streamPref[c.id + '|' + ctx] = q;
  try { localStorage.setItem('nvr-stream-pref', JSON.stringify(streamPref)); } catch (e) {}
}
function updateSubscriptions() {
  if (!status) return;
  const l = status.live, subs = [];
  for (const c of status.cameras) {
    if (!c.enabled) continue;
    const v = c.view || {};
    // Data saver: at most 2 pictures a second in the grid and 4 when a camera is open.
    const gridFps = dataSaver ? Math.min(2, v.grid_fps || l.fps) : (v.grid_fps || l.fps);
    const hdFps = dataSaver ? Math.min(4, v.hd_fps || l.hd_fps) : (v.hd_fps || l.hd_fps);
    const focused = page === 'live' && ((focusId === c.id && !focusPlayback) || (wall && wallSolo === c.id));
    // Data saver: only pictures that are on screen (the grid isn't kept
    // running in the background).
    const shown = page === 'live' && !focusId
      || (page === 'settings' && typeof section !== 'undefined' && section === 'cameras')
      || ($('cam-dialog').open && editingId === c.id);
    const want = new Map();
    const add = (q, fps) => want.set(q, Math.max(want.get(q) || 0, fps));
    const gridQ = feedQ(c, 'grid'), focusQ = feedQ(c, 'focus');
    // The grid keeps running in the background so switching back is instant.
    if (!dataSaver || shown) add(gridQ, focused && gridQ === focusQ ? hdFps : gridFps);
    if (focused) {
      add(focusQ, hdFps);
      // The sub-stream stands in while the full-quality picture loads.
      if (focusQ === 'main' && !dataSaver) add('sub', gridFps);
    } else if (gridQ === 'sub' && l.preload_hd_streams && !dataSaver) add('main', 1);
    for (const [q, fps] of want) subs.push({camera: c.id, q, fps});
  }
  if (dataSaver) subs.forEach(x => x.lite = true);
  Live.setWanted(subs);
}

// Overlays (name, clock, fps, status, fullscreen) for a live surface.
// Little symbols for what was detected (also used on timelines).
const LABEL_ICON = {person: '🚶', car: '🚗', truck: '🚚', bus: '🚌', motorcycle: '🏍️', bicycle: '🚲', dog: '🐕', cat: '🐈', bird: '🐦',
  horse: '🐎', cow: '🐄', sheep: '🐑', bear: '🐻', backpack: '🎒', suitcase: '🧳', handbag: '👜', umbrella: '☂️', motion: '〰️'};
const labelIcon = (l) => LABEL_ICON[l] || '🔷';
// What the timeline colours and symbols mean.
function legendHtml(kind) {
  const objs = ['person', 'car', 'truck', 'bus', 'motorcycle', 'bicycle', 'dog', 'cat', 'bird'];
  const bars = kind === 'tl'
    ? [['<i class="lg-bar rec"></i>', 'Recording'], ['<i class="lg-bar clip"></i>', 'HD alert clip'], ['<i class="lg-bar ev"></i>', 'Alert (AI)'], ['<i class="lg-bar mo"></i>', 'Motion only']]
    : [['<i class="lg-bar rec"></i>', 'Recording'], ['<i class="lg-bar clip"></i>', 'HD alert clip'], ['<i class="lg-tick"></i>', 'Motion only']];
  return bars.map(([i, t]) => `<span>${i}${t}</span>`).join('') + '<span class="lg-sep"></span>'
    + objs.map(o => `<span>${labelIcon(o)} ${o}</span>`).join('') + `<span>${labelIcon('?')} other</span>`;
}
document.querySelectorAll('[data-legend]').forEach(el => el.innerHTML = legendHtml(el.dataset.legend));
// On-screen display: the camera's own settings, or the global ones.
function osdFor(c) {
  if (c.osd && c.osd.custom) return c.osd;
  const d = status.display;
  return {show_name: d.show_name, name_corner: 'bl', show_clock: d.show_clock, clock_corner: d.clock_corner, clock_format: d.clock_format,
    show_status: true, status_corner: 'tr', show_detections: true, detections_corner: 'bl', show_fps: d.show_fps, fps_corner: 'tl', text: '', text_corner: 'br'};
}
function overlayHtml(c, q) {
  const d = status.display, o = osdFor(c);
  const parts = {tl: [], tr: [], bl: [], br: []};
  if (o.show_name) parts[o.name_corner].push(`<span class="ovtext name">${esc(c.name)}</span>`);
  if (o.text) parts[o.text_corner].push(`<span class="ovtext">${esc(o.text)}</span>`);
  if (o.show_status) parts[o.status_corner].push(`<span class="ov-pill">${pillHtml(c)}</span>`);
  if (o.show_detections) parts[o.detections_corner].push('<span class="ov-alert"></span>');
  if (o.show_clock) parts[o.clock_corner].push(`<span class="ovtext ov-clock" data-fmt="${esc(o.clock_format || d.clock_format || '')}">${esc(fmtDate(new Date(), o.clock_format || d.clock_format || undefined))}</span>`);
  if (o.show_fps) parts[o.fps_corner].push('<span class="ovtext ov-fps"></span>');
  // Stream 1 / Stream 2 switch, next to the first fullscreen button (or top right).
  const sc = d.fullscreen_corners[0] || 'tr';
  parts[sc].push(`<button type="button" class="fsbtn sbtn" title="${q === 'main' ? 'Showing stream 1 (main, full quality). Click for stream 2 (sub-stream, lighter).' : 'Showing stream 2 (sub-stream). Click for stream 1 (main, full quality).'}">${q === 'main' ? 'S1' : 'S2'}</button>`);
  for (const corner of d.fullscreen_corners) parts[corner].push('<button type="button" class="fsbtn" title="Fullscreen">⛶</button>');
  return Object.entries(parts).filter(([, v]) => v.length).map(([k, v]) => `<div class="ov ${k}">${v.join('')}</div>`).join('');
}
function wireOverlay(surface) {
  surface.querySelectorAll('.fsbtn:not(.sbtn)').forEach(b => b.onclick = (e) => { e.stopPropagation(); fullscreen(surface); });
  surface.querySelectorAll('.sbtn').forEach(b => {
    b.onpointerdown = (e) => e.stopPropagation();
    b.onclick = (e) => { e.stopPropagation(); switchStream(surface); };
  });
}
// Flip a live picture between stream 1 and stream 2.
function switchStream(surface) {
  const isFocus = surface.id === 'focus-surface';
  const c = cam(isFocus ? focusId : surface.dataset.id);
  if (!c) return;
  const ctx = isFocus || (wall && wallSolo === c.id) ? 'focus' : 'grid';
  const q = feedQ(c, ctx) === 'main' ? 'sub' : 'main';
  setFeedQ(c, ctx, q);
  if (isFocus) bindFocusImage(c); else renderDash();
  updateSubscriptions();
  toast(`${c.name}: ${q === 'main' ? 'stream 1 (main, full quality)' : 'stream 2 (sub-stream)'}`);
}
function updateOverlay(surface, c, q) {
  // Rebuild when the camera's or the global on-screen display settings changed.
  const sig = JSON.stringify([osdFor(c), status.display.fullscreen_corners, c.name, q]);
  if (surface.dataset.osd !== sig) {
    surface.querySelectorAll(':scope > .ov').forEach(o => o.remove());
    surface.insertAdjacentHTML('beforeend', overlayHtml(c, q));
    wireOverlay(surface);
    surface.dataset.osd = sig;
  }
  const clock = surface.querySelector('.ov-clock');
  if (clock) clock.textContent = fmtDate(new Date(), clock.dataset.fmt || undefined);
  const p = surface.querySelector('.ov-pill');
  if (p) p.innerHTML = pillHtml(c);
  const fps = surface.querySelector('.ov-fps');
  if (fps) {
    const info = c.info && (q === 'main' ? c.info.main : c.info.sub);
    const shown = Live.fps(c.id, q) || (q === 'main' ? Live.fps(c.id, 'sub') : 0);
    fps.textContent = (shown ? `${shown.toFixed(1)} fps` : '– fps') + (info ? ` · ${info.width}×${info.height} ${String(info.codec).toUpperCase()}${info.fps ? ` (camera ${info.fps} fps)` : ''}` : '');
  }
  const a = surface.querySelector('.ov-alert');
  if (a) {
    const labels = c.detect && c.detect.event_active ? c.detect.active_labels : [];
    a.innerHTML = labels.length ? `<span class="alertbadge">${labels.map(l => labelIcon(l.label) + ' ' + esc(l.label)).join(', ')}</span>`
      : (c.detect && c.detect.motion ? `<span class="alertbadge" style="background:rgba(245,158,11,.85)">${labelIcon('motion')} motion</span>` : '');
  }
  surface.classList.toggle('event', !!(c.detect && c.detect.event_active));
}

// ======================================================================
// Routing & status
// ======================================================================
const PAGES = ['live', 'recordings', 'system', 'logs', 'settings'];
function route() {
  const [p, sub, extra] = (location.hash.slice(1) || 'live').split('/');
  page = PAGES.includes(p) ? p : 'live';
  if (typeof wall !== 'undefined' && wall && (page !== 'live' || sub)) exitWall();
  document.querySelectorAll('nav a').forEach(a => a.classList.toggle('active', a.dataset.page === page));
  for (const s of PAGES) $('page-' + s).hidden = s !== page;
  if (page !== 'recordings') { pauseAllPlayback(); if ($('page-recordings').classList.contains('rec-fs')) recFullscreen(false); }
  if (page === 'live') showLive(sub ? decodeURIComponent(sub) : null);
  else if (page === 'recordings') showRecordings(sub ? decodeURIComponent(sub) : null, extra);
  else if (page === 'system') showSystem();
  else if (page === 'logs') showLogs();
  else showSettings(sub || 'cameras');
  updateSubscriptions();
}
window.addEventListener('hashchange', route);

async function refreshStatus() {
  try { status = await api('api/status'); } catch (e) { return; }
  $('nvr-name').textContent = status.name;
  document.title = status.name;
  const pr = status.problems, b = $('log-badge');
  b.hidden = !(pr.errors || pr.warnings);
  b.textContent = pr.errors || pr.warnings;
  b.className = pr.errors ? 'badge' : 'badge warn';
  b.title = `${pr.errors} errors, ${pr.warnings} warnings in the last hour`;
  updateSubscriptions();
  dashSync();
  if (page === 'live' && focusId) updateFocus();
  if (page === 'settings') settingsTick();
}

// Show sprite picture `n` (scrubbing preview) as the background of `el`
// (W×H px), centered and letterboxed. Sprites come in pages of pictures.
function spriteStyle(el, sp, n, W, H) {
  const i = sp.info;
  n = Math.min(i.count - 1, Math.max(0, n));
  const page = Math.floor(n / i.per_page), k = n % i.per_page;
  const col = k % i.columns, row = Math.floor(k / i.columns), rows = i.per_page / i.columns;
  const z = Math.min(W / i.tile_width, H / i.tile_height);
  const ox = (W - i.tile_width * z) / 2, oy = (H - i.tile_height * z) / 2;
  el.style.backgroundImage = `url("${sp.url}${page}")`;
  el.style.backgroundSize = `${i.columns * i.tile_width * z}px ${rows * i.tile_height * z}px`;
  el.style.backgroundPosition = `${ox - col * i.tile_width * z}px ${oy - row * i.tile_height * z}px`;
  return {ox, oy};
}
function preloadSprite(sp) {
  if (!sp) return sp;
  for (let p = 0; p < sp.info.pages; p++) new Image().src = sp.url + p;
  return sp;
}

// ======================================================================
// Shared recording player: custom scrubber with frame previews, speed,
// clip start/end handles and clip download, auto-advance to next file.
// ======================================================================
// URL that plays a recording converted to H.264 on the fly (for browsers that
// can't play the camera's format, usually H.265), or null.
function convertedUrl(url) {
  const m = String(url).match(/^media\/([^/]+)\/(rec|events)\/([^?]+)/);
  return m ? `media/${m[1]}/play/${m[2] === 'events' ? 'clip' : 'rec'}/${m[3]}` : null;
}
// URL of the converted stream starting at `t` seconds (small and light with the data saver).
function convertedAt(url, t) { return convertedUrl(url) + '?t=' + t.toFixed(2) + (dataSaver ? '&lite=true' : ''); }

// ---------- data saver (remote viewing) ----------
// Auto: on when this browser connects from outside the home network
// (Tailscale, the internet). Smaller live pictures, fewer frames per second,
// and recordings streamed as small low-bitrate video.
let dataSaver = false, dsMode = store_get('data-saver', 'auto'), dsRemote = false;
const dsStats = {bytes: 0, since: performance.now(), rate: 0};
function applyDataSaver() {
  const was = dataSaver;
  dataSaver = dsMode === 'on' || (dsMode === 'auto' && dsRemote);
  const b = $('ds-btn');
  if (b) {
    b.classList.toggle('on', dataSaver);
    const mode = {auto: 'Auto', on: 'On', off: 'Off'}[dsMode];
    $('ds-label').textContent = window.innerWidth > 760 ? `Data saver: ${mode}${dsMode === 'auto' ? (dataSaver ? ' (on)' : ' (off)') : ''}` : 'Saver';
    b.title = `Data saver: ${mode}${dsMode === 'auto' ? ` — ${dsRemote ? 'away from home, so it is on' : 'on the home network, so it is off'}` : ''}.\n`
      + 'Smaller live pictures, fewer frames per second and light video for recordings, for slow or mobile connections.\nClick to switch Auto → On → Off.'
      + (dsStats.rate ? `\nLive video now: ${fmtRate(dsStats.rate)}` : '');
  }
  if (was !== dataSaver && typeof updateSubscriptions === 'function') updateSubscriptions();
}
function fmtRate(bps) { const m = bps * 8 / 1e6; return m >= 1 ? m.toFixed(1) + ' Mbit/s' : Math.round(bps * 8 / 1000) + ' kbit/s'; }
async function detectRemote() {
  try { dsRemote = !!(await api('api/client')).remote; } catch (e) {}
  applyDataSaver();
}
// Cameras whose recordings this browser can't play directly.
const noNativePlayback = new Set(store_get('no-native', []));
function store_get(k, d) { try { const v = localStorage.getItem('nvr-' + k); return v === null ? d : JSON.parse(v); } catch (e) { return d; } }
function markNoNative(camId) {
  noNativePlayback.add(camId);
  try { localStorage.setItem('nvr-no-native', JSON.stringify([...noNativePlayback])); } catch (e) {}
}

class Player {
  constructor(video, controls, msgEl) {
    this.video = video; this.box = controls; this.msg = msgEl;
    this.item = null; this.sprite = null; this.inT = null; this.outT = null; this.marks = [];
    // tc: playing a converted stream that starts at `base` seconds into the file.
    this.tc = false; this.base = 0;
    // A second, hidden player loads the next file near the end, so moving to
    // it doesn't blank the picture.
    this.alt = video.cloneNode(false);
    this.alt.removeAttribute('id');
    Object.assign(this.alt.style, {position: 'absolute', inset: '0', visibility: 'hidden'});
    video.after(this.alt);
    this.nextSeg = null;
    for (const v of [video, this.alt]) this.wire(v);
  }
  wire(video) {
    const on = (ev, f) => video.addEventListener(ev, (e) => { if (video === this.video) f(e); });
    video.controls = false;
    on('timeupdate', () => { this.paint(); this.prepareNext(); });
    on('play', () => this.paintButtons());
    on('pause', () => this.paintButtons());
    on('loadedmetadata', () => this.paint());
    on('ended', () => this.next());
    on('error', () => {
      if (!video.getAttribute('src') || !this.item) return;
      if (!this.tc && convertedUrl(this.item.url)) return this.useConverted();
      if (!this.msg) return;
      this.msg.hidden = false;
      this.msg.textContent = 'This recording could not be played. Use Download and play it in VLC.';
    });
    // Some browsers "play" unsupported video as sound only: no picture size.
    on('loadeddata', () => {
      if (!this.tc && this.item && video.videoWidth === 0 && convertedUrl(this.item.url)) this.useConverted();
    });
    on('playing', () => { if (this.msg) this.msg.hidden = true; });
    video.addEventListener('click', singleClick(() => { if (video === this.video) video.paused ? video.play() : video.pause(); }));
  }
  setHidden(h) { for (const v of [this.video, this.alt]) v.hidden = h; }
  // In the last seconds of a file, find and load the next one in the hidden player.
  async prepareNext() {
    if (!this.item || this.item.kind === 'clip' || this.tc || this.preparing) return;
    const left = this.dur() - this.cur();
    if (!(left > 0 && left < 8)) return;
    if (this.nextSeg && this.nextSeg.after === this.item.name) return;
    this.preparing = true;
    try {
      const end = new Date(new Date(this.item.file_start).getTime() + this.dur() * 1000 - 2000);
      const [t] = await api(`api/timeline?cameras=${enc(this.item.camera)}&from=${enc(end.toISOString())}&to=${enc(new Date(end.getTime() + 86400000).toISOString())}`);
      const seg = t && t.segments.find(x => x.kind === 'recording' && x.name !== this.item.name && new Date(x.start) >= end - 5000);
      this.nextSeg = {after: this.item.name, seg};
      if (seg) { this.alt.preload = 'auto'; this.alt.src = seg.url; this.alt.dataset.url = seg.url; }
    } catch (e) {} finally { this.preparing = false; }
  }
  build() {
    this.box.innerHTML = `
      <div class="scrub"><div class="track"></div><div class="sel" hidden></div><div class="fill"></div>
        <div class="handle in" hidden title="Clip start"></div><div class="handle out" hidden title="Clip end"></div>
        <div class="head"></div><div class="preview" hidden><div class="img"></div><div class="t"></div></div></div>
      <div class="controls">
        <button type="button" class="p-play primary">▶</button>
        <button type="button" class="p-back" title="Back 10 s">−10s</button>
        <button type="button" class="p-fwd" title="Forward 10 s">+10s</button>
        <select class="p-speed"><option value="0.5">0.5×</option><option value="1" selected>1×</option><option value="2">2×</option><option value="4">4×</option><option value="8">8×</option><option value="16">16×</option></select>
        <span class="timecode p-time"></span>
        <span class="spacer"></span>
        <button type="button" class="p-in" title="Mark the clip start at the current position">[ Start</button>
        <button type="button" class="p-out" title="Mark the clip end at the current position">End ]</button>
        <button type="button" class="p-clip" hidden>⇩ Download clip</button>
        <a class="btn p-dl" title="Download the whole file">⇩ File</a>
        <button type="button" class="p-mute" title="Sound">🔊</button>
        <button type="button" class="p-fs" title="Fullscreen">⛶</button>
      </div>`;
    const q = (s) => this.box.querySelector(s);
    this.el = {scrub: q('.scrub'), fill: q('.fill'), head: q('.head'), sel: q('.sel'), hin: q('.handle.in'), hout: q('.handle.out'),
      prev: q('.preview'), pimg: q('.preview .img'), pt: q('.preview .t'), time: q('.p-time'), play: q('.p-play'), clip: q('.p-clip')};
    q('.p-play').onclick = () => this.video.paused ? this.video.play() : this.video.pause();
    q('.p-back').onclick = () => this.seekTo(Math.max(0, this.cur() - 10));
    q('.p-fwd').onclick = () => this.seekTo(Math.min(this.dur(), this.cur() + 10));
    q('.p-speed').onchange = (e) => this.video.playbackRate = +e.target.value;
    q('.p-in').onclick = () => { this.inT = this.cur(); if (this.outT != null && this.outT <= this.inT) this.outT = null; this.paint(); };
    q('.p-out').onclick = () => { this.outT = this.cur(); if (this.inT == null || this.inT >= this.outT) this.inT = Math.max(0, this.outT - 30); this.paint(); };
    q('.p-mute').onclick = (e) => { this.video.muted = !this.video.muted; e.target.textContent = this.video.muted ? '🔇' : '🔊'; };
    q('.p-fs').onclick = () => fullscreen(this.video.parentElement);
    this.el.clip.onclick = () => this.downloadClip();
    this.dl = q('.p-dl');
    this.wireScrub();
  }
  dur() {
    if (this.tc) return this.sprite ? this.sprite.info.duration : this.base + (isFinite(this.video.duration) ? this.video.duration : 0);
    const d = this.video.duration; return isFinite(d) && d > 0 ? d : (this.sprite ? this.sprite.info.duration : 0);
  }
  // Position in the recording, in seconds.
  cur() { return this.base + this.video.currentTime; }
  seekTo(t, play) {
    if (!this.tc) { this.video.currentTime = t; if (play) this.video.play().catch(() => {}); return; }
    const wasPlaying = play ?? !this.video.paused;
    this.base = Math.max(0, t);
    this.video.src = convertedAt(this.item.url, this.base);
    this.video.playbackRate = +this.box.querySelector('.p-speed').value;
    if (wasPlaying) this.video.play().catch(() => {});
    this.paint();
  }
  // Switch to the converted stream, keeping the position.
  useConverted() {
    const t = this.tc ? this.cur() : (this.video.currentTime || this.item.offset || 0);
    this.tc = true;
    markNoNative(this.item.camera);
    if (this.msg) { this.msg.hidden = false; this.msg.textContent = 'Converting for this browser…'; }
    this.seekTo(t, true);
  }
  wallAt(t) { return new Date(new Date(this.item.file_start).getTime() + t * 1000); }
  async load(item, autoplay = true) {
    if (!this.el) this.build();
    this.item = item; this.inT = this.outT = null; this.sprite = null;
    if (this.msg) this.msg.hidden = true;
    const start = item.offset || 0;
    // Known not to play here: go straight to the converted stream.
    this.tc = (dataSaver || noNativePlayback.has(item.camera)) && !!convertedUrl(item.url);
    this.base = 0;
    if (this.tc) {
      this.seekTo(start, autoplay);
    } else {
      this.video.src = item.url;
      this.video.playbackRate = +this.box.querySelector('.p-speed').value;
      const seek = () => { this.video.currentTime = start; if (autoplay) this.video.play().catch(() => {}); };
      if (this.video.readyState >= 1) seek(); else this.video.addEventListener('loadedmetadata', seek, {once: true});
    }
    this.dl.href = item.url + '?download=1';
    this.marks = item.marks || [];
    this.paint();
    const kind = item.kind === 'clip' ? 'clip' : 'rec';
    try { this.sprite = preloadSprite(await api(`api/sprite/${enc(item.camera)}/${kind}/${enc(item.name)}`)); } catch (e) { this.sprite = null; }
  }
  stop() {
    for (const v of [this.video, this.alt]) { v.pause(); v.removeAttribute('src'); v.load(); }
    this.item = null; this.nextSeg = null;
  }
  paintButtons() { if (this.el) this.el.play.textContent = this.video.paused ? '▶' : '❚❚'; }
  paint() {
    if (!this.el || !this.item) return;
    const d = this.dur() || 1, t = this.cur(), pct = (x) => Math.min(100, Math.max(0, x / d * 100)) + '%';
    this.el.fill.style.width = pct(t);
    this.el.head.style.left = pct(t);
    this.el.time.textContent = `${fmtTime(this.wallAt(t))} · ${fmtClock(t)} / ${fmtClock(d)}`;
    const hasSel = this.inT != null && this.outT != null;
    this.el.hin.hidden = this.inT == null; this.el.hout.hidden = this.outT == null; this.el.sel.hidden = !hasSel;
    if (this.inT != null) this.el.hin.style.left = pct(this.inT);
    if (this.outT != null) this.el.hout.style.left = pct(this.outT);
    if (hasSel) { this.el.sel.style.left = pct(this.inT); this.el.sel.style.width = `calc(${pct(this.outT)} - ${pct(this.inT)})`; }
    this.el.clip.hidden = !hasSel;
    if (hasSel) this.el.clip.textContent = `⇩ Download clip (${fmtClock(this.outT - this.inT)})`;
    this.el.scrub.querySelectorAll('.mark').forEach(m => m.remove());
    for (const m of this.marks) {
      const x = (new Date(m).getTime() - new Date(this.item.file_start).getTime()) / 1000;
      if (x >= 0 && x <= d) { const e = document.createElement('div'); e.className = 'mark'; e.style.left = pct(x); this.el.scrub.appendChild(e); }
    }
    this.paintButtons();
  }
  showPreview(t, x) {
    const s = this.sprite, p = this.el.prev;
    p.hidden = false;
    p.style.left = Math.min(Math.max(x, 100), this.el.scrub.clientWidth - 100) + 'px';
    this.el.pt.textContent = fmtTime(this.wallAt(t)) + ' · ' + fmtClock(t);
    if (s) spriteStyle(this.el.pimg, s, Math.floor(t / s.info.interval), 192, 108);
  }
  wireScrub() {
    const sc = this.el.scrub;
    const tAt = (e) => { const r = sc.getBoundingClientRect(); const x = Math.min(Math.max(e.clientX - r.left, 0), r.width); return [x / r.width * this.dur(), x]; };
    let drag = null, lastSeek = 0;
    sc.addEventListener('pointermove', (e) => {
      if (!this.item) return;
      const [t, x] = tAt(e);
      this.showPreview(t, x);
      if (!drag) return;
      if (drag === 'head') {
        // Seek while dragging, but not more often than the browser can keep up.
        // Seek while dragging (direct playback only; converted playback seeks on release).
        if (!this.tc && performance.now() - lastSeek > 120) { this.video.currentTime = t; lastSeek = performance.now(); }
        this.el.head.style.left = (t / this.dur() * 100) + '%';
      } else if (drag === 'in') { this.inT = Math.min(t, (this.outT ?? this.dur()) - 1); this.paint(); }
      else if (drag === 'out') { this.outT = Math.max(t, (this.inT ?? 0) + 1); this.paint(); }
    });
    sc.addEventListener('pointerleave', () => { if (!drag) this.el.prev.hidden = true; });
    sc.addEventListener('pointerdown', (e) => {
      if (!this.item) return;
      sc.setPointerCapture(e.pointerId);
      drag = e.target.classList.contains('in') ? 'in' : e.target.classList.contains('out') ? 'out' : 'head';
      if (drag === 'head') { this.wasPlaying = !this.video.paused; this.video.pause(); if (!this.tc) this.video.currentTime = tAt(e)[0]; }
    });
    const end = (e) => {
      if (!drag) return;
      if (drag === 'head') this.seekTo(tAt(e)[0], this.wasPlaying);
      drag = null; this.el.prev.hidden = true; this.paint();
    };
    sc.addEventListener('pointerup', end);
    sc.addEventListener('pointercancel', end);
  }
  downloadClip() {
    if (this.inT == null || this.outT == null) return;
    const from = this.wallAt(this.inT).toISOString(), to = this.wallAt(this.outT).toISOString();
    toast('Preparing clip…');
    location.href = `api/export?camera=${enc(this.item.camera)}&from=${enc(from)}&to=${enc(to)}`;
  }
  // Continue with the camera's next recording when a file ends.
  async next() {
    if (!this.item || this.item.kind === 'clip') return;
    // Ready in the hidden player: swap players without a gap.
    const pre = this.nextSeg && this.nextSeg.after === this.item.name && this.nextSeg.seg;
    if (pre && !this.tc && this.alt.dataset.url === pre.url && this.alt.readyState >= 2) {
      const old = this.video;
      [this.video, this.alt] = [this.alt, old];
      this.video.style.visibility = ''; old.style.visibility = 'hidden';
      this.video.playbackRate = old.playbackRate; this.video.muted = old.muted;
      this.item = {...this.item, url: pre.url, name: pre.name, file_start: pre.start, offset: 0};
      this.inT = this.outT = null; this.base = 0;
      this.video.currentTime = 0;
      this.video.play().catch(() => {});
      old.pause();
      this.dl.href = pre.url + '?download=1';
      this.paint();
      const kind = 'rec';
      api(`api/sprite/${enc(this.item.camera)}/${kind}/${enc(pre.name)}`).then(sp => { this.sprite = preloadSprite(sp); }).catch(() => { this.sprite = null; });
      return;
    }
    const end = new Date(new Date(this.item.file_start).getTime() + this.dur() * 1000 - 2000);
    try {
      const [t] = await api(`api/timeline?cameras=${enc(this.item.camera)}&from=${enc(end.toISOString())}&to=${enc(new Date(end.getTime() + 86400000).toISOString())}`);
      const seg = t && t.segments.find(s => s.kind === 'recording' && s.name !== this.item.name && new Date(s.start) >= end - 5000);
      if (seg) this.load({...this.item, url: seg.url, name: seg.name, file_start: seg.start, offset: 0});
    } catch (e) {}
  }
}

function pauseAllPlayback() {
  document.querySelectorAll('video').forEach(v => { if (!v.paused) v.pause(); });
}

// ======================================================================
// Live dashboard
// ======================================================================
let focusId = null, focusPlayback = null, editing = false, layout = [], dashIds = '';
const unbinders = new Map();
const store = {
  get: (k, d) => { try { const v = localStorage.getItem('nvr-' + k); return v === null ? d : JSON.parse(v); } catch (e) { return d; } },
  set: (k, v) => { try { localStorage.setItem('nvr-' + k, JSON.stringify(v)); } catch (e) {} },
};
// Camera group shown ('' = all cameras), video-wall state, and the camera
// enlarged on the wall (click a tile; click again to go back).
let groupId = store.get('group', ''), wall = false, wallSolo = null;

const groups = () => (status && status.dashboard.groups) || [];
const group = () => groups().find(g => g.id === groupId) || null;
function viewCams() {
  const g = group();
  if (!g) return status.cameras;
  return g.cameras.map(id => cam(id)).filter(Boolean);
}
const layoutKey = () => wall ? 'fullscreen_tiles' : 'tiles';

// Grid layout: 12 columns. Normal view: rows are 16:9 for square units.
// Video wall: the layout is scaled to exactly fill the screen.
function defaultLayout(cams) {
  const n = cams.length;
  if (!n) return [];
  if (!wall) {
    const cols = n <= 1 ? 1 : n <= 4 ? 2 : n <= 9 ? 3 : 4, w = 12 / cols;
    return cams.map((c, i) => ({camera: c.id, x: (i % cols) * w, y: Math.floor(i / cols) * w, w, h: w}));
  }
  // Pick the columns × rows that give each 16:9 picture the most screen.
  const W = window.innerWidth, H = window.innerHeight;
  let best = null;
  for (const c of [1, 2, 3, 4, 6]) for (const r of [1, 2, 3, 4, 6]) {
    if (c * r < n) continue;
    const tw = W / c, th = H / r, vw = Math.min(tw, th * 16 / 9), vh = vw * 9 / 16;
    const score = vw * vh - (c * r - n) * 1; // prefer fewer empty cells on ties
    if (!best || score > best.score + 1) best = {c, r, score};
  }
  const w = 12 / best.c, h = 12 / best.r;
  return cams.map((c, i) => ({camera: c.id, x: (i % best.c) * w, y: Math.floor(i / best.c) * h, w, h}));
}
function currentLayout() {
  const cams = viewCams(), owner = group() || status.dashboard;
  const saved = (owner[layoutKey()] || []).filter(t => cams.some(c => c.id === t.camera)).map(t => ({...t}));
  if (!saved.length) return defaultLayout(cams);
  const missing = cams.filter(c => !saved.some(t => t.camera === c.id));
  if (missing.length && wall) return defaultLayout(cams);
  let maxY = saved.reduce((m, t) => Math.max(m, t.y + t.h), 0);
  for (const c of missing) { saved.push({camera: c.id, x: 0, y: maxY, w: 4, h: 4}); maxY += 4; }
  return saved;
}
const overlaps = (a, b) => a !== b && a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
// Push tiles out of the way of `moved`, then pull everything up.
function settle(tiles, moved) {
  let changed = true, guard = 0;
  while (changed && guard++ < 200) {
    changed = false;
    for (const t of tiles) {
      const hit = tiles.find(o => o !== t && overlaps(o, t) && (o === moved || o.y <= t.y));
      if (hit && t !== moved) { t.y = hit.y + hit.h; changed = true; }
    }
  }
  for (const t of [...tiles].sort((a, b) => a.y - b.y)) {
    while (t.y > 0 && !tiles.some(o => overlaps(o, {...t, y: t.y - 1}) && o !== t)) t.y--;
  }
}

function dashMetrics() {
  if (wall) {
    const W = window.innerWidth, H = window.innerHeight;
    const rows = Math.max(12, layout.reduce((m, t) => Math.max(m, t.y + t.h), 0));
    return {W, H, colW: W / 12, rowH: H / rows, narrow: false, gap: 2};
  }
  const W = $('dash').clientWidth || 800;
  return {W, colW: W / 12, rowH: W / 12 * 9 / 16, narrow: W < 640, gap: 8};
}

function renderGroupSelect() {
  const sel = $('grp-sel');
  if (groupId && !group()) groupId = '';
  sel.innerHTML = `<option value="">All cameras (${status.cameras.length})</option>` +
    groups().map(g => `<option value="${esc(g.id)}">${esc(g.name)} (${g.cameras.length})</option>`).join('');
  sel.value = groupId;
  const many = groups().length > 0;
  $('grp-prev').hidden = $('grp-next').hidden = !many;
  $('wall-cycle').parentElement.hidden = $('wall-cycle-s').hidden = !many;
}

function renderDash() {
  const dash = $('dash');
  if (!status) return;
  renderGroupSelect();
  const cams = viewCams();
  $('no-cameras').hidden = status.cameras.length > 0;
  $('dash-bar').hidden = !status.cameras.length;
  dashIds = dashKey();
  if (!editing) layout = currentLayout();
  for (const u of unbinders.values()) u();
  unbinders.clear();
  dash.innerHTML = '';
  dash.classList.toggle('editing', editing);
  $('live-grid').classList.toggle('editing-wall', editing && wall);
  dash.classList.remove('fit-stretch', 'fit-crop', 'fit-fit');
  dash.classList.add('fit-' + (status.display.tile_fit || 'stretch'));
  const shown = wallSolo && !editing ? [{camera: wallSolo, x: 0, y: 0, w: 12, h: 12}] : layout;
  for (const t of shown) {
    const c = cam(t.camera);
    if (!c) continue;
    const q = feedQ(c, wallSolo === c.id ? 'focus' : 'grid');
    const el = document.createElement('div');
    el.className = 'surface tile';
    el.dataset.id = c.id;
    el.dataset.q = q;
    el.innerHTML = `<img alt=""><div class="msg">${c.enabled ? 'Connecting…' : 'Camera disabled'}</div>${overlayHtml(c, q)}${editing ? '<div class="rz"></div>' : ''}`;
    const img = el.querySelector('img');
    img.onload = () => { el.querySelector('.msg').hidden = true; };
    unbinders.set(c.id, Live.bind(img, c.id, q));
    wireOverlay(el);
    el.onclick = singleClick(() => {
      if (editing || document.fullscreenElement === el) return;
      if (wall) { wallSolo = wallSolo ? null : c.id; renderDash(); updateSubscriptions(); }
      else location.hash = '#live/' + enc(c.id);
    });
    if (editing) wireTileEdit(el, t);
    dash.appendChild(el);
  }
  placeTiles();
  const g = group();
  $('dash-info').textContent = editing
    ? (wall ? 'Full-screen layout: drag to move, drag corners to resize' : 'Editing the layout — drag to move, drag the corner to resize')
    : cams.length ? `${cams.length} camera${cams.length > 1 ? 's' : ''}` + (g ? '' : '') : (g ? 'No cameras in this group — use Groups… to add some.' : '');
  toggleEditButtons();
}
function dashKey() {
  return status.cameras.map(c => c.id + ':' + c.name + ':' + c.enabled + JSON.stringify(c.osd) + JSON.stringify(c.view)).join(',') + JSON.stringify(status.display) + JSON.stringify(status.dashboard) + groupId + wall + wallSolo;
}

function placeTiles() {
  const m = dashMetrics(), dash = $('dash'), g = m.gap / 2;
  let maxB = 0, y = 0;
  for (const el of dash.querySelectorAll('.tile')) {
    const t = wallSolo && !editing ? {x: 0, y: 0, w: 12, h: 12} : layout.find(x => x.camera === el.dataset.id);
    if (!t) continue;
    if (m.narrow && !editing) {
      const h = (m.W - 8) * 9 / 16;
      Object.assign(el.style, {left: '4px', top: y + 4 + 'px', width: m.W - 8 + 'px', height: h + 'px'});
      y += h + 8; maxB = y;
    } else {
      const rowH = wallSolo && !editing ? m.H / 12 : m.rowH;
      Object.assign(el.style, {left: t.x * m.colW + g + 'px', top: t.y * rowH + g + 'px', width: t.w * m.colW - m.gap + 'px', height: t.h * rowH - m.gap + 'px'});
      maxB = Math.max(maxB, (t.y + t.h) * rowH);
    }
  }
  dash.style.height = wall ? '' : maxB + 'px';
}
window.addEventListener('resize', () => { if (page === 'live' && !focusId) placeTiles(); });

function wireTileEdit(el, t) {
  el.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    const m = dashMetrics(), resize = e.target.classList.contains('rz');
    const start = {x: e.clientX, y: e.clientY, tx: t.x, ty: t.y, tw: t.w, th: t.h};
    const ghost = document.createElement('div'); ghost.className = 'ghost'; $('dash').appendChild(ghost);
    el.classList.add('dragging');
    el.setPointerCapture(e.pointerId);
    const calc = (ev) => {
      const dx = (ev.clientX - start.x) / m.colW, dy = (ev.clientY - start.y) / m.rowH;
      if (resize) return {x: t.x, y: t.y, w: Math.max(1, Math.min(12 - t.x, Math.round(start.tw + dx))), h: Math.max(1, Math.round(start.th + dy))};
      return {x: Math.max(0, Math.min(12 - t.w, Math.round(start.tx + dx))), y: Math.max(0, Math.round(start.ty + dy)), w: t.w, h: t.h};
    };
    const move = (ev) => {
      const gr = calc(ev);
      Object.assign(ghost.style, {left: gr.x * m.colW + 'px', top: gr.y * m.rowH + 'px', width: gr.w * m.colW + 'px', height: gr.h * m.rowH + 'px'});
      if (resize) { el.style.width = (start.tw * m.colW - m.gap + ev.clientX - start.x) + 'px'; el.style.height = (start.th * m.rowH - m.gap + ev.clientY - start.y) + 'px'; }
      else { el.style.left = start.tx * m.colW + m.gap / 2 + ev.clientX - start.x + 'px'; el.style.top = start.ty * m.rowH + m.gap / 2 + ev.clientY - start.y + 'px'; }
    };
    const up = (ev) => {
      el.removeEventListener('pointermove', move); el.removeEventListener('pointerup', up);
      Object.assign(t, calc(ev));
      ghost.remove(); el.classList.remove('dragging');
      settle(layout, t);
      placeTiles();
    };
    el.addEventListener('pointermove', move); el.addEventListener('pointerup', up);
  });
}

async function saveDashboard(d) {
  await api('api/dashboard', {method: 'PUT', json: d});
  status.dashboard = d;
}
$('dash-edit').onclick = () => { wallSolo = null; editing = true; renderDash(); };
$('dash-cancel').onclick = () => { editing = false; renderDash(); };
$('dash-auto').onclick = () => { layout = defaultLayout(viewCams()); renderDash(); };
$('dash-save').onclick = async () => {
  const d = JSON.parse(JSON.stringify(status.dashboard));
  const owner = groupId ? d.groups.find(g => g.id === groupId) : d;
  owner[layoutKey()] = layout.map(t => ({...t}));
  try { await saveDashboard(d); editing = false; renderDash(); toast(wall ? 'Full-screen layout saved' : 'Layout saved'); }
  catch (e) { toast(e.message, true); }
};
function toggleEditButtons() {
  $('dash-edit').hidden = editing || !!wallSolo;
  for (const id of ['dash-auto', 'dash-cancel', 'dash-save']) $(id).hidden = !editing;
  for (const id of ['grp-sel', 'grp-prev', 'grp-next', 'grp-manage', 'wall-btn']) $(id).disabled = editing;
  $('wall-btn').textContent = wall ? '✕ Exit full screen' : '⛶ Full screen';
}

// ---------- camera groups ----------
function setGroup(id) {
  if (editing) return;
  groupId = id; wallSolo = null;
  store.set('group', id);
  renderDash();
}
function stepGroup(dir) {
  const ids = ['', ...groups().map(g => g.id)];
  setGroup(ids[(ids.indexOf(groupId) + dir + ids.length) % ids.length]);
}
$('grp-sel').onchange = (e) => setGroup(e.target.value);
$('grp-prev').onclick = () => stepGroup(-1);
$('grp-next').onclick = () => stepGroup(1);
$('grp-manage').onclick = () => openGroupDialog();

function openGroupDialog() {
  let list = JSON.parse(JSON.stringify(groups()));
  const f = $('f-grp');
  const render = () => {
    f.innerHTML = `<div class="head"><h2>Camera groups</h2><button type="button" class="small" data-close>✕</button></div>
      <div class="body"><p class="muted" style="margin-top:0">Groups let you switch between sets of cameras on the Live page and in full screen (e.g. “Outside”, “Driveway”). Each group has its own layout.</p>
        ${list.map((g, i) => `<div class="grp-row" data-i="${i}">
          <div class="row"><input type="text" class="g-name" value="${esc(g.name)}" placeholder="Group name" style="flex:1"><button type="button" class="small" data-del="${i}">Delete</button></div>
          <div class="chips" style="margin-top:8px">${status.cameras.map(c => `<label><input type="checkbox" value="${esc(c.id)}" ${g.cameras.includes(c.id) ? 'checked' : ''}>${esc(c.name)}</label>`).join('')}</div></div>`).join('')
          || '<div class="empty" style="padding:16px">No groups yet.</div>'}
        <button type="button" id="g-add">+ New group</button>
        <div class="err" id="g-err" hidden style="margin-top:8px"></div></div>
      <div class="foot"><button type="button" data-close>Cancel</button><button class="primary">Save groups</button></div>`;
    f.querySelectorAll('[data-close]').forEach(b => b.onclick = () => $('grp-dialog').close());
    f.querySelectorAll('[data-del]').forEach(b => b.onclick = () => { collect(); list.splice(+b.dataset.del, 1); render(); });
    $('g-add').onclick = () => { collect(); list.push({id: 'g' + Date.now().toString(36), name: '', cameras: [], tiles: [], fullscreen_tiles: []}); render(); const names = f.querySelectorAll('.g-name'); names[names.length - 1].focus(); };
  };
  // Read the form back into `list`; a changed camera set resets that group's layouts.
  const collect = () => f.querySelectorAll('.grp-row').forEach(row => {
    const g = list[+row.dataset.i];
    g.name = row.querySelector('.g-name').value.trim();
    const cams = [...row.querySelectorAll('input[type=checkbox]:checked')].map(i => i.value);
    if (cams.join() !== g.cameras.join()) { g.cameras = cams; }
  });
  f.onsubmit = async (e) => {
    e.preventDefault(); collect();
    if (list.some(g => !g.name)) { $('g-err').hidden = false; $('g-err').textContent = 'Give every group a name.'; return; }
    const d = JSON.parse(JSON.stringify(status.dashboard));
    d.groups = list;
    try { await saveDashboard(d); $('grp-dialog').close(); renderDash(); toast('Groups saved'); }
    catch (err) { $('g-err').hidden = false; $('g-err').textContent = err.message; }
  };
  render();
  $('grp-dialog').showModal();
}

// ---------- full-screen video wall ----------
async function enterWall() {
  const el = $('live-grid');
  wall = true; wallSolo = null;
  el.classList.add('wall');
  try { if (el.requestFullscreen) await el.requestFullscreen({navigationUI: 'hide'}); } catch (e) { /* CSS-only full window */ }
  showWallBar();
  renderDash(); updateSubscriptions();
}
function exitWall() {
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  leaveWall();
}
function leaveWall() {
  if (!wall) return;
  wall = false; wallSolo = null; editing = false;
  $('live-grid').classList.remove('wall', 'show-bar', 'hide-cursor', 'editing-wall');
  stopCycle(); $('wall-cycle').checked = false;
  renderDash(); updateSubscriptions();
}
$('wall-btn').onclick = () => wall ? exitWall() : enterWall();
document.addEventListener('fullscreenchange', () => {
  if (!document.fullscreenElement && wall) leaveWall();
  else if (wall) placeTiles();
});
// The toolbar appears when the mouse moves and hides again after 3 s.
let wallBarTimer;
function showWallBar() {
  const el = $('live-grid');
  el.classList.add('show-bar'); el.classList.remove('hide-cursor');
  clearTimeout(wallBarTimer);
  wallBarTimer = setTimeout(() => { if (!editing) { el.classList.remove('show-bar'); el.classList.add('hide-cursor'); } }, 3000);
}
$('live-grid').addEventListener('pointermove', () => { if (wall) showWallBar(); });
document.addEventListener('keydown', (e) => {
  if (!wall || editing || e.target.matches('input, select, textarea')) return;
  if (e.key === 'ArrowRight') stepGroup(1);
  else if (e.key === 'ArrowLeft') stepGroup(-1);
  else if (e.key === 'Escape') { if (wallSolo) { wallSolo = null; renderDash(); updateSubscriptions(); } else exitWall(); }
});
let cycleTimer = null;
function stopCycle() { clearInterval(cycleTimer); cycleTimer = null; }
function startCycle() {
  stopCycle();
  if ($('wall-cycle').checked) cycleTimer = setInterval(() => { if (!editing && !wallSolo) stepGroup(1); }, +$('wall-cycle-s').value * 1000);
}
$('wall-cycle-s').value = String(store.get('cycle-s', 20));
$('wall-cycle').onchange = startCycle;
$('wall-cycle-s').onchange = () => { store.set('cycle-s', +$('wall-cycle-s').value); startCycle(); };

// Keep the dashboard current (it runs in the background on every page).
function dashSync() {
  if (!status) return;
  if (dashKey() !== dashIds && !editing) renderDash();
  for (const el of document.querySelectorAll('#dash .tile')) {
    const c = cam(el.dataset.id);
    if (c) updateOverlay(el, c, el.dataset.q || 'sub');
  }
}
setInterval(() => {
  // Clocks on overlays tick every second.
  document.querySelectorAll('.ov-clock').forEach(e => e.textContent = fmtDate(new Date(), e.dataset.fmt || undefined));
  $('clock').textContent = status ? fmtDate(new Date()) : '';
}, 1000);

// ---------- single camera ----------
let focusUnbind = null, focusPlayer = null;

function showLive(id) {
  if (!status) return;
  if (!id && status.cameras.length === 1 && !editing && !location.hash.includes('grid')) {
    // With one camera the "grid" is just that camera.
  }
  focusId = id && cam(id) ? id : null;
  $('live-grid').hidden = !!focusId;
  $('live-focus').hidden = !focusId;
  if (!focusId) { stopFocus(); renderDash(); return; }
  const c = cam(focusId);
  stopFocusPlayback(false);
  const surf = $('focus-surface');
  bindFocusImage(c);
  $('focus-snap').href = 'snapshot/' + enc(c.id);
  $('focus-recs').href = '#recordings/' + enc(c.id) + '/timeline';
  $('focus-edit').onclick = () => openCameraDialog(c.id);
  setupQuickView(c);
  setupPtz(c);
  camTl.open(c.id);
  updateFocus();
  loadActivity();
}

let loadingTimer = null;
// Show the opened camera's chosen stream in the big picture.
function bindFocusImage(c) {
  const surf = $('focus-surface'), q = feedQ(c, 'focus');
  surf.querySelectorAll(':scope > .ov').forEach(o => o.remove());
  surf.insertAdjacentHTML('beforeend', overlayHtml(c, q));
  delete surf.dataset.osd;
  wireOverlay(surf);
  if (focusUnbind) focusUnbind();
  const img = $('focus-img');
  img.removeAttribute('src');
  $('focus-msg').hidden = false; $('focus-msg').textContent = 'Connecting…';
  img.onload = () => { $('focus-msg').hidden = true; };
  focusUnbind = Live.bind(img, c.id, q);
  if (img.getAttribute('src')) $('focus-msg').hidden = true;
  // Spinner until the first full-quality frame arrives (the sub-stream
  // picture is shown meanwhile).
  if (q === 'main') showFocusLoading(); else hideFocusLoading();
}
function showFocusLoading() {
  const el = $('focus-loading');
  el.hidden = false;
  el.querySelector('span').textContent = 'Loading full quality…';
  clearTimeout(loadingTimer);
  loadingTimer = setTimeout(() => { el.querySelector('span').textContent = 'Full-quality stream not available — showing sub-stream'; setTimeout(() => el.hidden = true, 5000); }, 20000);
}
function hideFocusLoading() { clearTimeout(loadingTimer); $('focus-loading').hidden = true; }

// Quick live-view options for this camera (saved without restarting it).
function setupQuickView(c) {
  const v = c.view || {grid_quality: 'auto', grid_fps: 0, hd_fps: 0}, l = status.live;
  const fpsOpts = (cur, def) => [0, 1, 2, 5, 10, 15, 20, 25, 30].map(f => `<option value="${f}" ${f === cur ? 'selected' : ''}>${f ? f + ' fps' : 'Default (' + def + ')'}</option>`).join('');
  $('fq-quality').value = v.grid_quality || 'auto';
  $('fq-gfps').innerHTML = fpsOpts(v.grid_fps || 0, l.fps);
  $('fq-hfps').innerHTML = fpsOpts(v.hd_fps || 0, l.hd_fps);
  const save = async () => {
    const nv = {grid_quality: $('fq-quality').value, grid_fps: +$('fq-gfps').value, hd_fps: +$('fq-hfps').value};
    try { await api(`api/cameras/${enc(c.id)}/view`, {method: 'PUT', json: nv}); c.view = nv; toast('Saved for ' + c.name); updateSubscriptions(); }
    catch (e) { toast(e.message, true); }
  };
  $('fq-quality').onchange = $('fq-gfps').onchange = $('fq-hfps').onchange = save;
}

// ---------- PTZ controls ----------
function setupPtz(c) {
  const pad = $('ptz-pad');
  pad.hidden = !c.ptz;
  if (!c.ptz) return;
  const id = c.id, msg = (t, bad) => { $('ptz-msg').textContent = t; $('ptz-msg').className = bad ? 'err' : 'hint'; };
  const move = (v) => api(`api/cameras/${enc(id)}/ptz/move`, {method: 'POST', json: {pan: v[0], tilt: v[1], zoom: v[2]}}).catch(e => msg(e.message, true));
  const stop = () => api(`api/cameras/${enc(id)}/ptz/stop`, {method: 'POST'}).catch(() => {});
  pad.querySelectorAll('[data-ptz]').forEach(b => {
    const v = b.dataset.ptz.split(',').map(Number);
    let held = false;
    b.onpointerdown = (e) => { e.preventDefault(); held = true; b.setPointerCapture(e.pointerId); move(v); };
    b.onpointerup = b.onpointercancel = b.onlostpointercapture = () => { if (held) { held = false; stop(); } };
  });
  pad.querySelector('[data-ptz-home]').onclick = stop;
  const loadPresets = async () => {
    try {
      const p = await api(`api/cameras/${enc(id)}/ptz/presets`);
      $('ptz-presets').innerHTML = `<option value="">Presets (${p.length})…</option>` + p.map(x => `<option value="${esc(x.token)}">${esc(x.name)}</option>`).join('');
      msg('Press and hold to move.');
    } catch (e) { msg(e.message, true); }
  };
  $('ptz-go').onclick = () => { const t = $('ptz-presets').value; if (t) api(`api/cameras/${enc(id)}/ptz/goto`, {method: 'POST', json: {token: t}}).catch(e => msg(e.message, true)); };
  $('ptz-presets').onchange = $('ptz-go').onclick;
  $('ptz-save').onclick = async () => {
    try { await api(`api/cameras/${enc(id)}/ptz/presets`, {method: 'POST', json: {name: $('ptz-name').value}}); $('ptz-name').value = ''; toast('Preset saved'); loadPresets(); }
    catch (e) { msg(e.message, true); }
  };
  loadPresets();
}

// ---------- per-camera timeline (camera view) ----------
const camTl = {
  id: null, data: null, zoom: 21600, end: null, loadedAt: 0,
  open(id) {
    this.id = id; this.zoom = +$('ctl-zoom').value; this.end = null;
    this.load();
  },
  win() { const end = this.end || Date.now(); return [end - this.zoom * 1000, end]; },
  async load() {
    const id = this.id; if (!id) return;
    const [a, b] = this.win();
    try {
      const [t] = await api(`api/timeline?cameras=${enc(id)}&from=${enc(new Date(a).toISOString())}&to=${enc(new Date(b).toISOString())}`);
      if (id !== this.id) return;
      this.data = t || {segments: [], events: []}; this.loadedAt = Date.now();
      this.draw();
    } catch (e) {}
  },
  draw() {
    if (!this.data) return;
    const [a, b] = this.win(), span = b - a, pct = (t) => (t - a) / span * 100;
    const steps = [600, 1800, 3600, 10800, 21600, 43200, 86400].map(x => x * 1000);
    const step = steps.find(x => span / x <= 7) || 86400000;
    let axis = '';
    for (let t = Math.ceil(a / step) * step; t <= b; t += step) {
      const d = new Date(t);
      axis += `<span style="left:${pct(t)}%">${span > 86400000 * 1.5 ? fmtDay(d) : String(d.getHours()).padStart(2, '0') + ':' + String(d.getMinutes()).padStart(2, '0')}</span>`;
    }
    $('ctl-axis').innerHTML = axis;
    let lane = '';
    for (const sg of this.data.segments) {
      const x0 = Math.max(0, pct(new Date(sg.start).getTime())), x1 = Math.min(100, pct(new Date(sg.end).getTime()));
      if (x1 > 0 && x0 < 100 && x1 > x0) lane += `<div class="seg ${sg.kind === 'clip' ? 'clip' : ''}" style="left:${x0}%;width:${Math.max(0.15, x1 - x0)}%"></div>`;
    }
    // Detections: a symbol per alert; nearby alerts with the same symbol are merged.
    let last = {};
    for (const e of this.data.events) {
      const x = pct(new Date(e.start).getTime());
      if (x < 0 || x > 100) continue;
      if (!e.labels.length) { lane += `<div class="mo" style="left:${x}%" data-t="${new Date(e.start).getTime()}" title="Motion ${fmtTime(e.start)} — click to watch"></div>`; continue; }
      const icon = labelIcon(e.labels[0].label);
      if (last[icon] != null && Math.abs(last[icon] - x) < 1.2) continue;
      last[icon] = x;
      lane += `<span class="ic" style="left:${x}%" data-t="${new Date(e.start).getTime()}" title="${esc(e.labels.map(l => l.label).join(', '))} ${fmtTime(e.start)} — click to watch">${icon}</span>`;
    }
    const lb = $('ctl-lane'), pv = lb.querySelector('.preview');
    lb.innerHTML = lane + '<div class="head" hidden></div>';
    lb.appendChild(pv);
  },
  // Play the camera's footage at time T in the camera view.
  playAt(T) {
    const segs = (this.data && this.data.segments) || [];
    const hit = (sg) => new Date(sg.start).getTime() <= T && T < new Date(sg.end).getTime() + 1000;
    const sg = segs.find(x => x.kind === 'recording' && hit(x)) || segs.find(hit);
    if (!sg) { toast('No recording at that time.'); return; }
    playInFocus({camera: this.id, play: {url: sg.url, name: sg.name, kind: sg.kind, file_start: sg.start, offset: (T - new Date(sg.start).getTime()) / 1000}});
  },
};
$('ctl-zoom').onchange = () => { camTl.zoom = +$('ctl-zoom').value; camTl.load(); };
$('ctl-left').onclick = () => { camTl.end = camTl.win()[1] - camTl.zoom * 500; camTl.load(); };
$('ctl-right').onclick = () => { const e = camTl.win()[1] + camTl.zoom * 500; camTl.end = e >= Date.now() ? null : e; camTl.load(); };
$('ctl-now').onclick = () => { camTl.end = null; camTl.load(); };
(function wireCamTl() {
  const lane = $('ctl-lane');
  const tAt = (e) => { const r = lane.getBoundingClientRect(), [a, b] = camTl.win(); return [Math.min(Date.now(), a + (e.clientX - r.left) / r.width * (b - a)), e.clientX - r.left]; };
  lane.addEventListener('pointermove', async (e) => {
    if (!camTl.data) return;
    const [T, x] = tAt(e), pv = lane.querySelector('.preview');
    pv.hidden = false;
    pv.style.left = Math.min(Math.max(x, 100), lane.clientWidth - 100) + 'px';
    pv.querySelector('.t').textContent = fmtDate(new Date(T));
    const sg = camTl.data.segments.find(s => new Date(s.start).getTime() <= T && T < new Date(s.end).getTime() + 1000);
    const im = pv.querySelector('.img');
    if (!sg) { im.style.backgroundImage = ''; return; }
    const sp = await spriteFor(camTl.id, sg);
    if (sp) spriteStyle(im, sp, Math.floor((T - new Date(sg.start).getTime()) / 1000 / sp.info.interval), 192, 108);
  });
  lane.addEventListener('pointerleave', () => { lane.querySelector('.preview').hidden = true; });
  lane.addEventListener('click', (e) => {
    const icon = e.target.closest('.ic, .mo');
    camTl.playAt(icon ? +icon.dataset.t - 3000 : tAt(e)[0]);
  });
})();
setInterval(() => { if (page === 'live' && focusId && !camTl.end) camTl.load(); }, 30000);
function stopFocus() { if (focusUnbind) { focusUnbind(); focusUnbind = null; } stopFocusPlayback(false); }

function updateFocus() {
  const c = cam(focusId);
  if (!c) { location.hash = '#live'; return; }
  updateOverlay($('focus-surface'), c, feedQ(c, 'focus'));
  const r = c.recorder || c.buffer;
  const btn = $('focus-rec');
  btn.hidden = !r || !c.enabled;
  if (r) {
    btn.innerHTML = r.paused ? '<i class="bi">⏺</i><span>Resume recording</span>' : '<i class="bi">⏸</i><span>Pause recording</span>';
    btn.className = r.paused ? 'primary' : '';
    btn.onclick = async () => {
      if (!r.paused && !confirm(`Pause recording for ${c.name}?\nIt resumes automatically when the NVR restarts.`)) return;
      await api(`api/cameras/${enc(c.id)}/${r.paused ? 'resume' : 'pause'}`, {method: 'POST'}).catch(e => toast(e.message, true));
      refreshStatus();
    };
  }
  const info = c.info || {}, main = info.main, sub = info.sub;
  const st = camState(c), d = c.detect;
  const s = (x) => x ? `${x.width}×${x.height} ${String(x.codec).toUpperCase()} ${x.fps ? x.fps + ' fps' : ''}` : '–';
  $('focus-info').innerHTML = `
    <dt>Status</dt><dd>${esc(st.text === 'REC' ? 'Recording' : st.text)}</dd>
    <dt>Recording</dt><dd>${esc(MODE_TEXT[c.record_mode] || c.record_mode)}</dd>
    ${r && r.since ? `<dt>Recording for</dt><dd>${fmtDur((Date.now() - new Date(r.since)) / 1000)}</dd>` : ''}
    <dt>Main stream</dt><dd>${esc(s(main))}</dd>
    ${c.has_sub ? `<dt>Sub-stream</dt><dd>${esc(s(sub))}</dd>` : ''}
    <dt>Live view</dt><dd>${Live.fps(c.id, feedQ(c, 'focus')).toFixed(1)} fps</dd>
    <dt>Detection</dt><dd>${!c.detection ? 'Off' : d ? (d.running ? `${d.motion ? 'Motion now' : 'Watching'}${c.ai ? ' + AI' : ''}` : 'Starting…') : 'Starting…'}</dd>
    ${d && d.last_event ? `<dt>Last alert</dt><dd>${fmtDate(d.last_event)}</dd>` : ''}
    <dt>Reconnects</dt><dd>${r ? r.restarts : 0}</dd>`;
  const errs = streamErrors(c);
  if (!errs.length && r && r.last_error && r.state !== 'recording') errs.push(r.last_error);
  if (r && r.gpu_fallback) errs.push('GPU failed, using CPU: ' + (r.gpu_error || ''));
  if (d && d.ai_error) errs.push('AI: ' + d.ai_error);
  if (d && d.error) errs.push('Detection: ' + d.error);
  $('focus-err').hidden = !errs.length;
  $('focus-err').textContent = errs.join(' — ');
}

async function loadActivity() {
  const id = focusId, box = $('focus-activity');
  if (!id) return;
  const f = $('focus-filter').value;
  const ai = FILTER_TYPES.map(t => t[0]).filter(t => t !== 'motion');
  const types = f === 'ai' ? ai.join(',') : f === 'motion' ? 'motion' : ['motion', ...ai].join(',');
  const from = new Date(Date.now() - 7 * 86400000).toISOString();
  let items = [];
  try { items = await api(`api/recordings?cameras=${enc(id)}&from=${enc(from)}&limit=80${types ? '&types=' + enc(types) : ''}`); } catch (e) {}
  if (focusId !== id) return;
  box.innerHTML = items.length ? '' : '<div class="empty" style="padding:20px">No alerts in the last 7 days. Recordings are on the timeline.</div>';
  for (const it of items) {
    const row = document.createElement('div');
    row.className = 'act';
    const dur = (new Date(it.end) - new Date(it.start)) / 1000;
    row.innerHTML = `<img loading="lazy" src="${esc(it.thumb || '')}" alt="" onerror="this.style.visibility='hidden'">
      <div class="meta"><div>${labelTags(it.event.labels, it.event.motion)}</div>
      <div class="muted">${fmtDate(it.start)} · ${fmtDur(dur)}</div></div>`;
    row.onclick = () => { box.querySelectorAll('.act').forEach(a => a.classList.remove('active')); row.classList.add('active'); playInFocus(it); };
    if (!it.play && !it.pending) row.style.opacity = .5;
    box.appendChild(row);
  }
}
$('focus-filter').onchange = loadActivity;
setInterval(() => { if (page === 'live' && focusId && !focusPlayback) loadActivity(); }, 30000);

function noVideo(it) {
  toast(it.pending ? 'This alert is still being recorded. Its full-quality clip will be ready a few seconds after the activity stops.' : 'The video for this alert is no longer available.', !it.pending);
}
function playInFocus(it) {
  if (!it.play) return noVideo(it);
  focusPlayback = it;
  updateSubscriptions();
  $('focus-img').hidden = true;
  $('focus-surface').querySelectorAll('.ov').forEach(o => o.hidden = true);
  if (!focusPlayer) focusPlayer = new Player($('focus-video'), $('focus-player-controls'), $('focus-msg'));
  focusPlayer.setHidden(false);
  $('focus-player-controls').hidden = false;
  focusPlayer.load({...it.play, camera: it.camera, marks: it.event ? [it.event.start] : []});
  $('focus-live').hidden = false;
}
function stopFocusPlayback(resub = true) {
  if (!focusPlayback) return;
  focusPlayback = null;
  if (focusPlayer) focusPlayer.stop();
  if (focusPlayer) focusPlayer.setHidden(true);
  $('focus-img').hidden = false;
  $('focus-player-controls').hidden = true;
  $('focus-surface').querySelectorAll('.ov').forEach(o => o.hidden = false);
  $('focus-live').hidden = true;
  $('focus-activity').querySelectorAll('.act').forEach(a => a.classList.remove('active'));
  if (resub) updateSubscriptions();
}
$('focus-live').onclick = () => stopFocusPlayback();

// "Show motion": boxes around moving objects, with their size in pixels.
let focusMotionTimer = null;
$('focus-motion').onclick = () => {
  const on = !focusMotionTimer;
  $('focus-motion').classList.toggle('on', on);
  // (SVG elements have no .hidden property: use the attribute.)
  $('focus-boxes').toggleAttribute('hidden', !on);
  $('focus-motion-info').hidden = !on;
  clearInterval(focusMotionTimer); focusMotionTimer = null;
  if (on) focusMotionTimer = setInterval(updateFocusMotion, 400);
};
async function updateFocusMotion() {
  if (page !== 'live' || !focusId || focusPlayback) { $('focus-boxes').innerHTML = ''; return; }
  const m = await api(`api/cameras/${enc(focusId)}/motion`).catch(() => null);
  if (!m) return;
  // Place the box layer over the picture (it is letterboxed inside the frame).
  const img = $('focus-img'), sv = $('focus-boxes'), W = img.clientWidth, H = img.clientHeight;
  const iw = img.naturalWidth || 16, ih = img.naturalHeight || 9, k = Math.min(W / iw, H / ih);
  Object.assign(sv.style, {left: (W - iw * k) / 2 + 'px', top: (H - ih * k) / 2 + 'px', width: iw * k + 'px', height: ih * k + 'px'});
  sv.innerHTML = m.objects.map(o => `<rect x="${o.x}" y="${o.y}" width="${o.w}" height="${o.h}" fill="none" stroke="${m.moving ? '#ef4444' : '#f59e0b'}" stroke-width="2" vector-effect="non-scaling-stroke"/>`).join('');
  $('focus-motion-info').textContent = !m.running ? 'Motion detection is off for this camera'
    : `Biggest moving object: ${m.biggest_px.toLocaleString()} px · needs ${m.min_object_px.toLocaleString()} px` + (m.moving ? ' · MOTION' : '');
}
$('focus-back').onclick = () => location.hash = '#live';

// ======================================================================
// Recordings
// ======================================================================
const startOfDay = (d) => { const x = new Date(d); x.setHours(0, 0, 0, 0); return x; };
const endOfDay = (d) => { const x = new Date(d); x.setHours(23, 59, 59, 999); return x; };
const rec = {mode: 'list', cameras: new Set(), from: startOfDay(new Date()), to: endOfDay(new Date()), todFrom: '', todTo: '', types: new Set(), items: [], q: ''};
let recPlayer = null;

function showRecordings(camId, mode) {
  if (camId && cam(camId)) rec.cameras = new Set([camId]);
  if (mode === 'timeline') rec.mode = 'timeline';
  renderFilters();
  setRecMode(rec.mode);
}

function setRecMode(mode) {
  rec.mode = mode;
  document.querySelectorAll('#page-recordings .tabs button').forEach(b => b.classList.toggle('active', b.dataset.mode === mode));
  $('rec-list').hidden = mode !== 'list';
  $('rec-timeline').hidden = mode !== 'timeline';
  $('f-types').hidden = mode !== 'list';
  if (mode === 'list') { tlStop(); loadRecordings(); }
  else { $('rec-player').hidden = true; if (recPlayer) recPlayer.stop(); loadTimeline(); }
}
document.querySelectorAll('#page-recordings .tabs button').forEach(b => b.onclick = () => setRecMode(b.dataset.mode));
// Search box: as you type (after a short pause) or on Enter.
{
  let t = null;
  const go = () => { clearTimeout(t); const v = $('f-q').value.trim(); if (v === rec.q) return; rec.q = v; if (rec.mode !== 'list') setRecMode('list'); else loadRecordings(); };
  $('f-q').addEventListener('input', () => { clearTimeout(t); t = setTimeout(go, 500); });
  $('f-q').addEventListener('keydown', (e) => { if (e.key === 'Enter') go(); });
}
$('f-refresh').onclick = () => rec.mode === 'list' ? loadRecordings() : loadTimeline();
// Full screen for browsing recordings (timeline videos get the whole width).
async function recFullscreen(on) {
  const el = $('page-recordings');
  el.classList.toggle('rec-fs', on);
  $('rec-fs').textContent = on ? '✕ Exit full screen' : '⛶ Full screen';
  if (on && !document.fullscreenElement && el.requestFullscreen) await el.requestFullscreen().catch(() => {});
  if (!on && document.fullscreenElement === el) document.exitFullscreen().catch(() => {});
  if (rec.mode === 'timeline') drawTimeline();
}
$('rec-fs').onclick = () => recFullscreen(!$('page-recordings').classList.contains('rec-fs'));
document.addEventListener('fullscreenchange', () => {
  if (!document.fullscreenElement && $('page-recordings').classList.contains('rec-fs')) recFullscreen(false);
});

function renderFilters() {
  if (!status) return;
  // Cameras
  const names = rec.cameras.size ? status.cameras.filter(c => rec.cameras.has(c.id)).map(c => c.name) : [];
  $('f-cams-btn').textContent = (names.length === 0 ? 'All cameras' : names.length <= 2 ? names.join(', ') : `${names.length} cameras`) + ' ▾';
  const pop = $('f-cams-pop');
  pop.innerHTML = `<label class="check" style="display:flex"><input type="checkbox" data-all ${rec.cameras.size ? '' : 'checked'}> All cameras</label><hr style="border-color:var(--border)">` +
    status.cameras.map(c => `<label class="check" style="display:flex"><input type="checkbox" value="${esc(c.id)}" ${rec.cameras.has(c.id) ? 'checked' : ''}> ${esc(c.name)}</label>`).join('');
  pop.querySelectorAll('input').forEach(i => i.onchange = () => {
    if (i.dataset.all !== undefined) rec.cameras.clear();
    else if (i.checked) rec.cameras.add(i.value); else rec.cameras.delete(i.value);
    renderFilters(); reloadRec();
  });
  // Dates
  const same = startOfDay(rec.from).getTime() === startOfDay(rec.to).getTime();
  const today = startOfDay(new Date()).getTime(), f0 = startOfDay(rec.from).getTime();
  let label = same ? (f0 === today ? 'Today' : f0 === today - 86400000 ? 'Yesterday' : fmtDay(rec.from)) : `${fmtDay(rec.from)} – ${fmtDay(rec.to)}`;
  if (rec.todFrom || rec.todTo) label += `, ${rec.todFrom || '00:00'}–${rec.todTo || '23:59'}`;
  $('f-date-btn').textContent = '📅 ' + label;
  // Types
  $('f-types').innerHTML = '<span class="muted" style="padding:3px 4px">Show:</span>' + FILTER_TYPES.map(([k, v]) =>
    `<label><input type="checkbox" value="${k}" ${rec.types.has(k) ? 'checked' : ''}>${v}</label>`).join('') +
    (rec.types.size ? '<button class="small" id="f-types-clear">All alerts</button>' : '');
  $('f-types').querySelectorAll('input').forEach(i => i.onchange = () => { i.checked ? rec.types.add(i.value) : rec.types.delete(i.value); renderFilters(); loadRecordings(); });
  const clr = $('f-types-clear'); if (clr) clr.onclick = () => { rec.types.clear(); renderFilters(); loadRecordings(); };
}
$('f-cams-btn').onclick = (e) => { e.stopPropagation(); $('f-cams-pop').hidden = !$('f-cams-pop').hidden; $('f-date-pop').hidden = true; };
$('f-date-btn').onclick = (e) => { e.stopPropagation(); const p = $('f-date-pop'); p.hidden = !p.hidden; $('f-cams-pop').hidden = true; if (!p.hidden) Calendar.open(); };
function reloadRec() { rec.mode === 'list' ? loadRecordings() : loadTimeline(); }

// ---------- calendar with range + time-of-day ----------
const Calendar = {
  month: null, a: null, b: null,
  open() { this.a = startOfDay(rec.from); this.b = startOfDay(rec.to); this.month = new Date(this.b.getFullYear(), this.b.getMonth(), 1); this.render(); },
  render() {
    const p = $('f-date-pop'), m = this.month, first = new Date(m), today = startOfDay(new Date()).getTime();
    first.setDate(1 - ((first.getDay() + 6) % 7));
    let cells = '';
    for (let i = 0; i < 42; i++) {
      const d = new Date(first); d.setDate(first.getDate() + i);
      const t = d.getTime(), lo = Math.min(this.a, this.b ?? this.a), hi = Math.max(this.a, this.b ?? this.a);
      const cls = [d.getMonth() !== m.getMonth() ? 'other' : '', t === today ? 'today' : '', (t === lo || t === hi) ? 'edge' : (t > lo && t < hi ? 'inrange' : '')].join(' ');
      cells += `<button type="button" class="${cls}" data-t="${t}" ${t > today ? 'disabled' : ''}>${d.getDate()}</button>`;
    }
    p.innerHTML = `<div class="cal">
      <div class="head"><button type="button" class="small" data-nav="-1">◀</button><b>${MONTHS[m.getMonth()]} ${m.getFullYear()}</b><button type="button" class="small" data-nav="1">▶</button></div>
      <div class="days">${DAYS.map(d => `<span>${d.slice(0, 2)}</span>`).join('')}${cells}</div>
      <div class="hint">Click one day, or a first and last day for a range.</div>
      <div class="row" style="margin-top:8px"><button type="button" class="small" data-p="0">Today</button><button type="button" class="small" data-p="1">Yesterday</button><button type="button" class="small" data-p="7">Last 7 days</button><button type="button" class="small" data-p="30">Last 30 days</button></div>
      <div class="row" style="margin-top:10px"><span class="muted">Only between</span><input type="time" id="cal-tf" value="${rec.todFrom}" style="width:auto"><span class="muted">and</span><input type="time" id="cal-tt" value="${rec.todTo}" style="width:auto"></div>
      <div class="actions" style="margin-top:10px"><button type="button" class="primary" data-apply>Apply</button><button type="button" data-clear-time>Any time</button></div></div>`;
    p.querySelectorAll('[data-nav]').forEach(b => b.onclick = () => { this.month.setMonth(this.month.getMonth() + +b.dataset.nav); this.render(); });
    p.querySelectorAll('[data-t]').forEach(b => b.onclick = () => {
      const t = +b.dataset.t;
      if (this.b !== null || this.a === null) { this.a = t; this.b = null; } else { this.b = t; }
      this.render();
    });
    p.querySelectorAll('[data-p]').forEach(b => b.onclick = () => {
      const n = +b.dataset.p, t = startOfDay(new Date());
      if (n === 1) { t.setDate(t.getDate() - 1); this.a = this.b = t.getTime(); }
      else { this.b = t.getTime(); const s = new Date(t); s.setDate(s.getDate() - Math.max(0, n - 1)); this.a = s.getTime(); }
      this.render();
    });
    p.querySelector('[data-clear-time]').onclick = () => { $('cal-tf').value = ''; $('cal-tt').value = ''; };
    p.querySelector('[data-apply]').onclick = () => {
      const lo = Math.min(this.a, this.b ?? this.a), hi = Math.max(this.a, this.b ?? this.a);
      rec.from = startOfDay(new Date(lo)); rec.to = endOfDay(new Date(hi));
      rec.todFrom = $('cal-tf').value; rec.todTo = $('cal-tt').value;
      p.hidden = true; renderFilters(); tl.day = null; reloadRec();
    };
  },
};

// ---------- list ----------
async function loadRecordings() {
  const q = new URLSearchParams({from: rec.from.toISOString(), to: rec.to.toISOString(), limit: 2000});
  if (rec.cameras.size) q.set('cameras', [...rec.cameras].join(','));
  // Alerts only (plain recordings are on the Timeline tab).
  q.set('types', (rec.types.size ? [...rec.types] : FILTER_TYPES.map(t => t[0])).join(','));
  if (rec.todFrom) q.set('tod_from', rec.todFrom);
  if (rec.todTo) q.set('tod_to', rec.todTo);
  if (rec.q) q.set('q', rec.q);
  $('rec-count').textContent = 'Loading…';
  rec.smart = '';
  try {
    const r = await fetch('api/recordings?' + q);
    if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || r.statusText);
    rec.items = await r.json();
    rec.smart = r.headers.get('x-smart-search') || '';
  } catch (e) { rec.items = []; toast(e.message, true); }
  renderRecordings();
}

function renderRecordings() {
  const box = $('rec-cards'), items = rec.items;
  const ev = items.filter(i => i.kind === 'event').length;
  $('rec-count').textContent = (items.length ? `${ev} alert${ev === 1 ? '' : 's'}` : '')
    + (rec.q && rec.smart === 'on' ? (items.length ? ' · smart search, best matches first' : '') : rec.q && rec.smart.startsWith('unavailable') ? ` · smart search ${rec.smart}` : items.length ? ' · recordings are on the Timeline tab' : '');
  if (!items.length) { box.innerHTML = `<div class="empty" style="grid-column:1/-1">${rec.q ? `No alerts match “${esc(rec.q)}” in this date range. Try a wider date range, fewer words, or another colour (cameras often see white as silver/grey).` : 'No alerts for these filters. Continuous recordings are on the Timeline tab.'}</div>`; return; }
  box.innerHTML = '';
  let day = '';
  for (const it of items) {
    const d = fmtDay(it.start);
    if (d !== day && it.score == null) { day = d; box.insertAdjacentHTML('beforeend', `<div class="dayhead">${esc(d)}</div>`); }
    const card = document.createElement('div');
    card.className = 'rc';
    const dur = (new Date(it.end) - new Date(it.start)) / 1000;
    const tags = it.kind === 'event' ? labelTags(it.event.labels, it.event.motion) : '<span class="tag">recording</span>';
    const icons = it.kind === 'event' ? (it.event.labels.length ? it.event.labels.map(l => `<span class="tag ${LABEL_GROUP[l.label] || ''}" title="${esc((l.color ? l.color + ' ' : '') + l.label)} ${Math.round(l.score * 100)}%">${l.color ? `<i class="cdot" style="background:${COLOR_CSS[l.color] || l.color}"></i>` : ''}${labelIcon(l.label)} ${Math.round(l.score * 100)}%</span>`).join('') : `<span class="tag motion">${labelIcon('motion')}</span>`) : '';
    card.innerHTML = `<div class="thumb">${it.thumb ? `<div class="zw" style="${zoomStyle(it.event && it.event.focus)}"><img loading="lazy" src="${esc(it.thumb)}" alt="" onerror="this.parentElement.remove()">${boxesHtml(it.event, false)}</div>` : ''}
        <div class="tags">${icons}${it.kind === 'event' && it.event.clip ? '<span class="tag">HD</span>' : ''}</div>
        <div class="tools">${it.download ? `<a class="btn small" href="${esc(it.download)}" title="Download" onclick="event.stopPropagation()">⇩</a>` : ''}<button type="button" class="small" data-a="del" title="Delete">✕</button></div>
        <div class="play">${it.play ? '▶' : it.pending ? '⏳' : ''}</div>
        <div class="cap"><b>${esc(it.camera_name)}</b><span class="t">${it.score != null ? fmtDate(it.start) : fmtTime(it.start)} · ${fmtDur(dur)}</span></div></div>`;
    card.title = `${it.camera_name} — ${fmtDate(it.start)}${it.size ? ' — ' + fmtBytes(it.size) : ''}${it.event && it.event.desc ? '\n' + it.event.desc : ''}`;
    card.onclick = () => playItem(it, card);
    card.querySelector('[data-a=del]').onclick = async (e) => {
      e.stopPropagation();
      const what = it.kind === 'event' ? 'this alert' + (it.event.clip ? ' and its clip' : '') : 'this recording';
      if (!confirm(`Delete ${what} from ${fmtDate(it.start)}?`)) return;
      try {
        if (it.kind === 'event') await api(`api/cameras/${enc(it.camera)}/events/${enc(it.event.id)}`, {method: 'DELETE'});
        else await api(`api/cameras/${enc(it.camera)}/recordings/${enc(it.name)}`, {method: 'DELETE'});
        card.remove();
      } catch (err) { toast(err.message, true); }
    };
    box.appendChild(card);
  }
}

function playItem(it, card) {
  if (!it.play) return noVideo(it);
  document.querySelectorAll('.rc.active').forEach(c => c.classList.remove('active'));
  if (card) card.classList.add('active');
  $('rec-player').hidden = false;
  if (!recPlayer) recPlayer = new Player($('rp-video'), $('rp-controls'), $('rp-msg'));
  recPlayer.load({...it.play, camera: it.camera, marks: it.event ? [it.event.start] : []});
  const dur = (new Date(it.end) - new Date(it.start)) / 1000;
  $('rp-info').innerHTML = `<h2>${it.kind === 'event' ? 'Alert' : 'Recording'}</h2>
    ${it.kind === 'event' && it.thumb ? `<div class="review"><img src="${esc(it.thumb)}" alt="">${boxesHtml(it.event, true)}</div>
      ${it.event.boxes && it.event.boxes.length ? `<label class="check" style="margin:-2px 0 8px"><input type="checkbox" class="bx-toggle" ${showBoxes ? 'checked' : ''}> Show detection boxes</label>` : ''}` : ''}
    <dl><dt>Camera</dt><dd>${esc(it.camera_name)}</dd><dt>Time</dt><dd>${fmtDate(it.start)}</dd><dt>Length</dt><dd>${fmtDur(dur)}</dd>
    ${it.kind === 'event' ? `<dt>Detected</dt><dd>${labelTags(it.event.labels, it.event.motion) || '–'}</dd>${it.event.desc ? `<dt>Description</dt><dd>${esc(it.event.desc)}</dd>` : ''}<dt>Video</dt><dd>${it.event.clip ? 'Full-quality clip' : 'From continuous recording'}</dd>` : ''}
    ${it.size ? `<dt>Size</dt><dd>${fmtBytes(it.size)}</dd>` : ''}</dl>
    <div class="hint" style="margin-top:10px">Hover over the bar under the video to preview any moment. Use <b>[ Start</b> and <b>End ]</b> (or drag the orange handles) to cut out a clip.</div>
    <div class="actions"><a class="btn small" href="#recordings/${enc(it.camera)}/timeline" onclick="tl.jump='${it.start}'">Open in timeline</a></div>`;
  const t = $('rp-info').querySelector('.bx-toggle');
  if (t) t.onchange = () => { showBoxes = t.checked; store.set('show-boxes', showBoxes); document.body.classList.toggle('no-boxes', !showBoxes); };
  $('rec-player').scrollIntoView({behavior: 'smooth', block: 'start'});
}

// ---------- multi-camera timeline ----------
const tl = {data: [], T: null, playing: false, speed: 1, zoom: 3600, winStart: null, inT: null, outT: null, sprites: new Map(), last: 0, jump: null, day: null};

async function loadTimeline() {
  const q = new URLSearchParams({from: rec.from.toISOString(), to: rec.to.toISOString()});
  if (rec.cameras.size) q.set('cameras', [...rec.cameras].join(','));
  try { tl.data = await api('api/timeline?' + q); } catch (e) { tl.data = []; toast(e.message, true); }
  tl.loadedAt = Date.now();
  const now = Date.now();
  if (tl.jump) { tl.T = new Date(tl.jump).getTime() - 3000; tl.jump = null; }
  else if (tl.T == null || tl.T < rec.from.getTime() || tl.T > rec.to.getTime()) {
    tl.T = Math.min(now - 60000, rec.to.getTime());
    const first = tl.data.flatMap(c => c.segments).map(s => new Date(s.start).getTime()).sort((a, b) => a - b)[0];
    if (rec.to.getTime() < now && first) tl.T = first;
  }
  tl.zoom = +$('tl-zoom').value;
  centerWindow();
  buildTimelineVideos();
  drawTimeline();
  tlSync(true);
  prefetchSprites();
}
function centerWindow() { tl.winStart = tl.T - tl.zoom / 2 * 1000; }

function buildTimelineVideos() {
  const box = $('tl-videos');
  box.innerHTML = tl.data.length ? '' : '<div class="empty">No cameras selected.</div>';
  for (const c of tl.data) {
    const el = document.createElement('div');
    el.className = 'surface';
    el.dataset.cam = c.camera;
    // Two players per camera: one plays while the next file loads in the
    // other, so moving from file to file doesn't blank the picture.
    el.innerHTML = `<video playsinline muted class="vid"></video><video playsinline muted class="vid" style="position:absolute;inset:0;visibility:hidden"></video>
      <div class="sprite" hidden></div><div class="msg" hidden>No recording</div>
      <div class="ov tl"><span class="ovtext name">${esc(c.name)}</span><span class="ovtext tl-t"></span></div>
      <div class="ov tr"><button type="button" class="small solo-off" hidden>▦ All cameras</button><button type="button" class="fsbtn">⛶</button></div>`;
    el.querySelector('.fsbtn').onclick = () => fullscreen(el);
    el.querySelector('.solo-off').onclick = (e) => { e.stopPropagation(); tlSolo(null); };
    el._vids = [...el.querySelectorAll('video')];
    el._active = 0;
    el._tc = dataSaver || noNativePlayback.has(c.camera);
    // Fall back to converted playback if this browser can't play the files.
    const fallback = (v) => { if (!el._tc && convertedUrl(v.dataset.url || '')) { el._tc = true; markNoNative(c.camera); el._vids.forEach(x => delete x.dataset.url); tlSync(true); } };
    for (const v of el._vids) {
      v.addEventListener('error', () => fallback(v));
      v.addEventListener('loadeddata', () => { if (v.videoWidth === 0) fallback(v); });
    }
    box.appendChild(el);
  }
  if (tl.solo && !tl.data.some(c => c.camera === tl.solo)) tl.solo = null;
  tlSolo(tl.solo);
}
const activeVideo = (el) => el._vids[el._active];
function showVideo(el, i) {
  el._vids.forEach((v, j) => { v.style.visibility = j === i ? '' : 'hidden'; if (j !== i && !v.paused) v.pause(); });
  el._active = i;
}
// The camera's recording after segment `s`, if it follows on (no gap).
function nextSegment(c, s) {
  const end = new Date(s.end).getTime();
  return c.segments.find(x => x !== s && x.kind === s.kind && Math.abs(new Date(x.start).getTime() - end) < 5000);
}
function segAt(c, T) {
  // The newest recording of a camera that was recording when the timeline
  // loaded is still growing: treat it as running up to now.
  const last = c.segments.filter(s => s.kind === 'recording').slice(-1)[0];
  const end = (s) => s === last && tl.loadedAt - new Date(s.end).getTime() < 90000 ? Date.now() : new Date(s.end).getTime() + 1000;
  const hit = (s) => new Date(s.start).getTime() <= T && T < end(s);
  return c.segments.find(s => s.kind === 'recording' && hit(s)) || c.segments.find(hit);
}
// Keep the timeline current while looking at recent footage.
setInterval(() => {
  if (page === 'recordings' && rec.mode === 'timeline' && !tl.playing && Date.now() - tl.T < 3600000) {
    const q = new URLSearchParams({from: rec.from.toISOString(), to: rec.to.toISOString()});
    if (rec.cameras.size) q.set('cameras', [...rec.cameras].join(','));
    api('api/timeline?' + q).then(d => { tl.data = d; tl.loadedAt = Date.now(); drawTimeline(); }).catch(() => {});
  }
}, 30000);

// Keep each camera's video at the shared time `tl.T`.
function tlSync(force) {
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const c = tl.data.find(x => x.camera === el.dataset.cam);
    const msg = el.querySelector('.msg');
    let v = activeVideo(el);
    el.querySelector('.tl-t').textContent = fmtDate(new Date(tl.T));
    const s = c && segAt(c, tl.T);
    if (!s) { msg.hidden = false; el._vids.forEach(x => { x.pause(); x.style.visibility = 'hidden'; }); continue; }
    msg.hidden = true;
    const want = (tl.T - new Date(s.start).getTime()) / 1000;
    if (el._tc) {
      // Converted stream: restart it at the wanted time when needed.
      showVideo(el, el._active);
      const cur = (v._base || 0) + v.currentTime;
      if (v.dataset.url !== s.url || force || Math.abs(cur - want) > Math.max(3, tl.speed * 2)) {
        v.dataset.url = s.url; v._base = want;
        v.src = convertedAt(s.url, want);
      }
      v.playbackRate = tl.speed;
      if (tl.playing && v.paused) v.play().catch(() => {});
      if (!tl.playing && !v.paused) v.pause();
      continue;
    }
    if (v.dataset.url !== s.url) {
      const other = el._vids[1 - el._active];
      if (other.dataset.url === s.url && other.readyState >= 2) {
        // The next file is already loaded: switch players without a gap.
        other.currentTime = want; other.playbackRate = tl.speed;
        if (tl.playing) other.play().catch(() => {});
        showVideo(el, 1 - el._active);
        v = other;
      } else {
        v.dataset.url = s.url; v.src = s.url;
        v.addEventListener('loadedmetadata', () => { v.currentTime = want; v.playbackRate = tl.speed; if (tl.playing) v.play().catch(() => {}); }, {once: true});
        showVideo(el, el._active);
        continue;
      }
    }
    showVideo(el, el._active);
    if (force || Math.abs(v.currentTime - want) > Math.max(0.8, tl.speed * 0.5)) v.currentTime = want;
    v.playbackRate = tl.speed;
    if (tl.playing && v.paused) v.play().catch(() => {});
    if (!tl.playing && !v.paused) v.pause();
    // Near the end of this file: load the next one in the other player.
    const left = (new Date(s.end).getTime() - tl.T) / 1000;
    const nx = left < 10 * Math.max(1, tl.speed) && nextSegment(c, s);
    const other = el._vids[1 - el._active];
    if (nx && other.dataset.url !== nx.url) {
      other.dataset.url = nx.url; other.preload = 'auto'; other.src = nx.url;
      other.addEventListener('loadedmetadata', () => { other.currentTime = 0; }, {once: true});
    }
  }
  drawHead();
}

function tlLoop(ts) {
  if (!tl.playing) return;
  if (tl.last) tl.T += (ts - tl.last) * tl.speed;
  tl.last = ts;
  if (tl.T > Date.now()) { tl.T = Date.now(); }
  // Scroll the window along.
  if (tl.T > tl.winStart + tl.zoom * 1000 * 0.9) { tl.winStart = tl.T - tl.zoom * 1000 * 0.1; drawTimeline(); }
  if (!tl.syncAt || ts - tl.syncAt > 400) { tl.syncAt = ts; tlSync(false); } else drawHead();
  requestAnimationFrame(tlLoop);
}
function tlPlay(on) {
  tl.playing = on; tl.last = 0;
  $('tl-play').textContent = on ? '❚❚ Pause' : '▶ Play';
  tlSync(false);
  if (on) requestAnimationFrame(tlLoop);
}
function tlStop() { if (tl.playing) tlPlay(false); document.querySelectorAll('#tl-videos video').forEach(v => v.pause()); }
$('tl-play').onclick = () => tlPlay(!tl.playing);
$('tl-speed').onchange = (e) => { tl.speed = +e.target.value; tlSync(false); };
$('tl-zoom').onchange = (e) => { tl.zoom = +e.target.value; centerWindow(); drawTimeline(); prefetchSprites(); };
$('tl-left').onclick = () => { tl.winStart -= tl.zoom * 500; drawTimeline(); prefetchSprites(); };
$('tl-right').onclick = () => { tl.winStart += tl.zoom * 500; drawTimeline(); prefetchSprites(); };
$('tl-back10').onclick = () => { tl.T -= 10000; tlSync(true); };
$('tl-fwd10').onclick = () => { tl.T += 10000; tlSync(true); };
function allEvents() { return tl.data.flatMap(c => c.events).map(e => new Date(e.start).getTime()).sort((a, b) => a - b); }
$('tl-prev-ev').onclick = () => { const e = allEvents().filter(t => t < tl.T - 4000).pop(); if (e) jumpTo(e - 3000); };
$('tl-next-ev').onclick = () => { const e = allEvents().find(t => t > tl.T + 4000); if (e) jumpTo(e - 3000); };
// Watch one alert: that camera's video large (others hidden), from just before it.
function tlShowEvent(camId, T) {
  tlSolo(camId);
  jumpTo(T - 3000);
  tlPlay(true);
}
function tlSolo(camId) {
  tl.solo = camId || null;
  const box = $('tl-videos');
  box.classList.toggle('solo', !!tl.solo);
  box.querySelectorAll('.surface').forEach(el => {
    el.classList.toggle('solo', el.dataset.cam === tl.solo);
    const b = el.querySelector('.solo-off'); if (b) b.hidden = el.dataset.cam !== tl.solo;
  });
  $('tl').querySelectorAll('.tl-row').forEach((r, i) => r.classList.toggle('solo', tl.solo === (tl.data[i] || {}).camera));
}
function jumpTo(T) { tl.T = T; if (T < tl.winStart || T > tl.winStart + tl.zoom * 1000) centerWindow(); drawTimeline(); tlSync(true); }
$('tl-in').onclick = () => { tl.inT = tl.T; if (tl.outT != null && tl.outT <= tl.inT) tl.outT = null; drawTimeline(); };
$('tl-out').onclick = () => { tl.outT = tl.T; if (tl.inT == null || tl.inT >= tl.outT) tl.inT = tl.outT - 30000; drawTimeline(); };
$('tl-clear').onclick = () => { tl.inT = tl.outT = null; drawTimeline(); };

function drawTimeline() {
  const box = $('tl'), W0 = tl.winStart, span = tl.zoom * 1000;
  const pct = (t) => ((t - W0) / span * 100);
  const steps = [60, 300, 600, 900, 1800, 3600, 7200, 10800, 21600].map(s => s * 1000);
  const step = steps.find(s => span / s <= 8) || 21600000;
  let axis = '';
  for (let t = Math.ceil(W0 / step) * step; t <= W0 + span; t += step) {
    const d = new Date(t);
    axis += `<span style="left:${pct(t)}%">${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}${d.getHours() === 0 && d.getMinutes() === 0 ? ' ' + fmtDay(d) : ''}</span>`;
  }
  let rows = '';
  for (const c of tl.data) {
    let lane = '';
    for (const s of c.segments) {
      const a = Math.max(0, pct(new Date(s.start).getTime())), b = Math.min(100, pct(new Date(s.end).getTime()));
      if (b > 0 && a < 100 && b > a) lane += `<div class="seg ${s.kind === 'clip' ? 'clip' : ''}" style="left:${a}%;width:${Math.max(b - a, 0.15)}%"></div>`;
    }
    for (const e of c.events) {
      const a = pct(new Date(e.start).getTime()), b = pct(new Date(e.end).getTime());
      if (b < 0 || a > 100) continue;
      const tip = e.labels.length ? e.labels.map(l => l.label).join(', ') : 'motion';
      lane += `<div class="ev ${e.labels.length ? '' : 'motion'}" style="left:${Math.max(0, a)}%;width:${Math.max(0.3, b - a)}%" title="${esc(tip)} ${fmtTime(e.start)} — click to watch" data-t="${new Date(e.start).getTime()}" data-cam="${esc(c.camera)}">${e.labels.length ? `<span class="ic">${labelIcon(e.labels[0].label)}</span>` : ''}</div>`;
    }
    rows += `<div class="tl-row"><div class="label" title="${esc(c.name)}">${esc(c.name)}</div><div class="lane">${lane}</div></div>`;
  }
  const sel = tl.inT != null && tl.outT != null ? `<div class="tl-sel" style="left:calc(120px + (100% - 120px) * ${pct(tl.inT) / 100});width:calc((100% - 120px) * ${(pct(tl.outT) - pct(tl.inT)) / 100})"></div>` : '';
  box.innerHTML = `<div class="tl-axis">${axis}</div>${rows}${sel}<div class="tl-head" id="tl-head"></div>`;
  box.querySelectorAll('.tl-row').forEach((r, i) => r.classList.toggle('solo', tl.solo === (tl.data[i] || {}).camera));
  // Clip export
  const has = tl.inT != null && tl.outT != null;
  $('tl-clear').hidden = !has;
  $('tl-sel-info').textContent = has ? `Clip ${fmtTime(new Date(tl.inT))} – ${fmtTime(new Date(tl.outT))} (${fmtClock((tl.outT - tl.inT) / 1000)})` : '';
  $('tl-export').innerHTML = has ? tl.data.map(c => `<a class="btn small" href="api/export?camera=${enc(c.camera)}&from=${enc(new Date(tl.inT).toISOString())}&to=${enc(new Date(tl.outT).toISOString())}">⇩ ${esc(c.name)}</a>`).join('') : '';
  drawHead();
}
function drawHead() {
  const h = $('tl-head'); if (!h) return;
  const p = (tl.T - tl.winStart) / (tl.zoom * 1000);
  h.hidden = p < 0 || p > 1;
  h.style.left = `calc(120px + (100% - 120px) * ${p})`;
  $('tl-time').textContent = fmtDate(new Date(tl.T));
}

// Scrubbing the timeline shows sprite previews on every camera.
(function wireTimeline() {
  const box = $('tl');
  let drag = false, was = false;
  const tAt = (e) => {
    const lane = box.querySelector('.lane'); if (!lane) return null;
    const r = lane.getBoundingClientRect();
    return tl.winStart + Math.min(Math.max((e.clientX - r.left) / r.width, 0), 1) * tl.zoom * 1000;
  };
  box.addEventListener('pointerdown', (e) => {
    // An alert marker or symbol: watch that alert on that camera.
    const mark = e.target.closest('.ev');
    if (mark) { e.preventDefault(); tlShowEvent(mark.dataset.cam, +mark.dataset.t); return; }
    const t = tAt(e); if (t == null || e.clientX < box.querySelector('.lane').getBoundingClientRect().left) return;
    drag = true; was = tl.playing; if (was) tlPlay(false);
    box.setPointerCapture(e.pointerId);
    tl.T = t; drawHead(); previewAll();
  });
  box.addEventListener('pointermove', (e) => { if (!drag) return; tl.T = Math.min(tAt(e), Date.now()); drawHead(); previewAll(); });
  const end = () => {
    if (!drag) return; drag = false;
    document.querySelectorAll('#tl-videos .sprite').forEach(s => s.hidden = true);
    tlSync(true); if (was) tlPlay(true);
  };
  box.addEventListener('pointerup', end); box.addEventListener('pointercancel', end);
})();

async function spriteFor(cam, seg) {
  const key = seg.url;
  if (!tl.sprites.has(key)) {
    tl.sprites.set(key, api(`api/sprite/${enc(cam)}/${seg.kind === 'clip' ? 'clip' : 'rec'}/${enc(seg.name)}`).then(preloadSprite).catch(() => null));
  }
  return tl.sprites.get(key);
}
// Prepare previews for every file in view (nearest to the playhead first),
// showing progress, so scrubbing is instant.
function prefetchSprites() {
  const gen = tl.pfGen = (tl.pfGen || 0) + 1;
  const a = tl.winStart, b = a + tl.zoom * 1000;
  const list = [];
  for (const c of tl.data) for (const sg of c.segments) {
    const s0 = new Date(sg.start).getTime(), s1 = new Date(sg.end).getTime();
    if (s1 >= a && s0 <= b) list.push({cam: c.camera, sg, d: Math.abs((s0 + s1) / 2 - tl.T)});
  }
  list.sort((x, y) => x.d - y.d);
  const todo = list.slice(0, 300);
  const st = {total: todo.length, done: 0};
  const show = () => {
    if (gen !== tl.pfGen) return;
    $('tl-sp').hidden = !st.total;
    $('tl-sp-bar').style.width = (st.total ? st.done / st.total * 100 : 0) + '%';
    $('tl-sp-t').textContent = st.done >= st.total ? `ready (${st.total} file${st.total === 1 ? '' : 's'})` : `${st.done} of ${st.total} files ready`;
  };
  show();
  tl.pagesDone = tl.pagesDone || new Set();
  const loadPages = (sp) => Promise.all([...Array(sp.info.pages).keys()].map(p => tl.pagesDone.has(sp.url + p) ? null :
    new Promise(r => { const im = new Image(); im.onload = im.onerror = () => { tl.pagesDone.add(sp.url + p); r(); }; im.src = sp.url + p; })));
  let i = 0;
  const worker = async () => {
    while (i < todo.length && gen === tl.pfGen) {
      const job = todo[i++];
      const sp = await spriteFor(job.cam, job.sg);
      if (sp) await loadPages(sp);
      st.done++; show();
    }
  };
  worker(); worker();
}
function previewAll() {
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const c = tl.data.find(x => x.camera === el.dataset.cam);
    el.querySelector('.tl-t').textContent = fmtDate(new Date(tl.T));
    const s = c && segAt(c, tl.T), sp = el.querySelector('.sprite');
    el.querySelector('.msg').hidden = !!s;
    if (!s) el._vids.forEach(x => x.style.visibility = 'hidden');
    if (!s) { sp.hidden = true; continue; }
    const T = tl.T;
    spriteFor(c.camera, s).then(info => {
      if (!info || tl.T !== T) return;
      const off = (T - new Date(s.start).getTime()) / 1000;
      sp.hidden = false;
      const {ox, oy} = spriteStyle(sp, info, Math.floor(off / info.info.interval), el.clientWidth, el.clientHeight);
      sp.style.clipPath = `inset(${oy}px ${ox}px)`;
    });
  }
}

// ======================================================================
// System monitor
// ======================================================================
let sysRange = 3600, sysTimer = null, sysHist = [];
const SERIES = [
  ['cpu', 'CPU (whole computer)', '#4f8cff'],
  ['nvr_cpu', 'CPU used by NVR', '#22c55e'],
  ['gpu', 'GPU', '#a78bfa'],
  ['ram', 'Memory (whole computer)', '#f59e0b'],
  ['nvr_ram', 'Memory used by NVR', '#ef4444'],
];
// Graph lines switched off by the viewer (remembered).
const graphHidden = new Set(store.get('graph-hidden', []));

function showSystem() {
  loadSystem();
  clearInterval(sysTimer);
  sysTimer = setInterval(() => { if (page === 'system') loadSystem(); else clearInterval(sysTimer); }, 5000);
}
document.querySelectorAll('#sys-range button').forEach(b => b.onclick = () => {
  document.querySelectorAll('#sys-range button').forEach(x => x.classList.toggle('active', x === b));
  sysRange = +b.dataset.r; loadSystem();
});

async function loadSystem() {
  let mon, usage;
  try { [mon, usage] = await Promise.all([api('api/monitor?range=' + sysRange), api('api/usage')]); } catch (e) { return; }
  const c = mon.current, s = c.sample;
  const card = (title, big, sub, pct) => `<div class="card"><h2>${title}</h2><div class="stat">${big}</div><div class="muted">${sub}</div>${pct != null ? `<div class="bar"><div style="width:${Math.min(100, pct)}%;${pct > 85 ? 'background:var(--red)' : ''}"></div></div>` : ''}</div>`;
  const disk = usage.locations[0];
  $('sys-cards').innerHTML =
    card('CPU', s.cpu.toFixed(0) + '%', `${esc(c.cpu_name || 'CPU')} · ${c.cores} threads<br>NVR + ffmpeg: ${s.nvr_cpu.toFixed(1)}%`, s.cpu) +
    card('GPU', s.gpu != null ? s.gpu.toFixed(0) + '%' : '–', s.gpu != null ? `${esc(c.gpu_name || 'GPU')}${c.gpu_temp != null ? ' · ' + c.gpu_temp.toFixed(0) + ' °C' : ''}${s.gpu_mem != null ? ' · memory ' + s.gpu_mem.toFixed(0) + '%' : ''}` : 'No GPU readings available (NVIDIA needs nvidia-smi; AMD on Linux and any GPU on Windows are read automatically).', s.gpu) +
    card('Memory', s.ram.toFixed(0) + '%', `${fmtBytes(c.ram_used)} of ${fmtBytes(c.ram_total)}<br>NVR + ffmpeg: ${fmtBytes(c.nvr_ram_used || 0)} (${(s.nvr_ram || 0).toFixed(1)}%)`, s.ram) +
    card('Disk', disk ? fmtBytes(disk.free_bytes) + ' free' : '–', disk ? `of ${fmtBytes(disk.total_bytes)} · ${esc(disk.path)}` : '', disk ? 100 * (1 - disk.free_bytes / disk.total_bytes) : null) +
    card('Recording rate', fmtBytes(usage.bytes_per_hour) + '/h', `${fmtBytes(usage.bytes_per_day)} per 24 hours`, null) +
    card('Days of recording', usage.days_capacity != null ? (usage.days_capacity < 2 ? usage.days_capacity.toFixed(1) + ' days' : Math.floor(usage.days_capacity) + ' days') : '–',
      usage.days_capacity != null ? `fit at the current rate (limited by ${esc(usage.limited_by)}). Retention is set to ${usage.retention_days ? usage.retention_days + ' days' : 'forever'}${usage.retention_days && usage.days_capacity < usage.retention_days ? ' — <span class="warnc">not enough space, oldest recordings will be deleted sooner</span>' : ''}.` : 'Shown once some recordings exist.', null);
  sysHist = mon.history;
  drawGraph();
  // Storage locations
  $('sys-storage').innerHTML = usage.locations.length ? `<table><thead><tr><th>Location</th><th>Cameras</th><th>Used by NVR</th><th>Free</th><th class="hide-sm">Per day</th><th>Full in</th></tr></thead><tbody>` +
    usage.locations.map(l => `<tr><td><code>${esc(l.path)}</code><div class="bar"><div style="width:${(100 * (1 - l.free_bytes / l.total_bytes)).toFixed(1)}%"></div></div></td>
      <td>${esc(l.cameras.join(', ') || '–')}</td><td>${fmtBytes(l.used_bytes)}</td><td>${fmtBytes(l.free_bytes)} of ${fmtBytes(l.total_bytes)}</td>
      <td class="hide-sm">${fmtBytes(l.bytes_per_day)}</td><td>${l.days_until_full != null ? l.days_until_full.toFixed(1) + ' days' : '–'}</td></tr>`).join('') + '</tbody></table>'
    + `<div class="hint">“Full in” is how long until free space runs out at the current rate (keeping the “free disk space” reserve). After that, the oldest recordings are deleted automatically. Total space allowed for recordings: ${usage.capacity_bytes ? fmtBytes(usage.capacity_bytes) : '–'}.</div>` : '<div class="muted">No data yet.</div>';
  $('sys-cams').innerHTML = usage.cameras.length ? `<table><thead><tr><th>Camera</th><th>Per hour</th><th>Per 24 h</th><th class="hide-sm">Stored</th><th class="hide-sm">Oldest</th><th>Keep</th></tr></thead><tbody>` +
    usage.cameras.map(u => `<tr><td>${esc(u.name)}<div class="sub">${esc(u.dir)}</div></td><td>${fmtBytes(u.bytes_per_hour)}</td><td>${fmtBytes(u.bytes_per_day)}</td>
      <td class="hide-sm">${fmtBytes(u.used_bytes)}</td><td class="hide-sm">${u.oldest ? fmtDate(u.oldest) : '–'}</td><td>${u.retention_days ? u.retention_days + ' days' : 'forever'}</td></tr>`).join('') + '</tbody></table>' : '<div class="muted">No cameras.</div>';
}

function drawGraph() {
  const svg = $('sys-graph'), W = svg.clientWidth || 800, H = 220, h = sysHist;
  svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
  if (h.length < 2) { svg.innerHTML = `<text x="${W / 2}" y="${H / 2}" fill="#8b93a3" text-anchor="middle" font-size="13">Collecting data… (a point every few seconds)</text>`; $('sys-legend').innerHTML = ''; return; }
  const t0 = h[0].t, t1 = h[h.length - 1].t, X = (t) => (t - t0) / Math.max(1, t1 - t0) * (W - 40) + 36, Y = (v) => H - 18 - v / 100 * (H - 28);
  let g = '';
  for (const v of [0, 25, 50, 75, 100]) g += `<line x1="36" x2="${W}" y1="${Y(v)}" y2="${Y(v)}" stroke="#2a2f3a"/><text x="30" y="${Y(v) + 4}" fill="#8b93a3" font-size="10" text-anchor="end">${v}%</text>`;
  const ticks = 6;
  for (let i = 0; i <= ticks; i++) {
    const t = t0 + (t1 - t0) * i / ticks, d = new Date(t * 1000);
    const p2 = (n) => String(n).padStart(2, '0');
    const lab = t1 - t0 > 86400 ? `${d.getDate()}/${d.getMonth() + 1} ${p2(d.getHours())}:00` : t1 - t0 < 900 ? `${p2(d.getHours())}:${p2(d.getMinutes())}:${p2(d.getSeconds())}` : `${p2(d.getHours())}:${p2(d.getMinutes())}`;
    g += `<text x="${X(t)}" y="${H - 3}" fill="#8b93a3" font-size="10" text-anchor="middle">${lab}</text>`;
  }
  // CPU peaks as a shaded band
  if (!graphHidden.has('cpu_max')) g += `<path d="${h.map((p, i) => (i ? 'L' : 'M') + X(p.t) + ',' + Y(p.cpu_max)).join('')}L${X(t1)},${Y(0)}L${X(t0)},${Y(0)}Z" fill="rgba(79,140,255,.12)"/>`;
  for (const [k, , color] of SERIES) {
    if (graphHidden.has(k)) continue;
    const pts = h.filter(p => p[k] != null);
    if (pts.length < 2) continue;
    g += `<path d="${pts.map((p, i) => (i ? 'L' : 'M') + X(p.t).toFixed(1) + ',' + Y(p[k]).toFixed(1)).join('')}" fill="none" stroke="${color}" stroke-width="1.6"/>`;
  }
  g += '<line id="g-cursor" y1="0" y2="' + (H - 18) + '" stroke="#fff" stroke-opacity=".4" hidden/>';
  svg.innerHTML = g;
  // Click a legend entry to show or hide that line.
  $('sys-legend').innerHTML = SERIES.filter(([k]) => h.some(p => p[k] != null)).concat([['cpu_max', 'CPU peaks', 'rgba(79,140,255,.3)']])
    .map(([k, n, c]) => `<span data-k="${k}" class="${graphHidden.has(k) ? 'off' : ''}" title="Click to show or hide"><i style="background:${c}"></i>${n}</span>`).join('');
  $('sys-legend').querySelectorAll('span').forEach(el => el.onclick = () => {
    const k = el.dataset.k;
    if (graphHidden.has(k)) graphHidden.delete(k); else graphHidden.add(k);
    store.set('graph-hidden', [...graphHidden]);
    drawGraph();
  });
  svg.onmousemove = (e) => {
    const r = svg.getBoundingClientRect(), x = (e.clientX - r.left) / r.width * W;
    const t = t0 + (x - 36) / (W - 40) * (t1 - t0);
    const p = h.reduce((a, b) => Math.abs(b.t - t) < Math.abs(a.t - t) ? b : a);
    const cur = svg.querySelector('#g-cursor'); cur.removeAttribute('hidden'); cur.setAttribute('x1', X(p.t)); cur.setAttribute('x2', X(p.t));
    $('sys-hover').textContent = `${fmtDate(new Date(p.t * 1000))} — CPU ${p.cpu.toFixed(0)}% (peak ${p.cpu_max.toFixed(0)}%), NVR ${p.nvr_cpu.toFixed(1)}%, memory ${p.ram.toFixed(0)}% (NVR ${(p.nvr_ram || 0).toFixed(1)}%)` + (p.gpu != null ? `, GPU ${p.gpu.toFixed(0)}%` : '');
  };
}
window.addEventListener('resize', () => { if (page === 'system') drawGraph(); });

// ======================================================================
// Logs
// ======================================================================
let logLast = 0, logTimer = null;
function showLogs() {
  if (status) {
    const sel = $('lg-cam'), cur = sel.value;
    sel.innerHTML = '<option value="">All cameras</option>' + status.cameras.map(c => `<option>${esc(c.name)}</option>`).join('');
    sel.value = cur;
  }
  reloadLogs();
  clearInterval(logTimer);
  logTimer = setInterval(() => { if (page !== 'logs') return clearInterval(logTimer); if ($('lg-live').checked) loadLogs(true); }, 3000);
}
function logQuery(after) {
  const q = new URLSearchParams({level: $('lg-level').value, limit: 1000});
  if ($('lg-kind').value) q.set('kind', $('lg-kind').value);
  if ($('lg-cam').value) q.set('camera', $('lg-cam').value);
  if ($('lg-q').value.trim()) q.set('q', $('lg-q').value.trim());
  if (after) q.set('after', after);
  return 'api/logs?' + q;
}
function reloadLogs() { logLast = 0; $('lg-list').innerHTML = ''; loadLogs(false); }
async function loadLogs(incremental) {
  let rows;
  try { rows = await api(logQuery(incremental ? logLast : 0)); } catch (e) { return; }
  const box = $('lg-list');
  const atBottom = box.scrollTop + box.clientHeight >= box.scrollHeight - 30;
  if (!incremental && !rows.length) box.innerHTML = '<div class="empty">No log entries match.</div>';
  else if (!incremental) box.innerHTML = '';
  for (const r of rows) {
    if (box.querySelector('.empty')) box.innerHTML = '';
    logLast = Math.max(logLast, r.id);
    box.insertAdjacentHTML('beforeend', `<div class="l ${r.kind}"><span class="muted">${fmtDate(r.time)}</span><span class="lv ${r.level}">${r.level.toUpperCase()}</span><span>${esc(r.camera || (r.kind === 'event' ? '' : 'System'))}</span><span class="msg">${esc(r.message.replace(/^\[[^\]]*\]\s*/, ''))}</span></div>`);
  }
  while (box.children.length > 3000) box.firstChild.remove();
  if (atBottom || !incremental) box.scrollTop = box.scrollHeight;
}
for (const id of ['lg-level', 'lg-kind', 'lg-cam']) $(id).onchange = reloadLogs;
let lgDeb; $('lg-q').oninput = () => { clearTimeout(lgDeb); lgDeb = setTimeout(reloadLogs, 300); };

// ======================================================================
// Settings
// ======================================================================
const SECTIONS = ['cameras', 'storage', 'display', 'ai', 'hardware', 'cloud', 'backup', 'system', 'info'];
let section = 'cameras';
const camPreviewUnbind = [];

async function loadSettings() { [settings, cameraCfgs] = await Promise.all([api('api/settings'), api('api/cameras')]); }

async function showSettings(sec) {
  section = SECTIONS.includes(sec) ? sec : 'cameras';
  document.querySelectorAll('#side a').forEach(a => a.classList.toggle('active', a.dataset.s === section));
  try { await loadSettings(); } catch (e) { toast(e.message, true); return; }
  camPreviewUnbind.splice(0).forEach(u => u());
  const body = $('settings-body');
  ({cameras: renderCameras, storage: renderStorage, display: renderDisplay, ai: renderAi, hardware: renderHardware,
    cloud: renderCloud, backup: renderBackup, system: renderSystemSettings, info: renderInfo})[section](body);
}
function settingsTick() {
  if (section !== 'cameras') return;
  for (const row of document.querySelectorAll('#cam-table tr[data-id]')) {
    const c = cam(row.dataset.id); if (!c) continue;
    row.querySelector('.st').innerHTML = pillHtml(c) + camErr(c);
  }
}
function camErr(c) {
  const r = c.recorder || c.buffer, d = c.detect, out = streamErrors(c);
  if (!out.length && r && r.last_error && r.state !== 'recording') out.push(r.last_error);
  if (r && r.gpu_fallback) out.push('GPU failed – using CPU');
  if (d && d.ai_error) out.push('AI: ' + d.ai_error);
  return out.length ? `<div class="sub err">${esc(out.join(' · '))}</div>` : '';
}

async function saveSettings(part, msg) {
  try {
    const r = await api('api/settings', {method: 'PUT', json: part});
    toast(r.restart_required ? 'Saved. Restart the NVR to use the new web address.' : (msg || 'Settings saved'));
    await loadSettings(); await refreshStatus();
    return true;
  } catch (e) { toast(e.message, true); return false; }
}
const val = (id) => $(id).value;
const num = (id) => +$(id).value;
const chk = (id) => $(id).checked;

// ---------- cameras ----------
function renderCameras(body) {
  body.innerHTML = `<div class="card">
    <div class="row" style="margin-bottom:8px"><h2 style="flex:1;margin:0">Cameras</h2><button class="primary" id="add-cam">+ Add camera</button></div>
    <table><thead><tr><th>Preview</th><th>Name</th><th class="hide-sm">Recording</th><th>Status</th><th></th></tr></thead><tbody id="cam-table"></tbody></table>
    ${cameraCfgs.length ? '' : '<div class="empty">No cameras yet. Click “Add camera”.</div>'}</div>`;
  $('add-cam').onclick = () => openCameraDialog();
  const tb = $('cam-table');
  for (const cfg of cameraCfgs) {
    const c = cam(cfg.id);
    const tr = document.createElement('tr');
    tr.dataset.id = cfg.id;
    const sched = cfg.schedule_enabled ? `<div class="sub">${cfg.schedule.days.map(d => DAYS[d]).join(', ')} ${esc(cfg.schedule.start)}–${esc(cfg.schedule.end)}</div>` : '';
    tr.innerHTML = `<td><div class="surface camprev"><img alt=""><div class="msg">${cfg.enabled ? '…' : 'off'}</div></div></td>
      <td><b>${esc(cfg.name)}</b><div class="sub">${esc(c ? c.url : '')}</div></td>
      <td class="hide-sm">${esc(MODE_TEXT[cfg.record_mode])}${cfg.detect.ai ? ' · AI' : cfg.detect.motion ? ' · motion' : ''}${sched}</td>
      <td class="st">${c ? pillHtml(c) + camErr(c) : ''}</td>
      <td style="text-align:right"><div class="row" style="justify-content:flex-end">
        <button class="small" data-a="edit">Edit</button><button class="small" data-a="clone">Clone</button><button class="small" data-a="del">Delete</button></div></td>`;
    const img = tr.querySelector('img');
    img.onload = () => tr.querySelector('.msg').hidden = true;
    camPreviewUnbind.push(Live.bind(img, cfg.id, 'sub'));
    tr.querySelector('.surface').onclick = () => location.hash = '#live/' + enc(cfg.id);
    tr.querySelector('[data-a=edit]').onclick = () => openCameraDialog(cfg.id);
    tr.querySelector('[data-a=clone]').onclick = () => openCameraDialog(cfg.id, true);
    tr.querySelector('[data-a=del]').onclick = () => deleteCamera(cfg.id);
    tb.appendChild(tr);
  }
}

async function deleteCamera(id) {
  const cfg = cameraCfgs.find(c => c.id === id);
  if (!confirm(`Delete camera "${cfg.name}"?`)) return;
  const delRecs = confirm(`Also delete all recordings and alerts from "${cfg.name}"?\n\nOK = delete them\nCancel = keep them on disk`);
  try { await api(`api/cameras/${enc(id)}?delete_recordings=${delRecs}`, {method: 'DELETE'}); toast('Camera deleted'); }
  catch (e) { toast(e.message, true); }
  await refreshStatus(); showSettings('cameras');
}

// Common RTSP URL formats. {auth} = user:password@
const BRANDS = [
  ['hikvision', 'Hikvision / Annke / HiLook', 554, '/Streaming/Channels/{ch}01', '/Streaming/Channels/{ch}02'],
  ['dahua', 'Dahua / Amcrest / Lorex', 554, '/cam/realmonitor?channel={ch}&subtype=0', '/cam/realmonitor?channel={ch}&subtype=1'],
  ['reolink', 'Reolink (H.264)', 554, '/h264Preview_0{ch}_main', '/h264Preview_0{ch}_sub'],
  ['reolink265', 'Reolink (H.265 / 4K)', 554, '/Preview_0{ch}_main', '/Preview_0{ch}_sub'],
  ['tapo', 'TP-Link Tapo / VIGI', 554, '/stream1', '/stream2'],
  ['uniview', 'Uniview (UNV)', 554, '/media/video1', '/media/video2'],
  ['axis', 'Axis', 554, '/axis-media/media.amp', '/axis-media/media.amp?resolution=640x360'],
  ['hanwha', 'Hanwha / Wisenet / Samsung', 554, '/profile2/media.smp', '/profile3/media.smp'],
  ['foscam', 'Foscam', 88, '/videoMain', '/videoSub'],
  ['swann', 'Swann', 554, '/ch0{ch}/0', '/ch0{ch}/1'],
  ['ezviz', 'EZVIZ (password = verification code)', 554, '/h264/ch{ch}/main/av_stream', '/h264/ch{ch}/sub/av_stream'],
  ['eufy', 'Eufy (RTSP turned on)', 554, '/live0', ''],
  ['wyze', 'Wyze (RTSP firmware)', 554, '/live', ''],
  ['onvif', 'Generic ONVIF', 554, '/onvif1', '/onvif2'],
  ['generic', 'Other (stream1 / stream2)', 554, '/stream1', '/stream2'],
];
function brandUrl(brand, which) {
  const b = BRANDS.find(x => x[0] === brand); if (!b) return '';
  const path = which === 'main' ? b[3] : b[4]; if (!path) return '';
  const ip = val('q-ip').trim() || '192.168.1.100', port = val('q-port').trim() || b[2], ch = val('q-ch').trim() || '1';
  const user = val('q-user').trim(), pass = $('q-pass').value;
  const auth = user ? `${enc(user)}${pass ? ':' + enc(pass) : ''}@` : '';
  return `rtsp://${auth}${ip}:${port}${path.replaceAll('{ch}', ch)}`;
}
// Put the quick-setup IP, port and login into an existing stream URL,
// keeping its path (e.g. /Streaming/Channels/102).
function rewriteUrl(u) {
  const m = u.trim().match(/^(rtsps?:\/\/)(?:([^@\/]*)@)?([^:\/?#]+)(?::(\d+))?(.*)$/i);
  if (!m) return u;
  const ip = val('q-ip').trim() || m[3], port = val('q-port').trim() || m[4];
  const user = val('q-user').trim(), pass = $('q-pass').value;
  // Keep the URL's own login unless a new one is typed.
  const auth = user ? `${enc(user)}${pass ? ':' + enc(pass) : ''}@` : (m[2] ? m[2] + '@' : '');
  return `${m[1]}${auth}${ip}${port ? ':' + port : ''}${m[5]}`;
}
// Fill the quick-setup fields from an existing URL.
function parseUrl(u) {
  try {
    const x = new URL(u.replace(/^rtsps?:/, 'http:'));
    return {ip: x.hostname, port: x.port, user: decodeURIComponent(x.username), pass: decodeURIComponent(x.password)};
  } catch (e) { return null; }
}

let editingId = null, cloneMode = false;
const previewImgs = [];
function stopPreviews() { previewImgs.splice(0).forEach(i => i.removeAttribute('src')); }

async function openCameraDialog(id, clone) {
  if (!settings) await loadSettings().catch(() => {});
  if (!sysinfo) sysinfo = await api('api/system').catch(() => null);
  editingId = clone ? null : (id || null); cloneMode = !!clone;
  const base = id ? cameraCfgs.find(c => c.id === id) : null;
  const c = base ? JSON.parse(JSON.stringify(base)) : {
    name: '', enabled: true, url: '', live_url: '', rtsp_transport: 'tcp', record_mode: 'continuous', schedule_enabled: false,
    schedule: {days: [0,1,2,3,4,5,6], start: '18:00', end: '07:00'}, record_audio: true, transcode: false, record_fps: 0,
    segment_seconds: 0, storage_dir: '', retention_days: 0, max_storage_gb: 0,
    osd: {custom: false, show_name: true, name_corner: 'bl', show_clock: true, clock_corner: 'tl', clock_format: '', show_status: true, status_corner: 'tr',
      show_detections: true, detections_corner: 'bl', show_fps: false, fps_corner: 'tl', text: '', text_corner: 'br'},
    view: {grid_quality: 'auto', grid_fps: 0, hd_fps: 0},
    ptz: {enabled: false, host: '', port: 80, username: '', password: '', speed: 0.5, invert_pan: false, invert_tilt: false},
    detect: {motion: true, min_contrast: 24, min_object_px: 300, make_time_ms: 500, min_travel_px: 0, mask: '', ai: false, objects: ['person','car','truck','dog','cat'], min_confidence: 0.5, trigger: 'motion', pre_seconds: 30, post_seconds: 15, fps: 5, ai_interval: 1}};
  if (clone) c.name = c.name + ' (copy)';
  const q = parseUrl(c.url) || {};
  const objs = (sysinfo && sysinfo.ai_objects) || ['person','car','truck','bus','motorcycle','bicycle','dog','cat','bird'];
  const aiOn = settings && settings.ai.provider !== 'none';
  const brandOpts = '<option value="">Format…</option>' + BRANDS.map(b => `<option value="${b[0]}">${esc(b[1])}</option>`).join('');
  const segOpts = [[0, 'Default (' + fmtDur(settings ? settings.storage.segment_seconds : 300) + ')'], [300, '5 minutes'], [900, '15 minutes'], [1800, '30 minutes'], [3600, '1 hour'], [7200, '2 hours'], [21600, '6 hours'], [86400, 'One long file per day']];
  $('f-cam').innerHTML = `
    <div class="head"><h2>${clone ? 'Clone camera' : id ? 'Edit camera' : 'Add camera'}</h2><button type="button" class="small" data-close>✕</button></div>
    <div class="body">
      <div class="profile-bar"><b>Profile</b>
        <select id="c-prof"><option value="">Choose a saved profile…</option></select>
        <button type="button" class="small" id="c-prof-apply" title="Fill in this profile's recording, motion and AI settings">Apply</button>
        <button type="button" class="small" id="c-prof-save" title="Save this camera's recording, motion and AI settings as a profile">💾 Save as profile…</button>
        <button type="button" class="small" id="c-prof-del" title="Delete the chosen profile">✕</button>
        <span class="hint" id="c-prof-msg">Profiles hold recording, motion and AI settings (not the motion mask).</span></div>
      <div class="tabs" id="c-tabs"><button type="button" data-t="general" class="active">General</button><button type="button" data-t="recording">Recording</button><button type="button" data-t="detection">Motion &amp; AI</button><button type="button" data-t="osd">On-screen display</button><button type="button" data-t="ptz">Pan / tilt / zoom</button><button type="button" data-t="device">Camera settings</button></div>
      <div data-tab="general" class="form">
        <label for="c-name">Name</label><div><input type="text" id="c-name" required value="${esc(c.name)}" placeholder="Front door"></div>
        <span></span><label class="check"><input type="checkbox" id="c-enabled" ${c.enabled ? 'checked' : ''}> Camera enabled</label>
        <h3>Quick setup</h3>
        <label for="q-brand">Camera brand</label>
        <div><div class="row"><select id="q-brand">${brandOpts.replace('Format…', 'Choose brand…')}</select></div>
          <div class="hint">Fill in the camera's address and login, pick the brand, and the stream URLs are filled in for you.</div></div>
        <label for="q-ip">IP address</label><div class="row"><input type="text" id="q-ip" value="${esc(q.ip || '')}" placeholder="192.168.1.100" style="flex:2"><input type="text" id="q-port" value="${esc(q.port || '')}" placeholder="port (554)" style="flex:1"><input type="text" id="q-ch" placeholder="channel 1" style="flex:1"></div>
        <label for="q-user">Camera login</label><div class="row"><input type="text" id="q-user" value="${esc(q.user || '')}" placeholder="username" style="flex:1" autocomplete="off"><input type="password" id="q-pass" value="${esc(q.pass || '')}" placeholder="password" style="flex:1" autocomplete="new-password"></div>
        <h3>Streams</h3>
        <label for="c-url">Main stream (HD)</label>
        <div><input type="text" id="c-url" required value="${esc(c.url)}" placeholder="rtsp://user:password@192.168.1.50:554/stream1">
          <div class="row" style="margin-top:6px"><select id="c-url-fmt">${brandOpts}</select><span class="hint" style="margin:0">Pick a format to fill this field from the quick setup above.</span></div></div>
        <label for="c-live">Sub-stream (low resolution)</label>
        <div><input type="text" id="c-live" value="${esc(c.live_url)}" placeholder="Optional – e.g. …/stream2">
          <div class="row" style="margin-top:6px"><select id="c-live-fmt">${brandOpts}</select><span class="hint" style="margin:0">Used for the live grid, motion detection and AI. Saves a lot of CPU.</span></div></div>
        <label for="c-transport">RTSP transport</label>
        <div><select id="c-transport"><option value="tcp">TCP (recommended)</option><option value="udp">UDP</option></select></div>
        <span></span><div><button type="button" id="c-test">▶ Test connection</button><div id="c-probe"></div></div>
      </div>

      <div data-tab="recording" class="form" hidden>
        <label>Recording mode</label>
        <div class="radio-cards" style="grid-template-columns:1fr">
          ${[['continuous', 'HD 24/7', 'Records the main stream around the clock.'],
             ['continuous_sub', 'Sub-stream 24/7', 'Records the low-resolution stream around the clock. Uses much less space.'],
             ['sub_plus_events', 'Sub-stream 24/7 + HD on alerts', 'Low-res around the clock, plus full-quality clips around motion/AI alerts.'],
             ['events', 'HD on alerts only', 'Only full-quality clips around motion/AI alerts. Uses the least space.'],
             ['off', 'Off', 'Live view (and alerts) only, nothing recorded.']]
            .map(([v, t, d]) => `<label><input type="radio" name="c-mode" value="${v}" ${c.record_mode === v ? 'checked' : ''}><b>${t}</b><small>${d}</small></label>`).join('')}
        </div>
        <span></span><div class="hint" id="c-mode-hint">Alert modes keep a rolling buffer of full-quality video, so each clip includes the seconds <i>before</i> the alert (set under Motion &amp; AI).</div>
        <span></span><label class="check"><input type="checkbox" id="c-sched-on" ${c.schedule_enabled ? 'checked' : ''}> Only record on a schedule</label>
        <label class="sched">Schedule</label>
        <div class="sched"><div class="chips" id="c-days">${DAYS.map((d, i) => `<label><input type="checkbox" value="${i}" ${c.schedule.days.includes(i) ? 'checked' : ''}>${d}</label>`).join('')}</div>
          <div class="row" style="margin-top:8px"><input type="time" id="c-start" value="${esc(c.schedule.start)}" style="width:auto"><span class="muted">to</span><input type="time" id="c-end" value="${esc(c.schedule.end)}" style="width:auto"></div>
          <div class="hint">If the end is earlier than the start, recording runs overnight. Same start and end = all day.</div></div>
        <h3>Video</h3>
        <span></span><label class="check"><input type="checkbox" id="c-audio" ${c.record_audio ? 'checked' : ''}> Record audio</label>
        <label for="c-fps">Recording frame rate</label>
        <div><select id="c-fps">${[0, 1, 2, 5, 10, 12, 15, 20, 25, 30].map(f => `<option value="${f}" ${c.record_fps === f ? 'selected' : ''}>${f ? f + ' fps' : "Camera's own (no re-encoding)"}</option>`).join('')}</select>
          <div class="hint">A lower frame rate saves space but re-encodes the video (uses the GPU encoder if set up).</div></div>
        <span></span><div><label class="check"><input type="checkbox" id="c-transcode" ${c.transcode ? 'checked' : ''}> Convert recordings to H.264</label>
          <div class="hint">For H.265 cameras, so recordings play in every browser.</div></div>
        <label for="c-seg">File length</label>
        <div><select id="c-seg">${segOpts.map(([v, t]) => `<option value="${v}" ${c.segment_seconds === v ? 'selected' : ''}>${t}</option>`).join('')}</select>
          <div class="hint">How long each recording file is.</div></div>
        <h3>Storage</h3>
        <label for="c-dir">Storage folder</label>
        <div><div class="row"><input type="text" id="c-dir" value="${esc(c.storage_dir)}" placeholder="Default: ${esc(settings ? settings.storage.dir : 'recordings')}" style="flex:1"><button type="button" id="c-browse">Browse…</button></div>
          ${id && !clone ? `<label class="check" id="c-move-row" hidden><input type="checkbox" id="c-move" checked> Move existing recordings to the new folder</label>` : ''}
          <div class="hint">Any folder this computer can write to: another drive (<code>D:\\NVR</code>), a NAS (<code>\\\\NAS\\cameras</code> or <code>/mnt/nas</code>), or a synced cloud folder (<code>G:\\My Drive\\NVR</code> with Google Drive for desktop). A sub-folder named after the camera is created.</div></div>
        <label for="c-ret">Keep recordings for</label>
        <div class="row"><input type="number" id="c-ret" min="0" value="${c.retention_days}" style="width:110px"><span class="muted">days (0 = use the default, ${settings ? settings.storage.retention_days : 14})</span></div>
        <label for="c-max">Space for this camera</label>
        <div class="row"><input type="number" id="c-max" min="0" step="0.5" value="${c.max_storage_gb}" style="width:110px"><span class="muted">GB (0 = no limit; oldest recordings are deleted above it)</span></div>
      </div>

      <div data-tab="detection" class="form" hidden>
        <span></span><label class="check"><input type="checkbox" id="d-motion" ${c.detect.motion ? 'checked' : ''}> Motion detection</label>
        <label for="d-contrast">Min. contrast</label>
        <div><div class="row"><input type="range" id="d-contrast" min="4" max="100" value="${c.detect.min_contrast}" style="flex:1"><span id="d-contrast-v" class="muted" style="width:40px">${c.detect.min_contrast}</span></div>
          <div class="hint">How different a pixel must be from the background to count as changed. Lower = more sensitive (night/IR may need lower); higher ignores noise and small light changes.</div></div>
        <label for="d-objpx">Min. object size</label>
        <div><div class="row"><input type="number" id="d-objpx" step="any" min="0" max="230400" value="${c.detect.min_object_px}" style="width:120px"><span class="muted">pixels (of a 640×360 picture)</span></div>
          <div class="hint">Moving areas smaller than this are ignored (rain, insects, noise). For scale: a person filling a quarter of the picture height ≈ 2,000 px; far away ≈ 200 px. Use the live readout below to choose.</div></div>
        <label for="d-make">Make time</label>
        <div class="row"><input type="number" id="d-make" min="0" max="30" step="0.1" value="${c.detect.make_time_ms / 1000}" style="width:120px"><span class="muted">seconds of motion before it triggers</span></div>
        <label for="d-travel">Object must travel</label>
        <div class="row"><input type="number" id="d-travel" min="0" max="640" step="10" value="${c.detect.min_travel_px}" style="width:120px"><span class="muted">pixels (0 = off). Ignores trees and flags moving in place.</span></div>
        <label>Ignore areas</label>
        <div><div class="surface" id="d-mask" style="aspect-ratio:16/9;max-width:560px;touch-action:none;cursor:crosshair">
            <img alt="" id="d-mask-img" style="object-fit:fill"><div class="msg" id="d-mask-msg">${id && !clone ? '' : 'Save the camera first to see its picture here.'}</div>
            <div id="d-mask-grid" style="position:absolute;inset:0;display:grid;grid-template-columns:repeat(32,1fr);grid-template-rows:repeat(18,1fr)"></div>
            <svg id="d-mask-boxes" style="position:absolute;inset:0;width:100%;height:100%;pointer-events:none" viewBox="0 0 1 1" preserveAspectRatio="none"></svg></div>
          <div class="row" style="margin-top:6px"><button type="button" class="small" id="d-mask-clear">Clear</button><button type="button" class="small" id="d-mask-invert">Invert</button>
            <span class="hint" style="margin:0">Drag over the picture to mark areas to ignore (red). Drag again to unmark.</span></div>
          <div id="d-meter" class="hint" style="margin-top:8px"></div></div>
        <span></span><div><label class="check"><input type="checkbox" id="d-ai" ${c.detect.ai ? 'checked' : ''}> Object recognition (AI)</label>
          <div class="hint">${aiOn ? `Uses <b>${esc(settings.ai.provider)}</b>. Pictures are only sent to the AI while motion is seen (needs motion detection).` : '<span class="warnc">No AI is set up yet.</span> Choose one in Settings → Object recognition.'}</div></div>
        <label>Alert on</label>
        <div class="chips" id="d-objs">${objs.map(o => `<label><input type="checkbox" value="${o}" ${c.detect.objects.includes(o) ? 'checked' : ''}>${o}</label>`).join('')}</div>
        <label for="d-conf">Minimum confidence</label>
        <div class="row"><input type="range" id="d-conf" min="5" max="95" value="${Math.round(c.detect.min_confidence * 100)}" style="flex:1"><span id="d-conf-v" class="muted" style="width:40px">${Math.round(c.detect.min_confidence * 100)}%</span></div>
        <label for="d-trig">What makes an alert</label>
        <div><select id="d-trig"><option value="motion">Any motion</option><option value="objects">Only recognized objects (ignores trees, shadows, rain)</option></select></div>
        <label for="d-pre">Record before alert</label>
        <div class="row"><input type="number" id="d-pre" min="0" max="120" value="${c.detect.pre_seconds}" style="width:110px"><span class="muted">seconds (pre-event buffer, in alert recording modes)</span></div>
        <label for="d-post">Record after activity stops</label>
        <div class="row"><input type="number" id="d-post" min="3" max="600" value="${c.detect.post_seconds}" style="width:110px"><span class="muted">seconds</span></div>
        <label for="d-fps">Analysis frame rate</label>
        <div class="row"><input type="number" id="d-fps" min="1" max="15" value="${c.detect.fps}" style="width:110px"><span class="muted">pictures per second checked for motion</span></div>
        <label for="d-int">AI check interval</label>
        <div class="row"><input type="number" id="d-int" min="0.2" max="60" step="0.1" value="${c.detect.ai_interval}" style="width:110px"><span class="muted">seconds between AI checks during motion (raise this for cloud AI)</span></div>
      </div>
      <div data-tab="osd" class="form" hidden>
        <span></span><div><label class="check"><input type="checkbox" id="o-custom" ${c.osd.custom ? 'checked' : ''}> Use settings just for this camera</label>
          <div class="hint">Off: this camera uses Settings → Live view &amp; display.</div></div>
        <label>Preview</label>
        <div class="surface" id="o-prev" style="aspect-ratio:16/9;max-width:560px"><img alt="" style="object-fit:fill"><div class="msg">${id && !clone ? '' : 'Save the camera to see its picture.'}</div></div>
        ${[['name', 'Camera name'], ['clock', 'Date &amp; time'], ['status', 'Recording status (REC / armed / offline)'], ['detections', 'What is moving (motion, 🚶 person, 🚗 car …)'], ['fps', 'Frame rate and stream details']]
          .map(([k, t]) => `<span></span><div class="row osd-row"><label class="check" style="padding:0;min-width:300px"><input type="checkbox" id="o-${k}" ${c.osd['show_' + k] ? 'checked' : ''}> ${t}</label>${cornerSel('o-' + k + '-c', c.osd[k + '_corner'])}</div>`).join('')}
        <label for="o-fmt2">Date &amp; time format</label>
        <div><select id="o-fmt2"><option value="">Same as everywhere (${esc(status.display.clock_format)})</option>${CLOCK_FORMATS.map(f => `<option ${f === c.osd.clock_format ? 'selected' : ''}>${f}</option>`).join('')}<option value="custom">Custom…</option></select>
          <input type="text" id="o-fmt2-c" value="${esc(c.osd.clock_format)}" hidden style="margin-top:6px" placeholder="e.g. DD/MM/YYYY HH:mm:ss"></div>
        <label for="o-text">Extra text</label>
        <div class="row"><input type="text" id="o-text" value="${esc(c.osd.text)}" maxlength="200" placeholder="e.g. Driveway – north" style="flex:1">${cornerSel('o-text-c', c.osd.text_corner)}</div>
      </div>

      <div data-tab="ptz" class="form" hidden>
        <span></span><div><label class="check"><input type="checkbox" id="p-on" ${c.ptz.enabled ? 'checked' : ''}> This camera can pan, tilt or zoom</label>
          <div class="hint">Uses ONVIF, which most PTZ cameras support (you may need to turn ONVIF on in the camera's own settings, and some brands need a separate ONVIF user).</div></div>
        <label for="p-host">Camera address</label><div><input type="text" id="p-host" value="${esc(c.ptz.host)}" placeholder="Same as the stream (${esc((parseUrl(c.url) || {}).ip || 'from the URL')})"></div>
        <label for="p-port">ONVIF port</label><div class="row"><input type="number" id="p-port" min="1" max="65535" value="${c.ptz.port}" style="width:110px">
          <select id="p-port-pick"><option value="">Common ports…</option><option value="80">80 – Hikvision, Dahua, Amcrest, most cameras</option><option value="8000">8000 – Reolink</option><option value="2020">2020 – TP-Link Tapo</option><option value="8899">8899 – many budget cameras</option><option value="8080">8080</option><option value="5000">5000</option></select></div>
        <label for="p-user">ONVIF login</label><div class="row"><input type="text" id="p-user" value="${esc(c.ptz.username)}" placeholder="Same as stream" style="flex:1" autocomplete="off"><input type="password" id="p-pass" value="${esc(c.ptz.password)}" placeholder="Same as stream" style="flex:1" autocomplete="new-password"></div>
        <label for="p-speed">Speed</label><div class="row"><input type="range" id="p-speed" min="10" max="100" value="${Math.round(c.ptz.speed * 100)}" style="flex:1"><span class="muted" id="p-speed-v" style="width:44px">${Math.round(c.ptz.speed * 100)}%</span></div>
        <span></span><div class="row"><label class="check"><input type="checkbox" id="p-inv-pan" ${c.ptz.invert_pan ? 'checked' : ''}> Reverse left/right</label><label class="check"><input type="checkbox" id="p-inv-tilt" ${c.ptz.invert_tilt ? 'checked' : ''}> Reverse up/down</label></div>
        <span></span><div><button type="button" id="p-test">Test PTZ connection</button><div id="p-result" class="hint" style="margin-top:6px"></div></div>
      </div>
      <div data-tab="device" hidden><div id="dv-body"></div></div>
      <div class="err" id="c-err" style="margin-top:12px" hidden></div>
    </div>
    <div class="foot"><button type="button" data-close>Cancel</button><button class="primary" id="c-save">Save camera</button></div>`;
  $('c-transport').value = c.rtsp_transport;
  $('d-trig').value = c.detect.trigger;
  const f = $('f-cam');
  f.querySelectorAll('[data-close]').forEach(b => b.onclick = closeCameraDialog);
  f.querySelectorAll('#c-tabs button').forEach(b => b.onclick = () => {
    f.querySelectorAll('#c-tabs button').forEach(x => x.classList.toggle('active', x === b));
    f.querySelectorAll('[data-tab]').forEach(t => t.hidden = t.dataset.tab !== b.dataset.t);
    if (b.dataset.t === 'device' && !deviceLoaded) loadDeviceTab();
  });
  deviceLoaded = false;
  setupProfiles();
  // A field the browser rejects may be on a hidden tab: show that tab and say why.
  f.addEventListener('invalid', (e) => {
    const tab = e.target.closest('[data-tab]');
    if (tab && tab.hidden) f.querySelector(`#c-tabs button[data-t="${tab.dataset.tab}"]`).click();
    $('c-err').hidden = false;
    $('c-err').textContent = `${(f.querySelector(`label[for="${e.target.id}"]`) || {}).textContent || 'A field'}: ${e.target.validationMessage}`;
  }, true);
  const syncSched = () => f.querySelectorAll('.sched').forEach(e => e.hidden = !chk('c-sched-on'));
  $('c-sched-on').onchange = syncSched; syncSched();
  // Object recognition runs only on motion, so it needs motion detection.
  $('d-ai').onchange = () => { if (chk('d-ai')) $('d-motion').checked = true; };
  $('d-motion').onchange = () => { if (!chk('d-motion')) $('d-ai').checked = false; };
  $('d-contrast').oninput = () => $('d-contrast-v').textContent = val('d-contrast');
  setupMaskEditor(c.detect.mask, id && !clone ? id : null);
  $('d-objpx').oninput = () => drawMotionBoxes($('d-mask-boxes'), lastMeterObjects);
  drawMotionBoxes($('d-mask-boxes'), []);
  setupOsdTab(c, id && !clone ? id : null);
  $('p-port-pick').onchange = (e) => { if (e.target.value) $('p-port').value = e.target.value; e.target.value = ''; };
  $('p-speed').oninput = () => $('p-speed-v').textContent = val('p-speed') + '%';
  $('p-test').onclick = async () => {
    $('p-result').textContent = 'Connecting…';
    const camBody = {...collectCamera(), id: id || ''};
    const r = await api('api/ptz/test', {method: 'POST', json: {camera: camBody}}).catch(e => ({ok: false, error: e.message}));
    $('p-result').innerHTML = r.ok ? `<span class="ok">✓ PTZ works.</span> ${r.presets.length} preset(s)${r.presets.length ? ': ' + r.presets.map(p => esc(p.name)).join(', ') : ''}.` : `<span class="err">✗ ${esc(r.error)}</span>`;
  };
  $('d-conf').oninput = () => $('d-conf-v').textContent = val('d-conf') + '%';
  $('q-brand').onchange = () => {
    const b = val('q-brand'); if (!b) return;
    const brand = BRANDS.find(x => x[0] === b);
    if (!val('q-port')) $('q-port').placeholder = 'port (' + brand[2] + ')';
    $('c-url').value = brandUrl(b, 'main'); $('c-live').value = brandUrl(b, 'sub');
  };
  // Changing the address or login updates the stream URLs: from the brand
  // template if one is chosen, otherwise by editing the existing URLs (e.g.
  // after cloning a camera, just type the new IP).
  for (const k of ['q-ip', 'q-port', 'q-ch', 'q-user', 'q-pass']) $(k).oninput = () => {
    if (val('q-brand')) return $('q-brand').onchange();
    for (const id of ['c-url', 'c-live']) $(id).value = rewriteUrl(val(id));
  };
  $('c-url-fmt').onchange = (e) => { if (e.target.value) $('c-url').value = brandUrl(e.target.value, 'main'); e.target.value = ''; };
  $('c-live-fmt').onchange = (e) => { if (e.target.value) $('c-live').value = brandUrl(e.target.value, 'sub'); e.target.value = ''; };
  $('c-test').onclick = testCamera;
  const origDir = c.storage_dir;
  $('c-dir').oninput = () => { if ($('c-move-row')) $('c-move-row').hidden = val('c-dir').trim() === origDir; };
  $('c-browse').onclick = async () => {
    const p = await pickFolder(val('c-dir') || (settings ? settings.storage.dir : ''), 'Storage folder for this camera');
    if (p) { $('c-dir').value = p; $('c-dir').oninput(); }
  };
  f.onsubmit = saveCamera;
  $('cam-dialog').showModal();
}
function closeCameraDialog() { stopPreviews(); $('cam-dialog').close(); }
$('cam-dialog').addEventListener('close', () => { stopPreviews(); stopMotionMeter(); if (osdPrevUnbind) { osdPrevUnbind(); osdPrevUnbind = null; } });

// ---------- motion mask editor & live motion readout ----------
let maskCells = [], maskUnbind = null, meterTimer = null;
const MASK_W = 32, MASK_H = 18;
function maskValue() { return maskCells.some(x => x) ? maskCells.map(x => x ? '1' : '0').join('') : ''; }
function setupMaskEditor(mask, camId) {
  maskCells = mask && mask.length === MASK_W * MASK_H ? [...mask].map(c => c === '1') : new Array(MASK_W * MASK_H).fill(false);
  const grid = $('d-mask-grid');
  grid.innerHTML = maskCells.map((on, i) => `<div data-i="${i}" style="${on ? 'background:rgba(239,68,68,.45)' : ''};border:1px solid rgba(255,255,255,.06)"></div>`).join('');
  const paint = () => grid.querySelectorAll('[data-i]').forEach(d => d.style.background = maskCells[+d.dataset.i] ? 'rgba(239,68,68,.45)' : '');
  let mode = null;
  const cellAt = (e) => { const r = grid.getBoundingClientRect(); const x = Math.floor((e.clientX - r.left) / r.width * MASK_W), y = Math.floor((e.clientY - r.top) / r.height * MASK_H);
    return x >= 0 && x < MASK_W && y >= 0 && y < MASK_H ? y * MASK_W + x : -1; };
  // 3×3-cell brush.
  const set = (e) => {
    const i = cellAt(e); if (i < 0) return;
    const cx = i % MASK_W, cy = Math.floor(i / MASK_W);
    let changed = false;
    for (let y = Math.max(0, cy - 1); y <= Math.min(MASK_H - 1, cy + 1); y++)
      for (let x = Math.max(0, cx - 1); x <= Math.min(MASK_W - 1, cx + 1); x++)
        if (maskCells[y * MASK_W + x] !== mode) { maskCells[y * MASK_W + x] = mode; changed = true; }
    if (changed) paint();
  };
  grid.onpointerdown = (e) => { const i = cellAt(e); if (i < 0) return; mode = !maskCells[i]; grid.setPointerCapture(e.pointerId); set(e); };
  grid.onpointermove = (e) => { if (mode !== null) set(e); };
  grid.onpointerup = grid.onpointercancel = () => { mode = null; };
  $('d-mask-clear').onclick = () => { maskCells.fill(false); paint(); };
  $('d-mask-invert').onclick = () => { maskCells = maskCells.map(x => !x); paint(); };
  if (maskUnbind) maskUnbind();
  maskUnbind = null;
  stopMotionMeter();
  if (camId) {
    const img = $('d-mask-img');
    img.onload = () => $('d-mask-msg').hidden = true;
    maskUnbind = Live.bind(img, camId, 'sub');
    meterTimer = setInterval(() => updateMotionMeter(camId), 500);
  }
}
function stopMotionMeter() { clearInterval(meterTimer); meterTimer = null; if (maskUnbind) { maskUnbind(); maskUnbind = null; } }
async function updateMotionMeter(camId) {
  const m = await api(`api/cameras/${enc(camId)}/motion`).catch(() => null);
  if (!m || !$('d-meter')) return;
  const need = m.min_object_px;
  $('d-meter').innerHTML = !m.running ? 'Live readout: motion detection is not running for this camera (turn it on and save).' :
    `Live readout (saved settings): biggest moving area now <b>${m.biggest_px.toLocaleString()} px</b>, peak in the last 3 s <b>${m.peak_px.toLocaleString()} px</b>; an alert needs ≥ ${need.toLocaleString()} px — ` +
    (m.moving ? '<span class="ok">triggering</span>' : m.peak_px >= need ? '<span class="warnc">big enough (waiting for make time / travel)</span>' : 'too small');
  drawMotionBoxes($('d-mask-boxes'), m.objects);
}
// Boxes around moving objects, in an SVG whose viewBox is 0..1.
let lastMeterObjects = [];
function drawMotionBoxes(svg, objects) {
  lastMeterObjects = objects || [];
  let out = lastMeterObjects.map(o => `<rect x="${o.x}" y="${o.y}" width="${o.w}" height="${o.h}" fill="none" stroke="#f59e0b" stroke-width="2" vector-effect="non-scaling-stroke"/>`).join('');
  // How big "min. object size" is: a person-shaped box (1 wide : 2 tall) of
  // that many pixels on a 640×360 picture, in the middle of the picture.
  const px = $('d-objpx') ? +$('d-objpx').value : 0;
  if (px > 0) {
    const wpx = Math.sqrt(px / 2), w = Math.min(1, wpx / 640), h = Math.min(1, wpx * 2 / 360);
    out += `<rect x="${(1 - w) / 2}" y="${(1 - h) / 2}" width="${w}" height="${h}" fill="rgba(79,140,255,.18)" stroke="#4f8cff" stroke-width="2" stroke-dasharray="6 4" vector-effect="non-scaling-stroke"/>`;
  }
  svg.innerHTML = out;
}
function cornerSel(id, cur) {
  return `<select id="${id}">${[['tl', 'Top left'], ['tr', 'Top right'], ['bl', 'Bottom left'], ['br', 'Bottom right']].map(([k, t]) => `<option value="${k}" ${k === cur ? 'selected' : ''}>${t}</option>`).join('')}</select>`;
}
// The camera's own on-screen display choices (kept while the controls show the global ones).
let osdOwn = null;
function readOsd(raw) {
  if (!raw && !chk('o-custom') && osdOwn) return {...osdOwn, custom: false};
  const f = val('o-fmt2') === 'custom' ? val('o-fmt2-c').trim() : val('o-fmt2');
  return {custom: chk('o-custom'), show_name: chk('o-name'), name_corner: val('o-name-c'), show_clock: chk('o-clock'), clock_corner: val('o-clock-c'), clock_format: f,
    show_status: chk('o-status'), status_corner: val('o-status-c'), show_detections: chk('o-detections'), detections_corner: val('o-detections-c'),
    show_fps: chk('o-fps'), fps_corner: val('o-fps-c'), text: val('o-text').trim(), text_corner: val('o-text-c')};
}
let osdPrevUnbind = null;
function setupOsdTab(c, camId) {
  const prev = $('o-prev');
  if (!CLOCK_FORMATS.includes(c.osd.clock_format) && c.osd.clock_format) { $('o-fmt2').value = 'custom'; $('o-fmt2-c').hidden = false; }
  const live = camId ? cam(camId) : null;
  const ctrls = ['o-name', 'o-name-c', 'o-clock', 'o-clock-c', 'o-status', 'o-status-c', 'o-detections', 'o-detections-c', 'o-fps', 'o-fps-c', 'o-fmt2', 'o-fmt2-c', 'o-text', 'o-text-c'];
  osdOwn = null;
  const showFollow = () => {
    const custom = chk('o-custom');
    if (!custom) {
      // Following Settings → Live view & display: show those values, read-only.
      if (osdOwn === null || $('o-name').disabled === false) osdOwn = readOsd(true);
      const g = osdFor({osd: {custom: false}});
      for (const k of ['name', 'clock', 'status', 'detections', 'fps']) { $('o-' + k).checked = !!g['show_' + k]; $('o-' + k + '-c').value = g[k + '_corner']; }
      $('o-fmt2').value = ''; $('o-fmt2-c').hidden = true;
    } else if (osdOwn && $('o-name').disabled) {
      for (const k of ['name', 'clock', 'status', 'detections', 'fps']) { $('o-' + k).checked = !!osdOwn['show_' + k]; $('o-' + k + '-c').value = osdOwn[k + '_corner']; }
    }
    ctrls.forEach(id => { if ($(id)) $(id).disabled = !custom; });
  };
  $('o-custom').addEventListener('change', showFollow);
  showFollow();
  const redraw = () => {
    prev.querySelectorAll('.ov').forEach(o => o.remove());
    const fake = {...(live || {id: '', name: val('c-name') || 'Camera', enabled: true, recorder: null, buffer: null, detect: null, record_mode: 'continuous'}), osd: {...readOsd(), custom: true}};
    if (!chk('o-custom')) fake.osd = {custom: false};
    fake.name = val('c-name') || fake.name;
    prev.insertAdjacentHTML('beforeend', overlayHtml(fake, 'sub'));
    if (live) updateOverlay(prev, fake, 'sub'); else prev.querySelectorAll('.ov-clock').forEach(e => e.textContent = fmtDate(new Date(), e.dataset.fmt || undefined));
  };
  $('f-cam').querySelector('[data-tab=osd]').addEventListener('input', redraw);
  $('f-cam').querySelector('[data-tab=osd]').addEventListener('change', () => { $('o-fmt2-c').hidden = val('o-fmt2') !== 'custom'; redraw(); });
  if (osdPrevUnbind) osdPrevUnbind();
  osdPrevUnbind = camId ? Live.bind(prev.querySelector('img'), camId, 'sub') : null;
  redraw();
}

async function testCamera() {
  const btn = $('c-test'), out = $('c-probe');
  stopPreviews();
  const urls = [['Main stream', val('c-url').trim()], ['Sub-stream', val('c-live').trim()]].filter(x => x[1]);
  if (!urls.length) { out.innerHTML = '<div class="err" style="margin-top:8px">Enter a stream URL first.</div>'; return; }
  btn.disabled = true; btn.textContent = 'Connecting…';
  out.innerHTML = `<div class="probe-grid">${urls.map((u, i) => `<div><div class="surface"><img id="pv-${i}" alt=""><div class="msg" id="pv-msg-${i}">Connecting…</div></div><div id="pv-info-${i}" class="hint"><b>${u[0]}</b></div></div>`).join('')}</div>`;
  await Promise.all(urls.map(async ([label, url], i) => {
    const transport = val('c-transport');
    try {
      const [r, pv] = await Promise.all([api('api/probe', {method: 'POST', json: {url, rtsp_transport: transport}}), api('api/preview', {method: 'POST', json: {url, rtsp_transport: transport}})]);
      const info = $('pv-info-' + i), msg = $('pv-msg-' + i);
      if (!r.ok) { msg.innerHTML = `<span class="err">✗ ${esc(explainError(r.error || 'Could not connect'))}</span>`; info.innerHTML = `<b>${label}</b>`; return; }
      const img = $('pv-' + i);
      img.onload = () => msg.hidden = true;
      img.src = pv.url; previewImgs.push(img);
      const v = r.streams.find(s => s.kind === 'video'), a = r.streams.find(s => s.kind === 'audio');
      const hevc = v && /hevc|h265/i.test(v.codec);
      info.innerHTML = `<b>${label}</b> <span class="ok">✓ connected</span><br>${v ? `${String(v.codec).toUpperCase()} ${v.width}×${v.height}${v.fps ? ' @ ' + v.fps + ' fps' : ''}` : ''}${a ? ' · audio ' + esc(a.codec) : ' · no audio'}`
        + (hevc && i === 0 ? '<br><span class="warnc">H.265: many browsers cannot play it. Consider “Convert recordings to H.264” or setting the camera to H.264.</span>' : '');
    } catch (e) { $('pv-msg-' + i).innerHTML = `<span class="err">${esc(e.message)}</span>`; }
  }));
  btn.disabled = false; btn.textContent = '▶ Test connection';
}

async function saveCamera(ev) {
  ev.preventDefault();
  const days = [...document.querySelectorAll('#c-days input:checked')].map(i => +i.value);
  const mode = document.querySelector('input[name=c-mode]:checked').value;
  const objects = [...document.querySelectorAll('#d-objs input:checked')].map(i => i.value);
  const body = collectCamera();
  const err = (m) => { $('c-err').hidden = false; $('c-err').textContent = m; };
  if (body.schedule_enabled && !days.length) return err('Choose at least one day for the schedule.');
  if (body.detect.ai && !objects.length) return err('Choose at least one object to alert on.');
  return finishSave(body);
}
// Read the camera form.
function collectCamera() {
  const days = [...document.querySelectorAll('#c-days input:checked')].map(i => +i.value);
  const mode = document.querySelector('input[name=c-mode]:checked').value;
  const objects = [...document.querySelectorAll('#d-objs input:checked')].map(i => i.value);
  return {
    name: val('c-name').trim(), enabled: chk('c-enabled'), url: val('c-url').trim(), live_url: val('c-live').trim(),
    rtsp_transport: val('c-transport'), record_mode: mode, schedule_enabled: chk('c-sched-on'),
    schedule: {days, start: val('c-start') || '00:00', end: val('c-end') || '00:00'},
    record_audio: chk('c-audio'), transcode: chk('c-transcode'), record_fps: num('c-fps'), segment_seconds: num('c-seg'),
    storage_dir: val('c-dir').trim(), retention_days: Math.round(num('c-ret')), max_storage_gb: num('c-max'),
    osd: readOsd(),
    view: (cameraCfgs.find(x => x.id === editingId) || {}).view || {grid_quality: 'auto', grid_fps: 0, hd_fps: 0},
    ptz: {enabled: chk('p-on'), host: val('p-host').trim(), port: Math.round(num('p-port')) || 80, username: val('p-user').trim(), password: $('p-pass').value,
      speed: num('p-speed') / 100, invert_pan: chk('p-inv-pan'), invert_tilt: chk('p-inv-tilt')},
    detect: {motion: chk('d-motion'), min_contrast: num('d-contrast'), min_object_px: Math.round(num('d-objpx')),
      make_time_ms: Math.round(num('d-make') * 1000), min_travel_px: Math.round(num('d-travel')), mask: maskValue(), ai: chk('d-ai'), objects, min_confidence: num('d-conf') / 100,
      trigger: val('d-trig'), pre_seconds: Math.round(num('d-pre')), post_seconds: Math.round(num('d-post')), fps: Math.round(num('d-fps')), ai_interval: num('d-int')},
  };
}
// ---------- camera's own settings over ONVIF ----------
let deviceLoaded = false, deviceReport = null;
const prefixMask = (p) => { const m = p ? (0xffffffff << (32 - p)) >>> 0 : 0; return [24, 16, 8, 0].map(s => (m >>> s) & 255).join('.'); };
async function loadDeviceTab() {
  const box = $('dv-body');
  if (!editingId) { box.innerHTML = '<div class="hint">Save the camera first, then its own settings can be changed here.</div>'; return; }
  deviceLoaded = true;
  box.innerHTML = '<div class="hint"><span class="spinner small"></span> Asking the camera for its settings (ONVIF)…</div>';
  let res;
  try { res = await api(`api/cameras/${enc(editingId)}/device`); } catch (e) {
    box.innerHTML = `<div class="err">${esc(e.message)}</div>`; deviceLoaded = false; return;
  }
  const r = deviceReport = res.report, er = r.errors || {};
  const na = (k) => er[k] ? `<div class="hint">Not available on this camera: ${esc(er[k])}</div>` : '';
  const intro = `<div class="hint" style="margin-bottom:10px">These are the camera's own settings, changed directly on the camera over ONVIF (${esc(res.address)}).
    The address, port and login come from the <b>Pan / tilt / zoom</b> tab (blank there = from the stream URL). Each section is saved to the camera on its own button.
    <button type="button" class="small" id="dv-reload" style="margin-left:6px">↻ Reload</button></div>`;
  if (er.connection) {
    box.innerHTML = intro + `<div class="err">Could not reach the camera: ${esc(er.connection)}</div>
      <div class="hint">Check that ONVIF is turned on in the camera (some brands need a separate ONVIF user) and the port on the Pan / tilt / zoom tab (80 for most, 2020 for Tapo, 8000 for Reolink).</div>`;
    $('dv-reload').onclick = loadDeviceTab; deviceLoaded = false; return;
  }
  const i = r.info || {}, n = r.network, t = r.time, im = r.imaging;
  let h = intro;
  h += `<div class="dv-sec"><h3>Device</h3><dl class="kv">
    <dt>Make &amp; model</dt><dd>${esc(i.manufacturer)} ${esc(i.model)}</dd><dt>Firmware</dt><dd>${esc(i.firmware) || '–'}</dd>
    <dt>Serial number</dt><dd>${esc(i.serial) || '–'}</dd>${n && n.mac ? `<dt>MAC address</dt><dd>${esc(n.mac)}</dd>` : ''}</dl>
    <button type="button" id="dv-reboot">⟳ Restart camera</button></div>`;
  // Network
  h += `<div class="dv-sec"><h3>Network</h3>${na('network')}`;
  if (n) h += `<div class="form">
    <label>Address</label><div class="row"><label class="check"><input type="radio" name="dv-dhcp" value="0" ${n.dhcp ? '' : 'checked'}> Fixed (static)</label><label class="check"><input type="radio" name="dv-dhcp" value="1" ${n.dhcp ? 'checked' : ''}> Automatic (DHCP)</label></div>
    <label for="dv-ip">IP address</label><div><input type="text" id="dv-ip" value="${esc(n.ip)}" placeholder="192.168.1.64"></div>
    <label for="dv-prefix">Subnet mask</label><div><select id="dv-prefix">${Array.from({length: 17}, (_, k) => 16 + k).map(p => `<option value="${p}" ${p === n.prefix ? 'selected' : ''}>${prefixMask(p)} (/${p})</option>`).join('')}</select></div>
    <label for="dv-gw">Gateway (router)</label><div><input type="text" id="dv-gw" value="${esc(n.gateway)}" placeholder="192.168.1.1"></div>
    <label for="dv-dns">DNS servers</label><div><input type="text" id="dv-dns" value="${esc((n.dns || []).join(', '))}" placeholder="192.168.1.1, 8.8.8.8"></div>
    <label for="dv-host">Host name</label><div><input type="text" id="dv-host" value="${esc(n.hostname)}"></div>
    <span></span><div><label class="check"><input type="checkbox" id="dv-upd" checked> Also change this camera's stream addresses in the NVR</label>
      <div class="hint">Changing the address makes the camera reconnect (some restart). Make sure the new address is free and on your network, or you'll have to find the camera again.</div></div>
    <span></span><div><button type="button" class="primary" id="dv-net-save">Save network settings to camera</button></div></div>`;
  h += '</div>';
  // Time
  h += `<div class="dv-sec"><h3>Date &amp; time</h3>${na('time')}`;
  if (t) {
    const cam = t.utc ? new Date(t.utc) : null, diff = cam ? Math.round((cam - new Date(res.computer_utc)) / 1000) : null;
    h += `<div class="form">
      <label>Camera clock</label><div>${cam ? `${fmtDate(cam)} — ${Math.abs(diff) < 3 ? '<span class="ok">matches this computer</span>' : `<span class="warnc">${fmtDur(Math.abs(diff))} ${diff > 0 ? 'ahead of' : 'behind'} this computer</span>`}` : '–'}
        <div class="hint">A wrong camera clock can make ONVIF logins fail and puts the wrong time on the picture.</div></div>
      <label for="dv-ntp">Time server</label><div><input type="text" id="dv-ntp" value="${esc((t.ntp || []).join(', '))}" placeholder="pool.ntp.org"></div>
      <label for="dv-mode">Keep time from</label><div><select id="dv-mode"><option value="NTP" ${t.mode !== 'Manual' ? 'selected' : ''}>The time server (recommended)</option><option value="Manual" ${t.mode === 'Manual' ? 'selected' : ''}>Its own clock</option></select></div>
      <label for="dv-tz">Time zone</label><div><input type="text" id="dv-tz" value="${esc(t.tz)}" placeholder="e.g. NZST-12NZDT,M9.5.0,M4.1.0/3">
        <div class="hint">In the camera's format (POSIX). Common: <a href="javascript:void 0" data-tz="NZST-12NZDT,M9.5.0,M4.1.0/3">New Zealand</a> · <a href="javascript:void 0" data-tz="AEST-10AEDT,M10.1.0,M4.1.0/3">Sydney</a> · <a href="javascript:void 0" data-tz="GMT0BST,M3.5.0/1,M10.5.0">UK</a> · <a href="javascript:void 0" data-tz="EST5EDT,M3.2.0,M11.1.0">US Eastern</a> · <a href="javascript:void 0" data-tz="PST8PDT,M3.2.0,M11.1.0">US Pacific</a> · <a href="javascript:void 0" data-tz="CET-1CEST,M3.5.0,M10.5.0/3">Central Europe</a></div></div>
      <span></span><div class="row"><button type="button" class="primary" id="dv-time-save">Save time settings</button><button type="button" id="dv-time-sync">Set camera clock to this computer's time</button></div></div>`;
  }
  h += '</div>';
  // Video
  h += `<div class="dv-sec"><h3>Video streams</h3>${na('video')}`;
  (r.video || []).forEach((v, k) => {
    const res2 = v.resolutions.length ? v.resolutions : [[v.width, v.height]];
    const fpsOpts = v.fps_options.length ? v.fps_options : Array.from(new Set([1, 2, 5, 8, 10, 12, 15, 20, 25, 30, v.fps])).filter(x => !v.fps_max || (x >= (v.fps_min || 1) && x <= v.fps_max)).sort((a, b) => a - b);
    h += `<div class="dv-stream" data-k="${k}"><b>${k === 0 ? 'Stream 1 (main)' : k === 1 ? 'Stream 2 (sub)' : 'Stream ' + (k + 1)}</b> <span class="muted">${esc(v.name)}</span>
      <div class="form">
        <label>Video format</label><div>${v.encodings.length > 1 ? `<select class="dv-enc">${v.encodings.map(e => `<option ${e === v.encoding ? 'selected' : ''}>${esc(e)}</option>`).join('')}</select>` : esc(v.encoding)}</div>
        <label>Resolution</label><div><select class="dv-res">${res2.map(([w, hh]) => `<option value="${w}x${hh}" ${w === v.width && hh === v.height ? 'selected' : ''}>${w} × ${hh}</option>`).join('')}</select></div>
        <label>Frame rate</label><div><select class="dv-fps">${fpsOpts.map(x => `<option value="${x}" ${x == v.fps ? 'selected' : ''}>${x} fps</option>`).join('')}</select></div>
        <label>Bitrate</label><div class="row"><input type="number" class="dv-br" value="${v.bitrate}" min="${v.bitrate_min || 32}" ${v.bitrate_max ? `max="${v.bitrate_max}"` : ''} style="width:120px"><span class="muted">kbit/s${v.bitrate_max ? ` (${v.bitrate_min}–${v.bitrate_max})` : ''}</span></div>
        <label>Keyframe interval</label><div class="row"><input type="number" class="dv-gov" value="${v.gov || ''}" min="1" max="400" style="width:120px"><span class="muted">frames (about ${v.fps ? Math.max(1, Math.round((v.gov || 0) / v.fps)) : '?'} s; 1–2 s seeks and starts fastest)</span></div>
        <span></span><div><button type="button" class="dv-video-save">Save stream ${k + 1} to camera</button></div></div></div>`;
  });
  h += '</div>';
  // Picture
  h += `<div class="dv-sec"><h3>Picture</h3>${na('imaging')}`;
  if (im) {
    const sl = (key, label) => im[key] == null ? '' : (() => { const [lo, hi] = (im.ranges || {})[key] || [0, 100]; return `<label>${label}</label><div class="row"><input type="range" id="dv-${key}" min="${lo}" max="${hi}" value="${im[key]}" style="flex:1"><span class="muted" style="width:40px" id="dv-${key}-v">${im[key]}</span></div>`; })();
    const irName = {AUTO: 'Automatic day / night', ON: 'Day (colour)', OFF: 'Night (black & white, infrared)'};
    h += `<div class="form">${sl('brightness', 'Brightness')}${sl('contrast', 'Contrast')}${sl('saturation', 'Colour')}${sl('sharpness', 'Sharpness')}
      ${im.ir_cut != null ? `<label>Day / night</label><div><select id="dv-ir">${(im.ir_modes.length ? im.ir_modes : ['AUTO', 'ON', 'OFF']).map(m => `<option value="${m}" ${m === im.ir_cut ? 'selected' : ''}>${irName[m] || m}</option>`).join('')}</select></div>` : ''}
      <span></span><div><button type="button" class="primary" id="dv-img-save">Save picture settings to camera</button></div></div>`;
  }
  h += '</div>';
  box.innerHTML = h;
  const busy = async (btn, fn) => { const t0 = btn.textContent; btn.disabled = true; btn.textContent = 'Saving…'; try { await fn(); } catch (e) { toast(e.message, true); } finally { btn.disabled = false; btn.textContent = t0; } };
  const url = (p) => `api/cameras/${enc(editingId)}/device/${p}`;
  $('dv-reload').onclick = loadDeviceTab;
  $('dv-reboot').onclick = () => { if (confirm('Restart the camera now? It will be offline for about a minute.')) busy($('dv-reboot'), async () => { await api(url('reboot'), {method: 'POST'}); toast('The camera is restarting.'); }); };
  box.querySelectorAll('[data-tz]').forEach(a => a.onclick = () => $('dv-tz').value = a.dataset.tz);
  box.querySelectorAll('input[type=range]').forEach(el => el.oninput = () => { const v = $(el.id + '-v'); if (v) v.textContent = el.value; });
  const syncDhcp = () => { const d = document.querySelector('input[name=dv-dhcp]:checked'); if (!d) return; ['dv-ip', 'dv-prefix', 'dv-gw', 'dv-dns'].forEach(x => $(x).disabled = d.value === '1'); };
  box.querySelectorAll('input[name=dv-dhcp]').forEach(x => x.onchange = syncDhcp); syncDhcp();
  if ($('dv-net-save')) $('dv-net-save').onclick = () => {
    const dhcp = document.querySelector('input[name=dv-dhcp]:checked').value === '1';
    const body = {token: n.token, mac: n.mac, dhcp, ip: val('dv-ip').trim(), prefix: +val('dv-prefix'), gateway: val('dv-gw').trim(),
      dns: val('dv-dns').split(/[,\s]+/).filter(Boolean), dns_dhcp: n.dns_dhcp, hostname: val('dv-host').trim(), update_nvr: chk('dv-upd')};
    const changing = dhcp !== n.dhcp || body.ip !== n.ip || body.prefix !== n.prefix;
    if (changing && !confirm((dhcp ? 'Switch the camera to an automatic (DHCP) address? Its address may change and the NVR will lose it until you enter the new address.'
      : `Change the camera's address from ${n.ip} to ${body.ip}?`) + '\n\nSome cameras restart to apply a new address; that is done automatically.')) return;
    busy($('dv-net-save'), async () => {
      const r2 = await api(url('network'), {method: 'PUT', json: body});
      toast('Network settings saved on the camera' + (r2.nvr_updated ? '; the NVR now uses the new address' : '')
        + (r2.restarted ? '. The camera is restarting to apply them (about a minute).' : r2.reboot_needed ? '. Restart the camera for them to apply.' : '.'));
      if (r2.nvr_updated) { await loadSettings().catch(() => {}); const c2 = cameraCfgs.find(x => x.id === editingId); if (c2) { $('c-url').value = c2.url; $('c-live').value = c2.live_url; $('p-host').value = c2.ptz.host; } }
    });
  };
  const timeBody = () => ({mode: val('dv-mode'), tz: val('dv-tz').trim(), ntp_dhcp: false, ntp: val('dv-ntp').split(/[,\s]+/).filter(Boolean)});
  if ($('dv-time-save')) $('dv-time-save').onclick = () => busy($('dv-time-save'), async () => { await api(url('time'), {method: 'PUT', json: timeBody()}); toast('Time settings saved on the camera.'); });
  if ($('dv-time-sync')) $('dv-time-sync').onclick = () => busy($('dv-time-sync'), async () => { await api(url('time'), {method: 'PUT', json: {...timeBody(), sync_clock: true}}); toast("The camera's clock now matches this computer."); loadDeviceTab(); });
  box.querySelectorAll('.dv-stream').forEach(el => {
    const v = r.video[+el.dataset.k];
    el.querySelector('.dv-video-save').onclick = (e) => {
      const [w, hh] = el.querySelector('.dv-res').value.split('x').map(Number), enc2 = el.querySelector('.dv-enc');
      busy(e.target, async () => {
        await api(url('video'), {method: 'PUT', json: {token: v.token, width: w, height: hh, fps: +el.querySelector('.dv-fps').value, bitrate: Math.round(+el.querySelector('.dv-br').value),
          gov: Math.round(+el.querySelector('.dv-gov').value) || 0, encoding: enc2 ? enc2.value : ''}});
        toast(`Stream ${+el.dataset.k + 1} saved on the camera. The NVR reconnects to it automatically.`);
      });
    };
  });
  if ($('dv-img-save')) $('dv-img-save').onclick = () => busy($('dv-img-save'), async () => {
    const g = (k) => $('dv-' + k) ? +$('dv-' + k).value : null;
    await api(url('imaging'), {method: 'PUT', json: {brightness: g('brightness'), contrast: g('contrast'), saturation: g('saturation'), sharpness: g('sharpness'), ir_cut: $('dv-ir') ? $('dv-ir').value : null}});
    toast('Picture settings saved on the camera.');
  });
}

// ---------- recording profiles ----------
const PROFILE_KEYS = ['record_mode', 'schedule_enabled', 'schedule', 'record_audio', 'transcode', 'record_fps', 'segment_seconds', 'retention_days', 'max_storage_gb', 'detect'];
function setupProfiles() {
  const sel = $('c-prof'), msg = (t) => $('c-prof-msg').textContent = t;
  const list = () => (settings && settings.profiles) || [];
  const fill = (cur) => {
    sel.innerHTML = '<option value="">Choose a saved profile…</option>' + list().map((p, i) => `<option value="${i}">${esc(p.name)}</option>`).join('');
    if (cur != null) sel.value = String(cur);
  };
  fill();
  const save = async (profiles) => { await api('api/profiles', {method: 'PUT', json: profiles}); settings.profiles = profiles; };
  $('c-prof-apply').onclick = () => {
    const p = list()[+sel.value];
    if (sel.value === '' || !p) return msg('Choose a profile first.');
    applyProfile(p);
    msg(`Applied “${p.name}” — check the Recording and Motion & AI tabs, then Save camera.`);
  };
  sel.onchange = () => { if (sel.value !== '') $('c-prof-apply').click(); };
  $('c-prof-save').onclick = async () => {
    const cur = sel.value !== '' ? list()[+sel.value] : null;
    const name = (prompt('Profile name (an existing name is replaced):', cur ? cur.name : '') || '').trim();
    if (!name) return;
    const body = collectCamera(), p = {name};
    for (const k of PROFILE_KEYS) p[k] = JSON.parse(JSON.stringify(body[k]));
    p.detect.mask = '';
    const profiles = list().filter(x => x.name.toLowerCase() !== name.toLowerCase()).concat([p]);
    try { await save(profiles); fill(profiles.length - 1); msg(`Saved profile “${name}”. Choose it when editing any camera.`); } catch (e) { msg(e.message); }
  };
  $('c-prof-del').onclick = async () => {
    const p = list()[+sel.value];
    if (sel.value === '' || !p || !confirm(`Delete the profile “${p.name}”? Cameras keep their settings.`)) return;
    try { await save(list().filter(x => x !== p)); fill(); msg(`Deleted “${p.name}”.`); } catch (e) { msg(e.message); }
  };
}
// Put a profile's settings into the camera form (the camera's mask stays).
function applyProfile(p) {
  const set = (id, v) => { const el = $(id); if (!el) return; if (el.type === 'checkbox') el.checked = !!v; else el.value = String(v); };
  const r = document.querySelector(`input[name=c-mode][value="${p.record_mode}"]`); if (r) r.checked = true;
  set('c-sched-on', p.schedule_enabled);
  document.querySelectorAll('#c-days input').forEach(i => i.checked = p.schedule.days.includes(+i.value));
  set('c-start', p.schedule.start); set('c-end', p.schedule.end);
  set('c-audio', p.record_audio); set('c-transcode', p.transcode); set('c-fps', p.record_fps);
  set('c-seg', p.segment_seconds); if ($('c-seg').value === '') $('c-seg').value = '0';
  set('c-ret', p.retention_days); set('c-max', p.max_storage_gb);
  const d = p.detect;
  set('d-motion', d.motion); set('d-contrast', d.min_contrast); set('d-objpx', d.min_object_px); set('d-make', d.make_time_ms / 1000);
  set('d-travel', d.min_travel_px); set('d-ai', d.ai); set('d-conf', Math.round(d.min_confidence * 100));
  if ($('d-conf-v')) $('d-conf-v').textContent = Math.round(d.min_confidence * 100) + '%';
  document.querySelectorAll('#d-objs input').forEach(i => i.checked = d.objects.includes(i.value));
  set('d-trig', d.trigger); set('d-pre', d.pre_seconds); set('d-post', d.post_seconds); set('d-fps', d.fps); set('d-int', d.ai_interval);
  // Let the form update anything that depends on these (hidden sections, hints).
  for (const el of $('f-cam').querySelectorAll('[data-tab=recording] input, [data-tab=recording] select, [data-tab=detection] input, [data-tab=detection] select')) el.dispatchEvent(new Event('change', {bubbles: true}));
  $('d-objpx') && $('d-objpx').dispatchEvent(new Event('input', {bubbles: true}));
}
async function finishSave(body) {
  const err = (m) => { $('c-err').hidden = false; $('c-err').textContent = m; };
  const btn = $('c-save'); btn.disabled = true; btn.textContent = 'Saving…';
  try {
    const move = $('c-move-row') && !$('c-move-row').hidden && chk('c-move');
    if (move) btn.textContent = 'Moving recordings…';
    if (editingId) await api(`api/cameras/${enc(editingId)}?move_recordings=${!!move}`, {method: 'PUT', json: body});
    else await api('api/cameras', {method: 'POST', json: body});
    closeCameraDialog();
    toast(editingId ? 'Camera updated' : cloneMode ? 'Camera cloned' : 'Camera added');
    await loadSettings(); await refreshStatus();
    if (page === 'settings') showSettings(section); else route();
  } catch (e) { err(e.message); }
  finally { btn.disabled = false; btn.textContent = 'Save camera'; }
}

// ---------- folder picker ----------
// Browse folders on the NVR computer (the browser may be on another device).
function pickFolder(initial, title) {
  return new Promise((resolve) => {
    const f = $('f-fs'), dlg = $('fs-dialog');
    let cur = null;
    const done = (v) => { dlg.close(); resolve(v); };
    const load = async (path) => {
      let l;
      try { l = await api('api/fs?path=' + enc(path || '')); }
      catch (e) { toast(e.message, true); if (path) return load(''); return; }
      cur = l;
      const space = (p) => p.free_bytes != null ? `<span class="muted">${fmtBytes(p.free_bytes)} free of ${fmtBytes(p.total_bytes)}</span>` : '';
      f.innerHTML = `<div class="head"><h2>${esc(title || 'Choose a folder')}</h2><button type="button" class="small" data-close>✕</button></div>
        <div class="body">
          <div class="row" style="margin-bottom:8px"><input type="text" id="fs-path" value="${esc(l.path)}" placeholder="Type a path, e.g. D:\\NVR, \\\\NAS\\cameras or /mnt/nas" style="flex:1"><button type="button" id="fs-go">Go</button></div>
          <div class="row" style="margin-bottom:10px">${l.places.map(p => `<button type="button" class="small" data-path="${esc(p.path)}" title="${esc(p.free_bytes != null ? fmtBytes(p.free_bytes) + ' free' : '')}">💾 ${esc(p.name)}</button>`).join('')}</div>
          ${l.path ? `<div class="row" style="margin-bottom:6px"><b style="flex:1;word-break:break-all">${esc(l.path)}</b>${space(l)}</div>
            <div class="fs-list">${l.parent ? `<button type="button" data-path="${esc(l.parent)}">⬑ Up one level</button>` : ''}
              ${l.folders.map(d => `<button type="button" data-path="${esc(d.path)}">📁 ${esc(d.name)}</button>`).join('') || '<div class="muted" style="padding:8px">No folders here.</div>'}</div>
            <div class="row" style="margin-top:8px"><input type="text" id="fs-new" placeholder="New folder name" style="flex:1"><button type="button" id="fs-mk">Create folder</button></div>
            ${l.writable ? '' : '<div class="err" style="margin-top:8px">The NVR cannot write to this folder. Choose another or change its permissions.</div>'}`
          : '<div class="muted">Choose a drive above, or type a path.</div>'}
          <div class="hint" style="margin-top:8px">Network drives: type the share path (e.g. <code>\\\\NAS\\cameras</code> on Windows, or a mounted folder like <code>/mnt/nas</code>). Cloud drives: pick the synced folder, e.g. Google Drive for desktop's <code>G:\\My Drive</code>.</div>
        </div>
        <div class="foot"><button type="button" data-close>Cancel</button><button type="button" class="primary" id="fs-ok" ${l.path && l.writable ? '' : 'disabled'}>Use this folder</button></div>`;
      f.querySelectorAll('[data-close]').forEach(b => b.onclick = () => done(null));
      f.querySelectorAll('[data-path]').forEach(b => b.onclick = () => load(b.dataset.path));
      $('fs-go').onclick = () => load(val('fs-path'));
      $('fs-path').onkeydown = (e) => { if (e.key === 'Enter') { e.preventDefault(); load(val('fs-path')); } };
      if ($('fs-mk')) $('fs-mk').onclick = async () => {
        try { const r = await api('api/fs/mkdir', {method: 'POST', json: {parent: cur.path, name: val('fs-new')}}); load(r.path); }
        catch (e) { toast(e.message, true); }
      };
      $('fs-ok').onclick = () => done(cur.path);
    };
    f.onsubmit = (e) => e.preventDefault();
    load(initial).then(() => dlg.showModal());
  });
}

// ---------- storage ----------
async function renderStorage(body) {
  const s = settings.storage;
  const segs = [[300, '5 minutes'], [600, '10 minutes'], [900, '15 minutes'], [1800, '30 minutes'], [3600, '1 hour'], [7200, '2 hours'], [21600, '6 hours'], [43200, '12 hours'], [86400, 'One long file per day']];
  if (!segs.some(x => x[0] === s.segment_seconds)) segs.unshift([s.segment_seconds, fmtDur(s.segment_seconds)]);
  const usage = await api('api/usage').catch(() => null);
  const used = (id) => usage && usage.cameras.find(u => u.camera === id);
  body.innerHTML = `<form class="card" id="f-storage"><h2>Recording &amp; storage</h2><div class="form">
    <label for="s-dir">Default recordings folder</label><div><div class="row"><input type="text" id="s-dir" required value="${esc(s.dir)}" style="flex:1"><button type="button" id="s-browse">Browse…</button></div>
      <div class="hint">Cameras save into a sub-folder here, unless they have their own folder (below).</div>
      <label class="check" id="s-move-row" hidden><input type="checkbox" id="s-move" checked> Move existing recordings to the new folder</label></div>
    <label for="s-seg">Default file length</label><div><select id="s-seg">${segs.map(([v, t]) => `<option value="${v}" ${v === s.segment_seconds ? 'selected' : ''}>${t}</option>`).join('')}</select>
      <div class="hint">Recordings are split into files of this length. Cameras can override it.</div></div>
    <label for="s-ret">Keep recordings for</label><div class="row"><input type="number" id="s-ret" min="0" value="${s.retention_days}" style="width:110px"><span class="muted">days (0 = forever)</span></div>
    <label for="s-max">Maximum storage</label><div class="row"><input type="number" id="s-max" min="0" step="0.5" value="${s.max_storage_gb}" style="width:110px"><span class="muted">GB in total (0 = use the whole disk)</span></div>
    <label for="s-free">Keep free on disk</label><div class="row"><input type="number" id="s-free" min="0" step="0.5" value="${s.min_free_gb}" style="width:110px"><span class="muted">GB (oldest recordings are deleted to keep this free)</span></div>
    <span></span><div><label class="check"><input type="checkbox" id="s-tl" ${s.prebuild_timelapse !== false ? 'checked' : ''}> Prepare time-lapse previews for the last 24 hours automatically</label>
      <div class="hint">The timeline's scrub previews are made in the background (newest first, a little CPU), so they appear instantly. Older days are still made when you open them.</div></div>
  </div><div class="actions"><button class="primary">Save</button><a class="btn" href="#system">See space &amp; days of recording →</a></div></form>
  <div class="card"><h2>Where each camera saves</h2>
    <table><thead><tr><th>Camera</th><th>Folder</th><th class="hide-sm">Stored</th><th></th></tr></thead><tbody>
    ${cameraCfgs.map(c => `<tr><td><b>${esc(c.name)}</b></td><td><code>${esc(used(c.id) ? used(c.id).dir : (c.storage_dir || s.dir) + '/' + c.id)}</code>
      <div class="sub">${c.storage_dir ? 'Own folder' : 'Default folder'}</div></td><td class="hide-sm">${used(c.id) ? fmtBytes(used(c.id).used_bytes) : '–'}</td>
      <td style="text-align:right"><div class="row" style="justify-content:flex-end"><button type="button" class="small" data-cam="${esc(c.id)}">Change…</button>${c.storage_dir ? `<button type="button" class="small" data-reset="${esc(c.id)}">Use default</button>` : ''}</div></td></tr>`).join('')}
    </tbody></table>
    <div class="hint">Changing a folder can move the camera's existing recordings too. Moving to another drive copies the files, which can take a while; the camera pauses recording during the move.</div></div>`;
  $('s-dir').oninput = () => $('s-move-row').hidden = val('s-dir').trim() === s.dir;
  $('s-browse').onclick = async () => { const p = await pickFolder(val('s-dir'), 'Default recordings folder'); if (p) { $('s-dir').value = p; $('s-dir').oninput(); } };
  $('f-storage').onsubmit = async (e) => {
    e.preventDefault();
    const moving = !$('s-move-row').hidden && chk('s-move');
    if (moving) toast('Saving and moving recordings… this can take a while for large folders.');
    await saveSettings({move_recordings: moving, storage: {dir: val('s-dir').trim(), segment_seconds: num('s-seg'), retention_days: Math.round(num('s-ret')), max_storage_gb: num('s-max'), min_free_gb: num('s-free'), prebuild_timelapse: chk('s-tl')}});
    renderStorage(body);
  };
  const changeCam = async (id, dir) => {
    const c = cameraCfgs.find(x => x.id === id);
    const move = confirm(`Move ${c.name}'s existing recordings to the new folder too?\n\nOK = move them\nCancel = leave them where they are (they will no longer show in the NVR)`);
    toast(move ? 'Moving recordings…' : 'Saving…');
    try {
      await api(`api/cameras/${enc(id)}?move_recordings=${move}`, {method: 'PUT', json: {...c, storage_dir: dir}});
      toast('Saved'); await loadSettings(); renderStorage(body);
    } catch (err) { toast(err.message, true); }
  };
  body.querySelectorAll('[data-cam]').forEach(b => b.onclick = async () => {
    const c = cameraCfgs.find(x => x.id === b.dataset.cam);
    const p = await pickFolder(c.storage_dir || s.dir, `Folder for ${c.name}`);
    if (p) changeCam(c.id, p);
  });
  body.querySelectorAll('[data-reset]').forEach(b => b.onclick = () => changeCam(b.dataset.reset, ''));
}

// ---------- display ----------
const CLOCK_FORMATS = ['DD/MM/YYYY HH:mm:ss', 'DD/MM/YYYY h:mm:ss a', 'MM/DD/YYYY h:mm:ss A', 'YYYY-MM-DD HH:mm:ss', 'ddd D MMM YYYY HH:mm:ss', 'dddd D MMMM YYYY h:mm a', 'HH:mm:ss'];
function renderDisplay(body) {
  const l = settings.live, d = settings.display;
  const corners = [['tl', 'Top left'], ['tr', 'Top right'], ['bl', 'Bottom left'], ['br', 'Bottom right']];
  const custom = !CLOCK_FORMATS.includes(d.clock_format);
  body.innerHTML = `<form class="card" id="f-display"><h2>Live view</h2><div class="form">
    <label for="l-fps">Grid frame rate</label><div class="row"><input type="number" id="l-fps" min="1" max="30" value="${l.fps}" style="width:110px"><span class="muted">fps for the camera grid (sub-streams)</span></div>
    <label for="l-width">Grid picture width</label><div><select id="l-width">${[[480, '480 (low)'], [640, '640'], [960, '960'], [1280, '1280 (HD)'], [1920, '1920'], [0, 'Native']].map(([v, t]) => `<option value="${v}" ${v === l.width ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    <label for="l-hd">Full-screen frame rate</label><div class="row"><input type="number" id="l-hd" min="1" max="30" value="${l.hd_fps}" style="width:110px"><span class="muted">fps when a camera is opened (main stream)</span></div>
    <label for="l-q">Picture quality</label><div><select id="l-q">${[[3, 'Very high'], [6, 'High'], [10, 'Medium'], [16, 'Low (saves bandwidth)']].map(([v, t]) => `<option value="${v}" ${v === l.quality ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    <span></span><div><label class="check"><input type="checkbox" id="l-pre" ${l.preload_hd_streams ? 'checked' : ''}> Keep HD streams ready in the background</label>
      <div class="hint">Off (recommended): a camera's full-quality stream is only decoded while you have it open; until it arrives you see the sub-stream picture. On: opening a camera shows HD instantly, but every camera's HD stream is decoded all the time — a lot of GPU/CPU.</div></div>
    <h3>Overlays on live video</h3>
    <label>Fullscreen button</label><div class="chips" id="g-fs">${corners.map(([k, t]) => `<label><input type="checkbox" value="${k}" ${d.fullscreen_corners.includes(k) ? 'checked' : ''}>${t}</label>`).join('')}</div>
    <span></span><label class="check"><input type="checkbox" id="g-clock" ${d.show_clock ? 'checked' : ''}> Show date and time</label>
    <label for="g-corner">Date &amp; time position</label><div><select id="g-corner">${corners.map(([k, t]) => `<option value="${k}" ${k === d.clock_corner ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    <label for="g-fmt">Date &amp; time format</label><div><select id="g-fmt">${CLOCK_FORMATS.map(f => `<option ${f === d.clock_format ? 'selected' : ''}>${f}</option>`).join('')}<option value="custom" ${custom ? 'selected' : ''}>Custom…</option></select>
      <input type="text" id="g-custom" value="${esc(d.clock_format)}" style="margin-top:6px" ${custom ? '' : 'hidden'}>
      <div class="hint">Preview: <b id="g-prev"></b><br>Codes: YYYY YY · MMMM MMM MM M · DD D · dddd ddd · HH H (24 h) hh h (12 h) · mm ss · A a (AM/PM)</div></div>
    <label for="g-tilefit">Video in dashboard tiles</label><div><select id="g-tilefit">
      <option value="stretch" ${d.tile_fit === 'stretch' ? 'selected' : ''}>Stretch to fill the box</option>
      <option value="crop" ${d.tile_fit === 'crop' ? 'selected' : ''}>Zoom to fill (trims the edges)</option>
      <option value="fit" ${d.tile_fit === 'fit' ? 'selected' : ''}>Show the whole picture (black bars)</option></select>
      <div class="hint">For cameras whose picture shape differs from the tile (e.g. 4:3 sub-streams).</div></div>
    <span></span><label class="check"><input type="checkbox" id="g-name" ${d.show_name ? 'checked' : ''}> Show camera name</label>
    <span></span><label class="check"><input type="checkbox" id="g-fps" ${d.show_fps ? 'checked' : ''}> Show frames per second and stream details</label>
  </div><div class="actions"><button class="primary">Save</button></div></form>`;
  const fmt = () => val('g-fmt') === 'custom' ? val('g-custom') : val('g-fmt');
  const prev = () => $('g-prev').textContent = fmtDate(new Date(), fmt());
  $('g-fmt').onchange = () => { $('g-custom').hidden = val('g-fmt') !== 'custom'; prev(); };
  $('g-custom').oninput = prev; prev();
  $('f-display').onsubmit = (e) => {
    e.preventDefault();
    saveSettings({
      live: {fps: num('l-fps'), width: num('l-width'), quality: num('l-q'), hd_fps: num('l-hd'), preload_hd_streams: chk('l-pre')},
      display: {fullscreen_corners: [...document.querySelectorAll('#g-fs input:checked')].map(i => i.value), show_clock: chk('g-clock'),
        clock_corner: val('g-corner'), clock_format: fmt() || 'DD/MM/YYYY HH:mm:ss', show_name: chk('g-name'), show_fps: chk('g-fps'), tile_fit: val('g-tilefit')},
    }, 'Saved').then(() => renderDash());
  };
}

// ---------- AI ----------
const PROVIDERS = [
  ['none', 'Off', 'No object recognition (motion detection still works).'],
  ['builtin', 'Built-in (this computer)', 'Free and private. Runs a YOLOX model on this computer\'s CPU. The model (4 MB for the fastest, up to 220 MB for the most accurate) downloads automatically the first time.'],
  ['deepstack', 'AI server on your network', 'CodeProject.AI Server or DeepStack on this or another computer — can use that computer\'s GPU. Free.'],
  ['ollama', 'Ollama (local vision model)', 'A vision model such as qwen2.5vl or llava running in Ollama on your network. Free, slower.'],
  ['gemini', 'Google Gemini (cloud)', 'Free tier with an API key from aistudio.google.com. Pictures are sent to Google.'],
  ['openai', 'OpenAI-compatible (cloud or local)', 'OpenAI, OpenRouter, Groq, LM Studio, vLLM… anything with the OpenAI chat API and vision.'],
];
function renderAi(body) {
  const a = settings.ai, st = status && status.ai;
  body.innerHTML = `<form class="card" id="f-ai"><h2>Object recognition (AI)</h2>
    <p class="muted" style="margin-top:0">When a camera sees motion, a picture is checked for people, vehicles and animals. Turn it on per camera under <i>Motion &amp; AI</i>.</p>
    <div class="radio-cards">${PROVIDERS.map(([k, t, dsc]) => `<label><input type="radio" name="ai-p" value="${k}" ${a.provider === k ? 'checked' : ''}><b>${t}</b><small>${dsc}</small></label>`).join('')}</div>
    <div class="form" style="margin-top:14px">
      <label class="p-builtin" for="ai-model">Model</label><div class="p-builtin"><select id="ai-model">
        <option value="yolox_nano" ${a.builtin_model === 'yolox_nano' ? 'selected' : ''}>Nano — fastest (≈30 ms), good for small PCs</option>
        <option value="yolox_tiny" ${a.builtin_model === 'yolox_tiny' ? 'selected' : ''}>Tiny — balanced (≈90 ms), recommended</option>
        <option value="yolox_s" ${a.builtin_model === 'yolox_s' ? 'selected' : ''}>Small — more accurate (≈350 ms)</option>
        <option value="yolox_m" ${a.builtin_model === 'yolox_m' ? 'selected' : ''}>Medium — for bigger PCs (≈0.9 s, 100 MB)</option>
        <option value="yolox_l" ${a.builtin_model === 'yolox_l' ? 'selected' : ''}>Large — most accurate, powerful PCs only (≈2 s, 220 MB)</option></select>
        <div class="hint">Times are per picture on a 4-core PC; the AI only runs while there is motion.</div></div>
      <label class="p-builtin" for="ai-verify">Double-check alerts</label><div class="p-builtin"><select id="ai-verify">
        <option value="">Off</option>
        ${[['yolox_s', 'Small model (≈350 ms)'], ['yolox_m', 'Medium model (≈0.9 s) — recommended'], ['yolox_l', 'Large model (≈2 s)']].map(([k, t]) => `<option value="${k}" ${a.verify_model === k ? 'selected' : ''}>${t}</option>`).join('')}</select>
        <div class="hint">When the model above thinks it sees something that would raise an alert, a bigger model looks at the same picture and has the final say: fewer false alerts and fewer mix-ups (a cat called a person). It only runs on those few pictures, so it suits normal PCs too.</div></div>
      <span class="p-builtin"></span><div class="p-builtin"><label class="check"><input type="checkbox" id="ai-night" ${a.night_boost !== false ? 'checked' : ''}> Night boost</label>
        <div class="hint">Dark and infrared (black &amp; white) pictures are brightened and their contrast stretched before the AI looks at them, so people and animals are recognized better at night.</div></div>
      <span class="p-any"></span><div class="p-any"><label class="check"><input type="checkbox" id="ai-smart" ${a.smart_search ? 'checked' : ''}> Smart search</label>
        <div class="hint">Lets you search alerts by description — “kid wearing a red backpack”, “white van”, “man with a ladder”. Each alert picture (and each person, vehicle or animal in it) is described by a CLIP model on this computer, in the background (≈0.1 s per alert, 600 MB download once, loaded only while in use). Works with any AI choice above.</div>
        <div class="hint" id="ai-smart-st"></div></div>
      <label class="p-url" for="ai-url">Server address</label><div class="p-url"><input type="text" id="ai-url" value="${esc(a.url)}"><div class="hint" id="ai-url-hint"></div></div>
      <label class="p-key" for="ai-key">API key</label><div class="p-key"><input type="password" id="ai-key" autocomplete="new-password" placeholder="${a.has_key ? 'Saved – leave empty to keep' : ''}"><div class="hint" id="ai-key-hint"></div></div>
      <label class="p-model" for="ai-mname">Model name</label><div class="p-model"><div class="row"><input type="text" id="ai-mname" value="${esc(a.model)}" list="ai-models" style="flex:1"><button type="button" id="ai-list">List models</button></div>
        <datalist id="ai-models"></datalist><div class="hint" id="ai-model-hint"></div><div class="hint" id="ai-list-out"></div></div>
      <label class="p-any" for="ai-rate">Limit</label><div class="p-any row"><input type="number" id="ai-rate" min="0" value="${a.max_per_minute}" style="width:110px"><span class="muted">requests per minute, all cameras together (0 = no limit). Keeps free cloud tiers within quota.</span></div>
      <label class="p-any" for="ai-timeout">Timeout</label><div class="p-any row"><input type="number" id="ai-timeout" min="1" max="300" value="${a.timeout_seconds}" style="width:110px"><span class="muted">seconds</span></div>
    </div>
    <div class="hint" id="ai-status" style="margin-top:10px"></div>
    <div class="actions"><button class="primary">Save</button>
      <select id="ai-cam">${(status ? status.cameras : []).map(c => `<option value="${esc(c.id)}">${esc(c.name)}</option>`).join('')}</select>
      <button type="button" id="ai-test">Test on this camera</button></div>
    <div id="ai-result" style="margin-top:12px"></div></form>`;
  const hints = {
    deepstack: ['http://192.168.1.20:32168', 'CodeProject.AI uses port 32168, DeepStack usually 80 or 5000.', '', ''],
    ollama: ['http://192.168.1.20:11434', 'Ollama listens on port 11434. Start it with OLLAMA_HOST=0.0.0.0 to allow other computers.', 'qwen2.5vl', 'Run “ollama pull qwen2.5vl” (or llava, llama3.2-vision) on that computer first.'],
    gemini: ['', '', 'automatic (newest Flash model)', 'Leave empty to use the newest Flash model automatically (it switches by itself when Google retires one).'],
    openai: ['https://api.openai.com/v1', 'Base URL ending in /v1, e.g. https://openrouter.ai/api/v1 or http://192.168.1.20:1234/v1 (LM Studio).', 'gpt-4o-mini', 'A model that accepts images.'],
  };
  const sync = () => {
    const p = document.querySelector('input[name=ai-p]:checked').value, h = hints[p] || ['', '', '', ''];
    const show = (cls, on) => document.querySelectorAll('.' + cls).forEach(e => e.hidden = !on);
    show('p-builtin', p === 'builtin'); show('p-url', ['deepstack', 'ollama', 'openai'].includes(p));
    show('p-key', ['gemini', 'openai', 'deepstack'].includes(p)); show('p-model', ['ollama', 'gemini', 'openai'].includes(p));
    show('p-any', p !== 'none');
    $('ai-url').placeholder = h[0]; $('ai-url-hint').textContent = h[1]; $('ai-mname').placeholder = h[2]; $('ai-model-hint').textContent = h[3];
    $('ai-key-hint').innerHTML = p === 'gemini' ? 'Get a free key at <a href="https://aistudio.google.com/apikey" target="_blank" rel="noopener">aistudio.google.com/apikey</a>.' : p === 'deepstack' ? 'Only if your server requires one.' : '';
    if (p === 'gemini' && !num('ai-rate')) $('ai-rate').value = 10;
    $('ai-test').disabled = p === 'none' || !$('ai-cam').value;
  };
  document.querySelectorAll('input[name=ai-p]').forEach(r => r.onchange = sync); sync();
  const form = () => ({provider: document.querySelector('input[name=ai-p]:checked').value, builtin_model: val('ai-model'), url: val('ai-url').trim(),
    api_key: val('ai-key'), model: val('ai-mname').trim(), max_per_minute: Math.round(num('ai-rate')), timeout_seconds: Math.round(num('ai-timeout')) || 20,
    verify_model: val('ai-verify'), night_boost: chk('ai-night'), smart_search: chk('ai-smart')});
  const showStatus = () => {
    const ss = status && status.smart_search;
    if (ss && $('ai-smart-st')) $('ai-smart-st').innerHTML = !a.smart_search ? '' : ss.downloading ? `Downloading the smart-search model… ${Math.round(ss.download_percent || 0)}%`
      : ss.error ? `<span class="err">${esc(ss.error)}</span>` : `<span class="ok">On</span> — ${ss.indexed} alert${ss.indexed === 1 ? '' : 's'} described so far (newest first, last 60 days).`;
    const s = status && status.ai; if (!s) return;
    $('ai-status').innerHTML = s.provider === 'none' ? '' : `Current: <b>${esc(s.provider)}</b> — ${s.downloading ? `downloading model ${Math.round(s.download_percent || 0)}%` : s.ready ? '<span class="ok">ready</span>' : 'loading'}` +
      ` · ${s.requests} checks${s.failures ? `, ${s.failures} failed` : ''}${s.last_ms != null ? ` · last took ${s.last_ms} ms` : ''}${s.error ? `<br><span class="err">${esc(s.error)}</span>` : ''}`;
  };
  showStatus();
  $('ai-list').onclick = async () => {
    const out = $('ai-list-out');
    out.textContent = 'Asking the provider…';
    try {
      const r = await api('api/ai/models', {method: 'POST', json: form()});
      $('ai-models').innerHTML = r.models.map(m => `<option value="${esc(m)}">`).join('');
      out.innerHTML = r.models.length ? `${r.models.length} models available${r.suggested ? ` — automatic choice: <b>${esc(r.suggested)}</b>` : ''}. Click the model box to pick one.` : 'No models found.';
    } catch (e) { out.innerHTML = `<span class="err">${esc(e.message)}</span>`; }
  };
  $('f-ai').onsubmit = async (e) => { e.preventDefault(); await saveSettings({ai: form()}, 'AI settings saved'); showStatus(); };
  $('ai-test').onclick = async () => {
    const btn = $('ai-test'), out = $('ai-result');
    btn.disabled = true; btn.textContent = 'Checking…'; out.innerHTML = '<span class="muted">Sending the camera\'s current picture to the AI… (the built-in model may need to download first)</span>';
    try {
      const r = await api('api/ai/test', {method: 'POST', json: {camera: val('ai-cam'), ai: form()}});
      const boxes = (r.detections || []).map(d => `<div class="box" style="left:${d.x * 100}%;top:${d.y * 100}%;width:${d.w * 100}%;height:${d.h * 100}%"><span>${esc(d.label)} ${Math.round(d.score * 100)}%</span></div>`).join('');
      out.innerHTML = (r.ok ? `<div class="ok" style="margin-bottom:6px">✓ ${r.detections.length ? r.detections.length + ' object(s) found' : 'Working — nothing recognized in this picture'} (${r.ms} ms)</div>`
        : `<div class="err" style="margin-bottom:6px">✗ ${esc(r.error)}</div>`) + (r.image ? `<div class="ai-test"><img src="${r.image}" alt="">${boxes}</div>` : '');
    } catch (e) { out.innerHTML = `<div class="err">${esc(e.message)}</div>`; }
    finally { btn.disabled = false; btn.textContent = 'Test on this camera'; }
  };
}

// ---------- GPU ----------
const DEC_LABELS = {none: 'Off (CPU)', auto: 'Automatic (use any GPU found)', cuda: 'NVIDIA (CUDA/NVDEC)', qsv: 'Intel Quick Sync (QSV)',
  vaapi: 'VAAPI – Intel/AMD on Linux', videotoolbox: 'Apple VideoToolbox (macOS)', d3d11va: 'Direct3D 11 (Windows, any GPU)', dxva2: 'DXVA2 (older Windows)'};
const ENC_LABELS = {auto: 'Automatic', libx264: 'CPU (libx264)', h264_nvenc: 'NVIDIA NVENC', h264_qsv: 'Intel Quick Sync',
  h264_vaapi: 'VAAPI (Linux Intel/AMD)', h264_videotoolbox: 'Apple VideoToolbox', h264_amf: 'AMD AMF (Windows)', h264_mf: 'Windows Media Foundation'};
async function renderHardware(body) {
  if (!sysinfo) sysinfo = await api('api/system').catch(() => null);
  const caps = sysinfo ? sysinfo.capabilities : {hwaccels: [], encoders: []}, h = settings.hardware;
  body.innerHTML = `<form class="card" id="f-hw"><h2>GPU acceleration</h2>
    <p class="muted" style="margin-top:0">The GPU decodes video for the live view and motion detection, and encodes when a camera converts or lowers the frame rate of recordings. Normal recording copies the camera's video and needs no GPU. If the GPU fails, the NVR switches to the CPU automatically.</p>
    <div class="form">
      <label for="h-dec">Video decoding</label><div><select id="h-dec">${Object.entries(DEC_LABELS).map(([k, v]) => { const ok = k === 'none' || k === 'auto' || caps.hwaccels.includes(k); return `<option value="${k}" ${ok ? '' : 'disabled'}>${v}${ok ? '' : ' – not in this ffmpeg'}</option>`; }).join('')}</select>
        <div class="hint">${caps.hwaccels.length ? 'Your ffmpeg supports: ' + esc(caps.hwaccels.join(', ')) + '. Use Test to check the hardware is really there.' : 'Your ffmpeg reports no GPU support.'}</div></div>
      <label for="h-enc">H.264 encoder</label><div><select id="h-enc">${Object.entries(ENC_LABELS).map(([k, v]) => { const ok = k === 'auto' || caps.encoders.includes(k); return `<option value="${k}" ${ok ? '' : 'disabled'}>${v}${ok ? '' : ' – not in this ffmpeg'}</option>`; }).join('')}</select>
        <div class="hint">“Automatic” picks the encoder that matches the decoder.</div></div>
      <span></span><div><label class="check"><input type="checkbox" id="h-sub" ${h.gpu_substreams ? 'checked' : ''}> Also use the GPU for sub-streams</label>
        <div class="hint">Off (recommended): the GPU only decodes full-quality streams you are watching. Small sub-streams (grid, motion detection) are cheaper on the CPU than on the GPU plus copying the pictures back.</div></div>
      <label for="h-dev" class="vaapi">GPU device</label><div class="vaapi"><input type="text" id="h-dev" value="${esc(h.device)}" placeholder="/dev/dri/renderD128"><div class="hint">VAAPI only. Empty = default.</div></div>
    </div>
    <div class="actions"><button class="primary">Save</button><button type="button" id="h-test">Test selected options</button></div>
    <div id="h-result" style="margin-top:12px"></div></form>`;
  $('h-dec').value = h.hwaccel; $('h-enc').value = h.encoder;
  const sync = () => document.querySelectorAll('.vaapi').forEach(e => e.hidden = !(val('h-dec') === 'vaapi' || val('h-enc') === 'h264_vaapi'));
  $('h-dec').onchange = $('h-enc').onchange = sync; sync();
  const form = () => ({hwaccel: val('h-dec'), encoder: val('h-enc'), device: val('h-dev').trim(), gpu_substreams: chk('h-sub')});
  $('f-hw').onsubmit = (e) => { e.preventDefault(); saveSettings({hardware: form()}, 'Saved. Cameras are restarting with the new settings.'); };
  $('h-test').onclick = async () => {
    const btn = $('h-test'), out = $('h-result');
    btn.disabled = true; btn.textContent = 'Testing…'; out.innerHTML = '<span class="muted">Running a short test clip through the GPU…</span>';
    try {
      const r = await api('api/hw-test', {method: 'POST', json: form()});
      const line = (l, t) => `<div><b>${l}:</b> <span class="${t.ok ? 'ok' : 'err'}">${t.ok ? '✓' : '✗'} ${esc(t.message)}</span></div>`;
      out.innerHTML = line('Decoding', r.decoder) + line('Encoding', r.encoder);
    } catch (e) { out.innerHTML = `<div class="err">${esc(e.message)}</div>`; }
    finally { btn.disabled = false; btn.textContent = 'Test selected options'; }
  };
}

// ---------- cloud ----------
function renderCloud(body) {
  const c = settings.cloud;
  body.innerHTML = `<form class="card" id="f-cloud"><h2>Cloud backup</h2>
    <p class="muted" style="margin-top:0">Copies recordings to Google Drive, OneDrive, Dropbox, Amazon S3, Backblaze B2 and 70+ other services using <a href="https://rclone.org" target="_blank" rel="noopener">rclone</a>. Files stay on this computer too.</p>
    <ol class="muted" style="margin-top:0;padding-left:20px"><li>Install rclone from rclone.org.</li><li>Run <code>rclone config</code> once and add your cloud account (e.g. name it <code>gdrive</code>).</li><li>Enter the destination below, e.g. <code>gdrive:NVR</code>, and click Test.</li></ol>
    <p class="hint">Simpler alternative: set a camera's storage folder to a synced folder such as Google Drive for desktop or OneDrive.</p>
    <div class="form">
      <span></span><label class="check"><input type="checkbox" id="cl-on" ${c.enabled ? 'checked' : ''}> Upload recordings to the cloud</label>
      <label for="cl-remote">Destination</label><div><input type="text" id="cl-remote" value="${esc(c.remote)}" placeholder="gdrive:NVR"></div>
      <label for="cl-what">What to upload</label><div><select id="cl-what">
        <option value="ai" ${c.upload === 'ai' ? 'selected' : ''}>Only verified AI alerts (a person, car… recognized by the AI)</option>
        <option value="events" ${c.upload === 'events' ? 'selected' : ''}>All alerts, including motion-only (clips and pictures)</option>
        <option value="all" ${c.upload === 'all' ? 'selected' : ''}>Everything, including 24/7 recordings (needs a fast upload)</option></select>
        <div class="hint">“Verified AI alerts” needs object recognition turned on for the cameras.</div></div>
      <label for="cl-path">rclone program</label><div><input type="text" id="cl-path" value="${esc(c.rclone_path)}"></div>
    </div>
    <div class="actions"><button class="primary">Save</button><button type="button" id="cl-test">Test</button></div><div id="cl-result" style="margin-top:10px"></div></form>`;
  const form = () => ({enabled: chk('cl-on'), remote: val('cl-remote').trim(), upload: val('cl-what'), rclone_path: val('cl-path').trim() || 'rclone'});
  $('f-cloud').onsubmit = (e) => { e.preventDefault(); saveSettings({cloud: form()}); };
  $('cl-test').onclick = async () => {
    $('cl-result').innerHTML = '<span class="muted">Testing…</span>';
    const r = await api('api/cloud/test', {method: 'POST', json: form()}).catch(e => ({ok: false, message: e.message}));
    $('cl-result').innerHTML = `<span class="${r.ok ? 'ok' : 'err'}">${r.ok ? '✓' : '✗'} ${esc(r.message)}</span>`;
  };
}

// ---------- backup ----------
function renderBackup(body) {
  body.innerHTML = `<div class="card"><h2>Export setup</h2>
    <p class="muted" style="margin-top:0">Save all cameras, settings and your dashboard layout to a file, to restore after reinstalling or to copy to another computer. Recordings are not included.</p>
    <label class="check"><input type="checkbox" id="bk-pw"> Include passwords and API keys (keep the file somewhere safe)</label>
    <div class="actions"><button class="primary" id="bk-export">⇩ Download setup file</button></div></div>
    <div class="card"><h2>Import setup</h2>
    <p class="muted" style="margin-top:0">Replaces all cameras and settings with the ones in the file. Cameras not in the file are removed (their recordings stay on disk).</p>
    <input type="file" id="bk-file" accept=".toml,text/plain">
    <label class="check"><input type="checkbox" id="bk-keep" checked> Keep this NVR's login and web address</label>
    <div class="actions"><button id="bk-import" class="danger">Import</button></div><div id="bk-result"></div></div>`;
  $('bk-export').onclick = () => location.href = 'api/config/export?passwords=' + chk('bk-pw');
  $('bk-import').onclick = async () => {
    const f = $('bk-file').files[0];
    if (!f) return toast('Choose a setup file first.', true);
    if (!confirm('Replace all cameras and settings with the imported file?')) return;
    try {
      const r = await api('api/config/import?keep_login=' + chk('bk-keep'), {method: 'POST', body: await f.text(), headers: {'Content-Type': 'text/plain'}});
      toast(`Imported ${r.cameras} camera(s).` + (r.restart_required ? ' Restart the NVR to use the new web address.' : ''));
      await refreshStatus(); location.hash = '#settings/cameras';
    } catch (e) { $('bk-result').innerHTML = `<div class="err" style="margin-top:8px">${esc(e.message)}</div>`; }
  };
}

// ---------- system & login ----------
async function renderSystemSettings(body) {
  const s = settings.server;
  if (!sysinfo) sysinfo = await api('api/system').catch(() => null);
  const addrs = (sysinfo && sysinfo.addresses) || [];
  const [bind, port] = (() => { const i = s.listen.lastIndexOf(':'); return [s.listen.slice(0, i), s.listen.slice(i + 1)]; })();
  body.innerHTML = `<form class="card" id="f-sys"><h2>System &amp; login</h2><div class="form">
    <label for="y-name">NVR name</label><div><input type="text" id="y-name" required value="${esc(s.name)}"></div>
    <label>Web address</label><div id="y-addr"></div>
    <label for="y-port">Port</label><div class="row"><input type="number" id="y-port" min="1" max="65535" value="${esc(port)}" style="width:110px">
      <select id="y-port-pick"><option value="">Common ports…</option><option value="80">80 – standard web port (no :port needed)</option><option value="8080">8080 – default</option><option value="8000">8000</option><option value="8888">8888</option><option value="5000">5000</option><option value="9000">9000</option></select></div>
    <label for="y-bind">Reachable from</label><div><select id="y-bind"><option value="0.0.0.0">Every device on my network (all network adapters)</option><option value="127.0.0.1">Only this computer</option>
      ${addrs.map(a => `<option value="${esc(a)}">Only via ${esc(a)}</option>`).join('')}</select><div class="hint">Changing the port or this setting takes effect after the NVR restarts.</div></div>
    <label for="y-user">Login username</label><div><input type="text" id="y-user" value="${esc(s.username)}" autocomplete="off"><div class="hint">Leave empty to turn off the login.</div></div>
    <label for="y-pass">Login password</label><div><input type="password" id="y-pass" autocomplete="new-password"><div class="hint">${s.has_password ? 'A password is set. Leave empty to keep it.' : 'Required when a username is set.'}</div></div>
    <label for="y-ffmpeg">ffmpeg program</label><div><input type="text" id="y-ffmpeg" required value="${esc(s.ffmpeg_path)}"><div class="hint">Name or full path of ffmpeg (ffprobe must be next to it).</div></div>
  </div><div class="actions"><button class="primary">Save</button></div></form>`;
  if (![...$('y-bind').options].some(o => o.value === bind)) $('y-bind').add(new Option('Only via ' + bind, bind));
  $('y-bind').value = bind;
  const showAddr = () => {
    const p = Math.round(num('y-port')) || 8080, b = val('y-bind');
    const hosts = b === '0.0.0.0' ? (addrs.length ? addrs : ['this-computer']) : [b === '127.0.0.1' ? 'localhost' : b];
    const urls = hosts.map(h => `http://${h}${p === 80 ? '' : ':' + p}`);
    $('y-addr').innerHTML = urls.map(u => `<a href="${esc(u)}" target="_blank" rel="noopener"><b>${esc(u)}</b></a>`).join('<br>') +
      `<div class="hint">${b === '127.0.0.1' ? 'Only this computer can open it.' : 'Open this address on your phone, tablet or another computer on the same network.'} Detected automatically from this computer's network connection.</div>`;
  };
  $('y-port').oninput = $('y-bind').onchange = showAddr;
  $('y-port-pick').onchange = (e) => { if (e.target.value) { $('y-port').value = e.target.value; showAddr(); } e.target.value = ''; };
  showAddr();
  $('f-sys').onsubmit = async (e) => {
    e.preventDefault();
    const user = val('y-user').trim();
    if (user && !val('y-pass') && !settings.server.has_password) return toast('Enter a password for the login.', true);
    const listen = `${val('y-bind')}:${Math.round(num('y-port'))}`;
    if (await saveSettings({server: {name: val('y-name'), listen, username: user, password: val('y-pass'), ffmpeg_path: val('y-ffmpeg')}})) { sysinfo = null; renderSystemSettings(body); }
  };
}

async function renderInfo(body) {
  sysinfo = await api('api/system').catch(() => null);
  if (!sysinfo) return;
  const c = sysinfo.capabilities;
  body.innerHTML = `<div class="card"><h2>About</h2><dl>
    <dt>NVR version</dt><dd>${esc(sysinfo.version)}</dd><dt>Operating system</dt><dd>${esc(sysinfo.os)} (${esc(sysinfo.arch)})</dd>
    <dt>Uptime</dt><dd>${fmtDur(sysinfo.uptime_seconds)}</dd><dt>Web address</dt><dd>${(sysinfo.addresses || []).map(a => esc(`http://${a}:${sysinfo.listen.split(':').pop()}`)).join('<br>') || esc(sysinfo.listen)}</dd>
    <dt>ffmpeg</dt><dd>${esc(c.ffmpeg_version || 'NOT FOUND')}</dd>
    <dt>GPU decoders in ffmpeg</dt><dd>${esc(c.hwaccels.join(', ') || 'none')}</dd><dt>H.264 encoders</dt><dd>${esc(c.encoders.join(', ') || 'none')}</dd>
    <dt>Config file</dt><dd>${esc(sysinfo.config_path)}</dd><dt>Data folder</dt><dd>${esc(sysinfo.data_dir)}</dd>
    <dt>Log file</dt><dd>${esc(sysinfo.log_file || '–')}</dd><dt>Recordings folder</dt><dd>${esc(sysinfo.storage_dir)}</dd></dl></div>`;
}

// ======================================================================
// Start
// ======================================================================
Live.connect();
setInterval(refreshStatus, 2000);
refreshStatus().then(route);

$('ds-btn').onclick = (e) => {
  e.preventDefault();
  dsMode = {auto: 'on', on: 'off', off: 'auto'}[dsMode];
  try { localStorage.setItem('nvr-data-saver', JSON.stringify(dsMode)); } catch (err) {}
  applyDataSaver();
  toast(`Data saver: ${{auto: 'Auto (on away from home)', on: 'On', off: 'Off'}[dsMode]}. Live view changes now; recordings from the next one you open.`);
};
window.addEventListener('resize', applyDataSaver);
detectRemote();
