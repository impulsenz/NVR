'use strict';
// ======================================================================
// Core helpers
// ======================================================================
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const enc = encodeURIComponent;
// A little ⓘ next to a setting: hover (or tap) it for what the setting does,
// with an example when one helps. One card for the whole page.
const tip = (text) => `<span class="info" tabindex="0" role="img" aria-label="${esc(text)}" data-tip="${esc(text)}">i</span>`;
(() => {
  let card = null, at = null;
  const hide = () => { if (card) card.hidden = true; at = null; };
  const show = (el) => {
    if (!card) { card = document.createElement('div'); card.id = 'tipcard'; card.setAttribute('role', 'tooltip'); }
    // Inside the open window (or full-screen picture) the icon is in: those
    // are drawn above the page, and would hide a card on the page itself.
    const home = el.closest('dialog[open]') || (document.fullscreenElement && document.fullscreenElement.contains(el) ? document.fullscreenElement : null) || document.body;
    if (card.parentElement !== home) home.appendChild(card);
    at = el; card.textContent = el.dataset.tip; card.hidden = false;
    const r = el.getBoundingClientRect(), w = Math.min(320, innerWidth - 24);
    card.style.maxWidth = w + 'px';
    const cw = card.offsetWidth, ch = card.offsetHeight;
    card.style.left = Math.max(12, Math.min(r.left + r.width / 2 - cw / 2, innerWidth - cw - 12)) + 'px';
    card.style.top = (r.top - ch - 8 > 8 ? r.top - ch - 8 : r.bottom + 8) + 'px';
  };
  document.addEventListener('mouseover', (e) => { const el = e.target.closest && e.target.closest('.info'); if (el) show(el); else if (at) hide(); });
  document.addEventListener('focusin', (e) => { if (e.target.classList && e.target.classList.contains('info')) show(e.target); });
  document.addEventListener('focusout', (e) => { if (e.target === at) hide(); });
  // Touch: a tap shows it (and keeps the click from ticking a box it's in).
  document.addEventListener('click', (e) => { const el = e.target.closest && e.target.closest('.info'); if (el) { e.preventDefault(); e.stopPropagation(); show(el); } else hide(); }, true);
  addEventListener('scroll', hide, true);
})();
const DAYS = ['Mon','Tue','Wed','Thu','Fri','Sat','Sun'];
// Car, truck, bus and motorcycle are all one "vehicle" (as GAZE now names them).
const VEHICLE_KINDS = ['car', 'truck', 'bus', 'motorcycle', 'van'];
const canonKind = (k) => VEHICLE_KINDS.includes(k) ? 'vehicle' : k;
const canonKinds = (list) => [...new Set((list || []).map(canonKind))];
const LABEL_GROUP = {person:'person', vehicle:'vehicle', car:'vehicle', truck:'vehicle', bus:'vehicle', motorcycle:'vehicle', bicycle:'vehicle',
  dog:'animal', cat:'animal', bird:'animal', horse:'animal', cow:'animal', sheep:'animal', bear:'animal'};
// Alert types. Plain recordings aren't listed: they are on the timeline.
// The Alerts filter chips. Faces and number plates show alerts where one
// was found.
const FILTER_BASE = [['motion','Motion'],['person','Person'],['vehicle','Vehicle'],['dog','Dog'],['cat','Cat'],['package','Item left/taken'],['sound','Sound'],
  ['face','Faces'],['plate','Number plates']];
// Only the kinds some camera can raise: a camera or profile that starts
// watching for dogs makes the Dog chip appear.
function filterTypes() {
  const cams = (status && status.cameras) || [];
  if (!cams.length) return FILTER_BASE;
  const on = new Set(cams.flatMap(c => [...(c.alerts_on || []), ...(c.enabled && c.ai ? c.watch || [] : [])]));
  const named = new Set(FILTER_BASE.map(t => t[0]));
  const extra = [...on].filter(k => !named.has(k)).sort().map(k => [k, k[0].toUpperCase() + k.slice(1)]);
  const list = FILTER_BASE.filter(t => on.has(t[0]));
  const at = list.findIndex(t => t[0] === 'package');
  list.splice(at < 0 ? list.length : at, 0, ...extra);
  return list;
}
// Every kind of alert, for "all alerts": also objects without a chip.
function allAlertTypes() {
  const base = FILTER_BASE.map(t => t[0]).filter(t => t !== 'face' && t !== 'plate');
  const extra = ((status && status.cameras) || []).flatMap(c => c.watch || []);
  return [...new Set([...base, 'bicycle', ...extra, 'other'])];
}

let status = null, settings = null, cameraCfgs = [], sysinfo = null;
let page = 'live';

const fmtBytes = (b) => {
  if (b == null || isNaN(b)) return '–';
  const u = ['B','KB','MB','GB','TB']; let i = 0;
  while (Math.abs(b) >= 1024 && i < u.length - 1) { b /= 1024; i++; }
  return b.toFixed(i ? (b < 10 ? 2 : 1) : 0) + ' ' + u[i];
};
// "3 days ago" for a date.
const ago = (d) => {
  const s = (Date.now() - new Date(d).getTime()) / 1000;
  if (!isFinite(s)) return '';
  if (s < 3600) return 'just now';
  if (s < 86400) { const h = Math.round(s / 3600); return h + (h === 1 ? ' hour ago' : ' hours ago'); }
  const n = Math.round(s / 86400); return n === 1 ? 'yesterday' : n + ' days ago';
};
// How long ago, for the alert cards: "just now", "5 min ago", "3 h ago".
const cardAgo = (d) => {
  const s = (Date.now() - new Date(d).getTime()) / 1000;
  if (!isFinite(s)) return '';
  if (s < 60) return 'just now';
  if (s < 3600) return Math.floor(s / 60) + ' min ago';
  if (s < 86400) return Math.floor(s / 3600) + ' h ago';
  const n = Math.floor(s / 86400); return n === 1 ? 'yesterday' : n + ' days ago';
};
// Keep "… ago" on the cards up to date.
setInterval(() => document.querySelectorAll('.rc .ago[data-t]').forEach(e => { e.textContent = cardAgo(e.dataset.t); }), 30000);
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

// A video's frame rate, measured while it plays (15 until known): for
// stepping one frame at a time.
function videoFps(v) { return (v && v._fps) || 15; }
function watchFps(v) {
  if (!v || !v.requestVideoFrameCallback || v._fpsWatch) return;
  v._fpsWatch = true;
  let last = null;
  const cb = (now, md) => {
    if (last && md.presentedFrames > last.f && md.mediaTime > last.t) {
      const fps = (md.presentedFrames - last.f) / (md.mediaTime - last.t);
      if (fps > 1 && fps < 121) v._fps = v._fps ? v._fps * 0.8 + fps * 0.2 : fps;
    }
    last = v.paused ? null : {f: md.presentedFrames, t: md.mediaTime};
    v.requestVideoFrameCallback(cb);
  };
  v.requestVideoFrameCallback(cb);
}
// Time-lapse previews are switched off for now (making them kept the
// computer busy): scrubbing moves the video itself instead.
// Time-lapse previews on (Settings → Recording & storage; from the status).
let TIMELAPSE = true;
if (!TIMELAPSE) document.documentElement.classList.add('no-timelapse');
async function api(path, opts = {}) {
  if (opts.json !== undefined) {
    opts.body = JSON.stringify(opts.json); delete opts.json;
    opts.headers = {'Content-Type': 'application/json'};
  }
  const r = await fetch(path, opts);
  if (r.status === 401 && !path.startsWith('api/auth')) { showAuth('login'); throw new Error('Please sign in'); }
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
let lastButton = null;
document.addEventListener('click', (e) => { const el = e.target.closest && e.target.closest('button'); if (el) { const home = el.closest('[id]'); lastButton = {el, at: Date.now(), home: home && home.id, text: el.textContent.trim()}; } }, true);
function toast(msg, isError) {
  const t = $('toast'), ok = !isError && /^saved|\bsaved\b/i.test(msg);
  t.textContent = (ok ? '✓ ' : '') + msg; t.className = isError ? 'error' : ok ? 'ok' : ''; t.hidden = false;
  // A green tick on the button that saved it, so it's clear it worked.
  const lb = ok && lastButton && Date.now() - lastButton.at < 15000 ? lastButton : null;
  if (lb) {
    lastButton = null;
    // The page may redraw the form after saving: then tick the same button
    // in the new form.
    const find = () => {
      if (lb.el.isConnected) return lb.el;
      const box = lb.home && document.getElementById(lb.home);
      return box && [...box.querySelectorAll('button')].find(x => x.textContent.trim() === lb.text);
    };
    const tick = () => {
      const b = find(); if (!b || b.querySelector('.saved-tick')) return;
      const k = document.createElement('span'); k.className = 'saved-tick'; k.textContent = '✓'; b.append(k);
      setTimeout(() => k.remove(), 2200);
    };
    tick(); setTimeout(tick, 400); setTimeout(tick, 1200);
  }
  clearTimeout(toastTimer); toastTimer = setTimeout(() => t.hidden = true, isError ? Math.max(7000, msg.length * 60) : 3500);
}

function fullscreen(el) {
  if (document.fullscreenElement) document.exitFullscreen();
  else goFullscreen(el);
}
// A camera on the Live grid goes full screen as its camera view, with the
// timeline that slides up from the bottom; leaving full screen goes back
// to the grid.
function goFullscreen(el) {
  if (el.classList.contains('tile') && el.closest('#dash') && el.dataset.id && typeof route === 'function') {
    fsFromTile = true;
    location.hash = '#live/' + enc(el.dataset.id); route();
    el = $('focus-surface');
  }
  (el.requestFullscreen || el.webkitRequestFullscreen || (() => {})).call(el)?.catch?.(() => {});
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
  else goFullscreen(s);
});
// "Connecting…" for at most 30 s, then "Camera offline" (red) until a
// picture arrives.
function offlineAfter(msg, img, secs = 30) {
  if (!msg || !msg.textContent.startsWith('Connecting')) return;
  const started = img.getAttribute('src') ? null : Date.now();
  if (!started) return;
  const t = setInterval(() => {
    if (!msg.isConnected || msg.hidden || img.getAttribute('src')) { clearInterval(t); msg.classList.remove('offline'); return; }
    if (Date.now() - started > secs * 1000) {
      msg.classList.add('offline');
      msg.innerHTML = '<span class="off-dot"></span>Camera offline<small>No picture for 30 seconds. It keeps trying to reconnect.</small>';
      clearInterval(t);
      const back = () => { msg.classList.remove('offline'); msg.hidden = true; img.removeEventListener('load', back); };
      img.addEventListener('load', back);
    }
  }, 1000);
}
// A single click that isn't the first half of a double-click.
function singleClick(fn, delay = 260) {
  let t = null;
  return (e) => {
    clearTimeout(t);
    if (e.detail > 1) return;
    t = setTimeout(() => fn(e), delay);
  };
}
// Zoom a whole-picture thumbnail into the recognized object (the box stays the same size).
// Boxes around what was detected in an alert picture, for review. The
// picture is the whole camera view, so boxes are placed in % of it.
let showBoxes = store_get('show-boxes', true);
document.body.classList.toggle('no-boxes', !showBoxes);
function boxesHtml(ev, labels) {
  if (!ev || !ev.boxes || !ev.boxes.length) return '';
  return '<div class="bxs">' + ev.boxes.map(b => {
    const motion = b.label === 'motion';
    const tip = motion ? 'motion' : `${b.label} ${Math.round(b.score * 100)}%`;
    return `<div class="bx ${motion ? 'motion' : ''} ${b.y < 0.07 ? 'top' : ''} ${b.x + b.w > 0.6 ? 'right' : ''}" title="${esc(tip)}" style="left:${(b.x * 100).toFixed(2)}%;top:${(b.y * 100).toFixed(2)}%;width:${(b.w * 100).toFixed(2)}%;height:${(b.h * 100).toFixed(2)}%">${labels ? `<span>${motion ? labelIcon('motion') + ' motion' : labelIcon(b.label) + ' ' + esc(tip)}</span>` : ''}</div>`;
  }).join('') + '</div>';
}
function labelTags(labels, motion) {
  if (labels && labels.length) return labels.map(l => `<span class="tag ${LABEL_GROUP[l.label] || ''} ${hdClass(l)}" title="${hdTip(l)}">${esc(l.label)} ${Math.round(l.score * 100)}%${hdMark(l)}</span>`).join('');
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
const MODE_TEXT = {continuous: 'HD 24/7', continuous_plus_events: 'HD 24/7 plus HD alert clips', continuous_sub: 'Sub-stream 24/7', sub_plus_events: 'Sub-stream 24/7 plus HD alerts', events: 'HD on alerts only', off: 'Not recording'};

// ======================================================================
// Live video over one WebSocket
// ======================================================================
const Live = {
  ws: null, wanted: [], imgs: new Map(), mseCount: new Map(), urls: new Map(), stats: new Map(), connected: false,
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
    liveBytes += b.length;
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
  fps(id, q) {
    const k = this.key(id, q), v = this.vstats.get(k);
    if (this.mse.has(k) && v && performance.now() - v.last < 3000) return v.fps;
    const s = this.stats.get(k); return s && performance.now() - s.last < 3000 ? s.fps : 0;
  },
  vstats: new Map(),
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

// Installing GAZE as an app (home screen / Start menu). Browsers allow it on
// a secure (https) address; on iPhone it's Share → Add to Home Screen.
if ('serviceWorker' in navigator && window.isSecureContext) navigator.serviceWorker.register('sw.js').catch(() => {});
let installPrompt = null;
addEventListener('beforeinstallprompt', (e) => { e.preventDefault(); installPrompt = e; $('um-install').hidden = false; });
addEventListener('appinstalled', () => { installPrompt = null; $('um-install').hidden = true; toast('GAZE is installed — open it from your home screen or Start menu'); });
$('um-install').onclick = async () => { if (!installPrompt) return; installPrompt.prompt(); await installPrompt.userChoice.catch(() => {}); installPrompt = null; $('um-install').hidden = true; paintOpenApp(); };
// Installed already (the browser then stops offering to install it): open
// this page in the app instead. Asked of the browser where it can tell, and
// remembered when GAZE was installed from this browser. The app opens on a
// web+gaze: link (its manifest's protocol handler).
const inApp = () => matchMedia('(display-mode: standalone), (display-mode: window-controls-overlay), (display-mode: minimal-ui)').matches || navigator.standalone === true;
async function paintOpenApp() {
  const b = $('um-openapp'); if (!b) return;
  let installed = false;
  try { installed = store.get('app-installed', false); } catch (_) {}
  if (!installed && navigator.getInstalledRelatedApps) { try { installed = (await navigator.getInstalledRelatedApps()).length > 0; } catch (_) {} }
  b.hidden = inApp() || !installed || !window.isSecureContext;
  if (!b.hidden) $('um-install').hidden = true;
}
$('um-openapp').onclick = (e) => { e.preventDefault(); location.href = 'web+gaze:' + (location.hash || '#live'); };
addEventListener('appinstalled', () => { store.set('app-installed', true); paintOpenApp(); });
paintOpenApp();
// Opened from that link in the app: go to the page it was about.
{
  const o = new URLSearchParams(location.search).get('open');
  if (o) {
    const h = decodeURIComponent(o).replace(/^web\+gaze:/, '');
    history.replaceState(null, '', location.pathname + (/^#[\w\/~?=&.-]*$/.test(h) ? h : '#live'));
    dispatchEvent(new HashChangeEvent('hashchange'));
  }
}

// Password fields: an eye to see what was typed.
function addPasswordEyes(root = document) {
  root.querySelectorAll('input[type=password]:not([data-eye])').forEach(i => {
    i.dataset.eye = '1';
    const wrap = document.createElement('span'); wrap.className = 'pw-wrap';
    // The box keeps its place and size in the row (its width / flex move to the wrapper).
    for (const k of ['flex', 'width', 'minWidth', 'maxWidth', 'marginTop']) if (i.style[k]) { wrap.style[k] = i.style[k]; i.style[k] = ''; }
    if (wrap.style.width || wrap.style.flex) wrap.style.display = 'inline-flex';
    i.replaceWith(wrap); wrap.appendChild(i);
    const b = document.createElement('button');
    b.type = 'button'; b.className = 'pw-eye'; b.tabIndex = -1; b.title = 'Show the password'; b.textContent = '👁';
    b.onclick = () => { const show = i.type === 'password'; i.type = show ? 'text' : 'password'; b.textContent = show ? '🙈' : '👁'; b.title = show ? 'Hide the password' : 'Show the password'; i.focus(); };
    wrap.appendChild(b);
  });
}
addPasswordEyes();
new MutationObserver(ms => { for (const m of ms) for (const n of m.addedNodes) if (n.nodeType === 1) addPasswordEyes(n.matches && n.matches('input[type=password]') ? n.parentElement : n); }).observe(document.body, {childList: true, subtree: true});

// Home / Away / Night: which cameras send phone notifications (recording
// and alerts carry on in every mode).
const MODE_INFO = {home: ['🏠', 'Home', 'Only the cameras chosen for Home notify — usually none, or the outside ones.'],
  away: ['🚶', 'Away', 'Every chosen camera notifies.'], night: ['🌙', 'Night', 'The cameras chosen for Night notify.']};
function paintMode() {
  const mm = $('mode-menu'); if (!status || !status.mode) return;
  mm.hidden = false;
  const m = status.mode, eff = m.effective, [ic, name] = MODE_INFO[eff] || MODE_INFO.away;
  $('mode-ic').textContent = ic;
  $('mode-name').textContent = name + (eff !== m.current ? ' (auto)' : '');
  $('mode-btn').classList.toggle('mode-home', eff === 'home');
  const can = me && me.role !== 'viewer';
  $('mode-btn').disabled = !can;
  $('mode-pop').innerHTML = Object.entries(MODE_INFO).map(([k, [i, t, d]]) => `<button type="button" data-mode="${k}" class="${k === m.current ? 'on' : ''}"><b>${i} ${t}</b><small>${d}</small></button>`).join('')
    + (me && me.role === 'admin' ? '<a href="#settings/notify" class="small muted" style="padding:6px 10px;display:block">Choose the cameras for each mode…</a>' : '');
  $('mode-pop').querySelectorAll('[data-mode]').forEach(b => b.onclick = async () => {
    $('mode-pop').hidden = true;
    try { await api('api/mode', {method: 'POST', json: {mode: b.dataset.mode}}); toast(`${MODE_INFO[b.dataset.mode][0]} ${MODE_INFO[b.dataset.mode][1]} mode`); await refreshStatus(); }
    catch (e) { toast(e.message, true); }
  });
}
$('mode-btn').onclick = (e) => { e.stopPropagation(); $('mode-pop').hidden = !$('mode-pop').hidden; };
document.addEventListener('click', (e) => { if (!e.target.closest('#mode-menu')) $('mode-pop').hidden = true; });

// Other GAZE servers' cameras, under the grid (pictures refreshed every 2 s).
let remotesAt = 0, remotesTimer = null;
async function drawRemotes() {
  const box = $('remotes');
  if (page !== 'live' || focusId || !status || !status.has_remotes) { box.hidden = true; return; }
  if (Date.now() - remotesAt > 30000) {
    remotesAt = Date.now();
    let list = []; try { list = await api('api/remotes'); } catch (_) { list = []; }
    box.hidden = !list.length;
    box.innerHTML = list.map(r => `<div class="remote"><h3>🌐 ${esc(r.name || r.server || r.url)} <a class="small muted" href="${esc(r.url)}" target="_blank" rel="noopener">open ↗</a></h3>
      ${r.ok ? `<div class="remote-grid">${r.cameras.map(c => `<a class="surface tile remote-tile" href="${esc(r.url)}/#live/${enc(c.id)}" target="_blank" rel="noopener" title="Open on ${esc(r.name)}"><img alt="" data-src="remote/${r.i}/${enc(c.id)}/frame.jpg"><div class="ov ov-tl"><span class="name">${esc(c.name)}</span></div>${c.online ? '' : '<div class="msg">Offline</div>'}</a>`).join('')}</div>`
        : `<div class="err">${esc(r.error || 'Not reachable')}</div>`}</div>`).join('');
  }
  box.querySelectorAll('img[data-src]').forEach(im => { if (!im.dataset.busy) { im.dataset.busy = 1; const n = new Image(); n.onload = () => { im.src = n.src; delete im.dataset.busy; }; n.onerror = () => { delete im.dataset.busy; }; n.src = im.dataset.src + '?t=' + Date.now(); } });
}
setInterval(() => { if (!document.hidden) drawRemotes(); }, 2000);

// Snoozing a camera's notifications (from here or the notification itself).
function paintSnooze(c) {
  const sel = $('focus-snooze'); if (!sel || !c) return;
  const until = c.snoozed_until && new Date(c.snoozed_until) > new Date() ? new Date(c.snoozed_until) : null;
  sel.innerHTML = (until ? `<option value="" selected>🔕 Snoozed until ${fmtTime(until).slice(0, 5)}</option><option value="0">🔔 Notify again now</option>` : '<option value="" selected>🔔 Notifications on</option>')
    + [[30, '30 minutes'], [60, '1 hour'], [240, '4 hours'], [-1, 'Until tomorrow morning']].map(([m, t]) => `<option value="${m}">🔕 Snooze ${t}</option>`).join('');
}

// Live view as real video: the camera's own stream, repackaged by GAZE and
// played by the browser (Media Source Extensions) behind a half-second
// buffer that smooths out Wi-Fi gaps. The picture (JPEG) stream shows first
// and stays as the fallback; while the video plays it isn't sent at all.
const MSE = window.ManagedMediaSource || window.MediaSource;
const liveVideoOn = () => !!MSE && !dataSaver && store.get('live-video', true) !== false;
Live.mse = new Set();
Live.noMse = new Set();
class LiveVideo {
  constructor(img, id, q) {
    this.img = img; this.id = id; this.q = q; this.key = Live.key(id, q); this.fails = 0; this.dead = false;
    const v = this.video = document.createElement('video');
    v.className = 'live-v'; v.muted = true; v.playsInline = true; v.autoplay = true; v.disableRemotePlayback = true;
    // This picture shows its stills until this video plays (another
    // camera's video may have been here a moment ago).
    if (img.parentElement) img.parentElement.classList.remove('mse');
    img.after(v);
    this.open();
  }
  // Move to another picture of the same camera (a rebuilt tile): the video
  // carries on from what it has, with no new connection and no flash.
  attach(img) {
    this.img = img; img.after(this.video);
    this.lastMove = performance.now();
    const s = img.parentElement;
    if (this.playing) {
      if (s) { s.classList.add('mse'); const m = s.querySelector(':scope > .msg'); if (m) m.hidden = true; }
      this.video.play().catch(() => {});
    } else if (s) s.classList.remove('mse');
  }
  open() {
    if (this.dead) return;
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = this.ws = new WebSocket(`${proto}//${location.host}${location.pathname.replace(/[^/]*$/, '')}live/${enc(this.id)}/video?q=${this.q}`);
    ws.binaryType = 'arraybuffer';
    this.queue = []; this.sb = null; this.head = new Uint8Array(0);
    ws.onmessage = (e) => {
      if (typeof e.data === 'string') { this.giveUp(); return; }
      const b = new Uint8Array(e.data);
      if (!this.sb) { this.head = concatBytes(this.head, b); if (!this.ms) this.setup(); return; }
      this.queue.push(b); this.pump();
    };
    ws.onclose = () => { if (this.dead) return; this.stopPlaying(); if (++this.fails > 3) return this.giveUp(); setTimeout(() => this.open(), 2000 * this.fails); };
  }
  setup() {
    // The codec, from the MP4 header (avcC / hvcC box).
    const codec = mp4Codec(this.head);
    if (!codec) { if (this.head.length > 200000) this.giveUp(); this.ms = null; return; }
    const mime = `video/mp4; codecs="${codec}"`;
    if (!MSE.isTypeSupported(mime)) { this.giveUp(); return; }
    const ms = this.ms = new MSE();
    this.video.disableRemotePlayback = true;
    this.video.src = URL.createObjectURL(ms);
    ms.addEventListener('sourceopen', () => {
      try { this.sb = ms.addSourceBuffer(mime); } catch (_) { this.giveUp(); return; }
      this.sb.addEventListener('updateend', () => this.pump());
      this.queue.unshift(this.head); this.pump();
    }, {once: true});
    const v = this.video;
    v.addEventListener('playing', () => {
      if (this.dead || this.playing) return;
      this.fails = 0; this.playing = true;
      // Several videos of the same stream can play (a grid tile and the
      // opened camera): the stream counts as video while any of them does.
      Live.mseCount.set(this.key, (Live.mseCount.get(this.key) || 0) + 1);
      Live.mse.add(this.key);
      // Only while on show: a video set aside (just left) mustn't change
      // the picture that now shows another camera.
      if (this.video.isConnected && this.img.parentElement) {
        this.img.parentElement.classList.add('mse');
        const m = this.img.parentElement.querySelector(':scope > .msg'); if (m) m.hidden = true;
      }
      if (this.q === 'main' && this.id === focusId && typeof hideFocusLoading === 'function') hideFocusLoading();
      updateSubscriptions();
    });
    // Frames shown, for the frame-rate readout.
    if (v.requestVideoFrameCallback) {
      // Counted on the video's own clock (frames per second of video), so
      // catching up a little faster, or the still pictures that arrive
      // alongside, don't change it. Kept apart from the pictures' count.
      let w = null;
      const tick = (now, md) => {
        if (this.dead) return;
        if (!w || md.mediaTime < w.t) w = {f: md.presentedFrames, t: md.mediaTime};
        else if (md.mediaTime - w.t >= 2) {
          const fps = (md.presentedFrames - w.f) / (md.mediaTime - w.t);
          if (fps > 0 && fps < 121) Live.vstats.set(this.key, {fps, last: performance.now()});
          w = {f: md.presentedFrames, t: md.mediaTime};
        } else { const s = Live.vstats.get(this.key); if (s) s.last = performance.now(); }
        v.requestVideoFrameCallback(tick);
      };
      v.requestVideoFrameCallback(tick);
    }
    // Stay close to live: a little faster when behind, a jump only when far
    // behind. Some cameras (Tapo, many Wi-Fi ones) send video in bursts:
    // half a second in hand rides those out without skipping, and catching
    // up gently keeps motion smooth.
    this.lastT = -1; this.lastMove = performance.now();
    this.timer = setInterval(() => {
      if (!this.sb || this.dead) return;
      // Frozen video (stalled decoder, a gap in the data, an error): start
      // the video again; the still pictures stand in meanwhile.
      const now = performance.now();
      if (document.hidden || !v.isConnected || v.currentTime !== this.lastT) { this.lastT = v.currentTime; this.lastMove = now; }
      else if (v.error || now - this.lastMove > 4000) { this.restart(); return; }
      if (!v.buffered.length) return;
      // Stuck just before a gap in what's buffered: jump over it.
      let inside = false;
      for (let i = 0; i < v.buffered.length; i++) if (v.currentTime >= v.buffered.start(i) - 0.05 && v.currentTime <= v.buffered.end(i)) inside = true;
      if (!inside) for (let i = 0; i < v.buffered.length; i++) if (v.buffered.start(i) > v.currentTime) { v.currentTime = v.buffered.start(i) + 0.05; this.lastT = v.currentTime; break; }
      const end = v.buffered.end(v.buffered.length - 1), lag = end - v.currentTime;
      if (lag > 3.5) { v.currentTime = end - 0.5; v.playbackRate = 1; this.lastT = v.currentTime; }  // a jump isn't playing
      else v.playbackRate = lag > 1.6 ? 1.1 : lag > 0.9 ? 1.04 : 1;
      if (v.paused) v.play().catch(() => {});
      if (!this.sb.updating && v.currentTime - v.buffered.start(0) > 20) try { this.sb.remove(0, v.currentTime - 10); } catch (_) {}
    }, 500);
  }
  pump() {
    if (!this.sb || this.sb.updating || !this.queue.length) return;
    // Far behind (a hidden tab): drop the backlog and start again from the next fragment.
    if (this.queue.length > 120) this.queue.splice(0, this.queue.length - 30);
    const b = this.queue.length > 1 ? concatBytes(...this.queue.splice(0)) : this.queue.shift();
    try { this.sb.appendBuffer(b); } catch (e) { if (e.name === 'QuotaExceededError') { try { this.sb.remove(0, this.video.currentTime - 2); } catch (_) {} } else this.ws.close(); }
  }
  stopPlaying() {
    clearInterval(this.timer); this.timer = null;
    if (this.ms) { try { URL.revokeObjectURL(this.video.src); } catch (_) {} this.video.removeAttribute('src'); this.video.load(); }
    this.ms = null; this.sb = null;
    if (this.playing) {
      this.playing = false;
      if (this.video.isConnected && this.img.parentElement) this.img.parentElement.classList.remove('mse');
      const n = (Live.mseCount.get(this.key) || 1) - 1;
      if (n > 0) Live.mseCount.set(this.key, n); else { Live.mseCount.delete(this.key); Live.mse.delete(this.key); }
      updateSubscriptions();
    }
  }
  // Start the video connection again at once (a frozen picture); too many
  // in a short time: pictures only for this stream.
  restart() {
    const now = Date.now();
    this.restarts = (this.restarts || []).filter(t => now - t < 120000); this.restarts.push(now);
    if (this.restarts.length > 6) return this.giveUp();
    if (this.ws) { this.ws.onclose = null; this.ws.close(); }
    this.stopPlaying();
    // A fresh video element: the old one may be what's stuck.
    const old = this.video, v = this.video = document.createElement('video');
    v.className = 'live-v'; v.muted = old.muted; v.volume = old.volume; v.playsInline = true; v.autoplay = true; v.disableRemotePlayback = true;
    if (old.isConnected) old.replaceWith(v);
    this.open();
  }
  giveUp() { Live.noMse.add(this.key); this.close(); }
  close() { this.dead = true; if (this.ws) { this.ws.onclose = null; this.ws.close(); } this.stopPlaying(); this.video.remove(); }
}
function concatBytes(...parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0)); let o = 0;
  for (const p of parts) { out.set(p, o); o += p.length; }
  return out;
}
// "avc1.64001f" / "hvc1.1.6.L120.90" from an MP4 header, or null until the
// whole sample description has arrived.
function mp4Codec(b) {
  const find = (tag) => { for (let i = 4; i + 4 < b.length; i++) if (b[i] === tag.charCodeAt(0) && b[i + 1] === tag.charCodeAt(1) && b[i + 2] === tag.charCodeAt(2) && b[i + 3] === tag.charCodeAt(3)) return i + 4; return -1; };
  const hex = (n) => n.toString(16).padStart(2, '0');
  let i = find('avcC');
  if (i > 0 && i + 4 <= b.length) return `avc1.${hex(b[i + 1])}${hex(b[i + 2])}${hex(b[i + 3])}`;
  i = find('hvcC');
  if (i > 0 && i + 13 <= b.length) {
    const p = b[i + 1], space = ['', 'A', 'B', 'C'][p >> 6], tier = (p >> 5) & 1 ? 'H' : 'L', prof = p & 31;
    let compat = (b[i + 2] << 24 | b[i + 3] << 16 | b[i + 4] << 8 | b[i + 5]) >>> 0, rev = 0;
    for (let k = 0; k < 32; k++) { rev = (rev << 1) | (compat & 1); compat >>>= 1; }
    return `hvc1.${space}${prof}.${(rev >>> 0).toString(16)}.${tier}${b[i + 12]}.${hex(b[i + 6])}`;
  }
  return null;
}
// A live picture: the picture stream, with smooth video on top when the
// browser can play it. Returns the unbind function.
// Players of pictures just taken off the page, kept a few seconds for the
// same camera's next picture (switching pages rebuilds the tiles).
const lvParked = new Map();
function bindLive(img, id, q) {
  const un = Live.bind(img, id, q), key = Live.key(id, q);
  let lv = null;
  if (liveVideoOn() && !Live.noMse.has(key)) {
    const parked = lvParked.get(key);
    if (parked && !parked.lv.dead) { clearTimeout(parked.timer); lvParked.delete(key); lv = parked.lv; lv.attach(img); }
    else lv = new LiveVideo(img, id, q);
  }
  return () => {
    un();
    if (!lv || lv.dead) return;
    const old = lvParked.get(key); if (old) { clearTimeout(old.timer); old.lv.close(); }
    lv.video.remove();
    lvParked.set(key, {lv, timer: setTimeout(() => { if (lvParked.get(key) && lvParked.get(key).lv === lv) { lvParked.delete(key); lv.close(); } }, 5000)});
  };
}

// Which streams the page needs right now. The dashboard keeps running in the
// background on every page so switching back or opening a camera is instant.
// Which stream a picture shows: Stream 1 (main, full quality) or Stream 2
// (sub-stream). `ctx` is 'grid' (tiles) or 'focus' (an opened camera, or
// one enlarged on the video wall). The viewer can switch each one with the
// "S1/S2" button on the picture; the choice is remembered in this browser.
const streamPref = store_get('stream-pref', {});
// The Stream 1 / Stream 2 switch above the grid sets every camera at once
// (stream 2 unless chosen otherwise).
let gridStream = store_get('grid-stream', 'sub');
function feedQ(c, ctx) {
  const p = streamPref[c.id + '|' + ctx];
  if (p === 'main' || p === 'sub') return p;
  return ctx === 'focus' ? 'main' : gridStream;
}
function setGridStream(q) {
  gridStream = q;
  try { localStorage.setItem('nvr-grid-stream', JSON.stringify(q)); } catch (e) {}
  for (const k of Object.keys(streamPref)) if (k.endsWith('|grid')) delete streamPref[k];
  try { localStorage.setItem('nvr-stream-pref', JSON.stringify(streamPref)); } catch (e) {}
  paintGridStream();
  renderDash(); updateSubscriptions();
}
function paintGridStream() {
  document.querySelectorAll('#grid-stream button').forEach(b => b.classList.toggle('on', b.dataset.q === gridStream));
}
document.querySelectorAll('#grid-stream button').forEach(b => b.onclick = () => setGridStream(b.dataset.q));
paintGridStream();
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
    // The camera editor's pictures (motion mask, package spot, zones) use stream 2.
    if ($('cam-dialog').open && editingId === c.id) add('sub', gridFps);
    if (focused) {
      add(focusQ, hdFps);
      // The sub-stream stands in while the full-quality picture loads.
      if (focusQ === 'main' && !dataSaver) add('sub', gridFps);
    } else if (gridQ === 'sub' && l.preload_hd_streams && !dataSaver) add('main', 1);
    // Live on the Recordings tab.
    let recLive = false; try { recLive = page === 'recordings' && tl.live && tl.data.some(x => x.camera === c.id); } catch (_) {}
    if (recLive) add(tl.quality === 'main' ? 'main' : 'sub', gridFps);
    for (const [q, fps] of want) subs.push({camera: c.id, q, fps});
  }
  if (dataSaver) subs.forEach(x => x.lite = true);
  // Pictures that play as video don't need the picture stream.
  Live.setWanted(subs.filter(x => !Live.mse.has(Live.key(x.camera, x.q))));
}

// Overlays (name, clock, fps, status, fullscreen) for a live surface.
// Little symbols for what was detected (also used on timelines).
const LABEL_ICON = {person: '🚶', vehicle: '🚗', car: '🚗', truck: '🚚', bus: '🚌', motorcycle: '🏍️', bicycle: '🚲', dog: '🐕', cat: '🐈', bird: '🐦',
  horse: '🐎', cow: '🐄', sheep: '🐑', bear: '🐻', backpack: '🎒', suitcase: '🧳', handbag: '👜', umbrella: '☂️', package: '📦', motion: '〰️', sound: '🔊', doorbell: '🔔'};
const labelIcon = (l) => LABEL_ICON[l] || '🔷';
// The full-quality (HD) check of what the AI found: ✓ confirmed, ? not.
const hdMark = (l) => l.hd === true ? ' <b class="hdc">HD✓</b>' : l.hd === false ? ' <b class="hdc no">HD?</b>' : '';
// On an alert, next to the camera's name: the AI's find was checked in HD.
const hdAi = (e) => !e || !e.labels ? '' : e.labels.some(l => l.hd === true) ? '<span class="hdai" title="What the AI found was confirmed in a full-quality picture">HD AI ✓</span>'
  : e.labels.some(l => l.hd === false) ? '<span class="hdai no" title="Not confirmed in a full-quality picture">HD AI ?</span>' : '';
const hdClass = (l) => l.hd === false ? 'unconfirmed' : '';
const hdTip = (l) => l.hd === true ? 'confirmed in a full-quality picture' : l.hd === false ? 'not confirmed in a full-quality picture' : 'found in the sub-stream';
// What the timeline colours and symbols mean.
// What the marks on a timeline mean: only what's on it — the kinds of
// footage shown and the objects these cameras look for.
function legendHtml(kind, data) {
  const rows = data || [], segs = rows.flatMap(c => c.segments || []), evs = rows.flatMap(c => c.events || []);
  const watched = [...new Set(rows.flatMap(c => (cam(c.camera) || {}).watch || []))];
  // Green: sub-stream footage; purple: main stream (HD), 24/7 or alert clips.
  const sm = (x) => x.stream || (x.kind === 'sub' ? 'sub' : 'main');
  const bars = [];
  if (!segs.length || segs.some(x => sm(x) === 'sub')) bars.push(['<i class="lg-bar rec"></i>', 'Sub-stream']);
  if (segs.some(x => sm(x) === 'main')) bars.push(['<i class="lg-bar clip"></i>', segs.every(x => sm(x) !== 'main' || x.kind === 'clip') ? 'HD alert clips' : 'Main stream (HD)']);
  // Recording now: purple stripes when the HD main stream is what's shown.
  const grow = segs.filter(isGrowing);
  const hdNow = kind === 'tl' ? tlQ() === 'main' : grow.length && grow.every(x => sm(x) === 'main');
  if (grow.length) bars.push([`<i class="lg-bar live${hdNow ? ' clip' : ''}"></i>`, 'Recording now']);
  // Alerts in the colour of their kind (only kinds that are on it).
  const cats = new Set(evs.map(evCat));
  for (const [k, t] of [['person', 'People'], ['vehicle', 'Vehicles'], ['animal', 'Animals'], ['package', 'Packages'], ['other', 'Other'], ['motion', 'Motion only']])
    if (cats.has(k)) bars.push([`<i class="lg-bar ev cat-${k}"></i>`, t]);
  if (TIMELAPSE) bars.push(['<i class="lg-bar tlr"></i>', 'Time-lapse ready']);
  return bars.map(([i, t]) => `<span>${i}${t}</span>`).join('')
    + (watched.length ? '<span class="lg-sep"></span>' + watched.map(o => `<span>${labelIcon(o)} ${esc(o)}</span>`).join('') : '');
}
// On-screen display: the camera's own settings, or the global ones.
function osdFor(c) {
  if (c.osd && c.osd.custom) return c.osd;
  return {...status.display.osd, clock_format: status.display.osd.clock_format || ''};
}
// The same on-screen display on every picture, live or recorded: the
// camera's items where its display settings put them, and one cluster of
// buttons in one corner — stream (live only), sound with volume, full
// screen. `opts.rec`: a recording (the clock shows its time and a ▶
// PLAYBACK tag stands where live shows REC); `opts.tools`: that view's own
// sound button(s) instead of the live one; `opts.extra`: a class for the boxes.
function overlayHtml(c, q, noFs, opts = {}) {
  const d = status.display, o = osdFor(c), rec = !!opts.rec;
  const parts = Object.fromEntries(SPOTS.map(([k]) => [k, []]));
  // A spot that isn't one of the nine ("p:X,Y") gets its own box.
  for (const k of ['name_corner', 'text_corner', 'status_corner', 'detections_corner', 'clock_corner', 'fps_corner', 'res_corner']) if (o[k] && !parts[o[k]]) parts[o[k]] = [];
  if (o.show_name) parts[o.name_corner].push(`<span class="ovtext name" data-osd="name">${esc(c.name)}</span>`);
  if (o.text) parts[o.text_corner].push(`<span class="ovtext" data-osd="text">${esc(o.text)}</span>`);
  // Live: the camera's state (REC / armed / offline); a recording: ▶ PLAYBACK, always.
  if (rec) parts[o.status_corner || 'tl'].push('<span class="ov-tag pb-tag">▶ PLAYBACK</span>');
  else if (o.show_status) parts[o.status_corner].push(`<span class="ov-pill" data-osd="status">${pillHtml(c)}</span>`);
  if (o.show_detections && !rec) parts[o.detections_corner].push('<span class="ov-alert" data-osd="detections"></span>');
  const fmt = o.clock_format || d.clock_format || '';
  if (o.show_clock) parts[o.clock_corner].push(rec ? `<span class="ovtext ov-rclock tl-t p-clock" data-fmt="${esc(fmt)}"></span>`
    : `<span class="ovtext ov-clock" data-osd="clock" data-fmt="${esc(fmt)}">${esc(fmtDate(new Date(), fmt || undefined))}</span>`);
  if (o.show_fps && !rec) parts[o.fps_corner].push('<span class="ovtext ov-fps" data-osd="fps"></span>');
  if (o.show_res && !rec) parts[o.res_corner || 'tl'].push('<span class="ovtext ov-res" data-osd="res"></span>');
  // The buttons: the first full-screen corner (top right unless chosen otherwise).
  const corners = noFs ? [] : d.fullscreen_corners;
  const sc = corners[0] || 'tr';
  // One row of buttons, in the same order everywhere.
  const row = [];
  if (opts.tools != null) row.push(opts.tools);
  else {
    // Live sound: speaker (click: on/off; hover: volume).
    if (c.sound !== false) row.push(`<span class="snd-wrap"><button type="button" class="fsbtn snd" data-cam="${esc(c.id)}" title="Sound on/off">${soundCam === c.id ? '🔊' : '🔇'}</button><input type="range" class="snd-vol" min="0" max="1" step="0.05" value="${soundVolume()}" title="Volume"></span>`);
    row.push(`<button type="button" class="fsbtn sbtn" title="${q === 'main' ? 'Showing stream 1 (main, full quality). Click for stream 2 (sub-stream, lighter).' : 'Showing stream 2 (sub-stream). Click for stream 1 (main, full quality).'}">${q === 'main' ? 'S1' : 'S2'}</button>`);
  }
  const fsBtn = '<button type="button" class="fsbtn fs-go" title="Full screen (or double-click the picture)">⛶</button>';
  // ⛶ once, with the other buttons (no second one in another corner).
  if (corners.length || opts.rec) row.push(fsBtn);
  parts[sc].unshift(`<span class="ov-btns">${row.join('')}</span>`);
  return Object.entries(parts).filter(([, v]) => v.length).map(([k, v]) => ovBox(k, v.join(''), opts.extra || '')).join('');
}
// One on-screen display box: at one of the nine places, or anywhere
// ("p:X,Y", per cent of the picture; the box stays inside the picture).
function ovBox(k, inner, extra = '') {
  const m = /^p:([\d.]+),([\d.]+)$/.exec(k || '');
  if (m) return `<div class="ov free ${extra}" style="left:${m[1]}%;top:${m[2]}%;transform:translate(-${m[1]}%,-${m[2]}%)">${inner}</div>`;
  return `<div class="ov ${k} ${extra}">${inner}</div>`;
}
// The on-screen display items and their places, as form rows (`px`: id prefix).
const OSD_ITEMS = [['name', 'Camera name'], ['clock', 'Date &amp; time'], ['status', 'Recording status (REC / armed / offline)'], ['detections', 'Alert type (motion, 🚶 person, 🚗 car …)'], ['fps', 'Frame rate'], ['res', 'Resolution and video format']];
function osdItemsHtml(px, o) {
  return OSD_ITEMS.map(([k, t]) => `<span></span><div class="row osd-row"><label class="check" style="padding:0;min-width:300px"><input type="checkbox" id="${px}-${k}" ${o['show_' + k] ? 'checked' : ''}> ${t}</label>${cornerSel(`${px}-${k}-c`, o[k + '_corner'])}</div>`).join('')
    + `<label for="${px}-text">Extra text</label><div class="row"><input type="text" id="${px}-text" value="${esc(o.text || '')}" maxlength="200" placeholder="e.g. Driveway – north" style="flex:1">${cornerSel(px + '-text-c', o.text_corner)}</div>
    <span></span><div class="hint">Drag any item on the preview and drop it where you want it, or pick one of the nine places.</div>`;
}
function readOsdItems(px) {
  const o = {};
  for (const [k] of OSD_ITEMS) { o['show_' + k] = chk(`${px}-${k}`); o[k + '_corner'] = val(`${px}-${k}-c`); }
  o.text = val(px + '-text').trim(); o.text_corner = val(px + '-text-c');
  return o;
}
// Drag the items on a preview to place them (`enabled()`: may they move now).
function wireOsdDrag(prev, px, enabled, redraw) {
  prev.onpointerdown = (e) => {
    const item = e.target.closest('[data-osd]');
    if (!item) return;
    if (!enabled()) { toast('Tick “Use settings just for this camera” to move items.'); return; }
    e.preventDefault();
    const k = item.dataset.osd, ghost = item.cloneNode(true), ir = item.getBoundingClientRect();
    // Where in the item it was picked up, so it lands exactly where it's let go.
    const gx = e.clientX - ir.left, gy = e.clientY - ir.top;
    ghost.classList.add('osd-ghost'); ghost.style.width = ir.width + 'px'; document.body.appendChild(ghost);
    const move = (ev) => { ghost.style.left = (ev.clientX - gx) + 'px'; ghost.style.top = (ev.clientY - gy) + 'px'; };
    move(e);
    item.style.opacity = .3;
    const up = (ev) => {
      document.removeEventListener('pointermove', move); document.removeEventListener('pointerup', up);
      ghost.remove(); item.style.opacity = '';
      const r = prev.getBoundingClientRect();
      if (ev.clientX < r.left - 40 || ev.clientX > r.right + 40 || ev.clientY < r.top - 40 || ev.clientY > r.bottom + 40) return;
      // The item's top-left corner where it was dropped, kept inside the
      // picture; stored so the box sits the same way on any picture size
      // (left = L·(W − w), as the box is shifted by L of its own width).
      const left = ev.clientX - gx - r.left, top = ev.clientY - gy - r.top;
      const fx = r.width > ir.width ? Math.min(1, Math.max(0, left / (r.width - ir.width))) : 0;
      const fy = r.height > ir.height ? Math.min(1, Math.max(0, top / (r.height - ir.height))) : 0;
      const spot = `p:${(fx * 100).toFixed(1)},${(fy * 100).toFixed(1)}`;
      const inp = $(`${px}-${k}-c`);
      if (inp) { inp.value = spot; paintCorners(); redraw(); }
    };
    document.addEventListener('pointermove', move); document.addEventListener('pointerup', up);
  };
}
function wireOverlay(surface) {
  surface.querySelectorAll('.snd').forEach(b => {
    b.onpointerdown = (e) => e.stopPropagation();
    b.ondblclick = (e) => e.stopPropagation();
    b.onclick = (e) => { e.stopPropagation(); soundCam === b.dataset.cam ? stopSound() : startSound(b.dataset.cam); };
  });
  surface.querySelectorAll('.snd-wrap:not(.p-tools) .snd-vol').forEach(r => {
    r.onpointerdown = (e) => e.stopPropagation();
    r.onclick = (e) => e.stopPropagation();
    r.oninput = (e) => { e.stopPropagation(); setSoundVolume(+r.value); };
  });
  surface.querySelectorAll('.fs-go').forEach(b => { if (!b.closest('.pov-osd')) b.onclick = (e) => { e.stopPropagation(); fullscreen(surface); }; });
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
  // The choice is the camera's, in the grid, opened and full screen alike,
  // so it doesn't flip back when going in or out of full screen.
  setFeedQ(c, 'focus', q); setFeedQ(c, 'grid', q);
  if (isFocus) bindFocusImage(c);
  else {
    // Swap the picture's stream in place: rebuilding the tiles would take
    // the picture out of full screen.
    const img = surface.querySelector(':scope > img'), u = unbinders.get(c.id);
    if (u) u();
    surface.dataset.q = q;
    if (img) unbinders.set(c.id, bindLive(img, c.id, q));
    updateOverlay(surface, c, q);
  }
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
    fps.textContent = shown ? `${shown.toFixed(1)} fps` : '– fps';
  }
  const res = surface.querySelector('.ov-res');
  if (res) {
    const info = c.info && (q === 'main' ? c.info.main : c.info.sub);
    res.textContent = info ? `${info.width}×${info.height} ${String(info.codec).toUpperCase()}` : '–';
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
const PAGES = ['live', 'recordings', 'system', 'settings'];
// A notification link: #alerts?open=<camera>~<alert id> opens that alert.
let openAlertId = null, lastNavPage = null;
function route() {
  const [h, q] = (location.hash.slice(1) || 'live').split('?');
  const want = q && new URLSearchParams(q).get('open');
  if (want) { openAlertId = want.split('~').pop(); history.replaceState(null, '', '#' + h); }
  // The logs are under Settings now.
  if (h === 'logs') { location.replace('#settings/logs'); return; }
  const [p0, sub, extra] = h.split('/');
  // Alerts and Recordings (timeline) share one page section.
  const p = p0 === 'alerts' ? 'recordings' : p0;
  page = PAGES.includes(p) ? p : 'live';
  const navPage = p0 === 'alerts' ? 'alerts' : page;
  // Coming back to Alerts from anywhere else: the filters start afresh.
  if (navPage === 'alerts' && lastNavPage && lastNavPage !== 'alerts') resetAlertFilters();
  lastNavPage = navPage;
  if (typeof wall !== 'undefined' && wall && (page !== 'live' || sub)) exitWall();
  document.querySelectorAll('nav a').forEach(a => a.classList.toggle('active', a.dataset.page === navPage));
  for (const s of PAGES) $('page-' + s).hidden = s !== page;
  if (page !== 'live' && typeof soundCam !== 'undefined' && soundCam) stopSound();
  if (page !== 'recordings') pauseAllPlayback();
  // Leaving a page: its downloads stop (see cutPageDownloads), and videos
  // left behind on it come back when it's shown again.
  if (page !== lastPage) { cutPageDownloads(); lastPage = page; }
  unparkVideos($('page-' + page));
  if (page === 'live') showLive(sub ? decodeURIComponent(sub) : null);
  else if (page === 'recordings') showRecordings(sub ? decodeURIComponent(sub) : null, p0 === 'alerts' ? 'list' : 'timeline');
  else if (page === 'system') showSystem();
  else showSettings(sub || 'cameras');
  updateSubscriptions();
}
window.addEventListener('hashchange', route);
// Clicking Recordings while it's open loads it afresh: newest footage,
// every camera's video reloaded at the same moment (cameras chosen stay),
// time-lapse pictures that failed asked for again.
document.querySelector('nav a[data-page="recordings"]').addEventListener('click', (e) => {
  if (page !== 'recordings' || rec.mode !== 'timeline') return;
  e.preventDefault();
  if (location.hash !== '#recordings' && !location.hash.startsWith('#recordings/timeline')) { location.hash = '#recordings'; return; }
  cutSpriteDownloads();
  for (const [u, x] of tl.sprites) if (x.failed) tl.sprites.delete(u);
  tl.hd = null; tl.pv = new Map();
  loadTimeline();
  toast('Recordings reloaded');
});
// A GAZE window or tab that isn't being looked at doesn't load Recordings
// in the background (several open windows would all be fetching and asking
// for time-lapse pictures); it carries on when it's shown again.
document.addEventListener('visibilitychange', () => {
  if (document.hidden) { cutPageDownloads(); pauseAllPlayback(); return; }
  if (page === 'recordings' && rec.mode === 'timeline' && tl.key) { tlSync(true); prefetchSprites(); }
  else if (page !== 'recordings' && !tl.key) setTimeout(preloadRecordings, 1500);
});
// A browser only opens a few connections (6) to the NVR at a time. The
// time-lapse pictures of Live and Recordings (each can take a while to
// make) and half-loaded videos kept them all busy after leaving those
// pages, so the next page (e.g. Settings) waited seconds for its turn.
let lastPage = null, pageNet = new AbortController();
function cutPageDownloads() {
  cutSpriteDownloads();
  // Recordings' continuous players stop fetching (they carry on when shown).
  document.querySelectorAll('#tl-videos .surface').forEach(el => { if (el._h) { el._h.hls.stopLoad(); el._h.started = false; } });
  for (const s of PAGES) if (s !== page) parkVideos($('page-' + s));
}
// Stop time-lapse pictures still being made or downloaded (asked again
// when needed).
function cutSpriteDownloads() {
  pageNet.abort(); pageNet = new AbortController();
  tl.pfGen = (tl.pfGen || 0) + 1; camTl.pfGen++; clearTimeout(camTl.pfRetry);
  for (const [u, e] of tl.sprites) if (!e.done) tl.sprites.delete(u);
  for (const [u, im] of spritePages) if (!im.complete) { im._cut = true; spritePages.delete(u); im.src = ''; }
}
// A paused video that's still downloading keeps its connection: let go of
// it (where it was is kept, and it's put back when the page is shown).
function parkVideos(root) {
  for (const v of root.querySelectorAll('video:not([data-hls])')) {
    if (v.dataset.parked || v.networkState !== HTMLMediaElement.NETWORK_LOADING || !v.getAttribute('src')) continue;
    v.dataset.parked = v.getAttribute('src'); v.dataset.parkedAt = v.currentTime;
    keepStill(v);
    v.pause(); v.removeAttribute('src'); v.load();
  }
}
function unparkVideos(root) {
  if (!root) return;
  for (const v of root.querySelectorAll('video[data-parked]')) {
    const src = v.dataset.parked, t = +v.dataset.parkedAt || 0;
    delete v.dataset.parked; delete v.dataset.parkedAt;
    if (v.getAttribute('src')) continue;  // already given something new
    v.addEventListener('loadedmetadata', () => { try { v.currentTime = t; } catch (_) {} }, {once: true});
    v.src = src;
    dropStillWhenShown(v);
  }
}
// The picture a video was showing, kept on screen while it's let go of and
// loaded again (no black box when coming back to the page).
function keepStill(v) {
  if (v.readyState < 2 || v.style.visibility === 'hidden' || v.hidden || !v.videoWidth) return;
  try {
    const w = Math.min(960, v.videoWidth), c = document.createElement('canvas');
    c.width = w; c.height = Math.round(v.videoHeight * w / v.videoWidth);
    c.getContext('2d').drawImage(v, 0, 0, c.width, c.height);
    if (v._still) v._still.remove();
    const im = v._still = document.createElement('img');
    im.className = 'park-still'; im.alt = '';
    im.style.objectFit = getComputedStyle(v).objectFit;
    im.src = c.toDataURL('image/jpeg', 0.85);
    v.after(im);
  } catch (_) {}
}
function dropStillWhenShown(v) {
  const im = v._still; if (!im) return;
  const done = () => { if (v._still === im) { v._still = null; im.remove(); } };
  // Gone once the video shows a picture again (or another player took over).
  const ck = () => { if (v._still !== im) return; if ((v.readyState >= 2 && !v.seeking) || v.style.visibility === 'hidden') done(); else setTimeout(ck, 80); };
  setTimeout(ck, 80); setTimeout(done, 10000);
}

async function refreshStatus() {
  try { status = await api('api/status?device=' + deviceKind()); } catch (e) { return; }
  $('nvr-name').title = status.name;
  if (status.version && $('brand-ver')) $('brand-ver').textContent = 'v' + status.version;
  // The server knows which sirens are sounding.
  for (const id of [...sirenOn]) if (!(status.sirens || []).includes(id)) sirenOn.delete(id);
  paintSirens();
  document.title = status.name && status.name !== 'GAZE' ? `GAZE — ${status.name}` : 'GAZE';
  // How live pictures fill their boxes, everywhere (stream 2 is often a
  // squeezed 4:3 picture: stretched, it fills the box like the main stream).
  document.body.dataset.fit = 'stretch';
  // New alerts.
  const ab = $('alert-badge'), n = status.alerts_unseen || 0;
  if (status.timelapse != null && status.timelapse !== TIMELAPSE) {
    TIMELAPSE = status.timelapse;
    tl.scrub = TIMELAPSE ? 'timelapse' : 'sub';
    document.documentElement.classList.toggle('no-timelapse', !TIMELAPSE);
  }
  paintMode();
  // A camera or profile now watches for something else: redo the chips.
  const kinds = filterTypes().map(t => t[0]).join();
  if (kinds !== refreshStatus.kinds) { const was = refreshStatus.kinds; refreshStatus.kinds = kinds; if (was != null) renderFilters(); }
  ab.hidden = !n; ab.textContent = n > 99 ? '99+' : n; ab.title = `${n} new alert${n === 1 ? '' : 's'}`;
  const da = $('f-seen-all');
  if (da) { da.textContent = n ? `✓ Dismiss all new (${n > 999 ? '999+' : n})` : '✓ No new alerts'; da.disabled = !n; da.classList.toggle('primary', !!n); }
  // Downloads (AI models, updates): a small chip in the header; the
  // details are in Settings → Tasks.
  const dls = status.downloads || [], chip = $('dl-chip');
  chip.hidden = !dls.length;
  if (dls.length) {
    const d = dls[0], pct = d.total ? Math.round(100 * d.done / d.total) : null;
    chip.textContent = `⬇ ${pct != null ? pct + '%' : '…'}${dls.length > 1 ? ` (+${dls.length - 1})` : ''}`;
    chip.title = dls.map(x => x.title).join('\n') + '\nClick for details (Settings → Tasks)';
  }
  showUpdateBar(status.update);
  paintUpdBtn(status.update);
  alertsChanged();
  const gb = $('g-banner');
  if (status.safe_mode) {
    // The settings file couldn't be read: GAZE started without it.
    gb.hidden = false; gb.classList.add('safe');
    if (gb.dataset.safe !== status.safe_mode) {
      gb.dataset.safe = status.safe_mode;
      gb.innerHTML = `<b>⚠ Safe mode: GAZE couldn't read its settings file</b>, so it started without your cameras (nothing is recorded or changed). ${me && me.role === 'admin'
        ? `<button type="button" class="small" id="sm-fix">Fix the settings file…</button>${status.safe_mode_good_copy ? ' <button type="button" class="small primary" id="sm-restore">Restore the last good settings</button>' : ''}<div class="muted small" style="margin-top:4px">${esc(status.safe_mode)}</div>` : 'Ask the admin to fix it.'}`;
      if ($('sm-fix')) $('sm-fix').onclick = openSafeEditor;
      if ($('sm-restore')) $('sm-restore').onclick = async () => {
        if (!confirm('Put back the settings from the last time GAZE started fine? The broken file is kept as nvr.toml.broken.')) return;
        try { const r = await api('api/safe-mode/restore', {method: 'POST'}); toast(r.message); setTimeout(() => location.reload(), 6000); } catch (e) { toast(e.message, true); }
      };
    }
  } else if (status.new_cameras && !store.get('newcam-dismissed-' + status.new_cameras, false)) {
    gb.hidden = false;
    gb.innerHTML = `<b>📷 New camera${status.new_cameras > 1 ? 's' : ''} found on your network</b> (${status.new_cameras}) — <a href="#settings/cameras">add ${status.new_cameras > 1 ? 'them' : 'it'}</a> · <a href="javascript:void 0" id="gb-x">dismiss</a>`;
    $('gb-x').onclick = () => { store.set('newcam-dismissed-' + status.new_cameras, true); gb.hidden = true; };
  } else gb.hidden = true;
  // Only errors count, until dismissed.
  const seen = store.get('errors-seen', 0), fresh = (status.problems.error_ids || []).filter(i => i > seen);
  const b = $('log-badge');
  b.hidden = !fresh.length;
  b.textContent = fresh.length > 99 ? '99+' : fresh.length;
  b.className = 'badge';
  b.title = `${fresh.length} new error${fresh.length === 1 ? '' : 's'} — click to review`;
  $('lg-dismiss').hidden = !fresh.length;
  updateSubscriptions();
  dashSync();
  if (page === 'live' && focusId) updateFocus();
  if (page === 'settings') settingsTick();
}

// Show sprite picture `n` (scrubbing preview) as the background of `el`
// (W×H px). Sprites come in pages of pictures, each stretched to fill its
// tile; `fill` stretches the picture over the whole box like the timeline's
// videos, otherwise it's shown in its own shape (info.aspect), centred.
// A page that hasn't loaded yet isn't shown (that blinked black): the
// previous picture stays until it's ready. Returns the picture's offsets
// in the box, or null while waiting.
const spritePages = new Map();
function spritePage(url) {
  let im = spritePages.get(url);
  if (!im) {
    im = new Image();
    // A page that failed to download is fetched afresh next time.
    im.addEventListener('error', () => { if (spritePages.get(url) === im) spritePages.delete(url); }, {once: true});
    im.src = url; spritePages.set(url, im);
  }
  return im;
}
function spriteStyle(el, sp, n, W, H, fill) {
  const i = sp.info;
  n = Math.min(i.count - 1, Math.max(0, n));
  const page = Math.floor(n / i.per_page), k = n % i.per_page;
  const col = k % i.columns, row = Math.floor(k / i.columns), rows = Math.ceil(Math.min(i.per_page, i.count - page * i.per_page) / i.columns);
  const url = sp.url + page, im = spritePage(url);
  el._want = [sp, n, W, H, fill];
  if (!(im.complete && im.naturalWidth)) {
    im.addEventListener('load', () => { const w = el._want; if (w && w[0] === sp && w[1] === n) spriteStyle(el, ...w); }, {once: true});
    return null;
  }
  const a = i.aspect || i.tile_width / i.tile_height;
  const dw = fill ? W : Math.min(W, H * a), dh = fill ? H : Math.min(H, W / a);
  const ox = (W - dw) / 2, oy = (H - dh) / 2;
  // Black bars around a picture that keeps its shape (a border, so the
  // pictures next to it on the sheet never show at the sides).
  el.style.boxSizing = 'border-box';
  el.style.border = `solid #000`;
  el.style.borderWidth = `${Math.floor(oy)}px ${Math.floor(ox)}px`;
  el.style.backgroundImage = `url("${url}")`;
  el.style.backgroundSize = `${i.columns * dw}px ${rows * dh}px`;
  el.style.backgroundPosition = `${-col * dw}px ${-row * dh}px`;
  return {ox, oy};
}
function preloadSprite(sp) {
  if (!sp) return sp;
  for (let p = 0; p < sp.info.pages; p++) spritePage(sp.url + p);
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

// ---------- data saver ----------
// Smaller live pictures, fewer frames per second, and recordings streamed
// as small low-bitrate video. Settings → Live view & display, per browser:
// "auto" is on for phone browsers (mobile data), off on computers.
let dataSaver = false, dsMode = store_get('data-saver', 'auto');
const isPhone = () => matchMedia('(pointer: coarse)').matches && Math.min(screen.width, screen.height) < 820;
// Live picture bytes received (for the bandwidth shown in the header).
let liveBytes = 0;
function applyDataSaver() {
  const was = dataSaver;
  dataSaver = dsMode === 'on' || (dsMode === 'auto' && isPhone());
  if (was !== dataSaver && typeof updateSubscriptions === 'function') updateSubscriptions();
}
function setDataSaver(mode) {
  dsMode = mode;
  try { localStorage.setItem('nvr-data-saver', JSON.stringify(mode)); } catch (err) {}
  applyDataSaver();
}
function fmtRate(bps) { const m = bps * 8 / 1e6; return m >= 1 ? m.toFixed(1) + ' Mbit/s' : Math.round(bps * 8 / 1000) + ' kbit/s'; }
// Bandwidth this page is using: live pictures plus video/pictures loaded.
const bw = {bytes: 0, since: performance.now()};
try {
  // Only what actually came over the network from this server: not
  // pictures shown from the browser's cache (transferSize 0), nor in-page
  // copies (blob:/data:) of what was already counted.
  new PerformanceObserver((list) => {
    for (const e of list.getEntries()) if (e.transferSize > 0 && e.name.startsWith(location.origin)) bw.bytes += e.transferSize;
  }).observe({type: 'resource', buffered: false});
} catch (e) {}
setInterval(() => {
  const now = performance.now(), secs = (now - bw.since) / 1000;
  const rate = (bw.bytes + liveBytes) / Math.max(1, secs);
  bw.bytes = 0; liveBytes = 0; bw.since = now;
  const el = $('bw');
  if (el) {
    el.textContent = '⇣ ' + fmtRate(rate);
    el.title = `Data streaming from GAZE to this device right now (live video, recordings, pictures)${dataSaver ? ' — data saver is on' : ''}`;
  }
}, 3000);
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
    // The controls sit on the video (and so stay there in full screen),
    // fading out while it plays until the mouse moves.
    this.surface = video.parentElement;
    this.surface.classList.add('has-pov');
    if (controls) { controls.classList.add('pov'); this.surface.appendChild(controls); }
    this.tlap = document.createElement('div');
    this.tlap.className = 'pov-tlap'; this.tlap.hidden = true;
    this.surface.appendChild(this.tlap);
    this.osd = document.createElement('div');
    this.osd.className = 'pov-osdbox';
    this.surface.appendChild(this.osd);
    let idle = null;
    const wake = () => {
      this.surface.classList.remove('pov-idle'); clearTimeout(idle);
      idle = setTimeout(() => { if (!this.video.paused && !this.surface.matches(':has(.pov:hover)')) this.surface.classList.add('pov-idle'); }, 2500);
    };
    for (const ev of ['pointermove', 'pointerdown', 'keydown']) this.surface.addEventListener(ev, wake);
    this.surface.addEventListener('pointerleave', () => { if (!this.video.paused) this.surface.classList.add('pov-idle'); });
    for (const v of [video, this.alt]) { v.addEventListener('pause', wake); v.addEventListener('play', wake); }
  }
  // The camera's on-screen display: its name, and the clock showing the
  // time in the recording.
  buildOsd() {
    const c = status && status.cameras.find(x => x.id === this.item.camera);
    // (Sound and full screen go back in the bar meanwhile.)
    if (this.ptools && this.el) this.box.querySelector('.controls').appendChild(this.ptools);
    if (!c) { this.osd.innerHTML = ''; return; }
    this.osd.innerHTML = overlayHtml(c, null, false, {rec: true, tools: '<span class="p-toolslot"></span>', extra: 'pov-osd'});
    const slot = this.osd.querySelector('.p-toolslot');
    if (slot && this.ptools) slot.replaceWith(this.ptools);
    this.osd.querySelectorAll('.fs-go').forEach(b => b.onclick = (e) => { e.stopPropagation(); fullscreen(this.surface); });
  }
  wire(video) {
    watchFps(video);
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
      this.msg.textContent = video.error && video.error.code === 4 && this.tc ? 'No recorded footage here — the file may have been deleted, or the camera was offline.'
        : 'This recording could not be played. Use Download and play it in VLC.';
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
        <button type="button" class="p-back" title="Back one frame (pauses)">−1 frame</button>
        <button type="button" class="p-fwd" title="Forward one frame (pauses)">+1 frame</button>
        <select class="p-speed"><option value="0.5">0.5×</option><option value="1" selected>1×</option><option value="2">2×</option><option value="4">4×</option><option value="8">8×</option><option value="16">16×</option></select>
        <span class="timecode p-time"></span>
        <span class="spacer"></span>
        <button type="button" class="p-in" title="The clip starts here">${CLIP_IN}</button>
        <button type="button" class="p-out" title="The clip ends here">${CLIP_OUT}</button>
        <button type="button" class="p-clip" hidden>⇩ Download clip</button>
        <a class="btn p-dl" title="Download the whole file">⇩ File</a>
        <span class="vol snd-wrap p-tools"><button type="button" class="fsbtn p-mute" title="Mute / unmute">🔊</button><input type="range" class="snd-vol p-vol" min="0" max="1" step="0.05" value="1" title="Volume"></span>
      </div>`;
    const q = (s) => this.box.querySelector(s);
    this.el = {scrub: q('.scrub'), fill: q('.fill'), head: q('.head'), sel: q('.sel'), hin: q('.handle.in'), hout: q('.handle.out'),
      prev: q('.preview'), pimg: q('.preview .img'), pt: q('.preview .t'), time: q('.p-time'), play: q('.p-play'), clip: q('.p-clip')};
    q('.p-play').onclick = () => this.video.paused ? this.video.play() : this.video.pause();
    q('.p-back').onclick = () => this.stepFrame(-1);
    q('.p-fwd').onclick = () => this.stepFrame(1);
    q('.p-speed').onchange = (e) => this.video.playbackRate = +e.target.value;
    q('.p-in').onclick = () => { this.inT = this.cur(); if (this.outT != null && this.outT <= this.inT) this.outT = null; this.paint(); };
    q('.p-out').onclick = () => { this.outT = this.cur(); if (this.inT == null || this.inT >= this.outT) this.inT = Math.max(0, this.outT - 30); this.paint(); };
    const vol = q('.p-vol'), mute = q('.p-mute');
    const paintVol = () => { const m = this.video.muted || this.video.volume === 0; mute.textContent = m ? '🔇' : this.video.volume < 0.5 ? '🔉' : '🔊'; vol.value = this.video.muted ? 0 : this.video.volume; };
    const v0 = store.get('volume', 1);
    for (const v of [this.video, this.alt]) v.volume = v0;
    mute.onclick = () => { const m = !this.video.muted; for (const v of [this.video, this.alt]) v.muted = m; if (!m && this.video.volume === 0) this.video.volume = this.alt.volume = 0.5; paintVol(); };
    vol.oninput = () => { for (const v of [this.video, this.alt]) { v.volume = +vol.value; v.muted = +vol.value === 0; } store.set('volume', +vol.value); paintVol(); };
    paintVol();
    this.ptools = q('.p-tools');
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
      this.dropHls();
      // A recording: streamed in pieces (opens and jumps without reading
      // the file through, see hls.rs); otherwise the file itself.
      const hu = item.kind !== 'clip' && hlsReady() && /^media\/[^/]+\/(rec|tl)\//.test(item.url) && item.url.replace(/^media\//, 'api/hls-file/');
      if (hu) this.openHls(hu, item, start, autoplay);
      else this.openFile(item, start, autoplay);
    }
    this.dl.href = item.url + '?download=1';
    this.marks = item.marks || [];
    this.buildOsd();
    this.paint();
    this.lightSegs = [];
    this.findLightCopies(item);
    const kind = item.kind === 'clip' ? 'clip' : 'rec';
    if (TIMELAPSE) try { this.sprite = preloadSprite(await api(`api/sprite/${enc(item.camera)}/${kind}/${enc(item.name)}`)); } catch (e) { this.sprite = null; }
  }
  openFile(item, start, autoplay) {
    const v = this.video;
    v.src = item.url;
    v.playbackRate = +this.box.querySelector('.p-speed').value;
    const seek = () => { v.currentTime = start; if (autoplay) v.play().catch(() => {}); };
    if (v.readyState >= 1) seek(); else v.addEventListener('loadedmetadata', seek, {once: true});
  }
  openHls(url, item, start, autoplay) {
    const v = this.video;
    const h = this._hls = new hlsLib({autoStartLoad: false, maxBufferLength: 20, backBufferLength: 30, startFragPrefetch: true,
      manifestLoadingMaxRetry: 1, levelLoadingMaxRetry: 1, fragLoadingMaxRetry: 2});
    h.attachMedia(v);
    h.on(hlsLib.Events.MANIFEST_PARSED, () => {
      if (this._hls !== h) return;
      h.startLoad(start);
      v.currentTime = start; v.playbackRate = +this.box.querySelector('.p-speed').value;
      if (autoplay) v.play().catch(() => {});
    });
    h.on(hlsLib.Events.ERROR, (_, d) => {
      if (!d.fatal || this._hls !== h) return;
      // Can't be streamed in pieces here: the file itself.
      this.dropHls();
      if (this.item === item) this.openFile(item, start, autoplay);
    });
    h.loadSource(url);
  }
  dropHls() { if (this._hls) { const h = this._hls; this._hls = null; h.destroy(); } }
  // The light timeline copies around this file: their time-lapse has a
  // picture every second, shown full size while dragging along the bar.
  async findLightCopies(item) {
    const a = new Date(item.file_start).getTime();
    try {
      const [t] = await api(`api/timeline?cameras=${enc(item.camera)}&from=${enc(new Date(a - 60000).toISOString())}&to=${enc(new Date(a + 3 * 3600000).toISOString())}`);
      // The 24/7 recording's time-lapse (the blue line of the timelines),
      // whichever stream it is.
      if (this.item === item) this.lightSegs = previewSources((t && t.segments) || []);
    } catch (e) {}
  }
  // While dragging: the time-lapse picture of moment `t` over the whole
  // video (the full-quality video is loaded there on release).
  // The time-lapse picture of moment `t` of this file: from the 24/7
  // recording's time-lapse (already made, the one the timelines use), or
  // this file's own. Resolves to [sprite, seconds into it] or null.
  async tlapAt(t) {
    const W = this.wallAt(t).getTime();
    const seg = (this.lightSegs || []).find(x => segMs(x)._a <= W && W < (isGrowing(x) ? Date.now() : x._b) + 1000);
    const sp = seg && await spriteFor(this.item.camera, seg);
    if (sp) return [sp, (W - new Date(seg.start).getTime()) / 1000];
    return this.sprite ? [this.sprite, t] : null;
  }
  showTimelapse(t) {
    const ov = this.tlap; if (!ov || !this.item) return;
    const want = this.tlapWant = t;
    this.tlapAt(t).then(r => {
      if (!r || this.tlapWant !== want) return;
      if (spriteStyle(ov, r[0], Math.floor(r[1] / r[0].info.interval), this.surface.clientWidth, this.surface.clientHeight, false)) ov.hidden = false;
    });
  }
  hideTimelapse() {
    const ov = this.tlap; if (!ov || ov.hidden) return;
    this.tlapWant = null;
    const v = this.video, t0 = performance.now();
    const check = () => { if ((v.readyState >= 2 && !v.seeking) || performance.now() - t0 > 5000) ov.hidden = true; else setTimeout(check, 80); };
    setTimeout(check, 60);
  }
  // One frame back or forward, paused there.
  stepFrame(n) {
    if (!this.item) return;
    const v = this.video;
    v.pause();
    const t = Math.min(this.dur(), Math.max(0, this.cur() + n / videoFps(v)));
    if (this.tc) this.seekTo(t, false); else v.currentTime = t;
    this.paint();
  }
  stop() {
    this.dropHls();
    for (const v of [this.video, this.alt]) { v.pause(); v.removeAttribute('src'); v.load(); }
    this.item = null; this.nextSeg = null;
    if (this.tlap) this.tlap.hidden = true;
    if (this.osd) this.osd.innerHTML = '';
  }
  paintButtons() { if (this.el) this.el.play.textContent = this.video.paused ? '▶' : '❚❚'; }
  paint() {
    if (!this.el || !this.item) return;
    const d = this.dur() || 1, t = this.cur(), pct = (x) => Math.min(100, Math.max(0, x / d * 100)) + '%';
    this.el.fill.style.width = pct(t);
    this.el.head.style.left = pct(t);
    this.el.time.textContent = `${fmtTime(this.wallAt(t))} · ${fmtClock(t)} / ${fmtClock(d)}`;
    const clk = this.osd && this.osd.querySelector('.p-clock');
    if (clk) clk.textContent = fmtDate(this.wallAt(t), clk.dataset.fmt || undefined);
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
  // The time-lapse picture of that moment, as big as its detail allows: up
  // to the time-lapse picture's own size (Recording & storage → Time-lapse
  // pictures), and a third of the player's width.
  showPreview(t, x) {
    const p = this.el.prev;
    p.hidden = false;
    this.el.pt.textContent = fmtTime(this.wallAt(t)) + ' · ' + fmtClock(t);
    const want = this.prevWant = t;
    const place = (w) => { p.style.left = Math.min(Math.max(x, w / 2 + 4), this.el.scrub.clientWidth - w / 2 - 4) + 'px'; };
    place(this.prevW || 192);
    this.tlapAt(t).then(r => {
      if (!r || this.prevWant !== want) return;
      const i = r[0].info, a = i.aspect || i.tile_width / i.tile_height || 16 / 9;
      const w = Math.round(Math.max(192, Math.min(i.tile_width || 256, this.surface.clientWidth / 3, 480))), h = Math.round(w / a);
      Object.assign(this.el.pimg.style, {width: w + 'px', height: h + 'px'});
      this.prevW = w; place(w);
      spriteStyle(this.el.pimg, r[0], Math.floor(r[1] / i.interval), w, h);
    });
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
        // Time-lapse (the Recordings scrub choice): pictures only, the video
        // is loaded on release; otherwise seek the video itself.
        if ((TIMELAPSE && tl.scrub === 'timelapse') || this.tc) this.showTimelapse(t);
        else if (performance.now() - lastSeek > 120) { this.video.currentTime = t; lastSeek = performance.now(); }
        this.el.head.style.left = (t / this.dur() * 100) + '%';
      } else if (drag === 'in') { this.inT = Math.min(t, (this.outT ?? this.dur()) - 1); this.paint(); }
      else if (drag === 'out') { this.outT = Math.max(t, (this.inT ?? 0) + 1); this.paint(); }
    });
    sc.addEventListener('pointerleave', () => { if (!drag) this.el.prev.hidden = true; });
    sc.addEventListener('pointerdown', (e) => {
      if (!this.item) return;
      sc.setPointerCapture(e.pointerId);
      drag = e.target.classList.contains('in') ? 'in' : e.target.classList.contains('out') ? 'out' : 'head';
      if (drag === 'head') {
        this.wasPlaying = !this.video.paused; this.video.pause();
        if ((TIMELAPSE && tl.scrub === 'timelapse') || this.tc) this.showTimelapse(tAt(e)[0]); else this.video.currentTime = tAt(e)[0];
      }
    });
    const end = (e) => {
      if (!drag) return;
      if (drag === 'head') { this.seekTo(tAt(e)[0], this.wasPlaying); this.hideTimelapse(); }
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
      this.dropHls();  // the player left behind
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
      if (TIMELAPSE) api(`api/sprite/${enc(this.item.camera)}/${kind}/${enc(pre.name)}`).then(sp => { this.sprite = preloadSprite(sp); }).catch(() => { this.sprite = null; });
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

const phoneGrid = () => ($('dash').clientWidth || window.innerWidth) < 640;
// Columns on a phone: one full-width camera per row unless this phone was
// set to two (Edit layout on the phone).
const phoneCols = () => store.get('phone-cols', 1) === 2 ? 2 : 1;
function renderPhoneEditor() {
  const dash = $('dash'), all = viewCams();
  const order = layout.map(t => t.camera).filter(id => cam(id));
  const hidden = all.filter(c => !order.includes(c.id)).map(c => c.id);
  let cols = phoneCols();
  const ids = [...order, ...hidden], shown = new Set(order.length ? order : all.map(c => c.id));
  const draw = () => {
    dash.style.height = '';
    dash.innerHTML = `<div class="phone-edit card">
      <div class="row" style="justify-content:space-between"><b>Phone layout</b>
        <span class="seg" id="pe-cols"><button type="button" data-c="1" class="${cols === 1 ? 'on' : ''}">1 column</button><button type="button" data-c="2" class="${cols === 2 ? 'on' : ''}">2 columns</button></span></div>
      <div class="hint">Order the cameras and choose which to show on this phone. Saved for your account on phones only.</div>
      ${ids.map((id, i) => `<div class="pe-row ${shown.has(id) ? '' : 'off'}" data-id="${esc(id)}">
        <label class="check"><input type="checkbox" ${shown.has(id) ? 'checked' : ''}> ${esc((cam(id) || {}).name || id)}</label>
        <span><button type="button" class="small" data-mv="-1" ${i === 0 ? 'disabled' : ''} title="Move up">▲</button><button type="button" class="small" data-mv="1" ${i === ids.length - 1 ? 'disabled' : ''} title="Move down">▼</button></span></div>`).join('')}
    </div>`;
    dash.querySelectorAll('#pe-cols button').forEach(b => b.onclick = () => { cols = +b.dataset.c; store.set('phone-cols', cols); apply(); draw(); });
    dash.querySelectorAll('.pe-row').forEach(r => {
      const id = r.dataset.id;
      r.querySelector('input').onchange = (e) => { if (e.target.checked) shown.add(id); else shown.delete(id); apply(); draw(); };
      r.querySelectorAll('[data-mv]').forEach(b => b.onclick = () => {
        const i = ids.indexOf(id), j = i + +b.dataset.mv;
        [ids[i], ids[j]] = [ids[j], ids[i]]; apply(); draw();
      });
    });
  };
  // The layout that "Save layout" stores.
  const apply = () => {
    const w = 12 / cols;
    layout = ids.filter(id => shown.has(id)).map((id, i) => ({camera: id, x: (i % cols) * w, y: Math.floor(i / cols) * w, w, h: w}));
  };
  apply(); draw();
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
  if (!status.cameras.length && page === 'live') setTimeout(autoScanForCameras, 0);
  $('dash-bar').hidden = !status.cameras.length;
  dashIds = dashKey();
  if (!editing) layout = currentLayout();
  for (const u of unbinders.values()) u();
  unbinders.clear();
  dash.innerHTML = '';
  dash.classList.toggle('editing', editing);
  $('live-grid').classList.toggle('editing-wall', editing && wall);
  // Camera pictures always stretch to fill their box.
  dash.classList.add('fit-stretch');
  // Phones: a simple list to order and show/hide cameras instead of
  // dragging tiles around (which doesn't work well with a finger).
  if (editing && !wall && phoneGrid()) { renderPhoneEditor(); $('dash-info').textContent = ''; toggleEditButtons(); return; }
  // Phones show the cameras in order (top to bottom, left to right).
  if (!editing && phoneGrid()) layout = [...layout].sort((a, b) => a.y - b.y || a.x - b.x);
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
    unbinders.set(c.id, bindLive(img, c.id, q));
    offlineAfter(el.querySelector('.msg'), img);
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
// Before the first camera: scan the network right from the Live page.
$('nc-scan').onclick = () => scanNetwork({btn: 'nc-scan', out: 'nc-out', nets: 'nc-nets'});
// The first time the page shows no cameras, look for them straight away.
let ncScanned = false;
function autoScanForCameras() {
  if (ncScanned || $('no-cameras').hidden || !me || me.role === 'viewer') return;
  ncScanned = true;
  scanNetwork({btn: 'nc-scan', out: 'nc-out', nets: 'nc-nets'});
}
$('nc-nets').value = store.get('scan-nets', '');
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
      // Phones: one camera across the whole width of the screen, the rest
      // stacked below (or two side by side, if chosen in the phone layout).
      const cols = phoneCols(), gap = 6;
      const i = [...dash.querySelectorAll('.tile')].indexOf(el), cw = (m.W - gap * (cols - 1)) / cols, h = cw * 9 / 16;
      const top = Math.floor(i / cols) * (h + gap);
      Object.assign(el.style, {left: (i % cols) * (cw + gap) + 'px', top: top + 'px', width: cw + 'px', height: h + 'px'});
      maxB = Math.max(maxB, top + h);
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
  await api('api/dashboard?device=' + deviceKind(), {method: 'PUT', json: d});
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
// ---------- ready-made layouts ----------
// Each makes the spots for n cameras on the 12-column grid (a w × w tile
// is a 16:9 picture), big spots first; cameras beyond a layout's spots
// carry on in rows of small pictures underneath.
const SLOT = (x, y, w, h) => ({x, y, w, h});
const rowsOf = (y0, n, w) => Array.from({length: Math.max(0, n)}, (_, i) => SLOT((i % (12 / w)) * w, y0 + Math.floor(i / (12 / w)) * w, w, w));
const LAYOUTS = [
  {id: 'auto', name: 'Equal grid (best fit)', big: 0, make: (n) => { const c = n <= 1 ? 1 : n <= 4 ? 2 : n <= 9 ? 3 : n <= 16 ? 4 : 6; return rowsOf(0, n, 12 / c); }},
  {id: 'g2', name: '2 across', big: 0, make: (n) => rowsOf(0, n, 6)},
  {id: 'g3', name: '3 across', big: 0, make: (n) => rowsOf(0, n, 4)},
  {id: 'g4', name: '4 across', big: 0, make: (n) => rowsOf(0, n, 3)},
  {id: 'g6', name: '6 across', big: 0, make: (n) => rowsOf(0, n, 2)},
  {id: '1+5', name: '1 big + 5 around', big: 1, make: (n) => [SLOT(0, 0, 8, 8), SLOT(8, 0, 4, 4), SLOT(8, 4, 4, 4), SLOT(0, 8, 4, 4), SLOT(4, 8, 4, 4), SLOT(8, 8, 4, 4), ...rowsOf(12, n - 6, 4)]},
  {id: '1+7', name: '1 big + 7 around', big: 1, make: (n) => [SLOT(0, 0, 9, 9), SLOT(9, 0, 3, 3), SLOT(9, 3, 3, 3), SLOT(9, 6, 3, 3), SLOT(0, 9, 3, 3), SLOT(3, 9, 3, 3), SLOT(6, 9, 3, 3), SLOT(9, 9, 3, 3), ...rowsOf(12, n - 8, 3)]},
  {id: '1c12', name: '1 big in the middle + 12 around', big: 1, make: (n) => [SLOT(3, 3, 6, 6),
    ...[0, 3, 6, 9].map(x => SLOT(x, 0, 3, 3)), SLOT(0, 3, 3, 3), SLOT(9, 3, 3, 3), SLOT(0, 6, 3, 3), SLOT(9, 6, 3, 3), ...[0, 3, 6, 9].map(x => SLOT(x, 9, 3, 3)), ...rowsOf(12, n - 13, 3)]},
  {id: '1+2', name: '1 big + 2 beside', big: 1, make: (n) => [SLOT(0, 0, 8, 8), SLOT(8, 0, 4, 4), SLOT(8, 4, 4, 4), ...rowsOf(8, n - 3, 4)]},
  {id: '1top', name: '1 big + strip below', big: 1, make: (n) => [SLOT(0, 0, 12, 12), ...rowsOf(12, n - 1, 3)]},
  {id: '1+2m', name: '1 big + 2 medium + small', big: 3, make: (n) => [SLOT(0, 0, 8, 8), SLOT(8, 0, 4, 4), SLOT(8, 4, 4, 4), ...rowsOf(8, n - 3, 3)]},
  {id: '2+', name: '2 big + small below', big: 2, make: (n) => [SLOT(0, 0, 6, 6), SLOT(6, 0, 6, 6), ...rowsOf(6, n - 2, 3)]},
  {id: '2col', name: '2 big stacked + column', big: 2, make: (n) => [SLOT(0, 0, 8, 8), SLOT(0, 8, 8, 8), SLOT(8, 0, 4, 4), SLOT(8, 4, 4, 4), SLOT(8, 8, 4, 4), SLOT(8, 12, 4, 4), ...rowsOf(16, n - 6, 4)]},
  {id: '3+', name: '3 big + small below', big: 3, make: (n) => [SLOT(0, 0, 4, 4), SLOT(4, 0, 4, 4), SLOT(8, 0, 4, 4), ...rowsOf(4, n - 3, 3)]},
  {id: '4+', name: '4 big + small below', big: 4, make: (n) => [SLOT(0, 0, 6, 6), SLOT(6, 0, 6, 6), SLOT(0, 6, 6, 6), SLOT(6, 6, 6, 6), ...rowsOf(12, n - 4, 3)]},
];
// The cameras in the order they fill a layout's spots: the biggest
// pictures now first (so a camera you made big stays big), then top to
// bottom, left to right.
function layoutOrder() {
  const cams = viewCams(), cur = currentLayout();
  const at = (id) => cur.find(t => t.camera === id);
  return [...cams].sort((a, b) => { const p = at(a.id), q = at(b.id); if (!p || !q) return !p - !q; return q.w * q.h - p.w * p.h || p.y - q.y || p.x - q.x; });
}
// The layout's whole shape: its own spots (dashed where a camera would be
// missing) and the rows your other cameras go in.
function layoutPreview(L, n) {
  const own = L.make(0).length, spots = L.make(Math.max(n, 1)).slice(0, Math.max(n, own, 1)), H = Math.max(...spots.map(s => s.y + s.h));
  return `<svg viewBox="-0.3 -0.3 12.6 ${H + 0.6}" preserveAspectRatio="xMidYMin meet">${spots.map((s, i) => `<rect x="${s.x + 0.15}" y="${s.y + 0.15}" width="${s.w - 0.3}" height="${s.h - 0.3}" rx="0.3" class="${i >= n ? 'empty' : i < L.big ? 'big' : ''}"/>`).join('')}</svg>`;
}
$('dash-layouts').onclick = () => {
  const cams = viewCams(), n = cams.length;
  if (!n) return;
  const order = layoutOrder();
  const dlg = document.createElement('dialog'); dlg.className = 'wn-dlg lay-dlg';
  dlg.innerHTML = `<div class="head"><h2>▦ Layouts</h2><span class="spacer"></span><button type="button" class="small" data-x>✕</button></div>
    <div class="body"><p class="muted" style="margin-top:0">Shown with your ${n} camera${n === 1 ? '' : 's'}${group() ? ` in “${esc(group().name)}”` : ''}${wall ? ', for the full-screen view' : ''}. Pick one, choose which cameras get the big spots, and it's saved — use Edit layout afterwards to fine-tune.</p>
      <div class="lay-grid">${LAYOUTS.map(L => `<button type="button" class="lay" data-id="${L.id}">${layoutPreview(L, n)}<span>${esc(L.name)}${L.make(0).length > n ? `<br><small class="muted">${L.make(0).length - n} spot${L.make(0).length - n === 1 ? '' : 's'} left empty</small>` : ''}</span></button>`).join('')}</div>
      <div id="lay-big" class="lay-big" hidden></div>
      <div class="actions"><button type="button" data-x>Cancel</button><button type="button" class="primary" id="lay-go" disabled>Use this layout</button></div></div>`;
  document.body.appendChild(dlg); dlg.showModal();
  const close = () => { dlg.close(); dlg.remove(); };
  dlg.querySelectorAll('[data-x]').forEach(b => b.onclick = close);
  let pick = null;
  const camOpts = (sel) => order.map(c => `<option value="${esc(c.id)}" ${c.id === sel ? 'selected' : ''}>${esc(c.name)}</option>`).join('');
  dlg.querySelectorAll('.lay').forEach(b => b.onclick = () => {
    pick = LAYOUTS.find(L => L.id === b.dataset.id);
    dlg.querySelectorAll('.lay').forEach(x => x.classList.toggle('on', x === b));
    const big = Math.min(pick.big, n), box = $('lay-big');
    box.hidden = !big;
    box.innerHTML = big ? `<b>Big spot${big === 1 ? '' : 's'}:</b> ${Array.from({length: big}, (_, i) => `<select data-i="${i}">${camOpts(order[i].id)}</select>`).join('')}` : '';
    $('lay-go').disabled = false;
  });
  $('lay-go').onclick = async () => {
    if (!pick) return;
    // The chosen cameras first, the rest in their usual order.
    const chosen = [...dlg.querySelectorAll('#lay-big select')].map(s => s.value).filter((v, i, a) => a.indexOf(v) === i);
    const seq = [...chosen.map(id => order.find(c => c.id === id)), ...order.filter(c => !chosen.includes(c.id))];
    const spots = pick.make(n);
    layout = seq.map((c, i) => ({camera: c.id, ...spots[i]}));
    close();
    if (editing) { renderDash(); return; }
    const d = JSON.parse(JSON.stringify(status.dashboard));
    const owner = groupId ? d.groups.find(g => g.id === groupId) : d;
    owner[layoutKey()] = layout.map(t => ({...t}));
    try { await saveDashboard(d); renderDash(); updateSubscriptions(); toast(`Layout saved: ${pick.name}`); } catch (e) { toast(e.message, true); }
  };
};

function toggleEditButtons() {
  $('dash-edit').hidden = editing || !!wallSolo;
  for (const id of ['dash-auto', 'dash-cancel', 'dash-save']) $(id).hidden = !editing;
  $('dash-layouts').hidden = !!wallSolo;
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
  // Instant: save the picture on screen (the live full-quality picture this
  // browser already has); the server snapshot only if there isn't one yet.
  $('focus-snap').onclick = (e) => {
    const url = Live.urls.get(Live.key(c.id, 'main')) || Live.urls.get(Live.key(c.id, 'sub'));
    if (!url) return; // server snapshot (the link)
    e.preventDefault();
    const d = new Date(), p2 = (n) => String(n).padStart(2, '0');
    const a = document.createElement('a');
    a.href = url;
    a.download = `${c.id}_${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())}_${p2(d.getHours())}-${p2(d.getMinutes())}-${p2(d.getSeconds())}.jpg`;
    document.body.appendChild(a); a.click(); a.remove();
    toast('Snapshot saved');
  };
  setupTalkButton(c);
  setupSayButton(c);
  setupSirenButton(c);
  $('focus-recs').href = '#recordings/' + enc(c.id) + '/timeline';
  $('focus-edit').onclick = () => openCameraDialog(c.id);
  paintSnooze(c);
  $('focus-snooze').onchange = async () => {
    const m = +val('focus-snooze');
    try { const r = await api(`api/cameras/${enc(c.id)}/snooze`, {method: 'POST', json: {minutes: m}});
      toast(m ? `🔕 ${c.name}: no notifications until ${fmtTime(r.until)}` : `🔔 ${c.name}: notifications on again`); await refreshStatus(); paintSnooze(cam(c.id)); }
    catch (e) { toast(e.message, true); }
  };
  $('focus-title').textContent = c.name;
  if (soundCam && soundCam !== c.id) stopSound();
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
  focusUnbind = bindLive(img, c.id, q);
  offlineAfter($('focus-msg'), img);
  // Stream 2 is often another shape (4:3): fill the box like stream 1.
  img.classList.toggle('fill', q === 'sub');
  if (img.getAttribute('src')) $('focus-msg').hidden = true;
  // Spinner until the first full-quality frame arrives (the sub-stream
  // picture is shown meanwhile).
  if (q === 'main') showFocusLoading(); else hideFocusLoading();
}
// Is the camera's full-quality picture already coming in (smooth video
// playing, or pictures in the last few seconds)?
function mainReady(id) {
  const k = Live.key(id, 'main'), st = Live.stats.get(k), vs = Live.vstats.get(k), now = performance.now();
  return (Live.mse.has(k) && vs && now - vs.last < 3000) || (st && now - st.last < 3000);
}
let loadingPoll = null;
function showFocusLoading() {
  const el = $('focus-loading'), id = focusId;
  clearTimeout(loadingTimer); clearInterval(loadingPoll);
  if (mainReady(id)) { el.hidden = true; return; }
  el.hidden = false;
  el.querySelector('span').textContent = 'Loading full quality…';
  // Also catches a picture that was already playing (no "first frame" then).
  loadingPoll = setInterval(() => { if (focusId !== id || mainReady(id)) hideFocusLoading(); }, 400);
  loadingTimer = setTimeout(() => { clearInterval(loadingPoll); el.querySelector('span').textContent = 'Full-quality stream not available — showing sub-stream'; setTimeout(() => el.hidden = true, 5000); }, 20000);
}
function hideFocusLoading() { clearTimeout(loadingTimer); clearInterval(loadingPoll); $('focus-loading').hidden = true; }

// ---------- PTZ controls ----------
// Pan / tilt / zoom on the picture: the pad and zoom buttons (press and
// hold), dragging on the picture (direction and speed follow the drag),
// the mouse wheel to zoom, and the arrow / + / - keys.
let ptzCam = null;
function setupPtz(c) {
  const pad = $('ptz-pad'), surf = $('focus-surface');
  ptzCam = c.ptz ? c.id : null;
  ptzPresets = []; ptzGoto = null;
  pad.hidden = !c.ptz;
  surf.classList.toggle('has-ptz', !!c.ptz);
  if (!c.ptz) return;
  const id = c.id, msg = (t, bad) => { const m = $('ptz-msg'); m.textContent = t; m.classList.toggle('err', !!bad); clearTimeout(m._t); if (!bad) m._t = setTimeout(() => { m.textContent = ''; }, 3000); };
  const move = (v) => api(`api/cameras/${enc(id)}/ptz/move`, {method: 'POST', json: {pan: v[0], tilt: v[1], zoom: v[2]}}).catch(e => msg(e.message, true));
  const stop = () => api(`api/cameras/${enc(id)}/ptz/stop`, {method: 'POST'}).catch(() => {});
  ptzMove = move; ptzStop = stop;
  pad.querySelectorAll('[data-ptz]').forEach(b => {
    const v = b.dataset.ptz.split(',').map(Number);
    let held = false;
    b.onpointerdown = (e) => { e.preventDefault(); e.stopPropagation(); held = true; b.setPointerCapture(e.pointerId); move(v); };
    b.onpointerup = b.onpointercancel = b.onlostpointercapture = () => { if (held) { held = false; stop(); } };
    b.ondblclick = (e) => e.stopPropagation();
  });
  pad.querySelector('[data-ptz-home]').onclick = stop;
  pad.onpointerdown = (e) => e.stopPropagation();
  pad.ondblclick = (e) => e.stopPropagation();
  const loadPresets = async () => {
    try {
      const p = await api(`api/cameras/${enc(id)}/ptz/presets`);
      if (ptzCam !== id) return;
      ptzPresets = p;
      // Numbered: keys 1–9 (and 0 for the tenth) go to them.
      $('ptz-presets').innerHTML = `<option value="">Presets (${p.length})…</option>` + p.map((x, i) => `<option value="${esc(x.token)}">${i < 10 ? ((i + 1) % 10) + ' · ' : ''}${esc(x.name)}</option>`).join('');
    } catch (e) { msg(e.message, true); }
  };
  ptzGoto = (t) => api(`api/cameras/${enc(id)}/ptz/goto`, {method: 'POST', json: {token: t}}).catch(e => msg(e.message, true));
  $('ptz-presets').onchange = () => { const t = $('ptz-presets').value; if (t) ptzGoto(t); };
  // Roaming and tracking: Off / Roam / Track / Roam + track.
  const box = $('ptz-roam-box');
  box.hidden = true;
  box.onpointerdown = (e) => e.stopPropagation(); box.ondblclick = (e) => e.stopPropagation();
  const MODES = {off: 'Off', roam: 'Roam', track: 'Track', both: 'Roam + track'};
  const modeOf = (r) => r.roam && r.follow ? 'both' : r.roam ? 'roam' : r.follow ? 'track' : 'off';
  const paintMode = (m) => { const b = $('ptz-roam'); b.querySelector('span').textContent = MODES[m]; b.classList.toggle('on', m !== 'off'); };
  let mode = 'off';
  const pick = (m) => {
    mode = m;
    $('ptz-mode-seg').querySelectorAll('button').forEach(b => b.classList.toggle('on', b.dataset.m === m));
    const roams = m === 'roam' || m === 'both';
    $('ptz-roam-list').hidden = !roams; box.querySelector('.ptz-stay').hidden = !roams;
    $('ptz-mode-hint').textContent = {off: 'The camera stays where it is.', roam: 'Visits the ticked presets in turn.',
      track: 'Turns to follow a person the AI finds, then goes back.', both: 'Roams; when the AI finds a person it stops and follows them, then carries on roaming.'}[m]
      + (m !== 'off' ? ' Pauses 2 minutes after you steer it.' : '');
  };
  $('ptz-mode-seg').querySelectorAll('button').forEach(b => b.onclick = () => pick(b.dataset.m));
  api(`api/cameras/${enc(id)}/ptz/patrol`).then(r => paintMode(modeOf(r))).catch(() => {});
  $('ptz-roam').onclick = async (e) => {
    e.stopPropagation();
    if (!box.hidden) { box.hidden = true; return; }
    let cur = {patrol: [], patrol_seconds: 30, roam: false, follow: false};
    try { cur = await api(`api/cameras/${enc(id)}/ptz/patrol`); } catch (_) {}
    $('ptz-roam-s').value = cur.patrol_seconds ?? 30;
    $('ptz-roam-list').innerHTML = ptzPresets.length ? ptzPresets.map((p, i) => `<label class="check"><input type="checkbox" value="${esc(p.token)}" ${!cur.patrol.length || cur.patrol.includes(p.token) ? 'checked' : ''}> ${i < 10 ? ((i + 1) % 10) + ' · ' : ''}${esc(p.name)}</label>`).join('')
      : '<span class="muted small">No presets yet: save some with ☆ first.</span>';
    pick(modeOf(cur));
    box.hidden = false;
  };
  $('ptz-roam-cancel').onclick = () => { box.hidden = true; };
  $('ptz-roam-save').onclick = async () => {
    const list = [...$('ptz-roam-list').querySelectorAll('input:checked')].map(i => i.value);
    const roam = mode === 'roam' || mode === 'both', follow = mode === 'track' || mode === 'both';
    if (roam && list.length < 2) return msg('Tick at least two presets to roam between', true);
    try {
      await api(`api/cameras/${enc(id)}/ptz/patrol`, {method: 'PUT', json: {patrol: list, patrol_seconds: Math.max(0, Math.round(+$('ptz-roam-s').value) || 0), roam, follow}});
      paintMode(mode); box.hidden = true; msg('');
    } catch (e) { msg(e.message, true); }
  };
  $('ptz-del').onclick = async () => {
    const t = $('ptz-presets').value, p = ptzPresets.find(x => x.token === t);
    if (!p) return msg('Choose the preset to delete first', true);
    if (!confirm(`Delete the preset “${p.name}”?`)) return;
    try { await api(`api/cameras/${enc(id)}/ptz/presets/${enc(t)}`, {method: 'DELETE'}); loadPresets(); msg(''); }
    catch (e) { msg(e.message, true); }
  };
  placePtzPad(id);
  $('ptz-save').onclick = async () => {
    const name = (prompt('Name for this position (e.g. Driveway, Gate):') || '').trim();
    if (!name) return;
    try { await api(`api/cameras/${enc(id)}/ptz/presets`, {method: 'POST', json: {name}}); toast('Preset saved'); loadPresets(); }
    catch (e) { msg(e.message, true); }
  };
  loadPresets();
}
let ptzMove = null, ptzStop = null, ptzGoto = null, ptzPresets = [];
// The PTZ controls can be dragged anywhere on the picture (by the grip on
// top); where is remembered per camera, on this device.
function placePtzPad(id) {
  const pad = $('ptz-pad'), surf = $('focus-surface');
  const pos = store.get('ptz-pos', {})[id];
  const apply = () => {
    if (!pos) { pad.style.left = pad.style.top = ''; pad.style.right = pad.style.bottom = ''; return; }
    const W = surf.clientWidth - pad.offsetWidth, H = surf.clientHeight - pad.offsetHeight;
    Object.assign(pad.style, {left: Math.max(0, pos.x * W) + 'px', top: Math.max(0, pos.y * H) + 'px', right: 'auto', bottom: 'auto'});
  };
  apply(); pad._place = apply;
  const grip = $('ptz-grip');
  grip.onpointerdown = (e) => {
    e.preventDefault(); e.stopPropagation(); grip.setPointerCapture(e.pointerId);
    const r0 = pad.getBoundingClientRect(), s0 = surf.getBoundingClientRect(), dx = e.clientX - r0.left, dy = e.clientY - r0.top;
    pad.classList.add('dragging');
    grip.onpointermove = (ev) => {
      const W = s0.width - r0.width, H = s0.height - r0.height;
      const x = Math.min(W, Math.max(0, ev.clientX - s0.left - dx)), y = Math.min(H, Math.max(0, ev.clientY - s0.top - dy));
      Object.assign(pad.style, {left: x + 'px', top: y + 'px', right: 'auto', bottom: 'auto'});
    };
    grip.onpointerup = grip.onpointercancel = () => {
      grip.onpointermove = grip.onpointerup = grip.onpointercancel = null; pad.classList.remove('dragging');
      const W = surf.clientWidth - pad.offsetWidth, H = surf.clientHeight - pad.offsetHeight;
      const all = store.get('ptz-pos', {});
      all[id] = {x: W > 0 ? pad.offsetLeft / W : 1, y: H > 0 ? pad.offsetTop / H : 1};
      store.set('ptz-pos', all);
      placePtzPad(id);
    };
  };
  grip.ondblclick = (e) => { e.stopPropagation(); const all = store.get('ptz-pos', {}); delete all[id]; store.set('ptz-pos', all); placePtzPad(id); };
}
window.addEventListener('resize', () => { const p = $('ptz-pad'); if (p && p._place) p._place(); });
document.addEventListener('fullscreenchange', () => setTimeout(() => { const p = $('ptz-pad'); if (p && p._place) p._place(); }, 100));
// Drag on the picture to pan / tilt.
(function wirePtzDrag() {
  const surf = $('focus-surface');
  let start = null, last = '', moving = false, t = 0;
  surf.addEventListener('pointerdown', (e) => {
    if (!ptzCam || focusPlayback || e.button !== 0 || e.target.closest('button, select, .ov')) return;
    start = {x: e.clientX, y: e.clientY}; last = ''; moving = false;
  });
  surf.addEventListener('pointermove', (e) => {
    if (!start || !ptzMove) return;
    const dx = e.clientX - start.x, dy = e.clientY - start.y, d = Math.hypot(dx, dy);
    if (d < 18) return;
    if (!moving) { moving = true; surf.setPointerCapture(e.pointerId); surf.classList.add('ptz-dragging'); }
    // Speed grows with the drag distance; snap tiny sideways components away.
    const k = 0.35 + 0.65 * Math.min(1, (d - 18) / 150), px = Math.abs(dx) / d > 0.3 ? Math.sign(dx) * k : 0, py = Math.abs(dy) / d > 0.3 ? -Math.sign(dy) * k : 0;
    const key = px.toFixed(1) + ',' + py.toFixed(1);
    if (key !== last && performance.now() - t > 150) { last = key; t = performance.now(); ptzMove([px, py, 0]); }
  });
  const end = () => { if (moving && ptzStop) ptzStop(); start = null; moving = false; surf.classList.remove('ptz-dragging'); };
  surf.addEventListener('pointerup', end); surf.addEventListener('pointercancel', end);
  // Mouse wheel: a short zoom burst per notch.
  let zt = null;
  surf.addEventListener('wheel', (e) => {
    if (!ptzCam || focusPlayback || !ptzMove) return;
    e.preventDefault();
    ptzMove([0, 0, e.deltaY < 0 ? 0.6 : -0.6]);
    clearTimeout(zt); zt = setTimeout(() => ptzStop && ptzStop(), 350);
  }, {passive: false});
  // Arrow keys and + / - while the camera view is open.
  const held = new Set();
  const keyVec = {ArrowUp: [0, 1, 0], ArrowDown: [0, -1, 0], ArrowLeft: [-1, 0, 0], ArrowRight: [1, 0, 0], '+': [0, 0, 1], '=': [0, 0, 1], '-': [0, 0, -1]};
  document.addEventListener('keydown', (e) => {
    if (!ptzCam || page !== 'live' || focusId !== ptzCam || focusPlayback || e.target.closest('input, select, textarea, dialog')) return;
    // 1–9 (and 0): go to that preset.
    if (/^[0-9]$/.test(e.key) && !e.ctrlKey && !e.altKey && !e.metaKey && ptzGoto) {
      const p = ptzPresets[(+e.key + 9) % 10];
      if (p) { e.preventDefault(); ptzGoto(p.token); $('ptz-presets').value = p.token; }
      return;
    }
    if (!keyVec[e.key] || held.has(e.key)) return;
    e.preventDefault(); held.add(e.key); ptzMove(keyVec[e.key]);
  });
  document.addEventListener('keyup', (e) => { if (held.delete(e.key) && ptzStop) ptzStop(); });
})();

// ---------- per-camera timeline (camera view) ----------
// Works like the Recordings timeline: drag to run through the time-lapse on
// the camera's picture, let go to play from there; a strip shows which
// parts of the time-lapse are loaded.
// Time marks for a window: labels, small ticks between, and grid lines.
// The time scale shared by every timeline: labels and ticks, faint bands
// for every other interval, a line at midnight and the "now" line.
function timeMarks(a, span, pct) {
  const steps = [60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400].map(x => x * 1000);
  const step = steps.find(x => span / x <= 12) || 86400000;
  const minor = step / ([120000, 600000, 1800000, 7200000].includes(step) ? 4 : [10800000, 43200000].includes(step) ? 3 : step === 86400000 ? 4 : 5);
  let axis = '', grid = '';
  // Bands: every other major interval slightly lighter.
  for (let t = Math.floor(a / step) * step; t < a + span; t += step) {
    if (Math.round(t / step) % 2) continue;
    const x0 = Math.max(0, pct(t)), x1 = Math.min(100, pct(t + step));
    if (x1 > x0) grid += `<b class="band" style="left:${x0}%;width:${x1 - x0}%"></b>`;
  }
  for (let t = Math.ceil(a / minor) * minor; t <= a + span; t += minor) {
    const d = new Date(t), midnight = d.getHours() === 0 && d.getMinutes() === 0;
    const major = Math.abs(t % step) < 1 || (step >= 86400000 && midnight);
    if (!major) { axis += `<i class="mn" style="left:${pct(t)}%"></i>`; continue; }
    const hm = String(d.getHours()).padStart(2, '0') + ':' + String(d.getMinutes()).padStart(2, '0');
    const cls = midnight ? 'day' : d.getMinutes() === 0 ? 'hr' : '';
    // Labels near either end stay inside the timeline.
    const x = pct(t), shift = x < 6 ? 'translateX(0)' : x > 94 ? 'translateX(-100%)' : '';
    axis += `<i class="mj" style="left:${x}%"></i><span class="${cls}" style="left:${x}%;${shift ? 'transform:' + shift : ''}">${step >= 86400000 ? fmtDay(d) : hm + (midnight ? ' ' + fmtDay(d) : '')}</span>`;
    grid += `<i class="${midnight ? 'day' : ''}" style="left:${pct(t)}%"></i>`;
  }
  // "Now": one small mark on the scale (the rows show it by the recording
  // bar that's still growing).
  const nowP = pct(Date.now());
  if (nowP >= 0 && nowP <= 100) axis += `<i class="now-ax ${nowP > 92 ? 'end' : ''}" style="left:${nowP}%" title="Now"><em>NOW</em></i>`;
  return {axis, grid};
}
// Hide time-scale labels that would run into each other (dates win over
// whole hours, whole hours over the rest).
function declutterAxis(axis) {
  if (!axis) return;
  const rank = (el) => el.classList.contains('day') ? 2 : el.classList.contains('hr') ? 1 : 0;
  let prev = null;
  for (const el of [...axis.querySelectorAll('span')].sort((a, b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left)) {
    el.style.visibility = '';
    if (prev && el.getBoundingClientRect().left < prev.getBoundingClientRect().right + 8) {
      if (rank(el) > rank(prev)) { prev.style.visibility = 'hidden'; prev = el; } else el.style.visibility = 'hidden';
    } else prev = el;
  }
}
// What kind of alert, for its colour on the timeline.
function evCat(e) {
  if (!e.labels.length) return 'motion';
  const l = e.labels[0].label;
  return l === 'package' ? 'package' : (LABEL_GROUP[l] || 'other');
}
// Alert markers for a timeline lane: a thin bar in the colour of the kind
// of alert for how long it lasted, and above the recording bar the alert's
// picture. Pictures too close to fit side by side are stacked into one with
// a count; clicking it shows them all to choose from. `groups` collects the
// alerts of each picture (for the click).
function evMarks(events, pct, attrs = '', cam = '', lanePx = 800, groups = []) {
  let out = '';
  for (const e of events) {
    const a = pct(new Date(e.start).getTime()), b = pct(new Date(e.end).getTime());
    if (b < 0 || a > 100) continue;
    const x = Math.max(0, a);
    const tip = e.labels.length ? e.labels.map(l => `${l.label} ${Math.round(l.score * 100)}%`).join(', ') : 'motion';
    out += `<div class="ev cat-${evCat(e)}" style="left:${x}%;width:${Math.max(0.25, Math.min(100, b) - x)}%" title="${esc(tip)} · ${fmtTime(e.start)} — click to watch" data-t="${new Date(e.start).getTime()}" ${attrs}></div>`;
  }
  // Pictures of AI alerts, grouped where they'd overlap (a picture is ~50 px).
  const step = 52 / Math.max(200, lanePx) * 100;
  let group = null;
  const flush = () => {
    if (!group) return;
    const e0 = group.evs[0], x = group.x, n = group.evs.length, gi = groups.push({cam, evs: group.evs}) - 1;
    const shift = x > 96 ? 'transform:translateX(-100%)' : '';
    out += `<div class="evt cat-${evCat(e0)}" data-g="${gi}" style="left:${x}%;${shift}" title="${n > 1 ? n + ' alerts — click to see them all' : esc(e0.labels.map(l => l.label).join(', ')) + ' · ' + fmtTime(e0.start) + ' — click to watch'}">`
      + `<img loading="lazy" src="media/${enc(cam)}/events/${enc(e0.thumb)}" alt="">${n > 1 ? `<b class="n">${n}</b>` : `<span class="ic">${labelIcon(e0.labels[0].label)}</span>`}</div>`;
    group = null;
  };
  for (const e of events.filter(e => e.labels.length && e.thumb).sort((p, q) => new Date(p.start) - new Date(q.start))) {
    const x = pct(new Date(e.start).getTime());
    if (x < 0 || x > 100) continue;
    if (group && x - group.x < step) { group.evs.push(e); continue; }
    flush();
    group = {x, evs: [e]};
  }
  flush();
  return out;
}
// The alert pictures of one stacked marker, to choose from.
function openAlertPicker(anchor, g, pick) {
  document.querySelectorAll('.evt-pop').forEach(x => x.remove());
  const pop = document.createElement('div');
  pop.className = 'evt-pop';
  pop.innerHTML = g.evs.map((e, i) => `<button type="button" class="it cat-${evCat(e)}" data-i="${i}"><img src="media/${enc(g.cam)}/events/${enc(e.thumb)}" alt="">
    <span class="cap">${e.labels.map(l => labelIcon(l.label)).join('')} ${esc(e.labels.map(l => l.label).join(', '))}<b>${fmtTime(e.start)}</b></span></button>`).join('');
  // In full screen only what's inside the full-screen picture shows.
  (document.fullscreenElement || document.body).appendChild(pop);
  const r = anchor.getBoundingClientRect(), pr = pop.getBoundingClientRect();
  pop.style.left = Math.max(8, Math.min(innerWidth - pr.width - 8, r.left + r.width / 2 - pr.width / 2)) + 'px';
  pop.style.top = (r.bottom + pr.height + 8 > innerHeight ? Math.max(8, r.top - pr.height - 6) : r.bottom + 6) + 'px';
  const close = (ev) => { if (ev && pop.contains(ev.target)) return; pop.remove(); document.removeEventListener('pointerdown', close, true); document.removeEventListener('keydown', esc1); };
  const esc1 = (ev) => { if (ev.key === 'Escape') close(); };
  setTimeout(() => { document.addEventListener('pointerdown', close, true); document.addEventListener('keydown', esc1); });
  pop.querySelectorAll('.it').forEach(b => b.onclick = () => { close(); pick(g.evs[+b.dataset.i]); });
}
const camTl = {
  id: null, data: null, zoom: 21600, end: null, loadedAt: 0, pv: new Map(), pfGen: 0,
  open(id) {
    hlsReady();  // for playing recordings (see Player.openHls)
    this.id = id; this.zoom = +$('ctl-zoom').value; this.end = null; this.pv = new Map();
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
      this.draw(); this.prefetch();
    } catch (e) {}
  },
  // The files the time-lapse comes from (the light stream 2 copies when kept).
  sources() { return previewSources((this.data && this.data.segments) || []); },
  draw() {
    if (!this.data || this.dragging) return;
    const [a, b] = this.win(), span = b - a, pct = (t) => (t - a) / span * 100;
    const {axis, grid} = timeMarks(a, span, pct);
    $('ctl-axis').innerHTML = axis;
    declutterAxis($('ctl-axis'));
    let lane = `<div class="tl-grid">${grid}</div>`;
    const lastRec = this.data.segments.filter(x => x.kind !== 'clip').slice(-1)[0], pieces = [];
    for (const sg of this.data.segments) {
      const live = sg === lastRec && isGrowing(sg);
      const x0 = Math.max(0, pct(new Date(sg.start).getTime())), x1 = Math.min(100, pct(live ? Date.now() : new Date(sg.end).getTime()));
      if (x1 > 0 && x0 < 100 && x1 > x0) pieces.push({a: x0, b: x1, cls: streamOf(sg) === 'main' ? 'clip' : 'rec', live});
    }
    lane += solidBars(pieces, 15000 / (b - a) * 100, 'div', 'seg');
    this.groups = [];
    lane += evMarks(this.data.events, pct, '', this.id, $('ctl-lane').clientWidth, this.groups);
    const lb = $('ctl-lane'), pv = lb.querySelector('.preview');
    lb.innerHTML = lane + '<div class="head" hidden><em></em></div><div class="hover-line" hidden></div>';
    lb.appendChild(pv);
    $('cam-tl').querySelector('.legend').innerHTML = legendHtml('cam', [{...this.data, camera: this.id}]);
    this.paintStrip(); this.paintHead();
  },
  // Which parts of the time-lapse are ready to scrub.
  paintStrip() {
    const el = $('ctl-tlp'); if (!el || !this.data) return;
    const [a, b] = this.win(), span = b - a, pct = (t) => (t - a) / span * 100;
    const h = [];
    for (const sg of this.sources()) {
      const s0 = new Date(sg.start).getTime(), s1 = new Date(sg.end).getTime();
      const st = this.pv.get(sg.url) || {state: 'wait'};
      // A finished file's preview covers the whole file (its last picture can
      // be up to one step before the end); a file still being recorded, up
      // to its newest picture and one step more.
      const end = st.state === 'ready' && st.secs && isGrowing(sg) ? Math.min(Date.now(), s0 + (st.secs + (st.step || 0)) * 1000) : s1;
      const x0 = Math.max(0, pct(s0)), x1 = Math.min(100, pct(end));
      if (x1 > 0 && x0 < 100 && x1 > x0) h.push({a: x0, b: x1, cls: 's-' + st.state});
    }
    el.innerHTML = solidBars(h, 15000 / span * 100, 'i');
  },
  // Load the time-lapse of everything in view, newest first (up to now).
  async prefetch() {
    if (!TIMELAPSE) return;
    const gen = ++this.pfGen, id = this.id;
    await knownPreviews(id, this.sources(), this.pv);
    if (gen !== this.pfGen || id !== this.id) return;
    this.paintStrip();
    const todo = this.sources().slice().sort((x, y) => new Date(y.start) - new Date(x.start)).slice(0, 300);
    let i = 0, missing = 0;
    const worker = async () => {
      while (i < todo.length && gen === this.pfGen && id === this.id) {
        const sg = todo[i++], had = this.pv.get(sg.url);
        if (had && had.state === 'ready' && !(isGrowing(sg) && Date.now() - had.at > 30000)) continue;
        if (spriteBackoff(sg.url)) { missing++; continue; }
        if (!had || had.state !== 'ready') { this.pv.set(sg.url, {state: 'loading'}); this.paintStrip(); }
        const sp = await previewReady(id, sg);
        if (gen !== this.pfGen) break;
        if (sp) this.pv.set(sg.url, {state: 'ready', at: Date.now(), secs: sp.info.count * sp.info.interval, step: sp.info.interval});
        else { missing++; if (!had || had.state !== 'ready') this.pv.set(sg.url, {state: isGrowing(sg) || (tl.sprites.get(sg.url) || {}).tries < 3 ? 'wait' : 'none'}); }
        this.paintStrip();
      }
    };
    await Promise.all([worker(), worker()]);
    // Not ready yet: tried again shortly while this camera is still shown.
    clearTimeout(this.pfRetry);
    if (missing && gen === this.pfGen) this.pfRetry = setTimeout(() => { if (gen === this.pfGen && id === this.id && page === 'live' && !document.hidden) this.prefetch(); }, 6000);
  },
  // The white line: where the drag is, or what's playing.
  paintHead(T) {
    const h = $('ctl-lane').querySelector('.head'); if (!h) return;
    if (T == null && focusPlayback && focusPlayer && focusPlayer.item) T = focusPlayer.wallAt(focusPlayer.cur()).getTime();
    const [a, b] = this.win(), p = T == null ? -1 : (T - a) / (b - a);
    h.hidden = p < 0 || p > 1;
    if (!h.hidden) { h.style.left = (p * 100) + '%'; const em = h.querySelector('em'); if (em) em.textContent = fmtTime(new Date(T)); }
  },
  segAt(T) {
    const segs = (this.data && this.data.segments) || [];
    const hit = (sg) => new Date(sg.start).getTime() <= T && T < (isGrowing(sg) ? Date.now() + 5000 : new Date(sg.end).getTime() + 1000);
    return segs.find(x => x.kind === 'sub' && hit(x)) || segs.find(x => x.kind === 'recording' && hit(x)) || segs.find(hit);
  },
  // The time-lapse picture at T on the camera's picture.
  async scrubTo(T) {
    const el = $('focus-sprite'), surf = $('focus-surface');
    el.querySelector('.scrub-t').textContent = fmtDate(new Date(T));
    const sg = this.sources().find(s => segMs(s)._a <= T && T < (isGrowing(s) ? Date.now() + 5000 : s._b + 1000));
    if (Date.now() - T < 15000) { liveStill(el, this.id); el.hidden = false; return; }
    if (!sg) return;
    const sp = await spriteFor(this.id, sg);
    if (!sp || this.scrubT !== T) return;
    if (spriteStyle(el, sp, Math.floor((T - sg._a) / 1000 / sp.info.interval), surf.clientWidth, surf.clientHeight, true)) el.hidden = false;
  },
  // Once per screen frame, for the newest position.
  scrubSoon(T) {
    this.scrubT = T; this.paintHead(T);
    if (this.scrubQueued) return; this.scrubQueued = true;
    requestAnimationFrame(() => { this.scrubQueued = false; if (this.dragging) this.scrubTo(this.scrubT); });
  },
  // Play the camera's footage at time T in the camera view.
  playAt(T) {
    const sg = this.segAt(T);
    if (!sg) { toast('No recorded footage at that time (camera offline or not recording).'); $('focus-sprite').hidden = true; return; }
    playInFocus({camera: this.id, play: {url: sg.url, name: sg.name, kind: sg.kind, file_start: sg.start, offset: (T - new Date(sg.start).getTime()) / 1000}});
    // Keep the time-lapse picture up until the video shows the new time.
    const v = $('focus-video'), el = $('focus-sprite'), gen = this.hideGen = (this.hideGen || 0) + 1;
    const done = () => { if (this.hideGen === gen) el.hidden = true; };
    const check = () => { if (this.hideGen !== gen) return; if (v.readyState >= 2 && !v.seeking && v.currentTime > 0) done(); else setTimeout(check, 100); };
    setTimeout(check, 100); setTimeout(done, 5000);
  },
};
$('ctl-zoom').onchange = () => { camTl.zoom = +$('ctl-zoom').value; camTl.load(); };
$('ctl-left').onclick = () => { camTl.end = camTl.win()[1] - camTl.zoom * 500; camTl.load(); };
$('ctl-right').onclick = () => { const e = camTl.win()[1] + camTl.zoom * 500; camTl.end = e >= Date.now() ? null : e; camTl.load(); };
$('ctl-now').onclick = () => { camTl.end = null; camTl.load(); };
(function wireCamTl() {
  const lane = $('ctl-lane'), strip = $('ctl-tlp');
  const tAt = (e) => { const r = lane.getBoundingClientRect(), [a, b] = camTl.win(); return [Math.min(Date.now(), a + Math.min(Math.max((e.clientX - r.left) / r.width, 0), 1) * (b - a)), e.clientX - r.left]; };
  const preview = async (e) => {
    const [T, x] = tAt(e), pv = lane.querySelector('.preview');
    pv.hidden = false;
    pv.style.left = Math.min(Math.max(x, 100), lane.clientWidth - 100) + 'px';
    pv.querySelector('.t').textContent = fmtDate(new Date(T));
    const sg = camTl.segAt(T), im = pv.querySelector('.img');
    if (!sg) { im.style.backgroundImage = ''; return; }
    const sp = await spriteFor(camTl.id, sg);
    if (sp) {
      // As big as the time-lapse picture's detail (up to 384 px wide).
      const i = sp.info, w = Math.round(Math.max(192, Math.min(i.tile_width || 256, 384))), h = Math.round(w / (i.aspect || i.tile_width / i.tile_height || 16 / 9));
      Object.assign(im.style, {width: w + 'px', height: h + 'px'});
      spriteStyle(im, sp, Math.floor((T - new Date(sg.start).getTime()) / 1000 / i.interval), w, h);
    }
  };
  const down = (e) => {
    const pic = e.target.closest('.evt');
    if (pic && camTl.groups) {
      e.preventDefault();
      const g = camTl.groups[+pic.dataset.g];
      if (g.evs.length === 1) camTl.playAt(new Date(g.evs[0].start).getTime() - 3000);
      else openAlertPicker(pic, g, (ev) => camTl.playAt(new Date(ev.start).getTime() - 3000));
      return;
    }
    if (!camTl.data || markAt(e)) return;
    e.preventDefault();
    camTl.dragging = true; lane.setPointerCapture(e.pointerId); $('focus-surface').classList.add('scrub');
    lane.querySelector('.preview').hidden = true;
    const [T] = tAt(e); camTl.scrubT = T; camTl.paintHead(T); camTl.scrubTo(T);
  };
  lane.addEventListener('pointerdown', down);
  strip.addEventListener('pointerdown', (e) => { lane.dispatchEvent(new PointerEvent('pointerdown', e)); });
  // The marker nearest the pointer lights up; a guide line follows it.
  let hot = null;
  const setHot = (m) => { if (hot === m) return; if (hot) hot.classList.remove('hot'); hot = m; if (hot) hot.classList.add('hot'); lane.style.cursor = hot ? 'pointer' : ''; };
  lane.addEventListener('pointermove', (e) => {
    const g = lane.querySelector('.hover-line');
    if (g) { g.hidden = camTl.dragging; g.style.left = (e.clientX - lane.getBoundingClientRect().left) + 'px'; }
    setHot(camTl.dragging ? null : markAt(e));
  });
  lane.addEventListener('pointerleave', () => { setHot(null); const g = lane.querySelector('.hover-line'); if (g) g.hidden = true; });
  lane.addEventListener('pointermove', (e) => {
    if (!camTl.data) return;
    if (!camTl.dragging) return preview(e);
    camTl.scrubSoon(tAt(e)[0]);
  });
  const up = (e) => {
    if (!camTl.dragging) return;
    camTl.dragging = false; $('focus-surface').classList.remove('scrub');
    const [T] = tAt(e); camTl.playAt(T); camTl.draw();
  };
  lane.addEventListener('pointerup', up); lane.addEventListener('pointercancel', up);
  lane.addEventListener('pointerleave', () => { lane.querySelector('.preview').hidden = true; });
  lane.addEventListener('click', (e) => {
    const m = markAt(e);
    if (m) camTl.playAt(+m.dataset.t - 3000);
  });
})();
// While open: new footage and its time-lapse keep coming in (up to now),
// and the line follows what's playing.
setInterval(() => { if (page === 'live' && focusId && !camTl.end && !camTl.dragging) camTl.load(); }, 30000);
setInterval(() => { if (page === 'live' && focusId && !camTl.dragging) camTl.paintHead(); }, 500);
function stopFocus() { if (focusUnbind) { focusUnbind(); focusUnbind = null; } stopFocusPlayback(false); stopTalk(); }

// ---------- live sound ----------
// The live picture is JPEGs (no sound): the camera's audio comes as its own
// MP3 stream, a second or two behind the picture. One camera at a time; the
// speaker on each live picture switches it, hovering it shows the volume.
let soundCam = null;
function soundVolume() { const v = +store.get('live-volume', 1); return isFinite(v) ? v : 1; }
function setSoundVolume(v) {
  store.set('live-volume', v);
  $('focus-audio').volume = v;
  document.querySelectorAll('.snd-vol').forEach(r => { if (+r.value !== v) r.value = v; });
}
function paintSound() {
  document.querySelectorAll('.snd').forEach(b => {
    const on = b.dataset.cam === soundCam;
    b.textContent = on ? '🔊' : '🔇';
    b.classList.toggle('on', on);
    b.title = on ? 'Sound on — click to mute' : 'Sound off — click to hear this camera';
  });
}
let soundCatchUp = null;
function startSound(camId) {
  const a = $('focus-audio');
  soundCam = camId;
  a.volume = soundVolume();
  a.src = `live/${enc(camId)}/audio.mp3?t=${Date.now()}`;
  // Live sound: the player builds up a cushion and falls behind. Cameras
  // send sound in bursts, so keep about 0.8 s in hand (less runs dry and
  // the sound stops and starts) and catch up by playing a touch faster —
  // inaudible — rather than jumping, which clicks. Only a big lag (after
  // the tab was in the background) is skipped.
  clearInterval(soundCatchUp);
  a.preservesPitch = true; a.playbackRate = 1;
  soundCatchUp = setInterval(() => {
    if (soundCam !== camId || a.paused || !a.buffered.length) return;
    const lag = a.buffered.end(a.buffered.length - 1) - a.currentTime;
    if (lag > 4) { a.currentTime = a.buffered.end(a.buffered.length - 1) - 0.8; a.playbackRate = 1; }
    else a.playbackRate = lag > 1.6 ? 1.08 : lag > 1.1 ? 1.03 : 1;
  }, 400);
  // The browser only says "no supported source"; ask GAZE why.
  const why = async () => {
    if (soundCam !== camId) return;
    stopSound();
    let msg = 'No sound from this camera.';
    try {
      const ctl = new AbortController();
      const r = await fetch(`live/${enc(camId)}/audio.mp3?t=${Date.now()}`, { signal: ctl.signal });
      if (r.ok) { ctl.abort(); msg = 'The sound wasn\'t ready yet. Press the speaker again.'; }
      else { const j = await r.json().catch(() => ({})); if (j.error) msg = j.error; }
    } catch (_) {}
    toast(msg, true);
  };
  a.onerror = why;
  a.onended = () => { if (soundCam !== camId) return; if (a.currentTime < 0.5) why(); else stopSound(); };
  a.play().then(paintSound).catch(e => { if (e && e.name === 'NotAllowedError') { toast('Could not play sound: ' + e.message, true); stopSound(); } else why(); });
  paintSound();
}
function stopSound() {
  const a = $('focus-audio'); if (!a) return;
  soundCam = null; clearInterval(soundCatchUp);
  a.onerror = a.onended = null;
  a.pause(); a.removeAttribute('src'); a.load();
  paintSound();
}

// ---------- sound detection (camera → Audio) ----------
// The kinds of sound to tick, and (for a saved camera) what it hears now.
function wireSoundDetect(camId) {
  const box = $('c-snd-kinds'); if (!box) return;
  $('c-snd-on').onchange = () => { $('c-snd-box').hidden = !chk('c-snd-on'); };
  $('c-snd-sens').oninput = () => { $('c-snd-sens-v').textContent = val('c-snd-sens'); };
  const paint = (r) => {
    const chosen = new Set(JSON.parse(box.dataset.kinds));
    if (!box.children.length) box.innerHTML = r.kinds.map(k => `<label class="check"><input type="checkbox" value="${esc(k.id)}" ${chosen.has(k.id) ? 'checked' : ''}> ${esc(k.name)}</label>`).join('');
    const st = camId && r.cameras[camId], el = $('c-snd-now'); if (!el) return;
    el.innerHTML = !st ? (chk('c-snd-on') ? 'Starts listening once saved.' : '')
      : st.error ? `<span class="err">Not listening: ${esc(st.error)}</span>`
      : `Hearing now: ${st.now.map(([n, v]) => `${esc(n)} ${Math.round(v * 100)}%`).join(' · ') || '…'}${st.last_heard ? ` — last alert: <b>${esc(st.last_heard)}</b> at ${fmtTime(new Date(st.last_at))}` : ''}`;
  };
  const tick = () => { if (!box.isConnected) return; api('api/sound').then(paint).catch(() => {}); setTimeout(tick, 1500); };
  tick();
}

// ---------- two-way audio ----------
// Push to talk: hold the button and speak; a quick tap keeps talking until
// the next tap. The microphone is captured in an AudioWorklet, brought down
// to 8 kHz 16-bit mono and sent over a WebSocket; GAZE plays it on the
// camera's speaker. What's said while connecting is kept and sent once the
// speaker is ready, so the first words aren't lost.
let talk = null;
const TALK_WORKLET = `class Mic extends AudioWorkletProcessor {
  process(inputs) { const ch = inputs[0][0]; if (ch) this.port.postMessage(ch.slice(0)); return true; }
}
registerProcessor('gaze-mic', Mic);`;
function talkLabel(text, cls) {
  document.querySelectorAll('#focus-talk, .p-talk').forEach(b => {
    b.querySelector('span').textContent = text;
    b.classList.toggle('talking', cls === 'talking'); b.classList.toggle('connecting', cls === 'connecting');
  });
}
function stopTalk() {
  if (!talk) return;
  const t = talk; talk = null;
  // Send what's left, then (with the chime on) a short "goodbye" tune so
  // whoever is there knows you've stopped talking, then close.
  try { if (t.flush) t.flush(); } catch (_) {}
  let wait = 150;
  if (t.chimeMs && t.ready && t.ws && t.ws.readyState === 1) {
    const end = goodbyeChime();
    for (let i = 0; i < end.length; i += 320) t.ws.send(end.slice(i, i + 320).buffer);
    wait = end.length / 8 + 400;
  }
  setTimeout(() => { try { t.ws && t.ws.close(); } catch (_) {} }, wait);
  t.stream && t.stream.getTracks().forEach(tr => tr.stop());
  t.ctx && t.ctx.close().catch(() => {});
  talkLabel('Push to talk');
}
// Notes ([Hz, start s]) as a soft bell-like tune: 8 kHz, 16-bit samples,
// like the voice sent to the camera's speaker.
function chimeTune(notes, seconds, decay, loud) {
  const rate = 8000, len = Math.round(rate * seconds), out = new Int16Array(len);
  for (const [f, start] of notes) {
    const s0 = Math.round(start * rate);
    for (let i = 0; s0 + i < len; i++) {
      const t = i / rate, env = Math.min(1, t / 0.008) * Math.exp(-t * decay);
      const v = Math.sin(2 * Math.PI * f * t) + 0.3 * Math.sin(2 * Math.PI * f * 2 * t);
      out[s0 + i] = Math.max(-32767, Math.min(32767, out[s0 + i] + v * env * loud * 32767 / 1.3));
    }
  }
  return out;
}
// Talking stopped: three quick falling notes (G, E, C), softer and shorter
// than the doorbell chime.
function goodbyeChime() { return chimeTune([[783.99, 0], [659.25, 0.13], [523.25, 0.26]], 0.7, 9, 0.4); }
// A two-tone "ding-dong" doorbell chime: 8 kHz, 16-bit samples, like the
// voice sent to the camera's speaker.
function doorbellChime() {
  const rate = 8000, notes = [[659.25, 0], [523.25, 0.55]], len = Math.round(rate * 1.6);
  const out = new Int16Array(len);
  for (const [f, start] of notes) {
    const s0 = Math.round(start * rate);
    for (let i = 0; s0 + i < len; i++) {
      const t = i / rate, env = Math.min(1, t / 0.01) * Math.exp(-t * 3.2);
      const v = Math.sin(2 * Math.PI * f * t) + 0.35 * Math.sin(2 * Math.PI * f * 2 * t) + 0.12 * Math.sin(2 * Math.PI * f * 3 * t);
      out[s0 + i] = Math.max(-32767, Math.min(32767, out[s0 + i] + v * env * 0.55 * 32767 / 1.47));
    }
  }
  return out;
}
async function startTalk(camId, latch) {
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia || !window.AudioWorkletNode) {
    toast('The browser only allows the microphone on secure pages: open GAZE as https://… (for example through Tailscale) or as http://localhost on this computer.', true);
    return;
  }
  const t = talk = {cam: camId, latch, ready: false, queue: []};
  // Hear the camera too, so the conversation goes both ways (the sound
  // stays on afterwards to hear the answer; the speaker button turns it off).
  if (soundCam !== camId) startSound(camId);
  // A doorbell chime first (camera's setting): sent ahead of the voice, which
  // starts once the chime has played.
  const camInfo = status && status.cameras.find(x => x.id === camId);
  if (!camInfo || camInfo.talk_chime !== false) {
    const chime = doorbellChime();
    for (let i = 0; i < chime.length; i += 320) t.queue.push(chime.slice(i, i + 320).buffer);
    t.chimeMs = chime.length / 8;
  }
  talkLabel('Connecting…', 'connecting');
  // Connect to the speaker and open the microphone at the same time.
  t.ws = new WebSocket(location.href.replace(/^http/, 'ws').replace(/[#?].*$/, '').replace(/[^/]*$/, '') + `api/cameras/${enc(camId)}/talk`);
  t.ws.binaryType = 'arraybuffer';
  t.ws.onmessage = (ev) => {
    let m; try { m = JSON.parse(ev.data); } catch (_) { return; }
    if (m.error) { toast(m.error, true); if (talk === t) stopTalk(); return; }
    if (!m.ok || talk !== t) return;
    t.ready = true;
    for (const b of t.queue) t.ws.send(b);
    t.queue = [];
    // The microphone joins once the chime has played (it starts now).
    if (t.chimeMs) t.muteUntil = performance.now() + t.chimeMs;
    talkLabel(t.latch ? 'Talking — tap to stop' : 'Talking… release to stop', 'talking');
  };
  t.ws.onclose = () => { if (talk === t) { if (t.ready) toast('Talking stopped'); else toast('Could not connect to the camera speaker', true); stopTalk(); } };
  try {
    t.stream = await navigator.mediaDevices.getUserMedia({audio: {echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1}});
  } catch (e) { toast('No microphone: ' + e.message, true); if (talk === t) stopTalk(); return; }
  if (talk !== t) { t.stream.getTracks().forEach(tr => tr.stop()); return; }
  t.ctx = new AudioContext();
  await t.ctx.audioWorklet.addModule(URL.createObjectURL(new Blob([TALK_WORKLET], {type: 'text/javascript'})));
  if (talk !== t) { t.ctx.close(); t.stream.getTracks().forEach(tr => tr.stop()); return; }
  const src = t.ctx.createMediaStreamSource(t.stream), node = new AudioWorkletNode(t.ctx, 'gaze-mic');
  const ratio = t.ctx.sampleRate / 8000; let pos = 0, acc = 0, cnt = 0, out = [];
  const push = () => {
    if (!out.length) return;
    const buf = new Int16Array(out).buffer; out = [];
    // Speech from before the speaker was ready isn't kept: sent late, it
    // would stay as a delay for the whole conversation.
    if (t.ready && t.ws.readyState === 1) t.ws.send(buf);
  };
  t.flush = push;
  node.port.onmessage = (e2) => {
    // While the chime plays, the microphone isn't sent (the chime goes first).
    if (!t.ready || (t.muteUntil && performance.now() < t.muteUntil)) { pos = acc = cnt = 0; return; }
    // Average the samples that fall into each 1/8000 s (a simple low-pass).
    for (const v of e2.data) {
      acc += v; cnt++; pos += 1;
      if (pos >= ratio) { pos -= ratio; out.push(Math.max(-1, Math.min(1, acc / cnt)) * 32767); acc = 0; cnt = 0; }
    }
    if (out.length >= 320) push();
  };
  src.connect(node);
}
// Push to talk (hold); a quick tap latches (tap again to stop).
// "Say…": type words and the camera's speaker says them (the computer
// running GAZE turns them into speech).
function setupSayButton(c, b = $('focus-say')) {
  if (!b) return;
  b.hidden = !c.talk || !me || me.role === 'viewer';
  b.onclick = (e) => { e.stopPropagation(); openSay(c, b); };
}
// 🚨 Siren: loud alarm with spoken warnings, through the camera's speaker.
const sirenOn = new Set();
function setupSirenButton(c, b = $('focus-siren')) {
  if (!b) return;
  b.hidden = !c.talk || !me || me.role === 'viewer';
  const paint = () => {
    const on = sirenOn.has(c.id) || ((status && status.sirens) || []).includes(c.id);
    b.classList.toggle('siren-on', on);
    const t = b.querySelector('span') || b; t.textContent = on ? 'Stop siren' : (b.querySelector('span') ? 'Siren' : '🚨 Siren');
  };
  paint(); b._paint = paint;
  b.onclick = async (e) => {
    e.stopPropagation();
    if (sirenOn.has(c.id) || ((status && status.sirens) || []).includes(c.id)) {
      sirenOn.delete(c.id); paint();
      await api(`api/cameras/${enc(c.id)}/siren/stop`, {method: 'POST'}).catch(() => {});
      return refreshStatus();
    }
    if (!confirm(`Sound the siren on ${c.name}? It's loud, with spoken warnings, until you stop it or the set time is up.`)) return;
    b.disabled = true;
    try {
      const r = await api(`api/cameras/${enc(c.id)}/siren`, {method: 'POST'});
      sirenOn.add(c.id); paint();
      clearTimeout(b._t); b._t = setTimeout(() => { sirenOn.delete(c.id); paint(); }, r.seconds * 1000 + 3000);
    } catch (err) { toast(err.message, true); } finally { b.disabled = false; }
  };
}
// The siren buttons follow what the server says is sounding.
function paintSirens() { document.querySelectorAll('#focus-siren, .p-siren').forEach(b => b._paint && b._paint()); }
const SAY_RECENT = 'gaze-say-recent';
function openSay(c, anchor) {
  document.querySelectorAll('.say-pop').forEach(x => x.remove());
  let recent = []; try { recent = JSON.parse(localStorage.getItem(SAY_RECENT) || '[]'); } catch (_) {}
  const pop = document.createElement('form');
  pop.className = 'say-pop';
  pop.innerHTML = `<div class="row"><input type="text" maxlength="400" placeholder="What should ${esc(c.name)} say?" autocomplete="off" list="say-recent"><button type="submit" class="primary">🔊 Say</button></div>
    <datalist id="say-recent">${recent.map(t => `<option value="${esc(t)}">`).join('')}</datalist>
    <div class="say-quick">${((status && status.say_phrases) || []).map(t => `<button type="button" class="small">${esc(t)}</button>`).join('')}</div>
    <div class="hint say-msg"></div>`;
  (anchor.closest('dialog, .fspov, body') || document.body).appendChild(pop);
  const r = anchor.getBoundingClientRect(), w = Math.min(420, innerWidth - 16);
  Object.assign(pop.style, {left: Math.max(8, Math.min(innerWidth - w - 8, r.left)) + 'px', width: w + 'px'});
  if (r.top > 200) pop.style.bottom = (innerHeight - r.top + 6) + 'px'; else pop.style.top = (r.bottom + 6) + 'px';
  const inp = pop.querySelector('input'), msg = pop.querySelector('.say-msg'), go = pop.querySelector('[type=submit]');
  inp.focus();
  // Sent one after another: more can be sent while one is being said.
  let waiting = 0;
  const show = () => { msg.className = 'hint say-msg'; msg.textContent = waiting > 1 ? `Saying it… (${waiting - 1} more waiting)` : 'Saying it…'; };
  const send = async (text) => {
    text = text.trim(); if (!text) return;
    waiting++; show(); inp.value = ''; inp.focus();
    recent = [text, ...recent.filter(t => t !== text)].slice(0, 10);
    try { localStorage.setItem(SAY_RECENT, JSON.stringify(recent)); } catch (_) {}
    try {
      await api(`api/cameras/${enc(c.id)}/say`, {method: 'POST', json: {text}});
      waiting--; if (waiting) show(); else { msg.className = 'hint say-msg okc'; msg.textContent = '✓ Said'; }
    } catch (e) { waiting--; msg.className = 'hint say-msg err'; msg.textContent = e.message; }
  };
  pop.onsubmit = (e) => { e.preventDefault(); send(inp.value); };
  pop.querySelectorAll('.say-quick button').forEach(b => b.onclick = () => send(b.textContent));
  pop.onkeydown = (e) => { if (e.key === 'Escape') { e.stopPropagation(); e.preventDefault(); close(); } };
  const away = (e) => { if (!pop.contains(e.target) && e.target !== anchor && !anchor.contains(e.target)) close(); };
  const close = () => { pop.remove(); document.removeEventListener('pointerdown', away, true); };
  setTimeout(() => document.addEventListener('pointerdown', away, true), 0);
}
function setupTalkButton(c, b = $('focus-talk')) {
  b.hidden = !c.talk;
  if (talk && talk.cam !== c.id) stopTalk();
  if (!c.talk) return;
  talkLabel(talk ? (talk.latch ? 'Talking — tap to stop' : 'Talking…') : 'Push to talk', talk ? 'talking' : '');
  let downAt = 0;
  b.onclick = null;
  b.oncontextmenu = (e) => e.preventDefault();
  b.onpointerdown = (e) => {
    e.preventDefault();
    if (talk && talk.latch) { stopTalk(); downAt = 0; return; }
    downAt = Date.now();
    try { b.setPointerCapture(e.pointerId); } catch (_) {}
    startTalk(c.id, false);
  };
  const up = () => {
    if (!downAt || !talk) return;
    const held = Date.now() - downAt; downAt = 0;
    if (held < 350) { talk.latch = true; talkLabel(talk.ready ? 'Talking — tap to stop' : 'Connecting…', talk.ready ? 'talking' : 'connecting'); }
    else stopTalk();
  };
  b.onpointerup = up; b.onpointercancel = up;
}

function updateFocus() {
  const c = cam(focusId);
  if (!c) { location.hash = '#live'; return; }
  updateOverlay($('focus-surface'), c, feedQ(c, 'focus'));
  const r = c.recorder || c.buffer;
  const btn = $('focus-rec');
  btn.hidden = !r || !c.enabled;
  if (r) {
    btn.innerHTML = r.paused ? '<i class="bi">⏺</i><span>Resume recording</span>' : '<i class="bi">⏸</i><span>Pause recording</span>';
    btn.classList.toggle('primary', r.paused);
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
    ${d && d.parked && d.parked.length ? `<dt>Parked</dt><dd title="Not alerting until they move">${esc(d.parked.join(', '))} (ignored until moved)</dd>` : ''}
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
  const ai = allAlertTypes().filter(t => t !== 'motion');
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
  hlsReady();
  focusPlayback = it;
  updateSubscriptions();
  $('focus-img').hidden = true;
  $('focus-surface').classList.add('pb');
  $('focus-surface').querySelectorAll('.ov:not(.pov-osd)').forEach(o => o.hidden = true);
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
  $('focus-surface').classList.remove('pb');
  $('focus-player-controls').hidden = true;
  $('focus-surface').querySelectorAll('.ov:not(.pov-osd)').forEach(o => o.hidden = false);
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
// The Alerts filter starts on the usual things (remembered per browser).
const DEFAULT_TYPES = ['person', 'vehicle', 'package', 'face', 'plate'];
// "Last 24 hours" (the default): from the start of the hour a day ago
// (so it only moves once an hour), up to the end of today.
const last24From = () => { const h = new Date(); h.setMinutes(0, 0, 0); return new Date(h.getTime() - 86400000); };
const rec = {mode: 'list', today: false, last24: true, cameras: new Set(), from: last24From(), to: endOfDay(new Date()), todFrom: '', todTo: '',
  types: new Set(canonKinds(store.get('alert-types', DEFAULT_TYPES))), items: [], who: ''};
const saveTypes = () => store.set('alert-types', [...rec.types]);
// The Alerts filters as they start: the last 24 hours, every camera, the usual kinds,
// new and viewed, anyone.
function resetAlertFilters() {
  rec.cameras = new Set(); rec.prevCameras = null;
  rec.from = last24From(); rec.to = endOfDay(new Date()); rec.todFrom = ''; rec.todTo = ''; rec.today = false; rec.last24 = true;
  rec.types = new Set(DEFAULT_TYPES); saveTypes();
  rec.seen = ''; rec.who = '';
  if ($('f-seen')) $('f-seen').value = '';
  if ($('f-who')) $('f-who').value = '';
}
let recPlayer = null;

function showRecordings(camId, mode) {
  hlsReady();  // the continuous player, ready by the time it's needed
  if (camId && cam(camId)) rec.cameras = new Set([camId]);
  renderFilters();
  setRecMode(mode || 'timeline');
}

function setRecMode(mode) {
  rec.mode = mode;
  $('rec-title').textContent = mode === 'list' ? 'Alerts' : 'Recordings';
  $('rec-list').hidden = mode !== 'list';
  $('rec-timeline').hidden = mode !== 'timeline';
  // Recordings: the title and filters in a column on the left (the video
  // gets the height); Alerts keep them across the top.
  $('page-recordings').classList.toggle('tl-mode', mode === 'timeline');
  $('f-types').hidden = mode !== 'list';
  document.querySelectorAll('#page-recordings .list-only').forEach(e => e.hidden = mode !== 'list');
  document.querySelectorAll('#page-recordings .tl-only').forEach(e => e.hidden = mode === 'list' || !TIMELAPSE);
  if (mode === 'list') { tlStop(); loadRecordings(); wireDescribeSearch(); }
  else { closeAlert(); if (!resumeTimeline()) loadTimeline(); }
}
// What the timeline shows (cameras and days): coming back to the same, it
// carries on as it was — players, pictures and time-lapse kept — instead of
// loading everything again (only newer footage is fetched).
const tlKey = () => [[...rec.cameras].sort().join(','), rec.from && rec.from.getTime(), rec.to && rec.to.getTime()].join('|');
function resumeTimeline() {
  if (tl.key !== tlKey() || !document.querySelector('#tl-videos .surface') || Date.now() - (tl.loadedAt || 0) > 3 * 3600000) return false;
  drawTimeline(); tlSync(true); prefetchSprites();
  refreshTimelineData();
  return true;
}
// Newer footage and alerts for the timeline as it is (nothing reloaded).
function refreshTimelineData() {
  const q = new URLSearchParams({from: rec.from.toISOString(), to: new Date(Math.max(rec.to.getTime(), Date.now())).toISOString()});
  if (rec.cameras.size) q.set('cameras', [...rec.cameras].join(','));
  return api('api/timeline?' + q).then(d => {
    if (tl.dragging || page !== 'recordings') return;
    // A camera added or removed meanwhile: build the players afresh.
    const same = d.length === tl.data.length && d.every((c, i) => c.camera === tl.data[i].camera);
    tl.data = d; tl.loadedAt = Date.now();
    if (!same) buildTimelineVideos();
    drawTimeline(); prefetchSprites();
    if (!same) tlSync(true);
  }).catch(() => {});
}
$('f-seen').onchange = () => { rec.seen = val('f-seen'); loadRecordings(); };
// People and vehicles taught in People & vehicles: only their alerts.
async function fillWhoFilter() {
  const sel = $('f-who'); if (!sel) return;
  let list = [];
  try { list = await api('api/known'); } catch (_) { return; }
  const people = list.filter(k => k.kind === 'person'), cars = list.filter(k => k.kind !== 'person');
  const opt = (k) => `<option value="${esc(k.id)}" ${rec.who === k.id ? 'selected' : ''}>${k.kind === 'person' ? '🧑' : '🚗'} ${esc(k.name)}${k.plates && k.plates.length ? ' — ' + esc(k.plates.join(', ')) : ''}</option>`;
  sel.innerHTML = `<option value="">Anyone</option><option value="__new" ${rec.who === '__new' ? 'selected' : ''}>🆕 New faces &amp; plates to name</option>`
    + (people.length ? `<optgroup label="People (faces)">${people.map(opt).join('')}</optgroup>` : '')
    + (cars.length ? `<optgroup label="Vehicles (number plates)">${cars.map(opt).join('')}</optgroup>` : '');
  sel.hidden = false;
}
$('f-who').onchange = () => { rec.who = val('f-who'); loadRecordings(); };
$('f-seen-all').onclick = async () => {
  await api('api/alerts/seen-all', {method: 'POST'}).catch(e => toast(e.message, true));
  rec.items.forEach(i => i.seen = true);
  renderRecordings(); refreshStatus();
  toast('All new alerts dismissed.');
};
// New alerts appear by themselves (no need to refresh the page).
// Refreshed as soon as the status shows a new or changed alert (it comes
// every 2 s), and every 8 s anyway (for pictures and details added later).
async function refreshAlertList() {
  if (page !== 'recordings' || rec.mode !== 'list' || rec.loading || rec.who === '__new' || refreshAlertList.busy || rec.describe) return;
  // "Today" moves on at midnight on a page left open.
  if (rec.today && rec.to.getTime() < Date.now()) { rec.from = startOfDay(new Date()); rec.to = endOfDay(new Date()); renderFilters(); }
  // ("Last 24 hours" moves on with the clock.)
  if (rec.last24 && last24From().getTime() !== rec.from.getTime()) { rec.from = last24From(); rec.to = endOfDay(new Date()); renderFilters(); }
  refreshAlertList.busy = true;
  try {
    const before = rec.items.map(i => i.event ? i.event.id + i.event.end + (i.event.notes || []).length + i.seen + (i.thumb || '') : i.name).join();
    const r = await fetchRecordings().catch(() => null);
    if (r && r.map(i => i.event ? i.event.id + i.event.end + (i.event.notes || []).length + i.seen + (i.thumb || '') : i.name).join() !== before) { rec.items = r; renderRecordings(); }
  } finally { refreshAlertList.busy = false; }
}
setInterval(() => { if (!document.hidden) refreshAlertList(); }, 8000);
let alertSig = '';
function alertsChanged() {
  const sig = (status.alerts_unseen || 0) + '|' + status.cameras.map(c => c.detect ? `${c.detect.last_event || ''}${c.detect.event_active ? '*' : ''}` : '').join(',');
  if (sig !== alertSig) { const first = !alertSig; alertSig = sig; if (!first) refreshAlertList(); }
}
$('f-refresh').onclick = () => loadRecordings();
// Full screen for the whole app, from the header (any page, phones too
// where the browser allows it).
(function wireAppFullscreen() {
  const b = $('app-fs'), root = document.documentElement;
  if (!(root.requestFullscreen || root.webkitRequestFullscreen)) { b.hidden = true; return; }
  const paint = () => {
    const on = !!(document.fullscreenElement || document.webkitFullscreenElement);
    b.querySelector('.fs-label').textContent = on ? ' Exit full screen' : ' Full screen';
    b.classList.toggle('on', on);
  };
  b.onclick = () => {
    if (document.fullscreenElement || document.webkitFullscreenElement) (document.exitFullscreen || document.webkitExitFullscreen).call(document);
    else (root.requestFullscreen || root.webkitRequestFullscreen).call(root);
  };
  document.addEventListener('fullscreenchange', paint);
  document.addEventListener('webkitfullscreenchange', paint);
})();

function renderFilters() {
  if (!status) return;
  // Cameras
  const names = rec.cameras.size ? status.cameras.filter(c => rec.cameras.has(c.id)).map(c => c.name) : [];
  $('f-cams-btn').textContent = (names.length === 0 ? 'All cameras' : names.length <= 2 ? names.join(', ') : `${names.length} cameras`) + ' ▾';
  const pop = $('f-cams-pop');
  pop.innerHTML = `<label class="check" style="display:flex"><input type="checkbox" data-all ${rec.cameras.size ? '' : 'checked'}> All cameras</label><hr style="border-color:var(--border)">` +
    status.cameras.map(c => `<label class="check" style="display:flex"><input type="checkbox" value="${esc(c.id)}" ${rec.cameras.has(c.id) ? 'checked' : ''}> ${esc(c.name)}</label>`).join('');
  pop.querySelectorAll('input').forEach(i => i.onchange = () => {
    rec.prevCameras = null; tl.hd = null;
    if (i.dataset.all !== undefined) rec.cameras.clear();
    else if (i.checked) rec.cameras.add(i.value); else rec.cameras.delete(i.value);
    renderFilters(); reloadRec();
  });
  // Dates
  const same = startOfDay(rec.from).getTime() === startOfDay(rec.to).getTime();
  const today = startOfDay(new Date()).getTime(), f0 = startOfDay(rec.from).getTime();
  let label = rec.last24 ? 'Last 24 hours' : rec.from.getFullYear() <= 2000 ? 'All time' : same ? (f0 === today ? 'Today' : f0 === today - 86400000 ? 'Yesterday' : fmtDay(rec.from)) : `${fmtDay(rec.from)} – ${fmtDay(rec.to)}`;
  if (rec.todFrom || rec.todTo) label += `, ${rec.todFrom || '00:00'}–${rec.todTo || '23:59'}`;
  $('f-date-btn').textContent = '📅 ' + label;
  // Types
  const types = filterTypes();
  // A kind no camera raises any more can't stay ticked out of sight.
  for (const k of [...rec.types]) if (!types.some(t => t[0] === k)) rec.types.delete(k);
  $('f-types').innerHTML = '<span class="muted" style="padding:3px 4px">Show:</span>' + types.map(([k, v]) =>
    `<label><input type="checkbox" value="${k}" ${rec.types.has(k) ? 'checked' : ''}>${v}</label>`).join('') +
    (rec.types.size ? '<button class="small" id="f-types-clear">All alerts</button>' : '');
  $('f-types').querySelectorAll('input').forEach(i => i.onchange = () => { i.checked ? rec.types.add(i.value) : rec.types.delete(i.value); saveTypes(); renderFilters(); loadRecordings(); });
  const clr = $('f-types-clear'); if (clr) clr.onclick = () => { rec.types.clear(); saveTypes(); renderFilters(); loadRecordings(); };
}
$('f-cams-btn').onclick = (e) => { e.stopPropagation(); $('f-cams-pop').hidden = !$('f-cams-pop').hidden; $('f-date-pop').hidden = true; };
$('f-date-btn').onclick = (e) => { e.stopPropagation(); const p = $('f-date-pop'); p.hidden = !p.hidden; $('f-cams-pop').hidden = true; if (!p.hidden) Calendar.open(); };
function reloadRec() { rec.mode === 'list' ? loadRecordings() : loadTimeline(); }

// ---------- calendar with range + time-of-day ----------
const Calendar = {
  month: null, a: null, b: null,
  open() { this.a = startOfDay(rec.from); this.b = startOfDay(rec.to); this.first = null; this.month = new Date(this.b.getFullYear(), this.b.getMonth(), 1); this.render(); },
  // Show the chosen days right away (close the calendar unless a range is
  // being picked).
  apply(close) {
    const lo = Math.min(this.a, this.b ?? this.a), hi = Math.max(this.a, this.b ?? this.a);
    rec.from = startOfDay(new Date(lo)); rec.to = endOfDay(new Date(hi));
    rec.today = rec.from.getTime() === startOfDay(new Date()).getTime() && rec.to.getTime() === endOfDay(new Date()).getTime();
    rec.last24 = false;
    if ($('cal-tf')) { rec.todFrom = $('cal-tf').value; rec.todTo = $('cal-tt').value; }
    if (close) $('f-date-pop').hidden = true;
    renderFilters(); tl.day = null; reloadRec();
  },
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
      <div class="hint">${this.first != null ? 'Click another day to show the days in between too.' : 'Click a day to show it; click a second day for a range.'}</div>
      <div class="row" style="margin-top:8px;flex-wrap:wrap"><button type="button" class="small" data-p24>Last 24 hours</button><button type="button" class="small" data-p="0">Today</button><button type="button" class="small" data-p="1">Yesterday</button><button type="button" class="small" data-p="7">Last 7 days</button><button type="button" class="small" data-p="30">Last 30 days</button><button type="button" class="small" data-p="all">All time</button></div>
      <div class="row" style="margin-top:10px"><span class="muted">Only between</span><input type="time" id="cal-tf" value="${rec.todFrom}" style="width:auto"><span class="muted">and</span><input type="time" id="cal-tt" value="${rec.todTo}" style="width:auto"></div>
      <div class="actions" style="margin-top:10px"><button type="button" class="primary" data-apply>Done</button><button type="button" data-clear-time title="Every alert there is, at any time of day">All time</button></div></div>`;
    p.querySelectorAll('[data-nav]').forEach(b => b.onclick = () => { this.month.setMonth(this.month.getMonth() + +b.dataset.nav); this.render(); });
    p.querySelectorAll('[data-t]').forEach(b => b.onclick = (e) => {
      e.stopPropagation();
      const t = +b.dataset.t;
      if (this.first != null && this.first !== t) {
        // Second day: the range between them.
        this.a = this.first; this.b = t; this.first = null;
        this.apply(true); return;
      }
      this.a = this.b = t; this.first = t;
      this.apply(false); this.render();
    });
    p.querySelector('[data-p24]').onclick = () => {
      rec.from = last24From(); rec.to = endOfDay(new Date()); rec.today = false; rec.last24 = true;
      $('f-date-pop').hidden = true; renderFilters(); tl.day = null; reloadRec();
    };
    p.querySelectorAll('[data-p]').forEach(b => b.onclick = () => {
      const t = startOfDay(new Date());
      if (b.dataset.p === 'all') { this.b = t.getTime(); this.a = new Date(2000, 0, 1).getTime(); }
      else {
        const n = +b.dataset.p;
        if (n === 1) { t.setDate(t.getDate() - 1); this.a = this.b = t.getTime(); }
        else { this.b = t.getTime(); const s = new Date(t); s.setDate(s.getDate() - Math.max(0, n - 1)); this.a = s.getTime(); }
      }
      this.first = null; this.apply(true);
    });
    // All time: every alert, from the first day, at any time of day.
    p.querySelector('[data-clear-time]').onclick = () => {
      $('cal-tf').value = ''; $('cal-tt').value = '';
      this.b = startOfDay(new Date()).getTime(); this.a = new Date(2000, 0, 1).getTime(); this.first = null;
      this.apply(true);
    };
    p.querySelectorAll('#cal-tf, #cal-tt').forEach(i => i.onchange = () => this.apply(false));
    p.querySelector('[data-apply]').onclick = () => this.apply(true);
  },
};

// ---------- search by description ----------
// Shown when it's switched on (Settings → Object recognition).
async function wireDescribeSearch() {
  const st = await api('api/search/status').catch(() => null);
  const on = !!(st && st.on);
  $('ds-bar').hidden = !on;
  if (!on) return;
  $('ds-info').textContent = st.downloading ? 'Downloading the search model…' : st.error ? 'Search unavailable: ' + st.error : '';
  $('ds-bar').onsubmit = async (e) => {
    e.preventDefault();
    const q = val('ds-q').trim(); if (!q) return;
    const days = Math.max(1, Math.ceil((Date.now() - rec.from.getTime()) / 864e5));
    $('ds-go').disabled = true; $('ds-info').textContent = 'Searching…';
    try {
      const r = await api(`api/search?q=${enc(q)}&days=${days}`);
      rec.describe = {q, hits: r.hits, waiting: r.waiting, closest: r.closest};
      $('ds-clear').hidden = false; $('ds-info').textContent = '';
      await loadRecordings();
    } catch (err) { $('ds-info').textContent = err.message; }
    finally { $('ds-go').disabled = false; }
  };
  $('ds-clear').onclick = () => { rec.describe = null; $('ds-q').value = ''; $('ds-clear').hidden = true; loadRecordings(); };
}

// ---------- list ----------
// The alert list for the current filters (shares the query with loadRecordings).
async function fetchRecordings() {
  const q = recQuery();
  const r = await fetch('api/recordings?' + q);
  if (!r.ok) throw new Error(r.statusText);
  return r.json();
}
function recQuery() {
  const q = new URLSearchParams({from: rec.from.toISOString(), to: rec.to.toISOString(), limit: rec.from.getFullYear() <= 2000 ? 5000 : 2000});
  if (rec.cameras.size) q.set('cameras', [...rec.cameras].join(','));
  // Alerts only (plain recordings are on the Recordings tab).
  q.set('types', (rec.types.size ? [...rec.types] : allAlertTypes()).join(','));
  if (rec.todFrom) q.set('tod_from', rec.todFrom);
  if (rec.todTo) q.set('tod_to', rec.todTo);
  if (rec.seen) q.set('seen', rec.seen);
  if (rec.who && rec.who !== '__new') q.set('who', rec.who);
  return q;
}
async function loadRecordings() {
  $('rec-count').textContent = 'Loading…';
  if (!(Date.now() - (loadRecordings.who || 0) < 60000)) { loadRecordings.who = Date.now(); fillWhoFilter(); }
  if (rec.who === '__new') return loadNewFaces();
  rec.loading = true;
  try {
    const r = await fetch('api/recordings?' + recQuery());
    if (!r.ok) throw new Error((await r.json().catch(() => ({}))).error || r.statusText);
    rec.items = await r.json();
    // Searching by description: only the alerts that match (in time order).
    if (rec.describe) {
      const hit = new Set(rec.describe.hits.map(h => h.id));
      rec.items = rec.items.filter(i => i.event && hit.has(i.event.id));
    }
  } catch (e) { rec.items = []; toast(e.message, true); }
  rec.loading = false;
  renderRecordings();
  if (rec.describe) $('rec-count').textContent = (!rec.items.length ? `No alerts here look like “${rec.describe.q}”. Try other words, or more days.`
      : rec.describe.closest
      ? `Nothing clearly looks like “${rec.describe.q}” — the ${rec.items.length} closest. Try describing what's special (“a man in a red jacket”); for any person or car use the filters above.`
      : `${rec.items.length} alert${rec.items.length === 1 ? '' : 's'} look like “${rec.describe.q}”`) + (rec.describe.waiting ? ` (${rec.describe.waiting} older alert pictures still being looked at)` : '');
  if (openAlertId) {
    const it = rec.items.find(i => i.event && i.event.id === openAlertId);
    openAlertId = null;
    if (it) playItem(it, null); else toast('That alert is no longer in the list', true);
  }
}

function renderRecordings() {
  const box = $('rec-cards'), items = rec.items;
  $('rec-collapse').hidden = false;
  const ev = items.filter(i => i.kind === 'event').length;
  $('rec-count').textContent = (items.length ? `${ev} alert${ev === 1 ? '' : 's'}` : '')
    + (items.length ? ' · recordings are on the Timeline tab' : '');
  if (!items.length) { box.innerHTML = `<div class="empty" style="grid-column:1/-1">No alerts for these filters. Continuous recordings are on the Timeline tab.</div>`; return; }
  box.innerHTML = '';
  // Groups (by day, part of the day or hour), each folded away with − and
  // opened again with +.
  const by = recGroup, counts = new Map(), keys = items.map(it => groupOf(it.start, by));
  keys.forEach(k => counts.set(k.key, (counts.get(k.key) || 0) + 1));
  let cur = null;
  items.forEach((it, idx) => {
    const g = keys[idx];
    if (g.key !== cur) {
      cur = g.key;
      const shut = recFolded.has(g.key), n = counts.get(g.key);
      const head = document.createElement('button');
      head.type = 'button'; head.className = 'dayhead fold' + (shut ? ' shut' : ''); head.dataset.g = g.key;
      head.innerHTML = `<span class="pm">${shut ? '+' : '−'}</span>${esc(g.label)} <span class="muted">· ${n} alert${n === 1 ? '' : 's'}</span>`;
      head.onclick = () => { recFolded.has(g.key) ? recFolded.delete(g.key) : recFolded.add(g.key); saveFolded(); renderRecordings(); };
      box.appendChild(head);
    }
    if (recFolded.has(g.key)) return;
    const card = document.createElement('div');
    // Plain motion is never "new": it's there to look up, not to review.
    const isNew = it.kind === 'event' && !it.seen && it.event.labels.length > 0;
    card.className = 'rc' + (isNew ? ' new' : '');
    card.dataset.eid = it.event ? it.event.id : '';
    const dur = (new Date(it.end) - new Date(it.start)) / 1000;
    const tags = it.kind === 'event' ? labelTags(it.event.labels, it.event.motion) : '<span class="tag">recording</span>';
    // Top left: a symbol for each thing found (how sure, in the tip).
    const icons = it.kind === 'event' ? (it.event.labels.length ? [...new Map(it.event.labels.map(l => [l.label, l])).values()].map(l => `<span class="sym ${LABEL_GROUP[l.label] || ''} ${hdClass(l)}" title="${esc(l.label)} ${Math.round(l.score * 100)}% — ${hdTip(l)}">${labelIcon(l.label)}</span>`).join('') : `<span class="sym motion" title="Motion">${labelIcon('motion')}</span>`) : '<span class="tag">recording</span>';
    card.innerHTML = `<div class="thumb">${it.thumb ? `<div class="zw"><img loading="lazy" src="${esc(it.thumb)}" alt="" onerror="this.parentElement.remove()">${boxesHtml(it.event, false)}</div>` : ''}
        <div class="tags">${icons}</div>${isNew ? '<span class="newb">NEW</span>' : ''}
        <div class="tools">${it.download ? `<a class="btn small" href="${esc(it.download)}" title="Download" onclick="event.stopPropagation()">⇩</a>` : ''}<button type="button" class="small op-only" data-a="del" title="Delete">✕</button></div>
        <div class="play">${it.play ? '▶' : it.pending ? '⏳' : ''}</div>
        <div class="cap"><span class="l"><b>${esc(it.camera_name)}</b>${hdAi(it.event)}<span class="ago" data-t="${esc(it.start)}">${cardAgo(it.start)}</span></span><span class="t">${fmtDay(it.start)}<br>${fmtTime(it.start)} · ${fmtDur(dur)}</span></div></div>`;
    card.title = `${it.camera_name} — ${fmtDate(it.start)}${it.size ? ' — ' + fmtBytes(it.size) : ''}${it.event && it.event.notes && it.event.notes.length ? '\n' + it.event.notes.join('\n') : ''}`;
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
  });
  $('rec-collapse').textContent = keys.some(k => !recFolded.has(k.key)) ? '− All' : '+ All';
}
// "New faces & plates to name": the faces and number plates of the chosen
// days that aren't anyone known yet (the same person in several alerts
// shown once). Choose who it is: GAZE names all those alerts and learns it.
async function loadNewFaces() {
  const box = $('rec-cards');
  let inbox = {faces: [], plates: []}, list = [];
  try {
    [inbox, list] = await Promise.all([
      api(`api/known-inbox?from=${enc(rec.from.toISOString())}&to=${enc(rec.to.toISOString())}`),
      api('api/known').catch(() => [])]);
  } catch (e) { toast(e.message, true); }
  rec.items = [];
  const people = list.filter(k => k.kind === 'person'), cars = list.filter(k => k.kind !== 'person');
  const n = inbox.faces.length + inbox.plates.length;
  $('rec-count').textContent = n ? `${inbox.faces.length} new face${inbox.faces.length === 1 ? '' : 's'} · ${inbox.plates.length} new plate${inbox.plates.length === 1 ? '' : 's'}` : '';
  $('rec-collapse').hidden = true;
  if (!n) { box.innerHTML = `<div class="empty" style="grid-column:1/-1">No new faces or number plates for these days. (Turn on faces and plates in Settings → People &amp; vehicles.)</div>`; return; }
  const pick = (face, i) => `<select class="nf-who op-only" data-face="${face ? 1 : 0}" data-i="${i}"><option value="">${face ? 'Who is this?' : 'Whose vehicle?'}</option>${(face ? people : cars).map(k => `<option value="${esc(k.id)}">${esc(k.name)}</option>`).join('')}<option value="+">${face ? 'New person…' : 'New vehicle…'}</option></select>`;
  box.innerHTML = `<div class="nf-grid" style="grid-column:1/-1">`
    + inbox.faces.map((g, i) => `<div class="ib"><img src="${esc(g.picture)}" alt=""><div class="muted small">${g.count} alert${g.count === 1 ? '' : 's'} · ${fmtAgo(g.last)}</div>${pick(true, i)}<button type="button" class="small nf-hide op-only" data-face="1" data-i="${i}">Hide</button></div>`).join('')
    + inbox.plates.map((g, i) => `<div class="ib">${g.picture ? `<img src="${esc(g.picture)}" alt="" class="wide">` : ''}<span class="plate">${esc(g.plate)}</span><div class="muted small">${g.count} alert${g.count === 1 ? '' : 's'} · ${fmtAgo(g.last)}</div>${pick(false, i)}<button type="button" class="small nf-hide op-only" data-face="0" data-i="${i}">Hide</button></div>`).join('')
    + `</div>`;
  box.querySelectorAll('.nf-who').forEach(sel => sel.onchange = async () => {
    let id = sel.value; if (!id) return;
    const face = sel.dataset.face === '1', g = (face ? inbox.faces : inbox.plates)[+sel.dataset.i];
    if (id === '+') {
      const name = (prompt(face ? 'Name of this person:' : 'Name of the vehicle (e.g. Dad’s car):') || '').trim();
      if (!name) { sel.value = ''; return; }
      try { id = (await api('api/known', {method: 'POST', json: {kind: face ? 'person' : 'vehicle', name, plates: []}})).id; } catch (e) { toast(e.message, true); sel.value = ''; return; }
      loadRecordings.who = 0;
    }
    try {
      const r = await api('api/known-inbox/assign', {method: 'POST', json: face ? {known: id, members: g.members} : {known: id, plate: g.plate}});
      toast(r.message); loadRecordings();
    } catch (e) { toast(e.message, true); sel.value = ''; }
  });
  box.querySelectorAll('.nf-hide').forEach(b => b.onclick = async () => {
    const face = b.dataset.face === '1', g = (face ? inbox.faces : inbox.plates)[+b.dataset.i];
    try { await api('api/known-inbox/dismiss', {method: 'POST', json: face ? {members: g.members} : {plate: g.plate}}); loadRecordings(); } catch (e) { toast(e.message, true); }
  });
}

// Alert groups: the day, the part of the day, or the hour.
let recGroup = store.get('rec-group', 'day');
const recFolded = new Set(store.get('rec-folded', []));
function saveFolded() { store.set('rec-folded', [...recFolded].slice(-300)); }
const DAY_PARTS = [[0, 6, 'Night'], [6, 12, 'Morning'], [12, 18, 'Afternoon'], [18, 24, 'Evening']];
function groupOf(t, by) {
  const d = new Date(t), day = fmtDay(t), ymd = `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`, h = d.getHours();
  const hh = (x) => String(x % 24).padStart(2, '0') + ':00';
  if (by === 'hour') return {key: `h:${ymd}:${h}`, label: `${day} · ${hh(h)}–${hh(h + 1)}`};
  if (by === 'part') { const [a, b, name] = DAY_PARTS.find(([a, b]) => h >= a && h < b); return {key: `p:${ymd}:${a}`, label: `${day} · ${name} (${hh(a)}–${hh(b)})`}; }
  return {key: `d:${ymd}`, label: day};
}
$('rec-group').value = recGroup;
$('rec-group').onchange = (e) => { recGroup = e.target.value; store.set('rec-group', recGroup); renderRecordings(); };
// Alerts side by side: Auto (as many as fit, 40 % bigger than before) or
// 2–10 columns — fewer columns, bigger pictures (remembered).
function applyRecCols() {
  const n = +store.get('rec-cols', 0) || 0, box = $('rec-cards');
  box.style.setProperty('--rec-cols', n ? String(n) : 'auto-fill');
  box.style.setProperty('--rec-min', n ? '0px' : '308px');
  $('rec-cols').value = n ? String(n) : '';
}
// What the badges and symbols on the alert cards mean. Objects: only the
// kinds these cameras look for. HD AI: only when the HD check is on.
function renderRecKey() {
  const cams = (status && status.cameras) || [];
  const objs = [...new Set(cams.flatMap(c => c.enabled && c.ai ? c.watch || [] : []))];
  const hd = true;
  const row = (k, d) => `<div><span class="k">${k}</span><span class="d">${d}</span></div>`;
  const ex = objs[0] || 'person';
  $('rec-key').innerHTML = [
    row('<span class="tag newtag">NEW</span><span class="newk"></span>', '<b>New</b> (top right) — an alert you haven\u2019t opened yet. It has a blue outline until you play it.'),
    row(`<span class="sym">${labelIcon(ex)}</span>`, '<b>What the AI found</b> (top left). Hover over it for the name and how sure the AI is.'),
    objs.length ? row(`<span class="sym">${objs.map(labelIcon).join(' ')}</span>`, `<b>Objects</b> these cameras look for: ${objs.map(esc).join(', ')}.`) : '',
    row(`<span class="tag motion">${labelIcon('motion')}</span>`, '<b>Motion only</b> — something moved, but no object was recognised.'),
    row('<span class="box"></span>', '<b>Green box</b> — where the object was in the picture.'),
    hd ? row('<span class="hdai">HD AI ✓</span>', '<b>Confirmed in HD</b> — the find from the smaller stream was checked again in a full-quality picture and is real.') : '',
    hd ? row('<span class="hdai no">HD AI ?</span>', '<b>Not confirmed in HD</b> — the full-quality picture didn\u2019t show it clearly. Often a shadow, reflection or something far away: worth a quick look.') : '',
    hd ? row(`<span class="tag unconfirmed">${labelIcon(ex)}</span>`, '<b>Faded, dashed symbol</b> — that object wasn\u2019t confirmed in HD.') : '',
    hd ? row('<span class="muted small">no badge</span>', '<b>Not checked in HD</b> — found in the full-quality stream already, motion only, or the HD check was off.') : '',
    row('<span class="sym">▶</span>', '<b>Play</b> — the alert\u2019s video is ready.'),
    row('<span class="sym">⏳</span>', '<b>Still recording</b> — the clip is ready a few seconds after the activity stops.'),
    row('<span class="sym">⇩ ✕</span>', '<b>Download</b> the clip, or <b>delete</b> the alert (when you hover over a card).'),
    row('<span class="muted small">5 min ago</span>', '<b>How long ago</b> (bottom left, under the camera name).'),
    row('<span class="muted small">07:01:43 · 6s</span>', '<b>Date and time</b> it started and how long the activity lasted (bottom right).'),
  ].join('');
}
$('rec-key-btn').onclick = () => { const k = $('rec-key'); k.hidden = !k.hidden; store.set('rec-key', !k.hidden); if (!k.hidden) renderRecKey(); };
if (store.get('rec-key', false)) { $('rec-key').hidden = false; setTimeout(renderRecKey, 1500); }
$('rec-cols').onchange = (e) => { store.set('rec-cols', +e.target.value || 0); applyRecCols(); };
applyRecCols();
$('rec-collapse').onclick = () => {
  const keys = [...new Set(rec.items.map(it => groupOf(it.start, recGroup).key))];
  if (keys.some(k => !recFolded.has(k))) keys.forEach(k => recFolded.add(k)); else keys.forEach(k => recFolded.delete(k));
  saveFolded(); renderRecordings();
};

// Known people, for "Who is this?".
let knownPeople = [];
async function loadKnownPeople() { knownPeople = (await api('api/known').catch(() => [])).filter(k => k.kind === 'person'); }
// Alerts open in a pop-up; the list stays where it is.
let alertIdx = -1;
async function playItem(it, card) {
  if (!it.play) return noVideo(it);
  if (it.event && it.event.faces && it.event.faces.length) await loadKnownPeople();
  alertIdx = rec.items.indexOf(it);
  document.querySelectorAll('.rc.active').forEach(c => c.classList.remove('active'));
  if (card) card.classList.add('active');
  const dlg = $('alert-dialog');
  if (!dlg.open) dlg.showModal();
  if (!recPlayer) recPlayer = new Player($('rp-video'), $('rp-controls'), $('rp-msg'));
  recPlayer.setHidden(false);
  recPlayer.load({...it.play, camera: it.camera, marks: it.event ? [it.event.start] : []});
  const dur = (new Date(it.end) - new Date(it.start)) / 1000;
  const e = it.event;
  $('ad-title').textContent = `${it.camera_name} — ${fmtDate(it.start)}`;
  $('ad-sharebox').hidden = true;
  $('ad-share').hidden = !it.thumb;
  $('ad-prev').disabled = alertIdx <= 0; $('ad-next').disabled = alertIdx < 0 || alertIdx >= rec.items.length - 1;
  $('rp-info').innerHTML = `
    ${e && e.suspicious ? `<div class="sus">⚠ ${esc(e.suspicious)}</div>` : ''}
    ${e && e.notes && e.notes.length ? `<div class="what"><div class="muted small">Seen</div>${e.notes.map(n => esc(n)).join('<br>')}</div>` : ''}
    ${e && it.thumb ? `<div class="review"><img src="${esc(it.thumb)}" alt="">${boxesHtml(e, true)}</div>
      ${e.boxes && e.boxes.length ? `<label class="check" style="margin:-2px 0 8px"><input type="checkbox" class="bx-toggle" ${showBoxes ? 'checked' : ''}> Show detection boxes</label>` : ''}` : ''}
    ${e && e.faces && e.faces.length ? `<div class="faces"><div class="muted small">Faces</div>${e.faces.map((f, i) => `<div class="face"><img src="media/${enc(it.camera)}/events/${enc(f.file)}" alt="">
      ${f.name ? `<b>${esc(f.name)}</b>${f.confirmed ? '<span class="muted small" title="Added to this person’s faces">✓ confirmed</span>' : `<button type="button" class="op-only face-ok" data-face="${i}" title="Right person: add this face to ${esc(f.name)}’s faces, so GAZE knows them better">✓ Right</button>`}<button type="button" class="op-only face-fix" data-face="${i}" title="Wrong person? Choose who it really is, or make a new person">✎ Wrong?</button>` : ''}
      <select class="who op-only" data-face="${i}" ${f.name ? 'hidden' : ''}><option value="">${f.name ? `Not ${esc(f.name)}? Who is it?` : 'Who is this?'}</option>${knownPeople.filter(k => k.name !== f.name).map(k => `<option value="${esc(k.id)}">${esc(k.name)}</option>`).join('')}<option value="+">New person…</option>${f.name ? '<option value="none">Not anyone I know</option>' : ''}</select></div>`).join('')}</div>` : ''}
    ${e && e.items && e.items.length ? `<div class="items">${e.items.map(x => `<div class="item"><div class="muted small">${esc(x.zone)}: before → after</div><div class="row"><img src="media/${enc(it.camera)}/events/${enc(x.before)}" alt=""><span>→</span><img src="media/${enc(it.camera)}/events/${enc(x.after)}" alt=""></div></div>`).join('')}</div>` : ''}
    ${e && e.plates && e.plates.length ? `<div class="plates">${e.plates.map(p => `<span class="plate">${esc(p)}</span>`).join('')}</div>` : ''}
    <dl><dt>Camera</dt><dd>${esc(it.camera_name)}</dd><dt>Time</dt><dd>${fmtDate(it.start)}</dd><dt>Length</dt><dd>${fmtDur(dur)}</dd>
    ${e ? `<dt>Detected</dt><dd>${labelTags(e.labels, e.motion) || '–'}</dd><dt>Video</dt><dd>${e.clip ? 'Full-quality clip' : 'From continuous recording'}</dd>` : ''}
    ${it.size ? `<dt>Size</dt><dd>${fmtBytes(it.size)}</dd>` : ''}</dl>
    <div class="actions"><a class="btn small" href="#recordings/${enc(it.camera)}/timeline" onclick="tl.jump='${it.start}'">Open in timeline</a></div>`;
  const t = $('rp-info').querySelector('.bx-toggle');
  if (t) t.onchange = () => { showBoxes = t.checked; store.set('show-boxes', showBoxes); document.body.classList.toggle('no-boxes', !showBoxes); };
  // "Who is this?": name a face; GAZE learns it.
  // A face named wrongly: show the choice of who it really is.
  $('rp-info').querySelectorAll('.face-fix').forEach(b => b.onclick = () => {
    const sel = $('rp-info').querySelector(`.who[data-face="${b.dataset.face}"]`);
    sel.hidden = false; b.hidden = true; sel.focus();
  });
  // The right person: confirm it (the face joins their profile).
  $('rp-info').querySelectorAll('.face-ok').forEach(b => b.onclick = async () => {
    const f = e.faces[+b.dataset.face], k = knownPeople.find(x => x.name === f.name);
    if (!k) { toast(`${f.name} isn't in People & vehicles any more`, true); return; }
    try {
      const r = await api(`api/known/${enc(k.id)}/learn`, {method: 'POST', json: {camera: it.camera, event: e.id, face: +b.dataset.face}});
      toast(r.message); f.confirmed = true; if (r.notes) e.notes = r.notes; playItem(it, card);
    } catch (err) { toast(err.message, true); }
  });
  $('rp-info').querySelectorAll('.who').forEach(sel => sel.onchange = async () => {
    let id = sel.value; if (!id) return;
    if (id === '+') {
      const name = (prompt('Name of this person:') || '').trim(); if (!name) { sel.value = ''; return; }
      try { id = (await api('api/known', {method: 'POST', json: {kind: 'person', name}})).id; } catch (err) { toast(err.message, true); return; }
    }
    try {
      const r = await api(`api/known/${enc(id)}/learn`, {method: 'POST', json: {camera: it.camera, event: e.id, face: +sel.dataset.face}});
      toast(r.message); await loadKnownPeople();
      e.faces[+sel.dataset.face].name = id === 'none' ? null : (knownPeople.find(k => k.id === id) || {}).name;
      e.faces[+sel.dataset.face].confirmed = true;
      // Keep the alert's notes in step (what the server now has).
      if (r.notes) e.notes = r.notes;
      playItem(it, card);
    } catch (err) { toast(err.message, true); }
  });
  // Viewed.
  if (e && !it.seen) {
    it.seen = true;
    if (card) { card.classList.remove('new'); const n = card.querySelector('.newb'); if (n) n.remove(); }
    api('api/alerts/seen', {method: 'POST', json: {ids: [e.id]}}).then(refreshStatus).catch(() => {});
  }
}
// Safe mode: fix the settings file by hand (GAZE checks it before saving).
async function openSafeEditor() {
  let r; try { r = await api('api/safe-mode/config'); } catch (e) { toast(e.message, true); return; }
  const dlg = document.createElement('dialog'); dlg.className = 'wn-dlg safe-ed';
  dlg.innerHTML = `<div class="head"><h2>Fix the settings file</h2><span class="spacer"></span><button type="button" class="small" data-x>✕</button></div>
    <div class="body"><p class="errc" style="margin-top:0">${esc(r.problem || '')}</p>
      <p class="muted small">${esc(r.path)} — correct the line the problem points to. GAZE checks the file before saving it, keeps the broken one as nvr.toml.broken, and restarts.</p>
      <textarea id="sm-text" spellcheck="false">${esc(r.text)}</textarea>
      <div class="actions"><button type="button" data-x>Cancel</button><button type="button" class="primary" id="sm-save">Check and save</button></div></div>`;
  document.body.appendChild(dlg); dlg.showModal();
  dlg.querySelectorAll('[data-x]').forEach(b => b.onclick = () => { dlg.close(); dlg.remove(); });
  $('sm-save').onclick = async () => {
    try { const x = await api('api/safe-mode/config', {method: 'POST', json: {text: val('sm-text')}}); toast(x.message); dlg.close(); dlg.remove(); setTimeout(() => location.reload(), 6000); }
    catch (e) { toast(e.message, true); }
  };
}

// ---------- a time-lapse video of a camera's day ----------
$('f-tlx').onclick = () => {
  const cams = status.cameras.filter(c => !rec.cameras.size || rec.cameras.has(c.id));
  const day = startOfDay(tl.day ? new Date(tl.day) : new Date());
  const p2 = (n) => String(n).padStart(2, '0'), local = (d) => `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())}T${p2(d.getHours())}:${p2(d.getMinutes())}`;
  const dlg = document.createElement('dialog'); dlg.className = 'wn-dlg';
  dlg.innerHTML = `<div class="head"><h2>🎞 Time-lapse video</h2><span class="spacer"></span><button type="button" class="small" data-x>✕</button></div>
    <div class="body form">
      <label for="tx-cam">Camera</label><div><select id="tx-cam">${cams.map(c => `<option value="${esc(c.id)}">${esc(c.name)}</option>`).join('')}</select></div>
      <label>Quick</label><div class="row tx-quick">${[24, 12, 6, 3, 1].map(h => `<button type="button" class="small" data-h="${h}">Last ${h} hour${h === 1 ? '' : 's'}</button>`).join('')}</div>
      <label for="tx-from">From</label><div><input type="datetime-local" id="tx-from" value="${local(day)}"></div>
      <label for="tx-to">To</label><div><input type="datetime-local" id="tx-to" value="${local(new Date(Math.min(Date.now(), day.getTime() + 86400000)))}"></div>
      <label for="tx-fps">Speed${tip('How many time-lapse pictures are shown each second. With one picture every 10 seconds (the standard), 30 per second turns a whole day into about 5 minutes; 60 into about 2½.')}</label><div><select id="tx-fps"><option value="10">Slow (10 pictures a second)</option><option value="30" selected>Normal (30 a second)</option><option value="60">Fast (60 a second)</option></select><div class="hint" id="tx-len"></div></div>
      <span></span><div class="actions" style="margin:0"><button type="button" data-x>Cancel</button><a class="btn primary" id="tx-go">⇩ Download video</a></div></div>`;
  document.body.appendChild(dlg); dlg.showModal();
  const close = () => { dlg.close(); dlg.remove(); };
  dlg.querySelectorAll('[data-x]').forEach(b => b.onclick = close);
  const upd = () => {
    const a = new Date(val('tx-from')), b = new Date(val('tx-to')), secs = (b - a) / 1000;
    const pics = secs / (status.timelapse_seconds || 10), len = pics / (+val('tx-fps') || 30);
    $('tx-len').textContent = secs > 0 ? `About ${fmtDur(len)} of video` : 'The end must be after the start';
    $('tx-go').href = `api/timelapse-export?camera=${enc(val('tx-cam'))}&from=${enc(a.toISOString())}&to=${enc(b.toISOString())}&fps=${val('tx-fps')}`;
  };
  ['tx-cam', 'tx-from', 'tx-to', 'tx-fps'].forEach(i => $(i).oninput = upd); upd();
  // Last 24 / 12 / 6 / 3 / 1 hours, up to now.
  dlg.querySelectorAll('.tx-quick [data-h]').forEach(b => b.onclick = () => {
    const now = new Date();
    $('tx-from').value = local(new Date(now - b.dataset.h * 3600e3)); $('tx-to').value = local(now);
    dlg.querySelectorAll('.tx-quick button').forEach(x => x.classList.toggle('primary', x === b));
    upd();
  });
  $('tx-go').onclick = () => { toast('Making the video — it downloads in a moment'); setTimeout(close, 300); };
};

// ---------- sharing an alert ----------
// The system share sheet (Messenger, WhatsApp, Facebook, email… whatever is
// on the phone or computer) when the browser offers it with files — it
// needs a secure address (https, or GAZE opened on this computer). Apps that
// can't be handed a file from a web page get the picture saved first and
// open with the message filled in.
let shareIt = null;
const SHARE_NAMES = {person: 'Person', vehicle: 'Vehicle', car: 'Car', truck: 'Truck', bus: 'Bus', motorcycle: 'Motorbike', bicycle: 'Bicycle', dog: 'Dog', cat: 'Cat', bird: 'Bird', package: 'Parcel'};
function shareWhat(it) {
  const e = it.event || {}, kinds = [...new Set((e.labels || []).map(l => l.label))];
  const names = kinds.map(k => SHARE_NAMES[k] || k[0].toUpperCase() + k.slice(1));
  const what = names.length ? names.join(' and ') : 'Movement';
  return {what, icon: kinds.length ? labelIcon(kinds[0]) : labelIcon('motion')};
}
function shareText(it) {
  const e = it.event || {}, {what, icon} = shareWhat(it), dur = (new Date(it.end) - new Date(it.start)) / 1000;
  const lines = [`${icon} ${what} at ${it.camera_name}`, fmtDate(it.start) + (dur >= 1 ? ` (${fmtDur(dur)})` : '')];
  for (const n of e.notes || []) lines.push(n);
  if (e.plates && e.plates.length) lines.push('Number plate: ' + e.plates.join(', '));
  return lines.join('\n');
}
const shareStamp = (d) => { d = new Date(d); const p = (n) => String(n).padStart(2, '0'); return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}-${p(d.getMinutes())}-${p(d.getSeconds())}`; };
const shareFileName = (it, ext) => `${it.camera_name.replace(/[\\/:*?"<>|]+/g, ' ').trim()} ${shareStamp(it.start)}.${ext}`;
// The alert picture with its boxes and a caption bar: makes sense on its own
// in a chat.
async function sharePicture(it) {
  const img = new Image();
  img.src = it.thumb;
  await img.decode();
  const w = img.naturalWidth, h = img.naturalHeight, bar = Math.max(44, Math.round(h * 0.09));
  const cv = document.createElement('canvas'); cv.width = w; cv.height = h + bar;
  const g = cv.getContext('2d');
  g.drawImage(img, 0, 0, w, h);
  const e = it.event || {};
  if (showBoxes) for (const b of e.boxes || []) {
    if (b.label === 'motion') continue;
    g.strokeStyle = '#22c55e'; g.lineWidth = Math.max(2, w / 400);
    g.strokeRect(b.x * w, b.y * h, b.w * w, b.h * h);
  }
  g.fillStyle = '#0f1218'; g.fillRect(0, h, w, bar);
  const {what, icon} = shareWhat(it), fs = Math.round(bar * 0.42);
  g.fillStyle = '#fff'; g.font = `600 ${fs}px system-ui, sans-serif`; g.textBaseline = 'middle';
  g.fillText(`${icon} ${what} · ${it.camera_name}`, bar * 0.3, h + bar / 2);
  g.font = `${Math.round(fs * 0.9)}px system-ui, sans-serif`; g.fillStyle = '#c9d1e0'; g.textAlign = 'right';
  g.fillText(fmtDate(it.start), w - bar * 0.3, h + bar / 2);
  const blob = await new Promise(r => cv.toBlob(r, 'image/jpeg', 0.9));
  return new File([blob], shareFileName(it, 'jpg'), {type: 'image/jpeg'});
}
// The alert's own clip, or (recorded 24/7) its part of the recording: from
// a few seconds before to a little after.
function alertVideoUrl(it) {
  if (it.download) return it.download;
  const a = new Date(it.start).getTime(), b = new Date(it.end).getTime();
  return fsClipHref(it.camera, a - 5000, Math.min(Date.now(), Math.max(b + 5000, a + 15000)));
}
async function shareVideo(it) {
  const r = await fetch(alertVideoUrl(it));
  if (!r.ok) throw new Error('The video could not be loaded');
  const blob = await r.blob();
  return new File([blob], shareFileName(it, 'mp4'), {type: 'video/mp4'});
}
const shareLink = (it) => it.event ? `${location.origin}${location.pathname}#alerts?open=${enc(it.camera)}~${enc(it.event.id)}` : '';
function saveFile(file) {
  const a = document.createElement('a'); a.href = URL.createObjectURL(file); a.download = file.name;
  document.body.appendChild(a); a.click(); a.remove(); setTimeout(() => URL.revokeObjectURL(a.href), 60000);
}
const canShareFiles = (type) => { try { return !!(navigator.canShare && navigator.canShare({files: [new File([new Blob(['x'])], 'x.' + (type === 'video/mp4' ? 'mp4' : 'jpg'), {type})]})); } catch (_) { return false; } };
const mobileOS = () => /Android|iPhone|iPad|iPod/i.test(navigator.userAgent);
function openShare(it) {
  shareIt = it;
  const box = $('ad-sharebox'), text = shareText(it), sheet = canShareFiles('image/jpeg'), sheetVid = !!it.play && canShareFiles('video/mp4');
  const copyPic = !!(navigator.clipboard && window.ClipboardItem && window.isSecureContext);
  const app = (k, icon, name, tipText) => `<button type="button" class="share-app" data-app="${k}" title="${esc(tipText)}"><span class="si">${icon}</span>${name}</button>`;
  box.innerHTML = `<div class="row" style="align-items:flex-start"><div style="flex:1"><b>Share this alert</b><div class="share-msg">${esc(text).replace(/\n/g, '<br>')}</div></div><button type="button" class="small" id="sh-x" title="Close">✕</button></div>
    ${sheet ? `<div class="row share-main"><button type="button" class="primary" id="sh-pic">📤 Share picture…</button>${sheetVid ? '<button type="button" class="primary" id="sh-vid">🎬 Share video…</button>' : ''}<span class="muted small">Opens this ${mobileOS() ? 'phone' : 'computer'}'s share menu: Messenger, WhatsApp, Facebook, email and any other app you have.</span></div>` : ''}
    <div class="share-apps">
      ${app('email', '✉️', 'Email', 'Saves the picture, then opens a new email with the details filled in: attach the picture from your Downloads')}
      ${app('whatsapp', '🟢', 'WhatsApp', 'Saves the picture, then opens WhatsApp with the message filled in')}
      ${app('messenger', '💬', 'Messenger', 'Saves the picture, then opens Messenger (the message is copied: paste it)')}
      ${app('facebook', '📘', 'Facebook', 'Saves the picture, then opens Facebook to post it (the message is copied: paste it)')}
      ${app('telegram', '✈️', 'Telegram', 'Saves the picture, then opens Telegram with the message filled in')}
      ${app('sms', '📱', 'Text message', 'Opens a text message with the details filled in (on a phone)')}
    </div>
    <div class="row share-more">
      ${copyPic ? '<button type="button" class="small" id="sh-copypic" title="Then paste it into any chat or email">📋 Copy picture</button>' : ''}
      <button type="button" class="small" id="sh-copytext">📋 Copy message</button>
      ${it.event ? `<button type="button" class="small" id="sh-link" title="A link that opens this alert in GAZE — for people in your home who have a GAZE account">🔗 Copy GAZE link</button>` : ''}
      <span class="spacer"></span>
      <button type="button" class="small" id="sh-savepic">⇩ Save picture</button>
      ${it.play ? '<button type="button" class="small" id="sh-savevid">⇩ Save video</button>' : ''}
    </div>
    ${sheet ? '' : `<div class="hint" style="margin:6px 0 0">${window.isSecureContext ? 'This browser can\u2019t hand pictures straight to other apps, so the buttons above save the picture first: attach it from your Downloads.' : 'Sending the picture straight into an app (the share menu) only works when GAZE is opened through a secure https address \u2014 the buttons above save it first instead: attach it from your Downloads.'}</div>`}`;
  box.hidden = false;
  const busy = async (btn, label, fn) => {
    const was = btn.innerHTML; btn.disabled = true; btn.textContent = label;
    try { await fn(); } catch (err) { if (err && err.name !== 'AbortError') toast(err.message || String(err), true); }
    btn.disabled = false; btn.innerHTML = was;
  };
  const copy = async (t) => { try { await navigator.clipboard.writeText(t); return true; } catch (_) {
    const ta = document.createElement('textarea'); ta.value = t; document.body.appendChild(ta); ta.select();
    const ok = document.execCommand('copy'); ta.remove(); return ok; } };
  $('sh-x').onclick = () => { box.hidden = true; };
  const q = (id) => $(id);
  if (q('sh-pic')) q('sh-pic').onclick = (ev) => busy(ev.currentTarget, 'Preparing…', async () => navigator.share({files: [await sharePicture(it)], title: shareWhat(it).what + ' at ' + it.camera_name, text}));
  if (q('sh-vid')) q('sh-vid').onclick = (ev) => busy(ev.currentTarget, 'Loading video…', async () => navigator.share({files: [await shareVideo(it)], title: shareWhat(it).what + ' at ' + it.camera_name, text}));
  if (q('sh-copypic')) q('sh-copypic').onclick = (ev) => busy(ev.currentTarget, 'Copying…', async () => {
    const f = await sharePicture(it);
    // Clipboards take PNG pictures.
    const bmp = await createImageBitmap(f), cv = document.createElement('canvas'); cv.width = bmp.width; cv.height = bmp.height; cv.getContext('2d').drawImage(bmp, 0, 0);
    const png = await new Promise(r => cv.toBlob(r, 'image/png'));
    await navigator.clipboard.write([new ClipboardItem({'image/png': png})]);
    toast('Picture copied — paste it into any chat or email');
  });
  q('sh-copytext').onclick = async () => toast(await copy(text) ? 'Message copied' : 'Could not copy', false);
  if (q('sh-link')) q('sh-link').onclick = async () => toast(await copy(shareLink(it)) ? 'Link copied — it opens this alert for anyone signed in to GAZE' : 'Could not copy');
  q('sh-savepic').onclick = (ev) => busy(ev.currentTarget, 'Saving…', async () => { saveFile(await sharePicture(it)); toast('Picture saved to your Downloads'); });
  if (q('sh-savevid')) q('sh-savevid').onclick = () => { const a = document.createElement('a'); a.href = alertVideoUrl(it); a.download = shareFileName(it, 'mp4'); document.body.appendChild(a); a.click(); a.remove(); };
  box.querySelectorAll('.share-app').forEach(b => b.onclick = () => busy(b, 'Opening…', async () => {
    const k = b.dataset.app, subject = `${shareWhat(it).what} at ${it.camera_name} — ${fmtDate(it.start)}`, t = encodeURIComponent(text);
    // The picture goes along wherever the app can't take it from a web page.
    if (k !== 'sms') saveFile(await sharePicture(it));
    const open = (url) => { const w = window.open(url, '_blank', 'noopener'); if (!w && !url.startsWith('http')) location.href = url; };
    if (k === 'email') { location.href = `mailto:?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(text + '\n\n(The picture is attached.)')}`; toast('Picture saved — attach it from your Downloads'); }
    else if (k === 'whatsapp') { open(`https://wa.me/?text=${t}`); toast('Picture saved — attach it in WhatsApp with 📎'); }
    else if (k === 'telegram') { open(`https://t.me/share/url?url=${encodeURIComponent(' ')}&text=${t}`); toast('Picture saved — attach it in Telegram with 📎'); }
    else if (k === 'messenger') { await copy(text); open(mobileOS() ? 'fb-messenger://' : 'https://www.messenger.com/'); toast('Picture saved and message copied — choose a chat, paste, and attach the picture'); }
    else if (k === 'facebook') { await copy(text); open('https://www.facebook.com/'); toast('Picture saved and message copied — start a post, paste, and add the picture'); }
    else if (k === 'sms') { location.href = `sms:?&body=${t}`; }
  }));
}
$('ad-share').onclick = () => { const b = $('ad-sharebox'); if (!b.hidden && shareIt === rec.items[alertIdx]) { b.hidden = true; return; } const it = rec.items[alertIdx]; if (it) openShare(it); };

function closeAlert() {
  const dlg = $('alert-dialog');
  if (recPlayer) recPlayer.stop();
  if (dlg.open) dlg.close();
  document.querySelectorAll('.rc.active').forEach(c => c.classList.remove('active'));
}
function stepAlert(d) {
  const it = rec.items[alertIdx + d];
  if (!it) return;
  const card = [...document.querySelectorAll('#rec-cards .rc')][alertIdx + d];
  playItem(it, card);
}
$('ad-close').onclick = closeAlert;
$('alert-dialog').addEventListener('close', () => { if (recPlayer) recPlayer.stop(); });
$('alert-dialog').addEventListener('click', (e) => { if (e.target === $('alert-dialog')) closeAlert(); });
$('ad-prev').onclick = () => stepAlert(-1);
$('ad-next').onclick = () => stepAlert(1);
$('ad-unseen').onclick = async () => {
  const it = rec.items[alertIdx]; if (!it || !it.event) return;
  await api('api/alerts/unseen', {method: 'POST', json: {camera: it.camera, id: it.event.id}}).catch(e => toast(e.message, true));
  it.seen = false; renderRecordings(); refreshStatus(); toast('Marked as new');
};
document.addEventListener('keydown', (e) => {
  if (!$('alert-dialog').open || e.target.closest('input, select, textarea')) return;
  if (e.key === 'ArrowLeft') stepAlert(-1); else if (e.key === 'ArrowRight') stepAlert(1);
});

// ---------- multi-camera timeline ----------
const tl = {badClips: new Set(), sound: store.get('tl-sound', false), soundCam: null, live: false, scrub: TIMELAPSE ? 'timelapse' : 'sub', quality: store.get('tl-quality', 'sub'), data: [], T: null, playing: false, speed: 1, zoom: 3600, winStart: null, inT: null, outT: null, sprites: new Map(), last: 0, jump: null, day: null};

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
  tl.key = tlKey();
  drawTimeline();
  tlSync(true);
  prefetchSprites();
}
function centerWindow() { tl.winStart = tl.T - tl.zoom / 2 * 1000; }

// A Recordings picture's on-screen display: the same as live (the camera's
// display settings), with the recording's time, ▶ PLAYBACK, and its sound.
function tlOverlay(c) {
  const tools = '<button type="button" class="small solo-off" hidden>▦ All cameras</button><button type="button" class="fsbtn tl-snd" title="Hear this camera">🔈</button>';
  const sc = cam(c.camera);
  if (!sc) return `<div class="ov tl"><span class="ovtext name">${esc(c.name)}</span><span class="ovtext tl-t"></span></div><div class="ov tr">${tools}<button type="button" class="fsbtn fs-go">⛶</button></div>`;
  return overlayHtml(sc, null, false, {rec: true, tools});
}
function buildTimelineVideos() {
  const box = $('tl-videos');
  tlLeaveLive();
  box.querySelectorAll('.surface').forEach(el => { if (el._h) { el._h.hls.destroy(); el._h = null; } });
  box.innerHTML = tl.data.length ? '' : '<div class="empty">No cameras selected.</div>';
  for (const c of tl.data) {
    const el = document.createElement('div');
    el.className = 'surface';
    el.dataset.cam = c.camera;
    // Two players per camera: one plays while the next file loads in the
    // other, so moving from file to file doesn't blank the picture.
    el.innerHTML = `<video playsinline muted class="vid"></video><video playsinline muted class="vid" style="position:absolute;inset:0;visibility:hidden"></video><video playsinline muted class="vid" data-hls style="position:absolute;inset:0;visibility:hidden"></video>
      <div class="sprite" hidden></div><div class="msg" hidden>No recorded footage at this time</div>
      ${tlOverlay(c)}`;
    el.querySelectorAll('.fs-go').forEach(b => b.onclick = (e) => { e.stopPropagation(); fullscreen(el); });
    el.querySelector('.tl-snd').onclick = (e) => {
      e.stopPropagation();
      const on = !(tl.sound && tlSoundCam() === c.camera);
      tl.soundCam = c.camera; tl.sound = on; store.set('tl-sound', on); tlApplySound();
    };
    el.querySelector('.solo-off').onclick = (e) => { e.stopPropagation(); tlSolo(null); };
    el._vids = [...el.querySelectorAll('video:not([data-hls])')]; el._vids.forEach(watchFps);
    // An alert clip this browser can't play (e.g. H.265): the sub-stream
    // stays on instead.
    el._vids.forEach(v => v.addEventListener('error', () => {
      const u = v.dataset.url;
      if (u && tl.data.some(c => c.segments.some(x => x.kind === 'clip' && x.url === u))) { tl.badClips.add(u); delete v.dataset.url; tlSync(true); }
    }));
    el._hlsV = el.querySelector('video[data-hls]'); watchFps(el._hlsV);
    // The continuous player is shown once it has the picture (until then
    // the last picture stays).
    for (const ev of ['seeked', 'loadeddata', 'playing', 'canplay'])
      el._hlsV.addEventListener(ev, () => { if (el._mode === 'hls' && el._hlsV.readyState >= 2 && !el._hlsV.seeking) showHls(el); });
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
// ---------- Continuous playback (HLS) ----------
// Each camera's recordings are played as one stream (the server indexes
// the pieces of every file, see hls.rs): only the part needed is fetched,
// the player buffers ahead, and it goes from one file to the next without
// a break. The file-by-file players are kept for HD alert clips, converted
// playback, and browsers that can't play the stream.
let hlsLib;  // undefined: not loaded yet; 0: loading; false: not usable
let hlsLoaded;  // settles once it's known (loaded or not usable)
function hlsReady() {
  if (hlsLib === undefined) {
    hlsLib = 0;
    hlsLoaded = new Promise(done => {
      const sc = document.createElement('script'); sc.src = 'hls.min.js';
      sc.onload = () => { hlsLib = window.Hls && Hls.isSupported() ? Hls : false; done(); if (hlsLib && page === 'recordings') tlSync(true); };
      sc.onerror = () => { hlsLib = false; done(); };
      document.head.appendChild(sc);
    });
  }
  return hlsLib || null;
}
// Opening GAZE (or refreshing the page) anywhere but Recordings: its
// timeline is loaded in the background — footage, each camera's stream at
// the moment it opens on, and the time-lapse pictures — so it's ready when
// you go there (it then carries on as loaded, see resumeTimeline). Leaving
// for another page stops its downloads as usual.
async function preloadRecordings() {
  // (Not with the data saver on: nothing is fetched until it's asked for.)
  if (page === 'recordings' || tl.key || document.hidden || dataSaver) return;
  hlsReady(); await hlsLoaded;
  if (page === 'recordings' || tl.key) return;
  const mode = rec.mode;
  rec.mode = 'timeline';
  try { await loadTimeline(); } finally { if (page !== 'recordings') rec.mode = mode; }
}
const HLS_WIN = 90 * 60000;  // a playlist covers 1½ h either side
function hlsFor(el, cam) {
  if (el._h) return el._h;
  const hls = new hlsLib({autoStartLoad: false, maxBufferLength: 20, maxMaxBufferLength: 60, backBufferLength: 30,
    startFragPrefetch: true, enableWorker: true, fragLoadingMaxRetry: 2, manifestLoadingMaxRetry: 1, levelLoadingMaxRetry: 1});
  hls.attachMedia(el._hlsV);
  const h = el._h = {hls, cam, frags: [], ready: false};
  hls.on(hlsLib.Events.LEVEL_LOADED, (_, d) => {
    h.frags = d.details.fragments.map(f => ({w: f.programDateTime, d: f.duration * 1000, m: f.start}));
    h.loadedAt = Date.now();
    // A growing (live) playlist comes again every few seconds with the
    // newest pieces: the player carries on as it is.
    if (h.ready) return;
    h.ready = true; h.started = false;
    tlSync(true);
  });
  hls.on(hlsLib.Events.ERROR, (_, d) => {
    if (!d.fatal) return;
    // Can't be played this way (e.g. a picture or sound format the browser
    // can't stream): this camera goes back to playing the files.
    console.warn('continuous playback off for', cam, d.details);
    // (Tried again after a minute: it may have been a passing fault.)
    hls.destroy(); el._h = null; el._noHls = Date.now(); leaveHls(el); tlSync(true);
  });
  return h;
}
function hlsLoad(h, T, stream) {
  const now = Date.now();
  // Reaching now: a growing playlist that keeps up with the recording
  // (asked again by the player every few seconds; nothing starts over).
  h.from = T - HLS_WIN; h.to = T + HLS_WIN; h.live = h.to > now;
  h.stream = stream; h.ready = false; h.frags = []; h.reqAt = now; h.started = false;
  h.hls.stopLoad();
  h.hls.loadSource(`api/hls/${enc(h.cam)}/${stream}.m3u8?from=${new Date(h.from).toISOString()}&to=${new Date(h.to).toISOString()}${h.live ? '&live=true' : ''}`);
}
// Wall time → time in the stream (a moment between two files: the start of
// the next one, if it's close; null when it's not in the stream).
function hlsAt(h, W) {
  const f = h.frags; let lo = 0, hi = f.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (f[mid].w + f[mid].d <= W) lo = mid + 1; else if (f[mid].w > W) hi = mid - 1; else return {m: f[mid].m + (W - f[mid].w) / 1000};
  }
  const nx = f[lo];
  return nx && nx.w - W < 15000 && lo > 0 ? {m: nx.m, gap: true} : null;
}
function showHls(el) {
  el._hlsBehind = false;
  el._hlsV.style.visibility = '';
  el._vids.forEach(v => { v.style.visibility = 'hidden'; if (!v.paused) v.pause(); });
  tlApplySound();
}
function leaveHls(el, soft) {
  if (el._mode !== 'hls') return;
  el._mode = 'file';
  // Into an alert's HD clip: the stream stays on screen until the clip has
  // a picture, and keeps going underneath, to take over again without a
  // break when the alert is over.
  if (soft) { el._hlsBehind = true; return; }
  el._hlsBehind = false;
  el._hlsV.pause(); el._hlsV.style.visibility = 'hidden';
  if (el._h) { el._h.hls.stopLoad(); el._h.started = false; }
  el._vids.forEach(v => delete v.dataset.url);  // the files load afresh at the right time
}
// Load the file coming up (an alert's HD clip) in the spare player, set
// to the moment it starts, so switching to it is instant.
function tlPreload(el, c, from) {
  const nx = segAt(c, from);
  if (!nx || nx.kind !== 'clip') return;
  const other = el._vids[1 - el._active];
  if (other.dataset.url === nx.url) return;
  other.dataset.url = nx.url; other.preload = 'auto'; other.src = nx.url;
  const at = Math.max(0, (from - new Date(nx.start).getTime()) / 1000);
  other.addEventListener('loadedmetadata', () => { other.currentTime = at; }, {once: true});
}
// Play camera `el` at tl.T with the continuous stream; false: play the file.
function tlHls(el, c, s, force) {
  const hd = tl.hd && tl.hd.camera === c.camera && tl.T >= tl.hd.from - 2000 && tl.T < tl.hd.until;
  // (Converted playback is for files this browser can't play, e.g. H.265
  // HD clips: the stream is tried for the rest; it says if it can't.)
  if (s.kind === 'clip' && !hd && hlsReady() && !(el._noHls && Date.now() - el._noHls < 60000)) { leaveHls(el, true); return false; }
  if ((el._noHls && Date.now() - el._noHls < 60000) || s.kind === 'clip' || hd || !hlsReady()) { leaveHls(el); return false; }
  const h = hlsFor(el, c.camera), q = tlQ(), T = tl.T, now = Date.now();
  const busy = !h.ready && now - (h.reqAt || 0) < 5000;
  // A playlist for this stream and time (a new one past its ends, and
  // every so often near now, where the recording keeps growing).
  if (h.stream !== q || ((T < h.from || T > h.to) && !busy)) hlsLoad(h, T, q);
  // Playlist on its way (a moment): the picture on screen stays.
  if (!h.ready) { el._vids.forEach(v => { if (!v.paused) v.pause(); }); return true; }
  const at = hlsAt(h, T);
  if (!at) { leaveHls(el); return false; }
  el._mode = 'hls';
  const v = el._hlsV;
  if (!h.started) { h.hls.startLoad(at.m); h.started = true; force = true; }
  v.playbackRate = tl.speed;
  if (force || Math.abs(v.currentTime - at.m) > Math.max(0.8, tl.speed * 0.5)) v.currentTime = at.m;
  // Between two files: wait at the start of the next one.
  if (tl.playing && !at.gap && v.paused) v.play().catch(() => {});
  if ((!tl.playing || at.gap) && !v.paused) v.pause();
  if (v.style.visibility === 'hidden' && v.readyState >= 2 && !v.seeking) showHls(el);
  // An alert's HD clip coming up: have it ready.
  const ahead = 6000 * Math.max(1, tl.speed);
  const nextClip = c.segments.find(x => x.kind === 'clip' && !tl.badClips.has(x.url) && segMs(x)._a > T && segMs(x)._a - T < ahead);
  if (nextClip) tlPreload(el, c, segMs(nextClip)._a + 50);
  return true;
}
const activeVideo = (el) => el._mode === 'hls' ? el._hlsV : el._vids[el._active];
// Make player `i` the active one, showing it once it has a picture (the
// other one keeps showing its last picture meanwhile).
function revealWhenReady(el, i) {
  const v = el._vids[i];
  el._active = i; el._pending = i;
  const done = () => {
    if (el._pending !== i) return; el._pending = null;
    if (el._active !== i) return;
    showVideo(el, i);
    // The player switched away from stops downloading its file (it held a
    // connection the other cameras' videos waited for).
    const old = el._vids[1 - i];
    if (old.networkState === HTMLMediaElement.NETWORK_LOADING && old.dataset.url !== (el._vids[i].dataset.url)) { old.removeAttribute('src'); old.load(); delete old.dataset.url; }
  };
  if (v.readyState >= 2 && !v.seeking) { done(); return; }
  v.addEventListener('seeked', () => { if (v.readyState >= 2) done(); }, {once: true});
  v.addEventListener('loadeddata', done, {once: true});
  v.addEventListener('playing', done, {once: true});
  setTimeout(done, 15000);
}
function showVideo(el, i) {
  // (The stream kept going under an alert's clip isn't stopped.)
  if (el._hlsV) { el._hlsV.style.visibility = 'hidden'; if (!el._hlsV.paused && !el._hlsBehind) el._hlsV.pause(); }
  el._vids.forEach((v, j) => { v.style.visibility = j === i ? '' : 'hidden'; if (j !== i && !v.paused) v.pause(); });
  el._active = i;
  tlApplySound();
}

// ---------- sound on the Recordings timeline ----------
// One camera at a time: the one chosen with 🔈 on its picture, otherwise
// the one shown on its own, the alert being watched, or the first.
function tlSoundCam() {
  const has = (id) => id && tl.data.some(c => c.camera === id);
  return has(tl.soundCam) ? tl.soundCam : has(tl.onlyCam) ? tl.onlyCam : tl.hd && has(tl.hd.camera) ? tl.hd.camera : (tl.data[0] || {}).camera;
}
function tlApplySound() {
  const cam = tl.sound ? tlSoundCam() : null;
  // (Fast playback is noise: quiet above 4×.)
  const audible = cam && !tl.live && tl.speed <= 4;
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const mine = el.dataset.cam === cam, act = activeVideo(el);
    for (const v of [...(el._vids || []), el._hlsV].filter(Boolean)) {
      v.muted = !(audible && mine && v === act);
      if (!v.muted) v.volume = soundVolume();
    }
    const b = el.querySelector('.tl-snd'); if (b) { b.textContent = mine ? '🔊' : '🔈'; b.classList.toggle('on', mine); }
  }
  $('tl-sound').textContent = cam ? '🔊 Sound' : '🔇 Sound';
  $('tl-sound').classList.toggle('on', !!cam);
  // Live: the camera's live sound.
  if (tl.live && cam) { if (soundCam !== cam) startSound(cam); } else if (tl.live && soundCam) stopSound();
}
$('tl-sound').onclick = () => { tl.sound = !tl.sound; store.set('tl-sound', tl.sound); tlApplySound(); };

// ---------- Live on the Recordings tab ----------
// Every camera's live picture over its player, the playhead kept at now;
// Play, dragging or clicking the timeline goes back to the recordings.
let tlLiveTimer = null;
function tlGoLive() {
  if (tl.playing) tlPlay(false);
  document.querySelectorAll('#tl-videos video').forEach(v => v.pause());
  tl.live = true;
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    if (el._liveUn) continue;
    const box = document.createElement('div'); box.className = 'tl-livebox';
    box.innerHTML = '<img alt=""><span class="tl-livetag">● LIVE</span>';
    el.querySelector('.ov.tl').before(box);
    el._liveUn = bindLive(box.querySelector('img'), el.dataset.cam, tl.quality === 'main' ? 'main' : 'sub');
    el._liveBox = box;
    el.querySelector('.sprite').hidden = true; el.querySelector('.msg').hidden = true;
  }
  const tick = () => { tl.T = Date.now(); if (tl.T > tl.winStart + tl.zoom * 1000 * 0.95) { centerWindow(); tl.winStart = Math.min(tl.winStart, Date.now() - tl.zoom * 1000 * 0.9); drawTimeline(); } else drawHead(); document.querySelectorAll('#tl-videos .tl-t').forEach(t => t.textContent = fmtDate(new Date(), t.dataset.fmt || undefined)); };
  clearInterval(tlLiveTimer); tlLiveTimer = setInterval(tick, 1000); tick();
  $('tl-live').classList.add('live-on');
  updateSubscriptions();
  tlApplySound(); paintTlBar();
}
function tlLeaveLive() {
  if (!tl.live) return;
  tl.live = false;
  clearInterval(tlLiveTimer); tlLiveTimer = null;
  for (const el of document.querySelectorAll('#tl-videos .surface')) { if (el._liveUn) { el._liveUn(); el._liveUn = null; } if (el._liveBox) { el._liveBox.remove(); el._liveBox = null; } }
  $('tl-live').classList.remove('live-on');
  if (soundCam) stopSound();
  updateSubscriptions();
  tlApplySound(); paintTlBar();
}
$('tl-live').onclick = () => tl.live ? (tlLeaveLive(), tlSync(true)) : tlGoLive();
// Bars on a timeline: pieces of the same kind that follow each other (a
// recording is saved as a file every few minutes, with a second or two
// between) are drawn as one solid bar. pieces: {a, b (%), cls, live}.
function solidBars(pieces, gap, tag = 'div', base = '') {
  const runs = [];
  for (const p of pieces.slice().sort((x, y) => x.a - y.a)) {
    const r = runs[runs.length - 1];
    if (r && r.cls === p.cls && p.a - r.b <= gap) { r.b = Math.max(r.b, p.b); r.live = p.live; }
    else runs.push({...p});
  }
  return runs.map(r => `<${tag} class="${base} ${r.cls} ${r.live ? 'live' : ''}" style="left:${r.a}%;width:${Math.max(r.b - r.a, 0.15)}%"${r.live ? ' title="Recording now"' : ''}></${tag}>`).join('');
}
// Which stream a piece of footage is (older servers didn't say).
const streamOf = (s) => s.stream || (s.kind === 'sub' ? 'sub' : 'main');
// The streams there's footage of here, and the one shown: the one chosen,
// or the other when the chosen one wasn't recorded at all.
// (Kept until the timeline's files change: asked for every file while scrubbing.)
let tlStreamsMemo = null;
function tlStreams() {
  const n = tl.data.reduce((a, c) => a + c.segments.length, 0);
  if (tlStreamsMemo && tlStreamsMemo.data === tl.data && tlStreamsMemo.n === n) return tlStreamsMemo.v;
  const v = new Set(tl.data.flatMap(c => c.segments.map(streamOf)));
  tlStreamsMemo = {data: tl.data, n, v}; return v;
}
function tlQ() { const have = tlStreams(); return have.has(tl.quality) || !have.size ? tl.quality : [...have][0]; }
// Footage drawn and played for the chosen stream: main stream = HD only
// (full-quality 24/7 recording and alert clips); sub-stream = the sub-stream
// recording, with the HD alert clips still marked (alerts play in HD).
function tlShows(s) { return tlQ() === 'main' ? streamOf(s) === 'main' : streamOf(s) === 'sub' || s.kind === 'clip'; }
// The stream choice shows what's recorded: a stream with no footage here
// can't be picked.
function paintQualityChoice() {
  const sel = $('tl-quality'), have = tlStreams(), q = tlQ();
  for (const o of sel.options) {
    const rec = !have.size || have.has(o.value);
    o.disabled = !rec;
    o.textContent = (o.value === 'main' ? 'Main stream (HD)' : 'Sub-stream') + (rec ? '' : ' — not recorded');
  }
  sel.value = q;
  for (const el of document.querySelectorAll('#tl-videos .surface .msg'))
    el.textContent = q === 'main' && have.has('sub') ? 'No main-stream (HD) footage at this time' : 'No recorded footage at this time';
}
const segLastsCache = new WeakMap();
function segLasts(segs) {
  const h = segLastsCache.get(segs); if (h && h.n === segs.length) return h.v;
  const v = new Set(['recording', 'sub'].map(k => segs.filter(s => s.kind === k).slice(-1)[0]));
  segLastsCache.set(segs, {n: segs.length, v}); return v;
}
function segAt(c, T, force) {
  // The newest file of a camera that was recording when the timeline
  // loaded is still growing: treat it as running up to now.
  const lasts = segLasts(c.segments);
  const end = (s) => lasts.has(s) && tl.loadedAt - s._b < 90000 ? Date.now() + 5000 : s._b + 1000;
  const hit = (s) => segMs(s)._a <= T && T < end(s);
  // Normally the light stream 2 copy (opens instantly); after clicking an
  // alert, its full-quality footage until the alert is over.
  const hd = tl.hd && tl.hd.camera === c.camera && T >= tl.hd.from - 2000 && T < tl.hd.until;
  // Watching stream 2, the alerts' HD clips play where there are any: the
  // picture goes to full quality for the alert and back afterwards.
  const order = force === 'main' ? ['clip', 'recording', 'sub'] : force === 'sub' ? ['sub', 'recording', 'clip']
    : ['clip', 'sub', 'recording'];
  // Only the chosen stream (an alert you clicked plays in HD either way).
  const ok = force || hd ? () => true : tlShows;
  for (const k of order) {
    const s = c.segments.find(x => x.kind === k && ok(x) && hit(x) && !(k === 'clip' && tl.badClips.has(x.url)));
    if (s) return s;
  }
  return null;
}
// Keep the timeline current while looking at recent footage.
// While looking at a window that reaches now, new footage and its time-lapse
// keep being added, however long the page stays open.
setInterval(() => {
  if (page !== 'recordings' || rec.mode !== 'timeline' || document.hidden || tl.dragging) return;
  if (tl.winStart + tl.zoom * 1000 < Date.now() - 300000) return;
  refreshTimelineData();
}, 30000);

// Keep each camera's video at the shared time `tl.T`.
function tlSync(force) {
  // While dragging only the time-lapse shows (a playlist or player that
  // loads meanwhile waits for the drop).
  if (tl.dragging) return;
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const c = tl.data.find(x => x.camera === el.dataset.cam);
    const msg = el.querySelector('.msg');
    let v = activeVideo(el);
    el.querySelectorAll('.tl-t').forEach(t => t.textContent = fmtDate(new Date(tl.T), t.dataset.fmt || undefined));
    const s = c && segAt(c, tl.T);
    if (!s) { msg.hidden = false; el.querySelector('.sprite').hidden = true; [...el._vids, el._hlsV].forEach(x => { x.pause(); x.style.visibility = 'hidden'; }); continue; }
    msg.hidden = true;
    // Played as one continuous stream where it can be (see tlHls).
    if (tlHls(el, c, s, force)) continue;
    v = activeVideo(el);
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
      // Another file: always in the other (hidden) player — usually already
      // loading, see below — and shown only once it has a picture; until
      // then the last picture of the file before stays (no black gap).
      const oi = 1 - el._active, other = el._vids[oi];
      if (other.dataset.url !== s.url) { other.dataset.url = s.url; other.preload = 'auto'; other.src = s.url; }
      const go = () => {
        other.currentTime = Math.max(0, (tl.T - new Date(s.start).getTime()) / 1000);
        other.playbackRate = tl.speed;
        if (tl.playing) other.play().catch(() => {});
      };
      if (other.readyState >= 1) go(); else other.addEventListener('loadedmetadata', go, {once: true});
      revealWhenReady(el, oi);
      continue;
    }
    if (el._pending == null) showVideo(el, el._active);
    // The file still being recorded has grown since it was loaded: load it
    // again to reach the newest part.
    if (isGrowing(s) && isFinite(v.duration) && want > v.duration + 1 && !(v._reloadAt && Date.now() - v._reloadAt < 5000)) {
      v._reloadAt = Date.now();
      v.src = s.url + (s.url.includes('?') ? '&' : '?') + 'n=' + Date.now();
      v.addEventListener('loadedmetadata', () => { v.currentTime = Math.min(want, v.duration); v.playbackRate = tl.speed; if (tl.playing) v.play().catch(() => {}); }, {once: true});
      continue;
    }
    if (force || Math.abs(v.currentTime - want) > Math.max(0.8, tl.speed * 0.5)) v.currentTime = want;
    v.playbackRate = tl.speed;
    if (tl.playing && v.paused) v.play().catch(() => {});
    if (!tl.playing && !v.paused) v.pause();
    // The stream under an alert's clip: kept in step (out of sight), to
    // carry on from the end of the alert at once.
    if (el._hlsBehind && el._h && el._h.ready) {
      const hv = el._hlsV, at = hlsAt(el._h, tl.T);
      hv.muted = true; hv.playbackRate = tl.speed;
      if (at && Math.abs(hv.currentTime - at.m) > Math.max(1.5, tl.speed)) hv.currentTime = at.m;
      if (tl.playing && !at?.gap && hv.paused) hv.play().catch(() => {});
      if ((!tl.playing || at?.gap) && !hv.paused) hv.pause();
    }
    // Near the end of this file (or of the alert's HD part): load what comes
    // next in the other player, at the right moment, for a seamless switch.
    const segEnd = new Date(s.end).getTime();
    const boundary = tl.hd && tl.hd.camera === c.camera && s.kind !== 'sub' ? Math.min(segEnd, tl.hd.until) : segEnd;
    const left = (boundary - tl.T) / 1000;
    const nx = left < 10 * Math.max(1, tl.speed) && segAt(c, boundary + 100);
    const other = el._vids[1 - el._active];
    // (Back to the stream after an alert's clip: nothing to load.)
    const streamNext = nx && nx.kind !== 'clip' && el._hlsBehind;
    if (nx && !streamNext && nx.url !== s.url && other.dataset.url !== nx.url) {
      other.dataset.url = nx.url; other.preload = 'auto'; other.src = nx.url;
      const at = Math.max(0, (boundary - new Date(nx.start).getTime()) / 1000);
      other.addEventListener('loadedmetadata', () => { other.currentTime = at; }, {once: true});
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
  // (Only while the playhead is in view: moved away with ◀ ▶, it stays put.)
  const winEnd = tl.winStart + tl.zoom * 1000;
  if (tl.T > winEnd - tl.zoom * 100 && tl.T <= winEnd + 2000) { tl.winStart = tl.T - tl.zoom * 1000 * 0.1; drawTimeline(); }
  if (!tl.syncAt || ts - tl.syncAt > 400) { tl.syncAt = ts; tlSync(false); } else drawHead();
  requestAnimationFrame(tlLoop);
}
function tlPlay(on) {
  // From Live: play the recording from a few seconds ago.
  if (on && tl.live) { tlLeaveLive(); tl.T = Date.now() - 15000; drawHead(); }
  tl.playing = on; tl.last = 0;
  $('tl-play').textContent = on ? '❚❚ Pause' : '▶ Play';
  tlSync(false);
  if (on) requestAnimationFrame(tlLoop);
}
function tlStop() { tlLeaveLive(); if (tl.playing) tlPlay(false); document.querySelectorAll('#tl-videos video').forEach(v => v.pause()); }
$('tl-play').onclick = () => tlPlay(!tl.playing);
$('tl-speed').onchange = (e) => { tl.speed = +e.target.value; tlSync(false); tlApplySound(); };
$('tl-quality').value = tl.quality;
// Scrubbing shows the time-lapse pictures (the video itself when time-lapse
// is switched off).
$('tl-quality').onchange = (e) => { tl.quality = e.target.value; store.set('tl-quality', tl.quality); drawTimeline(); tlSync(true); };
$('tl-zoom').onchange = (e) => { tl.zoom = +e.target.value; centerWindow(); drawTimeline(); prefetchSprites(); };
$('tl-left').onclick = () => { tl.winStart -= tl.zoom * 500; drawTimeline(); prefetchSprites(); };
$('tl-right').onclick = () => { tl.winStart += tl.zoom * 500; drawTimeline(); prefetchSprites(); };
// One frame back or forward on every camera (pauses playback).
const tlStep = (n) => {
  tlLeaveLive();
  if (tl.playing) tlPlay(false);
  const v = document.querySelector('#tl-videos .surface') && activeVideo(document.querySelector('#tl-videos .surface'));
  tl.T += n * 1000 / videoFps(v);
  drawHead(); tlSync(true);
};
$('tl-back10').onclick = () => tlStep(-1);
$('tl-fwd10').onclick = () => tlStep(1);
// Previous / next alert: played like clicking it — in full quality (main
// stream) from just before it, the other cameras carrying on as they are.
function allAlerts() { return tl.data.flatMap(c => c.events.map(e => ({cam: c.camera, t: new Date(e.start).getTime()}))).sort((a, b) => a.t - b.t); }
function tlGoAlert(a) {
  if (!a) return;
  tl.hd = {camera: a.cam, from: a.t - 3000, until: tlHdUntil(a.cam, a.t)};
  jumpTo(a.t - 3000);
}
$('tl-prev-ev').onclick = () => tlGoAlert(allAlerts().filter(a => a.t < tl.T - 4000).pop());
$('tl-next-ev').onclick = () => tlGoAlert(allAlerts().find(a => a.t > tl.T + 4000));
// Watch one alert: that camera's video large (others hidden), from just before it.
// Watch one alert: just that camera (the camera choice follows; "← All
// cameras" goes back), in full quality from just before the alert, then on
// into the light stream 2 recording without a break.
// Until when an alert at T plays in full quality: its clip, or the alert
// and 10 s after it.
function tlHdUntil(camId, T) {
  const c = tl.data.find(x => x.camera === camId);
  const e = c && c.events.find(x => Math.abs(new Date(x.start).getTime() - T) < 1000);
  const clip = c && c.segments.find(s => s.kind === 'clip' && new Date(s.start).getTime() <= T && new Date(s.end).getTime() > T);
  return clip ? new Date(clip.end).getTime() : (e ? new Date(e.end).getTime() + 10000 : T + 30000);
}
async function tlShowEvent(camId, T) {
  tl.hd = {camera: camId, from: T - 3000, until: tlHdUntil(camId, T)};
  // In full screen: stay there and play the alert in full quality on the
  // same picture (rebuilding the page would end full screen).
  const fe = document.fullscreenElement;
  if (fe && fe.closest('#tl-videos')) {
    if (fe.dataset.cam !== camId) return;
    if (tl.playing) tlPlay(false);
    jumpTo(T - 3000);
    tlPlay(true);
    return;
  }
  if (!(rec.cameras.size === 1 && rec.cameras.has(camId))) {
    if (!rec.prevCameras) rec.prevCameras = new Set(rec.cameras);
    rec.cameras = new Set([camId]);
    renderFilters();
  }
  tl.jump = T;
  await loadTimeline();
  tlPlay(true);
}
// Click a camera's name on the timeline: show just that camera, at the same
// moment ("← All cameras" goes back).
function tlOnlyCamera(camId) {
  if (rec.cameras.size === 1 && rec.cameras.has(camId)) return;
  if (!rec.prevCameras) rec.prevCameras = new Set(rec.cameras);
  rec.cameras = new Set([camId]);
  if (tl.T != null) tl.jump = tl.T + 3000;
  renderFilters(); loadTimeline();
}
function tlBackToAll() {
  rec.cameras = rec.prevCameras || new Set();
  rec.prevCameras = null; tl.hd = null;
  renderFilters(); loadTimeline();
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
function jumpTo(T) { tlLeaveLive(); tl.T = T; if (T < tl.winStart || T > tl.winStart + tl.zoom * 1000) centerWindow(); drawTimeline(); tlSync(true); }
$('tl-in').onclick = () => { tl.inT = tl.T; if (tl.outT != null && tl.outT <= tl.inT) tl.outT = null; drawTimeline(); };
$('tl-out').onclick = () => { tl.outT = tl.T; if (tl.inT == null || tl.inT >= tl.outT) tl.inT = tl.outT - 30000; drawTimeline(); };
$('tl-clear').onclick = () => { tl.inT = tl.outT = null; drawTimeline(); };
$('tl-back').onclick = tlBackToAll;

function selStyle(pct) {
  return `left:calc(var(--tl-label) + (100% - var(--tl-label)) * ${pct(tl.inT) / 100});width:calc((100% - var(--tl-label)) * ${(pct(tl.outT) - pct(tl.inT)) / 100})`;
}
function drawTimeline() {
  const box = $('tl'), W0 = tl.winStart, span = tl.zoom * 1000;
  const pct = (t) => ((t - W0) / span * 100);
  paintQualityChoice();
  const {axis, grid} = timeMarks(W0, span, pct);
  const lw = parseFloat(getComputedStyle(box).getPropertyValue('--tl-label')), lanePx = box.clientWidth - (isNaN(lw) ? 160 : lw);
  tl.groups = [];
  let rows = '';
  // In full screen: only the camera shown (more room for the picture).
  for (const c of tl.data.filter(x => !tl.onlyCam || x.camera === tl.onlyCam)) {
    let lane = '';
    // The newest recording, still being made, reaches up to now (and shows
    // it's still being recorded).
    const shown = c.segments.filter(tlShows), lastRec = shown.filter(x => x.kind !== 'clip').slice(-1)[0], pieces = [];
    for (const s of shown) {
      const live = s === lastRec && isGrowing(s);
      const a = Math.max(0, pct(new Date(s.start).getTime())), b = Math.min(100, pct(live ? Date.now() : new Date(s.end).getTime()));
      if (b > 0 && a < 100 && b > a) pieces.push({a, b, cls: streamOf(s) === 'main' ? 'clip' : 'rec', live});
    }
    lane += solidBars(pieces, 15000 / span * 100, 'div', 'seg');
    lane += evMarks(c.events, pct, `data-cam="${esc(c.camera)}"`, c.camera, lanePx, tl.groups);
    const st = cam(c.camera), recOn = !!(st && st.recorder && st.recorder.state === 'recording');
    const nEv = c.events.filter(e => { const t = new Date(e.start).getTime(); return t >= W0 && t <= W0 + span; }).length;
    rows += `<div class="tl-row"><button type="button" class="label" data-cam="${esc(c.camera)}" title="${esc(c.name)} — show just this camera"><i class="dot ${recOn ? 'on' : ''}" title="${recOn ? 'Recording' : 'Not recording'}"></i><span class="nm">${esc(c.name)}</span>${nEv ? `<span class="n" title="Alerts in view">${nEv}</span>` : ''}</button><div class="lanes"><div class="lane"><div class="tl-grid">${grid}</div>${lane}</div>
      <div class="tlp" data-cam="${esc(c.camera)}" title="Time-lapse previews — blue: ready to scrub · striped: loading · grey: not loaded yet · red: couldn't be made"></div></div></div>`;
  }
  // A clip mark shows as soon as it's set (one alone: an orange line).
  const sel = tl.inT != null && tl.outT != null ? `<div class="tl-sel" style="${selStyle(pct)}"><div class="tl-h in" title="Drag to change the clip start"></div><div class="tl-h out" title="Drag to change the clip end"></div></div>`
    : tl.inT != null || tl.outT != null ? `<div class="tl-mark1" style="left:calc(var(--tl-label) + (100% - var(--tl-label)) * ${pct(tl.inT ?? tl.outT) / 100})" title="${tl.inT != null ? 'Clip start' : 'Clip end'}"></div>` : '';
  box.innerHTML = `<div class="tl-axis">${axis}</div>${rows}${sel}<div class="tl-head" id="tl-head"><em></em></div><div class="tl-hover" id="tl-hover" hidden><em></em></div>`;
  declutterAxis(box.querySelector('.tl-axis'));
  box.querySelectorAll('.tl-row').forEach((r, i) => r.classList.toggle('solo', tl.solo === (tl.data[i] || {}).camera));
  box.querySelectorAll('.tl-row .label').forEach(b => {
    b.onpointerdown = (e) => e.stopPropagation();
    b.onclick = (e) => { e.stopPropagation(); tlOnlyCamera(b.dataset.cam); };
  });
  document.querySelector('#rec-timeline .legend').innerHTML = legendHtml('tl', tl.data);
  paintPreviewStrips();
  // Clip export
  const has = tl.inT != null && tl.outT != null;
  $('tl-clear').hidden = !has;
  $('tl-back').hidden = !rec.prevCameras;
  $('tl-sel-info').textContent = has ? `Clip ${fmtTime(new Date(tl.inT))} – ${fmtTime(new Date(tl.outT))} (${fmtClock((tl.outT - tl.inT) / 1000)})` : '';
  $('tl-export').innerHTML = has ? tl.data.map(c => `<a class="btn small" href="api/export?camera=${enc(c.camera)}&from=${enc(new Date(tl.inT).toISOString())}&to=${enc(new Date(tl.outT).toISOString())}">⇩ ${esc(c.name)}</a>`).join('') : '';
  drawHead();
}
function drawHead() {
  paintTlBar();
  const h = $('tl-head'); if (!h) return;
  const p = (tl.T - tl.winStart) / (tl.zoom * 1000);
  h.hidden = p < 0 || p > 1;
  h.style.left = `calc(var(--tl-label) + (100% - var(--tl-label)) * ${p})`;
  const em = h.querySelector('em'); if (em) em.textContent = fmtTime(new Date(tl.T));
  $('tl-time').textContent = fmtDate(new Date(tl.T));
}

// ---------- On-screen controls in full screen ----------
// A video in full screen gets the same controls on the picture as the alert
// player: Recordings — scrub bar, play, frames, alerts, speed, clip start /
// end and download, sound; a live camera — sound, push to talk (two-way
// audio), snapshot, clip start / end and download (from the recording).
// They fade while nothing moves.
// Timelines lent to a full-screen panel: [element, its place-holder].
const fsLent = [];
document.addEventListener('fullscreenchange', () => {
  // Put back what the full-screen panel borrowed — also when that picture
  // was taken off the page meanwhile (the panel then can't be found).
  if (!document.fullscreenElement) {
    for (const [el, home] of fsLent.splice(0)) {
      if (home.isConnected) { home.replaceWith(el); if (el.id === 'tl') { tl.onlyCam = null; requestAnimationFrame(drawTimeline); } else requestAnimationFrame(() => camTl.draw()); }
    }
  }
  document.querySelectorAll('.fspov').forEach(x => {
    const s = x.parentElement;
    // The Recordings timeline goes back to its place under the videos.
    const tlEl = x.querySelector('#tl');
    if (tlEl && s._tlHome) { s._tlHome.replaceWith(tlEl); s._tlHome = null; tl.onlyCam = null; requestAnimationFrame(drawTimeline); }
    const ctl = x.querySelector('#cam-tl');
    if (ctl && s._ctlHome) { s._ctlHome.replaceWith(ctl); s._ctlHome = null; requestAnimationFrame(() => camTl.draw()); }
    x.remove();
    if (!s.querySelector('.pov')) s.classList.remove('has-pov', 'pov-idle');
  });
  const fe = document.fullscreenElement;
  document.querySelectorAll('.surface.fs-panel').forEach(x => { if (x !== fe) x.classList.remove('fs-panel'); });
  // Left the camera view opened from a grid camera in full screen: out of
  // the grid camera's full screen too.
  if (!fe && fsFromTile) { fsFromTile = false; if (location.hash.startsWith('#live/')) { location.hash = '#live'; route(); } }
  // (The alert player has its own controls; the camera view gets its
  // timeline also while it plays something back.)
  if (!fe || !fe.classList.contains('surface') || (fe.querySelector('.pov') && fe.id !== 'focus-surface')) return;
  if (fe.closest('#tl-videos')) tlFsBar(fe);
  else if (fe.classList.contains('tile') || fe.id === 'focus-surface') liveFsBar(fe, fe.dataset.id || focusId);
});
function fsIdle(el, bar) {
  let t = null;
  const wake = () => { el.classList.remove('pov-idle'); clearTimeout(t); t = setTimeout(() => { if (!bar.matches(':hover') && document.fullscreenElement === el) el.classList.add('pov-idle'); }, 2500); };
  el.addEventListener('pointermove', wake); el.addEventListener('pointerdown', wake); wake();
}
const fsClipHref = (camId, a, b) => `api/export?camera=${enc(camId)}&from=${enc(new Date(a).toISOString())}&to=${enc(new Date(b).toISOString())}`;
function tlFsBar(el) {
  const camId = el.dataset.cam, c = cam(camId) || {};
  const bar = document.createElement('div');
  bar.className = 'pov fspov tlbar tl-panel';
  bar.innerHTML = `<div class="fs-tlslot"></div>
    <div class="controls">
      <button type="button" class="p-play primary">▶</button>
      <button type="button" class="p-back" title="Back one frame">−1 frame</button><button type="button" class="p-fwd" title="Forward one frame">+1 frame</button>
      <button type="button" class="p-pev" title="Previous alert">⏮ Alert</button><button type="button" class="p-nev" title="Next alert">Alert ⏭</button>
      <button type="button" class="p-live" title="Watch this camera live now (Play or the timeline goes back to the recording)">● Live</button>
      ${c.talk && me && me.role !== 'viewer' ? '<button type="button" class="p-talk" hidden title="Hold to talk through the camera (tap to keep talking)"><span>Push to talk</span></button><button type="button" class="p-say" hidden title="Type words for the camera to say">💬 Say…</button><button type="button" class="p-siren siren-btn" hidden title="Sound the siren">🚨 Siren</button>' : ''}
      <select class="p-speed">${[1, 2, 4, 8, 16].map(x => `<option value="${x}" ${x === tl.speed ? 'selected' : ''}>${x}×</option>`).join('')}</select>
      <span class="timecode p-time"></span><span class="spacer"></span>
      <button type="button" class="p-in" title="The clip starts here">${CLIP_IN}</button><button type="button" class="p-out" title="The clip ends here">${CLIP_OUT}</button>
      <a class="btn p-clip" hidden>⇩ Download clip</a>
      <select class="p-zoom" title="Zoom">${[...$('tl-zoom').options].map(o => `<option value="${o.value}" ${o.value === $('tl-zoom').value ? 'selected' : ''}>${o.textContent}</option>`).join('')}</select>
      <button type="button" class="p-left" title="Earlier">◀</button><button type="button" class="p-right" title="Later">▶</button>
    </div>`;
  el.appendChild(bar); el.classList.add('has-pov');
  // The Recordings timeline itself (every camera, alert pictures, the
  // time-lapse line, scrubbing) moves into the panel while in full screen.
  const tlEl = $('tl');
  tl.onlyCam = camId;
  el._tlHome = document.createComment('timeline'); tlEl.before(el._tlHome);
  fsLent.push([tlEl, el._tlHome]);
  bar.querySelector('.fs-tlslot').appendChild(tlEl);
  requestAnimationFrame(() => { drawTimeline(); drawHead(); });
  bar.querySelector('.p-zoom').onchange = (e) => { $('tl-zoom').value = e.target.value; $('tl-zoom').dispatchEvent(new Event('change')); };
  bar.querySelector('.p-left').onclick = () => $('tl-left').click();
  bar.querySelector('.p-right').onclick = () => $('tl-right').click();
  slidePanel(el, bar, () => tl.dragging);
  const q = (x) => bar.querySelector(x);
  q('.p-play').onclick = () => $('tl-play').click();
  q('.p-back').onclick = () => tlStep(-1); q('.p-fwd').onclick = () => tlStep(1);
  q('.p-pev').onclick = () => $('tl-prev-ev').click(); q('.p-nev').onclick = () => $('tl-next-ev').click();
  // Live, in this window: the camera's live picture, and its push-to-talk.
  q('.p-live').onclick = () => { tl.live ? (tlLeaveLive(), tlSync(true)) : tlGoLive(); paintTlBar(); };
  if (q('.p-talk')) setupTalkButton(c, q('.p-talk'));
  if (q('.p-say')) { const sb = q('.p-say'), hid = sb.hidden; setupSayButton(c, sb); sb.hidden = hid; }
  if (q('.p-siren')) { const sb = q('.p-siren'), hid = sb.hidden; setupSirenButton(c, sb); sb.hidden = hid; }
  q('.p-speed').onchange = (e) => { $('tl-speed').value = e.target.value; $('tl-speed').dispatchEvent(new Event('change')); };
  q('.p-in').onclick = () => $('tl-in').click(); q('.p-out').onclick = () => $('tl-out').click();
  q('.p-clip').download = '';
  paintTlBar();
}
// A full-screen panel that slides up when the mouse comes to the bottom of
// the screen (and stays while it's used), and away otherwise. Above the
// recording player's own controls when they're showing.
function slidePanel(el, bar, busy) {
  let hideT = null;
  const place = () => { const pb = el.querySelector('.pov:not(.fspov)'); bar.style.bottom = pb && !pb.hidden && !el.classList.contains('pov-idle') ? pb.offsetHeight + 'px' : '0px'; };
  const show = (on) => { clearTimeout(hideT); place(); if (on) bar.classList.remove('away'); else { if (!busy() && !bar.matches(':hover')) bar.classList.add('away'); else hideT = setTimeout(() => show(false), 200); } };
  el.addEventListener('pointermove', (e) => { if (document.fullscreenElement !== el) return; show(e.clientY > innerHeight - Math.max(140, bar.offsetHeight + 30) - (parseFloat(bar.style.bottom) || 0) || bar.contains(e.target) || busy()); });
  bar.addEventListener('pointerleave', () => show(false));
  show(true); hideT = setTimeout(() => { if (!bar.matches(':hover')) bar.classList.add('away'); }, 2500);
}
function paintTlBar() {
  const bar = document.querySelector('.fspov.tlbar'); if (!bar) return;
  const lv = bar.querySelector('.p-live'); if (lv) { lv.classList.toggle('live-on', !!tl.live); lv.textContent = tl.live ? '● Live — back to recording' : '● Live'; }
  const tk = bar.querySelector('.p-talk'); if (tk) tk.hidden = !tl.live;
  const sy = bar.querySelector('.p-say'); if (sy) sy.hidden = !tl.live;
  const sr = bar.querySelector('.p-siren'); if (sr) sr.hidden = !tl.live;
  bar.querySelector('.p-time').textContent = fmtDate(new Date(tl.T));
  bar.querySelector('.p-play').textContent = tl.playing ? '❚❚' : '▶';
  const z = bar.querySelector('.p-zoom'); if (z && z.value !== $('tl-zoom').value) z.value = $('tl-zoom').value;
  const has = tl.inT != null && tl.outT != null, clip = bar.querySelector('.p-clip');
  clip.hidden = !has;
  if (has) {
    clip.href = fsClipHref(bar.parentElement.dataset.cam, tl.inT, tl.outT);
    clip.textContent = `⇩ Download clip (${fmtClock((tl.outT - tl.inT) / 1000)})`;
  }
}
function liveFsBar(el, camId) {
  const c = cam(camId); if (!c) return;
  const bar = document.createElement('div');
  const inView = el.id === 'focus-surface';
  bar.className = 'pov fspov livebar' + (inView ? ' tl-panel' : '');
  // Two sets of buttons in the camera view: live (snapshot, talk) and, while
  // something recorded plays, the player's (its own bar is hidden: one
  // toolbar for everything).
  bar.innerHTML = `${inView ? '<div class="fs-tlslot"></div>' : ''}<div class="controls">
      <span class="p-live live-only">● LIVE</span>
      <span class="play-only"><button type="button" class="p-pplay primary">❚❚</button></span>
      <button type="button" class="p-pback play-only" title="Back one frame">−1 frame</button><button type="button" class="p-pfwd play-only" title="Forward one frame">+1 frame</button>
      <select class="p-pspeed play-only">${[0.5, 1, 2, 4, 8, 16].map(x => `<option value="${x}" ${x === 1 ? 'selected' : ''}>${x}×</option>`).join('')}</select>
      <span class="timecode p-time"></span><span class="spacer"></span>
      ${c.talk && me && me.role !== 'viewer' ? '<button type="button" class="p-talk live-only" title="Hold to talk through the camera (tap to keep talking)"><span>Push to talk</span></button><button type="button" class="p-say live-only" title="Type words for the camera to say">💬 Say…</button><button type="button" class="p-siren siren-btn live-only" title="Sound the siren">🚨 Siren</button>' : ''}
      <a class="btn p-snap live-only" title="Save a full-quality picture">📷 Snapshot</a>
      <button type="button" class="p-in" title="The clip starts here">${CLIP_IN}</button><button type="button" class="p-out" title="The clip ends here">${CLIP_OUT}</button>
      <a class="btn p-clip" hidden>⇩ Download clip</a>
      <a class="btn p-file play-only" title="Download the whole recording file">⇩ File</a>
      ${inView ? `<button type="button" class="p-golive play-only" title="Back to the live picture">● Back to live</button>
      <select class="p-zoom" title="Zoom">${[...$('ctl-zoom').options].map(o => `<option value="${o.value}" ${o.value === $('ctl-zoom').value ? 'selected' : ''}>${o.textContent}</option>`).join('')}</select>
      <button type="button" class="p-left" title="Earlier">◀</button><button type="button" class="p-right" title="Later">▶</button>` : ''}
    </div>`;
  el.appendChild(bar); el.classList.add('has-pov');
  const q = (x) => bar.querySelector(x);
  const playing = () => inView && !!focusPlayback && !!focusPlayer;
  const pq = (x) => focusPlayer && focusPlayer.box.querySelector(x);
  if (inView) {
    el.classList.add('fs-panel');
    // The camera's timeline (as under the picture): drag along it to look
    // back; it plays here, in full screen.
    const ctl = $('cam-tl');
    el._ctlHome = document.createComment('camera timeline'); ctl.before(el._ctlHome);
    fsLent.push([ctl, el._ctlHome]);
    q('.fs-tlslot').appendChild(ctl);
    requestAnimationFrame(() => camTl.draw());
    q('.p-golive').onclick = () => stopFocusPlayback();
    q('.p-pplay').onclick = () => { const v = focusPlayer.video; v.paused ? v.play().catch(() => {}) : v.pause(); };
    q('.p-pback').onclick = () => focusPlayer.stepFrame(-1);
    q('.p-pfwd').onclick = () => focusPlayer.stepFrame(1);
    q('.p-pspeed').onchange = (e) => { const s = pq('.p-speed'); if (s) { s.value = e.target.value; s.dispatchEvent(new Event('change')); } };
    q('.p-zoom').onchange = (e) => { $('ctl-zoom').value = e.target.value; $('ctl-zoom').dispatchEvent(new Event('change')); };
    q('.p-left').onclick = () => $('ctl-left').click();
    q('.p-right').onclick = () => $('ctl-right').click();
    slidePanel(el, bar, () => !!camTl.dragging);
  } else {
    fsIdle(el, bar);
  }
  const p2 = (n) => String(n).padStart(2, '0'), stamp = (d) => `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())} ${p2(d.getHours())}-${p2(d.getMinutes())}-${p2(d.getSeconds())}`;
  const snap = q('.p-snap');
  snap.href = 'snapshot/' + enc(camId);
  snap.onclick = () => { snap.download = `${c.name} ${stamp(new Date())}.jpg`; };
  if (q('.p-talk')) setupTalkButton(c, q('.p-talk'));
  if (q('.p-say')) { const sb = q('.p-say'), hid = sb.hidden; setupSayButton(c, sb); sb.hidden = hid; }
  if (q('.p-siren')) { const sb = q('.p-siren'), hid = sb.hidden; setupSirenButton(c, sb); sb.hidden = hid; }
  // Clip marks: of what's playing (the player's), or of the live picture
  // (from the recording, between the marks).
  let a = null, b = null;
  const clip = q('.p-clip');
  const paintClip = () => {
    if (playing()) {
      const pc = pq('.p-clip'); clip.hidden = !pc || pc.hidden;
      if (!clip.hidden) { clip.textContent = pc.textContent; }
      q('.p-in').classList.toggle('on', focusPlayer.inT != null && focusPlayer.outT == null);
      return;
    }
    clip.hidden = !(a && b);
    if (a && b) { clip.href = fsClipHref(camId, a, b); clip.textContent = `⇩ Download clip (${fmtClock((b - a) / 1000)})`; }
    q('.p-in').classList.toggle('on', !!a && !b);
  };
  q('.p-in').onclick = () => { if (playing()) { pq('.p-in').click(); return paintClip(); } a = Date.now(); b = null; paintClip(); toast(`Clip start marked — press ${CLIP_OUT} to finish it`); };
  q('.p-out').onclick = () => { if (playing()) { pq('.p-out').click(); return paintClip(); } if (!a) a = Date.now() - 30000; b = Date.now(); paintClip(); };
  clip.onclick = (e) => { if (playing()) { e.preventDefault(); focusPlayer.downloadClip(); } };
  const refresh = () => {
    const pl = playing();
    bar.classList.toggle('playing', pl);
    if (pl) {
      const t = pq('.p-time'); q('.p-time').textContent = t ? t.textContent : '';
      q('.p-pplay').textContent = focusPlayer.video.paused ? '▶' : '❚❚';
      const f = pq('.p-dl'); if (f) q('.p-file').href = f.href;
    } else q('.p-time').textContent = fmtDate(new Date());
    paintClip();
  };
  const tick = setInterval(() => { if (!bar.isConnected) return clearInterval(tick); refresh(); }, 400);
  refresh();
}
// The clip buttons, named the same on every player and timeline.
const CLIP_IN = '[ Clip start', CLIP_OUT = 'Clip end ]';
// A grid camera shown full screen as its camera view: leaving full screen
// goes back to the grid.
let fsFromTile = false;

// The alert marker of the timeline row under the pointer that is nearest
// to it (within 8 px), or null: markers can be a few pixels wide and close
// together, so the pointer doesn't have to be exactly on one.
function markAt(e) {
  const el = document.elementFromPoint(e.clientX, e.clientY);
  const row = el && (el.closest('#tl .tl-row') || el.closest('#ctl-lane'));
  if (!row) return null;
  let best = null, score = Infinity;
  for (const m of row.querySelectorAll('.ev')) {
    const r = m.getBoundingClientRect();
    if (e.clientY < r.top - 6 || e.clientY > r.bottom + 6) continue;
    const d = e.clientX < r.left ? r.left - e.clientX : e.clientX > r.right ? e.clientX - r.right : 0;
    if (d > 8) continue;
    const sc = d * 1000 + Math.abs((r.left + r.right) / 2 - e.clientX);
    if (sc < score) { score = sc; best = m; }
  }
  return best;
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
  let handle = null;
  // Dragging up to (or past) either end of the timeline slides it along, so
  // you can keep going back (or forward up to now) without letting go —
  // slowly at the edge, faster the further out you go.
  const EDGE = 32;
  let lastX = null, edgeRaf = 0;
  const edgeStep = () => {
    edgeRaf = 0;
    if (!drag || lastX == null) return;
    const lane = box.querySelector('.lane'); if (!lane) return;
    const r = lane.getBoundingClientRect(), span = tl.zoom * 1000;
    const out = lastX < r.left + EDGE ? lastX - (r.left + EDGE) : lastX > r.right - EDGE ? lastX - (r.right - EDGE) : 0;
    if (!out) return;
    // Up to 4× at a pointer well past the edge: about two thirds of the view
    // a second at the edge itself.
    const speed = Math.sign(out) * Math.min(Math.abs(out) / EDGE, 4);
    const min = Math.min(tl.winStart, rec.from.getTime()), max = Math.max(tl.winStart, Date.now() - span * 0.95);
    const ws = Math.max(min, Math.min(max, tl.winStart + speed * span * 0.012));
    if (ws !== tl.winStart) {
      tl.winStart = ws;
      tl.T = Math.min(ws + Math.min(Math.max((lastX - r.left) / r.width, 0), 1) * span, Date.now());
      drawTimeline(); previewSoon(); knownPreviewsSoon();
    }
    edgeRaf = requestAnimationFrame(edgeStep);
  };
  const edgeWatch = (e) => { lastX = e.clientX; if (drag && !edgeRaf) edgeRaf = requestAnimationFrame(edgeStep); };
  box.addEventListener('pointerdown', (e) => {
    // Clip start / end handles: drag to change the clip.
    const hd = e.target.closest('.tl-h');
    if (hd) { e.preventDefault(); handle = hd.classList.contains('in') ? 'in' : 'out'; box.setPointerCapture(e.pointerId); return; }
    // An alert picture: watch it, or choose among a stack of them.
    const pic = e.target.closest('.evt');
    if (pic) {
      e.preventDefault();
      const g = tl.groups[+pic.dataset.g];
      if (g.evs.length === 1) tlShowEvent(g.cam, new Date(g.evs[0].start).getTime());
      else openAlertPicker(pic, g, (ev) => tlShowEvent(g.cam, new Date(ev.start).getTime()));
      return;
    }
    // An alert marker or symbol (the nearest to the pointer, see below):
    // watch that alert on that camera.
    const mark = markAt(e);
    if (mark) { e.preventDefault(); tlShowEvent(mark.dataset.cam, +mark.dataset.t); return; }
    const t = tAt(e); if (t == null || e.clientX < box.querySelector('.lane').getBoundingClientRect().left) return;
    tlLeaveLive();
    drag = true; tl.dragging = true; was = tl.playing; if (was) tlPlay(false);
    box.setPointerCapture(e.pointerId);
    tl.T = t; drawHead(); previewAll();
  });
  // Highlight the alert marker the pointer is on (or a few pixels from),
  // so close markers are easy to pick.
  let hot = null;
  const setHot = (m) => { if (hot === m) return; if (hot) hot.classList.remove('hot'); hot = m; if (hot) hot.classList.add('hot'); box.style.cursor = hot ? 'pointer' : ''; };
  const hov = () => $('tl-hover');
  box.addEventListener('pointerleave', () => { setHot(null); if (hov()) hov().hidden = true; });
  box.addEventListener('pointermove', (e) => {
    if (!drag && !handle) setHot(markAt(e));
    // A guide line with the time under the pointer.
    const g = hov(), lane = box.querySelector('.lane'); if (!g || !lane) return;
    const r = lane.getBoundingClientRect(), p = (e.clientX - r.left) / r.width;
    g.hidden = drag || p < 0 || p > 1;
    if (!g.hidden) { g.style.left = `calc(var(--tl-label) + (100% - var(--tl-label)) * ${p})`; g.querySelector('em').textContent = fmtTime(new Date(tl.winStart + p * tl.zoom * 1000)); }
  });
  box.addEventListener('pointermove', (e) => {
    if (handle) {
      const t = Math.min(tAt(e), Date.now());
      if (handle === 'in') tl.inT = Math.min(t, tl.outT - 1000); else tl.outT = Math.max(t, tl.inT + 1000);
      const pct = (x) => ((x - tl.winStart) / (tl.zoom * 1000) * 100);
      const sel = box.querySelector('.tl-sel'); if (sel) sel.style.cssText = selStyle(pct);
      $('tl-sel-info').textContent = `Clip ${fmtTime(new Date(tl.inT))} – ${fmtTime(new Date(tl.outT))} (${fmtClock((tl.outT - tl.inT) / 1000)})`;
      // Show the picture at the handle while dragging.
      tl.T = handle === 'in' ? tl.inT : tl.outT; drawHead(); previewSoon();
      return;
    }
    if (!drag) return; tl.T = Math.min(tAt(e), Date.now()); drawHead(); previewSoon(); edgeWatch(e);
  });
  const end = () => {
    if (handle) { handle = null; drawTimeline(); tlSync(true); hideSpritesWhenReady(); return; }
    if (!drag) return; drag = false; tl.dragging = false;
    cancelAnimationFrame(edgeRaf); edgeRaf = 0; lastX = null;
    document.querySelectorAll('#tl-videos video').forEach(v => { v._want = null; });
    // Let go: the videos come first. Dragging asked for the time-lapse of
    // every file passed over (each can take a while to make), and those
    // requests held the browser's few connections to the NVR, so the
    // videos waited for them. They're dropped, and loading the time-lapse
    // carries on once the videos show.
    cutSpriteDownloads();
    tlSync(true); if (was) tlPlay(true);
    hideSpritesWhenReady().then(() => { if (page === 'recordings' && !tl.dragging) prefetchSprites(); });
  };
  box.addEventListener('pointerup', end); box.addEventListener('pointercancel', end);
})();

// The file still being recorded keeps growing: ask for its newer preview
// every half minute (the old one stays in use meanwhile).
const isGrowing = (seg) => Date.now() - new Date(seg.end).getTime() < 120000;
// A file's preview (info + page addresses), or null. A failed try is asked
// again after a pause that grows with each failure (5 s … 5 min); a file
// still being recorded is refreshed every 30 s. While asking again, the
// previous good preview keeps being used.
async function spriteFor(cam, seg) {
  if (!TIMELAPSE) return null;
  const key = seg.url, have = tl.sprites.get(key), now = Date.now();
  const again = !have || (have.done && ((isGrowing(seg) && now - have.at > 30000) || (have.failed && now >= have.retryAt)));
  if (again) {
    const tries = have && have.failed ? have.tries + 1 : 0;
    const kind = {clip: 'clip', sub: 'tl'}[seg.kind] || 'rec';
    const entry = {at: now, tries, ok: have && have.ok};
    entry.p = Promise.race([api(`api/sprite/${enc(cam)}/${kind}/${enc(seg.name)}`, {signal: pageNet.signal}), new Promise((_, no) => setTimeout(() => no(new Error('timed out')), 120000))])
      .then(preloadSprite).catch(() => null)
      .then(sp => { entry.done = true; if (sp) { entry.ok = sp; entry.failed = false; } else spriteFailed(entry); return entry.ok || null; });
    tl.sprites.set(key, entry);
  }
  const e = tl.sprites.get(key);
  return e.ok || e.p;
}
// A file's preview with every page downloaded, or null (then asked again
// later; a page that couldn't be downloaded is fetched afresh).
const pagesDone = new Set();
async function previewReady(cam, sg) {
  const sp = await spriteFor(cam, sg);
  if (!sp) return null;
  const ok = await Promise.all([...Array(sp.info.pages).keys()].map(p => pagesDone.has(sp.url + p) ? true :
    new Promise(r => { const im = spritePage(sp.url + p);
      const done = () => { if (im.naturalWidth) { pagesDone.add(sp.url + p); r(true); } else if (im._cut) r('cut'); else { if (spritePages.get(sp.url + p) === im) spritePages.delete(sp.url + p); r(false); } };
      if (im.complete) done(); else { im.addEventListener('load', done, {once: true}); im.addEventListener('error', done, {once: true}); } })));
  if (ok.every(x => x === true)) return sp;
  if (ok.includes('cut')) return null;  // page left: not a failure
  const e = tl.sprites.get(sg.url);
  if (e) { e.ok = null; e.done = true; spriteFailed(e); }
  return null;
}
// Previews already made for these files, asked in one go (nothing is made):
// they show as ready at once, e.g. after the page is reloaded.
async function knownPreviews(cam, segs, pv) {
  const kindOf = (sg) => ({clip: 'clip', sub: 'tl'}[sg.kind] || 'rec');
  // Already here: on the line at once.
  for (const sg of segs) { const e = tl.sprites.get(sg.url); if (e && e.ok && pv === tl.pv) markPreviewReady(sg.url, e.ok); }
  const want = segs.filter(sg => { const e = tl.sprites.get(sg.url); return !(e && e.ok); });
  if (!TIMELAPSE || !want.length) return;
  let r = {};
  try { r = await api('api/sprites-ready', {method: 'POST', json: {camera: cam, files: want.map(sg => [kindOf(sg), sg.name])}}); } catch (_) { return; }
  for (const sg of want) {
    const sp = r[kindOf(sg) + '/' + sg.name]; if (!sp) continue;
    preloadSprite(sp);
    tl.sprites.set(sg.url, {at: Date.now(), done: true, ok: sp, p: Promise.resolve(sp), tries: 0});
    pv.set(sg.url, {state: 'ready', at: Date.now(), secs: sp.info.count * sp.info.interval, step: sp.info.interval});
  }
}
function spriteFailed(entry) {
  entry.failed = true;
  entry.retryAt = Date.now() + Math.min(300000, 5000 * 2 ** Math.min(entry.tries || 0, 6));
}
// Waiting to ask again after a failure (and nothing loaded before).
function spriteBackoff(key) {
  const e = tl.sprites.get(key);
  return !!(e && e.done && e.failed && !e.ok && Date.now() < e.retryAt);
}
// Prepare previews for every file in view (nearest to the playhead first),
// showing progress, so scrubbing is instant.
// The files the time-lapse of a camera comes from: the light timeline
// copies, and the recordings of any time without a copy.
// A file's start and end in ms (worked out once: scrubbing asks often).
function segMs(x) { if (x._a == null) { x._a = Date.parse(x.start); x._b = Date.parse(x.end); } return x; }
// (Kept per list of files: dragging along the timeline asks many times a second.)
const pvSrcCache = new WeakMap();
function previewSources(segs) {
  const hit = pvSrcCache.get(segs); if (hit && hit.n === segs.length) return hit.v;
  const subs = segs.filter(x => x.kind === 'sub').map(x => segMs(x)).sort((p, q) => p._a - q._a);
  // Is [a, b] covered by one stream 2 file? (sorted: a binary search for
  // the last one starting by a + 5 s)
  const covered = (a, b) => {
    let lo = 0, hi = subs.length - 1, k = -1;
    while (lo <= hi) { const m = (lo + hi) >> 1; if (subs[m]._a <= a + 5000) { k = m; lo = m + 1; } else hi = m - 1; }
    for (let i = k; i >= 0 && i > k - 4; i--) if (subs[i]._b >= b - 5000) return true;
    return false;
  };
  const v = segs.filter(x => {
    if (x.kind === 'sub') return true;
    if (x.kind !== 'recording') return false;
    segMs(x); return !covered(x._a, x._b);
  });
  pvSrcCache.set(segs, {n: segs.length, v});
  return v;
}
// Under each camera's lane: which parts of the time-lapse are loaded and
// ready to scrub (filled), still loading (striped) or not loaded yet.
function paintPreviewStrips() {
  const W0 = tl.winStart, span = tl.zoom * 1000, pct = (t) => (t - W0) / span * 100;
  for (const el of document.querySelectorAll('#tl .tlp')) {
    const c = tl.data.find(x => x.camera === el.dataset.cam); if (!c) continue;
    const h = [];
    for (const sg of previewSources(c.segments)) {
      const s0 = new Date(sg.start).getTime(), s1 = new Date(sg.end).getTime();
      const st = (tl.pv && tl.pv.get(sg.url)) || {state: 'wait'};
      // A finished file's preview covers the whole file (its last picture can
      // be up to one step before the end); a file still being recorded, up
      // to its newest picture and one step more.
      const end = st.state === 'ready' && st.secs && isGrowing(sg) ? Math.min(Date.now(), s0 + (st.secs + (st.step || 0)) * 1000) : s1;
      const a = Math.max(0, pct(s0)), b = Math.min(100, pct(end));
      if (b <= 0 || a >= 100 || b <= a) continue;
      h.push({a, b, cls: 's-' + st.state});
    }
    el.innerHTML = solidBars(h, 15000 / span * 100, 'i');
  }
}
// A file's time-lapse showed while scrubbing: its part of the blue line
// fills in now, not only after letting go.
let stripsQueued = false;
function stripsSoon() { if (stripsQueued) return; stripsQueued = true; requestAnimationFrame(() => { stripsQueued = false; paintPreviewStrips(); }); }
function markPreviewReady(url, sp) {
  tl.pv = tl.pv || new Map();
  const had = tl.pv.get(url);
  if (had && had.state === 'ready') return;
  tl.pv.set(url, {state: 'ready', at: Date.now(), secs: sp.info.count * sp.info.interval, step: sp.info.interval});
  stripsSoon();
}
// While the timeline slides along (dragging past its end): what's already
// made in the part coming into view, asked a few times a second.
let knownSoonT = 0;
function knownPreviewsSoon() {
  if (knownSoonT) return;
  knownSoonT = setTimeout(async () => {
    knownSoonT = 0;
    const a = tl.winStart, b = a + tl.zoom * 1000;
    tl.pv = tl.pv || new Map();
    await Promise.all(tl.data.map(c => knownPreviews(c.camera, previewSources(c.segments).filter(sg => segMs(sg)._b >= a && sg._a <= b), tl.pv)));
    stripsSoon();
  }, 250);
}
async function prefetchSprites() {
  if (!TIMELAPSE) return;
  const gen = tl.pfGen = (tl.pfGen || 0) + 1;
  tl.pv = tl.pv || new Map();
  {
    const a0 = tl.winStart, b0 = a0 + tl.zoom * 1000;
    // Every camera asked at once (one after another took a while with many).
    await Promise.all(tl.data.map(c => knownPreviews(c.camera, previewSources(c.segments).filter(sg => new Date(sg.end).getTime() >= a0 && new Date(sg.start).getTime() <= b0), tl.pv)));
    if (gen !== tl.pfGen) return;
    paintPreviewStrips();
  }
  const a = tl.winStart, b = a + tl.zoom * 1000;
  const list = [];
  for (const c of tl.data) for (const sg of previewSources(c.segments)) {
    const s0 = new Date(sg.start).getTime(), s1 = new Date(sg.end).getTime();
    if (s1 >= a && s0 <= b) list.push({cam: c.camera, sg, d: Math.abs((s0 + s1) / 2 - tl.T)});
  }
  list.sort((x, y) => x.d - y.d);
  const todo = list.slice(0, 300);
  tl.pv = tl.pv || new Map();
  let i = 0, missing = 0;
  const worker = async () => {
    while (i < todo.length && gen === tl.pfGen) {
      const job = todo[i++], key = job.sg.url, had = tl.pv.get(key);
      // Already loaded (and not still growing): nothing to do.
      if (had && had.state === 'ready' && !(isGrowing(job.sg) && Date.now() - had.at > 30000)) continue;
      // Failed a moment ago: asked again later.
      if (spriteBackoff(key)) { missing++; continue; }
      if (!had || had.state !== 'ready') { tl.pv.set(key, {state: 'loading'}); paintPreviewStrips(); }
      const sp = await previewReady(job.cam, job.sg), ok = !!sp;
      if (gen !== tl.pfGen) break;
      if (ok) tl.pv.set(key, {state: 'ready', at: Date.now(), secs: sp.info.count * sp.info.interval, step: sp.info.interval});
      else if (!had || had.state !== 'ready') { missing++; tl.pv.set(key, {state: isGrowing(job.sg) || (tl.sprites.get(key) || {}).tries < 3 ? 'wait' : 'none'}); }
      else missing++;
      paintPreviewStrips();
    }
  };
  // Anything not ready yet is tried again shortly (as long as this view is
  // still the one shown), so gaps fill in by themselves.
  Promise.all([worker(), worker()]).then(() => {
    clearTimeout(tl.pfRetry);
    if (missing && gen === tl.pfGen) tl.pfRetry = setTimeout(() => { if (gen === tl.pfGen && page === 'recordings' && !document.hidden) prefetchSprites(); }, 6000);
  });
}
// A camera's current picture as the preview (scrubbing up to now).
function liveStill(sp, cam) {
  const now = Date.now();
  if (sp._liveAt && now - sp._liveAt < 2000 && sp._liveCam === cam) return;
  sp._liveAt = now; sp._liveCam = cam; sp._want = null;
  const url = `live/${enc(cam)}/frame.jpg?t=${now}`, im = new Image();
  im.onload = () => { if (!sp._want) { sp.style.backgroundImage = `url("${url}")`; sp.style.backgroundSize = '100% 100%'; sp.style.backgroundPosition = '0 0'; } };
  im.src = url;
}
// After scrubbing: keep the preview up until the video shows the new time
// (hiding it at once showed black while the video loaded).
function hideSpritesWhenReady() {
  const all = [];
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const sp = el.querySelector('.sprite');
    if (sp.hidden) continue;
    const gen = sp._gen = (sp._gen || 0) + 1;
    let ready; all.push(new Promise(r => ready = r));
    // Fade the time-lapse picture out over the video (no hard cut).
    const fade = () => {
      ready();
      if (sp._gen !== gen || sp.classList.contains('fading')) return;
      sp.classList.add('fading');
      setTimeout(() => { if (sp._gen === gen) { sp.hidden = true; sp.classList.remove('fading'); } }, 220);
    };
    // Only once the video (the player that will be shown) has actually
    // drawn the picture at the new time; however long that takes to load.
    const check = () => {
      if (sp._gen !== gen) return;
      const v = activeVideo(el);
      if (el._pending == null && v.style.visibility !== 'hidden' && v.readyState >= 2 && !v.seeking) {
        if (v.requestVideoFrameCallback) { v.requestVideoFrameCallback(() => fade()); setTimeout(fade, 400); } else fade();
        return;
      }
      setTimeout(check, 60);
    };
    setTimeout(check, 60);
    setTimeout(fade, 15000);
  }
  return Promise.all(all);
}
// Pointer moves come faster than the screen redraws: show the pictures
// once per frame, for the newest position.
let previewQueued = false;
function previewSoon() { if (previewQueued) return; previewQueued = true; requestAnimationFrame(() => { previewQueued = false; if (tl.dragging || tl.T != null) previewAll(); }); }
function previewAll() {
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const c = tl.data.find(x => x.camera === el.dataset.cam);
    el.querySelectorAll('.tl-t').forEach(t => t.textContent = fmtDate(new Date(tl.T), t.dataset.fmt || undefined));
    const T = tl.T;
    // The time-lapse is of the 24/7 recording (the blue line), whichever
    // stream is chosen: dragging shows it even where the chosen stream (say
    // Main stream, recorded only for alerts) has no footage; letting go
    // there says so.
    const ps = c && previewSources(c.segments).find(x => x._a <= T && T < (isGrowing(x) ? Date.now() : x._b) + 1000);
    // (The pictures don't depend on how the video plays: also for cameras
    // whose HD footage plays converted.)
    const s = (c && segAt(c, T)) || (tl.scrub === 'timelapse' ? ps : null), sp = el.querySelector('.sprite');
    if (sp.classList.contains('fading')) { sp._gen = (sp._gen || 0) + 1; sp.classList.remove('fading'); }
    el.querySelector('.msg').hidden = !!s;
    if (!s) [...el._vids, el._hlsV].forEach(x => x.style.visibility = 'hidden');
    if (!s) { sp.hidden = true; continue; }
    // Scrubbing with the video itself (sub-stream or main stream): seek it.
    if (tl.scrub !== 'timelapse' && !el._tc) {
      const vs = segAt(c, T, tl.scrub) || s;
      sp.hidden = true;
      scrubVideo(el, vs, (T - new Date(vs.start).getTime()) / 1000);
      continue;
    }
    // The last moments (newer than the preview): the live picture.
    if (Date.now() - T < 15000) { liveStill(sp, c.camera); sp.hidden = false; continue; }
    const src = ps || s;
    spriteFor(c.camera, src).then(info => {
      if (info) markPreviewReady(src.url, info);
      if (!info || tl.T !== T) return;
      const off = (T - new Date(src.start).getTime()) / 1000;
      if (spriteStyle(sp, info, Math.floor(off / info.info.interval), el.clientWidth, el.clientHeight, true)) sp.hidden = false;
    });
  }
}
// Seek a camera's video while scrubbing: one seek at a time, always to the
// newest wanted moment, timing how long each picture takes to show.
function scrubVideo(el, seg, at) {
  const v = activeVideo(el);
  showVideo(el, el._active);
  v._want = Math.max(0, at);
  if (!v._scrubWired) {
    v._scrubWired = true;
    v._go = () => { if (v._want == null || v.readyState < 1 || v.seeking) return; if (v.fastSeek && tl.scrub === 'main') v.fastSeek(v._want); else v.currentTime = v._want; };
    v.addEventListener('seeked', () => {
      if (v._want != null && Math.abs(v.currentTime - v._want) > 0.3) v._go();
    });
    v.addEventListener('loadedmetadata', () => v._go());
  }
  if (v.dataset.url !== seg.url) { v.pause(); v.dataset.url = seg.url; v.preload = 'auto'; v.src = seg.url; return; }
  if (!v.paused) v.pause();
  v._go();
}

// ======================================================================
// System monitor
// ======================================================================
let sysRange = 3600, sysTimer = null, sysHist = [];
// The computer's load over time: [key, name, colour, key of its peaks].
const SERIES = [
  ['cpu', 'CPU', '#4f8cff', 'cpu_max'],
  ['gpu', 'GPU', '#a78bfa', 'gpu_max'],
  ['ram', 'Memory', '#f59e0b', null],
];
// Graph lines switched off by the viewer (remembered).
const graphHidden = new Set(store.get('graph-hidden', []));

// ---------- System → Tasks ----------
const PAUSE_KINDS = [['ai', 'AI on alerts'], ['previews', 'Previews & pictures'], ['cloud', 'Cloud backup'], ['maintenance', 'Maintenance']];
async function setPaused(kind, paused) {
  try { await api('api/jobs/pause', {method: 'POST', json: {kind, paused}}); } catch (e) { toast(e.message, true); }
  loadJobs();
}
const JOB_ICON = {ai: '🧠', download: '⬇️', previews: '🖼️', cloud: '☁️', scan: '📡', maintenance: '🛠️'};
let jobsTimer = null, showDoneJobs = false;
function fmtAgo(t) {
  const s = Math.max(0, (Date.now() - new Date(t)) / 1000);
  return s < 60 ? Math.round(s) + ' s ago' : s < 3600 ? Math.round(s / 60) + ' min ago' : Math.round(s / 3600) + ' h ago';
}
function fmtTook(a, b) {
  const s = Math.max(0, (new Date(b || Date.now()) - new Date(a)) / 1000);
  return s < 60 ? Math.round(s) + ' s' : s < 3600 ? Math.round(s / 60) + ' min' : (s / 3600).toFixed(1) + ' h';
}
async function loadJobs() {
  let r; try { r = await api('api/jobs'); } catch (_) { return; }
  const live = r.jobs.filter(j => !j.finished), done = r.jobs.filter(j => j.finished);
  const row = (j) => {
    const pct = j.total ? Math.min(100, 100 * j.done / j.total) : null;
    const prog = j.bytes ? `${fmtBytes(j.done)}${j.total ? ' of ' + fmtBytes(j.total) : ''}` : j.total ? `${j.done} of ${j.total}` : j.done ? `${j.done} done` : '';
    const isPaused = (r.paused || []).includes(j.kind);
    const state = j.state === 'paused' || (isPaused && !j.finished) ? '<span class="paused-tag">⏸ paused</span>' : j.state === 'waiting' ? '<span class="muted">waiting for spare CPU</span>' : j.state === 'failed' ? '<span class="errc">failed</span>' : j.state === 'done' ? '<span class="okc">done</span>' : '<span class="spin-dot"></span>';
    return `<div class="job ${j.state}"><div class="row"><span class="ji">${JOB_ICON[j.kind] || '•'}</span><b style="flex:1">${esc(j.title)}</b>${state}${j.pausable && !j.finished && me && me.role === 'admin' ? `<button type="button" class="small job-pause" data-kind="${j.kind}" data-p="${isPaused ? 0 : 1}" title="${isPaused ? 'Carry on with this kind of work' : 'Stop this kind of work for now to lighten the load, until you resume it'}">${isPaused ? '▶ Resume' : '⏸ Pause'}</button>` : ''}</div>
      <div class="muted small">${esc(j.detail)}${prog ? ` · ${prog}` : ''} · ${j.finished ? 'took ' + fmtTook(j.started, j.finished) + ', ' + fmtAgo(j.finished) : 'running ' + fmtTook(j.started)}</div>
      ${j.error ? `<div class="errc small">${esc(j.error)}</div>` : ''}
      ${!j.finished && pct != null ? `<div class="bar"><div style="width:${pct.toFixed(1)}%"></div></div>` : ''}</div>`;
  };
  const target = $('set-tasks') || $('sys-tasks');
  const w = $('tasks-waiting');
  if (w && r.waiting) w.innerHTML = `<span class="wait-pill">${r.waiting.identify} waiting for faces &amp; plates</span>`;
  const pc = $('tasks-pause');
  if (pc && me && me.role === 'admin') {
    const p = r.paused || [], all = PAUSE_KINDS.every(([k]) => p.includes(k));
    pc.innerHTML = `<span class="muted">Background work (click to pause or resume):</span>` + PAUSE_KINDS.map(([k, t]) =>
      `<button type="button" class="small pause-chip ${p.includes(k) ? 'on' : ''}" data-kind="${k}" data-p="${p.includes(k) ? 0 : 1}" title="${p.includes(k) ? 'Paused — click to resume' : 'Running — click to pause'}"><i class="pdot"></i>${t}${p.includes(k) ? ' — paused' : ''}</button>`).join('') +
      `<button type="button" class="small ${all ? 'primary' : ''}" data-kind="all" data-p="${all ? 0 : 1}">${all ? '▶ Resume all' : '⏸ Pause all'}</button>` +
      (p.length ? `<div class="hint" style="width:100%">Paused work waits until you resume it (or GAZE restarts). Recording, live view and new alerts carry on as normal.</div>` : '');
  }
  target.innerHTML = (live.length ? live.map(row).join('') : '<div class="hint">Nothing running right now.</div>') +
    (done.length ? `<details class="jobs-done" ${showDoneJobs ? 'open' : ''}><summary>Finished (${done.length})</summary>${done.map(row).join('')}</details>` : '');
  const d = target.querySelector('details'); if (d) d.ontoggle = () => { showDoneJobs = d.open; };
  [target, pc].forEach(el => el && el.querySelectorAll('button[data-kind]').forEach(b => b.onclick = () => setPaused(b.dataset.kind, b.dataset.p === '1')));
}

// ---------- update notice ----------
// A bar under the header when a newer GAZE is out (admins), until updated
// or dismissed for that version.
function showUpdateBar(u) {
  const bar = $('upd-bar');
  const hide = () => { bar.classList.remove('show'); bar.hidden = true; bar.dataset.v = ''; };
  if (!u || !me || me.role !== 'admin') { hide(); return; }
  const from = esc(status.version || ''), to = esc(u.latest || '');
  const head = (title, sub, icon = '⬆') => `<div class="ub-icon">${icon}</div><div class="ub-text"><b>${title}</b><small>${sub}</small></div>`;
  const busy = ['downloading', 'installing', 'restarting'].includes(u.state);
  if (busy) {
    const pct = u.total ? Math.min(100, Math.round(u.done / u.total * 100)) : 0;
    const what = u.state === 'downloading' ? (u.total ? `Downloading… ${pct}% (${fmtBytes(u.done)} of ${fmtBytes(u.total)})` : 'Downloading…')
      : u.state === 'installing' ? 'Installing…' : 'Restarting GAZE… the page reloads by itself.';
    bar.dataset.v = 'busy';
    // A ring that fills with the download, around an arrow dropping into a tray.
    const ic = `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><g class="ub-arrow"><path d="M12 3v11"/><path d="M7.5 9.5 12 14l4.5-4.5"/></g><path d="M5 19h14"/></svg>`;
    bar.style.setProperty('--ub-pct', (u.state === 'downloading' ? pct : 100) + '%');
    bar.innerHTML = head(`Updating to GAZE ${to}`, `<span class="ub-ver">${from} → ${to}</span> ${what}`, ic)
      + `<div class="ub-prog"><i style="width:${u.state === 'downloading' ? pct : 100}%"></i></div>`;
    bar.classList.toggle('ub-busy', true);
    bar.hidden = false; requestAnimationFrame(() => bar.classList.add('show'));
    if (u.state === 'restarting') waitForGazeRestart();
    return;
  }
  bar.classList.remove('ub-busy');
  if (!u.available || store.get('upd-dismissed', '') === u.latest) { hide(); return; }
  const key = u.latest + '|' + (u.error || '');
  if (!bar.hidden && bar.dataset.v === key) return;
  bar.dataset.v = key;
  const sub = u.error ? `<span class="ub-err">The last try didn't work: ${esc(u.error)}</span>`
    : `<span class="ub-ver">${from} → ${to}</span>`;
  bar.innerHTML = head(`GAZE ${to} is ready to install`, sub)
    + `<div class="ub-actions"><button type="button" class="primary small" id="ub-go">${u.error ? 'Try again' : 'Update now'}</button>
       <button type="button" class="small ub-later" id="ub-x" title="Hide until the next version">Later</button></div>`;
  bar.hidden = false; requestAnimationFrame(() => bar.classList.add('show'));
  $('ub-x').onclick = () => { store.set('upd-dismissed', u.latest); hide(); };
  $('ub-go').onclick = async () => {
    const go = $('ub-go'); go.disabled = true; go.textContent = 'Starting…';
    try { await api('api/update/install', {method: 'POST'}); const busy = {...u, state: 'downloading', done: 0, total: 0, error: null}; showUpdateBar(busy); paintUpdBtn(busy); }
    catch (e) { toast(e.message, true); go.disabled = false; go.textContent = 'Update now'; }
  };
}
let restartWait = null;
function waitForGazeRestart() {
  if (restartWait) return;
  restartWait = setInterval(async () => {
    try { const r = await fetch('api/update'); if (r.ok && (await r.json()).state !== 'restarting') { clearInterval(restartWait); location.reload(); } } catch (_) {}
  }, 2000);
}
// Phones: the account menu has System, Logs and the update check.
if ($('um-upd')) $('um-upd').onclick = () => { $('um-pop').hidden = true; $('upd-btn').click(); };
document.querySelectorAll('.um-links a').forEach(a => a.addEventListener('click', () => { $('um-pop').hidden = true; }));
// After an update: once, a window saying which version is now running and
// what's new in it (from the changelog built into GAZE).
async function showWhatsNew() {
  const v = status && status.version; if (!v) return;
  const last = store.get('seen-version', '');
  store.set('seen-version', v);
  if (!last || last === v) return;
  let r; try { r = await api('api/whatsnew'); } catch (_) { return; }
  const lines = (r.text || '').split('\n').filter(l => l.trim());
  const md = (t) => esc(t).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/`(.+?)`/g, '<code>$1</code>');
  const dlg = document.createElement('dialog');
  dlg.className = 'wn-dlg';
  dlg.innerHTML = `<div class="head"><h2>🎉 You're now running GAZE ${esc(v)}</h2><span class="spacer"></span><button type="button" class="small" data-x>✕</button></div>
    <div class="body"><p class="muted" style="margin-top:0">Updated from ${esc(last)}. What's new:</p>
      ${lines.length ? `<ul>${lines.map(l => `<li>${md(l.replace(/^-\s*/, ''))}</li>`).join('')}</ul>` : '<p>Fixes and improvements.</p>'}
      <div class="actions"><button type="button" class="primary" data-x>Got it</button></div></div>`;
  document.body.appendChild(dlg);
  const close = () => { dlg.close(); dlg.remove(); };
  dlg.querySelectorAll('[data-x]').forEach(b => b.onclick = close);
  dlg.addEventListener('click', (e) => { if (e.target === dlg) close(); });
  dlg.showModal();
}
// The header's Updates button: spins while checking; while an update is
// installing, a ring fills with the download and the label says what it's
// doing; a dot when a new version is ready.
function paintUpdBtn(u) {
  const b = $('upd-btn'); if (!b) return;
  const busy = u && ['downloading', 'installing', 'restarting'].includes(u.state);
  // Downloading starts at 0% (also before the size is known); installing
  // and restarting show a full ring.
  const pct = !busy ? 0 : u.state === 'downloading' ? (u.total ? Math.min(100, Math.round(u.done / u.total * 100)) : 0) : 100;
  b.classList.toggle('busy', !!busy);
  b.classList.toggle('ready', !busy && !!(u && u.available));
  b.style.setProperty('--upd-pct', pct + '%');
  const label = b.querySelector('.upd-label');
  if (label) label.textContent = !busy ? 'Updates' : u.state === 'downloading' ? `Updating ${pct}%` : u.state === 'installing' ? 'Installing…' : 'Restarting…';
  b.title = !busy ? (u && u.available ? `GAZE ${u.latest} is ready to install` : 'Check for updates') : label ? label.textContent : 'Updating';
}
$('upd-btn').onclick = async () => {
  const b = $('upd-btn'); b.disabled = true;
  b.classList.remove('ok'); b.classList.add('checking');
  const tick = b.querySelector('.upd-tick'); if (tick) tick.remove();
  try {
    const u = await api('api/update/check', {method: 'POST'});
    if (u.available) { store.set('upd-dismissed', ''); showUpdateBar({...(status.update || {}), available: true, latest: u.latest, state: u.state, published: u.published, error: null}); }
    else {
      toast(u.latest ? `GAZE is up to date (${u.current}).` : 'No releases found yet.');
      // A green tick after the button: checked, nothing new.
      b.insertAdjacentHTML('beforeend', '<span class="upd-tick" title="Up to date">✓</span>');
      b.classList.add('ok');
      setTimeout(() => { b.classList.remove('ok'); const t = b.querySelector('.upd-tick'); if (t) t.remove(); }, 60000);
    }
  } catch (e) { toast(e.message, true); }
  b.classList.remove('checking');
  b.disabled = false;
};

// "What runs where": the graphics card or the processor, and why.
async function loadWhere() {
  const box = $('where'); if (!box) return;
  let w; try { w = await api('api/where'); } catch (e) { box.textContent = e.message; return; }
  const tag = (gpu) => gpu ? '<span class="pu gpu">GPU</span>' : '<span class="pu cpu">CPU</span>';
  const row = (gpu, what, why) => `<tr><td>${tag(gpu)}</td><td>${what}${why ? `<div class="muted small">${esc(why)}</div>` : ''}</td></tr>`;
  const v = w.video, rows = [];
  rows.push(`<tr><th colspan="2">AI</th></tr>`);
  if (w.models.length) w.models.forEach(m => rows.push(row(m.gpu, esc(m.what), m.gpu ? '' : m.why)));
  else rows.push(`<tr><td></td><td class="muted">No AI model loaded yet (they load when first needed). ${esc(w.ai_device)}</td></tr>`);
  rows.push(`<tr><th colspan="2">Video</th></tr>`);
  rows.push(row(v.decoder !== 'none', `Video decoding${v.decoder !== 'none' ? ` (${esc(v.decoder)})` : ''}${v.auto ? ' — chosen automatically' : ''}`, v.why_not));
  rows.push(row(v.encoder !== 'libx264', `Converting recordings (${esc(v.encoder)})`, v.encoder === 'libx264' ? 'no GPU encoder works with this ffmpeg — only used when a camera converts or lowers its recording frame rate' : ''));
  w.cameras.forEach(c => {
    const parts = [['live view (HD)', c.live_hd], ['live grid (small)', c.live_small], ['motion detection', c.detection]].filter(([, x]) => x != null);
    rows.push(`<tr><td></td><td><b>${esc(c.name)}</b> — ${parts.map(([t, g]) => `${t}: ${tag(g)}`).join(' · ')}</td></tr>`);
  });
  if (v.moved_to_cpu) rows.push(row(false, `${v.moved_to_cpu} video job(s) moved to the processor`, 'smart load balancing: the GPU was over 85 % busy at the time'));
  rows.push(`<tr><th colspan="2">Always on the processor</th></tr>`);
  rows.push(row(false, 'Motion analysis, live-view pictures (JPEG), previews and alert pictures', 'small, quick work on pictures already decoded — moving it to the GPU would cost more in copying than it saves'));
  rows.push(row(false, 'Recording', 'needs no decoding at all: the camera\'s video is copied as it is'));
  box.className = '';
  box.innerHTML = `<table class="where"><tbody>${rows.join('')}</tbody></table>`;
}

// Header: delete all saved footage (for testing), after typing DELETE.
$('wipe-btn').onclick = async () => {
  const typed = prompt('Delete ALL saved footage?\n\nEvery recording, timeline copy, alert, clip and alert picture on every camera is removed. This cannot be undone. Cameras, settings and known people stay, and recording carries on.\n\nType DELETE to confirm:');
  if (typed === null) return;
  if (typed.trim().toUpperCase() !== 'DELETE') { toast('Not deleted (type DELETE to confirm)'); return; }
  try {
    const r = await api('api/maintenance/delete-footage', {method: 'POST'});
    toast(r.message);
    tl.sprites.clear(); if (tl.pv) tl.pv.clear();
    refreshStatus();
    if (page === 'recordings' || page === 'alerts') reloadRec();
  } catch (e) { toast(e.message, true); }
};

// ---------- Settings → Tasks ----------
const TASK_ACTIONS = [
  ['previews', '🖼️ Rebuild previews', 'Throws away the scrub and time-lapse previews and makes the last 24 hours again. Use it if previews look wrong or black.', null,
    'Throw away all scrub and time-lapse previews and make them again? Older days are remade when you look at them.'],
  ['alert-pictures', '📸 Refresh alert pictures', 'Replaces the pictures of the last 7 days of alerts with full-quality frames from the recordings (sharper thumbnails).', {days: 7}, null],
  ['reidentify', '🧑 Look for faces & plates again', 'Checks the last 7 days of alerts for faces and number plates again — after teaching GAZE new people or vehicles.', {days: 7},
    'Look for faces and number plates in the last 7 days of alerts again?'],
];
if (!TIMELAPSE) TASK_ACTIONS.splice(TASK_ACTIONS.findIndex(a => a[0] === 'previews'), 1);
function renderTasks(body) {
  body.innerHTML = `<div class="card"><h2>What the AI is doing</h2>
    <p class="muted" style="margin-top:0">Besides recording, GAZE works on alerts in the background: finding faces and number plates, preparing previews, downloading AI models and copying to the cloud. It waits for spare processor time, so it never slows down recording or live view.</p>
    <div id="tasks-pause" class="tasks-pause"></div>
    <div id="tasks-waiting" style="margin-bottom:8px"></div>
    <div id="set-tasks"></div></div>
    <div class="card"><h2>What runs where</h2><div id="where" class="muted">Reading…</div></div>
    <div class="card"><h2>Run again</h2><div class="task-actions">${TASK_ACTIONS.map(([k, t, d]) => `<div class="task-action"><button type="button" data-k="${k}">${t}</button><div class="hint">${d}</div></div>`).join('')}</div></div>`;
  body.querySelectorAll('.task-actions button').forEach(b => b.onclick = async () => {
    const [k, , , json, ask] = TASK_ACTIONS.find(a => a[0] === b.dataset.k);
    if (ask && !confirm(ask)) return;
    b.disabled = true;
    try { toast((await api('api/maintenance/' + k, {method: 'POST', json: json || {}})).message); setTimeout(loadJobs, 800); } catch (e) { toast(e.message, true); }
    setTimeout(() => { b.disabled = false; }, 3000);
  });
  loadJobs(); loadWhere();
  clearInterval(jobsTimer);
  let n = 0;
  jobsTimer = setInterval(() => { if ($('set-tasks') && location.hash.startsWith('#settings/tasks')) { loadJobs(); if (++n % 3 === 0) loadWhere(); } else clearInterval(jobsTimer); }, 2000);
}

function showSystem() {
  loadSystem(); loadJobs();
  clearInterval(jobsTimer);
  jobsTimer = setInterval(() => { if (page === 'system') loadJobs(); else clearInterval(jobsTimer); }, 2000);
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
    card('GPU', s.gpu != null ? s.gpu.toFixed(0) + '%' : '–', s.gpu != null ? `${esc(c.gpu_name || 'GPU')}${c.gpu_temp != null ? ' · ' + c.gpu_temp.toFixed(0) + ' °C' : ''}${s.gpu_mem != null ? ' · memory ' + s.gpu_mem.toFixed(0) + '%' : ''}${c.moved_to_cpu ? `<br>${c.moved_to_cpu} video job${c.moved_to_cpu === 1 ? '' : 's'} moved to the CPU while the GPU was busy` : ''}` : 'No GPU readings available (NVIDIA needs nvidia-smi; AMD on Linux and any GPU on Windows are read automatically).', s.gpu) +
    card('Memory', s.ram.toFixed(0) + '%', `${fmtBytes(c.ram_used)} of ${fmtBytes(c.ram_total)}<br>NVR + ffmpeg: ${fmtBytes(c.nvr_ram_used || 0)} (${(s.nvr_ram || 0).toFixed(1)}%)`, s.ram) +
    card('Disk', disk ? fmtBytes(disk.free_bytes) + ' free' : '–', disk ? `of ${fmtBytes(disk.total_bytes)} · ${esc(disk.path)}` : '', disk ? 100 * (1 - disk.free_bytes / disk.total_bytes) : null) +
    card('Recording rate', fmtBytes(usage.bytes_per_hour) + '/h', `${fmtBytes(usage.bytes_per_day)} per 24 hours`, null) +
    card('Days of recording', usage.days_capacity != null ? (usage.days_capacity < 2 ? usage.days_capacity.toFixed(1) + ' days' : Math.floor(usage.days_capacity) + ' days') : '–',
      usage.days_capacity != null ? `fit at the current rate (limited by ${esc(usage.limited_by)}). Retention is set to ${usage.retention_days ? usage.retention_days + ' days' : 'forever'}${usage.retention_days && usage.days_capacity < usage.retention_days ? ' — <span class="warnc">not enough space, oldest recordings will be deleted sooner</span>' : ''}.` : 'Shown once some recordings exist.', null);
  sysHist = mon.history; sysSpan = mon.range || sysRange;
  drawGraph();
  // Storage locations
  $('sys-storage').innerHTML = usage.locations.length ? `<table><thead><tr><th>Location</th><th>Cameras</th><th>Used by NVR</th><th>Free</th><th class="hide-sm">Per day</th><th>Full in</th></tr></thead><tbody>` +
    usage.locations.map(l => `<tr><td><code>${esc(l.path)}</code><div class="bar"><div style="width:${(100 * (1 - l.free_bytes / l.total_bytes)).toFixed(1)}%"></div></div></td>
      <td>${esc(l.cameras.join(', ') || '–')}</td><td>${fmtBytes(l.used_bytes)}</td><td>${fmtBytes(l.free_bytes)} of ${fmtBytes(l.total_bytes)}</td>
      <td class="hide-sm">${fmtBytes(l.bytes_per_day)}</td><td>${l.days_until_full != null ? l.days_until_full.toFixed(1) + ' days' : '–'}</td></tr>`).join('') + '</tbody></table>'
    + `<div class="hint">“Full in” is how long until free space runs out at the current rate (keeping the “free disk space” reserve). After that, the oldest recordings are deleted automatically. Total space allowed for recordings: ${usage.capacity_bytes ? fmtBytes(usage.capacity_bytes) : '–'}.</div>` : '<div class="muted">No data yet.</div>';
  loadKinds();
  $('sys-cams').innerHTML = usage.cameras.length ? `<table><thead><tr><th>Camera</th><th>Per hour</th><th>Per 24 h</th><th class="hide-sm">Stored</th><th class="hide-sm">Oldest</th><th>Keep</th></tr></thead><tbody>` +
    usage.cameras.map(u => `<tr><td>${esc(u.name)}<div class="sub">${esc(u.dir)}</div></td><td>${fmtBytes(u.bytes_per_hour)}</td><td>${fmtBytes(u.bytes_per_day)}</td>
      <td class="hide-sm">${fmtBytes(u.used_bytes)}</td><td class="hide-sm">${u.oldest ? fmtDate(u.oldest) : '–'}</td><td>${u.retention_days ? u.retention_days + ' days' : 'forever'}</td></tr>`).join('') + '</tbody></table>' : '<div class="muted">No cameras.</div>';
}

// Space taken by each kind of footage (counted by the NVR every few minutes).
const KINDS = [['sub', 'Sub-stream recordings', '#3f9d63'], ['main', 'Main stream (HD) recordings', '#7c5fd0'], ['alerts', 'Alerts (HD clips & pictures)', '#f59e0b'], ['timelapse', 'Time-lapse previews', '#22d3ee'], ['other', 'Other', '#6b7280']];
async function loadKinds() {
  if (loadKinds.busy || Date.now() - (loadKinds.at || 0) < 60000) return;
  loadKinds.busy = true;
  let list;
  try { list = await api('api/usage/kinds'); } catch (e) { return; } finally { loadKinds.busy = false; }
  loadKinds.at = Date.now();
  const tot = {}; for (const [k] of KINDS) tot[k] = list.reduce((a, u) => a + (u[k] || 0), 0);
  const sum = (u) => KINDS.reduce((a, [k]) => a + (u[k] || 0), 0);
  const bar = (u) => { const t = sum(u) || 1; return `<div class="kbar">${KINDS.map(([k, , c]) => u[k] ? `<i style="width:${(u[k] / t * 100).toFixed(2)}%;background:${c}" title="${esc(KINDS.find(x => x[0] === k)[1])}: ${fmtBytes(u[k])}"></i>` : '').join('')}</div>`; };
  const cell = (u, k) => `<td>${u[k] ? fmtBytes(u[k]) : '–'}</td>`;
  $('sys-kinds').innerHTML = `<div class="klegend">${KINDS.map(([k, t, c]) => `<span><i style="background:${c}"></i>${t}: <b>${fmtBytes(tot[k])}</b></span>`).join('')}</div>${bar(tot)}
    <table><thead><tr><th>Camera</th>${KINDS.map(([, t]) => `<th class="${t === 'Other' ? 'hide-sm' : ''}">${t.replace(/ \(.*\)| recordings| previews/g, '')}</th>`).join('')}<th>Total</th></tr></thead><tbody>
    ${list.map(u => `<tr><td>${esc(u.name)}${bar(u)}</td>${KINDS.map(([k]) => k === 'other' ? `<td class="hide-sm">${u[k] ? fmtBytes(u[k]) : '–'}</td>` : cell(u, k)).join('')}<td><b>${fmtBytes(sum(u))}</b></td></tr>`).join('')}
    </tbody></table><div class="hint">Counted every few minutes. “Sub-stream” includes the light copies kept for the timeline; time-lapse previews are deleted along with their recordings.</div>`;
}
let sysSpan = 3600;
// Time marks at round times for the range shown: [step in s, label].
function graphTicks(t0, t1) {
  const span = t1 - t0, p2 = (n) => String(n).padStart(2, '0');
  const hm = (d) => `${p2(d.getHours())}:${p2(d.getMinutes())}`;
  const day = (d) => d.toLocaleDateString(undefined, {weekday: 'short', day: 'numeric'});
  const date = (d) => d.toLocaleDateString(undefined, {day: 'numeric', month: 'short'});
  const [step, fmt] = span <= 3600 ? [600, hm] : span <= 6 * 3600 ? [3600, hm] : span <= 86400 ? [3 * 3600, hm]
    : span <= 7 * 86400 ? [86400, day] : [5 * 86400, date];
  const out = [];
  // From local midnight, so marks fall on round local times.
  const m = new Date(t0 * 1000); m.setHours(0, 0, 0, 0);
  for (let t = m.getTime() / 1000; t <= t1; t += step) if (t >= t0) out.push([t, fmt(new Date(t * 1000))]);
  return out;
}
function drawGraph() {
  const svg = $('sys-graph'), W = svg.clientWidth || 800, H = 260, h = sysHist;
  const L = 40, R = 10, T = 12, B = 26;  // margins
  svg.setAttribute('viewBox', `0 0 ${W} ${H}`);
  const live = h.filter(p => !p.gap);
  if (live.length < 2) { svg.innerHTML = `<text x="${W / 2}" y="${H / 2}" fill="#8b93a3" text-anchor="middle" font-size="13">Collecting data… (a reading every few seconds)</text>`; $('sys-legend').innerHTML = ''; return; }
  // The whole range asked for (not just where there's data).
  const t1 = h[h.length - 1].t, t0 = t1 - sysSpan;
  const X = (t) => L + (t - t0) / Math.max(1, t1 - t0) * (W - L - R), Y = (v) => T + (1 - Math.min(100, Math.max(0, v)) / 100) * (H - T - B);
  let g = '<defs>' + SERIES.map(([k, , c]) => `<linearGradient id="gg-${k}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${c}" stop-opacity=".38"/><stop offset="1" stop-color="${c}" stop-opacity="0"/></linearGradient>`).join('')
    + '<pattern id="gg-gap" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="3" height="6" fill="rgba(255,255,255,.05)"/></pattern></defs>';
  for (const v of [0, 25, 50, 75, 100]) g += `<line x1="${L}" x2="${W - R}" y1="${Y(v)}" y2="${Y(v)}" stroke="rgba(255,255,255,${v ? .06 : .14})" ${v ? 'stroke-dasharray="3 4"' : ''}/><text x="${L - 8}" y="${Y(v) + 3.5}" fill="#8b93a3" font-size="10.5" text-anchor="end">${v}%</text>`;
  for (const [t, lab] of graphTicks(t0, t1)) g += `<line x1="${X(t)}" x2="${X(t)}" y1="${T}" y2="${H - B}" stroke="rgba(255,255,255,.05)"/><text x="${X(t)}" y="${H - 8}" fill="#8b93a3" font-size="10.5" text-anchor="middle">${lab}</text>`;
  // Runs of readings between gaps (no line drawn across a gap).
  const runs = []; let cur = [];
  for (const p of h) { if (p.gap) { if (cur.length) runs.push(cur); cur = []; } else cur.push(p); }
  if (cur.length) runs.push(cur);
  // Gaps: hatched (GAZE wasn't running or the computer was off).
  let gs = null;
  const w1 = h.length > 1 ? (h[1].t - h[0].t) : 60;
  h.forEach((p, i) => { if (p.gap && gs == null) gs = p.t - w1; if ((!p.gap || i === h.length - 1) && gs != null) { const e = p.gap ? p.t : p.t - w1; g += `<rect x="${X(gs)}" y="${T}" width="${Math.max(0, X(e) - X(gs))}" height="${H - T - B}" fill="url(#gg-gap)"><title>No readings (GAZE wasn't running)</title></rect>`; gs = null; } });
  // Smooth line through the points (curves through the midpoints).
  const line = (pts) => pts.map((q, i) => i === 0 ? `M${q[0].toFixed(1)},${q[1].toFixed(1)}` : `Q${pts[i - 1][0].toFixed(1)},${pts[i - 1][1].toFixed(1)} ${((pts[i - 1][0] + q[0]) / 2).toFixed(1)},${((pts[i - 1][1] + q[1]) / 2).toFixed(1)}`).join('') + (pts.length > 1 ? `T${pts[pts.length - 1][0].toFixed(1)},${pts[pts.length - 1][1].toFixed(1)}` : '');
  const shown = SERIES.filter(([k]) => !graphHidden.has(k) && live.some(p => p[k] != null));
  for (const [k, , c, peak] of shown) {
    for (const run of runs) {
      const pts = run.filter(p => p[k] != null).map(p => [X(p.t), Y(p[k])]);
      if (pts.length < 2) continue;
      // Peaks as a faint band above the average.
      if (peak) { const top = run.filter(p => p[peak] != null).map(p => [X(p.t), Y(p[peak])]); if (top.length > 1) g += `<path d="${line(top)}L${pts[pts.length - 1][0]},${pts[pts.length - 1][1]}${pts.slice().reverse().map(q => `L${q[0].toFixed(1)},${q[1].toFixed(1)}`).join('')}Z" fill="${c}" fill-opacity=".10"/>`; }
      const d = line(pts);
      g += `<path d="${d}L${pts[pts.length - 1][0]},${Y(0)}L${pts[0][0]},${Y(0)}Z" fill="url(#gg-${k})"/>`;
      g += `<path d="${d}" fill="none" stroke="${c}" stroke-width="2" stroke-linejoin="round" stroke-linecap="round" style="filter:drop-shadow(0 0 3px ${c}66)"/>`;
    }
  }
  // Now: the latest readings.
  const last = live[live.length - 1];
  for (const [k, , c] of shown) if (last[k] != null) g += `<circle cx="${X(last.t)}" cy="${Y(last[k])}" r="3.5" fill="${c}" stroke="#0f1218" stroke-width="2"/>`;
  g += `<g id="g-cursor" visibility="hidden"><line y1="${T}" y2="${H - B}" stroke="#fff" stroke-opacity=".35"/>${shown.map(([k, , c]) => `<circle data-k="${k}" r="4.5" fill="${c}" stroke="#fff" stroke-width="1.5"/>`).join('')}</g>`;
  svg.innerHTML = g;
  // Legend: now, the average and the peak over the range shown.
  const stat = (k, peak) => { const v = live.map(p => p[k]).filter(x => x != null); if (!v.length) return null;
    return {now: last[k], avg: v.reduce((a, b) => a + b, 0) / v.length, max: Math.max(...live.map(p => p[peak || k]).filter(x => x != null))}; };
  $('sys-legend').innerHTML = SERIES.filter(([k]) => live.some(p => p[k] != null)).map(([k, n, c, peak]) => {
    const st = stat(k, peak);
    return `<span data-k="${k}" class="gl ${graphHidden.has(k) ? 'off' : ''}" title="Click to show or hide" style="--c:${c}"><i></i><b>${n}</b><em>${st.now != null ? st.now.toFixed(0) + '%' : '–'}</em><small>avg ${st.avg.toFixed(0)}% · peak ${st.max.toFixed(0)}%</small></span>`;
  }).join('');
  $('sys-legend').querySelectorAll('span').forEach(el => el.onclick = () => {
    const k = el.dataset.k;
    if (graphHidden.has(k)) graphHidden.delete(k); else graphHidden.add(k);
    store.set('graph-hidden', [...graphHidden]);
    drawGraph();
  });
  // Hover: a line, a dot on each line and the readings of that moment.
  const wrap = svg.parentElement; wrap.style.position = 'relative';
  let tip = wrap.querySelector('.g-tip'); if (!tip) { tip = document.createElement('div'); tip.className = 'g-tip'; tip.hidden = true; wrap.appendChild(tip); }
  svg.onmousemove = (e) => {
    const r = svg.getBoundingClientRect(), x = (e.clientX - r.left) / r.width * W;
    const t = t0 + (x - L) / (W - L - R) * (t1 - t0);
    const p = h.reduce((a, b) => Math.abs(b.t - t) < Math.abs(a.t - t) ? b : a);
    const cur = svg.querySelector('#g-cursor'); cur.setAttribute('visibility', 'visible');
    cur.querySelector('line').setAttribute('x1', X(p.t)); cur.querySelector('line').setAttribute('x2', X(p.t));
    cur.querySelectorAll('circle').forEach(c => { const v = p[c.dataset.k]; c.setAttribute('cx', X(p.t)); c.setAttribute('cy', Y(v || 0)); c.setAttribute('visibility', p.gap || v == null ? 'hidden' : 'visible'); });
    tip.innerHTML = `<div class="tt">${fmtDate(new Date(p.t * 1000))}</div>` + (p.gap ? '<div class="muted">No readings</div>'
      : shown.filter(([k]) => p[k] != null).map(([k, n, c, peak]) => `<div><i style="background:${c}"></i>${n}<b>${p[k].toFixed(0)}%</b>${peak && p[peak] != null && p[peak] > p[k] + 1 ? `<small>peak ${p[peak].toFixed(0)}%</small>` : ''}</div>`).join(''));
    tip.hidden = false;
    const px = X(p.t) / W * r.width, tw = tip.offsetWidth;
    tip.style.left = Math.min(r.width - tw - 4, Math.max(4, px + 14 > r.width - tw ? px - tw - 14 : px + 14)) + 'px';
    tip.style.top = '14px';
  };
  svg.onmouseleave = () => { svg.querySelector('#g-cursor').setAttribute('visibility', 'hidden'); tip.hidden = true; };
}
window.addEventListener('resize', () => { if (page === 'system') drawGraph(); });

// ======================================================================
// Logs
// ======================================================================
let logLast = 0, logTimer = null;
$('log-badge').addEventListener('click', (e) => { e.preventDefault(); e.stopPropagation(); $('lg-level').value = 'error'; if (page === 'settings' && section === 'logs') reloadLogs(); else location.hash = '#settings/logs'; });
$('lg-dismiss').onclick = () => {
  const ids = (status && status.problems.error_ids) || [];
  if (ids.length) store.set('errors-seen', ids[ids.length - 1]);
  refreshStatus();
};
function showLogs() {
  if (status) {
    const sel = $('lg-cam'), cur = sel.value;
    sel.innerHTML = '<option value="">All cameras</option>' + status.cameras.map(c => `<option>${esc(c.name)}</option>`).join('');
    sel.value = cur;
  }
  reloadLogs();
  clearInterval(logTimer);
  logTimer = setInterval(() => { if (page !== 'settings' || section !== 'logs') return clearInterval(logTimer); if ($('lg-live').checked) loadLogs(true); }, 3000);
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
const SECTIONS = ['cameras', 'profiles', 'storage', 'display', 'ai', 'known', 'faces', 'notify', 'smarthome', 'voice', 'tasks', 'hardware', 'cloud', 'backup', 'users', 'system', 'logs', 'info'];
let section = 'cameras';
const camPreviewUnbind = [];

async function loadSettings() { [settings, cameraCfgs] = await Promise.all([api('api/settings'), api('api/cameras')]); }

// ---------- searching the settings ----------
// Every setting's name (from each settings page and the camera window's
// tabs): [section, name, camera tab].
const SETTINGS_INDEX = [["cameras", "Cameras"], ["profiles", "Profiles"], ["storage", "Default recordings folder"], ["storage", "Default file length"], ["storage", "Keep recordings for"], ["storage", "Keep alerts for"], ["storage", "Maximum storage"], ["storage", "Keep free on disk"], ["storage", "Time-lapse pictures"], ["storage", "Recording & storage"], ["storage", "Where each camera saves"], ["storage", "🖼️ New time-lapse settings"], ["storage", "Move existing recordings to the new folder"], ["storage", "Time-lapse previews"], ["display", "Smoothness in the grid"], ["display", "Picture size in the grid"], ["display", "Smoothness when opened"], ["display", "Picture quality"], ["display", "Data saver (this browser)"], ["display", "Live pictures (this browser)"], ["display", "Full-screen button"], ["display", "Preview"], ["display", "Date & time format"], ["display", "Live view"], ["display", "Keep HD streams ready in the background"], ["ai", "Run AI on"], ["ai", "AI model"], ["ai", "Model file"], ["ai", "Picture size"], ["ai", "Class names"], ["ai", "Double-check alerts"], ["ai", "How GAZE's AI works"], ["ai", "Object recognition (AI)"], ["ai", "Search alerts by description"], ["ai", "Night boost"], ["hardware", "Video decoding"], ["hardware", "Video encoder"], ["hardware", "Graphics card device"], ["hardware", "Your graphics card"], ["hardware", "GPU acceleration"], ["hardware", "Choose automatically (recommended)"], ["hardware", "Also use the graphics card for sub-streams"], ["hardware", "Share the work between graphics card and processor"], ["cloud", "Folder"], ["cloud", "NAS login"], ["cloud", "NAS address"], ["cloud", "Login"], ["cloud", "Step 1"], ["cloud", "Step 2"], ["cloud", "Step 3"], ["cloud", "Backup — cloud or NAS"], ["cloud", "Copy new recordings there automatically"], ["backup", "Export setup"], ["backup", "Import setup"], ["backup", "Include passwords, keys and tokens"], ["backup", "Keep this NVR's login and web address"], ["users", "Stay signed in for"], ["users", "Sign out when not used"], ["users", "User name"], ["users", "Password"], ["users", "Access"], ["users", "Cameras"], ["users", "Users"], ["users", "Signing in"], ["users", "Add a user"], ["known", "＋ Photo"], ["known", "New faces & number plates"], ["known", "People & vehicles"], ["known", "Known people and vehicles"], ["known", "Look for faces in alerts and name the people below"], ["known", "Read number plates of vehicles in alerts"], ["faces", "Check faces"], ["notify", "Topic"], ["notify", "ntfy server"], ["notify", "Access token"], ["notify", "Notify for"], ["notify", "Cameras"], ["notify", "How often"], ["notify", "At night"], ["notify", "Open GAZE at"], ["notify", "Phone notifications"], ["notify", "Home, Away and Night"], ["notify", "Send notifications"], ["notify", "Warn me when something's wrong"], ["notify", "Send a summary when the alert is over"], ["notify", "While at Home, switch to Night by itself during the night hours above"], ["smarthome", "Broker address"], ["smarthome", "Login"], ["smarthome", "Topic prefix"], ["smarthome", "Private link (works without signing in — keep it to yourself)"], ["smarthome", "Home Assistant (MQTT)"], ["smarthome", "Webhooks"], ["smarthome", "Overview of all cameras"], ["smarthome", "Connect to an MQTT broker"], ["smarthome", "Add everything to Home Assistant automatically"], ["smarthome", "Include the alert picture"], ["smarthome", "Only cameras with something going on"], ["voice", "Voice"], ["voice", "Download voices"], ["voice", "Speed"], ["voice", "Loudness"], ["voice", "Ready phrases"], ["voice", "Try it"], ["voice", "Sounds for"], ["voice", "Warnings"], ["voice", "Voice & talking"], ["voice", "Play a short chime first, to get attention"], ["voice", "Speak warnings between the wails (in the voice above)"], ["tasks", "What the AI is doing"], ["tasks", "What runs where"], ["tasks", "Run again"], ["system", "Name of this system"], ["system", "Web address"], ["system", "Port"], ["system", "Reachable from"], ["system", "Secure port"], ["system", "Certificate"], ["system", "ffmpeg program"], ["system", "System"], ["system", "trust its own certificate"], ["info", "Update from"], ["info", "About"], ["info", "Updates"], ["info", "What's new"], ["info", "Install updates automatically"], ["camera", "Name", "general"], ["camera", "Camera brand", "general"], ["camera", "IP address", "general"], ["camera", "Camera login", "general"], ["camera", "Main stream (HD)", "general"], ["camera", "Sub-stream (low resolution)", "general"], ["camera", "Rotate picture", "general"], ["camera", "Privacy areas", "general"], ["camera", "Fisheye (360°) camera", "general"], ["camera", "Stream live", "general"], ["camera", "Connection type", "general"], ["camera", "Picture in the grid", "general"], ["camera", "Smoothness in the grid", "general"], ["camera", "Smoothness when opened", "general"], ["camera", "Camera enabled", "general"], ["camera", "Show a flat picture instead of the circle", "general"], ["camera", "Stream this camera live", "general"], ["camera", "Recording mode", "recording"], ["camera", "Schedule", "recording"], ["camera", "Recording frame rate", "recording"], ["camera", "File length", "recording"], ["camera", "Storage folder", "recording"], ["camera", "Keep recordings for", "recording"], ["camera", "Space for this camera", "recording"], ["camera", "Only record on a schedule", "recording"], ["camera", "Also record the sub-stream, for a quick timeline", "recording"], ["camera", "Convert recordings to H.264", "recording"], ["camera", "Move existing recordings to the new folder", "recording"], ["camera", "Sensitivity", "detection"], ["camera", "Smallest movement", "detection"], ["camera", "Must move for", "detection"], ["camera", "Must move at least", "detection"], ["camera", "Ignore areas", "detection"], ["camera", "Brush", "detection"], ["camera", "Record before alert", "detection"], ["camera", "Record after activity stops", "detection"], ["camera", "Join alerts within", "detection"], ["camera", "Analysis frame rate", "detection"], ["camera", "Motion detection", "detection"], ["camera", "Recording", "audio"], ["camera", "Live sound", "audio"], ["camera", "Two-way audio", "audio"], ["camera", "Sound detection", "audio"], ["camera", "Sensitivity", "audio"], ["camera", "Record audio", "audio"], ["camera", "Speaker button on the live picture", "audio"], ["camera", "Talk through this camera's speaker", "audio"], ["camera", "Listen for sounds and alert on them", "audio"], ["camera", "Alert on", "ai"], ["camera", "How sure the AI must be", "ai"], ["camera", "What makes an alert", "ai"], ["camera", "Double-check in full quality (HD)", "ai"], ["camera", "AI looks every", "ai"], ["camera", "Stationary objects", "ai"], ["camera", "Package delivery", "ai"], ["camera", "Brush", "ai"], ["camera", "Camera's own detection", "ai"], ["camera", "Object recognition (AI)", "ai"], ["camera", "Stop alerting for objects that stay in one place", "ai"], ["camera", "Notice parcels delivered or picked up", "ai"], ["camera", "Use what the camera itself detects", "ai"], ["camera", "Also the camera's plain motion detection", "ai"], ["camera", "Preview", "osd"], ["camera", "Date & time format", "osd"], ["camera", "Use settings just for this camera", "osd"], ["camera", "Camera address", "ptz"], ["camera", "Control port", "ptz"], ["camera", "Control login", "ptz"], ["camera", "Speed", "ptz"], ["camera", "Patrol", "ptz"], ["camera", "This camera can pan, tilt or zoom", "ptz"], ["camera", "Reverse left/right", "ptz"], ["camera", "Reverse up/down", "ptz"], ["camera", "0) ? 'checked' : ''}> Roam between the ticked presets,", "ptz"], ["camera", "Follow people (auto-tracking)", "ptz"], ["camera", "ONVIF camera settings", "device"], ["camera", "Camera IP address and network (ONVIF)", "device"], ["camera", "Camera date & time / time zone", "device"], ["camera", "Camera video streams: resolution, frame rate, bitrate", "device"], ["camera", "Camera picture: brightness, contrast, day / night", "device"], ["camera", "Open the camera's web page", "device"]];
const CAM_TABS = {general: 'General', recording: 'Recording', detection: 'Motion', ai: 'AI', audio: 'Audio', osd: 'On-screen display', ptz: 'Pan / tilt / zoom', device: 'ONVIF', zones: 'Zones'};
function sectionName(sec) { const a = document.querySelector(`#side a[data-s="${sec}"]`); return a ? a.textContent.trim() : sec; }
// Find a setting's row on the page (or in the camera window) and show it.
function flashSetting(root, name) {
  const want = name.toLowerCase();
  const el = [...root.querySelectorAll('label, h2, h3, .check, b')].find(e => e.textContent.trim().toLowerCase().startsWith(want.slice(0, 40)));
  if (!el) return;
  const row = el.closest('label, .check') || el;
  row.scrollIntoView({block: 'center', behavior: 'smooth'});
  row.classList.remove('set-flash'); void row.offsetWidth; row.classList.add('set-flash');
  setTimeout(() => row.classList.remove('set-flash'), 2400);
}
async function goToSetting([sec, name, tab]) {
  $('set-res').hidden = true;
  if (sec === 'camera') {
    // A camera's own setting: in its window (the first camera's, as an example).
    location.hash = '#settings/cameras';
    if (!cameraCfgs.length) { toast('Add a camera first — this is in each camera’s window', true); return; }
    await openCameraDialog(cameraCfgs[0].id);
    const b = document.querySelector(`#c-tabs [data-t="${tab}"]`); if (b) b.click();
    setTimeout(() => flashSetting($('cam-dialog'), name), 150);
    toast(`${CAM_TABS[tab] || tab} tab — every camera has this in its own window`);
    return;
  }
  if (location.hash !== '#settings/' + sec) location.hash = '#settings/' + sec;
  else showSettings(sec);
  // (Drawn after the page's settings load.)
  let tries = 0;
  const look = () => { const body = $('settings-body'); if (body && body.textContent.toLowerCase().includes(name.toLowerCase().slice(0, 30))) flashSetting(body, name); else if (tries++ < 20) setTimeout(look, 150); };
  setTimeout(look, 100);
}
(function wireSettingsSearch() {
  const q = $('set-q'), res = $('set-res'); if (!q) return;
  let hits = [], at = -1;
  const paint = () => {
    const words = q.value.toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) { res.hidden = true; return; }
    const where = (x) => x[0] === 'camera' ? `Camera window › ${CAM_TABS[x[2]] || x[2]}` : sectionName(x[0]);
    hits = SETTINGS_INDEX.filter(x => { const t = (x[1] + ' ' + where(x)).toLowerCase(); return words.every(w => t.includes(w)); }).slice(0, 12);
    at = hits.length ? 0 : -1;
    res.innerHTML = hits.length ? hits.map((x, i) => `<button type="button" data-i="${i}" class="${i === at ? 'on' : ''}"><b>${esc(x[1])}</b><small>${esc(where(x))}</small></button>`).join('')
      : '<div class="muted small" style="padding:8px">No settings match.</div>';
    res.hidden = false;
    res.querySelectorAll('[data-i]').forEach(b => b.onclick = () => { goToSetting(hits[+b.dataset.i]); });
  };
  q.oninput = paint;
  q.onfocus = () => { if (q.value) paint(); };
  q.onkeydown = (e) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); at = Math.max(0, Math.min(hits.length - 1, at + (e.key === 'ArrowDown' ? 1 : -1))); res.querySelectorAll('[data-i]').forEach((b, i) => b.classList.toggle('on', i === at)); }
    else if (e.key === 'Enter' && hits[at]) { e.preventDefault(); goToSetting(hits[at]); }
    else if (e.key === 'Escape') { q.value = ''; res.hidden = true; }
  };
  document.addEventListener('click', (e) => { if (!e.target.closest('.set-search')) res.hidden = true; });
})();

async function showSettings(sec) {
  section = SECTIONS.includes(sec) ? sec : 'cameras';
  document.querySelectorAll('#side a').forEach(a => a.classList.toggle('active', a.dataset.s === section));
  // Drawn at once from the settings already here (fetched fresh behind,
  // and drawn again only if something changed meanwhile).
  if (settings && cameraCfgs) {
    const before = JSON.stringify([settings, cameraCfgs]), want = section;
    loadSettings().then(() => {
      if (page === 'settings' && section === want && JSON.stringify([settings, cameraCfgs]) !== before
        && !$('settings-body').contains(document.activeElement)) drawSettings();
    }).catch(() => {});
  } else {
    try { await loadSettings(); } catch (e) { toast(e.message, true); return; }
  }
  drawSettings();
}
function drawSettings() {
  camPreviewUnbind.splice(0).forEach(u => u());
  const body = $('settings-body');
  // The log view is kept, and lent to the Logs section while it's open.
  const lb = $('logs-box'); if (lb.parentElement !== $('page-logs')) $('page-logs').appendChild(lb);
  ({cameras: renderCameras, profiles: renderProfiles, storage: renderStorage, display: renderDisplay, ai: renderAi, hardware: renderHardware,
    cloud: renderCloud, backup: renderBackup, users: renderUsers, known: renderKnown, faces: renderFaces, notify: renderNotify, smarthome: renderSmartHome, voice: renderVoice, tasks: renderTasks, system: renderSystemSettings, info: renderInfo,
    logs: (b) => { b.innerHTML = ''; b.appendChild($('logs-box')); showLogs(); }})[section](body);
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
    toast(r.restart_required ? 'Saved. Restart GAZE to use the new address or secure (https) settings.' : (msg || 'Settings saved'));
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
    <div class="row" style="margin-bottom:8px"><h2 style="flex:1;margin:0">Cameras</h2><input type="text" id="scan-nets" placeholder="Also search e.g. 192.168.5.0/24" title="Networks this computer isn't directly on (behind a router), separated by commas. The computer's own networks and your cameras' networks are always searched." style="width:230px" value="${esc(store.get('scan-nets', ''))}"><button id="scan-cams" title="Look for cameras on every network this computer is on, your cameras' networks and any you add (ONVIF and RTSP)">🔍 Scan network</button><button class="primary" id="add-cam">+ Add camera</button></div>
    <div id="scan-out"></div>
    <table><thead><tr><th>Preview</th><th>Name</th><th class="hide-sm">Profile</th><th class="hide-sm">Recording</th><th>Status</th><th></th></tr></thead><tbody id="cam-table"></tbody></table>
    ${cameraCfgs.length ? '' : '<div class="empty">No cameras yet. Click “Add camera”.</div>'}</div>`;
  $('add-cam').onclick = () => openCameraDialog();
  $('scan-cams').onclick = () => scanNetwork();
  // The last scan stays (with "already added" brought up to date) until
  // Scan network is pressed again.
  if (lastScan) renderScan($('scan-out'), lastScan); else if (status && status.new_cameras) scanNetwork();
  const tb = $('cam-table');
  for (const cfg of cameraCfgs) {
    const c = cam(cfg.id);
    const tr = document.createElement('tr');
    tr.dataset.id = cfg.id;
    const sched = cfg.schedule_enabled ? `<div class="sub">${cfg.schedule.days.map(d => DAYS[d]).join(', ')} ${esc(cfg.schedule.start)}–${esc(cfg.schedule.end)}</div>` : '';
    tr.innerHTML = `<td><div class="surface camprev"><img alt=""><div class="msg">${cfg.enabled ? '…' : 'off'}</div></div></td>
      <td><b>${esc(cfg.name)}</b><div class="sub">${esc(c ? c.url : '')}</div></td>
      <td class="hide-sm">${cfg.profile ? `<span class="tag prof" title="Follows this profile: changing the profile changes this camera">${esc(cfg.profile)}</span>` : '<span class="muted">–</span>'}</td>
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
  // Gone from the list straight away (deleting recordings can take a while).
  document.querySelectorAll(`#cam-table tr[data-id="${CSS.escape(id)}"]`).forEach(r => r.remove());
  cameraCfgs = cameraCfgs.filter(c => c.id !== id);
  if (status) status.cameras = status.cameras.filter(c => c.id !== id);
  dashSync();
  try { await api(`api/cameras/${enc(id)}?delete_recordings=${delRecs}`, {method: 'DELETE'}); toast('Camera deleted'); }
  catch (e) { toast(e.message, true); }
  await refreshStatus(); if (page === 'settings') showSettings('cameras');
}

// ---------- network scan ----------
function guessBrand(f) {
  const t = `${f.manufacturer} ${f.hardware} ${f.name}`.toLowerCase();
  const map = [['hikvision', 'hikvision'], ['hilook', 'hikvision'], ['annke', 'hikvision'], ['dahua', 'dahua'], ['amcrest', 'dahua'], ['lorex', 'dahua'],
    ['reolink', 'reolink'], ['tp-link', 'tapo'], ['tapo', 'tapo'], ['vigi', 'tapo'], ['uniview', 'uniview'], ['unv', 'uniview'], ['axis', 'axis'],
    ['hanwha', 'hanwha'], ['wisenet', 'hanwha'], ['samsung', 'hanwha'], ['foscam', 'foscam'], ['swann', 'swann'], ['ezviz', 'ezviz']];
  const m = map.find(([k]) => t.includes(k));
  return m ? m[1] : (f.onvif_port ? 'onvif' : 'generic');
}
// Look for cameras (Settings → Cameras, or the Live page before the first
// camera); `ids`: the button, the results box and the extra-networks box.
async function scanNetwork(ids = {btn: 'scan-cams', out: 'scan-out', nets: 'scan-nets'}) {
  const out = $(ids.out), btn = $(ids.btn), label = btn.textContent;
  btn.disabled = true; btn.textContent = 'Scanning…';
  out.innerHTML = '<div class="hint" style="margin:8px 0"><span class="spinner small"></span> Looking for cameras on your network (about 5 seconds)…</div>';
  let found = [];
  const nets = ($(ids.nets) && val(ids.nets).trim()) || '';
  store.set('scan-nets', nets);
  try { found = await api('api/discover', {method: 'POST', json: {networks: nets}}); } catch (e) { btn.disabled = false; btn.textContent = label; out.innerHTML = `<div class="err">${esc(e.message)}</div>`; return; }
  btn.disabled = false; btn.textContent = label;
  if (out.id === 'scan-out') lastScan = found;
  renderScan(out, found);
}
let lastScan = null;
// Is a camera at this address already set up?
function ipAdded(ip) {
  const re = new RegExp('(^|[/@])' + ip.replace(/\./g, '\\.') + '([:/]|$)');
  return cameraCfgs.some(c => [c.url, c.live_url, c.talk_url, c.onvif && c.onvif.host].some(u => u && re.test(u)));
}
function renderScan(out, found) {
  if (!out) return;
  if (!found.length) { out.innerHTML = '<div class="hint" style="margin:8px 0">No cameras answered. Check that ONVIF is turned on in the camera. A camera on a network this computer is not on (behind another router) is found by adding that network, e.g. 192.168.5.0/24, in the box next to Scan network.</div>'; return; }
  out.innerHTML = `<div class="scan-res"><b>Found on the network</b> <span class="muted">(${found.length})</span>
    <table><tbody>${found.map((f, i) => `<tr><td><b>${esc(f.ip)}</b></td><td>${esc([f.manufacturer, f.hardware, f.name].filter(Boolean).join(' · ') || (f.rtsp ? 'Device with a video stream (RTSP)' : 'Device'))}</td>
      <td>${f.onvif_port ? '<span class="tag">ONVIF</span>' : ''}${f.rtsp ? '<span class="tag">RTSP</span>' : ''}</td>
      <td style="text-align:right">${f.added || ipAdded(f.ip) ? '<span class="muted">already added</span>' : `<button type="button" class="small primary" data-add="${i}">+ Add</button>`}</td></tr>`).join('')}</tbody></table></div>`;
  out.querySelectorAll('[data-add]').forEach(b => b.onclick = async () => {
    const f = found[+b.dataset.add];
    await openCameraDialog();
    // Fill in the quick setup: brand, address and a name; the login is up to you.
    $('c-name').value = f.name || `${(f.manufacturer || 'Camera')} ${f.ip.split('.').pop()}`;
    $('q-ip').value = f.ip;
    $('q-brand').value = guessBrand(f);
    $('q-brand').onchange(); tapoDefaults();
    if (f.onvif_port && $('p-port')) $('p-port').value = f.onvif_port;
    $('q-user').focus();
    toast('Enter the camera’s login, then Test connection');
  });
}

function tapoDefaults() {
  if (val('q-brand') !== 'tapo') return;
  $('c-transport').value = 'udp';
  if ($('p-port')) $('p-port').value = 2020;
}
// Common RTSP URL formats. {auth} = user:password@
const BRANDS = [
  ['hikvision', 'Hikvision / Annke / HiLook / HiWatch', 554, '/Streaming/Channels/{ch}01', '/Streaming/Channels/{ch}02'],
  ['hikold', 'Hikvision (older firmware)', 554, '/h264/ch{ch}/main/av_stream', '/h264/ch{ch}/sub/av_stream'],
  ['dahua', 'Dahua', 554, '/cam/realmonitor?channel={ch}&subtype=0', '/cam/realmonitor?channel={ch}&subtype=1'],
  ['amcrest', 'Amcrest', 554, '/cam/realmonitor?channel={ch}&subtype=0', '/cam/realmonitor?channel={ch}&subtype=1'],
  ['lorex', 'Lorex (Dahua-based)', 554, '/cam/realmonitor?channel={ch}&subtype=0', '/cam/realmonitor?channel={ch}&subtype=1'],
  ['imou', 'Imou', 554, '/cam/realmonitor?channel={ch}&subtype=0', '/cam/realmonitor?channel={ch}&subtype=1'],
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
  ['milesight', 'Milesight', 554, '/main', '/sub'],
  ['bosch', 'Bosch', 554, '/video?inst=1', '/video?inst=2'],
  ['vivotek', 'Vivotek', 554, '/live.sdp', '/live2.sdp'],
  ['panasonic', 'Panasonic / i-PRO', 554, '/MediaInput/h264/stream_1', '/MediaInput/h264/stream_2'],
  ['instar', 'INSTAR', 554, '/11', '/12'],
  ['dlink', 'D-Link', 554, '/live1.sdp', '/live2.sdp'],
  ['onvif', 'Generic ONVIF (Vstarcam, Sricam …)', 554, '/onvif1', '/onvif2'],
  ['generic', 'Other (stream1 / stream2)', 554, '/stream1', '/stream2'],
];
function brandUrl(brand, which) {
  const b = BRANDS.find(x => x[0] === brand); if (!b) return '';
  const path = which === 'main' ? b[3] : b[4]; if (!path) return '';
  // The brand's usual port and channel 1 (change them in the stream addresses).
  const ip = val('q-ip').trim() || '192.168.1.100', port = b[2], ch = '1';
  const user = val('q-user').trim(), pass = $('q-pass').value;
  const auth = user ? `${enc(user)}${pass ? ':' + enc(pass) : ''}@` : '';
  return `rtsp://${auth}${ip}:${port}${path.replaceAll('{ch}', ch)}`;
}
// Put the quick-setup IP and login into an existing stream URL,
// keeping its path (e.g. /Streaming/Channels/102).
function rewriteUrl(u) {
  const m = u.trim().match(/^(rtsps?:\/\/)(?:([^@\/]*)@)?([^:\/?#]+)(?::(\d+))?(.*)$/i);
  if (!m) return u;
  const ip = val('q-ip').trim() || m[3], port = m[4];
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

// Changes each time a camera's settings window is opened.
let camDlgGen = 0;
// Editing a profile in the camera window ({idx, name}; idx -1 = new), or null.
let profileEdit = null;
async function openCameraDialog(id, clone, prof) {
  camDlgGen++;
  profileEdit = prof || null;
  if (prof) { id = null; clone = false; }
  if (!settings) await loadSettings().catch(() => {});
  if (!sysinfo) sysinfo = await api('api/system').catch(() => null);
  editingId = clone ? null : (id || null); cloneMode = !!clone;
  const base = id ? cameraCfgs.find(c => c.id === id) : null;
  const c = base ? JSON.parse(JSON.stringify(base)) : {
    name: '', enabled: true, url: '', live_url: '', rtsp_transport: 'tcp', record_mode: 'continuous', schedule_enabled: false,
    schedule: {days: [0,1,2,3,4,5,6], start: '18:00', end: '07:00'}, record_audio: true, live_sound: true, talk: false, talk_chime: true, talk_url: '', transcode: false, record_fps: 0,
    segment_seconds: 0, storage_dir: '', retention_days: 0, max_storage_gb: 0,
    osd: {custom: false, show_name: true, name_corner: 'bl', show_clock: true, clock_corner: 'tl', clock_format: '', show_status: true, status_corner: 'tr',
      show_detections: true, detections_corner: 'bl', show_fps: false, fps_corner: 'tl', text: '', text_corner: 'br'},
    view: {grid_quality: 'auto', grid_fps: 0, hd_fps: 0},
    ptz: {enabled: false, host: '', port: 80, username: '', password: '', speed: 0.5, invert_pan: false, invert_tilt: false},
    detect: {motion: true, min_contrast: 24, min_object_px: 300, make_time_ms: 500, min_travel_px: 0, mask: '', ai: false, objects: ['person','vehicle'], min_confidence: 0.7, trigger: 'objects', pre_seconds: 3, post_seconds: 15, join_seconds: 30, fps: 5, ai_interval: 1, ignore_parked: true, ignore_animals: true, ai_ignores_mask: true, parked_seconds: 60}};
  if (clone) c.name = c.name + ' (copy)';
  // A profile: its settings on top of a new camera's.
  if (prof) { Object.assign(c, JSON.parse(JSON.stringify(prof.values || {}))); c.detect = {...c.detect, mask: ''}; c.name = prof.name; }
  const q = parseUrl(c.url) || {};
  // A new camera: the login of the camera added last (most cameras in one
  // home share it) — change it if this one's is different.
  let sameLogin = null;
  if (!id && !clone && !prof && !q.user) {
    const last = [...cameraCfgs].reverse().map(x => ({c: x, u: parseUrl(x.url)})).find(x => x.u && x.u.user);
    if (last) { q.user = last.u.user; q.pass = last.u.pass; sameLogin = last.c.name; }
  }
  // The usual objects, plus any this camera already watches for.
  const objs = [...new Set([...((sysinfo && sysinfo.ai_objects) || ['person','vehicle']), ...canonKinds(c.detect.objects)])];
  const more = ((sysinfo && sysinfo.ai_more) || []).filter(o => !objs.includes(o));
  const aiOn = settings && settings.ai.provider !== 'none';
  const brandOpts = '<option value="">Choose brand…</option>' + BRANDS.map(b => `<option value="${b[0]}">${esc(b[1])}</option>`).join('');
  const segOpts = [[0, 'Default (' + fmtDur(settings ? settings.storage.segment_seconds : 300) + ')'], [300, '5 minutes'], [900, '15 minutes'], [1800, '30 minutes'], [3600, '1 hour'], [7200, '2 hours'], [21600, '6 hours'], [86400, 'One long file per day']];
  $('f-cam').innerHTML = `
    <div class="head"><h2>${clone ? 'Clone camera' : id ? 'Edit camera' : 'Add camera'}</h2>
      ${id && !clone && cameraCfgs.length > 1 ? (() => { const i = cameraCfgs.findIndex(x => x.id === id); return `<span class="cam-nav"><button type="button" class="small" data-nav="-1" title="Previous camera">◀</button><span class="muted small">${i + 1} of ${cameraCfgs.length}</span><button type="button" class="small" data-nav="1" title="Next camera">▶</button></span>`; })() : ''}
      <button type="button" class="small" data-close>✕</button></div>
    <div class="body">
      <div class="profile-bar"><b>Profile</b>
        <select id="c-prof"><option value="">Choose a saved profile…</option></select>
        <button type="button" class="small" id="c-prof-apply" hidden>Apply</button>
        <a class="btn small" href="#settings/profiles" id="c-prof-manage" title="Create and edit profiles">Manage profiles →</a>
        <span class="hint" id="c-prof-msg">Profiles hold recording, motion, AI and sound settings.</span></div>
      <div class="tabs" id="c-tabs"><button type="button" data-t="general" class="active">General</button><button type="button" data-t="recording">Recording</button><button type="button" data-t="detection">Motion</button><button type="button" data-t="ai">AI</button><button type="button" data-t="audio">Audio</button><button type="button" data-t="osd">On-screen display</button><button type="button" data-t="ptz">Pan / tilt / zoom</button><button type="button" data-t="device">ONVIF</button><button type="button" data-t="zones">Zones</button></div>
      <div data-tab="general" class="form">
        <label for="c-name">Name</label><div><input type="text" id="c-name" required value="${esc(c.name)}" placeholder="Front door"></div>
        <span></span><label class="check"><input type="checkbox" id="c-enabled" ${c.enabled ? 'checked' : ''}> Camera enabled</label>
        <span class="c-restart" hidden></span><div class="c-restart row" hidden><button type="button" id="c-restart">⟳ Restart camera</button><span class="hint" id="c-restart-info"></span></div>
        <h3>Quick setup</h3>
        <label for="q-brand">Camera brand</label>
        <div><div class="row"><select id="q-brand">${brandOpts}</select></div>
          <div class="hint">Fill in the camera's address and login, pick the brand, and the stream URLs are filled in for you.</div></div>
        <label for="q-ip">IP address</label><div class="row"><input type="text" id="q-ip" value="${esc(q.ip || '')}" placeholder="192.168.1.100" style="flex:2"><button type="button" id="q-web" title="Open the camera's own web page (its settings) in a new tab">🌐 Camera's page</button></div>
        <label for="q-user">Camera login</label><div><div class="row"><input type="text" id="q-user" value="${esc(q.user || '')}" placeholder="username" style="flex:1" autocomplete="off"><input type="password" id="q-pass" value="${esc(q.pass || '')}" placeholder="password" style="flex:1" autocomplete="new-password"></div>
          ${sameLogin ? `<div class="hint" id="q-same">The same login as ${esc(sameLogin)} — change it if this camera's is different.</div>` : ''}</div>
        <h3>Streams</h3>
        <label for="c-url">Main stream (HD)${tip('The camera’s full-quality video address, usually starting rtsp://. Example: rtsp://admin:password@192.168.1.50:554/stream1 — the brand list fills it in for you.')}</label>
        <div><input type="text" id="c-url" required value="${esc(c.url)}" placeholder="rtsp://user:password@192.168.1.50:554/stream1"></div>
        <label for="c-live">Sub-stream (low resolution)${tip('A second, smaller video most cameras also send (often …/stream2 or subtype=1). GAZE uses it for the camera grid, motion detection and AI, so the computer works much less. Optional, but recommended.')}</label>
        <div><input type="text" id="c-live" value="${esc(c.live_url)}" placeholder="Optional – e.g. …/stream2">
          <div class="hint">Used for the live grid, motion detection and AI. Saves a lot of CPU.</div></div>
        <label for="c-rotate">Rotate picture</label>
        <div><select id="c-rotate">${[[0, 'No rotation'], [90, '90° clockwise'], [180, '180° (upside down)'], [270, '90° anticlockwise']].map(([v, t]) => `<option value="${v}" ${(c.rotate || 0) === v ? 'selected' : ''}>${t}</option>`).join('')}</select>
          <div class="hint">For cameras mounted sideways or upside down: live view, alerts and new recordings are turned the right way.</div></div>
        <label>Privacy areas${tip('Black out parts of the picture for good \u2014 a neighbour\u2019s window or garden, a public footpath. They\u2019re black in the live view, new recordings, snapshots and alert pictures, and nothing is ever detected there. This camera\u2019s recordings are then converted while recording (more work for the computer), and its live view uses pictures instead of smooth video. Recordings made before stay as they are.')}</label>
        <div><div class="surface priv-ed" id="c-priv" style="aspect-ratio:16/9;max-width:560px;cursor:crosshair;touch-action:none"><img alt="" style="object-fit:fill"><div class="msg">${id && !clone ? '' : 'Save the camera first to see its picture here.'}</div></div>
          <div class="row" style="margin-top:6px"><button type="button" class="small" id="c-priv-clear">Clear all</button><span class="hint" style="margin:0">Drag on the picture to black out an area; ✕ removes one.</span></div></div>
        <label>Fisheye (360°) camera${tip('For cameras with a round, fisheye picture: the live view and snapshots become a normal flat picture of one direction \u2014 choose where to look and how wide. Recordings keep the whole circle, so nothing outside the view is lost. Example: point the view at the front gate.')}</label>
        <div><label class="check"><input type="checkbox" id="c-fe" ${c.dewarp && c.dewarp.enabled ? 'checked' : ''}> Show a flat picture instead of the circle</label>
          <div class="fe-sl" style="margin-top:6px">${[['yaw', 'Look left / right', -180, 180, 0], ['pitch', 'Look up / down', -90, 90, 0], ['width', 'Width of view (smaller = zoomed in)', 30, 160, 100], ['lens', 'Lens angle', 120, 360, 180]].map(([k, t, a, b, d]) => `<label class="muted small" style="display:flex;gap:8px;align-items:center;margin:2px 0"><span style="width:240px">${t}</span><input type="range" id="c-fe-${k}" min="${a}" max="${b}" value="${(c.dewarp && c.dewarp[k] != null) ? c.dewarp[k] : d}" style="flex:1"><span id="c-fe-${k}-v" style="width:44px">${(c.dewarp && c.dewarp[k] != null) ? c.dewarp[k] : d}°</span></label>`).join('')}</div>
          <div class="hint">Save, then look at the camera's live view to check the direction.</div></div>
        <label for="c-rtmp">Stream live${tip('Broadcast this camera live on YouTube, Facebook, Twitch or any RTMP server \u2014 a bird feeder, the view, the weather. In YouTube Studio: Create \u2192 Go live \u2192 Stream; copy the stream URL and key and put them together here, e.g. rtmp://a.rtmp.youtube.com/live2/abcd-1234-efgh. The camera\u2019s own video is passed through, so it costs almost nothing (H.265 cameras are converted).')}</label>
        <div><label class="check"><input type="checkbox" id="c-rtmp-on" ${c.stream_live ? 'checked' : ''}> Stream this camera live</label>
          <input type="password" id="c-rtmp" value="${esc(c.stream_url || '')}" placeholder="rtmp://a.rtmp.youtube.com/live2/your-stream-key" autocomplete="off" style="margin-top:6px">
          <div class="hint">${(() => { const st = id && cam(id) && cam(id).streaming; return !st ? 'The address includes your stream key: keep it private.' : st.live ? `<span class="okc">● Live since ${fmtTime(st.since)}</span>` : `<span class="errc">● ${esc(st.error || 'Not streaming')}</span>`; })()}</div></div>
        <label for="c-transport">Connection type${tip('How the video travels from the camera. TCP is reliable and right for most cameras. Tapo cameras are much smoother with UDP (chosen for you when you pick the Tapo brand). Try the other one if a camera stutters or won’t connect.')}</label>
        <div><select id="c-transport"><option value="tcp">TCP — reliable (recommended)</option><option value="udp">UDP</option></select></div>
        <span></span><div><button type="button" id="c-test">▶ Test connection</button><div id="c-probe"></div></div>
        <h3>Live view</h3>
        ${(() => { const v = c.view || {}, l = status.live, fpsOpts = (cur, def) => [0, 1, 2, 5, 10, 15, 20, 25, 30].map(f => `<option value="${f}" ${f === (cur || 0) ? 'selected' : ''}>${f ? f + ' fps' : 'Default (' + def + ')'}</option>`).join('');
          return `<label for="c-vq">Picture in the grid${tip('Which stream this camera shows in the Live grid. The sub-stream is light on the computer and the network; full quality is sharper but much heavier with several cameras.')}</label><div><select id="c-vq"><option value="auto" ${v.grid_quality !== 'main' ? 'selected' : ''}>Sub-stream (light)</option><option value="main" ${v.grid_quality === 'main' ? 'selected' : ''}>Full quality</option></select></div>
        <label for="c-vg">Smoothness in the grid${tip('Pictures per second (fps) in the Live grid. Higher is smoother but uses more of the computer. Default follows Settings → Live view & display.')}</label><div><select id="c-vg">${fpsOpts(v.grid_fps, l.fps)}</select></div>
        <label for="c-vh">Smoothness when opened${tip('Pictures per second (fps) when you open this camera or watch it full screen.')}</label><div><select id="c-vh">${fpsOpts(v.hd_fps, l.hd_fps)}</select>
          <div class="hint">“Default” follows Settings → Live view &amp; display.</div></div>`; })()}
      </div>

      <div data-tab="recording" class="form" hidden>
        <label>Recording mode</label>
        <div class="radio-cards" style="grid-template-columns:1fr">
          ${[['continuous', 'HD 24/7', 'Records the main stream around the clock.'],
             ['continuous_plus_events', 'HD 24/7 plus HD alert clips', 'Full quality around the clock, and each alert also saved as its own full-quality clip — kept for as long as Storage → “Keep alerts for” says, even after the 24/7 footage around it is deleted to make room. Uses a little more space.'],
             ['continuous_sub', 'Sub-stream 24/7', 'Records the low-resolution stream around the clock. Uses much less space.'],
             ['sub_plus_events', 'Sub-stream 24/7 plus HD alerts', 'Low-res around the clock, plus full-quality clips around motion/AI alerts.'],
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
        <span></span><div><label class="check"><input type="checkbox" id="c-tlcopy" ${c.timeline_copy !== false ? 'checked' : ''}> Also record the sub-stream, for a quick timeline</label>
          <div class="hint">With “HD 24/7”: the sub-stream is recorded too, so the timeline opens and scrubs instantly (alerts and downloads still use HD). Uses roughly 5–15% extra space.</div></div>
        <label for="c-fps">Recording frame rate${tip('Pictures per second saved in recordings. Fewer saves space (10 fps uses about a third of 30 fps) but the computer has to re-encode the video. “Camera’s own” keeps it exactly as sent.')}</label>
        <div><select id="c-fps">${[0, 1, 2, 5, 10, 12, 15, 20, 25, 30].map(f => `<option value="${f}" ${c.record_fps === f ? 'selected' : ''}>${f ? f + ' fps' : "Camera's own (no re-encoding)"}</option>`).join('')}</select>
          <div class="hint">A lower frame rate saves space but re-encodes the video (uses the GPU encoder if set up).</div></div>
        <span></span><div><label class="check"><input type="checkbox" id="c-transcode" ${c.transcode ? 'checked' : ''}> Convert recordings to H.264${tip('H.265 (HEVC) is a newer video format some cameras use; not every browser or phone can play it. Converting to H.264 makes recordings play everywhere, but uses more of the computer while recording.')}</label>
          <div class="hint">For H.265 cameras, so recordings play in every browser.</div></div>
        <label for="c-seg">File length${tip('Recordings are saved as a series of files of this length. Shorter files are quicker to open and download; it doesn’t change how much is kept. 5 minutes suits most cameras.')}</label>
        <div><select id="c-seg">${segOpts.map(([v, t]) => `<option value="${v}" ${c.segment_seconds === v ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
        <h3>Storage</h3>
        <label for="c-dir">Storage folder</label>
        <div><div class="row"><input type="text" id="c-dir" value="${esc(c.storage_dir)}" placeholder="Default: ${esc(settings ? settings.storage.dir : 'recordings')}" style="flex:1"><button type="button" id="c-browse">Browse…</button></div>
          ${id && !clone ? `<label class="check" id="c-move-row" hidden><input type="checkbox" id="c-move" checked> Move existing recordings to the new folder</label>` : ''}
          <div class="hint">Any folder this computer can write to: another drive (<code>D:\\NVR</code>), a NAS (<code>\\\\NAS\\cameras</code> or <code>/mnt/nas</code>), or a synced cloud folder (<code>G:\\My Drive\\NVR</code> with Google Drive for desktop). A sub-folder named after the camera is created.</div></div>
        <label for="c-ret">Keep recordings for</label>
        <div class="row"><input type="number" id="c-ret" min="0" value="${c.retention_days}" style="width:110px"><span class="muted">days (0 = use the default, ${settings ? settings.storage.retention_days : 14})</span></div>
        <label for="c-max">Space for this camera${tip('The most disk space this camera’s recordings may use. When it’s full, the oldest recordings are deleted. Example: 200 GB keeps roughly 2 weeks of a 4 MP camera recording 24/7.')}</label>
        <div class="row"><input type="number" id="c-max" min="0" step="0.5" value="${c.max_storage_gb}" style="width:110px"><span class="muted">GB (0 = no limit; oldest recordings are deleted above it)</span></div>
      </div>

      <div data-tab="detection" class="form" hidden>
        <span></span><label class="check"><input type="checkbox" id="d-motion" ${c.detect.motion ? 'checked' : ''}> Motion detection</label>
        <label for="d-contrast">Sensitivity</label>
        <div><div class="row"><input type="range" id="d-contrast" min="4" max="100" value="${c.detect.min_contrast}" style="flex:1"><span id="d-contrast-v" class="muted" style="width:40px">${c.detect.min_contrast}</span></div>
          <div class="hint">How big a change in brightness counts as movement. Lower = more sensitive (night and infrared pictures may need lower); higher ignores noise, shadows and small light changes.</div></div>
        <label for="d-objpx">Smallest movement${tip('Moving things smaller than this are ignored. Example: rain and insects are under 50 px; a person far away about 200 px; a person close up 2,000 px or more. Watch the live readout below while someone walks past to choose.')}</label>
        <div><div class="row"><input type="number" id="d-objpx" step="any" min="0" max="230400" value="${c.detect.min_object_px}" style="width:120px"><span class="muted">pixels (of a 640×360 picture)</span></div>
          <div class="hint">Moving areas smaller than this are ignored (rain, insects, noise). For scale: a person filling a quarter of the picture height ≈ 2,000 px; far away ≈ 200 px. Use the live readout below to choose.</div></div>
        <label for="d-make">Must move for${tip('Movement has to last this long before it counts. Example: 0.5 s ignores a flash of headlights or a bird flying past.')}</label>
        <div class="row"><input type="number" id="d-make" min="0" max="30" step="0.1" value="${c.detect.make_time_ms / 1000}" style="width:120px"><span class="muted">seconds of motion before it triggers</span></div>
        <label for="d-travel">Must move at least${tip('Something has to move this far across the picture to count. Ignores trees, bushes and flags that sway in place. 0 = off.')}</label>
        <div class="row"><input type="number" id="d-travel" min="0" max="640" step="10" value="${c.detect.min_travel_px}" style="width:120px"><span class="muted">pixels (0 = off). Ignores trees and flags moving in place. <span style="color:#22c55e">Green ring</span> on the picture below: how far something must move.</span></div>
        <label>Ignore areas</label>
        <div><div class="surface" id="d-mask" style="aspect-ratio:16/9;max-width:560px;touch-action:none;cursor:crosshair">
            <img alt="" id="d-mask-img" style="object-fit:fill"><div class="msg" id="d-mask-msg">${id && !clone ? '' : 'Save the camera first to see its picture here.'}</div>
            <canvas id="d-mask-grid" width="160" height="90" style="position:absolute;inset:0;width:100%;height:100%;image-rendering:pixelated"></canvas>
            <svg id="d-mask-boxes" style="position:absolute;inset:0;width:100%;height:100%;pointer-events:none" viewBox="0 0 1 1" preserveAspectRatio="none"></svg></div>
          <div class="row mask-tools" style="margin-top:6px">
            <div class="seg" id="d-mask-tool"><button type="button" class="small on" data-v="paint">🖌 Paint</button><button type="button" class="small" data-v="erase">⌫ Erase</button></div>
            <label class="muted small" style="margin:0">Brush <select id="d-mask-brush" class="small"><option value="1">Fine</option><option value="3" selected>Small</option><option value="7">Medium</option><option value="15">Large</option></select></label>
            <button type="button" class="small" id="d-mask-clear">Clear</button><button type="button" class="small" id="d-mask-invert">Invert</button></div>
          <div class="hint" style="margin:4px 0 0">Paint over the picture to mark areas to ignore (red); switch to Erase to unmark. Right-click (or hold Shift) erases too.</div>
          <label class="check" style="margin-top:6px"><input type="checkbox" id="d-mask-ai" ${c.detect.ai_ignores_mask !== false ? 'checked' : ''}> AI ignores these areas too — something it finds mostly inside them (say a parked car) doesn't alert, even when something else set off the check. Untick to still get AI alerts there (motion there is ignored either way).</label>
          <div id="d-meter" class="hint" style="margin-top:8px"></div></div>
        <label for="d-pre">Record before alert</label>
        <div class="row"><input type="number" id="d-pre" min="0" max="120" value="${c.detect.pre_seconds}" style="width:110px"><span class="muted">seconds (pre-event buffer, in alert recording modes)</span></div>
        <label for="d-post">Record after activity stops</label>
        <div class="row"><input type="number" id="d-post" min="3" max="600" value="${c.detect.post_seconds}" style="width:110px"><span class="muted">seconds</span></div>
        <label for="d-join">Join alerts within</label>
        <div class="row"><input type="number" id="d-join" min="0" max="600" value="${c.detect.join_seconds ?? 30}" style="width:110px"><span class="muted">seconds — if it triggers again within this time, it's added to the same alert: one longer video instead of lots of short ones. The timer starts again with each trigger (an alert is at most 10 minutes).</span></div>
        <label for="d-fps">Analysis frame rate</label>
        <div class="row"><input type="number" id="d-fps" min="1" max="15" value="${c.detect.fps}" style="width:110px"><span class="muted">pictures per second checked for motion</span></div>
      </div>
      <div data-tab="audio" class="form" hidden>
        <label>Recording</label>
        <div><label class="check"><input type="checkbox" id="c-audio" ${c.record_audio ? 'checked' : ''}> Record audio</label>
          <div class="hint">The camera's microphone is saved with the video (if it has one).</div></div>
        <label>Live sound</label>
        <div><label class="check"><input type="checkbox" id="c-sound" ${c.live_sound !== false ? 'checked' : ''}> Speaker button on the live picture</label>
          <div class="hint">Click 🔇 on the camera's picture (grid, camera view or full screen) to hear it; hover it for the volume. One camera plays at a time.</div></div>
        <label class="cam-only">Two-way audio</label>
        <div class="cam-only"><label class="check"><input type="checkbox" id="c-talk" ${c.talk ? 'checked' : ''}> Talk through this camera's speaker${tip('Adds a 🎙️ Push to talk button to the camera’s live view: hold it and speak into this phone’s or computer’s microphone, and whoever is there hears you. Works with cameras and doorbells that have a speaker and support two-way audio over ONVIF (most Dahua, Hikvision, Amcrest and Axis).')}</label>
          <div class="hint">Shows a <b>🎙️ Push to talk</b> button on the camera's live view.</div>
          <label class="check" style="margin-top:6px"><input type="checkbox" id="c-chime" ${c.talk_chime !== false ? 'checked' : ''}> Ring a doorbell chime first, to get the attention of whoever is there (and a short "goodbye" tune when you stop)</label>
          <input type="text" id="c-talk-url" value="${esc(c.talk_url || '')}" placeholder="Speaker address (optional) – empty uses the main stream address" style="margin-top:6px">
          <div class="tapo-talk" id="c-tapo-row" hidden><div class="row" style="margin-top:6px"><span class="muted">Tapo camera:</span><input type="password" id="c-tapo-pw" placeholder="TP-Link account password" autocomplete="new-password" style="flex:1"><button type="button" id="c-tapo-use">Use for talking</button></div>
            <div class="hint">Tapo cameras talk through their own connection, not the camera account made for the video. Enter the password of the TP-Link / Tapo app account the camera is in. GAZE then uses <code>tapo://admin:…@camera</code> as the speaker address (the password stays hidden like the others).</div></div>
          <div class="row" style="margin-top:6px"><button type="button" id="c-talk-test">🔊 Test (plays a beep on the camera)</button><span class="hint" id="c-talk-res" style="margin:0"></span></div></div>
        <label>Sound detection</label>
        <div><label class="check"><input type="checkbox" id="c-snd-on" ${c.sound && c.sound.enabled ? 'checked' : ''}> Listen for sounds and alert on them${tip('GAZE listens to the camera’s microphone with a small AI model (YAMNet, by Google; downloads once, 16 MB) that knows hundreds of everyday sounds. What you tick below makes an alert with a notification, even when nothing moves in the picture — e.g. glass breaking round the back, or the smoke alarm while you’re out. Runs on this computer; the sound never leaves it.')}</label>
          <div id="c-snd-box" ${c.sound && c.sound.enabled ? '' : 'hidden'}>
            <div class="checks" id="c-snd-kinds" style="margin-top:6px" data-kinds="${esc(JSON.stringify((c.sound && c.sound.kinds) || ['glass', 'alarm', 'scream', 'gunshot']))}"></div>
            <label for="c-snd-sens" style="margin-top:8px;display:block">Sensitivity: <b id="c-snd-sens-v">${(c.sound && c.sound.sensitivity) || 6}</b></label>
            <input type="range" id="c-snd-sens" min="1" max="10" value="${(c.sound && c.sound.sensitivity) || 6}" style="width:240px">
            <div class="hint">Lower: only clear, loud sounds. Higher: fainter ones too (more false alerts).</div>
            <div class="hint" id="c-snd-now"></div></div></div>
      </div>
      <div data-tab="ai" class="form" hidden>
        <span></span><div><label class="check"><input type="checkbox" id="d-ai" ${c.detect.ai ? 'checked' : ''}> Object recognition (AI)</label>
          <div class="hint">${aiOn ? `Uses the AI on this computer, only while motion is seen (so motion detection stays on).` : '<span class="warnc">No AI is set up yet.</span> Choose one in Settings → Object recognition.'}</div></div>
        <label>Alert on</label>
        <div><div class="chips" id="d-objs">${objs.map(o => `<label><input type="checkbox" value="${o}" ${c.detect.objects.includes(o) ? 'checked' : ''}>${o}</label>`).join('')}</div>
          ${more.length ? `<details class="more-objs"><summary>More things to watch for…</summary><div class="chips" id="d-more">${more.map(o => `<label><input type="checkbox" value="${o}">${o}</label>`).join('')}</div>
            <div class="hint">Tick one to add it to the list above.</div></details>` : ''}</div>
        <label for="d-conf">How sure the AI must be${tip('The AI gives each thing it finds a score: how sure it is. Lower finds more (and more mistakes); higher is stricter. Example: at 50% a person it’s 70% sure about alerts; a shadow it’s 30% sure about doesn’t.')}</label>
        <div class="row"><input type="range" id="d-conf" min="5" max="95" value="${Math.round(c.detect.min_confidence * 100)}" style="flex:1"><span id="d-conf-v" class="muted" style="width:40px">${Math.round(c.detect.min_confidence * 100)}%</span></div>
        <label for="d-trig">What makes an alert</label>
        <div><select id="d-trig"><option value="motion">Any motion</option><option value="objects">Only recognized objects (ignores trees, shadows, rain)</option></select>${tip('Any motion: every movement can become an alert, and the AI adds what it saw. Only recognized objects: an alert only when the AI finds something ticked above — far fewer alerts from wind, rain and headlights.')}
          <label class="check" style="margin-top:6px"><input type="checkbox" id="d-animals" ${c.detect.ignore_animals !== false ? 'checked' : ''}> Check motion with AI first — no alert when it's a cat (or another animal you don't watch for)${tip('With “Any motion”, the AI looks at each movement before it becomes an alert. If it’s an animal that isn’t ticked above (and nothing you do watch for), it isn’t kept. Tick “cat” above to get alerts for cats.')}</label>
          <label for="d-hdcheck" style="margin-top:10px">Double-check in full quality (HD)${tip('The AI normally watches the light sub-stream. With this on, when it finds something, GAZE takes one full-quality picture and the AI looks again at that spot: confirmed (HD AI ✓), corrected (a cat, not a dog), or not confirmed (HD AI ?). “Leave out” drops unconfirmed finds and sends no notification for them. Needs a sub-stream.')}</label>
          <select id="d-hdcheck">
            <option value="mark" ${(c.detect.hd_check || 'mark') === 'mark' ? 'selected' : ''}>On — mark what HD doesn't confirm</option>
            <option value="drop" ${c.detect.hd_check === 'drop' ? 'selected' : ''}>On — leave out what HD doesn't confirm</option>
            <option value="off" ${c.detect.hd_check === 'off' ? 'selected' : ''}>Off</option>
          </select>
</div>
        <label for="d-int">AI looks every${tip('How often the AI checks the picture while something is moving. Lower recognises things quicker but works the computer harder. Example: 1 second suits most cameras; 0.5 for a busy street.')}</label>
        <div class="row"><input type="number" id="d-int" min="0.2" max="60" step="0.1" value="${c.detect.ai_interval}" style="width:110px"><span class="muted">seconds while there's motion</span></div>
        <label>Stationary objects</label>
        <div><label class="check"><input type="checkbox" id="d-parked" ${c.detect.ignore_parked !== false ? 'checked' : ''}> Stop alerting for objects that stay in one place${tip('A car parked in the driveway alerts when it arrives, then stays quiet while it stands still (people walking past still alert). It alerts again as soon as it moves or drives off.')}</label>
          <div class="row" style="margin-top:4px"><span class="muted">after</span><input type="number" id="d-parked-s" min="10" max="3600" value="${c.detect.parked_seconds || 60}" style="width:90px"><span class="muted">seconds without moving, for:</span></div>
          ${(() => { const parked = canonKinds(c.detect.parked_objects || ['vehicle', 'bicycle', 'boat']);
            const chip = (o) => `<label><input type="checkbox" value="${esc(o)}" ${parked.includes(o) ? 'checked' : ''}>${esc(o)}</label>`;
            const rest = [...new Set([...parked, 'bicycle', 'motorcycle', 'boat', ...more])].filter(o => !objs.includes(o));
            return `<div class="chips" id="d-parked-objs" style="margin-top:4px">${objs.map(chip).join('')}</div>
              <details class="more-objs"><summary>More things to watch for…</summary><div class="chips" id="d-parked-more">${rest.map(chip).join('')}</div></details>`; })()}
</div>
        <label>Package delivery</label>
        <div><label class="check"><input type="checkbox" id="d-pkg" ${c.detect.packages ? 'checked' : ''}> Notice parcels delivered or picked up</label>
          <div class="surface" id="d-pkg-area" style="aspect-ratio:16/9;max-width:560px;touch-action:none;cursor:crosshair;margin-top:6px">
            <img alt="" id="d-pkg-img" style="object-fit:fill"><div class="msg">${id && !clone ? '' : 'Save the camera first to see its picture here.'}</div>
            <canvas id="d-pkg-grid" width="160" height="90" style="position:absolute;inset:0;width:100%;height:100%;image-rendering:pixelated"></canvas></div>
          <div class="row mask-tools" style="margin-top:6px">
            <div class="seg" id="d-pkg-tool"><button type="button" class="small on" data-v="paint">🖌 Paint</button><button type="button" class="small" data-v="erase">⌫ Erase</button></div>
            <label class="muted small" style="margin:0">Brush <select id="d-pkg-brush" class="small"><option value="1">Fine</option><option value="3">Small</option><option value="7" selected>Medium</option><option value="15">Large</option></select></label>
            <button type="button" class="small" id="d-pkg-clear">Clear</button></div>
          <div class="hint" style="margin:4px 0 0" id="d-pkg-hint">Paint over where parcels are left (doorstep, porch) — just that spot. After each visit GAZE compares it with how it looked before, and notes a parcel left or taken, with before and after pictures in the alert. It must stay a minute to count.</div></div>
        <label>Camera's own detection</label>
        <div><label class="check"><input type="checkbox" id="c-onvif-ev" ${c.onvif_events ? 'checked' : ''}> Use what the camera itself detects${tip('Many cameras detect people, vehicles, line crossings or a doorbell press themselves (Tapo, Hikvision, Dahua, Reolink, Amcrest\u2026). With this on, those become alerts in GAZE too \u2014 a doorbell press always notifies. Uses ONVIF events, with the Control address, port and login on the Pan / tilt / zoom tab (the same as the video, unless you changed them).')}</label>
          <label class="check" style="margin-top:4px"><input type="checkbox" id="c-onvif-mo" ${c.onvif_motion ? 'checked' : ''}> Also the camera's plain motion detection${tip('Usually not needed: GAZE does its own motion detection. Turn on for a camera whose own motion detection works better (e.g. a battery or PIR camera).')}</label>
          <div class="hint" id="c-onvif-st">${(() => { const st = id && cam(id) && cam(id).camera_events; return !st ? 'Person, vehicle, line crossing, doorbell press and tampering become alerts.' : st.connected ? `<span class="okc">● Listening</span>${st.last ? ` — last: ${esc(st.last.split('/').pop())}, ${fmtAgo(st.last_at)}` : ''}` : `<span class="errc">● ${esc(st.error || 'Not connected')}</span>`; })()}</div></div>
      </div>
      <div data-tab="osd" class="form" hidden>
        <span></span><div><label class="check"><input type="checkbox" id="o-custom" ${c.osd.custom ? 'checked' : ''}> Use settings just for this camera</label>
          <div class="hint">Off: this camera uses Settings → Live view &amp; display.</div></div>
        <label>Preview</label>
        <div class="surface" id="o-prev" style="aspect-ratio:16/9;max-width:560px"><img alt="" style="object-fit:fill"><div class="msg">${id && !clone ? '' : 'Save the camera to see its picture.'}</div></div>
        ${osdItemsHtml('o', c.osd)}
        <label for="o-fmt2">Date &amp; time format</label>
        <div><select id="o-fmt2"><option value="">Same as everywhere (${esc(status.display.clock_format)})</option>${CLOCK_FORMATS.map(f => `<option ${f === c.osd.clock_format ? 'selected' : ''}>${f}</option>`).join('')}<option value="custom">Custom…</option></select>
          <input type="text" id="o-fmt2-c" value="${esc(c.osd.clock_format)}" hidden style="margin-top:6px" placeholder="e.g. DD/MM/YYYY HH:mm:ss"></div>
      </div>

      <div data-tab="ptz" class="form" hidden>
        <span></span><div><label class="check"><input type="checkbox" id="p-on" ${c.ptz.enabled ? 'checked' : ''}> This camera can pan, tilt or zoom</label>
          <div class="hint">Turns on the joystick on the camera's picture.</div>${tip('Moves the camera using ONVIF, a standard most moving cameras support. You may need to turn ONVIF on in the camera’s own app or web page; some brands (e.g. Tapo, Reolink) need a separate ONVIF user made there.')}</div>
        <label for="p-host">Camera address</label><div><input type="text" id="p-host" value="${esc(c.ptz.host)}" placeholder="Same as the stream (${esc((parseUrl(c.url) || {}).ip || 'from the URL')})"></div>
        <label for="p-port">Control port${tip('The network port the camera listens on for control (ONVIF). Usually 80; Reolink uses 8000, Tapo 2020. Check the camera’s manual or app if it doesn’t connect.')}</label><div class="row"><input type="number" id="p-port" min="1" max="65535" value="${c.ptz.port}" style="width:110px">
          <span class="muted">80 for most cameras, 8000 Reolink, 2020 Tapo</span></div>
        <label for="p-user">Control login${tip('Usually the same user name and password as the video. Leave empty to use those. Tapo cameras: the camera account made in the Tapo app (Advanced settings → Camera account).')}</label><div class="row"><input type="text" id="p-user" value="${esc(c.ptz.username)}" placeholder="Same as stream" style="flex:1" autocomplete="off"><input type="password" id="p-pass" value="${esc(c.ptz.password)}" placeholder="Same as stream" style="flex:1" autocomplete="new-password"></div>
        <label for="p-speed">Speed</label><div class="row"><input type="range" id="p-speed" min="10" max="100" value="${Math.round(c.ptz.speed * 100)}" style="flex:1"><span class="muted" id="p-speed-v" style="width:44px">${Math.round(c.ptz.speed * 100)}%</span></div>
        <span></span><div class="row"><label class="check"><input type="checkbox" id="p-inv-pan" ${c.ptz.invert_pan ? 'checked' : ''}> Reverse left/right</label><label class="check"><input type="checkbox" id="p-inv-tilt" ${c.ptz.invert_tilt ? 'checked' : ''}> Reverse up/down</label></div>
        <h3>Moving on its own</h3>
        <label>Patrol${tip('The camera visits the presets ticked here in turn, staying a while at each \u2014 like a guard doing rounds. It pauses while something is going on, and for 2 minutes after you steer it by hand. Save presets with \u2606 on the camera\u2019s picture first.')}</label>
        <div><div class="chips" id="p-patrol"><span class="muted small">${id && !clone ? 'Loading presets…' : 'Save the camera first.'}</span></div>
          <div class="row" style="margin-top:6px"><label class="check" style="margin:0"><input type="checkbox" id="p-roam" ${(c.ptz.roam ?? (c.ptz.patrol || []).length > 0) ? 'checked' : ''}> Roam between the ticked presets,</label><span class="muted">stay</span><input type="number" id="p-patrol-s" min="0" max="3600" value="${c.ptz.patrol_seconds ?? 30}" style="width:80px"><span class="muted">seconds at each (0 = straight on)</span></div></div>
        <span></span><div><label class="check"><input type="checkbox" id="p-follow" ${c.ptz.follow ? 'checked' : ''}> Follow people (auto-tracking)${tip('When the AI finds a person (or what you choose below) away from the middle of the picture, the camera turns to keep them in view. When they\u2019re gone, it goes back to its first patrol preset, or its home position. Needs object recognition on for this camera.')}</label>
          <div class="chips" id="p-follow-objs" style="margin-top:4px">${['person', 'vehicle', 'dog', 'cat'].map(o => `<label><input type="checkbox" value="${o}" ${canonKinds(c.ptz.follow_objects && c.ptz.follow_objects.length ? c.ptz.follow_objects : ['person']).includes(o) ? 'checked' : ''}>${labelIcon(o)} ${o}</label>`).join('')}</div></div>
        <span></span><div><button type="button" id="p-test">Test PTZ connection</button><div id="p-result" class="hint" style="margin-top:6px"></div></div>
      </div>
      <div data-tab="device" hidden><div id="dv-body"></div></div>
      <div data-tab="zones" hidden>
        <div class="row" style="margin-bottom:8px"><button type="button" id="z-add-area">＋ Area</button><button type="button" id="z-add-line">＋ Line</button>
          <span class="muted" id="z-msg">Click on the picture to place the corners; click the first corner (or Done) to finish.</span>${tip('Areas: alert only for things inside them, or when something stays too long (loitering). Lines: alert when something crosses, in one or both directions — e.g. a line across your gate. Each can be limited to some kinds of objects.')}<button type="button" id="z-done" hidden class="primary">Done</button></div>
        <div class="surface zone-ed" id="z-prev"><img alt=""><svg id="z-svg" viewBox="0 0 1 1" preserveAspectRatio="none"></svg></div>
        <div id="z-list"></div>
      </div>
    </div>
    <div class="foot"><div class="err c-err" id="c-err" hidden></div><button type="button" data-close>Cancel</button><button class="primary" id="c-save">Save camera</button></div>`;
  $('c-transport').value = c.rtsp_transport;
  $('d-trig').value = c.detect.trigger;
  const f = $('f-cam');
  f.querySelectorAll('[data-close]').forEach(b => b.onclick = closeCameraDialog);
  wireSoundDetect(c.id);
  f.querySelectorAll('#c-tabs button').forEach(b => b.onclick = () => {
    f.querySelectorAll('#c-tabs button').forEach(x => x.classList.toggle('active', x === b));
    f.querySelectorAll('[data-tab]').forEach(t => t.hidden = t.dataset.tab !== b.dataset.t);
    if (b.dataset.t === 'device' && !deviceLoaded) loadDeviceTab();
    if (b.dataset.t === 'zones') drawZones();
  });
  deviceLoaded = false; deviceDirty = new Set(); saveDevice = null;
  setupRestartButton(id && !clone ? id : null);
  setupPrivacy(c, id && !clone ? id : null);
  ['yaw', 'pitch', 'width', 'lens'].forEach(k => { const r = $('c-fe-' + k); if (r) r.oninput = () => { $('c-fe-' + k + '-v').textContent = r.value + '°'; }; });
  // Patrol: the camera's presets to choose from.
  if (id && !clone && c.ptz.enabled) api(`api/cameras/${enc(id)}/ptz/presets`).then(list => {
    const box = $('p-patrol'); if (!box) return;
    box.dataset.loaded = '1';
    box.innerHTML = list.length ? list.map(x => `<label><input type="checkbox" value="${esc(x.token)}" ${(c.ptz.patrol || []).includes(x.token) ? 'checked' : ''}>${esc(x.name)}</label>`).join('')
      : '<span class="muted small">No presets yet: save some with ☆ on the camera\u2019s picture.</span>';
  }).catch(e => { const box = $('p-patrol'); if (box) box.innerHTML = `<span class="muted small">Couldn't read the presets: ${esc(e.message)}</span>`; });
  else if ($('p-patrol') && id && !clone) $('p-patrol').innerHTML = '<span class="muted small">Turn on pan / tilt / zoom and save, then choose presets here.</span>';
  setupProfiles(c);
  $('cam-dialog').classList.toggle('profile-mode', !!prof);
  if (prof) {
    // Only the settings a profile holds: Recording, Motion and AI.
    f.querySelector('.head h2').textContent = prof.idx >= 0 ? `Edit profile “${prof.name}”` : `New profile “${prof.name}”`;
    f.querySelectorAll('#c-tabs button').forEach(b => b.hidden = !['recording', 'detection', 'ai', 'audio'].includes(b.dataset.t));
    // Talking needs this camera's own speaker: not in a profile.
    f.querySelectorAll('.cam-only').forEach(x => x.hidden = true);
    f.querySelector('#c-tabs [data-t=recording]').click();
    $('c-save').textContent = 'Save profile';
    // The camera-only fields (hidden here) mustn't stop the form being sent.
    f.querySelectorAll('[required]').forEach(x => x.required = false);
    // Areas drawn on a camera's own picture stay with each camera.
    for (const el of [$('d-mask'), $('d-pkg-area')]) {
      const box = el.closest('div:not(.surface)'), label = box && box.previousElementSibling;
      if (box) box.hidden = true; if (label && label.tagName === 'LABEL') label.hidden = true;
    }
  }
  setupZones(c, id && !clone ? id : null);
  setupPackageArea(c, id && !clone ? id : null);
  // Passwords hidden: in the stream addresses too, with an eye to show them.
  for (const x of ['c-url', 'c-live', 'c-talk-url']) if ($(x)) maskUrlInput($(x));
  for (const x of ['q-pass', 'p-pass', 'c-rtmp']) if ($(x)) eyeFor($(x));
  // "More things to watch for": a ticked one joins the list (ticked).
  if ($('d-more')) $('d-more').onchange = (e) => {
    const inp = e.target; if (!inp.checked) return;
    const o = inp.value; inp.closest('label').remove();
    $('d-objs').insertAdjacentHTML('beforeend', `<label><input type="checkbox" value="${esc(o)}" checked>${esc(o)}</label>`);
    const pm = $('d-parked-more') && $('d-parked-more').querySelector(`input[value="${CSS.escape(o)}"]`);
    if (pm) $('d-parked-objs').appendChild(pm.closest('label'));
    else if (!$('d-parked-objs').querySelector(`input[value="${CSS.escape(o)}"]`)) $('d-parked-objs').insertAdjacentHTML('beforeend', `<label><input type="checkbox" value="${esc(o)}">${esc(o)}</label>`);
  };
  // Tapo: the account password makes the speaker address.
  const tapoLike = () => /tapo|vigi/i.test(val('q-brand') || '') || /^tapo:\/\//i.test(val('c-talk-url').trim()) || /:554\/stream[12]\s*$/.test(val('c-url').trim());
  const paintTapo = () => { $('c-tapo-row').hidden = !tapoLike(); };
  paintTapo();
  for (const x of ['q-brand', 'c-url', 'c-talk-url']) if ($(x)) $(x).addEventListener('change', paintTapo);
  $('c-tapo-use').onclick = () => {
    const pw = val('c-tapo-pw'), host = cameraHost();
    if (!pw) return toast('Enter the TP-Link account password', true);
    if (!host) return toast('Enter the camera’s IP address first (General tab)', true);
    const el = $('c-talk-url'); el.value = `tapo://admin:${encodeURIComponent(pw)}@${host}`;
    el.dispatchEvent(new Event('input', {bubbles: true})); el.dispatchEvent(new Event('change', {bubbles: true}));
    $('c-tapo-pw').value = '';
    if (!chk('c-talk')) { $('c-talk').checked = true; $('c-talk').dispatchEvent(new Event('change', {bubbles: true})); }
    toast('Speaker address set — press Test, then Save camera');
  };
  $('c-talk-test').onclick = async () => {
    const r = $('c-talk-res'); r.textContent = 'Connecting to the speaker…'; r.className = 'hint';
    try {
      const j = await api('api/talk-test', {method: 'POST', json: {camera: id && !clone ? id : '', url: val('c-talk-url').trim() || val('c-url').trim()}});
      r.textContent = '✓ ' + j.message; r.className = 'hint okc';
      $('c-talk').checked = true;
    } catch (e) { r.textContent = '✗ ' + e.message; r.className = 'hint errc'; }
  };
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
  $('d-objpx').oninput = $('d-travel').oninput = () => drawMotionBoxes($('d-mask-boxes'), lastMeterObjects);
  drawMotionBoxes($('d-mask-boxes'), []);
  setupOsdTab(c, id && !clone ? id : null);
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
    $('c-url').value = brandUrl(b, 'main'); $('c-live').value = brandUrl(b, 'sub');
  };
  // Choosing Tapo: UDP video (much smoother from these Wi-Fi cameras) and
  // Tapo's control port for pan / tilt / zoom. Nothing else changes.
  $('q-brand').addEventListener('change', tapoDefaults);
  // Changing the address or login updates the stream URLs: from the brand
  // template if one is chosen, otherwise by editing the existing URLs (e.g.
  // after cloning a camera, just type the new IP).
  for (const k of ['q-ip', 'q-user', 'q-pass']) $(k).oninput = () => {
    if (val('q-brand')) return $('q-brand').onchange();
    for (const id of ['c-url', 'c-live']) $(id).value = rewriteUrl(val(id));
  };
  $('c-test').onclick = testCamera;
  $('q-web').onclick = () => openCameraPage(cameraHost());
  $('c-rotate').addEventListener('change', testRotate);
  const origDir = c.storage_dir;
  $('c-dir').oninput = () => { if ($('c-move-row')) $('c-move-row').hidden = val('c-dir').trim() === origDir; };
  $('c-browse').onclick = async () => {
    const p = await pickFolder(val('c-dir') || (settings ? settings.storage.dir : ''), 'Storage folder for this camera');
    if (p) { $('c-dir').value = p; $('c-dir').oninput(); }
  };
  f.onsubmit = saveCamera;
  // Previous / next camera without closing the editor (same tab stays open).
  let dirty = false;
  f.addEventListener('input', () => { dirty = true; });
  f.addEventListener('change', () => { dirty = true; });
  f.querySelectorAll('[data-nav]').forEach(b => b.onclick = async () => {
    if (dirty && !confirm('Leave this camera without saving your changes?')) return;
    const i = cameraCfgs.findIndex(x => x.id === id), n = cameraCfgs.length;
    const next = cameraCfgs[(i + +b.dataset.nav + n) % n];
    const tab = (f.querySelector('#c-tabs button.active') || {}).dataset;
    stopPreviews(); stopMotionMeter();
    await openCameraDialog(next.id);
    if (tab && tab.t) { const t = $('f-cam').querySelector(`#c-tabs button[data-t="${tab.t}"]`); if (t) t.click(); }
  });
  if (!$('cam-dialog').open) $('cam-dialog').showModal();
}
function closeCameraDialog() { stopPreviews(); $('cam-dialog').close(); }
$('cam-dialog').addEventListener('close', () => { if (privUnbind) { privUnbind(); privUnbind = null; } if (pkgUnbind) { pkgUnbind(); pkgUnbind = null; } stopPreviews(); stopMotionMeter(); if (osdPrevUnbind) { osdPrevUnbind(); osdPrevUnbind = null; } if (zoneUnbind) { zoneUnbind(); zoneUnbind = null; } });

// ---------- privacy areas: boxes dragged on the picture ----------
let privRects = [], privUnbind = null;
function setupPrivacy(c, camId) {
  privRects = (c.privacy || []).map(b => [...b]);
  const box = $('c-priv');
  const draw = () => {
    box.querySelectorAll('.pv').forEach(e => e.remove());
    privRects.forEach((b, i) => {
      const d = document.createElement('div');
      d.className = 'pv';
      Object.assign(d.style, {left: b[0] * 100 + '%', top: b[1] * 100 + '%', width: b[2] * 100 + '%', height: b[3] * 100 + '%'});
      d.innerHTML = '<button type="button" class="small" title="Remove this area">✕</button>';
      d.querySelector('button').onpointerdown = (e) => e.stopPropagation();
      d.querySelector('button').onclick = () => { privRects.splice(i, 1); draw(); };
      box.appendChild(d);
    });
  };
  const at = (e) => { const r = box.getBoundingClientRect(); return [Math.min(Math.max((e.clientX - r.left) / r.width, 0), 1), Math.min(Math.max((e.clientY - r.top) / r.height, 0), 1)]; };
  let start = null, live = null;
  box.onpointerdown = (e) => {
    if (e.target.closest('button')) return;
    start = at(e); box.setPointerCapture(e.pointerId);
    live = document.createElement('div'); live.className = 'pv drawing'; box.appendChild(live);
  };
  box.onpointermove = (e) => {
    if (!start) return;
    const p = at(e), x = Math.min(start[0], p[0]), y = Math.min(start[1], p[1]);
    Object.assign(live.style, {left: x * 100 + '%', top: y * 100 + '%', width: Math.abs(p[0] - start[0]) * 100 + '%', height: Math.abs(p[1] - start[1]) * 100 + '%'});
  };
  box.onpointerup = (e) => {
    if (!start) return;
    const p = at(e), b = [Math.min(start[0], p[0]), Math.min(start[1], p[1]), Math.abs(p[0] - start[0]), Math.abs(p[1] - start[1])];
    start = null; live.remove();
    if (b[2] > 0.01 && b[3] > 0.01) { privRects.push(b.map(v => +v.toFixed(4))); draw(); }
  };
  $('c-priv-clear').onclick = () => { privRects = []; draw(); };
  if (privUnbind) { privUnbind(); privUnbind = null; }
  if (camId) privUnbind = Live.bind(box.querySelector('img'), camId, 'sub');
  draw();
}

// ---------- motion mask editor & live motion readout ----------
let maskCells = [], maskUnbind = null, meterTimer = null;
// One cell per pixel motion detection looks at (a 640×360 picture shrunk 4×).
const MASK_W = 160, MASK_H = 90;
// Stored as "160x90:" + run lengths (unmasked, masked, unmasked, …).
function maskValue() { return encodeCells(maskCells); }
function decodeMask(m) {
  const out = new Array(MASK_W * MASK_H).fill(false);
  m = (m || '').trim();
  if (m.length === 576 && /^[01]+$/.test(m)) { // the old 32×18 grid
    for (let i = 0; i < out.length; i++) { const x = i % MASK_W, y = Math.floor(i / MASK_W); out[i] = m[Math.floor(y * 18 / MASK_H) * 32 + Math.floor(x * 32 / MASK_W)] === '1'; }
  } else if (m.startsWith(`${MASK_W}x${MASK_H}:`)) {
    let i = 0, on = false;
    for (const r of m.slice(m.indexOf(':') + 1).split(',')) { const n = +r || 0; out.fill(on, i, Math.min(out.length, i + n)); i += n; on = !on; }
  }
  return out;
}
function encodeCells(cells) {
  if (!cells.some(x => x)) return '';
  const runs = []; let cur = false, n = 0;
  for (const v of cells) { if (v === cur) n++; else { runs.push(n); cur = v; n = 1; } }
  runs.push(n);
  return `${MASK_W}x${MASK_H}:` + runs.join(',');
}
// Painting cells on a camera picture (ignore areas, the package spot):
// paint / erase tools, a round brush, strokes without gaps. `cells()`
// gives the array painted; `rgba` its colour; `changed` is called after
// each stroke.
function wirePainter({cv, cells, rgba, tools, brush, brushKey, brushDefault, changed}) {
  const ctx = cv.getContext('2d');
  let tool = 'paint';
  const paint = () => {
    const img = ctx.createImageData(MASK_W, MASK_H), d = img.data, c = cells();
    for (let i = 0; i < c.length; i++) if (c[i]) { d[i * 4] = rgba[0]; d[i * 4 + 1] = rgba[1]; d[i * 4 + 2] = rgba[2]; d[i * 4 + 3] = rgba[3]; }
    ctx.putImageData(img, 0, 0);
  };
  paint();
  tools.querySelectorAll('button').forEach(b => {
    b.classList.toggle('on', b.dataset.v === tool);
    b.onclick = () => { tool = b.dataset.v; tools.querySelectorAll('button').forEach(x => x.classList.toggle('on', x === b)); cv.style.cursor = tool === 'erase' ? 'cell' : 'crosshair'; };
  });
  brush.value = store.get(brushKey, brushDefault); brush.onchange = () => store.set(brushKey, brush.value);
  const at = (e) => { const r = cv.getBoundingClientRect(); return [(e.clientX - r.left) / r.width * MASK_W, (e.clientY - r.top) / r.height * MASK_H]; };
  const dab = (x, y, val) => {
    const size = +brush.value || 3, rad = size / 2, c = cells();
    let ch = false;
    for (let cy = Math.floor(y - rad); cy <= Math.ceil(y + rad); cy++) for (let cx = Math.floor(x - rad); cx <= Math.ceil(x + rad); cx++) {
      if (cx < 0 || cy < 0 || cx >= MASK_W || cy >= MASK_H) continue;
      if (size > 1 && (cx + 0.5 - x) ** 2 + (cy + 0.5 - y) ** 2 > rad * rad) continue;
      if (size === 1 && (cx !== Math.floor(x) || cy !== Math.floor(y))) continue;
      const i = cy * MASK_W + cx;
      if (c[i] !== val) { c[i] = val; ch = true; }
    }
    return ch;
  };
  let val = null, last = null;
  const stroke = (e) => {
    const p = at(e), from = last || p;
    const steps = Math.max(1, Math.ceil(Math.hypot(p[0] - from[0], p[1] - from[1]) / 0.5));
    let ch = false;
    for (let k = 1; k <= steps; k++) ch = dab(from[0] + (p[0] - from[0]) * k / steps, from[1] + (p[1] - from[1]) * k / steps, val) || ch;
    last = p;
    if (ch) paint();
  };
  cv.oncontextmenu = (e) => e.preventDefault();
  cv.onpointerdown = (e) => { e.preventDefault(); val = !(tool === 'erase' || e.button === 2 || e.shiftKey); last = null; cv.setPointerCapture(e.pointerId); stroke(e); };
  cv.onpointermove = (e) => { if (val !== null) stroke(e); };
  cv.onpointerup = cv.onpointercancel = () => { if (val !== null && changed) changed(); val = null; last = null; };
  return paint;
}
function setupMaskEditor(mask, camId) {
  maskCells = decodeMask(mask);
  const cv = $('d-mask-grid');
  const paint = wirePainter({cv, cells: () => maskCells, rgba: [239, 68, 68, 115], tools: $('d-mask-tool'), brush: $('d-mask-brush'), brushKey: 'maskBrush', brushDefault: '3'});
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
// Package delivery spot (camera → AI): painted on the camera picture like
// the ignore areas.
let pkgCells = [], pkgUnbind = null;
function setupPackageArea(c, camId) {
  pkgCells = decodeMask(c.detect.package_mask);
  const paint = wirePainter({cv: $('d-pkg-grid'), cells: () => pkgCells, rgba: [255, 176, 32, 120], tools: $('d-pkg-tool'), brush: $('d-pkg-brush'), brushKey: 'pkgBrush', brushDefault: '7',
    changed: () => { if (pkgCells.some(x => x)) $('d-pkg').checked = true; }});
  $('d-pkg-clear').onclick = () => { pkgCells.fill(false); paint(); };
  // The camera's live picture (light stream 2, as for the motion mask).
  if (pkgUnbind) { pkgUnbind(); pkgUnbind = null; }
  if (camId) {
    const img = $('d-pkg-img'), m = $('d-pkg-area').querySelector('.msg');
    if (m) { m.textContent = 'Connecting…'; m.hidden = false; }
    img.onload = () => { if (m) m.hidden = true; };
    pkgUnbind = Live.bind(img, camId, 'sub');
  }
}

// A password box with an eye button that shows or hides what's typed.
// (Every password box has its eye now — see addPasswordEyes.)
function eyeFor(input) {
  return;
  const b = input._eye = document.createElement('button');
  b.type = 'button'; b.className = 'eye-btn'; b.title = 'Show the password'; b.textContent = '👁';
  b.onclick = () => { const show = input.type === 'password'; input.type = show ? 'text' : 'password'; b.classList.toggle('on', show); b.title = show ? 'Hide the password' : 'Show the password'; };
  input.after(b);
}
// A stream address box: the password in it shows as dots (the eye shows
// it). Reading the box (.value) still gives the real address; editing the
// rest of it keeps the password; typing over the dots replaces it.
function maskUrlInput(input) {
  if (input._masked) return;
  input._masked = true;
  const desc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value');
  const re = /^([a-z][a-z0-9+.-]*:\/\/[^:\/@\s]*):([^@\s]*)@/i;
  let real = desc.get.call(input), shown = false;
  const dots = (p) => '•'.repeat(Math.min(10, Math.max(6, p.length)));
  const paint = () => desc.set.call(input, shown ? real : real.replace(re, (m, a, pw) => pw ? `${a}:${dots(pw)}@` : m));
  Object.defineProperty(input, 'value', {configurable: true, get: () => real, set: (v) => { real = String(v ?? ''); paint(); }});
  input.addEventListener('input', () => {
    const typed = desc.get.call(input);
    if (shown) { real = typed; return; }
    const m = typed.match(re), r = real.match(re);
    if (m && r && /^•+$/.test(m[2])) real = typed.replace(re, `${m[1]}:${r[2]}@`);     // password untouched
    else if (m && m[2].includes('•')) real = typed.replace(re, `${m[1]}:${m[2].replace(/•/g, '')}@`);  // typed over the dots
    else real = typed;
  });
  input.addEventListener('blur', paint);
  const b = document.createElement('button');
  b.type = 'button'; b.className = 'eye-btn'; b.title = 'Show the password in the address'; b.textContent = '👁';
  b.onclick = () => { shown = !shown; b.classList.toggle('on', shown); b.title = shown ? 'Hide the password' : 'Show the password in the address'; paint(); };
  input.after(b);
  paint();
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
  // "Object must travel": how far an object's centre has to move from where
  // it first moved (pixels of a 640-wide picture, the same both ways), as a
  // ring around the middle with an arrow to its edge.
  const tr = $('d-travel') ? +$('d-travel').value : 0;
  if (tr > 0) {
    const rx = tr / 640, ry = tr / 360;
    out += `<ellipse cx="0.5" cy="0.5" rx="${rx}" ry="${ry}" fill="rgba(34,197,94,.08)" stroke="#22c55e" stroke-width="2" stroke-dasharray="3 4" vector-effect="non-scaling-stroke"/>`
      + `<line x1="0.5" y1="0.5" x2="${Math.min(0.995, 0.5 + rx)}" y2="0.5" stroke="#22c55e" stroke-width="2" vector-effect="non-scaling-stroke"/>`
      + `<circle cx="0.5" cy="0.5" r="0.006" fill="#22c55e"/>`;
  }
  svg.innerHTML = out;
}
// Places on the picture: a 3×3 grid (top/middle/bottom × left/centre/right).
const SPOTS = [['tl', 'Top left'], ['tc', 'Top centre'], ['tr', 'Top right'], ['ml', 'Middle left'], ['mc', 'Centre'], ['mr', 'Middle right'], ['bl', 'Bottom left'], ['bc', 'Bottom centre'], ['br', 'Bottom right']];
// Position picker: nine buttons laid out like places on the picture.
function cornerSel(id, cur) {
  return `<span class="corner-pick" data-for="${id}"><input type="hidden" id="${id}" value="${cur || 'tl'}">${SPOTS
    .map(([k, t]) => `<button type="button" data-c="${k}" title="${t}" class="${k === (cur || 'tl') ? 'on' : ''}"></button>`).join('')}</span>`;
}
document.addEventListener('click', (e) => {
  const b = e.target.closest('.corner-pick button');
  if (!b || b.disabled) return;
  const inp = b.parentElement.querySelector('input');
  inp.value = b.dataset.c;
  paintCorners();
  inp.dispatchEvent(new Event('input', {bubbles: true}));
  inp.dispatchEvent(new Event('change', {bubbles: true}));
});
function paintCorners() {
  document.querySelectorAll('.corner-pick').forEach(p => {
    const inp = p.querySelector('input');
    p.querySelectorAll('button').forEach(b => { b.classList.toggle('on', b.dataset.c === inp.value); b.disabled = inp.disabled; });
  });
}
// The camera's own on-screen display choices (kept while the controls show the global ones).
let osdOwn = null;
function readOsd(raw) {
  if (!raw && !chk('o-custom') && osdOwn) return {...osdOwn, custom: false};
  const f = val('o-fmt2') === 'custom' ? val('o-fmt2-c').trim() : val('o-fmt2');
  return {custom: chk('o-custom'), show_name: chk('o-name'), name_corner: val('o-name-c'), show_clock: chk('o-clock'), clock_corner: val('o-clock-c'), clock_format: f,
    show_status: chk('o-status'), status_corner: val('o-status-c'), show_detections: chk('o-detections'), detections_corner: val('o-detections-c'),
    show_fps: chk('o-fps'), fps_corner: val('o-fps-c'), show_res: chk('o-res'), res_corner: val('o-res-c'), text: val('o-text').trim(), text_corner: val('o-text-c')};
}
let osdPrevUnbind = null;
function setupOsdTab(c, camId) {
  const prev = $('o-prev');
  if (!CLOCK_FORMATS.includes(c.osd.clock_format) && c.osd.clock_format) { $('o-fmt2').value = 'custom'; $('o-fmt2-c').hidden = false; }
  const live = camId ? cam(camId) : null;
  const ctrls = ['o-name', 'o-name-c', 'o-clock', 'o-clock-c', 'o-status', 'o-status-c', 'o-detections', 'o-detections-c', 'o-fps', 'o-fps-c', 'o-res', 'o-res-c', 'o-fmt2', 'o-fmt2-c', 'o-text', 'o-text-c'];
  osdOwn = null;
  const showFollow = () => {
    const custom = chk('o-custom');
    if (!custom) {
      // Following Settings → Live view & display: show those values, read-only.
      if (osdOwn === null || $('o-name').disabled === false) osdOwn = readOsd(true);
      const g = osdFor({osd: {custom: false}});
      for (const k of ['name', 'clock', 'status', 'detections', 'fps', 'res']) { $('o-' + k).checked = !!g['show_' + k]; $('o-' + k + '-c').value = g[k + '_corner'] || 'tl'; }
      $('o-fmt2').value = ''; $('o-fmt2-c').hidden = true;
    } else if (osdOwn && $('o-name').disabled) {
      for (const k of ['name', 'clock', 'status', 'detections', 'fps', 'res']) { $('o-' + k).checked = !!osdOwn['show_' + k]; $('o-' + k + '-c').value = osdOwn[k + '_corner'] || 'tl'; }
    }
    ctrls.forEach(id => { if ($(id)) $(id).disabled = !custom; });
    paintCorners();
  };
  $('o-custom').addEventListener('change', showFollow);
  showFollow();
  const redraw = () => {
    prev.querySelectorAll('.ov').forEach(o => o.remove());
    const fake = {...(live || {id: '', name: val('c-name') || 'Camera', enabled: true, recorder: null, buffer: null, detect: null, record_mode: 'continuous'}), osd: {...readOsd(), custom: true}};
    if (!chk('o-custom')) fake.osd = {custom: false};
    fake.name = val('c-name') || fake.name;
    prev.insertAdjacentHTML('beforeend', overlayHtml(fake, 'sub'));
    // Example alert so it can be seen and placed.
    prev.querySelectorAll('.ov-alert').forEach(a => a.innerHTML = `<span class="alertbadge">${labelIcon('person')} person</span>`);
    prev.querySelectorAll('[data-osd]').forEach(el => el.classList.add('osd-drag'));
    paintCorners();
    if (live) updateOverlay(prev, fake, 'sub'); else prev.querySelectorAll('.ov-clock').forEach(e => e.textContent = fmtDate(new Date(), e.dataset.fmt || undefined));
  };
  $('f-cam').querySelector('[data-tab=osd]').addEventListener('input', redraw);
  // Drag an item on the preview to place it.
  wireOsdDrag(prev, 'o', () => chk('o-custom'), redraw);
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
  out.innerHTML = `<div class="probe-grid">${urls.map((u, i) => `<div><div class="surface"><img id="pv-${i}" alt="" data-rot="${+val('c-rotate') || 0}"><div class="msg" id="pv-msg-${i}">Connecting…</div></div><div id="pv-info-${i}" class="hint"><b>${u[0]}</b></div></div>`).join('')}</div>`;
  // One at a time: many cameras allow only two or three connections at once
  // (Tapo, Reolink …), and refuse more ("SETUP failed: 500"). Each stream
  // is checked first, then the live pictures start.
  const transport = val('c-transport');
  const busy = (e) => /SETUP failed|\b(500|503|453|454)\b|Server returned 5|too many|session/i.test(e || '');
  const results = [];
  for (const [i, [label, url]] of urls.entries()) {
    let r;
    for (let attempt = 0; attempt < 3; attempt++) {
      try { r = await api('api/probe', {method: 'POST', json: {url, rtsp_transport: transport}}); } catch (e) { r = {ok: false, error: e.message}; }
      if (r.ok || !busy(r.error)) break;
      $('pv-msg-' + i).textContent = 'The camera is busy — trying again…';
      await new Promise(res => setTimeout(res, 1500 * (attempt + 1)));
    }
    results.push(r);
  }
  for (const [i, [label, url]] of urls.entries()) {
    const r = results[i];
    try {
      const info = $('pv-info-' + i), msg = $('pv-msg-' + i);
      if (!r.ok) { msg.innerHTML = `<span class="err">✗ ${esc(explainError(r.error || 'Could not connect'))}</span>` + (busy(r.error) ? '<div class="hint">The camera refused another connection — it may allow only a few at once. Once saved, GAZE uses one connection per stream.</div>' : ''); info.innerHTML = `<b>${label}</b>`; continue; }
      const pv = await api('api/preview', {method: 'POST', json: {url, rtsp_transport: transport}});
      const img = $('pv-' + i);
      img.onload = () => msg.hidden = true;
      // A busy camera may refuse the picture at first: once more.
      img.onerror = () => { if (!img._again) { img._again = true; setTimeout(() => { img.src = pv.url + (pv.url.includes('?') ? '&' : '?') + 'r=' + Date.now(); }, 2000); } };
      img.src = pv.url; previewImgs.push(img);
      const v = r.streams.find(s => s.kind === 'video'), a = r.streams.find(s => s.kind === 'audio');
      const hevc = v && /hevc|h265/i.test(v.codec);
      info.innerHTML = `<b>${label}</b> <span class="ok">✓ connected</span><br>${v ? `${String(v.codec).toUpperCase()} ${v.width}×${v.height}${v.fps ? ' @ ' + v.fps + ' fps' : ''}` : ''}${a ? ' · audio ' + esc(a.codec) : ' · no audio'}`
        + (hevc && i === 0 ? '<br><span class="warnc">H.265: many browsers cannot play it. Consider “Convert recordings to H.264” or setting the camera to H.264.</span>' : '');
    } catch (e) { $('pv-msg-' + i).innerHTML = `<span class="err">${esc(e.message)}</span>`; }
  }
  btn.disabled = false; btn.textContent = '▶ Test connection';
  testRotate();
}
// The test pictures turned as chosen under "Rotate picture".
function testRotate() {
  const r = +val('c-rotate') || 0;
  document.querySelectorAll('#c-probe .probe-grid img').forEach(i => { i.dataset.rot = r; });
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
  if (profileEdit) return saveProfileForm(body);
  // ONVIF tab changes not yet applied: sent to the camera first.
  if (editingId && deviceLoaded && saveDevice && deviceDirty.size) {
    const btn = $('c-save'); btn.disabled = true; btn.textContent = 'Saving to the camera…';
    try { const done = await saveDevice(); if (done.length) toast('On the camera: ' + done.join(', ') + ' changed'); }
    catch (e) { return err('The camera refused: ' + e.message); }
    finally { btn.disabled = false; btn.textContent = 'Save camera'; }
  }
  return finishSave(body);
}
// Save the camera window's Recording, Motion and AI settings as the profile
// being edited; cameras following it change too.
async function saveProfileForm(body) {
  const pe = profileEdit, p = {name: pe.name};
  for (const k of PROFILE_KEYS) p[k] = JSON.parse(JSON.stringify(body[k]));
  p.detect.mask = ''; p.detect.packages = false; p.detect.package_area = []; p.detect.package_mask = '';
  const profiles = [...((settings && settings.profiles) || [])];
  if (pe.idx >= 0) profiles[pe.idx] = p; else profiles.push(p);
  // The window closes at once; the profile is saved (and the cameras
  // following it updated) in the background. If it's refused, the window
  // comes back with what was set and why, unless another was opened since.
  const gen = camDlgGen;
  closeCameraDialog();
  toast(`Saving profile “${pe.name}”…`);
  try {
    await api('api/profiles', {method: 'PUT', json: profiles});
    const n = cameraCfgs.filter(c => (c.profile || '').toLowerCase() === pe.name.toLowerCase()).length;
    toast(`Profile “${pe.name}” saved${n ? ` — ${n} camera${n === 1 ? '' : 's'} following it updated` : ''}`);
    if (page === 'settings' && section === 'profiles') showSettings('profiles'); else loadSettings().catch(() => {});
  } catch (e) {
    if (camDlgGen === gen && !$('cam-dialog').open) { profileEdit = pe; $('cam-dialog').showModal(); $('c-err').hidden = false; $('c-err').textContent = e.message; }
    else toast('Profile not saved: ' + e.message, true);
  }
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
    talk: chk('c-talk'), talk_chime: chk('c-chime'), talk_url: val('c-talk-url').trim(), live_sound: chk('c-sound'),
    sound: {enabled: chk('c-snd-on'), sensitivity: +val('c-snd-sens') || 6,
      kinds: $('c-snd-kinds').children.length ? [...document.querySelectorAll('#c-snd-kinds input:checked')].map(i => i.value) : JSON.parse($('c-snd-kinds').dataset.kinds)},
    record_audio: chk('c-audio'), transcode: chk('c-transcode'), record_fps: num('c-fps'), segment_seconds: num('c-seg'),
    storage_dir: val('c-dir').trim(), retention_days: Math.round(num('c-ret')), max_storage_gb: num('c-max'),
    rotate: +val('c-rotate') || 0, profile: camProfile, zones: dlgZones.map(z => ({...z})),
    timeline_copy: $('c-tlcopy') ? chk('c-tlcopy') : true, onvif_events: chk('c-onvif-ev'), onvif_motion: chk('c-onvif-mo'), privacy: privRects, stream_live: chk('c-rtmp-on'), stream_url: val('c-rtmp').trim(), dewarp: {enabled: chk('c-fe'), yaw: num('c-fe-yaw'), pitch: num('c-fe-pitch'), width: num('c-fe-width'), lens: num('c-fe-lens')},
    osd: readOsd(),
    view: {grid_quality: val('c-vq') || 'auto', grid_fps: +val('c-vg') || 0, hd_fps: +val('c-vh') || 0},
    ptz: {enabled: chk('p-on'), host: val('p-host').trim(), port: Math.round(num('p-port')) || 80, username: val('p-user').trim(), password: $('p-pass').value,
      speed: num('p-speed') / 100, invert_pan: chk('p-inv-pan'), invert_tilt: chk('p-inv-tilt'),
      patrol: $('p-patrol').dataset.loaded ? [...document.querySelectorAll('#p-patrol input:checked')].map(i => i.value) : ((cameraCfgs.find(x => x.id === editingId) || {ptz: {}}).ptz.patrol || []), patrol_seconds: Math.max(0, Math.round(num('p-patrol-s')) || 0), roam: chk('p-roam'),
      follow: chk('p-follow'), follow_objects: [...document.querySelectorAll('#p-follow-objs input:checked')].map(i => i.value)},
    detect: {motion: chk('d-motion'), min_contrast: num('d-contrast'), min_object_px: Math.round(num('d-objpx')),
      make_time_ms: Math.round(num('d-make') * 1000), min_travel_px: Math.round(num('d-travel')), mask: maskValue(), ai: chk('d-ai'), objects, min_confidence: num('d-conf') / 100,
      trigger: val('d-trig'), pre_seconds: Math.round(num('d-pre')), post_seconds: Math.round(num('d-post')), join_seconds: Math.max(0, Math.round(num('d-join')) || 0), fps: Math.round(num('d-fps')), ai_interval: num('d-int'), ignore_parked: chk('d-parked'), ignore_animals: chk('d-animals'), hd_check: val('d-hdcheck'), ai_ignores_mask: chk('d-mask-ai'), parked_seconds: Math.max(10, Math.round(num('d-parked-s')) || 60),
      parked_objects: [...document.querySelectorAll('#d-parked-objs input:checked, #d-parked-more input:checked')].map(i => i.value), packages: chk('d-pkg') && pkgCells.some(x => x), package_mask: encodeCells(pkgCells), package_area: []},
  };
}
// ---------- restart the camera over ONVIF ----------
// Shown under General when the camera answers over ONVIF (address and login
// from the Pan / tilt / zoom tab, or the stream URL).
async function setupRestartButton(id) {
  const rows = document.querySelectorAll('.c-restart');
  rows.forEach(r => r.hidden = true);
  if (!id) return;
  let r; try { r = await api(`api/cameras/${enc(id)}/device`); } catch (_) { return; }
  if (!r.ok || editingId !== id) return;
  rows.forEach(x => x.hidden = false);
  $('c-restart-info').textContent = [r.info.manufacturer, r.info.model].filter(Boolean).join(' ');
  $('c-restart').onclick = async () => {
    if (!confirm('Restart the camera now? It will be offline for about a minute.')) return;
    const b = $('c-restart'); b.disabled = true;
    try { await api(`api/cameras/${enc(id)}/device/reboot`, {method: 'POST'}); toast('The camera is restarting — it will be back in about a minute.'); }
    catch (e) { toast(e.message, true); }
    b.disabled = false;
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
  try { res = await api(`api/cameras/${enc(editingId)}/device/report`); } catch (e) {
    box.innerHTML = `<div class="err">${esc(e.message)}</div>`; deviceLoaded = false; return;
  }
  const r = deviceReport = res.report, er = r.errors || {};
  const na = (k) => er[k] ? `<div class="hint">Not available on this camera: ${esc(er[k])}</div>` : '';
  const intro = `<div class="hint" style="margin-bottom:10px">These are the camera's own settings, changed directly on the camera over ONVIF (${esc(res.address)}).
    The address, port and login come from the <b>Pan / tilt / zoom</b> tab (blank there = from the stream URL). Changes are sent to the camera with <b>Apply to the camera</b> at the bottom (or when you press <b>Save camera</b>).
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
    <div class="row"><button type="button" id="dv-reboot">⟳ Restart camera</button><button type="button" id="dv-web">🌐 Open the camera's web page</button></div></div>`;
  // Network
  h += `<div class="dv-sec" data-sec="network"><h3>Network</h3>${na('network')}`;
  if (n) h += `<div class="form">
    <label>Address</label><div class="row"><label class="check"><input type="radio" name="dv-dhcp" value="0" ${n.dhcp ? '' : 'checked'}> Fixed (static)</label><label class="check"><input type="radio" name="dv-dhcp" value="1" ${n.dhcp ? 'checked' : ''}> Automatic (DHCP)</label></div>
    <label for="dv-ip">IP address</label><div><input type="text" id="dv-ip" value="${esc(n.ip)}" placeholder="192.168.1.64"></div>
    <label for="dv-prefix">Subnet mask</label><div><select id="dv-prefix">${Array.from({length: 17}, (_, k) => 16 + k).map(p => `<option value="${p}" ${p === n.prefix ? 'selected' : ''}>${prefixMask(p)} (/${p})</option>`).join('')}</select></div>
    <label for="dv-gw">Gateway (router)</label><div><input type="text" id="dv-gw" value="${esc(n.gateway)}" placeholder="192.168.1.1"></div>
    <label for="dv-dns">DNS servers</label><div><input type="text" id="dv-dns" value="${esc((n.dns || []).join(', '))}" placeholder="192.168.1.1, 8.8.8.8"></div>
    <label for="dv-host">Host name</label><div><input type="text" id="dv-host" value="${esc(n.hostname)}"></div>
    <span></span><div><label class="check"><input type="checkbox" id="dv-upd" checked> Also change this camera's stream addresses in the NVR</label>
      <div class="hint">Changing the address makes the camera reconnect (some restart). Make sure the new address is free and on your network, or you'll have to find the camera again.</div></div>
    </div>`;
  h += '</div>';
  // Time
  h += `<div class="dv-sec" data-sec="time"><h3>Date &amp; time</h3>${na('time')}`;
  if (t) {
    const cam = t.utc ? new Date(t.utc) : null, diff = cam ? Math.round((cam - new Date(res.computer_utc)) / 1000) : null;
    h += `<div class="form">
      <label>Camera clock</label><div>${cam ? `${fmtDate(cam)} — ${Math.abs(diff) < 3 ? '<span class="ok">matches this computer</span>' : `<span class="warnc">${fmtDur(Math.abs(diff))} ${diff > 0 ? 'ahead of' : 'behind'} this computer</span>`}` : '–'}
        <div class="hint">A wrong camera clock can make ONVIF logins fail and puts the wrong time on the picture.</div></div>
      <label for="dv-ntp">Time server</label><div><input type="text" id="dv-ntp" value="${esc((t.ntp || []).join(', '))}" placeholder="pool.ntp.org"></div>
      <label for="dv-mode">Keep time from</label><div><select id="dv-mode"><option value="NTP" ${t.mode !== 'Manual' ? 'selected' : ''}>The time server (recommended)</option><option value="Manual" ${t.mode === 'Manual' ? 'selected' : ''}>Its own clock</option></select></div>
      <label for="dv-tz">Time zone</label><div><input type="text" id="dv-tz" value="${esc(t.tz)}" placeholder="e.g. NZST-12NZDT,M9.5.0,M4.1.0/3">
        <div class="hint">In the camera's format (POSIX). Common: <a href="javascript:void 0" data-tz="NZST-12NZDT,M9.5.0,M4.1.0/3">New Zealand</a> · <a href="javascript:void 0" data-tz="AEST-10AEDT,M10.1.0,M4.1.0/3">Sydney</a> · <a href="javascript:void 0" data-tz="GMT0BST,M3.5.0/1,M10.5.0">UK</a> · <a href="javascript:void 0" data-tz="EST5EDT,M3.2.0,M11.1.0">US Eastern</a> · <a href="javascript:void 0" data-tz="PST8PDT,M3.2.0,M11.1.0">US Pacific</a> · <a href="javascript:void 0" data-tz="CET-1CEST,M3.5.0,M10.5.0/3">Central Europe</a></div></div>
      <span></span><div class="row"><button type="button" id="dv-time-sync">Set camera clock to this computer's time now</button></div></div>`;
  }
  h += '</div>';
  // Video
  h += `<div class="dv-sec"><h3>Video streams</h3>${na('video')}`;
  (r.video || []).forEach((v, k) => {
    const res2 = v.resolutions.length ? v.resolutions : [[v.width, v.height]];
    const fpsOpts = v.fps_options.length ? v.fps_options : Array.from(new Set([1, 2, 5, 8, 10, 12, 15, 20, 25, 30, v.fps])).filter(x => !v.fps_max || (x >= (v.fps_min || 1) && x <= v.fps_max)).sort((a, b) => a - b);
    h += `<div class="dv-stream" data-sec="video" data-k="${k}"><b>${k === 0 ? 'Stream 1 (main)' : k === 1 ? 'Stream 2 (sub)' : 'Stream ' + (k + 1)}</b> <span class="muted">${esc(v.name)}</span>
      <div class="form">
        <label>Video format</label><div>${v.encodings.length > 1 ? `<select class="dv-enc">${v.encodings.map(e => `<option ${e === v.encoding ? 'selected' : ''}>${esc(e)}</option>`).join('')}</select>` : esc(v.encoding)}</div>
        <label>Resolution</label><div><select class="dv-res">${res2.map(([w, hh]) => `<option value="${w}x${hh}" ${w === v.width && hh === v.height ? 'selected' : ''}>${w} × ${hh}</option>`).join('')}</select></div>
        <label>Frame rate</label><div><select class="dv-fps">${fpsOpts.map(x => `<option value="${x}" ${x == v.fps ? 'selected' : ''}>${x} fps</option>`).join('')}</select></div>
        <label>Bitrate</label><div class="row"><input type="number" class="dv-br" value="${v.bitrate}" min="${v.bitrate_min || 32}" ${v.bitrate_max ? `max="${v.bitrate_max}"` : ''} style="width:120px"><span class="muted">kbit/s${v.bitrate_max ? ` (${v.bitrate_min}–${v.bitrate_max})` : ''}</span></div>
        <label>Keyframe interval</label><div class="row"><input type="number" class="dv-gov" value="${v.gov || ''}" min="1" max="400" style="width:120px"><span class="muted">frames (about ${v.fps ? Math.max(1, Math.round((v.gov || 0) / v.fps)) : '?'} s; 1–2 s seeks and starts fastest)</span></div>
        </div></div>`;
  });
  h += '</div>';
  // Picture
  h += `<div class="dv-sec" data-sec="imaging"><h3>Picture</h3>${na('imaging')}`;
  if (im) {
    const sl = (key, label) => im[key] == null ? '' : (() => { const [lo, hi] = (im.ranges || {})[key] || [0, 100]; return `<label>${label}</label><div class="row"><input type="range" id="dv-${key}" min="${lo}" max="${hi}" value="${im[key]}" style="flex:1"><span class="muted" style="width:40px" id="dv-${key}-v">${im[key]}</span></div>`; })();
    const irName = {AUTO: 'Automatic day / night', ON: 'Day (colour)', OFF: 'Night (black & white, infrared)'};
    h += `<div class="form">${sl('brightness', 'Brightness')}${sl('contrast', 'Contrast')}${sl('saturation', 'Colour')}${sl('sharpness', 'Sharpness')}
      ${im.ir_cut != null ? `<label>Day / night</label><div><select id="dv-ir">${(im.ir_modes.length ? im.ir_modes : ['AUTO', 'ON', 'OFF']).map(m => `<option value="${m}" ${m === im.ir_cut ? 'selected' : ''}>${irName[m] || m}</option>`).join('')}</select></div>` : ''}
      </div>`;
  }
  h += '</div>';
  h += '<div class="row dv-apply"><span class="spacer"></span><button type="button" class="primary" id="dv-apply" disabled>Apply to the camera</button></div>';
  box.innerHTML = h;
  const busy = async (btn, fn) => { const t0 = btn.textContent; btn.disabled = true; btn.textContent = 'Saving…'; try { await fn(); } catch (e) { toast(e.message, true); } finally { btn.disabled = false; btn.textContent = t0; } };
  const url = (p) => `api/cameras/${enc(editingId)}/device/${p}`;
  $('dv-reload').onclick = loadDeviceTab;
  $('dv-reboot').onclick = () => { if (confirm('Restart the camera now? It will be offline for about a minute.')) busy($('dv-reboot'), async () => { await api(url('reboot'), {method: 'POST'}); toast('The camera is restarting.'); }); };
  box.querySelectorAll('[data-tz]').forEach(a => a.onclick = () => $('dv-tz').value = a.dataset.tz);
  box.querySelectorAll('input[type=range]').forEach(el => el.oninput = () => { const v = $(el.id + '-v'); if (v) v.textContent = el.value; });
  const syncDhcp = () => { const d = document.querySelector('input[name=dv-dhcp]:checked'); if (!d) return; ['dv-ip', 'dv-prefix', 'dv-gw', 'dv-dns'].forEach(x => $(x).disabled = d.value === '1'); };
  box.querySelectorAll('input[name=dv-dhcp]').forEach(x => x.onchange = syncDhcp); syncDhcp();
  // Remember which parts were changed; "Save camera" sends only those.
  deviceDirty = new Set();
  box.querySelectorAll('[data-sec]').forEach(sec => {
    const mark = () => deviceDirty.add(sec.dataset.sec === 'video' ? 'video:' + sec.dataset.k : sec.dataset.sec);
    sec.addEventListener('input', mark); sec.addEventListener('change', mark);
    const on = () => { $('dv-apply').disabled = false; };
    sec.addEventListener('input', on); sec.addEventListener('change', on);
  });
  $('dv-web').onclick = () => openCameraPage(res.address.replace(/:\d+$/, ''));
  $('dv-apply').onclick = () => busy($('dv-apply'), async () => { const done = await saveDevice(); toast(done.length ? 'On the camera: ' + done.join(', ') + ' changed' : 'Nothing to change'); $('dv-apply').disabled = true; });
  const timeBody = () => ({mode: val('dv-mode'), tz: val('dv-tz').trim(), ntp_dhcp: false, ntp: val('dv-ntp').split(/[,\s]+/).filter(Boolean)});
  if ($('dv-time-sync')) $('dv-time-sync').onclick = () => busy($('dv-time-sync'), async () => { await api(url('time'), {method: 'PUT', json: {...timeBody(), sync_clock: true}}); toast("The camera's clock now matches this computer."); loadDeviceTab(); });
  saveDevice = async () => {
    const done = [];
    if (deviceDirty.has('network') && n) {
      const dhcp = document.querySelector('input[name=dv-dhcp]:checked').value === '1';
      const body = {token: n.token, mac: n.mac, dhcp, ip: val('dv-ip').trim(), prefix: +val('dv-prefix'), gateway: val('dv-gw').trim(),
        dns: val('dv-dns').split(/[,\s]+/).filter(Boolean), dns_dhcp: n.dns_dhcp, hostname: val('dv-host').trim(), update_nvr: chk('dv-upd')};
      const changing = dhcp !== n.dhcp || body.ip !== n.ip || body.prefix !== n.prefix;
      if (!changing || confirm((dhcp ? 'Switch the camera to an automatic (DHCP) address? Its address may change and the NVR will lose it until you enter the new address.'
        : `Change the camera's address from ${n.ip} to ${body.ip}?`) + '\n\nSome cameras restart to apply a new address; that is done automatically.')) {
        const r2 = await api(url('network'), {method: 'PUT', json: body});
        done.push('network' + (r2.nvr_updated ? ' (the NVR now uses the new address)' : '') + (r2.restarted ? ' — the camera is restarting' : ''));
      }
    }
    if (deviceDirty.has('time') && t) { await api(url('time'), {method: 'PUT', json: timeBody()}); done.push('date & time'); }
    for (const el of box.querySelectorAll('.dv-stream')) {
      if (!deviceDirty.has('video:' + el.dataset.k)) continue;
      const v = r.video[+el.dataset.k], [w, hh] = el.querySelector('.dv-res').value.split('x').map(Number), enc2 = el.querySelector('.dv-enc');
      await api(url('video'), {method: 'PUT', json: {token: v.token, width: w, height: hh, fps: +el.querySelector('.dv-fps').value, bitrate: Math.round(+el.querySelector('.dv-br').value),
        gov: Math.round(+el.querySelector('.dv-gov').value) || 0, encoding: enc2 ? enc2.value : ''}});
      done.push(`stream ${+el.dataset.k + 1}`);
    }
    if (deviceDirty.has('imaging') && im) {
      const g = (k) => $('dv-' + k) ? +$('dv-' + k).value : null;
      await api(url('imaging'), {method: 'PUT', json: {brightness: g('brightness'), contrast: g('contrast'), saturation: g('saturation'), sharpness: g('sharpness'), ir_cut: $('dv-ir') ? $('dv-ir').value : null}});
      done.push('picture');
    }
    deviceDirty.clear();
    return done;
  };
}
let deviceDirty = new Set(), saveDevice = null;
// The camera's own web page (its settings), in a new tab.
function openCameraPage(host) {
  if (!host) return toast('Enter the camera’s IP address first', true);
  window.open('http://' + host + '/', '_blank', 'noopener');
}
// The camera's address, from the quick setup or the stream address.
function cameraHost() {
  const ip = ($('q-ip') && val('q-ip').trim()) || '';
  if (ip) return ip;
  const m = (val('c-url') || '').match(/^[a-z]+:\/\/(?:[^@\/]*@)?([^:\/?]+)/i);
  return m ? m[1] : '';
}

// ---------- zones ----------
let dlgZones = [], zoneDraft = null, zoneUnbind = null;
const ZONE_COLORS = ['#22c55e', '#4f8cff', '#f59e0b', '#ec4899', '#a78bfa', '#14b8a6', '#ef4444', '#eab308'];
const ZONE_OBJECTS = ['person', 'vehicle', 'bicycle', 'dog', 'cat', 'bird'];
function setupZones(c, camId) {
  dlgZones = JSON.parse(JSON.stringify(c.zones || []));
  zoneDraft = null;
  if (zoneUnbind) zoneUnbind();
  zoneUnbind = camId ? Live.bind($('z-prev').querySelector('img'), camId, 'sub') : null;
  const svg = $('z-svg');
  const at = (e) => { const r = svg.getBoundingClientRect(); return [Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (e.clientY - r.top) / r.height))]; };
  const startDraft = (kind) => { zoneDraft = {kind, points: []}; $('z-done').hidden = kind === 'line'; $('z-msg').textContent = kind === 'line' ? 'Click the two ends of the line.' : 'Click the corners; click the first corner or Done to finish.'; drawZones(); };
  $('z-add-area').onclick = () => startDraft('area');
  $('z-add-line').onclick = () => startDraft('line');
  $('z-done').onclick = () => finishDraft();
  // Drag corners to adjust; click to place new corners.
  let drag = null;
  svg.onpointerdown = (e) => {
    const h = e.target.closest('[data-zi]');
    if (h && !zoneDraft) { e.preventDefault(); drag = {zi: +h.dataset.zi, pi: +h.dataset.pi}; svg.setPointerCapture(e.pointerId); return; }
    if (!zoneDraft) return;
    const p = at(e), d = zoneDraft;
    if (d.kind === 'area' && d.points.length >= 3 && Math.hypot(p[0] - d.points[0][0], p[1] - d.points[0][1]) < 0.03) return finishDraft();
    d.points.push(p);
    if (d.kind === 'line' && d.points.length === 2) return finishDraft();
    drawZones();
  };
  svg.onpointermove = (e) => { if (!drag) return; dlgZones[drag.zi].points[drag.pi] = at(e); drawZones(); };
  svg.onpointerup = svg.onpointercancel = () => { drag = null; };
  drawZones(); renderZoneList();
}
function finishDraft() {
  const d = zoneDraft; zoneDraft = null;
  $('z-done').hidden = true; $('z-msg').textContent = 'Drag the corners to adjust.';
  if (!d || d.points.length < (d.kind === 'line' ? 2 : 3)) { drawZones(); return; }
  const n = dlgZones.filter(z => z.kind === d.kind).length + 1;
  dlgZones.push({name: d.kind === 'line' ? `Line ${n}` : `Area ${n}`, kind: d.kind, points: d.points, objects: [], required: false, loiter_seconds: 0, direction: 'any'});
  drawZones(); renderZoneList();
}
function drawZones() {
  const svg = $('z-svg'); if (!svg) return;
  const pts = (p) => p.map(q => q.join(',')).join(' ');
  let h = '';
  dlgZones.forEach((z, zi) => {
    const c = ZONE_COLORS[zi % ZONE_COLORS.length];
    if (z.kind === 'line') {
      const [a, b] = z.points;
      h += `<line x1="${a[0]}" y1="${a[1]}" x2="${b[0]}" y2="${b[1]}" stroke="${c}" stroke-width="3" vector-effect="non-scaling-stroke"/>`;
    } else h += `<polygon points="${pts(z.points)}" fill="${c}" fill-opacity=".22" stroke="${c}" stroke-width="2" vector-effect="non-scaling-stroke"/>`;
  });
  if (zoneDraft && zoneDraft.points.length) {
    h += zoneDraft.kind === 'area' ? `<polyline points="${pts(zoneDraft.points)}" fill="none" stroke="#fff" stroke-dasharray="4 3" stroke-width="2" vector-effect="non-scaling-stroke"/>` : '';
  }
  svg.innerHTML = h;
  // Corner handles and labels as HTML on top (round at any picture shape).
  const box = $('z-prev');
  box.querySelectorAll('.zh, .zl').forEach(e => e.remove());
  dlgZones.forEach((z, zi) => {
    const c = ZONE_COLORS[zi % ZONE_COLORS.length];
    z.points.forEach((p, pi) => box.insertAdjacentHTML('beforeend', `<span class="zh" data-zi="${zi}" data-pi="${pi}" style="left:${p[0] * 100}%;top:${p[1] * 100}%;background:${c}">${z.kind === 'line' ? (pi ? 'B' : 'A') : ''}</span>`));
    const cx = z.points.reduce((s, p) => s + p[0], 0) / z.points.length, cy = z.points.reduce((s, p) => s + p[1], 0) / z.points.length;
    box.insertAdjacentHTML('beforeend', `<span class="zl" style="left:${cx * 100}%;top:${cy * 100}%;border-color:${c}">${esc(z.name)}</span>`);
  });
  if (zoneDraft) zoneDraft.points.forEach(p => box.insertAdjacentHTML('beforeend', `<span class="zh draft" style="left:${p[0] * 100}%;top:${p[1] * 100}%"></span>`));
  // Handles are HTML; pass their presses to the SVG handler.
  box.querySelectorAll('.zh[data-zi]').forEach(hd => hd.onpointerdown = (e) => { if (!zoneDraft) $('z-svg').onpointerdown(e); });
}
function renderZoneList() {
  const box = $('z-list'); if (!box) return;
  box.innerHTML = dlgZones.length ? '' : '<div class="hint">No zones yet: the whole picture counts.</div>';
  dlgZones.forEach((z, zi) => {
    const c = ZONE_COLORS[zi % ZONE_COLORS.length], el = document.createElement('div');
    el.className = 'zone-item';
    el.style.borderLeftColor = c;
    el.innerHTML = `<div class="row"><input type="text" class="z-name" value="${esc(z.name)}" style="width:180px"><span class="muted">${z.kind === 'line' ? 'Line' : 'Area'}</span><span class="spacer"></span><button type="button" class="small danger z-del">Delete</button></div>
      <div class="chips z-objs"><span class="muted small">Objects:</span>${ZONE_OBJECTS.map(o => `<label><input type="checkbox" value="${o}" ${z.objects.includes(o) ? 'checked' : ''}>${labelIcon(o)} ${o}</label>`).join('')}<span class="muted small">(none ticked = any)</span></div>
      ${z.kind === 'line' ? `<div class="row"><span class="muted">Report crossings</span><select class="z-dir"><option value="any">both ways</option><option value="ab">only A → B</option><option value="ba">only B → A</option></select></div>`
        : `<div class="row" style="flex-wrap:wrap;gap:14px">
            <label class="check"><input type="checkbox" class="z-req" ${z.required ? 'checked' : ''}> Only alert for objects inside this area${tip('Objects outside this area don\u2019t raise an alert. Example: draw your driveway so cars driving past on the street are ignored.')}</label>
            <label class="check">Alert if something stays longer than <input type="number" class="z-loiter" min="0" max="3600" value="${z.loiter_seconds}" style="width:80px"> seconds${tip('Loitering: someone (or something) staying in this area longer than this raises an alert. 0 = off. Example: 60 for a person hanging around your front gate.')}</label>
            <label class="check">Real length <input type="number" class="z-dist" min="0" max="1000" step="0.5" value="${z.distance_m || 0}" style="width:80px"> metres${tip('To measure speed: draw the area along a road or path, and enter how long its longer side really is (pace it out, or use a map). Alerts then say e.g. \u201cCar going 42 km/h through Road\u201d. 0 = off. It\u2019s an estimate: best when the camera looks across the road rather than along it.')}</label></div>`}`;
    const upd = () => {
      z.name = el.querySelector('.z-name').value.trim() || z.name;
      z.objects = [...el.querySelectorAll('.z-objs input:checked')].map(i => i.value);
      if (z.kind === 'line') z.direction = el.querySelector('.z-dir').value;
      else { z.required = el.querySelector('.z-req').checked; z.loiter_seconds = Math.max(0, Math.round(+el.querySelector('.z-loiter').value || 0)); z.distance_m = Math.max(0, +el.querySelector('.z-dist').value || 0); }
      drawZones();
    };
    if (z.kind === 'line') el.querySelector('.z-dir').value = z.direction || 'any';
    el.addEventListener('input', upd); el.addEventListener('change', upd);
    el.querySelector('.z-del').onclick = () => { dlgZones.splice(zi, 1); drawZones(); renderZoneList(); };
    box.appendChild(el);
  });
}

// ---------- recording profiles ----------
const PROFILE_KEYS = ['record_mode', 'schedule_enabled', 'schedule', 'record_audio', 'transcode', 'record_fps', 'segment_seconds', 'retention_days', 'max_storage_gb', 'detect',
  'sound', 'live_sound', 'talk_chime', 'onvif_events', 'onvif_motion'];
// The profile the camera being edited follows ('' = none).
let camProfile = '';
function setupProfiles(cfg) {
  const sel = $('c-prof'), msg = (t) => $('c-prof-msg').textContent = t;
  const list = () => (settings && settings.profiles) || [];
  const fill = (cur) => {
    sel.innerHTML = '<option value="">No profile (own settings)</option>' + list().map((p, i) => `<option value="${i}">${esc(p.name)}</option>`).join('');
    if (cur != null) sel.value = String(cur);
  };
  camProfile = (cfg && cfg.profile) || '';
  const linkedIdx = list().findIndex(p => p.name.toLowerCase() === camProfile.toLowerCase());
  if (linkedIdx < 0) camProfile = '';
  fill(linkedIdx >= 0 ? linkedIdx : null);
  const showLink = () => msg(camProfile ? `Follows the profile “${camProfile}”: when that profile is changed, this camera changes too. Choose “No profile” to give it its own settings.`
    : 'Profiles hold recording, motion, AI and sound settings (not the motion mask, privacy areas or camera connection settings).');
  showLink();
  // Editing a profile setting by hand gives the camera its own settings.
  const tabs = $('f-cam').querySelectorAll('[data-tab=recording], [data-tab=detection], [data-tab=audio]');
  tabs.forEach(t => t.addEventListener('input', (e) => {
    if (!camProfile || e.isTrusted === false || e.target.closest('#d-mask, .cam-only')) return;
    toast(`Own settings now — no longer following “${camProfile}” (to change it for every camera, edit the profile in Settings → Profiles).`);
    camProfile = ''; sel.value = ''; showLink();
  }));
  $('c-prof-apply').onclick = () => {
    const p = list()[+sel.value];
    if (sel.value === '' || !p) { camProfile = ''; showLink(); return; }
    applyProfile(p);
    camProfile = p.name; showLink();
    toast(`Following “${p.name}” — press Save camera to keep it.`);
  };
  sel.onchange = () => $('c-prof-apply').click();
  $('c-prof-manage').onclick = () => closeCameraDialog();
}
// ---------- Settings → Profiles ----------
// Recording, motion and AI settings shared by cameras: edit a profile and
// every camera following it changes too.
const REC_MODE_NAMES = {continuous: 'Records all the time (full quality)', continuous_plus_events: 'Records all the time + alert clips (full quality)', continuous_sub: 'Records all the time (sub-stream)', sub_plus_events: 'Sub-stream all the time + full-quality alerts', events: 'Records alerts only', off: 'Not recording'};
function renderProfiles(body) {
  const list = settings.profiles || [];
  const followers = (name) => cameraCfgs.filter(c => (c.profile || '').toLowerCase() === name.toLowerCase());
  const summary = (p) => [REC_MODE_NAMES[p.record_mode] || p.record_mode,
    p.detect.motion ? 'motion detection' : 'no motion detection',
    p.detect.ai ? 'AI: ' + p.detect.objects.join(', ') : 'no AI', p.sound && p.sound.enabled ? 'sound alerts' : ''].filter(Boolean).join(' · ');
  body.innerHTML = `<div class="card"><div class="row" style="margin-bottom:6px"><h2 style="margin:0;flex:1">Profiles</h2>
      <button type="button" id="pr-new">＋ New profile</button>
      ${cameraCfgs.length ? `<select id="pr-from" title="Start a profile from a camera's recording, motion and AI settings"><option value="">＋ New from a camera…</option>${cameraCfgs.map(c => `<option value="${esc(c.id)}">${esc(c.name)}</option>`).join('')}</select>` : ''}</div>
    <p class="muted" style="margin-top:0">A profile holds recording, motion, AI and sound settings (sound alerts, live sound, the doorbell chime, and using what the camera itself detects). Cameras following a profile change when it's edited — set it up once for all the cameras that should behave the same. Each camera keeps its own picture-based settings (ignore areas, package spot, zones). Choose which profile a camera follows in its settings (top of the window).</p>
    ${list.length ? '' : '<div class="hint">No profiles yet.</div>'}
    <div class="prof-list">${list.map((p, i) => { const f = followers(p.name); return `<div class="prof" data-i="${i}">
      <div class="row"><b style="flex:1">${esc(p.name)}</b><button type="button" class="small pr-edit">Edit</button><button type="button" class="small pr-ren">Rename</button><button type="button" class="small danger pr-del">Delete</button></div>
      <div class="muted small">${esc(summary(p))}</div>
      <div class="small" style="margin-top:4px">${f.length ? 'Followed by: ' + f.map(c => esc(c.name)).join(', ') : '<span class="muted">No camera follows it yet</span>'}</div></div>`; }).join('')}</div></div>`;
  const askName = (cur) => {
    const name = (prompt('Profile name:', cur || '') || '').trim();
    if (!name) return null;
    if (list.some(p => p.name.toLowerCase() === name.toLowerCase() && p.name !== cur)) { toast(`There's already a profile called “${name}”.`, true); return null; }
    return name;
  };
  const valuesOf = (src) => { const v = {}; for (const k of PROFILE_KEYS) v[k] = JSON.parse(JSON.stringify(src[k])); return v; };
  $('pr-new').onclick = () => { const name = askName(); if (name) openCameraDialog(null, false, {idx: -1, name, values: null}); };
  if ($('pr-from')) $('pr-from').onchange = (e) => {
    const c = cameraCfgs.find(x => x.id === e.target.value); e.target.value = '';
    if (!c) return;
    const name = askName(c.name + ' settings'); if (name) openCameraDialog(null, false, {idx: -1, name, values: valuesOf(c)});
  };
  const save = async (profiles, rename) => {
    const q = rename ? `?rename_from=${enc(rename[0])}&rename_to=${enc(rename[1])}` : '';
    try { await api('api/profiles' + q, {method: 'PUT', json: profiles}); return true; } catch (e) { toast(e.message, true); return false; }
  };
  body.querySelectorAll('.prof').forEach(el => {
    const i = +el.dataset.i, p = list[i];
    el.querySelector('.pr-edit').onclick = () => openCameraDialog(null, false, {idx: i, name: p.name, values: valuesOf(p)});
    el.querySelector('.pr-ren').onclick = async () => {
      const name = askName(p.name); if (!name || name === p.name) return;
      const profiles = list.map((x, j) => j === i ? {...x, name} : x);
      if (await save(profiles, [p.name, name])) { toast(`Renamed to “${name}”`); showSettings('profiles'); }
    };
    el.querySelector('.pr-del').onclick = async () => {
      if (!confirm(`Delete the profile “${p.name}”? Cameras following it keep their current settings.`)) return;
      if (await save(list.filter((_, j) => j !== i))) { toast(`Deleted “${p.name}”`); showSettings('profiles'); }
    };
  });
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
  set('d-trig', d.trigger); set('d-pre', d.pre_seconds); set('d-post', d.post_seconds); set('d-join', d.join_seconds ?? 30); set('d-fps', d.fps); set('d-int', d.ai_interval); set('d-parked', d.ignore_parked !== false); set('d-animals', d.ignore_animals !== false); set('d-hdcheck', d.hd_check || 'mark'); set('d-mask-ai', d.ai_ignores_mask !== false); set('d-parked-s', d.parked_seconds || 60);
  if (d.parked_objects) document.querySelectorAll('#d-parked-objs input, #d-parked-more input').forEach(i => i.checked = d.parked_objects.includes(i.value));
  // Sound and the camera's own detection (profiles from before 0.6.79 have none: kept).
  if (p.sound) {
    set('c-snd-on', p.sound.enabled); set('c-snd-sens', p.sound.sensitivity || 6);
    if ($('c-snd-sens-v')) $('c-snd-sens-v').textContent = p.sound.sensitivity || 6;
    $('c-snd-kinds').dataset.kinds = JSON.stringify(p.sound.kinds || []);
    document.querySelectorAll('#c-snd-kinds input').forEach(i => i.checked = (p.sound.kinds || []).includes(i.value));
    if ($('c-snd-box')) $('c-snd-box').hidden = !p.sound.enabled;
  }
  if (p.live_sound != null) set('c-sound', p.live_sound);
  if (p.talk_chime != null) set('c-chime', p.talk_chime);
  if (p.onvif_events != null) set('c-onvif-ev', p.onvif_events);
  if (p.onvif_motion != null) set('c-onvif-mo', p.onvif_motion);
  // Let the form update anything that depends on these (hidden sections, hints).
  for (const el of $('f-cam').querySelectorAll('[data-tab=recording] input, [data-tab=recording] select, [data-tab=detection] input, [data-tab=detection] select, [data-tab=ai] input, [data-tab=ai] select')) el.dispatchEvent(new Event('change', {bubbles: true}));
  $('d-objpx') && $('d-objpx').dispatchEvent(new Event('input', {bubbles: true}));
}
async function finishSave(body) {
  const err = (m) => { $('c-err').hidden = false; $('c-err').textContent = m; };
  const btn = $('c-save');
  const move = $('c-move-row') && !$('c-move-row').hidden && chk('c-move');
  const id = editingId, clone = cloneMode;
  const send = () => id ? api(`api/cameras/${enc(id)}?move_recordings=${!!move}`, {method: 'PUT', json: body}) : api('api/cameras', {method: 'POST', json: body});
  const after = async () => {
    await loadSettings(); await refreshStatus();
    if (page === 'settings') showSettings(section); else route();
  };
  // Moving recordings to a new folder can take a while: wait for it here.
  if (move) {
    btn.disabled = true; btn.textContent = 'Moving recordings…';
    try { await send(); closeCameraDialog(); toast('Camera updated'); await after(); }
    catch (e) { err(e.message); }
    finally { btn.disabled = false; btn.textContent = 'Save camera'; }
    return;
  }
  // Otherwise the window closes at once and the camera is saved (and its
  // video restarted) in the background. If it's refused, the window comes
  // back with what was typed and why, unless another camera was opened since.
  const gen = camDlgGen;
  closeCameraDialog();
  toast('Saving camera…');
  try {
    await send();
    toast(id ? 'Camera updated' : clone ? 'Camera cloned' : 'Camera added');
    await after();
  } catch (e) {
    // Shown next to Save (and as a message), so it's seen whichever tab is open.
    if (camDlgGen === gen && !$('cam-dialog').open) { $('cam-dialog').showModal(); err('Not saved: ' + e.message); }
    toast('Camera not saved: ' + e.message, true);
  }
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
    <label for="s-aret">Keep alerts for${tip('Alerts are what you look back at, so they can be kept longer than ordinary footage. An alert’s clip, picture and the stretch of 24/7 recording it plays from are kept this long; everything else goes after “Keep recordings for”. When space runs out, ordinary footage is deleted first. Example: recordings 7 days, alerts 60 days.')}</label><div class="row"><input type="number" id="s-aret" min="0" value="${s.alert_retention_days || 0}" style="width:110px"><span class="muted">days (0 = as long as recordings)</span></div>
    <label for="s-max">Maximum storage${tip('The most space all recordings together may use. When it’s reached, the oldest recordings are deleted to make room. Example: 1000 on a 2 TB drive leaves the other half for your own files.')}</label><div class="row"><input type="number" id="s-max" min="0" step="0.5" value="${s.max_storage_gb}" style="width:110px"><span class="muted">GB in total (0 = use the whole disk)</span></div>
    <label for="s-free">Keep free on disk${tip('GAZE never fills the disk past this: the oldest recordings are deleted to keep this much space free, so Windows and other programs keep working. Example: 20 GB.')}</label><div class="row"><input type="number" id="s-free" min="0" step="0.5" value="${s.min_free_gb}" style="width:110px"><span class="muted">GB (oldest recordings are deleted to keep this free)</span></div>
    <span></span><div><label class="check"><input type="checkbox" id="s-tlon" ${s.timelapse !== false ? 'checked' : ''}> Time-lapse previews</label>
      <div class="hint">The pictures shown while dragging along a timeline. Off: dragging moves the video itself, and nothing is kept or made.</div></div>
    <label for="s-tlsec">Time-lapse pictures${tip('How often a small picture is kept for the time-lapse, and how big. More often is smoother when you drag along the timeline; sharper is clearer. Example: one every 10 seconds at standard size uses about 35 MB a day per camera.')}</label><div><div class="row">
        <select id="s-tlsec">${[...new Set([2, 5, 10, 20, 30, 60, s.timelapse_seconds || 10])].sort((a, b) => a - b).map(n => `<option value="${n}" ${n === (s.timelapse_seconds || 10) ? 'selected' : ''}>One every ${n} seconds${n === 2 ? ' — smoothest' : n === 10 ? ' — standard' : n === 60 ? ' — lightest' : ''}</option>`).join('')}</select>
        <select id="s-tlw">${[[256, 'Standard (256 × 144)'], [384, 'Sharp (384 × 216)'], [512, 'Sharpest (512 × 288)']].map(([w, t]) => `<option value="${w}" ${w === (s.timelapse_width || 256) ? 'selected' : ''}>${t}</option>`).join('')}</select>
        <span class="muted" id="s-tlsec-info"></span></div>
      <div class="hint">Kept as the camera records (from what motion detection already looks at), so more and sharper pictures cost little — mainly some disk space. <b>A change applies to new footage from now on</b>; the time-lapse already made stays as it is (you can choose to remake it when you save).</div>
      <button type="button" class="small" id="s-tlremake" style="margin-top:8px">🖼️ Remake time-lapse with these settings…</button></div>
  </div><div class="actions"><button class="primary">Save</button><a class="btn" href="#system">See space &amp; days of recording →</a></div></form>
  <div class="card"><h2>Where each camera saves</h2>
    <table><thead><tr><th>Camera</th><th>Folder</th><th class="hide-sm">Stored</th><th></th></tr></thead><tbody>
    ${cameraCfgs.map(c => `<tr><td><b>${esc(c.name)}</b></td><td><code>${esc(used(c.id) ? used(c.id).dir : (c.storage_dir || s.dir) + '/' + c.id)}</code>
      <div class="sub">${c.storage_dir ? 'Own folder' : 'Default folder'}</div></td><td class="hide-sm">${used(c.id) ? fmtBytes(used(c.id).used_bytes) : '–'}</td>
      <td style="text-align:right"><div class="row" style="justify-content:flex-end"><button type="button" class="small" data-cam="${esc(c.id)}">Change…</button>${c.storage_dir ? `<button type="button" class="small" data-reset="${esc(c.id)}">Use default</button>` : ''}</div></td></tr>`).join('')}
    </tbody></table>
    <div class="hint">Changing a folder can move the camera's existing recordings too. Moving to another drive copies the files, which can take a while; the camera pauses recording during the move.</div></div>`;
  $('s-dir').oninput = () => $('s-move-row').hidden = val('s-dir').trim() === s.dir;
  const tlInfo = () => {
    const n = +val('s-tlsec') || 10, w = +val('s-tlw') || 256;
    // About 4 KB a picture at 256 wide, growing with the area.
    const perDay = 86400 / n * 4000 * (w / 256) ** 2;
    $('s-tlsec-info').textContent = `${Math.ceil(300 / n)} pictures per 5 minutes · about ${fmtBytes(perDay)} a day per camera`;
  };
  $('s-tlsec').onchange = $('s-tlw').onchange = tlInfo; tlInfo();
  const tlOn = () => ['s-tlsec', 's-tlw', 's-tlremake'].forEach(x => $(x).disabled = !chk('s-tlon'));
  $('s-tlon').onchange = tlOn; tlOn();
  // New time-lapse settings: from now on only (what's made stays), or
  // remake what's already made too. Resolves 'new', 'remake' or null.
  const tlChoice = (remakeFirst) => new Promise(done => {
    const dlg = document.createElement('dialog'); dlg.className = 'wn-dlg';
    dlg.innerHTML = `<div class="head"><h2>🖼️ New time-lapse settings</h2></div><div class="body">
      <label class="tl-opt"><input type="radio" name="tlc" value="new" ${remakeFirst ? '' : 'checked'}><span><b>From now on only</b><br><span class="muted">New footage gets time-lapse pictures at the new settings. What's already made stays as it is — nothing is rebuilt and the computer isn't kept busy.</span></span></label>
      <label class="tl-opt"><input type="radio" name="tlc" value="remake" ${remakeFirst ? 'checked' : ''}><span><b>Also remake the older time-lapse</b><br><span class="muted">All time-lapse pictures are thrown away and made again at the new settings: the last 24 hours first, older days when you open them. Footage older than a few hours is read again from the recordings, which can keep the computer busy for a while (System → Tasks).</span></span></label>
      <div class="actions"><button type="button" data-v="">Cancel</button><button type="button" class="primary" data-v="ok">Save</button></div></div>`;
    document.body.appendChild(dlg); dlg.showModal();
    dlg.querySelectorAll('[data-v]').forEach(b => b.onclick = () => { const v = b.dataset.v && dlg.querySelector('input[name=tlc]:checked').value; dlg.close(); dlg.remove(); done(v || null); });
    dlg.addEventListener('cancel', () => { dlg.remove(); done(null); });
  });
  const remakeTimelapse = async () => { try { const r = await api('api/maintenance/previews', {method: 'POST'}); toast(r.message || 'Remaking the time-lapse'); } catch (e) { toast(e.message, true); } };
  let tlRemake = false;
  $('s-tlremake').onclick = async () => {
    const v = await tlChoice(true); if (!v) return;
    tlRemake = v === 'remake'; tlAsked = true; $('f-storage').requestSubmit();
  };
  let tlAsked = false;
  $('s-browse').onclick = async () => { const p = await pickFolder(val('s-dir'), 'Default recordings folder'); if (p) { $('s-dir').value = p; $('s-dir').oninput(); } };
  $('f-storage').onsubmit = async (e) => {
    e.preventDefault();
    // Time-lapse settings changed: say what happens to what's already made.
    const tlChanged = chk('s-tlon') && ((+val('s-tlsec') || 10) !== (s.timelapse_seconds || 10) || (+val('s-tlw') || 256) !== (s.timelapse_width || 256));
    if (tlChanged && !tlAsked) { const v = await tlChoice(false); if (!v) return; tlRemake = v === 'remake'; }
    const remake = tlRemake; tlAsked = false; tlRemake = false;
    const moving = !$('s-move-row').hidden && chk('s-move');
    if (moving) toast('Saving and moving recordings… this can take a while for large folders.');
    await saveSettings({move_recordings: moving, storage: {dir: val('s-dir').trim(), segment_seconds: num('s-seg'), retention_days: Math.round(num('s-ret')), alert_retention_days: Math.max(0, Math.round(num('s-aret')) || 0), max_storage_gb: num('s-max'), min_free_gb: num('s-free'), timelapse: chk('s-tlon'), timelapse_seconds: Math.min(120, Math.max(1, Math.round(num('s-tlsec')) || 10)), timelapse_width: +val('s-tlw') || 256}});
    if (remake) await remakeTimelapse();
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
  const corners = SPOTS;
  const custom = !CLOCK_FORMATS.includes(d.clock_format);
  body.innerHTML = `<form class="card" id="f-display"><h2>Live view</h2><div class="form">
    <label for="l-fps">Smoothness in the grid${tip('Pictures per second (fps) for each camera in the Live grid. Higher is smoother but uses more of the computer and network. 5 is plenty for watching several cameras at once.')}</label><div class="row"><input type="number" id="l-fps" min="1" max="30" value="${l.fps}" style="width:110px"><span class="muted">pictures per second</span></div>
    <label for="l-width">Picture size in the grid${tip('How wide (in pixels) each camera’s picture in the Live grid is made. Bigger is sharper on a large screen, but uses more of the computer and network. 640 suits most screens.')}</label><div><select id="l-width">${[[480, '480 (low)'], [640, '640'], [960, '960'], [1280, '1280 (HD)'], [1920, '1920'], [0, 'Native']].map(([v, t]) => `<option value="${v}" ${v === l.width ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    <label for="l-hd">Smoothness when opened${tip('Pictures per second when you open a camera or watch it full screen (full quality). Example: 15 looks smooth; 25–30 is as smooth as the camera.')}</label><div class="row"><input type="number" id="l-hd" min="1" max="30" value="${l.hd_fps}" style="width:110px"><span class="muted">pictures per second</span></div>
    <label for="l-q">Picture quality${tip('How much the live pictures are compressed to send to your browser. Higher looks cleaner but uses more network — choose Low for watching over mobile data. Recordings are not affected.')}</label><div><select id="l-q">${[[3, 'Very high'], [6, 'High'], [10, 'Medium'], [16, 'Low (saves bandwidth)']].map(([v, t]) => `<option value="${v}" ${v === l.quality ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    <label for="l-ds">Data saver (this browser)${tip('For phones on mobile data: smaller live pictures, fewer per second, and lighter video when playing recordings. Set separately on each phone or computer.')}</label><div><select id="l-ds">
        <option value="auto" ${dsMode === 'auto' ? 'selected' : ''}>Automatic — on for phones (mobile data), off on computers</option>
        <option value="on" ${dsMode === 'on' ? 'selected' : ''}>Always on</option><option value="off" ${dsMode === 'off' ? 'selected' : ''}>Off</option></select>
      <div class="hint">Smaller live pictures, fewer frames per second and light video for recordings. Saved in this browser only; now ${dataSaver ? '<b>on</b>' : 'off'} here.</div></div>
    <label for="l-video">Live pictures (this browser)${tip('Smooth video plays the camera\u2019s own video at its full frame rate, with half a second of buffer to smooth out Wi-Fi hiccups \u2014 and uses much less of this computer. Pictures is the older way: a series of still images, a few per second. Data saver always uses pictures.')}</label><div><select id="l-video">
        <option value="1" ${store.get('live-video', true) !== false ? 'selected' : ''}>Smooth video (recommended)</option><option value="0" ${store.get('live-video', true) === false ? 'selected' : ''}>Pictures</option></select></div>
    <span></span><div><label class="check"><input type="checkbox" id="l-pre" ${l.preload_hd_streams ? 'checked' : ''}> Keep HD streams ready in the background${tip('Off (recommended): a camera’s full-quality picture is only prepared while you have it open — for a second you see the lighter picture. On: opening a camera shows full quality instantly, but every camera’s full-quality video is processed all the time, which uses a lot of the computer.')}</label>
      <div class="hint">Opens cameras in full quality instantly, at the cost of much more work for the computer.</div></div>
    <h3>Overlays on live video</h3>
    <label>Full-screen button${tip('Where the ⛶ full-screen button sits on each camera’s picture. Tick more than one to have it in several corners.')}</label><div class="chips" id="g-fs">${corners.map(([k, t]) => `<label><input type="checkbox" value="${k}" ${d.fullscreen_corners.includes(k) ? 'checked' : ''}>${t}</label>`).join('')}</div>
    <label>Preview</label>
    <div class="surface" id="go-prev" style="aspect-ratio:16/9;max-width:560px">${status.cameras[0] ? `<img alt="" style="object-fit:fill" src="live/${enc(status.cameras[0].id)}/frame.jpg">` : ''}</div>
    <span></span><div class="hint" style="margin-top:-4px">For every camera without its own on-screen display (camera settings → On-screen display).</div>
    ${osdItemsHtml('go', d.osd)}
    <label for="g-fmt">Date &amp; time format</label><div><select id="g-fmt">${CLOCK_FORMATS.map(f => `<option ${f === d.clock_format ? 'selected' : ''}>${f}</option>`).join('')}<option value="custom" ${custom ? 'selected' : ''}>Custom…</option></select>
      <input type="text" id="g-custom" value="${esc(d.clock_format)}" style="margin-top:6px" ${custom ? '' : 'hidden'}>
      <div class="hint">Preview: <b id="g-prev"></b><br>Codes: YYYY YY · MMMM MMM MM M · DD D · dddd ddd · HH H (24 h) hh h (12 h) · mm ss · A a (AM/PM)</div></div>
  </div><div class="actions"><button class="primary">Save</button></div></form>`;
  $('l-video').onchange = () => { store.set('live-video', val('l-video') === '1'); Live.noMse.clear(); toast('Saved for this browser — reloading the live view'); setTimeout(() => location.reload(), 600); };
  const fmt = () => val('g-fmt') === 'custom' ? val('g-custom') : val('g-fmt');
  const prev = () => $('g-prev').textContent = fmtDate(new Date(), fmt());
  $('g-fmt').onchange = () => { $('g-custom').hidden = val('g-fmt') !== 'custom'; prev(); };
  $('g-custom').oninput = prev; prev();
  // Live preview of the layout; drag items on it.
  const gp = $('go-prev');
  const redrawG = () => {
    gp.querySelectorAll('.ov').forEach(o => o.remove());
    const c0 = status.cameras[0] || {id: '', name: 'Camera', enabled: true};
    const fake = {...c0, sound: false, osd: {...readOsdItems('go'), custom: true, clock_format: fmt()}};
    gp.insertAdjacentHTML('beforeend', overlayHtml(fake, 'sub', true));
    gp.querySelectorAll('.ov-alert').forEach(a => a.innerHTML = `<span class="alertbadge">${labelIcon('person')} person</span>`);
    gp.querySelectorAll('.ov-fps').forEach(a => a.textContent = '15.0 fps');
    gp.querySelectorAll('.ov-res').forEach(a => a.textContent = '640×360 H264');
    gp.querySelectorAll('[data-osd]').forEach(el => el.classList.add('osd-drag'));
  };
  $('f-display').addEventListener('input', redrawG);
  wireOsdDrag(gp, 'go', () => true, redrawG);
  redrawG();
  $('l-ds').onchange = () => { setDataSaver(val('l-ds')); toast('Data saver: ' + $('l-ds').selectedOptions[0].textContent.split(' —')[0]); };
  $('f-display').onsubmit = (e) => {
    e.preventDefault();
    saveSettings({
      live: {fps: num('l-fps'), width: num('l-width'), quality: num('l-q'), hd_fps: num('l-hd'), preload_hd_streams: chk('l-pre')},
      display: {fullscreen_corners: [...document.querySelectorAll('#g-fs input:checked')].map(i => i.value), clock_format: fmt() || 'DD/MM/YYYY HH:mm:ss',
        osd: {...readOsdItems('go'), custom: false, clock_format: ''}},
    }, 'Saved').then(() => renderDash());
  };
}

// ---------- AI ----------
const PROVIDERS = [
  ['none', 'Off', 'No object recognition (motion detection still works).'],
  ['builtin', 'Built-in (this computer)', 'Free and private. Runs a YOLOX model on this computer\'s graphics card or processor. The model (4 MB for the fastest, up to 220 MB for the most accurate) downloads automatically the first time.'],
];
function renderAi(body) {
  const a = settings.ai, st = status && status.ai;
  body.innerHTML = `<div class="card ai-explain"><h2>How GAZE's AI works</h2>
    <ol>
      <li><b>Motion detection</b> (always local, very light) notices that something changed in the picture.</li>
      <li><b>Object recognition</b> then checks that picture for people, vehicles and animals. With the <b>built-in</b> choice this is a <b>YOLOX</b> model (by Megvii, open source,
        trained on the COCO set of 80 everyday objects) running on this computer's graphics card or processor — nothing leaves your home. Five sizes from <i>Nano</i> (fast, for small PCs) to <i>Large</i> (most accurate, for powerful PCs).</li>
      <li><b>Double-check</b> (optional): a bigger YOLOX model looks again at pictures that would raise an alert, to cut false alarms.</li>
      <li><b>Night boost</b>: dark and infrared pictures are brightened before recognition.</li>
    </ol>
    <form class="card" id="f-ai"><h2>Object recognition (AI)</h2>
    <p class="muted" style="margin-top:0">When a camera sees motion, a picture is checked for people, vehicles and animals. Turn it on per camera under <i>Motion &amp; AI</i>.</p>
    <div class="radio-cards">${PROVIDERS.map(([k, t, dsc]) => `<label><input type="radio" name="ai-p" value="${k}" ${a.provider === k ? 'checked' : ''}><b>${t}</b><small>${dsc}</small></label>`).join('')}</div>
    <div class="form" style="margin-top:14px">
      <label for="ai-device">Run AI on${tip('The graphics card (GPU) is much faster and leaves the processor free for video. It works with any recent NVIDIA, AMD or Intel graphics on Windows (DirectX 12). The GPU add-on (about 25 MB) downloads by itself the first time; until then, or if the graphics card can’t be used, the processor takes over.')}</label><div><select id="ai-device">
        <option value="gpu" ${a.device !== 'cpu' ? 'selected' : ''}>Graphics card (GPU) — recommended</option>
        <option value="cpu" ${a.device === 'cpu' ? 'selected' : ''}>Processor (CPU)</option></select>
        <div class="hint">For object recognition and faces &amp; plates.</div>
        <div class="hint" id="ai-device-st"></div></div>
      <label class="p-builtin" for="ai-model">AI model${tip('Bigger models recognise more, from further away, but take longer for each picture. Small or Medium suits most computers; Nano for an older PC with many cameras.')}</label><div class="p-builtin"><select id="ai-model">
        <option value="yolox_nano" ${a.builtin_model === 'yolox_nano' ? 'selected' : ''}>Nano — fastest (≈30 ms), good for small PCs</option>
        <option value="yolox_tiny" ${a.builtin_model === 'yolox_tiny' ? 'selected' : ''}>Tiny — balanced (≈90 ms), recommended</option>
        <option value="yolox_s" ${a.builtin_model === 'yolox_s' ? 'selected' : ''}>Small — more accurate (≈350 ms)</option>
        <option value="yolox_m" ${a.builtin_model === 'yolox_m' ? 'selected' : ''}>Medium — for bigger PCs (≈0.9 s, 100 MB)</option>
        <option value="yolox_l" ${a.builtin_model === 'yolox_l' ? 'selected' : ''}>Large — most accurate, powerful PCs only (≈2 s, 220 MB)</option>
        <option value="custom" ${a.builtin_model === 'custom' ? 'selected' : ''}>Your own model (.onnx file)…</option></select>
        <div class="hint">Times are per picture on a 4-core PC; the AI only runs while there is motion.</div>
        <div id="ai-custom" class="ai-custom" ${a.builtin_model === 'custom' ? '' : 'hidden'}>
          <label for="ai-cpath">Model file${tip('An object detection model in ONNX format: YOLOv8 or YOLO11 exported by Ultralytics (yolo export format=onnx), or YOLOX. Train one on your own pictures to recognise what matters to you \u2014 a fox, your own dog, a delivery van, the gate open or closed.')}</label><input type="text" id="ai-cpath" value="${esc(a.custom_model || '')}" placeholder="e.g. C:\\GAZE\\models\\my-yard.onnx">
          <label for="ai-csize">Picture size${tip('The size the model was trained and exported at (imgsz). Usually 640.')}</label><select id="ai-csize">${[320, 416, 480, 512, 608, 640, 736, 800, 960, 1024, 1280].map(n => `<option ${n === (a.custom_size || 640) ? 'selected' : ''}>${n}</option>`).join('')}</select>
          <label for="ai-clabels">Class names${tip('What the model recognises, one per line, in the model\u2019s own order (the names list from your training data.yaml). Leave empty for a model trained on the usual 80 COCO classes (person, car, dog\u2026). Your classes then appear in each camera\u2019s \u201cMore things to watch for\u201d.')}</label><textarea id="ai-clabels" rows="4" placeholder="fox&#10;hedgehog&#10;delivery van">${esc((a.custom_labels || []).join('\n'))}</textarea>
        </div></div>
      <label class="p-builtin" for="ai-verify">Double-check alerts${tip('When the AI thinks it sees something that would raise an alert, a bigger model looks at the same picture and has the final say: fewer false alerts and fewer mix-ups (a cat called a person). It only runs on those few pictures, so it suits normal PCs too.')}</label><div class="p-builtin"><select id="ai-verify">
        <option value="">Off</option>
        ${[['yolox_s', 'Small model (≈350 ms)'], ['yolox_m', 'Medium model (≈0.9 s) — recommended'], ['yolox_l', 'Large model (≈2 s)']].map(([k, t]) => `<option value="${k}" ${a.verify_model === k ? 'selected' : ''}>${t}</option>`).join('')}</select>
        <div class="hint">A bigger model confirms each find before it alerts: fewer false alerts.</div></div>
      <span class="p-builtin"></span><div class="p-builtin"><label class="check"><input type="checkbox" id="ai-search" ${a.search ? 'checked' : ''}> Search alerts by description${tip('Type what you’re looking for on the Alerts page — “a red car”, “someone carrying a box”, “a dog at night”, “a man in a white shirt” — and GAZE finds the alerts whose picture looks like it, without anyone having labelled them. Uses CLIP (by OpenAI, open source), which downloads once (420 MB) and then looks at each alert picture on this computer (about a second each; older alerts are done in the background when the computer is quiet).')}</label>
        <div class="hint" id="ai-search-st">Find alerts by what's in them: “a red car”, “someone with a ladder”.</div></div>
      <span class="p-builtin"></span><div class="p-builtin"><label class="check"><input type="checkbox" id="ai-night" ${a.night_boost !== false ? 'checked' : ''}> Night boost${tip('Dark and infrared (black & white) pictures are brightened and their contrast stretched before the AI looks at them, so people and animals are recognised better at night. Recordings aren’t changed.')}</label>
        <div class="hint">Better recognition in dark and infrared pictures.</div></div>
    </div>
    <div class="hint" id="ai-status" style="margin-top:10px"></div>
    <div class="actions"><button class="primary">Save</button>
      <select id="ai-cam">${(status ? status.cameras : []).map(c => `<option value="${esc(c.id)}">${esc(c.name)}</option>`).join('')}</select>
      <button type="button" id="ai-test">Test on this camera</button></div>
    <div id="ai-result" style="margin-top:12px"></div></form>`;
  const sync = () => {
    const p = document.querySelector('input[name=ai-p]:checked').value;
    document.querySelectorAll('.p-builtin').forEach(e => e.hidden = p !== 'builtin');
    $('ai-test').disabled = p === 'none' || !$('ai-cam').value;
  };
  document.querySelectorAll('input[name=ai-p]').forEach(r => r.onchange = sync); sync();
  $('ai-model').onchange = () => { $('ai-custom').hidden = val('ai-model') !== 'custom'; };
  api('api/search/status').then(s => { const el = $('ai-search-st'); if (el && a.search) el.textContent = s.downloading ? 'Downloading the search model (420 MB)…' : s.error ? 'Problem: ' + s.error : s.waiting ? `Looking at alert pictures: ${s.waiting} to go.` : s.ready ? 'Ready — search on the Alerts page.' : 'Starting…'; }).catch(() => {});
  const form = () => ({...settings.ai, search: chk('ai-search'), provider: document.querySelector('input[name=ai-p]:checked').value, builtin_model: val('ai-model'),
    custom_model: val('ai-cpath').trim(), custom_size: +val('ai-csize') || 640, custom_labels: val('ai-clabels').split('\n').map(x => x.trim()).filter(Boolean),
    verify_model: val('ai-verify'), night_boost: chk('ai-night'), device: val('ai-device')});
  const showStatus = () => {
    if ($('ai-device-st') && status) $('ai-device-st').innerHTML = `Running on: <b>${esc(status.ai_device || '')}</b>`;
    const s = status && status.ai; if (!s) return;
    $('ai-status').innerHTML = s.provider === 'none' ? '' : `Current: <b>${esc(s.provider)}</b> — ${s.downloading ? `downloading model ${Math.round(s.download_percent || 0)}%` : s.ready ? '<span class="ok">ready</span>' : 'loading'}` +
      ` · ${s.requests} checks${s.failures ? `, ${s.failures} failed` : ''}${s.last_ms != null ? ` · last took ${s.last_ms} ms` : ''}${s.error ? `<br><span class="err">${esc(s.error)}</span>` : ''}`;
  };
  showStatus();
  $('f-ai').onsubmit = async (e) => { e.preventDefault(); await saveSettings({ai: form()}, 'AI settings saved'); showStatus(); };
  $('ai-test').onclick = async () => {
    const btn = $('ai-test'), out = $('ai-result');
    btn.disabled = true; btn.textContent = 'Checking…'; out.innerHTML = '<span class="muted">Sending the camera\'s current picture to the AI… (the built-in model may need to download first)</span>';
    try {
      const r = await api('api/ai/test', {method: 'POST', json: {camera: val('ai-cam'), ai: form()}});
      const watched = new Set(r.watched || []);
      const boxes = (ds, hd) => (ds || []).map(d => `<div class="box ${hd ? (d.ok ? 'hd-ok' : 'hd-no') : watched.has(d.label) ? '' : 'unwatched'}" style="left:${d.x * 100}%;top:${d.y * 100}%;width:${d.w * 100}%;height:${d.h * 100}%"><span>${esc(d.label)}${hd ? '' : ' ' + Math.round(d.score * 100) + '%'}</span></div>`).join('');
      const modelName = (k) => ({yolox_nano: 'Nano', yolox_tiny: 'Tiny', yolox_s: 'Small', yolox_m: 'Medium', yolox_l: 'Large', custom: 'your own'})[k] || k;
      // Each step GAZE takes before an alert, on the same moment: the quick
      // model on the sub-stream picture, the bigger model double-checking
      // it, then what was found looked at again in a full-quality picture.
      const stages = r.stages || [], img0 = stages.length && stages[0].image;
      const card = (st, i) => {
        const title = st.kind === 'fast' ? `1. Quick check — ${esc(modelName(st.model))} model`
          : st.kind === 'verify' ? `${i + 1}. Double-check — ${esc(modelName(st.model))} model`
          : `${i + 1}. HD check — full-quality picture`;
        const note = st.kind === 'fast' ? (stages.length > 1 ? 'Runs on every look while there is motion.' : 'Its answer is used for alerts.')
          : st.kind === 'verify' ? 'Looks again when the quick check finds something; its answer is used.'
          : st.setting === 'off' ? 'Off for this camera (camera → AI → Check in HD) — shown here so you can see what it would do.' : st.setting === 'drop' ? 'Finds not seen in HD are left out of the alert.' : 'Finds not seen in HD are marked as not confirmed.';
        const im = st.kind === 'hd' ? st.image : img0;
        const n = (st.detections || []).length;
        return `<figure class="ai-stage"><figcaption><b>${title}</b> <span class="muted small">${st.ms != null ? st.ms + ' ms' : ''}</span>
          <div class="small ${st.error ? 'err' : ''}">${st.error ? esc(st.error) : st.kind === 'hd' ? (n ? (st.detections.map(d => esc(d.label)).join(' · ')) : 'Nothing to check') : n ? n + ' found' : 'Nothing recognized'}</div><div class="muted small">${note}</div></figcaption>
          ${im ? `<div class="ai-test"><img src="${im}" alt="">${boxes(st.detections, st.kind === 'hd')}</div>` : ''}</figure>`;
      };
      out.innerHTML = (r.ok ? `<div class="ok" style="margin-bottom:6px">✓ ${r.detections.length ? r.detections.length + ' object(s) found' : 'Working — nothing recognized in this picture'} (${r.ms} ms in all)</div>`
        : `<div class="err" style="margin-bottom:6px">✗ ${esc(r.error)}</div>`)
        + (stages.length ? `<div class="ai-stages">${stages.map(card).join('')}</div>${stages.some(st => st.kind !== 'hd' && (st.detections || []).some(d => !watched.has(d.label))) ? '<div class="hint">Faint boxes: things this camera isn’t watching for (only watched things are checked in HD).</div>' : ''}`
          : r.image ? `<div class="ai-test"><img src="${r.image}" alt=""></div>` : '');
    } catch (e) { out.innerHTML = `<div class="err">${esc(e.message)}</div>`; }
    finally { btn.disabled = false; btn.textContent = 'Test on this camera'; }
  };
}

// ---------- GPU ----------
const DEC_LABELS = {none: 'Off (CPU)', auto: 'Automatic (use any GPU found)', cuda: 'NVIDIA (CUDA/NVDEC)', qsv: 'Intel Quick Sync (QSV)',
  vaapi: 'VAAPI – Intel/AMD on Linux', videotoolbox: 'Apple VideoToolbox (macOS)', d3d11va: 'Direct3D 11 (Windows, any GPU)', dxva2: 'DXVA2 (older Windows)'};
const ENC_LABELS = {auto: 'Automatic', libx264: 'CPU (libx264)', h264_nvenc: 'NVIDIA NVENC', h264_qsv: 'Intel Quick Sync',
  h264_vaapi: 'VAAPI (Linux Intel/AMD)', h264_videotoolbox: 'Apple VideoToolbox', h264_amf: 'AMD AMF (Windows)', h264_mf: 'Windows Media Foundation'};
// Live card: which GPU, what it's used for, how busy it is.
let gpuTimer = null;
async function paintGpuNow() {
  const el = $('gpu-now');
  if (!el || page !== 'settings' || section !== 'hardware') { clearInterval(gpuTimer); gpuTimer = null; return; }
  const m = await api('api/monitor?range=300').catch(() => null);
  if (!m) return;
  const c = m.current, s = c.sample, h = settings.hardware;
  const using = h.hwaccel === 'none' ? 'not used — video is decoded on the CPU'
    : `decoding ${h.gpu_substreams ? 'all streams' : 'full-quality streams you watch'} with ${esc(DEC_LABELS[h.hwaccel] || h.hwaccel)}` + (h.encoder && h.encoder !== 'auto' ? `, encoding with ${esc(h.encoder)}` : '');
  const fallback = (status ? status.cameras : []).filter(x => (x.recorder || x.buffer || {}).gpu_fallback).map(x => x.name);
  el.innerHTML = s.gpu == null
    ? `<b>${esc(c.gpu_name || 'No GPU readings')}</b><div class="hint">GAZE can't read GPU usage on this computer (NVIDIA needs nvidia-smi; AMD on Linux and any GPU on Windows are read automatically).</div><div>GPU acceleration: ${using}.</div>`
    : `<div class="gpu-row"><div><b>${esc(c.gpu_name || 'GPU')}</b>${c.gpu_temp != null ? ` · ${c.gpu_temp.toFixed(0)} °C` : ''}<div class="muted small">read via ${esc(c.gpu_source || '')}</div></div>
       <div class="gpu-pct"><span>${s.gpu.toFixed(0)}%</span><div class="bar"><div style="width:${Math.min(100, s.gpu).toFixed(0)}%"></div></div><small>in use now${s.gpu_mem != null ? ` · memory ${s.gpu_mem.toFixed(0)}%` : ''}</small></div></div>
       <div style="margin-top:8px">GPU acceleration: ${using}.</div>
       ${fallback.length ? `<div class="warnc">Using the CPU instead (the GPU failed) for: ${esc(fallback.join(', '))}</div>` : ''}`;
}
async function renderHardware(body) {
  clearInterval(gpuTimer); gpuTimer = setInterval(paintGpuNow, 3000); setTimeout(paintGpuNow, 50);
  if (!sysinfo) sysinfo = await api('api/system').catch(() => null);
  const caps = sysinfo ? sysinfo.capabilities : {hwaccels: [], encoders: []}, h = settings.hardware;
  body.innerHTML = `<div class="card gpu-now"><h2>Your graphics card</h2><div id="gpu-now" class="muted">Reading…</div></div>
    <form class="card" id="f-hw"><h2>GPU acceleration</h2>
    <p class="muted" style="margin-top:0">The GPU decodes video for the live view and motion detection, and encodes when a camera converts or lowers the frame rate of recordings. Normal recording copies the camera's video and needs no GPU. If the GPU fails, the NVR switches to the CPU automatically.</p>
    <div class="form">
      <span></span><div><label class="check"><input type="checkbox" id="h-auto" ${h.auto !== false ? 'checked' : ''}> Choose automatically (recommended)${tip('At every start GAZE tries the graphics card’s video features and uses the first that works, so the processor stays free. Untick only if you want to choose below yourself.')}</label>
        <div class="hint">GAZE finds the fastest way this computer can handle video.</div></div>
      <label for="h-dec">Video decoding${tip('Turning the camera’s compressed video into pictures (for live view and motion detection). A graphics card (NVIDIA, Intel Quick Sync, AMD) does this far more efficiently than the processor. Use Test to check it works.')}</label><div><select id="h-dec">${Object.entries(DEC_LABELS).map(([k, v]) => { const ok = k === 'none' || k === 'auto' || caps.hwaccels.includes(k); return `<option value="${k}" ${ok ? '' : 'disabled'}>${v}${ok ? '' : ' – not in this ffmpeg'}</option>`; }).join('')}</select>
        <div class="hint">${caps.hwaccels.length ? 'Your ffmpeg supports: ' + esc(caps.hwaccels.join(', ')) + '. Use Test to check the hardware is really there.' : 'Your ffmpeg reports no GPU support.'}</div></div>
      <label for="h-enc">Video encoder${tip('Making video in the common H.264 format: used when recordings are converted, slowed to a lower frame rate, exported or played on a browser that can’t play the camera’s own format. “Automatic” picks the one that matches the decoder.')}</label><div><select id="h-enc">${Object.entries(ENC_LABELS).map(([k, v]) => { const ok = k === 'auto' || caps.encoders.includes(k); return `<option value="${k}" ${ok ? '' : 'disabled'}>${v}${ok ? '' : ' – not in this ffmpeg'}</option>`; }).join('')}</select>
        <div class="hint">“Automatic” picks the encoder that matches the decoder.</div></div>
      <span></span><div><label class="check"><input type="checkbox" id="h-sub" ${h.gpu_substreams ? 'checked' : ''}> Also use the graphics card for sub-streams${tip('Off (recommended): the graphics card only handles full-quality video you’re watching. The small sub-streams (grid, motion detection) are actually cheaper on the processor than sending them to the graphics card and back.')}</label></div>
      <span></span><div><label class="check"><input type="checkbox" id="h-bal" ${h.balance !== false ? 'checked' : ''}> Share the work between graphics card and processor${tip('When the graphics card is busy (85% or more, e.g. a game) and the processor has room, new live views, conversions and exports run on the processor instead, and motion detection moves to whichever has room — and back when the graphics card is free again.')}</label>
        <div class="hint">Keeps GAZE smooth while you game or edit video on this computer.</div></div>
      <label for="h-dev" class="vaapi">Graphics card device</label><div class="vaapi"><input type="text" id="h-dev" value="${esc(h.device)}" placeholder="/dev/dri/renderD128"><div class="hint">Linux only, for computers with more than one graphics card. Leave empty to use the default.</div></div>
    </div>
    <div class="actions"><button class="primary">Save</button><button type="button" id="h-test">Test selected options</button></div>
    <div id="h-result" style="margin-top:12px"></div></form>`;
  $('h-dec').value = h.hwaccel; $('h-enc').value = h.encoder;
  const sync = () => document.querySelectorAll('.vaapi').forEach(e => e.hidden = !(val('h-dec') === 'vaapi' || val('h-enc') === 'h264_vaapi'));
  $('h-dec').onchange = $('h-enc').onchange = sync; sync();
  const form = () => ({auto: chk('h-auto'), hwaccel: val('h-dec'), encoder: val('h-enc'), device: val('h-dev').trim(), gpu_substreams: chk('h-sub'), balance: chk('h-bal')});
  // Choosing by hand switches "automatically" off.
  ['h-dec', 'h-enc', 'h-sub'].forEach(id => $(id).addEventListener('change', () => { $('h-auto').checked = false; }));
  $('f-hw').onsubmit = (e) => {
    e.preventDefault();
    const h = form(), same = Object.keys(h).every(k => JSON.stringify(h[k]) === JSON.stringify(settings.hardware[k]));
    saveSettings({hardware: h}, same ? 'Saved' : 'Saved. Cameras are restarting with the new settings.');
  };
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
  let method = !c.remote ? 'folder' : /^ftp:\/\//.test(c.remote.trim()) ? 'ftp' : (/^([a-zA-Z]:[\\/]|\\\\|\/)/.test(c.remote.trim()) ? 'folder' : 'rclone');
  const ftp = (() => { const m = /^ftp:\/\/([^/:]+)(?::(\d+))?\/?(.*)$/.exec(c.remote.trim()); return m ? {host: m[1], port: m[2] || '21', folder: m[3]} : {host: '', port: '21', folder: 'GAZE'}; })();
  body.innerHTML = `<form class="card" id="f-cloud"><h2>Backup — cloud or NAS</h2>
    <p class="muted" style="margin-top:0">Keeps a copy of your alerts (or all recordings) in the cloud or on a NAS, so they're safe even if this computer is stolen or breaks. Everything also stays on this computer.</p>
    <div class="cl-status" id="cl-status"></div>
    <h3>1. Where to back up</h3>
    <div class="radio-cards cl-methods">
      <label><input type="radio" name="cl-m" value="folder" ${method === 'folder' ? 'checked' : ''}><b>A folder or NAS shared folder — easiest</b>
        <small>A NAS's shared folder (\\\\NAS\\cameras), another drive, or the folder of Google Drive for desktop, OneDrive or Dropbox (that app uploads what GAZE copies there). Nothing else to install.</small></label>
      <label><input type="radio" name="cl-m" value="ftp" ${method === 'ftp' ? 'checked' : ''}><b>A NAS over FTP</b>
        <small>Synology, QNAP, a router's USB drive… anything with an FTP server on your network. Built in, nothing to install.</small></label>
      <label><input type="radio" name="cl-m" value="rclone" ${method === 'rclone' ? 'checked' : ''}><b>A cloud account through rclone</b>
        <small>No sync app needed; works with Google Drive, OneDrive, Dropbox, Amazon S3, Backblaze B2 and 70+ more. Needs the free rclone program set up once.</small></label>
    </div>
    <div class="form" id="cl-folder">
      <label for="cl-dir">Folder</label><div class="row"><input type="text" id="cl-dir" value="${method === 'folder' ? esc(c.remote) : ''}" placeholder="e.g. \\\\NAS\\cameras or C:\\Users\\you\\Google Drive\\GAZE" style="flex:1"><button type="button" id="cl-pick">Browse…</button></div>
    </div>
    <div class="form" id="cl-share">
      <label for="cl-suser">NAS login</label><div class="row"><input type="text" id="cl-suser" value="${esc(c.user || '')}" placeholder="user name (only if the share asks)" style="flex:1" autocomplete="off"><input type="password" id="cl-spass" placeholder="${c.has_password ? 'Saved – leave empty to keep' : 'password'}" style="flex:1" autocomplete="new-password"></div>
    </div>
    <div class="form" id="cl-ftp">
      <label for="cl-fhost">NAS address</label><div class="row"><input type="text" id="cl-fhost" value="${esc(ftp.host)}" placeholder="e.g. 192.168.1.20 or nas.local" style="flex:3"><input type="number" id="cl-fport" value="${esc(ftp.port)}" min="1" max="65535" style="flex:1" title="FTP port (21)"></div>
      <label for="cl-ffold">Folder</label><div><input type="text" id="cl-ffold" value="${esc(ftp.folder)}" placeholder="e.g. Cameras/GAZE"><div class="hint">Made if it doesn't exist. Each camera gets its own folder inside.</div></div>
      <label for="cl-fuser">Login</label><div class="row"><input type="text" id="cl-fuser" value="${esc(c.user || '')}" placeholder="user name" style="flex:1" autocomplete="off"><input type="password" id="cl-fpass" placeholder="${c.has_password ? 'Saved – leave empty to keep' : 'password'}" style="flex:1" autocomplete="new-password"></div>
      <span></span><div class="hint">Turn FTP on in the NAS first (Synology: Control Panel → File Services → FTP). Plain FTP, as NAS boxes offer it at home: use it on your own network.</div>
    </div>
    <div class="form" id="cl-rclone">
      <label>Step 1</label><div>Install rclone from <a href="https://rclone.org/downloads/" target="_blank" rel="noopener">rclone.org</a>.
        <div class="row" style="margin-top:6px"><input type="text" id="cl-path" value="${esc(c.rclone_path)}" style="flex:1" title="rclone program (name or full path)"><button type="button" id="cl-check">Check</button></div></div>
      <label>Step 2</label><div>Add your cloud account once: open a command prompt and run <code>rclone config</code> (choose “n” for new, give it a name like <i>gdrive</i>, pick the service and sign in).</div>
      <label for="cl-acc">Step 3</label><div class="row"><select id="cl-acc"><option value="">Choose the account…</option></select><input type="text" id="cl-sub" placeholder="folder, e.g. GAZE" style="flex:1"></div>
    </div>
    <h3>2. What to back up</h3>
    <div class="form"><span></span><div><select id="cl-what">
        <option value="ai" ${c.upload === 'ai' ? 'selected' : ''}>Only alerts the AI recognized (a person, car…) — recommended</option>
        <option value="events" ${c.upload === 'events' ? 'selected' : ''}>All alerts, including motion-only</option>
        <option value="all" ${c.upload === 'all' ? 'selected' : ''}>Everything, including 24/7 recordings (needs a fast upload and lots of cloud space)</option></select></div></div>
    <h3>3. Turn it on</h3>
    <div class="form"><span></span><label class="check"><input type="checkbox" id="cl-on" ${c.enabled ? 'checked' : ''}> Copy new recordings there automatically</label></div>
    <div class="actions"><button type="button" id="cl-test">Test</button><button class="primary">Save</button></div><div id="cl-result" style="margin-top:10px"></div></form>`;
  const sync = () => {
    method = document.querySelector('input[name=cl-m]:checked').value;
    $('cl-folder').hidden = method !== 'folder'; $('cl-rclone').hidden = method !== 'rclone'; $('cl-ftp').hidden = method !== 'ftp';
    $('cl-share').hidden = !(method === 'folder' && /^\\\\/.test(val('cl-dir').trim()));
  };
  $('cl-dir').addEventListener('input', sync);
  document.querySelectorAll('input[name=cl-m]').forEach(r => r.onchange = sync); sync();
  if (method === 'rclone' && c.remote) { const [acc, ...rest] = c.remote.split(':'); $('cl-acc').innerHTML += `<option value="${esc(acc)}" selected>${esc(acc)}</option>`; $('cl-sub').value = rest.join(':'); }
  const remote = () => method === 'folder' ? val('cl-dir').trim()
    : method === 'ftp' ? (val('cl-fhost').trim() ? `ftp://${val('cl-fhost').trim()}${val('cl-fport') && val('cl-fport') !== '21' ? ':' + val('cl-fport') : ''}/${val('cl-ffold').trim().replace(/^\/+/, '')}` : '')
    : (val('cl-acc') ? `${val('cl-acc')}:${val('cl-sub').trim()}` : '');
  const login = () => method === 'ftp' ? {user: val('cl-fuser').trim(), password: val('cl-fpass')} : method === 'folder' ? {user: val('cl-suser').trim(), password: val('cl-spass')} : {user: '', password: ''};
  const form = () => ({enabled: chk('cl-on'), remote: remote(), ...login(), upload: val('cl-what'), rclone_path: val('cl-path').trim() || 'rclone'});
  const result = (ok, msg) => $('cl-result').innerHTML = `<span class="${ok ? 'ok' : 'err'}">${ok ? '✓' : '✗'} ${esc(msg)}</span>`;
  $('cl-pick').onclick = async () => { const p = await pickFolder(val('cl-dir'), 'Choose the synced folder'); if (p) $('cl-dir').value = p; };
  $('cl-check').onclick = async () => {
    const r = await api('api/cloud/remotes', {method: 'POST', json: form()}).catch(e => ({ok: false, message: e.message}));
    if (!r.ok) return result(false, r.message);
    const cur = val('cl-acc');
    $('cl-acc').innerHTML = '<option value="">Choose the account…</option>' + r.remotes.map(x => `<option ${x === cur ? 'selected' : ''}>${esc(x)}</option>`).join('');
    result(true, r.remotes.length ? `rclone works — ${r.remotes.length} account${r.remotes.length === 1 ? '' : 's'} found. Choose one in step 3.` : 'rclone works, but no account is set up yet — do step 2, then Check again.');
  };
  $('f-cloud').onsubmit = async (e) => { e.preventDefault(); if (chk('cl-on') && !remote()) return result(false, method === 'folder' ? 'Choose the folder first.' : method === 'ftp' ? 'Enter the NAS address first.' : 'Choose the rclone account first.'); await saveSettings({cloud: form()}); paintCloudStatus(); };
  $('cl-test').onclick = async () => {
    $('cl-result').innerHTML = '<span class="muted">Testing…</span>';
    const r = await api('api/cloud/test', {method: 'POST', json: form()}).catch(e => ({ok: false, message: e.message}));
    result(r.ok, r.message);
  };
  if (method === 'rclone') $('cl-check').click();
  paintCloudStatus();
}
async function paintCloudStatus() {
  const el = $('cl-status'); if (!el) return;
  const st = await api('api/cloud/status').catch(() => null);
  const c = settings.cloud;
  if (!c.enabled) { el.innerHTML = '<span class="muted">● Backup is off.</span>'; return; }
  el.innerHTML = !st ? '' : `<span class="${st.last_error ? 'err' : 'ok'}">●</span> Backing up to <b>${esc(c.remote)}</b> · ${st.uploaded} file${st.uploaded === 1 ? '' : 's'} uploaded since GAZE started`
    + (st.last_upload ? ` · last ${fmtDate(st.last_upload)}` : '') + (st.waiting ? ` · ${st.waiting} waiting` : ' · up to date')
    + (st.last_error ? `<div class="err">Problem: ${esc(st.last_error)}</div>` : '');
}

// ---------- backup ----------
function renderBackup(body) {
  body.innerHTML = `<div class="card"><h2>Export setup</h2>
    <p class="muted" style="margin-top:0">One file with everything you've set up, to restore after reinstalling or to copy to another computer: every camera and its settings, profiles, zones and masks, modes, notifications, smart-home links, users with their camera access and layouts, and People &amp; vehicles with their faces, plates and photos. Recordings and alerts are not included.</p>
    <label class="check"><input type="checkbox" id="bk-pw"> Include passwords, keys and tokens${tip('Camera passwords, users’ passwords (stored scrambled) and two-step sign-in, API tokens, notification and MQTT passwords, live-streaming keys. Without them, importing on this computer keeps the passwords it has now; on a new computer you type them in again.')} (keep the file somewhere safe)</label>
    <div class="actions"><button class="primary" id="bk-export">⇩ Download backup</button></div></div>
    <div class="card"><h2>Import setup</h2>
    <p class="muted" style="margin-top:0">Replaces all cameras and settings with the ones in the backup. Cameras not in it are removed (their recordings stay on disk). Users and People &amp; vehicles in the backup are added or updated; others here are kept. Passwords the backup doesn't have are kept from this computer.</p>
    <input type="file" id="bk-file" accept=".zip,.toml,application/zip,text/plain">
    <label class="check"><input type="checkbox" id="bk-keep" checked> Keep this NVR's login and web address</label>
    <div class="actions"><button id="bk-import" class="danger">Import</button></div><div id="bk-result"></div></div>`;
  $('bk-export').onclick = () => location.href = 'api/config/export?passwords=' + chk('bk-pw');
  $('bk-import').onclick = async () => {
    const f = $('bk-file').files[0];
    if (!f) return toast('Choose a setup file first.', true);
    if (!confirm('Replace all cameras and settings with the imported file?')) return;
    try {
      const r = await api('api/config/import?keep_login=' + chk('bk-keep'), {method: 'POST', body: await f.arrayBuffer(), headers: {'Content-Type': 'application/octet-stream'}});
      $('bk-result').innerHTML = `<div class="ok" style="margin-top:8px">✓ ${esc(r.message)}${r.restart_required ? ' Restart GAZE to use the new web address.' : ''}</div>`;
      toast(r.message);
      await loadSettings(); await refreshStatus(); await loadKnownPeople().catch(() => {});
    } catch (e) { $('bk-result').innerHTML = `<div class="err" style="margin-top:8px">${esc(e.message)}</div>`; }
  };
}

// ---------- system & login ----------
async function renderSystemSettings(body) {
  const s = settings.server;
  if (!sysinfo) sysinfo = await api('api/system').catch(() => null);
  const addrs = (sysinfo && sysinfo.addresses) || [];
  const [bind, port] = (() => { const i = s.listen.lastIndexOf(':'); return [s.listen.slice(0, i), s.listen.slice(i + 1)]; })();
  body.innerHTML = `<form class="card" id="f-sys"><h2>System</h2><div class="form">
    <label for="y-name">Name of this system${tip('Shown in the browser tab and on notifications — handy with more than one GAZE. Example: Home, or Shop.')}</label><div><input type="text" id="y-name" required value="${esc(s.name)}"></div>
    <label>Web address</label><div id="y-addr"></div>
    <label for="y-port">Port${tip('The number after the colon in GAZE’s web address, e.g. http://192.168.1.10:8080. Only change it if another program already uses 8080.')}</label><div class="row"><input type="number" id="y-port" min="1" max="65535" value="${esc(port)}" style="width:110px"><span class="muted">1–65535 (default 8080)</span></div>
    <label for="y-bind">Reachable from${tip('Who can open GAZE. “Every device on my network” lets your phone and other computers at home use it. “Only this computer” keeps it private to this PC.')}</label><div><select id="y-bind"><option value="0.0.0.0">Every device on my network (all network adapters)</option><option value="127.0.0.1">Only this computer</option>
      ${addrs.map(a => `<option value="${esc(a)}">Only via ${esc(a)}</option>`).join('')}</select><div class="hint">Changing the port or this setting takes effect after the NVR restarts.</div></div>
    <h3>Other GAZE servers${tip('See the cameras of another GAZE (a holiday house, the shop, a relative\u2019s) on this Live page. On that GAZE, make an API token (\u{1F464} menu \u2192 API tokens), then add its address and the token here. Its cameras show under your own; click one to open it there.')}</h3>
    <span></span><div id="y-remotes"></div>
    <span></span><div><button type="button" class="small" id="y-remote-add">＋ Add a server</button></div>
    <h3>Secure address (https)</h3>
    <label for="y-hport">Secure port${tip('GAZE also answers on a secure https address. Phones need it for the share menu and to install GAZE as an app, and it keeps passwords and video private on the way. 0 turns it off.')}</label><div class="row"><input type="number" id="y-hport" min="0" max="65535" value="${s.https_port ?? 8443}" style="width:110px"><span class="muted">0 = off (default 8443)</span></div>
    <label>Certificate${tip('Proves to browsers that this really is your GAZE. GAZE\u2019s own works straight away, but each browser warns once (\u201cAdvanced \u2192 Continue\u201d). A Tailscale certificate is trusted everywhere with no warning, and is also the easy, safe way to reach GAZE away from home.')}</label><div>
      <select id="y-cert"><option value="own">GAZE's own — browsers warn once</option><option value="tailscale">From Tailscale — trusted, works away from home</option><option value="files">My own certificate files</option></select>
      <div id="y-cert-files" class="row" style="margin-top:6px" hidden><input type="text" id="y-hcert" placeholder="Certificate file (.crt / .pem)" value="${esc(s.https_cert || '')}" style="flex:1"><input type="text" id="y-hkey" placeholder="Key file (.key / .pem)" value="${esc(s.https_key || '')}" style="flex:1"></div>
      <div id="y-cert-ts" style="margin-top:6px" hidden><button type="button" id="y-ts">Get a certificate from Tailscale</button>
        <div class="hint">Needs Tailscale installed and signed in on this computer, with MagicDNS and HTTPS certificates turned on in the Tailscale admin page (DNS tab). Renewed by itself.</div></div>
      <div class="hint" id="y-https-st"></div></div>
    <label for="y-ffmpeg">ffmpeg program${tip('ffmpeg is the free video tool GAZE uses to record and convert. Leave as “ffmpeg” unless it’s somewhere unusual; then give its full path, e.g. C:\\ffmpeg\\bin\\ffmpeg.exe (ffprobe must be in the same folder).')}</label><div><input type="text" id="y-ffmpeg" required value="${esc(s.ffmpeg_path)}"><div class="hint">The video tool GAZE uses. Leave as it is unless you were told otherwise.</div></div>
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
  showAddr();
  // Other GAZE servers.
  const rmRow = (r) => `<div class="rm row" style="flex-wrap:wrap;gap:6px;margin-bottom:6px"><input type="text" class="rm-name" placeholder="Name, e.g. Holiday house" value="${esc(r.name || '')}" style="width:170px"><input type="text" class="rm-url" placeholder="https://its-address:8443" value="${esc(r.url || '')}" style="flex:1;min-width:200px"><input type="password" class="rm-token" placeholder="${r.url ? 'Token saved – leave empty to keep' : 'API token from that GAZE'}" style="width:230px" autocomplete="off"><label class="check" title="That GAZE uses its own certificate (browsers warn about it): accept it"><input type="checkbox" class="rm-trust" ${r.trust_own_cert ? 'checked' : ''}> trust its own certificate</label><button type="button" class="small danger rm-del">✕</button></div>`;
  const rmWire = () => document.querySelectorAll('#y-remotes .rm-del').forEach(b => b.onclick = () => b.closest('.rm').remove());
  $('y-remotes').innerHTML = (settings.remotes || []).map(rmRow).join(''); rmWire();
  $('y-remote-add').onclick = () => { $('y-remotes').insertAdjacentHTML('beforeend', rmRow({})); rmWire(); };
  // https: which certificate, and how it's going.
  const certKind = () => !s.https_cert ? 'own' : /tailscale\.crt$/.test(s.https_cert) ? 'tailscale' : 'files';
  $('y-cert').value = certKind();
  const certShow = () => { $('y-cert-files').hidden = val('y-cert') !== 'files'; $('y-cert-ts').hidden = val('y-cert') !== 'tailscale'; };
  $('y-cert').onchange = certShow; certShow();
  api('api/https').then(h => {
    const st = h.status || {}, hosts = val('y-bind') === '0.0.0.0' ? (addrs.length ? addrs : ['this-computer']) : [val('y-bind') === '127.0.0.1' ? 'localhost' : val('y-bind')];
    $('y-https-st').innerHTML = st.on ? `On (${esc(st.source)}): ${hosts.map(x => `<a href="https://${esc(x)}:${st.port}/" target="_blank" rel="noopener">https://${esc(x)}:${st.port}</a>`).join(' · ')}`
      : st.error ? `<span class="errc">Not working: ${esc(st.error)}</span>` : h.port ? 'Starts when GAZE restarts.' : 'Off.';
  }).catch(() => {});
  $('y-ts').onclick = async () => {
    const b = $('y-ts'); b.disabled = true; b.textContent = 'Asking Tailscale…';
    try { const r = await api('api/https/tailscale', {method: 'POST'}); toast(r.message); s.https_cert = 'tailscale.crt'; $('y-https-st').textContent = r.message; }
    catch (err) { toast(err.message, true); }
    b.disabled = false; b.textContent = 'Get a certificate from Tailscale';
  };
  $('f-sys').onsubmit = async (e) => {
    e.preventDefault();
    const listen = `${val('y-bind')}:${Math.round(num('y-port'))}`;
    const remotes = [...document.querySelectorAll('#y-remotes .rm')].map(el => ({name: el.querySelector('.rm-name').value.trim(), url: el.querySelector('.rm-url').value.trim(), token: el.querySelector('.rm-token').value.trim(), trust_own_cert: el.querySelector('.rm-trust').checked})).filter(r => r.url);
    const kind = val('y-cert'), files = kind === 'files';
    const server = {name: val('y-name'), listen, ffmpeg_path: val('y-ffmpeg'), https_port: Math.max(0, Math.round(num('y-hport')) || 0)};
    if (kind === 'own') { server.https_cert = ''; server.https_key = ''; }
    if (files) { server.https_cert = val('y-hcert'); server.https_key = val('y-hkey'); }
    if (await saveSettings({server, remotes})) { remotesAt = 0; sysinfo = null; renderSystemSettings(body); }
  };
}

async function renderInfo(body) {
  sysinfo = await api('api/system').catch(() => null);
  if (!sysinfo) return;
  const c = sysinfo.capabilities;
  body.innerHTML = `<div class="card"><h2>About</h2><dl>
    <dt>Version</dt><dd><b>GAZE ${esc(sysinfo.version)}</b></dd><dt>Operating system</dt><dd>${esc(sysinfo.os)} (${esc(sysinfo.arch)})</dd>
    <dt>Uptime</dt><dd>${fmtDur(sysinfo.uptime_seconds)}</dd><dt>Web address</dt><dd>${(sysinfo.addresses || []).map(a => esc(`http://${a}:${sysinfo.listen.split(':').pop()}`)).join('<br>') || esc(sysinfo.listen)}</dd>
    <dt>ffmpeg</dt><dd>${esc(c.ffmpeg_version || 'NOT FOUND')}</dd>
    <dt>GPU decoders in ffmpeg</dt><dd>${esc(c.hwaccels.join(', ') || 'none')}</dd><dt>H.264 encoders</dt><dd>${esc(c.encoders.join(', ') || 'none')}</dd>
    <dt>Config file</dt><dd>${esc(sysinfo.config_path)}</dd><dt>Data folder</dt><dd>${esc(sysinfo.data_dir)}</dd>
    <dt>Log file</dt><dd>${esc(sysinfo.log_file || '–')}</dd><dt>Recordings folder</dt><dd>${esc(sysinfo.storage_dir)}</dd></dl></div>
    <div class="card admin-only"><h2>Updates</h2><div id="upd"></div>
      <div class="form" style="margin-top:8px"><span></span><div><label class="check"><input type="checkbox" id="upd-auto" ${settings.update && settings.update.auto ? 'checked' : ''}> Install updates automatically${tip('GAZE checks for a new version once a day. Off: a bar at the top tells you, and you choose when (⟳ Updates checks right away). On: it installs as soon as one is found, when no alert is going on, and restarts in a few seconds.')}</label>
        <div class="hint">Off: you're told when there's a new version and choose when to install it.</div></div>
        <label for="upd-repo">Update from${tip('Where new versions come from. Leave empty for the normal GAZE releases. Only change it if you build your own copy of GAZE.')}</label><div><div class="row"><span class="muted">github.com/</span><input type="text" id="upd-repo" value="${esc((settings.update && settings.update.repo) || '')}" placeholder="owner/repository" style="flex:1"><button type="button" id="upd-repo-save">Save</button></div>
          <div class="hint">Empty: the GitHub repository this copy of GAZE was built from. Enter your own (e.g. <code>you/NVR</code>) to get updates from your fork's releases — see UPDATES.md.</div></div></div></div>
    <div class="card changelog"><h2>What's new</h2><div id="chlog" class="muted">Loading…</div></div>`;
  try { $('chlog').innerHTML = mdLite(await (await fetch('api/changelog')).text()); } catch (e) {}
  if (!$('upd')) return;
  const saveUpd = (msg) => saveSettings({update: {auto: chk('upd-auto'), repo: val('upd-repo').trim().replace(/^https?:\/\/github\.com\//, '').replace(/\/$/, '')}}, msg);
  $('upd-auto').onchange = () => saveUpd(chk('upd-auto') ? 'Updates will install automatically' : 'Automatic updates off');
  $('upd-repo-save').onclick = async () => { if (await saveUpd('Saved')) { try { show(await api('api/update/check', {method: 'POST'})); } catch (e) { toast(e.message, true); } } };
  let timer = null;
  const show = (u) => {
    const box = $('upd'); if (!box) { clearInterval(timer); return; }
    const busy = ['downloading', 'installing', 'restarting'].includes(u.state);
    const pct = u.total ? Math.round(100 * u.done / u.total) : null;
    box.innerHTML = `<dl><dt>This version</dt><dd>${esc(u.current)}</dd><dt>Updates from</dt><dd><a href="https://github.com/${esc(u.repo || '')}/releases" target="_blank" rel="noopener">github.com/${esc(u.repo || '')}</a></dd><dt>Newest</dt><dd>${u.latest ? esc(u.latest) : '–'}${u.checked ? ` <span class="muted small">(checked ${fmtAgo(u.checked)})</span>` : ''}</dd></dl>
      ${u.error ? `<div class="errc small">${esc(u.error)}</div>` : ''}
      ${u.state === 'restarting' ? '<div class="okc">Installed — GAZE is restarting; this page reloads in a moment…</div>'
        : busy ? `<div>${u.state === 'downloading' ? 'Downloading' : 'Installing'} GAZE ${esc(u.latest || '')}… ${pct != null ? pct + '%' : ''}</div>${pct != null ? `<div class="bar"><div style="width:${pct}%"></div></div>` : ''}`
        : u.available ? `<div class="row" style="gap:8px"><b class="okc">GAZE ${esc(u.latest)} is available</b><button type="button" class="primary" id="upd-go">Update now</button></div>${u.notes ? `<details style="margin-top:6px"><summary>What's new in ${esc(u.latest)}</summary><div class="muted">${mdLite(u.notes)}</div></details>` : ''}`
        : `<div class="row" style="gap:8px"><span class="muted">${u.latest ? 'GAZE is up to date.' : ''}</span><button type="button" id="upd-check">Check for updates</button></div>`}`;
    if ($('upd-check')) $('upd-check').onclick = async () => { $('upd-check').disabled = true; try { show(await api('api/update/check', {method: 'POST'})); } catch (e) { toast(e.message, true); show(await api('api/update')); } };
    if ($('upd-go')) $('upd-go').onclick = async () => {
      if (!confirm(`Update to GAZE ${u.latest} now? GAZE restarts (a few seconds without recording).`)) return;
      try { await api('api/update/install', {method: 'POST'}); poll(); } catch (e) { toast(e.message, true); }
    };
    if (u.state === 'restarting') waitForRestart();
  };
  const poll = () => { clearInterval(timer); timer = setInterval(async () => { try { show(await api('api/update')); } catch (_) {} }, 1000); };
  const waitForRestart = () => {
    clearInterval(timer);
    const t = setInterval(async () => { try { const r = await fetch('api/update'); if (r.ok) { const u = await r.json(); if (u.state !== 'restarting') { clearInterval(t); location.reload(); } } } catch (_) {} }, 2000);
  };
  show(await api('api/update').catch(() => ({current: sysinfo.version, state: 'idle'})));
}

// ======================================================================
// Start
// ======================================================================
// Phones and computers each keep their own layout.
function deviceKind() { return isPhone() || window.innerWidth < 700 ? 'mobile' : 'desktop'; }
// A tiny Markdown reader for the changelog (headings, bold, lists).
function mdLite(md) {
  const inline = (t) => esc(t).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/\*(.+?)\*/g, '<i>$1</i>');
  let out = '', inList = false;
  for (const line of md.split('\n')) {
    const li = line.match(/^\s*-\s+(.*)/);
    if (li) { if (!inList) { out += '<ul>'; inList = true; } out += `<li>${inline(li[1])}</li>`; continue; }
    if (inList) { out += '</ul>'; inList = false; }
    const h = line.match(/^(#{1,3})\s+(.*)/);
    if (h) out += `<h${h[1].length + 1}>${inline(h[2])}</h${h[1].length + 1}>`;
    else if (line.trim()) out += `<p>${inline(line)}</p>`;
  }
  return out + (inList ? '</ul>' : '');
}

// ---------- signing in ----------
let me = null, started = false;
function showAuth(mode) {
  $('auth').hidden = false;
  document.body.classList.add('signed-out');
  const setup = mode === 'setup';
  $('auth-sub').textContent = setup ? 'Welcome! Create the admin account' : 'Sign in to watch your cameras';
  $('auth-intro').hidden = !setup;
  $('auth-intro').textContent = 'This account can change every setting and add other users (for example family members who may only watch). Keep the password safe.';
  $('auth-pass2-row').hidden = !setup;
  $('auth-pass2').required = setup;
  $('auth-pass').autocomplete = setup ? 'new-password' : 'current-password';
  $('auth-remember-row').hidden = setup;
  $('auth-go').textContent = setup ? 'Create admin account' : 'Sign in';
  $('auth-err').hidden = true;
  $('auth-form').onsubmit = async (e) => {
    e.preventDefault();
    const fail = (m) => { $('auth-err').hidden = false; $('auth-err').textContent = m; };
    if (setup && $('auth-pass').value !== $('auth-pass2').value) return fail('The two passwords are different.');
    $('auth-go').disabled = true;
    try {
      const r = await fetch(setup ? 'api/auth/setup' : 'api/auth/login', {method: 'POST', headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({username: val('auth-user'), password: $('auth-pass').value, remember: chk('auth-remember'), code: val('auth-code') || null})});
      if (!r.ok) {
        const j = await r.json().catch(() => ({}));
        // Right password: now the code from the authenticator app.
        if (j.need_code) { $('auth-code-row').hidden = false; $('auth-err').hidden = true; $('auth-code').focus(); $('auth-go').textContent = 'Sign in'; return; }
        return fail(j.error || 'Could not sign in');
      }
      $('auth-pass').value = $('auth-pass2').value = $('auth-code').value = '';
      $('auth-code-row').hidden = true;
      boot();
    } catch (err) { fail(err.message); } finally { $('auth-go').disabled = false; }
  };
  setTimeout(() => (val('auth-user') ? $('auth-pass') : $('auth-user')).focus(), 50);
  let idleOut = null; try { idleOut = sessionStorage.getItem('gaze-idle-out'); sessionStorage.removeItem('gaze-idle-out'); } catch (_) {}
  if (idleOut) { $('auth-err').hidden = false; $('auth-err').textContent = `Signed out after ${idleOut} minute${idleOut === '1' ? '' : 's'} with no activity. Sign in again to carry on.`; }
}
async function boot() {
  let st;
  try { st = await (await fetch('api/auth/state')).json(); } catch (e) { setTimeout(boot, 3000); return; }
  $('auth-name').textContent = st.name || 'GAZE';
  if (st.setup_needed) return showAuth('setup');
  if (!st.user) return showAuth('login');
  me = st.user;
  watchIdle(st.idle_minutes || 0);
  $('auth').hidden = true;
  document.body.classList.remove('signed-out');
  document.body.classList.remove('role-admin', 'role-operator', 'role-viewer');
  document.body.classList.add('role-' + me.role);
  $('user-menu').hidden = false;
  $('um-name').textContent = me.username;
  $('um-role').textContent = {admin: 'Admin — full access', operator: 'Operator — watch, move cameras, manage alerts', viewer: 'Viewer — watch only'}[me.role] + (me.cameras.length ? ` · ${me.cameras.length} camera${me.cameras.length === 1 ? '' : 's'}` : '');
  if (!started) {
    started = true;
    Live.connect();
    setInterval(refreshStatus, 2000);
  } else Live.ws && Live.ws.close();
  settings = null;
  await refreshStatus();
  showWhatsNew();
  route();
  // Recordings loads in the background meanwhile (ready when you go there).
  setTimeout(preloadRecordings, 2500);
}
// Sign out after no activity (Settings → Users → Signing in): no mouse,
// keyboard or touch for that long on this page. (GAZE also signs out a
// session nothing has used for that long, e.g. with the app closed.)
let idleTimer = null, idleLimit = 0, idleLast = Date.now();
function watchIdle(minutes) {
  idleLimit = minutes; idleLast = Date.now();
  clearInterval(idleTimer); idleTimer = null;
  if (!minutes) return;
  idleTimer = setInterval(async () => {
    if (Date.now() - idleLast < idleLimit * 60000) return;
    clearInterval(idleTimer); idleTimer = null;
    try { await fetch('api/auth/logout', {method: 'POST'}); } catch (_) {}
    try { sessionStorage.setItem('gaze-idle-out', String(idleLimit)); } catch (_) {}
    location.reload();
  }, 15000);
}
for (const ev of ['pointerdown', 'pointermove', 'keydown', 'wheel', 'touchstart']) addEventListener(ev, () => { idleLast = Date.now(); }, {passive: true, capture: true});
const isAdmin = () => me && me.role === 'admin';
$('um-btn').onclick = (e) => { e.stopPropagation(); $('um-ds').value = dsMode; $('um-pop').hidden = !$('um-pop').hidden; };
// Day / night colours on this device (Automatic follows the device).
function applyTheme(t) {
  document.documentElement.dataset.theme = t;
  const light = t === 'light' || (t === 'auto' && matchMedia('(prefers-color-scheme: light)').matches);
  const m = document.querySelector('meta[name=theme-color]'); if (m) m.content = light ? '#ffffff' : '#14161c';
}
$('um-theme').value = store.get('theme', 'auto'); applyTheme($('um-theme').value);
$('um-theme').onchange = () => { store.set('theme', val('um-theme')); applyTheme(val('um-theme')); };
matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => applyTheme(store.get('theme', 'auto')));
$('um-ds').onchange = () => { setDataSaver(val('um-ds')); toast('Data saver: ' + $('um-ds').selectedOptions[0].textContent); };
$('um-out').onclick = async () => { await fetch('api/auth/logout', {method: 'POST'}); location.hash = '#live'; location.reload(); };
// API tokens: keys for scripts, Home Assistant or another GAZE.
$('um-tokens').onclick = async () => {
  $('um-pop').hidden = true;
  const dlg = document.createElement('dialog'); dlg.className = 'wn-dlg';
  document.body.appendChild(dlg);
  const close = () => { dlg.close(); dlg.remove(); };
  const draw = async (fresh) => {
    let list = []; try { list = await api('api/auth/tokens'); } catch (e) { toast(e.message, true); }
    dlg.innerHTML = `<div class="head"><h2>🔑 API tokens</h2><span class="spacer"></span><button type="button" class="small" data-x>✕</button></div>
      <div class="body"><p class="muted" style="margin-top:0">A token lets a script, Home Assistant or another GAZE use GAZE as you (with your access), sent as <code>Authorization: Bearer …</code>. Remove one to stop it working.</p>
      ${fresh ? `<div class="tok-new"><b>Copy this token now — it won't be shown again:</b><div class="row" style="margin-top:6px"><input type="text" readonly value="${esc(fresh)}" id="tok-val" style="flex:1"><button type="button" id="tok-copy">📋 Copy</button></div></div>` : ''}
      ${list.length ? `<table><tbody>${list.map(t => `<tr><td><b>${esc(t.name)}</b></td><td class="muted small">${t.created ? fmtDate(t.created) : ''}</td><td style="text-align:right"><button type="button" class="small danger" data-del="${esc(t.id)}">Remove</button></td></tr>`).join('')}</tbody></table>` : '<p class="muted">No tokens yet.</p>'}
      <div class="row" style="margin-top:10px"><input type="text" id="tok-name" placeholder="What it's for, e.g. Home Assistant" style="flex:1"><button type="button" class="primary" id="tok-make">Make a token</button></div></div>`;
    dlg.querySelectorAll('[data-x]').forEach(b => b.onclick = close);
    if ($('tok-copy')) $('tok-copy').onclick = () => navigator.clipboard && navigator.clipboard.writeText(val('tok-val')).then(() => toast('Copied')).catch(() => $('tok-val').select());
    $('tok-make').onclick = async () => { try { const r = await api('api/auth/tokens', {method: 'POST', json: {name: val('tok-name')}}); draw(r.token); } catch (e) { toast(e.message, true); } };
    dlg.querySelectorAll('[data-del]').forEach(b => b.onclick = async () => { if (!confirm('Remove this token? Whatever uses it stops working.')) return; try { await api('api/auth/tokens/' + enc(b.dataset.del), {method: 'DELETE'}); draw(); } catch (e) { toast(e.message, true); } });
  };
  await draw(); dlg.showModal();
};

// Two-step sign-in: a code from an authenticator app after the password.
$('um-2fa').onclick = async () => {
  $('um-pop').hidden = true;
  const dlg = document.createElement('dialog'); dlg.className = 'wn-dlg';
  const close = () => { dlg.close(); dlg.remove(); };
  const head = `<div class="head"><h2>🔐 Two-step sign-in</h2><span class="spacer"></span><button type="button" class="small" data-x>✕</button></div>`;
  document.body.appendChild(dlg);
  const wire = () => dlg.querySelectorAll('[data-x]').forEach(b => b.onclick = close);
  if (me.two_step) {
    dlg.innerHTML = head + `<div class="body"><p>Two-step sign-in is <b>on</b>: signing in asks for the code from your authenticator app after your password.</p>
      <label>Your password, to turn it off<input type="password" id="tf-pw" autocomplete="current-password"></label>
      <div class="actions"><button type="button" data-x>Keep it on</button><button type="button" class="danger" id="tf-off">Turn off</button></div></div>`;
    wire(); dlg.showModal();
    $('tf-off').onclick = async () => { try { await api('api/auth/two-step/off', {method: 'POST', json: {password: val('tf-pw')}}); me.two_step = false; toast('Two-step sign-in is off'); close(); } catch (e) { toast(e.message, true); } };
    return;
  }
  let r; try { r = await api('api/auth/two-step/start', {method: 'POST'}); } catch (e) { toast(e.message, true); dlg.remove(); return; }
  dlg.innerHTML = head + `<div class="body">
    <p style="margin-top:0">After your password, GAZE will also ask for a 6-digit code from an app on your phone — so a stolen password alone isn't enough.</p>
    <ol class="steps"><li>Install an authenticator app: Google Authenticator, Microsoft Authenticator, Authy or your password manager.</li>
      <li>In the app, add an account and scan this code:</li></ol>
    <div class="tf-qr">${r.qr_svg}</div>
    <p class="muted small" style="text-align:center">Can't scan? Type this key instead: <code>${esc(r.secret)}</code></p>
    <label>3. Type the 6-digit code the app shows<input type="text" id="tf-code" inputmode="numeric" autocomplete="one-time-code" placeholder="123456"></label>
    <div class="actions"><button type="button" data-x>Cancel</button><button type="button" class="primary" id="tf-ok">Turn on</button></div></div>`;
  wire(); dlg.showModal(); $('tf-code').focus();
  $('tf-ok').onclick = async () => {
    try {
      const x = await api('api/auth/two-step/finish', {method: 'POST', json: {code: val('tf-code')}});
      me.two_step = true;
      dlg.innerHTML = head + `<div class="body"><p><b>Two-step sign-in is on.</b> Keep these recovery codes somewhere safe (print them, or save them in your password manager). Each works once, instead of a code, if you lose your phone:</p>
        <pre class="tf-codes">${x.recovery.map(esc).join('\n')}</pre>
        <div class="actions"><button type="button" id="tf-copy">📋 Copy</button><button type="button" class="primary" data-x>I've saved them</button></div></div>`;
      wire();
      $('tf-copy').onclick = () => { navigator.clipboard && navigator.clipboard.writeText(x.recovery.join('\n')).then(() => toast('Copied')).catch(() => {}); };
    } catch (e) { toast(e.message, true); }
  };
};
$('um-pass').onclick = async () => {
  $('um-pop').hidden = true;
  const cur = prompt('Current password:'); if (cur == null) return;
  const nw = prompt('New password (at least 8 characters):'); if (!nw) return;
  if (prompt('New password again:') !== nw) return toast('The two passwords are different.', true);
  try { await api('api/auth/password', {method: 'POST', json: {current: cur, new: nw}}); toast('Password changed'); } catch (e) { toast(e.message, true); }
};

// ---------- Settings → People & vehicles ----------
// Photos are shrunk in the browser and sent as JPEG.
function photoToJpeg(file, max = 1600) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => {
      const s = Math.min(1, max / Math.max(img.width, img.height)), c = document.createElement('canvas');
      c.width = Math.round(img.width * s); c.height = Math.round(img.height * s);
      c.getContext('2d').drawImage(img, 0, 0, c.width, c.height);
      c.toBlob(b => b ? resolve(b) : reject(new Error('could not read the photo')), 'image/jpeg', 0.9);
      URL.revokeObjectURL(img.src);
    };
    img.onerror = () => reject(new Error('could not read the photo'));
    img.src = URL.createObjectURL(file);
  });
}
// ---------- check faces ----------
// Every face found in alerts, to go through: confirm the right ones (they
// join that person's faces, so GAZE knows them better) and correct the
// wrong ones (GAZE won't take that face for them again).
const facesView = {who: store_get('faces-who', ''), show: store_get('faces-show', 'check'), days: +store_get('faces-days', 30)};
async function renderFaces(body) {
  body.innerHTML = '<div class="card"><h2>Check faces</h2><div class="muted">Loading…</div></div>';
  const [all, list] = await Promise.all([api('api/faces?days=' + facesView.days).catch(() => []), api('api/known').catch(() => [])]);
  if (section !== 'faces') return;
  const people = list.filter(k => k.kind === 'person');
  const v = facesView;
  const shown = all.filter(f => (v.show === 'check' ? f.name && !f.confirmed : v.show === 'done' ? f.confirmed && f.name : v.show === 'unknown' ? !f.name : true)
    && (!v.who || f.name === v.who));
  const toCheck = all.filter(f => f.name && !f.confirmed).length;
  const names = [...new Set(all.map(f => f.name).filter(Boolean))].sort();
  body.innerHTML = `<div class="card"><div class="row" style="margin-bottom:6px;flex-wrap:wrap;gap:8px"><h2 style="margin:0;flex:1">Check faces <span class="muted small">${toCheck ? `${toCheck} to check` : ''}</span></h2>
      <select id="fc-show"><option value="check">To check</option><option value="done">Confirmed</option><option value="unknown">Not named</option><option value="all">All faces</option></select>
      <select id="fc-who"><option value="">Everyone</option>${names.map(n => `<option ${n === v.who ? 'selected' : ''}>${esc(n)}</option>`).join('')}</select>
      <select id="fc-days">${[[1, 'Today and yesterday'], [7, 'Last 7 days'], [30, 'Last 30 days'], [90, 'Last 90 days']].map(([d, t]) => `<option value="${d}" ${d === v.days ? 'selected' : ''}>${t}</option>`).join('')}</select>
      ${v.show === 'check' && shown.length ? `<button type="button" class="op-only" id="fc-all" title="Confirm every face shown here is who GAZE says">✓ All ${shown.length} are right</button>` : ''}</div>
    <p class="muted" style="margin-top:0">Every face GAZE found in alerts. <b>✓ Right</b> adds the face to that person, so GAZE knows them better; <b>✎ Wrong?</b> says who it really is (or nobody you know), and GAZE won't make that mistake again. Click a face to open its alert.</p>
    ${shown.length ? '' : `<div class="hint">${v.show === 'check' ? 'Nothing to check — every named face here has been confirmed.' : 'No faces for these choices.'}</div>`}
    <div class="fc-grid">${shown.slice(0, 600).map((f, i) => `<div class="fc ${f.confirmed ? 'fc-done' : ''}" data-i="${i}">
        <a href="#alerts?open=${enc(f.camera)}~${enc(f.event)}" title="Open the alert"><img src="${esc(f.picture)}" alt="" loading="lazy"></a>
        <b>${f.name ? esc(f.name) : '<span class="muted">Not named</span>'}</b>
        <div class="muted small">${esc(f.camera_name)} · ${esc(fmtAgo(f.time))}</div>
        <div class="row op-only">${f.name && !f.confirmed ? '<button type="button" class="small fc-ok">✓ Right</button>' : f.confirmed ? '<span class="muted small">✓ confirmed</span>' : ''}
          <select class="small fc-fix"><option value="">${f.name ? '✎ Wrong?' : 'Who is this?'}</option>${people.filter(p => p.name !== f.name).map(p => `<option value="${esc(p.id)}">${f.name ? "It's " : ''}${esc(p.name)}</option>`).join('')}<option value="+">New person…</option>${f.name ? '<option value="none">Not anyone I know</option>' : ''}</select></div></div>`).join('')}</div>
    ${shown.length > 600 ? `<div class="hint">Showing the newest 600 of ${shown.length}: choose fewer days or one person to see the rest.</div>` : ''}</div>`;
  $('fc-show').value = v.show;
  const redraw = () => renderFaces(body);
  $('fc-show').onchange = () => { v.show = val('fc-show'); store.set('faces-show', v.show); redraw(); };
  $('fc-who').onchange = () => { v.who = val('fc-who'); store.set('faces-who', v.who); redraw(); };
  $('fc-days').onchange = () => { v.days = +val('fc-days'); store.set('faces-days', v.days); redraw(); };
  const learn = (f, id) => api(`api/known/${enc(id)}/learn`, {method: 'POST', json: {camera: f.camera, event: f.event, face: f.face}});
  const idOf = (name) => (people.find(p => p.name === name) || {}).id;
  body.querySelectorAll('.fc').forEach(el => {
    const f = shown[+el.dataset.i];
    const ok = el.querySelector('.fc-ok');
    if (ok) ok.onclick = async () => {
      const id = idOf(f.name); if (!id) { toast(`${f.name} isn't in People & vehicles any more`, true); return; }
      try { await learn(f, id); f.confirmed = true; el.classList.add('fc-done'); ok.outerHTML = '<span class="muted small">✓ confirmed</span>'; if (v.show === 'check') setTimeout(() => el.remove(), 400); }
      catch (e) { toast(e.message, true); }
    };
    el.querySelector('.fc-fix').onchange = async (ev) => {
      let id = ev.target.value; if (!id) return;
      if (id === '+') {
        const name = (prompt('Name of this person:') || '').trim(); if (!name) { ev.target.value = ''; return; }
        try { id = (await api('api/known', {method: 'POST', json: {kind: 'person', name}})).id; } catch (e) { toast(e.message, true); return; }
      }
      try { const r = await learn(f, id); toast(r.message); await loadKnownPeople().catch(() => {}); redraw(); } catch (e) { toast(e.message, true); }
    };
  });
  const all1 = $('fc-all');
  if (all1) all1.onclick = async () => {
    if (!confirm(`Confirm all ${shown.length} faces shown are who GAZE says? Each is added to that person's faces.`)) return;
    all1.disabled = true; let n = 0;
    for (const f of shown) { const id = idOf(f.name); if (!id) continue; try { await learn(f, id); n++; all1.textContent = `✓ ${n} of ${shown.length}…`; } catch (_) {} }
    toast(`Saved — ${n} face${n === 1 ? '' : 's'} confirmed`); redraw();
  };
}

async function renderKnown(body) {
  const [list, inbox, seen] = await Promise.all([api('api/known').catch(() => []), api('api/known-inbox').catch(() => ({faces: [], plates: []})), api('api/known-seen').catch(() => ({}))]);
  const a = settings.ai;
  const people = list.filter(k => k.kind === 'person'), vehicles = list.filter(k => k.kind === 'vehicle');
  const pick = (kind, i) => `<select class="ib-who" data-kind="${kind}" data-i="${i}"><option value="">${kind === 'face' ? 'Who is this?' : 'Whose vehicle?'}</option>${(kind === 'face' ? people : vehicles).map(k => `<option value="${esc(k.id)}">${esc(k.name)}</option>`).join('')}<option value="+">${kind === 'face' ? 'New person…' : 'New vehicle…'}</option></select>`;
  const inboxCard = `<div class="card"><h2>New faces &amp; number plates <span class="muted small">${inbox.faces.length + inbox.plates.length ? `(${inbox.faces.length + inbox.plates.length})` : ''}</span></h2>
    <p class="muted" style="margin-top:0">Faces GAZE couldn't name and number plates of no known vehicle, from the last 30 days. The same person seen in several alerts is shown once. Choose who it is and GAZE names all those alerts and learns the face; or hide passers-by.</p>
    ${inbox.faces.length + inbox.plates.length ? '' : `<div class="hint">${a.faces || a.plates ? 'Nothing new. New faces and plates appear here as alerts come in.' : 'Turn on faces and/or number plates below first.'}</div>`}
    <div class="ib-grid">${inbox.faces.map((g, i) => `<div class="ib"><img src="${esc(g.picture)}" alt=""><div class="muted small">${g.count} alert${g.count === 1 ? '' : 's'} · ${fmtAgo(g.last)}</div>${pick('face', i)}<button type="button" class="small ib-hide" data-kind="face" data-i="${i}">Hide</button></div>`).join('')}
    ${inbox.plates.map((g, i) => `<div class="ib">${g.picture ? `<img src="${esc(g.picture)}" alt="" class="wide">` : ''}<span class="plate">${esc(g.plate)}</span><div class="muted small">${g.count} alert${g.count === 1 ? '' : 's'} · ${fmtAgo(g.last)}</div>${pick('plate', i)}<button type="button" class="small ib-hide" data-kind="plate" data-i="${i}">Hide</button></div>`).join('')}</div></div>`;
  body.innerHTML = `<div class="card"><h2>People &amp; vehicles</h2>
    <p class="muted" style="margin-top:0">Teach GAZE who and what it sees. Alerts then say “Recognized Mum” or “Plate ABC123 — Dad's car”, and you can search for them. Everything runs on this computer.</p>
    <div class="form">
      <span></span><div><label class="check"><input type="checkbox" id="k-faces" ${a.faces ? 'checked' : ''}> Look for faces in alerts and name the people below${tip('Runs on this computer (about 40 MB downloaded once; nothing is sent anywhere). Faces need to be fairly large and facing the camera, so it works best on door and entrance cameras. Example: alerts say “Recognized Sam”, and you can filter alerts by person.')}</label>
        <div class="hint">Works best on door and entrance cameras, where faces are close.</div></div>
      <span></span><div><label class="check"><input type="checkbox" id="k-plates" ${a.plates ? 'checked' : ''}> Read number plates of vehicles in alerts${tip('Runs on this computer (about 10 MB downloaded once). The plate must be readable in the full-quality video, so it suits a driveway or gate camera. Name a plate (e.g. “Sam’s car”) and alerts say whose car arrived.')}</label>
        <div class="hint">The plate must be readable in the full-quality video.</div></div>
    </div><div class="actions"><button type="button" class="primary" id="k-save-ai">Save</button></div></div>
    ${inboxCard}
    <div class="card"><div class="row" style="margin-bottom:8px"><h2 style="margin:0;flex:1">Known people and vehicles</h2>
      <button type="button" id="k-add-person">＋ Person</button><button type="button" id="k-add-vehicle">＋ Vehicle</button></div>
      ${list.length ? '' : '<div class="hint">Nobody yet. Add a person and upload a few clear photos of their face, or a vehicle with its number plate — or name faces straight from an alert (“Who is this?”).</div>'}
      <div class="known-grid">${list.map(k => `<div class="known" data-id="${esc(k.id)}">
        <div class="row"><b style="flex:1">${k.kind === 'person' ? '🧑' : '🚗'} ${esc(k.name)}</b><button type="button" class="small k-ren">Rename</button><button type="button" class="small danger k-del">Delete</button></div>
        <div class="k-photos">${k.photos.map(p => `<img src="api/known/${enc(k.id)}/photo/${enc(p)}" alt="">`).join('')}
          <label class="k-add btn small">＋ Photo<input type="file" accept="image/*" multiple hidden></label></div>
        <div class="hint">${k.kind === 'person' ? `${k.faces} face${k.faces === 1 ? '' : 's'} learned${k.faces < 3 ? ' — 3 or more clear photos work best' : ''}`
          : `Add photos of the vehicle: GAZE reads its plate from them.<br>Plates: <input type="text" class="k-plates" value="${esc(k.plates.join(', '))}" placeholder="ABC123, …" style="width:180px"> <button type="button" class="small k-psave">Save</button>${k.looks ? ` · ${k.looks} look${k.looks === 1 ? '' : 's'} learned` : ''}`}</div>
        ${k.kind === 'person' && (seen[k.name] || []).length ? `<details class="k-seen"><summary>Recognised as ${esc(k.name)} in alerts (${seen[k.name].length}${seen[k.name].length >= 30 ? '+' : ''}, last 30 days) — check for mistakes</summary>
          <div class="k-seen-grid">${seen[k.name].map((f, i) => `<div class="k-seen-f" data-i="${i}"><img src="${esc(f.picture)}" alt="" loading="lazy" title="${esc(fmtDate(f.time))}"><div class="muted small">${esc(fmtAgo(f.time))}</div>
            <select class="k-fix op-only"><option value="">✎ Wrong?</option>${people.filter(p => p.id !== k.id).map(p => `<option value="${esc(p.id)}">It's ${esc(p.name)}</option>`).join('')}<option value="+">New person…</option><option value="none">Not anyone I know</option></select></div>`).join('')}</div></details>` : ''}
      </div>`).join('')}</div></div>`;
  $('k-save-ai').onclick = async () => { await saveSettings({ai: {...settings.ai, api_key: '', faces: chk('k-faces'), plates: chk('k-plates')}}, 'Saved'); };
  const add = async (kind) => {
    const name = (prompt(kind === 'person' ? 'Name of the person:' : 'Name of the vehicle (e.g. Dad’s car):') || '').trim(); if (!name) return;
    const plates = kind === 'vehicle' ? (prompt('Number plate (optional):') || '').split(',').map(x => x.trim()).filter(Boolean) : [];
    try { await api('api/known', {method: 'POST', json: {kind, name, plates}}); renderKnown(body); } catch (e) { toast(e.message, true); }
  };
  // New faces & plates: assign or hide.
  body.querySelectorAll('.ib-who').forEach(sel => sel.onchange = async () => {
    let id = sel.value; if (!id) return;
    const face = sel.dataset.kind === 'face', g = (face ? inbox.faces : inbox.plates)[+sel.dataset.i];
    if (id === '+') {
      const name = (prompt(face ? 'Name of this person:' : 'Name of the vehicle (e.g. Dad’s car):') || '').trim();
      if (!name) { sel.value = ''; return; }
      try { id = (await api('api/known', {method: 'POST', json: {kind: face ? 'person' : 'vehicle', name, plates: []}})).id; } catch (e) { toast(e.message, true); return; }
    }
    try {
      const r = await api('api/known-inbox/assign', {method: 'POST', json: face ? {known: id, members: g.members} : {known: id, plate: g.plate}});
      toast(r.message); renderKnown(body);
    } catch (e) { toast(e.message, true); }
  });
  body.querySelectorAll('.ib-hide').forEach(b => b.onclick = async () => {
    const face = b.dataset.kind === 'face', g = (face ? inbox.faces : inbox.plates)[+b.dataset.i];
    try { await api('api/known-inbox/dismiss', {method: 'POST', json: face ? {members: g.members} : {plate: g.plate}}); renderKnown(body); } catch (e) { toast(e.message, true); }
  });
  $('k-add-person').onclick = () => add('person');
  $('k-add-vehicle').onclick = () => add('vehicle');
  // A face wrongly taken for this person: name it right (or "not anyone I
  // know"); GAZE stops taking that face for them.
  body.querySelectorAll('.known').forEach(el => {
    const k = list.find(x => x.id === el.dataset.id);
    el.querySelectorAll('.k-seen-f').forEach(tile => {
      const sel = tile.querySelector('.k-fix'), f = seen[k.name][+tile.dataset.i];
      sel.onchange = async () => {
        let to = sel.value; if (!to) return;
        if (to === '+') {
          const name = (prompt('Name of this person:') || '').trim(); if (!name) { sel.value = ''; return; }
          try { to = (await api('api/known', {method: 'POST', json: {kind: 'person', name}})).id; } catch (e) { toast(e.message, true); sel.value = ''; return; }
        }
        try {
          const r = await api(`api/known/${enc(to)}/learn`, {method: 'POST', json: {camera: f.camera, event: f.event, face: f.face}});
          toast(r.message);
          const d = el.querySelector('.k-seen'); const open = d && d.open;
          await renderKnown(body);
          // Keep this person's list open to carry on checking.
          const again = body.querySelector(`.known[data-id="${CSS.escape(k.id)}"] .k-seen`); if (again && open) again.open = true;
        } catch (e) { toast(e.message, true); sel.value = ''; }
      };
    });
  });
  body.querySelectorAll('.known').forEach(el => {
    const id = el.dataset.id;
    el.querySelector('.k-del').onclick = async () => { if (confirm('Forget this ' + (el.querySelector('b').textContent.trim()) + '?')) { await api(`api/known/${enc(id)}`, {method: 'DELETE'}).catch(e => toast(e.message, true)); renderKnown(body); } };
    el.querySelector('.k-ren').onclick = async () => { const n = prompt('New name:'); if (n) { await api(`api/known/${enc(id)}`, {method: 'PUT', json: {name: n}}).catch(e => toast(e.message, true)); renderKnown(body); } };
    const ps = el.querySelector('.k-psave');
    if (ps) ps.onclick = async () => { await api(`api/known/${enc(id)}`, {method: 'PUT', json: {plates: el.querySelector('.k-plates').value.split(',').map(x => x.trim()).filter(Boolean)}}).then(() => toast('Plates saved')).catch(e => toast(e.message, true)); };
    el.querySelector('.k-add input').onchange = async (e) => {
      for (const f of e.target.files) {
        toast('Learning from ' + f.name + '… (the first time downloads the models)');
        try {
          const jpg = await photoToJpeg(f);
          const r = await fetch(`api/known/${enc(id)}/photo`, {method: 'POST', headers: {'Content-Type': 'image/jpeg'}, body: jpg});
          const j = await r.json(); if (!r.ok) throw new Error(j.error || r.statusText);
          toast(j.message);
        } catch (err) { toast(`${f.name}: ${err.message}`, true); }
      }
      renderKnown(body);
    };
  });
}

// ---------- Settings → Notifications ----------
const NOTIFY_OBJECTS = [['person', 'People'], ['vehicle', 'Vehicles'], ['bicycle', 'Bicycles'], ['package', 'Packages'], ['dog', 'Dogs'], ['cat', 'Cats']];
function randomTopic() {
  const a = new Uint8Array(16); crypto.getRandomValues(a);
  return 'gaze-' + Array.from(a, b => 'abcdefghijkmnpqrstuvwxyz23456789'[b % 32]).join('');
}
// Home Assistant (MQTT) and webhooks.
// Settings → Voice & talking: the voice "Say…" speaks with.
function renderVoice(body) {
  const t = settings.tts || {voice: '', rate: 0, volume: 100, phrases: [], chime: false};
  const talkers = status.cameras.filter(c => c.talk);
  body.innerHTML = `<form class="card" id="f-tts"><h2>Voice &amp; talking${tip('Used by 💬 Say… on the live view of a camera with two-way audio: type words and the camera\u2019s speaker says them. The computer running GAZE makes the speech, so nothing is sent to the internet.')}</h2>
    <div class="form">
      <label for="v-voice">Voice</label><div><div class="row"><select id="v-voice" style="flex:1"><option value="">The computer's default voice</option>${t.voice ? `<option value="${esc(t.voice)}" selected>${esc(t.voice.replace(/^\w+:/, ''))}</option>` : ''}</select><span class="spinner small" id="v-load"></span></div>
        <div class="hint">More voices: download natural-sounding ones below (they run on this computer, nothing goes online), or on Windows add them in Settings → Time &amp; language → Speech (then restart GAZE).</div></div>
      <label>Download voices${tip('Free voices from the open-source Piper project, which run on this computer. The first one also downloads the voice engine (about 20 MB); each voice is 20\u2013110 MB.')}</label><div><div class="dl-voices" id="v-dl"><span class="muted small">Loading…</span></div></div>
      <label for="v-rate">Speed</label><div class="row"><input type="range" id="v-rate" min="-10" max="10" value="${t.rate || 0}" style="flex:1"><span class="muted" id="v-rate-v" style="width:70px"></span></div>
      <label for="v-vol">Loudness</label><div class="row"><input type="range" id="v-vol" min="10" max="100" step="5" value="${t.volume || 100}" style="flex:1"><span class="muted" id="v-vol-v" style="width:70px"></span></div>
      <span></span><label class="check"><input type="checkbox" id="v-chime" ${t.chime ? 'checked' : ''}> Play a short chime first, to get attention</label>
      <label for="v-phr">Ready phrases${tip('Shown as buttons under the text box of Say…, one per line.')}</label><div><textarea id="v-phr" rows="5">${esc((t.phrases || []).join('\n'))}</textarea></div>
      <label for="v-try">Try it</label><div><div class="row"><input type="text" id="v-try" value="Hello, this is GAZE." maxlength="400" style="flex:1"><button type="button" id="v-hear">▶ Hear it here</button>${talkers.length ? `<select id="v-cam">${talkers.map(c => `<option value="${esc(c.id)}">${esc(c.name)}</option>`).join('')}</select><button type="button" id="v-say">🔊 Say on camera</button>` : ''}</div>
        <div class="hint" id="v-msg">${talkers.length ? 'Say on camera uses the saved settings: save first.' : 'No camera has two-way audio turned on (camera settings → Audio).'}</div></div>
      <h3>🚨 Siren${tip('The 🚨 Siren button on the live view of a camera with two-way audio: a loud wailing alarm through the camera\u2019s speaker, with the warnings below spoken in between, to scare off an intruder. It stops after the time set here, or when you press Stop siren.')}</h3>
      <label for="v-sir-s">Sounds for</label><div class="row"><input type="number" id="v-sir-s" min="5" max="300" value="${t.siren_seconds || 30}" style="width:90px"><span class="muted">seconds (or until stopped)</span></div>
      <span></span><label class="check"><input type="checkbox" id="v-sir-voice" ${t.siren_voice !== false ? 'checked' : ''}> Speak warnings between the wails (in the voice above)</label>
      <label for="v-sir-msg">Warnings${tip('Spoken in turn, one between each wail. One per line.')}</label><div><textarea id="v-sir-msg" rows="3">${esc((t.siren_messages || []).join('\n'))}</textarea>
        <div class="chips" style="margin-top:4px">${['Warning! You are being recorded. The homeowner has been notified.', 'Leave the property now. The police have been notified.', 'Security has been alerted. You are on camera.', 'This is private property. Leave immediately.'].map(m => `<button type="button" class="small v-sir-add">${esc(m)}</button>`).join('')}</div></div>
      <label>Try it</label><div class="row"><button type="button" id="v-sir-hear">▶ Hear the siren here</button><span class="hint" style="margin:0">a few seconds, at a sensible volume</span></div>
    </div>
    <div class="actions"><span class="spacer"></span><button type="submit" class="primary">Save</button></div></form>`;
  const rateTxt = (v) => +v === 0 ? 'normal' : (+v < 0 ? 'slower ' : 'faster ') + Math.abs(v);
  const paint = () => { $('v-rate-v').textContent = rateTxt(val('v-rate')); $('v-vol-v').textContent = val('v-vol') + '%'; };
  $('v-rate').oninput = $('v-vol').oninput = paint; paint();
  const loadVoices = () => api('api/tts/voices').then(list => {
    const sel = $('v-voice'), cur = sel ? sel.value || t.voice : t.voice;
    if (!sel) return;
    sel.innerHTML = '<option value="">The computer\'s default voice</option>' + list.map(v => `<option value="${esc(v.id)}" ${v.id === cur ? 'selected' : ''}>${esc(v.name)}${v.lang ? ' — ' + esc(v.lang) : ''}</option>`).join('')
      + (cur && !list.some(v => v.id === cur) ? `<option value="${esc(cur)}" selected>${esc(cur.replace(/^\w+:/, ''))} (not found)</option>` : '');
    if (!list.length) $('v-msg').textContent = 'No voices found on this computer — the default one is used.';
  }).catch(() => {}).finally(() => { const l = $('v-load'); if (l) l.remove(); });
  loadVoices();
  // Downloadable voices: download / delete, with progress on the Tasks page.
  let dlPoll = null;
  const paintDl = (list) => {
    const box = $('v-dl'); if (!box) return clearInterval(dlPoll);
    box.innerHTML = list.map(v => `<div class="dl-voice"><span>${esc(v.name)} <span class="muted small">${esc(v.lang)}</span></span>${v.downloading ? '<span class="muted small"><span class="spinner small"></span> downloading…</span>'
      : v.installed ? `<button type="button" class="small" data-use="${esc(v.id)}">Use</button><button type="button" class="small danger" data-del="${esc(v.id)}">Delete</button>` : `<button type="button" class="small" data-dl="${esc(v.id)}">⇩ Download</button>`}</div>`).join('');
    box.querySelectorAll('[data-dl]').forEach(b => b.onclick = async () => { await api(`api/tts/downloads/${enc(b.dataset.dl)}`, {method: 'POST'}).catch(e => toast(e.message, true)); refreshDl(); });
    box.querySelectorAll('[data-del]').forEach(b => b.onclick = async () => { if (!confirm('Delete this voice?')) return; await api(`api/tts/downloads/${enc(b.dataset.del)}`, {method: 'DELETE'}).catch(e => toast(e.message, true)); refreshDl(); loadVoices(); });
    box.querySelectorAll('[data-use]').forEach(b => b.onclick = () => { const sel = $('v-voice'), id = 'piper:' + b.dataset.use; if (![...sel.options].some(o => o.value === id)) sel.insertAdjacentHTML('beforeend', `<option value="${esc(id)}">${esc(id)}</option>`); sel.value = id; toast('Chosen — press Save to keep it'); });
  };
  let wasBusy = false;
  const refreshDl = () => api('api/tts/downloads').then(list => {
    paintDl(list);
    const busy = list.some(v => v.downloading);
    if (wasBusy && !busy) loadVoices();
    wasBusy = busy;
    clearInterval(dlPoll); if (busy) dlPoll = setInterval(refreshDl, 2000);
  }).catch(() => { const box = $('v-dl'); if (box) box.innerHTML = '<span class="muted small">Not available</span>'; });
  refreshDl();
  const form = () => ({voice: val('v-voice'), rate: +val('v-rate'), volume: +val('v-vol'), chime: chk('v-chime'), phrases: val('v-phr').split('\n').map(x => x.trim()).filter(Boolean),
    siren_seconds: Math.max(5, Math.min(300, Math.round(+val('v-sir-s')) || 30)), siren_voice: chk('v-sir-voice'), siren_messages: val('v-sir-msg').split('\n').map(x => x.trim()).filter(Boolean)});
  body.querySelectorAll('.v-sir-add').forEach(b => b.onclick = () => { const ta = $('v-sir-msg'); if (!ta.value.includes(b.textContent)) ta.value = (ta.value.trim() ? ta.value.trim() + '\n' : '') + b.textContent; });
  $('v-sir-hear').onclick = async () => {
    const b = $('v-sir-hear'); b.disabled = true; msg('Making the siren…');
    try {
      const r = await fetch('api/tts/siren-preview', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(form())});
      if (!r.ok) throw new Error((await r.text()) || r.statusText);
      const a = new Audio(URL.createObjectURL(await r.blob())); a.volume = 0.35; await a.play(); msg('Playing the siren (quieter here than on the camera).');
    } catch (e) { msg(e.message, true); } finally { b.disabled = false; }
  };
  const msg = (m, bad) => { $('v-msg').textContent = m; $('v-msg').className = 'hint' + (bad ? ' err' : ''); };
  $('v-hear').onclick = async () => {
    const b = $('v-hear'); b.disabled = true; msg('Making the speech…');
    try {
      const r = await fetch('api/tts/preview', {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify({...form(), text: val('v-try')})});
      if (!r.ok) throw new Error((await r.text()) || r.statusText);
      const a = new Audio(URL.createObjectURL(await r.blob())); await a.play(); msg('Playing — this is how the camera\u2019s speaker gets it.');
    } catch (e) { msg(e.message, true); } finally { b.disabled = false; }
  };
  if ($('v-say')) $('v-say').onclick = async () => {
    const b = $('v-say'); b.disabled = true; msg('Saying it on the camera…');
    try { await api(`api/cameras/${enc(val('v-cam'))}/say`, {method: 'POST', json: {text: val('v-try')}}); msg('✓ Said'); }
    catch (e) { msg(e.message, true); } finally { b.disabled = false; }
  };
  $('f-tts').onsubmit = (e) => { e.preventDefault(); saveSettings({tts: form()}, 'Voice settings saved'); };
}
function renderSmartHome(body) {
  const q = settings.mqtt || {}, st = q.status || {};
  const hooks = JSON.parse(JSON.stringify(settings.webhooks || []));
  const objs = [...new Set(['person', 'vehicle', 'bicycle', 'dog', 'cat', 'package', ...status.cameras.flatMap(c => c.watch || [])]), 'motion'];
  body.innerHTML = `<form class="card" id="f-mqtt"><h2>Home Assistant (MQTT)${tip('MQTT is how home automation systems talk to each other. Home Assistant gets a sensor for motion and for each object on every camera, the last alert picture, an \u201calert\u201d event, whether each camera is online, and a Home / Away / Night switch. Example automation: turn the porch light on when Front Door sees a person after dark.')}</h2>
    <p class="muted" style="margin-top:0">${st.connected ? '<span class="okc">● Connected</span>' : q.enabled ? `<span class="errc">● Not connected${st.error ? ': ' + esc(st.error) : ''}</span>` : 'Off.'} In Home Assistant, install the <b>Mosquitto broker</b> add-on and the MQTT integration, then enter its address here.</p>
    <div class="form">
      <span></span><label class="check"><input type="checkbox" id="q-on" ${q.enabled ? 'checked' : ''}> Connect to an MQTT broker</label>
      <label for="q-host">Broker address${tip('The computer running the MQTT broker \u2014 usually your Home Assistant box. Example: 192.168.1.20 or homeassistant.local. Port 1883 is the usual one.')}</label><div class="row"><input type="text" id="q-host" value="${esc(q.host || '')}" placeholder="e.g. homeassistant.local" style="flex:3"><input type="number" id="q-port" value="${q.port || 1883}" min="1" max="65535" style="flex:1" title="Port (1883)"></div>
      <label for="q-user">Login</label><div class="row"><input type="text" id="q-user" value="${esc(q.username || '')}" placeholder="user name" style="flex:1" autocomplete="off"><input type="password" id="q-pass" placeholder="${q.has_password ? 'Saved – leave empty to keep' : 'password'}" style="flex:1" autocomplete="new-password"></div>
      <label for="q-prefix">Topic prefix${tip('Every message GAZE sends starts with this, e.g. gaze/front_door/person. Only change it if you run more than one GAZE.')}</label><div><input type="text" id="q-prefix" value="${esc(q.prefix || 'gaze')}" style="width:160px"></div>
      <span></span><label class="check"><input type="checkbox" id="q-disc" ${q.discovery !== false ? 'checked' : ''}> Add everything to Home Assistant automatically${tip('Home Assistant \u201cdiscovery\u201d: each camera appears as a device with its sensors, without writing any YAML.')}</label>
    </div><div class="actions"><button class="primary">Save</button></div></form>
  <div class="card"><h2>Webhooks${tip('A webhook sends each new alert to a web address as JSON, so other services can act on it: IFTTT, Node-RED, Zapier, Make, a smart plug, or your own script. Example: an IFTTT applet that flashes your smart lights when a person is at the door.')}</h2>
    <p class="muted" style="margin-top:0">Each new alert is sent to these addresses (POST, JSON: camera, what was seen, time, notes, plates, names).</p>
    <div id="wh-list"></div>
    <div class="actions"><button type="button" id="wh-add">＋ Add a webhook</button><span class="spacer"></span><button type="button" class="primary" id="wh-save">Save webhooks</button></div></div>`;
  const draw = () => {
    $('wh-list').innerHTML = hooks.length ? hooks.map((h, i) => `<div class="wh card" data-i="${i}" style="margin:0 0 10px;padding:12px">
      <div class="row"><label class="check" style="margin:0"><input type="checkbox" class="wh-on" ${h.enabled ? 'checked' : ''}></label><input type="text" class="wh-name" value="${esc(h.name)}" placeholder="Name, e.g. Porch light" style="width:200px"><input type="text" class="wh-url" inputmode="url" value="${esc(h.url)}" placeholder="https://…" style="flex:1"><button type="button" class="small wh-test">Test</button><button type="button" class="small danger wh-del">Delete</button></div>
      <div class="chips" style="margin-top:8px"><span class="muted small">Cameras:</span>${status.cameras.map(c => `<label><input type="checkbox" class="wh-cam" value="${esc(c.id)}" ${!h.cameras.length || h.cameras.includes(c.id) ? 'checked' : ''}>${esc(c.name)}</label>`).join('')}</div>
      <div class="chips" style="margin-top:6px"><span class="muted small">For:</span>${objs.map(o => `<label><input type="checkbox" class="wh-obj" value="${o}" ${!h.objects.length || h.objects.includes(o) ? 'checked' : ''}>${labelIcon(o)} ${o}</label>`).join('')}</div>
      <label class="check" style="margin-top:6px"><input type="checkbox" class="wh-pic" ${h.picture ? 'checked' : ''}> Include the alert picture</label></div>`).join('') : '<div class="hint">No webhooks yet.</div>';
    $('wh-list').querySelectorAll('.wh').forEach(el => {
      const h = hooks[+el.dataset.i];
      const read = () => {
        const cams = [...el.querySelectorAll('.wh-cam')], os = [...el.querySelectorAll('.wh-obj')];
        Object.assign(h, {enabled: el.querySelector('.wh-on').checked, name: el.querySelector('.wh-name').value.trim(), url: el.querySelector('.wh-url').value.trim(), picture: el.querySelector('.wh-pic').checked,
          cameras: cams.every(x => x.checked) ? [] : cams.filter(x => x.checked).map(x => x.value), objects: os.every(x => x.checked) ? [] : os.filter(x => x.checked).map(x => x.value)});
      };
      el.addEventListener('change', read); el.addEventListener('input', read);
      el.querySelector('.wh-del').onclick = () => { hooks.splice(+el.dataset.i, 1); draw(); };
      el.querySelector('.wh-test').onclick = async () => { read(); try { const r = await api('api/webhooks/test', {method: 'POST', json: h}); toast(r.message); } catch (e) { toast(e.message, true); } };
    });
  };
  draw();
  // The overview: every camera in one picture, with a private link.
  body.insertAdjacentHTML('beforeend', `<div class="card"><h2>Overview of all cameras${tip('Every camera in one moving picture, made from what GAZE already decodes \u2014 almost no extra work. Handy for a wall tablet, a slow phone connection, or a Home Assistant dashboard (add it as an MJPEG camera with the link below). \u201cOnly cameras with something going on\u201d shows just the busy ones (full size when it\u2019s only one), and every camera when all is quiet.')}</h2>
    <div class="surface" style="aspect-ratio:16/9;max-width:640px"><img id="ov-img" alt="" style="object-fit:contain"></div>
    <label class="check" style="margin-top:10px"><input type="checkbox" id="ov-active"> Only cameras with something going on</label>
    <label style="margin-top:10px;display:block">Private link (works without signing in — keep it to yourself)</label>
    <div class="row"><input type="text" id="ov-link" readonly style="flex:1"><button type="button" id="ov-copy">📋 Copy</button><button type="button" id="ov-new" title="Make a new link: the old one stops working">New link</button></div>
    <div class="hint">Open it on a tablet, or in Home Assistant: Settings → Devices → Add integration → MJPEG IP Camera, and paste it as the MJPEG URL.</div>
    <h3 style="margin:18px 0 4px">Each camera on its own</h3>
    <div class="hint" style="margin-top:0">A private link to one camera, with its own key: it opens that camera's picture and nothing else. “New link” above also replaces these.</div>
    <div id="ov-cams" class="ov-cams"></div></div>`);
  let ovKey = '', ovCamKeys = {};
  // One camera's link: which stream, how smooth, moving or a still picture.
  const ovCams = () => {
    const box = $('ov-cams'); if (!box) return;
    const base = location.origin + location.pathname.replace(/[^/]*$/, '');
    const cams = cameraCfgs.filter(c => ovCamKeys[c.id]);
    if (!box.children.length) box.innerHTML = cams.map(c => `<div class="ov-cam" data-id="${esc(c.id)}">
        <div class="oc-pic" title="Open the link"><img alt=""></div>
        <b>${esc(c.name)}</b>
        <select class="oc-q" title="Which stream">${(c.live_url || '').trim() ? '<option value="sub">Low resolution (sub-stream)</option>' : ''}<option value="main">Full quality (main stream)</option></select>
        <select class="oc-fps" title="Pictures per second">${[[0, 'Live view rate'], [1, '1 fps'], [2, '2 fps'], [5, '5 fps'], [10, '10 fps'], [15, '15 fps']].map(([v, t]) => `<option value="${v}">${t}</option>`).join('')}</select>
        <select class="oc-kind" title="Moving picture or one picture"><option value="stream.mjpeg">Moving picture (MJPEG)</option><option value="frame.jpg">Still picture (JPEG, newest)</option></select>
        <div class="row"><input type="text" class="oc-link" readonly style="flex:1"><button type="button" class="oc-copy">📋 Copy</button><a class="btn oc-open" target="_blank" rel="noopener">Open</a></div></div>`).join('') || '<div class="muted">No cameras yet.</div>';
    box.querySelectorAll('.ov-cam').forEach(el => {
      const id = el.dataset.id, kind = el.querySelector('.oc-kind'), fps = el.querySelector('.oc-fps');
      const make = () => {
        fps.disabled = kind.value !== 'stream.mjpeg';
        const q = el.querySelector('.oc-q').value, f = +fps.value;
        const url = `${base}live/${enc(id)}/${kind.value}?k=${ovCamKeys[id]}&q=${q}${f && kind.value === 'stream.mjpeg' ? '&fps=' + f : ''}`;
        el.querySelector('.oc-link').value = url; el.querySelector('.oc-open').href = url;
      };
      el.querySelectorAll('select').forEach(x => x.onchange = () => { make(); el._pic(); }); make();
      // A preview of what the link shows (a still every 2 s, on screen only).
      const img = el.querySelector('.oc-pic img');
      el._pic = () => { img.src = `live/${enc(id)}/frame.jpg?q=${el.querySelector('.oc-q').value}&t=${Date.now()}`; };
      el.querySelector('.oc-pic').onclick = () => window.open(el.querySelector('.oc-link').value, '_blank', 'noopener');
      el.querySelector('.oc-copy').onclick = () => navigator.clipboard && navigator.clipboard.writeText(el.querySelector('.oc-link').value).then(() => toast('Link copied')).catch(() => el.querySelector('.oc-link').select());
    });
    // Refresh the previews that are on screen.
    if (!box._seen) {
      const seen = box._seen = new Set();
      const io = new IntersectionObserver(es => es.forEach(e => { if (e.isIntersecting) { seen.add(e.target); e.target._pic(); } else seen.delete(e.target); }));
      box.querySelectorAll('.ov-cam').forEach(el => io.observe(el));
      const tick = () => {
        if (!box.isConnected || box._seen !== seen) { io.disconnect(); return; }
        if (!document.hidden) seen.forEach(el => el._pic());
        setTimeout(tick, 2000);
      };
      setTimeout(tick, 2000);
    }
  };
  const ovLink = () => { const base = location.origin + location.pathname.replace(/[^/]*$/, ''); $('ov-link').value = `${base}overview.mjpeg?k=${ovKey}${chk('ov-active') ? '&active=true' : ''}`; };
  api('api/overview/key').then(r => { ovKey = r.key; ovCamKeys = r.cameras || {}; ovLink(); ovCams(); }).catch(() => {});
  const ovPic = () => { if (!$('ov-img') || !$('ov-img').isConnected) return; $('ov-img').src = `overview.jpg?w=960${chk('ov-active') ? '&active=true' : ''}&t=${Date.now()}`; setTimeout(ovPic, 2000); };
  ovPic();
  $('ov-active').onchange = ovLink;
  $('ov-copy').onclick = () => navigator.clipboard && navigator.clipboard.writeText(val('ov-link')).then(() => toast('Link copied')).catch(() => { $('ov-link').select(); });
  $('ov-new').onclick = async () => { if (!confirm('Make a new link? The old one stops working (update it wherever you used it).')) return; const r = await api('api/overview/key', {method: 'POST'}); ovKey = r.key; ovCamKeys = r.cameras || {}; ovLink(); $('ov-cams').innerHTML = ''; $('ov-cams')._seen = null; ovCams(); toast('New links made'); };
  $('wh-add').onclick = () => { hooks.push({name: '', enabled: true, url: '', cameras: [], objects: [], picture: false}); draw(); };
  $('wh-save').onclick = () => saveSettings({webhooks: hooks}, 'Webhooks saved');
  $('f-mqtt').onsubmit = async (e) => {
    e.preventDefault();
    await saveSettings({mqtt: {enabled: chk('q-on'), host: val('q-host').trim(), port: Math.round(num('q-port')) || 1883, username: val('q-user').trim(), password: val('q-pass'), prefix: val('q-prefix').trim() || 'gaze', discovery: chk('q-disc')}}, 'Saved — connecting');
    setTimeout(async () => { await loadSettings().catch(() => {}); if (section === 'smarthome') renderSmartHome(body); }, 2500);
  };
}

async function renderNotify(body) {
  const n = settings.notify, st = await api('api/notify/status').catch(() => ({}));
  const sub = () => `${(val('n-server') || 'https://ntfy.sh').replace(/\/$/, '')}/${val('n-topic')}`;
  body.innerHTML = `<div class="card"><h2>Phone notifications</h2>
    <p class="muted" style="margin-top:0">GAZE sends the alert picture to your phone as soon as it sees someone, then a short summary of what happened. It uses <b>ntfy</b>, a free notification app (Android and iPhone).</p>
    <ol class="steps">
      <li>Install the <b>ntfy</b> app on your phone.</li>
      <li>In the app, tap <b>+</b> and subscribe to your topic: <code id="n-sub"></code> <button type="button" class="small" id="n-copy">Copy</button></li>
      <li>Switch notifications on below, save, and press <b>Send a test</b>.</li>
    </ol>
    <div class="form">
      <span></span><div><label class="check"><input type="checkbox" id="n-on" ${n.enabled ? 'checked' : ''}> Send notifications</label></div>
      <label for="n-topic">Topic${tip('Like a private channel name: your phone listens to it and GAZE posts to it. Anyone who knows the name can read it, so keep the long random one GAZE made.')}</label><div><div class="row"><input type="text" id="n-topic" value="${esc(n.topic || randomTopic())}" style="flex:1"><button type="button" id="n-new">New private topic</button></div>
        <div class="hint">On the public ntfy.sh server anyone who knows the topic name can read it, so keep this long random name.</div></div>
      <label for="n-server">ntfy server${tip('The service that passes notifications to your phone. Leave empty for the free public one (ntfy.sh). For full privacy you can run your own and put its address here.')}</label><div><input type="text" id="n-server" value="${esc(n.server)}" placeholder="https://ntfy.sh">
        <div class="hint">Pictures pass through this server. For full privacy run your own ntfy server and enter its address (and an access token below).</div></div>
      <label for="n-token">Access token${tip('A password for a protected topic, or for your own ntfy server. Not needed with the public server.')}</label><div><input type="password" id="n-token" autocomplete="new-password" placeholder="${n.has_token ? 'Saved – leave empty to keep' : 'Only for a protected topic or your own server'}"></div>
      <label>Notify for</label><div class="checks">${NOTIFY_OBJECTS.map(([k, t]) => `<label class="check"><input type="checkbox" class="n-obj" value="${k}" ${n.objects.includes(k) ? 'checked' : ''}> ${t}</label>`).join('')}</div>
      <label>Cameras</label><div class="checks">${status.cameras.map(c => `<label class="check"><input type="checkbox" class="n-cam" value="${esc(c.id)}" ${!n.cameras.length || n.cameras.includes(c.id) ? 'checked' : ''}> ${esc(c.name)}</label>`).join('')}</div>
      <label for="n-gap">How often${tip('So your phone doesn’t buzz constantly: after a notification, the same camera waits this long before sending another. 0 = every alert. Example: 5 minutes.')}</label><div class="row"><span class="muted">at most one every</span><input type="number" id="n-gap" min="0" max="240" value="${n.gap_minutes}" style="width:80px"><span class="muted">minutes per camera</span></div>
      <label for="n-nfrom">At night${tip('A different limit for night time, usually shorter, since someone outside at night matters more. Example: 23:00 to 06:00, one every 0 minutes = every alert at night.')}</label><div class="row"><input type="time" id="n-nfrom" value="${esc(n.night_from)}" style="width:130px"><span class="muted">to</span><input type="time" id="n-nto" value="${esc(n.night_to)}" style="width:130px"><span class="muted">: one every</span><input type="number" id="n-ngap" min="0" max="240" value="${n.night_gap_minutes}" style="width:80px"><span class="muted">minutes — people at night count as suspicious</span></div>
      <span></span><div><label class="check"><input type="checkbox" id="n-health" ${n.health !== false ? 'checked' : ''}> Warn me when something's wrong${tip('A notification when a camera goes offline for more than 2 minutes (and when it\u2019s back), when a camera stops recording, and when the recordings drive is nearly full \u2014 so a dead camera never goes unnoticed.')}</label>
        <div class="hint">Camera offline, recording stopped, drive nearly full.</div></div>
      <span></span><div><label class="check"><input type="checkbox" id="n-sum" ${n.summary ? 'checked' : ''}> Send a summary when the alert is over</label>
        <div class="hint">What was seen and for how long, who was recognised, number plates, zones and parcels.</div></div>
      <label for="n-link">Open GAZE at${tip('The address your phone uses to open GAZE when you tap a notification. Empty works on your home Wi-Fi. For away from home, use a remote-access address, e.g. your computer’s Tailscale name.')}</label><div><input type="text" id="n-link" value="${esc(n.link)}" placeholder="https://my-pc.tail1234.ts.net">
        <div class="hint">Tapping a notification opens that alert here. Empty: your home network address.</div></div>
    </div></div>
    <div class="card"><div class="actions" style="margin:0"><button type="button" class="primary" id="n-save">Save</button><button type="button" id="n-test">Send a test</button>
      <span class="muted" style="margin-left:auto">${st.sent ? `${st.sent} sent since GAZE started${st.last_sent ? ', last ' + fmtDate(st.last_sent) : ''}` : ''}${st.last_error ? ` <span class="errc">· ${esc(st.last_error)}</span>` : ''}</span></div></div>`;
  // Home / Away / Night: which cameras notify in each.
  const md = JSON.parse(JSON.stringify((status.mode && status.mode.modes) || {}));
  const modeKeys = ['home', 'away', 'night'];
  const camOn = (k, id) => !md[k] || !md[k].cameras || !md[k].cameras.length || md[k].cameras.includes(id);
  body.insertAdjacentHTML('beforeend', `<div class="card"><h2>Home, Away and Night${tip('Like an alarm system: switch modes with the button at the top of every page. Each mode chooses which cameras send phone notifications. Recording and alerts carry on in every mode \u2014 only the notifications change. Example: Home = only the driveway; Away = everything; Night = everything outside.')}</h2>
    <p class="muted" style="margin-top:0">Choose which cameras send notifications in each mode, then switch with the mode button at the top (now: <b>${esc((MODE_INFO[status.mode.current] || MODE_INFO.away)[1])}</b>).</p>
    <table class="modes"><thead><tr><th></th>${modeKeys.map(k => `<th>${MODE_INFO[k][0]} ${MODE_INFO[k][1]}</th>`).join('')}</tr></thead><tbody>
      <tr class="mode-on"><td><b>Notifications</b></td>${modeKeys.map(k => `<td><label class="check"><input type="checkbox" class="md-on" data-m="${k}" ${md[k] && md[k].notify ? 'checked' : ''}> On</label></td>`).join('')}</tr>
      ${status.cameras.map(c => `<tr><td>${esc(c.name)}</td>${modeKeys.map(k => `<td><input type="checkbox" class="md-cam" data-m="${k}" value="${esc(c.id)}" ${camOn(k, c.id) ? 'checked' : ''}></td>`).join('')}</tr>`).join('')}
    </tbody></table>
    <label class="check" style="margin-top:10px"><input type="checkbox" id="md-auto" ${md.auto_night ? 'checked' : ''}> While at Home, switch to Night by itself during the night hours above${tip('Uses the \u201cAt night\u201d hours set above. Example: Home all day, then Night from 23:00 to 06:00 without touching anything. Away stays Away.')}</label>
    <div class="actions"><button type="button" class="primary" id="md-save">Save modes</button></div></div>`);
  const syncOn = () => modeKeys.forEach(k => body.querySelectorAll(`.md-cam[data-m="${k}"]`).forEach(i => i.disabled = !body.querySelector(`.md-on[data-m="${k}"]`).checked));
  body.querySelectorAll('.md-on').forEach(i => i.onchange = syncOn); syncOn();
  $('md-save').onclick = async () => {
    const out = {current: status.mode.current, auto_night: chk('md-auto')};
    for (const k of modeKeys) {
      const all = [...body.querySelectorAll(`.md-cam[data-m="${k}"]`)], picked = all.filter(i => i.checked).map(i => i.value);
      // Every camera ticked = all cameras (also ones added later).
      out[k] = {notify: body.querySelector(`.md-on[data-m="${k}"]`).checked, cameras: picked.length === all.length ? [] : picked};
      if (out[k].notify && !picked.length) out[k].notify = false;
    }
    try { await api('api/modes', {method: 'PUT', json: out}); toast('Modes saved'); await refreshStatus(); } catch (e) { toast(e.message, true); }
  };
  const upd = () => { $('n-sub').textContent = sub(); };
  upd(); $('n-topic').oninput = upd; $('n-server').oninput = upd;
  $('n-new').onclick = () => { $('n-topic').value = randomTopic(); upd(); };
  $('n-copy').onclick = () => { navigator.clipboard && navigator.clipboard.writeText(sub()).then(() => toast('Copied')); };
  const form = () => {
    const cams = [...body.querySelectorAll('.n-cam')], picked = cams.filter(c => c.checked).map(c => c.value);
    return {enabled: chk('n-on'), server: val('n-server').trim() || 'https://ntfy.sh', topic: val('n-topic').trim(), token: val('n-token'),
      objects: [...body.querySelectorAll('.n-obj:checked')].map(c => c.value), cameras: picked.length === cams.length ? [] : picked,
      gap_minutes: Math.max(0, num('n-gap') | 0), night_from: val('n-nfrom') || '22:00', night_to: val('n-nto') || '06:00',
      night_gap_minutes: Math.max(0, num('n-ngap') | 0), summary: chk('n-sum'), health: chk('n-health'), link: val('n-link').trim()};
  };
  $('n-save').onclick = async () => {
    const f = form();
    if (f.enabled && !f.topic) return toast('Choose a topic first', true);
    if (await saveSettings({notify: f}, 'Notifications saved')) renderNotify(body);
  };
  $('n-test').onclick = async () => {
    try { toast((await api('api/notify/test', {method: 'POST', json: form()})).message); } catch (e) { toast(e.message, true); }
  };
}

// ---------- Settings → Users (admin) ----------
async function renderUsers(body) {
  if (!isAdmin()) { body.innerHTML = '<div class="card"><h2>Users</h2><p class="muted">Only an admin can manage users. Use the 👤 menu at the top to change your own password.</p></div>'; return; }
  const users = await api('api/users').catch(() => []);
  const cams = (status && status.cameras) || [];
  const roleSel = (cur) => `<select class="u-role">${[['admin', 'Admin — everything'], ['operator', 'Operator — watch, PTZ, alerts'], ['viewer', 'Viewer — watch only']].map(([k, t]) => `<option value="${k}" ${k === cur ? 'selected' : ''}>${t}</option>`).join('')}</select>`;
  const camPick = (sel) => `<div class="chips u-cams">${cams.map(c => `<label><input type="checkbox" value="${esc(c.id)}" ${sel.includes(c.id) ? 'checked' : ''}>${esc(c.name)}</label>`).join('')}</div>`;
  body.innerHTML = `<div class="card"><h2>Users</h2>
    <p class="muted" style="margin-top:0">Who can open GAZE and what they can do. <b>Admin</b>: everything. <b>Operator</b>: watch, move PTZ cameras, pause recording, manage alerts. <b>Viewer</b>: watch live view, alerts and recordings only. Tick cameras to limit someone to those (none ticked = all cameras).</p>
    <table class="users"><thead><tr><th>User</th><th>Access</th><th>Cameras</th><th></th></tr></thead><tbody>
    ${users.map(u => `<tr data-u="${esc(u.username)}"><td><b>${esc(u.username)}</b>${me && u.username === me.username ? ' <span class="muted">(you)</span>' : ''}</td><td>${roleSel(u.role)}</td><td>${camPick(u.cameras)}</td>
      <td style="text-align:right;white-space:nowrap"><button type="button" class="small u-save">Save</button> <button type="button" class="small u-pw">Set password</button> ${u.two_step ? '<button type="button" class="small u-2fa" title="Two-step sign-in is on. Turn it off for someone who lost their phone and recovery codes.">🔐 Turn off two-step</button> ' : ''}<button type="button" class="small danger u-del" ${me && u.username === me.username ? 'disabled' : ''}>Delete</button></td></tr>`).join('')}
    </tbody></table></div>
    <form class="card" id="u-signin"><h2>Signing in</h2><div class="form">
      <label for="si-days">Stay signed in for${tip('With “Keep me signed in” ticked when signing in, how long GAZE remembers this browser or app — also while it’s closed. After that it asks for the password again. Without the tick, GAZE forgets it when the browser is closed.')}</label>
        <div class="row"><select id="si-days">${[[1, '1 day'], [3, '3 days'], [7, '1 week'], [14, '2 weeks'], [30, '30 days'], [90, '90 days'], [365, '1 year']].map(([d, t]) => `<option value="${d}" ${d === (settings.server.remember_days || 30) ? 'selected' : ''}>${t}</option>`).join('')}</select><span class="muted">with “Keep me signed in”</span></div>
      <label for="si-idle">Sign out when not used${tip('Signs out after this long with no mouse, keyboard or touch on GAZE’s page, and when GAZE hasn’t been used at all for that long (the app or browser closed). Handy on a shared or wall-mounted screen. Note: just watching the Live page without touching it counts as not used.')}</label>
        <div class="row"><select id="si-idle">${[[0, 'Never'], [5, 'After 5 minutes'], [10, 'After 10 minutes'], [15, 'After 15 minutes'], [30, 'After 30 minutes'], [60, 'After 1 hour'], [120, 'After 2 hours'], [480, 'After 8 hours']].map(([m, t]) => `<option value="${m}" ${m === (settings.server.idle_minutes || 0) ? 'selected' : ''}>${t}</option>`).join('')}</select><span class="muted">with no activity</span></div>
    </div><div class="actions"><button class="primary">Save</button></div></form>
    <form class="card" id="u-new"><h2>Add a user</h2><div class="form">
      <label for="un-name">User name</label><div><input type="text" id="un-name" required autocomplete="off"></div>
      <label for="un-pass">Password</label><div><input type="password" id="un-pass" required minlength="8" autocomplete="new-password"><div class="hint">At least 8 characters. They can change it later from the 👤 menu.</div></div>
      <label>Access</label><div id="un-role">${roleSel('viewer')}</div>
      <label>Cameras</label><div id="un-cams">${camPick([])}</div>
    </div><div class="actions"><button class="primary">Add user</button></div></form>`;
  const rowData = (tr) => ({role: tr.querySelector('.u-role').value, cameras: [...tr.querySelectorAll('.u-cams input:checked')].map(i => i.value)});
  body.querySelectorAll('tr[data-u]').forEach(tr => {
    const name = tr.dataset.u;
    tr.querySelector('.u-save').onclick = async () => { try { await api(`api/users/${enc(name)}`, {method: 'PUT', json: rowData(tr)}); toast(`Saved ${name}`); } catch (e) { toast(e.message, true); } };
    const tf = tr.querySelector('.u-2fa');
    if (tf) tf.onclick = async () => {
      if (!confirm(`Turn off two-step sign-in for ${name}? They'll sign in with just their password until they turn it on again.`)) return;
      try { await api(`api/users/${enc(name)}/two-step-off`, {method: 'POST'}); toast(`Two-step sign-in off for ${name}`); renderUsers(body); } catch (e) { toast(e.message, true); }
    };
    tr.querySelector('.u-pw').onclick = async () => {
      const p = prompt(`New password for ${name} (at least 8 characters):`); if (!p) return;
      try { await api(`api/users/${enc(name)}`, {method: 'PUT', json: {password: p}}); toast(`Password set for ${name} (signed out everywhere)`); } catch (e) { toast(e.message, true); }
    };
    tr.querySelector('.u-del').onclick = async () => {
      if (!confirm(`Delete the user ${name}?`)) return;
      try { await api(`api/users/${enc(name)}`, {method: 'DELETE'}); renderUsers(body); } catch (e) { toast(e.message, true); }
    };
  });
  $('u-signin').onsubmit = async (e) => {
    e.preventDefault();
    const remember_days = +val('si-days'), idle_minutes = +val('si-idle');
    if (await saveSettings({sign_in: {remember_days, idle_minutes}}, 'Saved — applies from the next sign-in (“no activity” applies now)')) watchIdle(idle_minutes);
  };
  $('u-new').onsubmit = async (e) => {
    e.preventDefault();
    try {
      await api('api/users', {method: 'POST', json: {username: val('un-name'), password: $('un-pass').value,
        role: $('un-role').querySelector('select').value, cameras: [...$('un-cams').querySelectorAll('input:checked')].map(i => i.value)}});
      toast('User added'); renderUsers(body);
    } catch (err) { toast(err.message, true); }
  };
}

boot();

window.addEventListener('resize', applyDataSaver);
applyDataSaver();
