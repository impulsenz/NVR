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
// The Alerts filter chips. Faces and number plates show alerts where one
// was found.
const FILTER_BASE = [['motion','Motion'],['person','Person'],['car','Car'],['truck','Truck'],
  ['motorcycle','Motorcycle'],['dog','Dog'],['cat','Cat'],['package','Item left/taken'],
  ['face','Faces'],['plate','Number plates']];
function filterTypes() { return FILTER_BASE; }
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
const TIMELAPSE = false;
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
function toast(msg, isError) {
  const t = $('toast'); t.textContent = msg; t.className = isError ? 'error' : ''; t.hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => t.hidden = true, isError ? Math.max(7000, msg.length * 60) : 3500);
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
function zoomStyle(f) {
  if (!f) return '';
  const [x, y, w, h] = f, s = Math.min(3, Math.max(1, 0.55 / Math.max(w, h * 0.9, 0.01)));
  if (s < 1.05) return '';
  return `--z:${s.toFixed(2)};transform-origin:${((x + w / 2) * 100).toFixed(1)}% ${((y + h / 2) * 100).toFixed(1)}%;transform:scale(${s.toFixed(2)})`;
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
    return `<div class="bx ${motion ? 'motion' : ''} ${b.y < 0.07 ? 'top' : ''} ${b.x + b.w > 0.6 ? 'right' : ''}" title="${esc(tip)}" style="left:${(b.x * 100).toFixed(2)}%;top:${(b.y * 100).toFixed(2)}%;width:${(b.w * 100).toFixed(2)}%;height:${(b.h * 100).toFixed(2)}%">${labels ? `<span>${motion ? labelIcon('motion') + ' motion' : labelIcon(b.label) + ' ' + esc(tip)}</span>` : ''}</div>`;
  }).join('') + '</div>';
}
function labelTags(labels, motion) {
  if (labels && labels.length) return labels.map(l => `<span class="tag ${LABEL_GROUP[l.label] || ''}">${esc(l.label)} ${Math.round(l.score * 100)}%</span>`).join('');
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
const MODE_TEXT = {continuous: 'HD 24/7', continuous_sub: 'Sub-stream 24/7', sub_plus_events: 'Substream 24/7 plus HD AI alerts', events: 'HD on alerts only', off: 'Not recording'};

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
    for (const [q, fps] of want) subs.push({camera: c.id, q, fps});
  }
  if (dataSaver) subs.forEach(x => x.lite = true);
  Live.setWanted(subs);
}

// Overlays (name, clock, fps, status, fullscreen) for a live surface.
// Little symbols for what was detected (also used on timelines).
const LABEL_ICON = {person: '🚶', car: '🚗', truck: '🚚', bus: '🚌', motorcycle: '🏍️', bicycle: '🚲', dog: '🐕', cat: '🐈', bird: '🐦',
  horse: '🐎', cow: '🐄', sheep: '🐑', bear: '🐻', backpack: '🎒', suitcase: '🧳', handbag: '👜', umbrella: '☂️', package: '📦', motion: '〰️'};
const labelIcon = (l) => LABEL_ICON[l] || '🔷';
// What the timeline colours and symbols mean.
// What the marks on a timeline mean: only what's on it — the kinds of
// footage shown and the objects these cameras look for.
function legendHtml(kind, data) {
  const rows = data || [], segs = rows.flatMap(c => c.segments || []), evs = rows.flatMap(c => c.events || []);
  const watched = [...new Set(rows.flatMap(c => (cam(c.camera) || {}).watch || []))];
  const bars = [['<i class="lg-bar rec"></i>', 'Recording']];
  if (segs.some(s => s.kind === 'clip')) bars.push(['<i class="lg-bar clip"></i>', 'HD alert clip']);
  if (kind === 'tl' && evs.some(e => e.labels.length)) bars.push(['<i class="lg-bar ev"></i>', 'Alert (AI)']);
  if (evs.some(e => !e.labels.length)) bars.push([kind === 'tl' ? '<i class="lg-bar mo"></i>' : '<i class="lg-tick"></i>', 'Motion only']);
  return bars.map(([i, t]) => `<span>${i}${t}</span>`).join('')
    + (watched.length ? '<span class="lg-sep"></span>' + watched.map(o => `<span>${labelIcon(o)} ${esc(o)}</span>`).join('') : '');
}
// On-screen display: the camera's own settings, or the global ones.
function osdFor(c) {
  if (c.osd && c.osd.custom) return c.osd;
  return {...status.display.osd, clock_format: status.display.osd.clock_format || ''};
}
function overlayHtml(c, q, noFs) {
  const d = status.display, o = osdFor(c);
  const parts = Object.fromEntries(SPOTS.map(([k]) => [k, []]));
  // A spot that isn't one of the nine ("p:X,Y") gets its own box.
  for (const k of ['name_corner', 'text_corner', 'status_corner', 'detections_corner', 'clock_corner', 'fps_corner', 'res_corner']) if (o[k] && !parts[o[k]]) parts[o[k]] = [];
  if (o.show_name) parts[o.name_corner].push(`<span class="ovtext name" data-osd="name">${esc(c.name)}</span>`);
  if (o.text) parts[o.text_corner].push(`<span class="ovtext" data-osd="text">${esc(o.text)}</span>`);
  if (o.show_status) parts[o.status_corner].push(`<span class="ov-pill" data-osd="status">${pillHtml(c)}</span>`);
  if (o.show_detections) parts[o.detections_corner].push('<span class="ov-alert" data-osd="detections"></span>');
  if (o.show_clock) parts[o.clock_corner].push(`<span class="ovtext ov-clock" data-osd="clock" data-fmt="${esc(o.clock_format || d.clock_format || '')}">${esc(fmtDate(new Date(), o.clock_format || d.clock_format || undefined))}</span>`);
  if (o.show_fps) parts[o.fps_corner].push('<span class="ovtext ov-fps" data-osd="fps"></span>');
  if (o.show_res) parts[o.res_corner || 'tl'].push('<span class="ovtext ov-res" data-osd="res"></span>');
  // Stream 1 / Stream 2 switch, next to the first fullscreen button (or top right).
  const corners = noFs ? [] : d.fullscreen_corners;
  const sc = corners[0] || 'tr';
  // Live sound: speaker (click: on/off; hover: volume).
  if (c.sound !== false) parts[sc].push(`<span class="snd-wrap"><button type="button" class="fsbtn snd" data-cam="${esc(c.id)}" title="Sound on/off">${soundCam === c.id ? '🔊' : '🔇'}</button><input type="range" class="snd-vol" min="0" max="1" step="0.05" value="${soundVolume()}" title="Volume"></span>`);
  parts[sc].push(`<button type="button" class="fsbtn sbtn" title="${q === 'main' ? 'Showing stream 1 (main, full quality). Click for stream 2 (sub-stream, lighter).' : 'Showing stream 2 (sub-stream). Click for stream 1 (main, full quality).'}">${q === 'main' ? 'S1' : 'S2'}</button>`);
  for (const corner of corners) parts[corner].push('<button type="button" class="fsbtn" title="Fullscreen (or double-click the picture)">⛶</button>');
  return Object.entries(parts).filter(([, v]) => v.length).map(([k, v]) => ovBox(k, v.join(''))).join('');
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
  surface.querySelectorAll('.snd-vol').forEach(r => {
    r.onpointerdown = (e) => e.stopPropagation();
    r.onclick = (e) => e.stopPropagation();
    r.oninput = (e) => { e.stopPropagation(); setSoundVolume(+r.value); };
  });
  surface.querySelectorAll('.fsbtn:not(.sbtn):not(.snd)').forEach(b => b.onclick = (e) => { e.stopPropagation(); fullscreen(surface); });
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
  if (isFocus) bindFocusImage(c);
  else {
    // Swap the picture's stream in place: rebuilding the tiles would take
    // the picture out of full screen.
    const img = surface.querySelector(':scope > img'), u = unbinders.get(c.id);
    if (u) u();
    surface.dataset.q = q;
    if (img) unbinders.set(c.id, Live.bind(img, c.id, q));
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
    surface.insertAdjacentHTML('beforeend', overlayHtml(c, q, surface.id === 'focus-surface'));
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
let openAlertId = null;
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
  if (typeof wall !== 'undefined' && wall && (page !== 'live' || sub)) exitWall();
  document.querySelectorAll('nav a').forEach(a => a.classList.toggle('active', a.dataset.page === navPage));
  for (const s of PAGES) $('page-' + s).hidden = s !== page;
  if (page !== 'live' && typeof soundCam !== 'undefined' && soundCam) stopSound();
  if (page !== 'recordings') pauseAllPlayback();
  if (page === 'live') showLive(sub ? decodeURIComponent(sub) : null);
  else if (page === 'recordings') showRecordings(sub ? decodeURIComponent(sub) : null, p0 === 'alerts' ? 'list' : 'timeline');
  else if (page === 'system') showSystem();
  else showSettings(sub || 'cameras');
  updateSubscriptions();
}
window.addEventListener('hashchange', route);

async function refreshStatus() {
  try { status = await api('api/status?device=' + deviceKind()); } catch (e) { return; }
  $('nvr-name').title = status.name;
  document.title = status.name && status.name !== 'GAZE' ? `GAZE — ${status.name}` : 'GAZE';
  // How live pictures fill their boxes, everywhere (stream 2 is often a
  // squeezed 4:3 picture: stretched, it fills the box like the main stream).
  document.body.dataset.fit = 'stretch';
  // New alerts.
  const ab = $('alert-badge'), n = status.alerts_unseen || 0;
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
  const gb = $('g-banner');
  if (status.new_cameras && !store.get('newcam-dismissed-' + status.new_cameras, false)) {
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
  el.style.backgroundImage = `url("${url}")`;
  el.style.backgroundSize = `${i.columns * dw}px ${rows * dh}px`;
  el.style.backgroundPosition = `${ox - col * dw}px ${oy - row * dh}px`;
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
    if (!c) { this.osd.innerHTML = ''; return; }
    const o = osdFor(c), d = status.display;
    const parts = {};
    const add = (corner, html) => (parts[corner] = parts[corner] || []).push(html);
    if (o.show_name) add(o.name_corner, `<span class="ovtext name">${esc(c.name)}</span>`);
    if (o.text) add(o.text_corner, `<span class="ovtext">${esc(o.text)}</span>`);
    if (o.show_clock) add(o.clock_corner, `<span class="ovtext p-clock" data-fmt="${esc(o.clock_format || d.clock_format || '')}"></span>`);
    add(o.show_clock ? o.clock_corner : 'tl', '<span class="ovtext p-rec">▶ Recording</span>');
    this.osd.innerHTML = Object.entries(parts).map(([k, v]) => ovBox(k, v.join(''), 'pov-osd')).join('');
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
        <button type="button" class="p-in" title="Mark the clip start at the current position">[ Start</button>
        <button type="button" class="p-out" title="Mark the clip end at the current position">End ]</button>
        <button type="button" class="p-clip" hidden>⇩ Download clip</button>
        <a class="btn p-dl" title="Download the whole file">⇩ File</a>
        <span class="vol"><button type="button" class="p-mute" title="Mute / unmute">🔊</button><input type="range" class="p-vol" min="0" max="1" step="0.05" value="1" title="Volume"></span>
        <button type="button" class="p-fs" title="Fullscreen">⛶</button>
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
    q('.p-fs').onclick = () => fullscreen(this.surface);
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
    this.buildOsd();
    this.paint();
    this.lightSegs = [];
    this.findLightCopies(item);
    const kind = item.kind === 'clip' ? 'clip' : 'rec';
    if (TIMELAPSE) try { this.sprite = preloadSprite(await api(`api/sprite/${enc(item.camera)}/${kind}/${enc(item.name)}`)); } catch (e) { this.sprite = null; }
  }
  // The light timeline copies around this file: their time-lapse has a
  // picture every second, shown full size while dragging along the bar.
  async findLightCopies(item) {
    const a = new Date(item.file_start).getTime();
    try {
      const [t] = await api(`api/timeline?cameras=${enc(item.camera)}&from=${enc(new Date(a - 60000).toISOString())}&to=${enc(new Date(a + 3 * 3600000).toISOString())}`);
      if (this.item === item) this.lightSegs = ((t && t.segments) || []).filter(x => x.kind === 'sub');
    } catch (e) {}
  }
  // While dragging: the time-lapse picture of moment `t` over the whole
  // video (the full-quality video is loaded there on release).
  showTimelapse(t) {
    const ov = this.tlap; if (!ov || !this.item) return;
    const W = this.wallAt(t).getTime(), cam = this.item.camera, want = this.tlapWant = t;
    const seg = (this.lightSegs || []).find(x => new Date(x.start).getTime() <= W && W < new Date(x.end).getTime() + 1000);
    const show = (sp, off) => {
      if (!sp || this.tlapWant !== want) return;
      if (spriteStyle(ov, sp, Math.floor(off / sp.info.interval), this.surface.clientWidth, this.surface.clientHeight, false)) ov.hidden = false;
    };
    if (seg) spriteFor(cam, seg).then(sp => sp ? show(sp, (W - new Date(seg.start).getTime()) / 1000) : show(this.sprite, t));
    else show(this.sprite, t);
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
    unbinders.set(c.id, Live.bind(img, c.id, q));
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
  $('focus-recs').href = '#recordings/' + enc(c.id) + '/timeline';
  $('focus-edit').onclick = () => openCameraDialog(c.id);
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
  surf.insertAdjacentHTML('beforeend', overlayHtml(c, q, true));
  delete surf.dataset.osd;
  wireOverlay(surf);
  if (focusUnbind) focusUnbind();
  const img = $('focus-img');
  img.removeAttribute('src');
  $('focus-msg').hidden = false; $('focus-msg').textContent = 'Connecting…';
  img.onload = () => { $('focus-msg').hidden = true; };
  focusUnbind = Live.bind(img, c.id, q);
  offlineAfter($('focus-msg'), img);
  // Stream 2 is often another shape (4:3): fill the box like stream 1.
  img.classList.toggle('fill', q === 'sub');
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

// ---------- PTZ controls ----------
// Pan / tilt / zoom on the picture: the pad and zoom buttons (press and
// hold), dragging on the picture (direction and speed follow the drag),
// the mouse wheel to zoom, and the arrow / + / - keys.
let ptzCam = null;
function setupPtz(c) {
  const pad = $('ptz-pad'), surf = $('focus-surface');
  ptzCam = c.ptz ? c.id : null;
  pad.hidden = !c.ptz;
  surf.classList.toggle('has-ptz', !!c.ptz);
  if (!c.ptz) return;
  const id = c.id, msg = (t, bad) => { $('ptz-msg').textContent = t; $('ptz-msg').classList.toggle('err', !!bad); };
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
      $('ptz-presets').innerHTML = `<option value="">Presets (${p.length})…</option>` + p.map(x => `<option value="${esc(x.token)}">${esc(x.name)}</option>`).join('');
    } catch (e) { msg(e.message, true); }
  };
  $('ptz-presets').onchange = () => { const t = $('ptz-presets').value; if (t) api(`api/cameras/${enc(id)}/ptz/goto`, {method: 'POST', json: {token: t}}).then(() => msg('Moving to the preset…')).catch(e => msg(e.message, true)); };
  $('ptz-save').onclick = async () => {
    const name = (prompt('Name for this position (e.g. Driveway, Gate):') || '').trim();
    if (!name) return;
    try { await api(`api/cameras/${enc(id)}/ptz/presets`, {method: 'POST', json: {name}}); toast('Preset saved'); loadPresets(); }
    catch (e) { msg(e.message, true); }
  };
  loadPresets();
}
let ptzMove = null, ptzStop = null;
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
    if (!ptzCam || page !== 'live' || focusId !== ptzCam || focusPlayback || e.target.closest('input, select, textarea, dialog') || !keyVec[e.key] || held.has(e.key)) return;
    e.preventDefault(); held.add(e.key); ptzMove(keyVec[e.key]);
  });
  document.addEventListener('keyup', (e) => { if (held.delete(e.key) && ptzStop) ptzStop(); });
})();

// ---------- per-camera timeline (camera view) ----------
// Works like the Recordings timeline: drag to run through the time-lapse on
// the camera's picture, let go to play from there; a strip shows which
// parts of the time-lapse are loaded.
// Time marks for a window: labels, small ticks between, and grid lines.
function timeMarks(a, span, pct) {
  const steps = [60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400].map(x => x * 1000);
  const step = steps.find(x => span / x <= 12) || 86400000;
  const minor = step / ([120000, 600000, 1800000, 7200000].includes(step) ? 4 : [10800000, 43200000].includes(step) ? 3 : step === 86400000 ? 4 : 5);
  let axis = '', grid = '';
  for (let t = Math.ceil(a / minor) * minor; t <= a + span; t += minor) {
    const major = Math.abs(t % step) < 1 || (step >= 86400000 && new Date(t).getHours() === 0 && new Date(t).getMinutes() === 0);
    if (!major) { axis += `<i class="mn" style="left:${pct(t)}%"></i>`; continue; }
    const d = new Date(t), hm = String(d.getHours()).padStart(2, '0') + ':' + String(d.getMinutes()).padStart(2, '0');
    axis += `<i class="mj" style="left:${pct(t)}%"></i><span style="left:${pct(t)}%">${step >= 86400000 ? fmtDay(d) : hm + (hm === '00:00' ? ' ' + fmtDay(d) : '')}</span>`;
    grid += `<i style="left:${pct(t)}%"></i>`;
  }
  const nowP = pct(Date.now());
  if (nowP >= 0 && nowP <= 100) grid += `<i class="now" style="left:${nowP}%" title="Now"></i>`;
  return {axis, grid};
}
const camTl = {
  id: null, data: null, zoom: 21600, end: null, loadedAt: 0, pv: new Map(), pfGen: 0,
  open(id) {
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
    let lane = `<div class="tl-grid">${grid}</div>`;
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
    $('cam-tl').querySelector('.legend').innerHTML = legendHtml('cam', [{...this.data, camera: this.id}]);
    this.paintStrip(); this.paintHead();
  },
  // Which parts of the time-lapse are ready to scrub.
  paintStrip() {
    const el = $('ctl-tlp'); if (!el || !this.data) return;
    const [a, b] = this.win(), span = b - a, pct = (t) => (t - a) / span * 100;
    let h = '';
    for (const sg of this.sources()) {
      const s0 = new Date(sg.start).getTime(), s1 = new Date(sg.end).getTime();
      const st = this.pv.get(sg.url) || {state: 'wait'};
      const end = st.state === 'ready' && st.secs ? Math.min(Math.max(s1, isGrowing(sg) ? Date.now() : s1), s0 + st.secs * 1000) : s1;
      const x0 = Math.max(0, pct(s0)), x1 = Math.min(100, pct(end));
      if (x1 > 0 && x0 < 100 && x1 > x0) h += `<i class="${st.state}" style="left:${x0}%;width:${Math.max(x1 - x0, 0.15)}%"></i>`;
    }
    el.innerHTML = h;
  },
  // Load the time-lapse of everything in view, newest first (up to now).
  async prefetch() {
    if (!TIMELAPSE) return;
    const gen = ++this.pfGen, id = this.id;
    const todo = this.sources().slice().sort((x, y) => new Date(y.start) - new Date(x.start)).slice(0, 300);
    let i = 0, missing = 0;
    const worker = async () => {
      while (i < todo.length && gen === this.pfGen && id === this.id) {
        const sg = todo[i++], had = this.pv.get(sg.url);
        if (had && had.state === 'ready' && !(isGrowing(sg) && Date.now() - had.at > 30000)) continue;
        if (spriteBackoff(sg.url)) { missing++; continue; }
        if (!had || had.state !== 'ready') { this.pv.set(sg.url, {state: 'loading'}); this.paintStrip(); }
        const sp = await previewReady(id, sg);
        if (sp) this.pv.set(sg.url, {state: 'ready', at: Date.now(), secs: sp.info.count * sp.info.interval});
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
    if (!h.hidden) h.style.left = (p * 100) + '%';
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
    const sg = this.sources().find(s => new Date(s.start).getTime() <= T && T < (isGrowing(s) ? Date.now() + 5000 : new Date(s.end).getTime() + 1000));
    if (Date.now() - T < 15000) { liveStill(el, this.id); el.hidden = false; return; }
    if (!sg) return;
    const sp = await spriteFor(this.id, sg);
    if (!sp || this.scrubT !== T) return;
    if (spriteStyle(el, sp, Math.floor((T - new Date(sg.start).getTime()) / 1000 / sp.info.interval), surf.clientWidth, surf.clientHeight)) el.hidden = false;
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
    if (sp) spriteStyle(im, sp, Math.floor((T - new Date(sg.start).getTime()) / 1000 / sp.info.interval), 192, 108);
  };
  const down = (e) => {
    if (!camTl.data || e.target.closest('.ic, .mo')) return;
    e.preventDefault();
    camTl.dragging = true; lane.setPointerCapture(e.pointerId);
    lane.querySelector('.preview').hidden = true;
    const [T] = tAt(e); camTl.scrubT = T; camTl.paintHead(T); camTl.scrubTo(T);
  };
  lane.addEventListener('pointerdown', down);
  strip.addEventListener('pointerdown', (e) => { lane.dispatchEvent(new PointerEvent('pointerdown', e)); });
  lane.addEventListener('pointermove', (e) => {
    if (!camTl.data) return;
    if (!camTl.dragging) return preview(e);
    const [T] = tAt(e); camTl.scrubT = T; camTl.paintHead(T); camTl.scrubTo(T);
  });
  const up = (e) => {
    if (!camTl.dragging) return;
    camTl.dragging = false;
    const [T] = tAt(e); camTl.playAt(T); camTl.draw();
  };
  lane.addEventListener('pointerup', up); lane.addEventListener('pointercancel', up);
  lane.addEventListener('pointerleave', () => { lane.querySelector('.preview').hidden = true; });
  lane.addEventListener('click', (e) => {
    const icon = e.target.closest('.ic, .mo');
    if (icon) camTl.playAt(+icon.dataset.t - 3000);
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
  // Live sound: the player builds up a cushion and falls behind; keep it
  // within about a third of a second of "now".
  clearInterval(soundCatchUp);
  soundCatchUp = setInterval(() => {
    if (soundCam !== camId || a.paused || !a.buffered.length) return;
    const end = a.buffered.end(a.buffered.length - 1);
    if (end - a.currentTime > 0.6) a.currentTime = end - 0.25;
  }, 500);
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
  const b = $('focus-talk'); if (!b) return;
  b.querySelector('span').textContent = text;
  b.classList.toggle('talking', cls === 'talking'); b.classList.toggle('connecting', cls === 'connecting');
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
function setupTalkButton(c) {
  const b = $('focus-talk');
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
  focusPlayback = it;
  updateSubscriptions();
  $('focus-img').hidden = true;
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
const DEFAULT_TYPES = ['person', 'car', 'truck', 'motorcycle', 'package', 'face', 'plate'];
const rec = {mode: 'list', cameras: new Set(), from: startOfDay(new Date()), to: endOfDay(new Date()), todFrom: '', todTo: '',
  types: new Set(store.get('alert-types', DEFAULT_TYPES)), items: [], who: ''};
const saveTypes = () => store.set('alert-types', [...rec.types]);
let recPlayer = null;

function showRecordings(camId, mode) {
  if (camId && cam(camId)) rec.cameras = new Set([camId]);
  renderFilters();
  setRecMode(mode || 'timeline');
}

function setRecMode(mode) {
  rec.mode = mode;
  $('rec-title').textContent = mode === 'list' ? 'Alerts' : 'Recordings';
  $('rec-list').hidden = mode !== 'list';
  $('rec-timeline').hidden = mode !== 'timeline';
  $('f-types').hidden = mode !== 'list';
  document.querySelectorAll('#page-recordings .list-only').forEach(e => e.hidden = mode !== 'list');
  if (mode === 'list') { tlStop(); loadRecordings(); }
  else { closeAlert(); loadTimeline(); }
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
setInterval(async () => {
  if (page !== 'recordings' || rec.mode !== 'list' || document.hidden || rec.loading || rec.who === '__new') return;
  const before = rec.items.map(i => i.event ? i.event.id + i.event.end + (i.event.notes || []).length + i.seen : i.name).join();
  const r = await fetchRecordings().catch(() => null);
  if (!r) return;
  if (r.map(i => i.event ? i.event.id + i.event.end + (i.event.notes || []).length + i.seen : i.name).join() !== before) { rec.items = r; renderRecordings(); }
}, 8000);
$('f-refresh').onclick = () => rec.mode === 'list' ? loadRecordings() : loadTimeline();
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
  let label = rec.from.getFullYear() <= 2000 ? 'All time' : same ? (f0 === today ? 'Today' : f0 === today - 86400000 ? 'Yesterday' : fmtDay(rec.from)) : `${fmtDay(rec.from)} – ${fmtDay(rec.to)}`;
  if (rec.todFrom || rec.todTo) label += `, ${rec.todFrom || '00:00'}–${rec.todTo || '23:59'}`;
  $('f-date-btn').textContent = '📅 ' + label;
  // Types
  $('f-types').innerHTML = '<span class="muted" style="padding:3px 4px">Show:</span>' + filterTypes().map(([k, v]) =>
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
      <div class="row" style="margin-top:8px;flex-wrap:wrap"><button type="button" class="small" data-p="0">Today</button><button type="button" class="small" data-p="1">Yesterday</button><button type="button" class="small" data-p="7">Last 7 days</button><button type="button" class="small" data-p="30">Last 30 days</button><button type="button" class="small" data-p="all">All time</button></div>
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
  } catch (e) { rec.items = []; toast(e.message, true); }
  rec.loading = false;
  renderRecordings();
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
    card.className = 'rc' + (it.kind === 'event' && !it.seen ? ' new' : '');
    card.dataset.eid = it.event ? it.event.id : '';
    const dur = (new Date(it.end) - new Date(it.start)) / 1000;
    const tags = it.kind === 'event' ? labelTags(it.event.labels, it.event.motion) : '<span class="tag">recording</span>';
    const icons = it.kind === 'event' ? (it.event.labels.length ? it.event.labels.map(l => `<span class="tag ${LABEL_GROUP[l.label] || ''}" title="${esc(l.label)} ${Math.round(l.score * 100)}%">${labelIcon(l.label)} ${Math.round(l.score * 100)}%</span>`).join('') : `<span class="tag motion">${labelIcon('motion')}</span>`) : '';
    card.innerHTML = `<div class="thumb">${it.thumb ? `<div class="zw" style="${zoomStyle(it.event && it.event.focus)}"><img loading="lazy" src="${esc(it.thumb)}" alt="" onerror="this.parentElement.remove()">${boxesHtml(it.event, false)}</div>` : ''}
        <div class="tags">${it.kind === 'event' && !it.seen ? '<span class="tag newtag">NEW</span>' : ''}${icons}${it.kind === 'event' && it.event.clip ? '<span class="tag">HD</span>' : ''}</div>
        <div class="tools">${it.download ? `<a class="btn small" href="${esc(it.download)}" title="Download" onclick="event.stopPropagation()">⇩</a>` : ''}<button type="button" class="small op-only" data-a="del" title="Delete">✕</button></div>
        <div class="play">${it.play ? '▶' : it.pending ? '⏳' : ''}</div>
        <div class="cap"><b>${esc(it.camera_name)}</b><span class="t">${fmtTime(it.start)} · ${fmtDur(dur)}</span></div></div>`;
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
  $('ad-prev').disabled = alertIdx <= 0; $('ad-next').disabled = alertIdx < 0 || alertIdx >= rec.items.length - 1;
  $('rp-info').innerHTML = `
    ${e && e.suspicious ? `<div class="sus">⚠ ${esc(e.suspicious)}</div>` : ''}
    ${e && e.notes && e.notes.length ? `<div class="what"><div class="muted small">Seen</div>${e.notes.map(n => esc(n)).join('<br>')}</div>` : ''}
    ${e && it.thumb ? `<div class="review"><img src="${esc(it.thumb)}" alt="">${boxesHtml(e, true)}</div>
      ${e.boxes && e.boxes.length ? `<label class="check" style="margin:-2px 0 8px"><input type="checkbox" class="bx-toggle" ${showBoxes ? 'checked' : ''}> Show detection boxes</label>` : ''}` : ''}
    ${e && e.faces && e.faces.length ? `<div class="faces"><div class="muted small">Faces</div>${e.faces.map((f, i) => `<div class="face"><img src="media/${enc(it.camera)}/events/${enc(f.file)}" alt="">
      ${f.name ? `<b>${esc(f.name)}</b><button type="button" class="op-only face-fix" data-face="${i}" title="Wrong person? Choose who it really is, or make a new person">✎ Wrong?</button>` : ''}
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
      // Keep the alert's notes in step (what the server now has).
      if (r.notes) e.notes = r.notes;
      playItem(it, card);
    } catch (err) { toast(err.message, true); }
  });
  // Viewed.
  if (e && !it.seen) {
    it.seen = true;
    if (card) { card.classList.remove('new'); const n = card.querySelector('.newtag'); if (n) n.remove(); }
    api('api/alerts/seen', {method: 'POST', json: {ids: [e.id]}}).then(refreshStatus).catch(() => {});
  }
}
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
const tl = {scrub: store.get('tl-scrub', 'sub'), quality: store.get('tl-quality', 'sub'), data: [], T: null, playing: false, speed: 1, zoom: 3600, winStart: null, inT: null, outT: null, sprites: new Map(), last: 0, jump: null, day: null};

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
      <div class="sprite" hidden></div><div class="msg" hidden>No recorded footage at this time</div>
      <div class="ov tl"><span class="ovtext name">${esc(c.name)}</span><span class="ovtext tl-t"></span></div>
      <div class="ov tr"><button type="button" class="small solo-off" hidden>▦ All cameras</button><button type="button" class="fsbtn">⛶</button></div>`;
    el.querySelector('.fsbtn').onclick = () => fullscreen(el);
    el.querySelector('.solo-off').onclick = (e) => { e.stopPropagation(); tlSolo(null); };
    el._vids = [...el.querySelectorAll('video')]; el._vids.forEach(watchFps);
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
function segAt(c, T, force) {
  // The newest file of a camera that was recording when the timeline
  // loaded is still growing: treat it as running up to now.
  const lasts = new Set(['recording', 'sub'].map(k => c.segments.filter(s => s.kind === k).slice(-1)[0]));
  const end = (s) => lasts.has(s) && tl.loadedAt - new Date(s.end).getTime() < 90000 ? Date.now() + 5000 : new Date(s.end).getTime() + 1000;
  const hit = (s) => new Date(s.start).getTime() <= T && T < end(s);
  // Normally the light stream 2 copy (opens instantly); after clicking an
  // alert, its full-quality footage until the alert is over.
  const hd = tl.hd && tl.hd.camera === c.camera && T >= tl.hd.from - 2000 && T < tl.hd.until;
  const order = force === 'main' ? ['clip', 'recording', 'sub'] : force === 'sub' ? ['sub', 'recording', 'clip']
    : hd || tl.quality === 'main' ? ['clip', 'recording', 'sub'] : ['sub', 'recording', 'clip'];
  for (const k of order) {
    const s = c.segments.find(x => x.kind === k && hit(x));
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
  const q = new URLSearchParams({from: rec.from.toISOString(), to: new Date(Math.max(rec.to.getTime(), Date.now())).toISOString()});
  if (rec.cameras.size) q.set('cameras', [...rec.cameras].join(','));
  api('api/timeline?' + q).then(d => { if (tl.dragging) return; tl.data = d; tl.loadedAt = Date.now(); drawTimeline(); prefetchSprites(); }).catch(() => {});
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
    // Near the end of this file (or of the alert's HD part): load what comes
    // next in the other player, at the right moment, for a seamless switch.
    const segEnd = new Date(s.end).getTime();
    const boundary = tl.hd && tl.hd.camera === c.camera && s.kind !== 'sub' ? Math.min(segEnd, tl.hd.until) : segEnd;
    const left = (boundary - tl.T) / 1000;
    const nx = left < 10 * Math.max(1, tl.speed) && segAt(c, boundary + 100);
    const other = el._vids[1 - el._active];
    if (nx && nx.url !== s.url && other.dataset.url !== nx.url) {
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
$('tl-quality').value = tl.quality;
// What scrubbing shows: the time-lapse pictures, or the sub-stream or main
// stream video itself (to compare how quickly each one keeps up).
if (!TIMELAPSE && tl.scrub === 'timelapse') tl.scrub = 'sub';
$('tl-scrub').querySelectorAll('button').forEach(b => {
  b.classList.toggle('on', b.dataset.v === tl.scrub);
  b.onclick = () => {
    tl.scrub = b.dataset.v; store.set('tl-scrub', tl.scrub); tl.scrubMs = [];
    $('tl-scrub').querySelectorAll('button').forEach(x => x.classList.toggle('on', x === b));
    $('tl-scrub-ms').textContent = '';
  };
});
$('tl-quality').onchange = (e) => { tl.quality = e.target.value; store.set('tl-quality', tl.quality); tlSync(true); };
$('tl-zoom').onchange = (e) => { tl.zoom = +e.target.value; centerWindow(); drawTimeline(); prefetchSprites(); };
$('tl-left').onclick = () => { tl.winStart -= tl.zoom * 500; drawTimeline(); prefetchSprites(); };
$('tl-right').onclick = () => { tl.winStart += tl.zoom * 500; drawTimeline(); prefetchSprites(); };
// One frame back or forward on every camera (pauses playback).
const tlStep = (n) => {
  if (tl.playing) tlPlay(false);
  const v = document.querySelector('#tl-videos .surface') && activeVideo(document.querySelector('#tl-videos .surface'));
  tl.T += n * 1000 / videoFps(v);
  drawHead(); tlSync(true);
};
$('tl-back10').onclick = () => tlStep(-1);
$('tl-fwd10').onclick = () => tlStep(1);
function allEvents() { return tl.data.flatMap(c => c.events).map(e => new Date(e.start).getTime()).sort((a, b) => a - b); }
$('tl-prev-ev').onclick = () => { const e = allEvents().filter(t => t < tl.T - 4000).pop(); if (e) jumpTo(e - 3000); };
$('tl-next-ev').onclick = () => { const e = allEvents().find(t => t > tl.T + 4000); if (e) jumpTo(e - 3000); };
// Watch one alert: that camera's video large (others hidden), from just before it.
// Watch one alert: just that camera (the camera choice follows; "← All
// cameras" goes back), in full quality from just before the alert, then on
// into the light stream 2 recording without a break.
async function tlShowEvent(camId, T) {
  const c = tl.data.find(x => x.camera === camId);
  const e = c && c.events.find(x => Math.abs(new Date(x.start).getTime() - T) < 1000);
  const clip = c && c.segments.find(s => s.kind === 'clip' && new Date(s.start).getTime() <= T && new Date(s.end).getTime() > T);
  const until = clip ? new Date(clip.end).getTime() : (e ? new Date(e.end).getTime() + 10000 : T + 30000);
  tl.hd = {camera: camId, from: T - 3000, until};
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
function jumpTo(T) { tl.T = T; if (T < tl.winStart || T > tl.winStart + tl.zoom * 1000) centerWindow(); drawTimeline(); tlSync(true); }
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
  // Labels about every 1/10 of the window, with small ticks between.
  const steps = [60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600].map(s => s * 1000);
  const step = steps.find(s => span / s <= 12) || 21600000;
  const minor = step / ([120000, 600000, 1800000, 7200000].includes(step) ? 4 : step === 10800000 ? 3 : 5);
  let axis = '', grid = '';
  for (let t = Math.ceil(W0 / minor) * minor; t <= W0 + span; t += minor) {
    const major = Math.abs(t % step) < 1;
    if (!major) { axis += `<i class="mn" style="left:${pct(t)}%"></i>`; continue; }
    const d = new Date(t);
    axis += `<i class="mj" style="left:${pct(t)}%"></i><span style="left:${pct(t)}%">${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}${d.getHours() === 0 && d.getMinutes() === 0 ? ' ' + fmtDay(d) : ''}</span>`;
    grid += `<i style="left:${pct(t)}%"></i>`;
  }
  const nowP = pct(Date.now());
  if (nowP >= 0 && nowP <= 100) grid += `<i class="now" style="left:${nowP}%" title="Now"></i>`;
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
    rows += `<div class="tl-row"><button type="button" class="label" data-cam="${esc(c.camera)}" title="${esc(c.name)} — show just this camera">${esc(c.name)}</button><div class="lanes"><div class="lane"><div class="tl-grid">${grid}</div>${lane}</div>
      <div class="tlp" data-cam="${esc(c.camera)}" title="Time-lapse previews — blue: ready to scrub · striped: loading · grey: not loaded yet · red: couldn't be made"></div></div></div>`;
  }
  const sel = tl.inT != null && tl.outT != null ? `<div class="tl-sel" style="${selStyle(pct)}"><div class="tl-h in" title="Drag to change the clip start"></div><div class="tl-h out" title="Drag to change the clip end"></div></div>` : '';
  box.innerHTML = `<div class="tl-axis">${axis}</div>${rows}${sel}<div class="tl-head" id="tl-head"></div>`;
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
  const h = $('tl-head'); if (!h) return;
  const p = (tl.T - tl.winStart) / (tl.zoom * 1000);
  h.hidden = p < 0 || p > 1;
  h.style.left = `calc(var(--tl-label) + (100% - var(--tl-label)) * ${p})`;
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
  let handle = null;
  box.addEventListener('pointerdown', (e) => {
    // Clip start / end handles: drag to change the clip.
    const hd = e.target.closest('.tl-h');
    if (hd) { e.preventDefault(); handle = hd.classList.contains('in') ? 'in' : 'out'; box.setPointerCapture(e.pointerId); return; }
    // An alert marker or symbol: watch that alert on that camera.
    const mark = e.target.closest('.ev');
    if (mark) { e.preventDefault(); tlShowEvent(mark.dataset.cam, +mark.dataset.t); return; }
    const t = tAt(e); if (t == null || e.clientX < box.querySelector('.lane').getBoundingClientRect().left) return;
    drag = true; tl.dragging = true; was = tl.playing; if (was) tlPlay(false);
    box.setPointerCapture(e.pointerId);
    tl.T = t; drawHead(); previewAll();
  });
  box.addEventListener('pointermove', (e) => {
    if (handle) {
      const t = Math.min(tAt(e), Date.now());
      if (handle === 'in') tl.inT = Math.min(t, tl.outT - 1000); else tl.outT = Math.max(t, tl.inT + 1000);
      const pct = (x) => ((x - tl.winStart) / (tl.zoom * 1000) * 100);
      const sel = box.querySelector('.tl-sel'); if (sel) sel.style.cssText = selStyle(pct);
      $('tl-sel-info').textContent = `Clip ${fmtTime(new Date(tl.inT))} – ${fmtTime(new Date(tl.outT))} (${fmtClock((tl.outT - tl.inT) / 1000)})`;
      // Show the picture at the handle while dragging.
      tl.T = handle === 'in' ? tl.inT : tl.outT; drawHead(); previewAll();
      return;
    }
    if (!drag) return; tl.T = Math.min(tAt(e), Date.now()); drawHead(); previewAll();
  });
  const end = () => {
    if (handle) { handle = null; drawTimeline(); tlSync(true); hideSpritesWhenReady(); return; }
    if (!drag) return; drag = false; tl.dragging = false;
    document.querySelectorAll('#tl-videos video').forEach(v => { v._want = null; });
    tlSync(true); hideSpritesWhenReady(); if (was) tlPlay(true);
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
    entry.p = Promise.race([api(`api/sprite/${enc(cam)}/${kind}/${enc(seg.name)}`), new Promise((_, no) => setTimeout(() => no(new Error('timed out')), 120000))])
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
      const done = () => { if (im.naturalWidth) { pagesDone.add(sp.url + p); r(true); } else { spritePages.delete(sp.url + p); r(false); } };
      if (im.complete) done(); else { im.addEventListener('load', done, {once: true}); im.addEventListener('error', done, {once: true}); } })));
  if (ok.every(Boolean)) return sp;
  const e = tl.sprites.get(sg.url);
  if (e) { e.ok = null; e.done = true; spriteFailed(e); }
  return null;
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
function previewSources(segs) {
  const subs = segs.filter(x => x.kind === 'sub').map(x => [new Date(x.start).getTime(), new Date(x.end).getTime()]);
  return segs.filter(x => {
    if (x.kind === 'sub') return true;
    if (x.kind !== 'recording') return false;
    const a = new Date(x.start).getTime(), b = new Date(x.end).getTime();
    return !subs.some(([s0, s1]) => s0 <= a + 5000 && s1 >= b - 5000);
  });
}
// Under each camera's lane: which parts of the time-lapse are loaded and
// ready to scrub (filled), still loading (striped) or not loaded yet.
function paintPreviewStrips() {
  const W0 = tl.winStart, span = tl.zoom * 1000, pct = (t) => (t - W0) / span * 100;
  for (const el of document.querySelectorAll('#tl .tlp')) {
    const c = tl.data.find(x => x.camera === el.dataset.cam); if (!c) continue;
    let h = '';
    for (const sg of previewSources(c.segments)) {
      const s0 = new Date(sg.start).getTime(), s1 = new Date(sg.end).getTime();
      const st = (tl.pv && tl.pv.get(sg.url)) || {state: 'wait'};
      const end = st.state === 'ready' && st.secs ? Math.min(Math.max(s1, isGrowing(sg) ? Date.now() : s1), s0 + st.secs * 1000) : s1;
      const a = Math.max(0, pct(s0)), b = Math.min(100, pct(end));
      if (b <= 0 || a >= 100 || b <= a) continue;
      h += `<i class="${st.state}" style="left:${a}%;width:${Math.max(b - a, 0.15)}%"></i>`;
    }
    el.innerHTML = h;
  }
}
function prefetchSprites() {
  if (!TIMELAPSE) return;
  const gen = tl.pfGen = (tl.pfGen || 0) + 1;
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
      if (ok) tl.pv.set(key, {state: 'ready', at: Date.now(), secs: sp.info.count * sp.info.interval});
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
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const sp = el.querySelector('.sprite'), v = activeVideo(el);
    if (sp.hidden) continue;
    const gen = sp._gen = (sp._gen || 0) + 1;
    const done = () => { if (sp._gen === gen) sp.hidden = true; };
    const check = () => { if (v.readyState >= 2 && !v.seeking) done(); else setTimeout(check, 100); };
    setTimeout(check, 50);
    setTimeout(done, 4000);
  }
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
    // Scrubbing with the video itself (sub-stream or main stream): seek it.
    if (tl.scrub !== 'timelapse' && !el._tc) {
      const vs = segAt(c, T, tl.scrub) || s;
      sp.hidden = true;
      scrubVideo(el, vs, (T - new Date(vs.start).getTime()) / 1000);
      continue;
    }
    // The last moments (newer than the preview): the live picture.
    if (Date.now() - T < 15000) { liveStill(sp, c.camera); sp.hidden = false; continue; }
    const asked = performance.now();
    spriteFor(c.camera, s).then(info => {
      if (!info || tl.T !== T) return;
      const off = (T - new Date(s.start).getTime()) / 1000;
      if (spriteStyle(sp, info, Math.floor(off / info.info.interval), el.clientWidth, el.clientHeight, true)) { sp.hidden = false; scrubTiming(performance.now() - asked); }
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
    v._go = () => { if (v._want == null || v.readyState < 1 || v.seeking) return; v._askedAt = performance.now(); if (v.fastSeek && tl.scrub === 'main') v.fastSeek(v._want); else v.currentTime = v._want; };
    v.addEventListener('seeked', () => {
      if (v._askedAt) { scrubTiming(performance.now() - v._askedAt); v._askedAt = 0; }
      if (v._want != null && Math.abs(v.currentTime - v._want) > 0.3) v._go();
    });
    v.addEventListener('loadedmetadata', () => v._go());
  }
  if (v.dataset.url !== seg.url) { v.pause(); v.dataset.url = seg.url; v.preload = 'auto'; v.src = seg.url; v._askedAt = performance.now(); return; }
  if (!v.paused) v.pause();
  v._go();
}
// How quickly scrubbing shows each picture (average of the last few).
function scrubTiming(ms) {
  tl.scrubMs = tl.scrubMs || [];
  tl.scrubMs.push(ms); if (tl.scrubMs.length > 12) tl.scrubMs.shift();
  const avg = tl.scrubMs.reduce((a, b) => a + b, 0) / tl.scrubMs.length;
  const e = $('tl-scrub-ms'); if (e) e.textContent = `~${Math.round(avg)} ms a picture`;
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
    bar.innerHTML = head(`Updating to GAZE ${to}`, `<span class="ub-ver">${from} → ${to}</span> ${what}`, '⟳')
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
  const when = u.published ? ` · released ${esc(ago(u.published))}` : '';
  const sub = u.error ? `<span class="ub-err">The last try didn't work: ${esc(u.error)}</span>`
    : `<span class="ub-ver">${from} → ${to}</span>${when} · takes a few seconds, recordings carry on after`;
  bar.innerHTML = head(`GAZE ${to} is ready to install`, sub)
    + `<div class="ub-actions"><button type="button" class="primary small" id="ub-go">${u.error ? 'Try again' : 'Update now'}</button>
       <button type="button" class="small ub-later" id="ub-x" title="Hide until the next version">Later</button></div>`;
  bar.hidden = false; requestAnimationFrame(() => bar.classList.add('show'));
  $('ub-x').onclick = () => { store.set('upd-dismissed', u.latest); hide(); };
  $('ub-go').onclick = async () => {
    const go = $('ub-go'); go.disabled = true; go.textContent = 'Starting…';
    try { await api('api/update/install', {method: 'POST'}); showUpdateBar({...u, state: 'downloading', done: 0, total: 0, error: null}); }
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
const SECTIONS = ['cameras', 'profiles', 'storage', 'display', 'ai', 'known', 'notify', 'tasks', 'hardware', 'cloud', 'backup', 'users', 'system', 'logs', 'info'];
let section = 'cameras';
const camPreviewUnbind = [];

async function loadSettings() { [settings, cameraCfgs] = await Promise.all([api('api/settings'), api('api/cameras')]); }

async function showSettings(sec) {
  section = SECTIONS.includes(sec) ? sec : 'cameras';
  document.querySelectorAll('#side a').forEach(a => a.classList.toggle('active', a.dataset.s === section));
  try { await loadSettings(); } catch (e) { toast(e.message, true); return; }
  camPreviewUnbind.splice(0).forEach(u => u());
  const body = $('settings-body');
  // The log view is kept, and lent to the Logs section while it's open.
  const lb = $('logs-box'); if (lb.parentElement !== $('page-logs')) $('page-logs').appendChild(lb);
  ({cameras: renderCameras, profiles: renderProfiles, storage: renderStorage, display: renderDisplay, ai: renderAi, hardware: renderHardware,
    cloud: renderCloud, backup: renderBackup, users: renderUsers, known: renderKnown, notify: renderNotify, tasks: renderTasks, system: renderSystemSettings, info: renderInfo,
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
    <div class="row" style="margin-bottom:8px"><h2 style="flex:1;margin:0">Cameras</h2><button id="scan-cams" title="Look for cameras on your network (ONVIF and RTSP)">🔍 Scan network</button><button class="primary" id="add-cam">+ Add camera</button></div>
    <div id="scan-out"></div>
    <table><thead><tr><th>Preview</th><th>Name</th><th class="hide-sm">Profile</th><th class="hide-sm">Recording</th><th>Status</th><th></th></tr></thead><tbody id="cam-table"></tbody></table>
    ${cameraCfgs.length ? '' : '<div class="empty">No cameras yet. Click “Add camera”.</div>'}</div>`;
  $('add-cam').onclick = () => openCameraDialog();
  $('scan-cams').onclick = scanNetwork;
  if (status && status.new_cameras) scanNetwork();
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
async function scanNetwork() {
  const out = $('scan-out'), btn = $('scan-cams');
  btn.disabled = true; btn.textContent = 'Scanning…';
  out.innerHTML = '<div class="hint" style="margin:8px 0"><span class="spinner small"></span> Looking for cameras on your network (about 5 seconds)…</div>';
  let found = [];
  try { found = await api('api/discover', {method: 'POST'}); } catch (e) { out.innerHTML = `<div class="err">${esc(e.message)}</div>`; }
  btn.disabled = false; btn.textContent = '🔍 Scan network';
  if (!found.length) { out.innerHTML = '<div class="hint" style="margin:8px 0">No cameras answered. Check that ONVIF is turned on in the camera, and that it is on the same network as this computer.</div>'; return; }
  out.innerHTML = `<div class="scan-res"><b>Found on the network</b> <span class="muted">(${found.length})</span>
    <table><tbody>${found.map((f, i) => `<tr><td><b>${esc(f.ip)}</b></td><td>${esc([f.manufacturer, f.hardware, f.name].filter(Boolean).join(' · ') || (f.rtsp ? 'Device with a video stream (RTSP)' : 'Device'))}</td>
      <td>${f.onvif_port ? '<span class="tag">ONVIF</span>' : ''}${f.rtsp ? '<span class="tag">RTSP</span>' : ''}</td>
      <td style="text-align:right">${f.added ? '<span class="muted">already added</span>' : `<button type="button" class="small primary" data-add="${i}">+ Add</button>`}</td></tr>`).join('')}</tbody></table></div>`;
  out.querySelectorAll('[data-add]').forEach(b => b.onclick = async () => {
    const f = found[+b.dataset.add];
    await openCameraDialog();
    // Fill in the quick setup: brand, address and a name; the login is up to you.
    $('c-name').value = f.name || `${(f.manufacturer || 'Camera')} ${f.ip.split('.').pop()}`;
    $('q-ip').value = f.ip;
    $('q-brand').value = guessBrand(f);
    $('q-brand').onchange();
    if (f.onvif_port && $('p-port')) $('p-port').value = f.onvif_port;
    $('q-user').focus();
    toast('Enter the camera’s login, then Test connection');
  });
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
    detect: {motion: true, min_contrast: 24, min_object_px: 300, make_time_ms: 500, min_travel_px: 0, mask: '', ai: false, objects: ['person','car','truck','dog','cat'], min_confidence: 0.5, trigger: 'motion', pre_seconds: 3, post_seconds: 15, fps: 5, ai_interval: 1, ignore_parked: true, ignore_animals: true, parked_seconds: 60}};
  if (clone) c.name = c.name + ' (copy)';
  // A profile: its settings on top of a new camera's.
  if (prof) { Object.assign(c, JSON.parse(JSON.stringify(prof.values || {}))); c.detect = {...c.detect, mask: ''}; c.name = prof.name; }
  const q = parseUrl(c.url) || {};
  // The usual objects, plus any this camera already watches for.
  const objs = [...new Set([...((sysinfo && sysinfo.ai_objects) || ['person','car','truck','motorcycle','bicycle','dog','cat']), ...c.detect.objects])];
  const more = ((sysinfo && sysinfo.ai_more) || []).filter(o => !objs.includes(o));
  const aiOn = settings && settings.ai.provider !== 'none';
  const brandOpts = '<option value="">Format…</option>' + BRANDS.map(b => `<option value="${b[0]}">${esc(b[1])}</option>`).join('');
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
        <span class="hint" id="c-prof-msg">Profiles hold recording, motion and AI settings (not the motion mask).</span></div>
      <div class="tabs" id="c-tabs"><button type="button" data-t="general" class="active">General</button><button type="button" data-t="recording">Recording</button><button type="button" data-t="detection">Motion</button><button type="button" data-t="ai">AI</button><button type="button" data-t="audio">Audio</button><button type="button" data-t="osd">On-screen display</button><button type="button" data-t="ptz">Pan / tilt / zoom</button><button type="button" data-t="zones">Zones</button></div>
      <div data-tab="general" class="form">
        <label for="c-name">Name</label><div><input type="text" id="c-name" required value="${esc(c.name)}" placeholder="Front door"></div>
        <span></span><label class="check"><input type="checkbox" id="c-enabled" ${c.enabled ? 'checked' : ''}> Camera enabled</label>
        <span class="c-restart" hidden></span><div class="c-restart row" hidden><button type="button" id="c-restart">⟳ Restart camera</button><span class="hint" id="c-restart-info"></span></div>
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
        <label for="c-rotate">Rotate picture</label>
        <div><select id="c-rotate">${[[0, 'No rotation'], [90, '90° clockwise'], [180, '180° (upside down)'], [270, '90° anticlockwise']].map(([v, t]) => `<option value="${v}" ${(c.rotate || 0) === v ? 'selected' : ''}>${t}</option>`).join('')}</select>
          <div class="hint">For cameras mounted sideways or upside down: live view, alerts and new recordings are turned the right way.</div></div>
        <label for="c-transport">RTSP transport</label>
        <div><select id="c-transport"><option value="tcp">TCP (recommended)</option><option value="udp">UDP</option></select></div>
        <span></span><div><button type="button" id="c-test">▶ Test connection</button><div id="c-probe"></div></div>
        <h3>Live view</h3>
        ${(() => { const v = c.view || {}, l = status.live, fpsOpts = (cur, def) => [0, 1, 2, 5, 10, 15, 20, 25, 30].map(f => `<option value="${f}" ${f === (cur || 0) ? 'selected' : ''}>${f ? f + ' fps' : 'Default (' + def + ')'}</option>`).join('');
          return `<label for="c-vq">Grid picture</label><div><select id="c-vq"><option value="auto" ${v.grid_quality !== 'main' ? 'selected' : ''}>Sub-stream (light)</option><option value="main" ${v.grid_quality === 'main' ? 'selected' : ''}>Full quality</option></select></div>
        <label for="c-vg">Grid frame rate</label><div><select id="c-vg">${fpsOpts(v.grid_fps, l.fps)}</select></div>
        <label for="c-vh">Full-screen frame rate</label><div><select id="c-vh">${fpsOpts(v.hd_fps, l.hd_fps)}</select>
          <div class="hint">“Default” follows Settings → Live view &amp; display.</div></div>`; })()}
      </div>

      <div data-tab="recording" class="form" hidden>
        <label>Recording mode</label>
        <div class="radio-cards" style="grid-template-columns:1fr">
          ${[['continuous', 'HD 24/7', 'Records the main stream around the clock.'],
             ['continuous_sub', 'Sub-stream 24/7', 'Records the low-resolution stream around the clock. Uses much less space.'],
             ['sub_plus_events', 'Substream 24/7 plus HD AI alerts', 'Low-res around the clock, plus full-quality clips around motion/AI alerts.'],
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
        <span></span><div><label class="check"><input type="checkbox" id="c-tlcopy" ${c.timeline_copy !== false ? 'checked' : ''}> Keep a light stream 2 copy for the timeline</label>
          <div class="hint">With “HD 24/7”: the sub-stream is recorded too, so the timeline opens and scrubs instantly (alerts and downloads still use HD). Uses roughly 5–15% extra space.</div></div>
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
          <div id="d-meter" class="hint" style="margin-top:8px"></div></div>
        <label for="d-pre">Record before alert</label>
        <div class="row"><input type="number" id="d-pre" min="0" max="120" value="${c.detect.pre_seconds}" style="width:110px"><span class="muted">seconds (pre-event buffer, in alert recording modes)</span></div>
        <label for="d-post">Record after activity stops</label>
        <div class="row"><input type="number" id="d-post" min="3" max="600" value="${c.detect.post_seconds}" style="width:110px"><span class="muted">seconds</span></div>
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
        <label>Two-way audio</label>
        <div><label class="check"><input type="checkbox" id="c-talk" ${c.talk ? 'checked' : ''}> Talk through this camera's speaker</label>
          <div class="hint">Shows a <b>🎙️ Push to talk</b> button on the camera's live view: hold it and speak into this phone's or computer's microphone; the visitor hears you through the camera. For cameras with a speaker that support the ONVIF audio back-channel (most Dahua, Hikvision, Amcrest, Axis… doorbells and cameras).</div>
          <label class="check" style="margin-top:6px"><input type="checkbox" id="c-chime" ${c.talk_chime !== false ? 'checked' : ''}> Ring a doorbell chime first, to get the attention of whoever is there (and a short "goodbye" tune when you stop)</label>
          <input type="text" id="c-talk-url" value="${esc(c.talk_url || '')}" placeholder="Speaker address (optional) – empty uses the main stream address" style="margin-top:6px">
          <div class="row" style="margin-top:6px"><button type="button" id="c-talk-test">🔊 Test (plays a beep on the camera)</button><span class="hint" id="c-talk-res" style="margin:0"></span></div></div>
      </div>
      <div data-tab="ai" class="form" hidden>
        <span></span><div><label class="check"><input type="checkbox" id="d-ai" ${c.detect.ai ? 'checked' : ''}> Object recognition (AI)</label>
          <div class="hint">${aiOn ? `Uses <b>${esc(settings.ai.provider)}</b>. Pictures are only sent to the AI while motion is seen (needs motion detection).` : '<span class="warnc">No AI is set up yet.</span> Choose one in Settings → Object recognition.'}</div></div>
        <label>Alert on</label>
        <div><div class="chips" id="d-objs">${objs.map(o => `<label><input type="checkbox" value="${o}" ${c.detect.objects.includes(o) ? 'checked' : ''}>${o}</label>`).join('')}</div>
          ${more.length ? `<details class="more-objs"><summary>More things to watch for…</summary><div class="chips" id="d-more">${more.map(o => `<label><input type="checkbox" value="${o}">${o}</label>`).join('')}</div>
            <div class="hint">Tick one to add it to the list above.</div></details>` : ''}</div>
        <label for="d-conf">Minimum confidence</label>
        <div class="row"><input type="range" id="d-conf" min="5" max="95" value="${Math.round(c.detect.min_confidence * 100)}" style="flex:1"><span id="d-conf-v" class="muted" style="width:40px">${Math.round(c.detect.min_confidence * 100)}%</span></div>
        <label for="d-trig">What makes an alert</label>
        <div><select id="d-trig"><option value="motion">Any motion</option><option value="objects">Only recognized objects (ignores trees, shadows, rain)</option></select>
          <label class="check" style="margin-top:6px"><input type="checkbox" id="d-animals" ${c.detect.ignore_animals !== false ? 'checked' : ''}> Check motion with AI first — no alert when it's a cat (or another animal you don't watch for)</label>
          <div class="hint">With “Any motion”, the frames of each motion are sent to the AI before it becomes an alert. If the AI sees a cat, dog, bird… that isn't ticked above, and nothing you do watch for, the motion isn't kept on the Alerts page. Tick “cat” above to get alerts for cats.</div></div>
        <label for="d-int">AI check interval</label>
        <div class="row"><input type="number" id="d-int" min="0.2" max="60" step="0.1" value="${c.detect.ai_interval}" style="width:110px"><span class="muted">seconds between AI checks while there's motion (lower = quicker to recognise, more work for the computer)</span></div>
        <label>Stationary objects</label>
        <div><label class="check"><input type="checkbox" id="d-parked" ${c.detect.ignore_parked !== false ? 'checked' : ''}> Stop alerting for objects that stay in one place</label>
          <div class="row" style="margin-top:4px"><span class="muted">after</span><input type="number" id="d-parked-s" min="10" max="3600" value="${c.detect.parked_seconds || 60}" style="width:90px"><span class="muted">seconds without moving, for:</span></div>
          <div class="chips" id="d-parked-objs" style="margin-top:4px">${[...new Set([...objs, ...(c.detect.parked_objects || [])])].map(o => `<label><input type="checkbox" value="${o}" ${(c.detect.parked_objects || ['car', 'truck', 'bus', 'motorcycle', 'bicycle', 'boat']).includes(o) ? 'checked' : ''}>${o}</label>`).join('')}</div>
          <div class="hint">A car parked in the driveway alerts when it arrives, then stays quiet while it stands still (people walking past still alert). It alerts again as soon as it moves or drives off.</div></div>
        <label>Package delivery</label>
        <div><label class="check"><input type="checkbox" id="d-pkg" ${c.detect.packages ? 'checked' : ''}> Notice parcels delivered or picked up</label>
          <div class="surface" id="d-pkg-area" style="aspect-ratio:16/9;max-width:560px;touch-action:none;cursor:crosshair;margin-top:6px">
            <img alt="" id="d-pkg-img" style="object-fit:fill"><div class="msg">${id && !clone ? '' : 'Save the camera first to see its picture here.'}</div>
            <svg id="d-pkg-svg" style="position:absolute;inset:0;width:100%;height:100%;pointer-events:none" viewBox="0 0 1 1" preserveAspectRatio="none"></svg></div>
          <div class="row" style="margin-top:6px"><button type="button" class="small" id="d-pkg-clear">Clear</button>
            <span class="hint" style="margin:0" id="d-pkg-hint">Drag a box on the picture around where parcels are left (doorstep, porch).</span></div>
          <div class="hint">GAZE compares this spot before and after each visit and notes when something was left or taken, with before/after pictures in the alert.</div></div>
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
          <div class="hint">Uses ONVIF, which most PTZ cameras support (you may need to turn ONVIF on in the camera's own settings, and some brands need a separate ONVIF user).</div></div>
        <label for="p-host">Camera address</label><div><input type="text" id="p-host" value="${esc(c.ptz.host)}" placeholder="Same as the stream (${esc((parseUrl(c.url) || {}).ip || 'from the URL')})"></div>
        <label for="p-port">ONVIF port</label><div class="row"><input type="number" id="p-port" min="1" max="65535" value="${c.ptz.port}" style="width:110px">
          <span class="muted">80 for most cameras, 8000 Reolink, 2020 Tapo</span></div>
        <label for="p-user">ONVIF login</label><div class="row"><input type="text" id="p-user" value="${esc(c.ptz.username)}" placeholder="Same as stream" style="flex:1" autocomplete="off"><input type="password" id="p-pass" value="${esc(c.ptz.password)}" placeholder="Same as stream" style="flex:1" autocomplete="new-password"></div>
        <label for="p-speed">Speed</label><div class="row"><input type="range" id="p-speed" min="10" max="100" value="${Math.round(c.ptz.speed * 100)}" style="flex:1"><span class="muted" id="p-speed-v" style="width:44px">${Math.round(c.ptz.speed * 100)}%</span></div>
        <span></span><div class="row"><label class="check"><input type="checkbox" id="p-inv-pan" ${c.ptz.invert_pan ? 'checked' : ''}> Reverse left/right</label><label class="check"><input type="checkbox" id="p-inv-tilt" ${c.ptz.invert_tilt ? 'checked' : ''}> Reverse up/down</label></div>
        <span></span><div><button type="button" id="p-test">Test PTZ connection</button><div id="p-result" class="hint" style="margin-top:6px"></div></div>
      </div>
      <div data-tab="zones" hidden>
        <div class="row" style="margin-bottom:8px"><button type="button" id="z-add-area">＋ Area</button><button type="button" id="z-add-line">＋ Line</button>
          <span class="muted" id="z-msg">Click on the picture to place the corners; click the first corner (or Done) to finish.</span><button type="button" id="z-done" hidden class="primary">Done</button></div>
        <div class="surface zone-ed" id="z-prev"><img alt=""><svg id="z-svg" viewBox="0 0 1 1" preserveAspectRatio="none"></svg></div>
        <div id="z-list"></div>
      </div>
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
    if (b.dataset.t === 'zones') drawZones();
  });
  setupRestartButton(id && !clone ? id : null);
  setupProfiles(c);
  $('cam-dialog').classList.toggle('profile-mode', !!prof);
  if (prof) {
    // Only the settings a profile holds: Recording, Motion and AI.
    f.querySelector('.head h2').textContent = prof.idx >= 0 ? `Edit profile “${prof.name}”` : `New profile “${prof.name}”`;
    f.querySelectorAll('#c-tabs button').forEach(b => b.hidden = !['recording', 'detection', 'ai'].includes(b.dataset.t));
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
  // "More things to watch for": a ticked one joins the list (ticked).
  if ($('d-more')) $('d-more').onchange = (e) => {
    const inp = e.target; if (!inp.checked) return;
    const o = inp.value; inp.closest('label').remove();
    $('d-objs').insertAdjacentHTML('beforeend', `<label><input type="checkbox" value="${esc(o)}" checked>${esc(o)}</label>`);
    if (!$('d-parked-objs').querySelector(`input[value="${CSS.escape(o)}"]`)) $('d-parked-objs').insertAdjacentHTML('beforeend', `<label><input type="checkbox" value="${esc(o)}">${esc(o)}</label>`);
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
$('cam-dialog').addEventListener('close', () => { if (pkgUnbind) { pkgUnbind(); pkgUnbind = null; } stopPreviews(); stopMotionMeter(); if (osdPrevUnbind) { osdPrevUnbind(); osdPrevUnbind = null; } if (zoneUnbind) { zoneUnbind(); zoneUnbind = null; } });

// ---------- motion mask editor & live motion readout ----------
let maskCells = [], maskUnbind = null, meterTimer = null;
// One cell per pixel motion detection looks at (a 640×360 picture shrunk 4×).
const MASK_W = 160, MASK_H = 90;
// Stored as "160x90:" + run lengths (unmasked, masked, unmasked, …).
function maskValue() {
  if (!maskCells.some(x => x)) return '';
  const runs = []; let cur = false, n = 0;
  for (const v of maskCells) { if (v === cur) n++; else { runs.push(n); cur = v; n = 1; } }
  runs.push(n);
  return `${MASK_W}x${MASK_H}:` + runs.join(',');
}
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
let maskTool = 'paint';
function setupMaskEditor(mask, camId) {
  maskCells = decodeMask(mask);
  const cv = $('d-mask-grid'), ctx = cv.getContext('2d');
  const paint = () => {
    const img = ctx.createImageData(MASK_W, MASK_H), d = img.data;
    for (let i = 0; i < maskCells.length; i++) if (maskCells[i]) { d[i * 4] = 239; d[i * 4 + 1] = 68; d[i * 4 + 2] = 68; d[i * 4 + 3] = 115; }
    ctx.putImageData(img, 0, 0);
  };
  paint();
  $('d-mask-tool').querySelectorAll('button').forEach(b => {
    b.classList.toggle('on', b.dataset.v === maskTool);
    b.onclick = () => { maskTool = b.dataset.v; $('d-mask-tool').querySelectorAll('button').forEach(x => x.classList.toggle('on', x === b)); cv.style.cursor = maskTool === 'erase' ? 'cell' : 'crosshair'; };
  });
  const brush = $('d-mask-brush');
  brush.value = store.get('maskBrush', '3'); brush.onchange = () => store.set('maskBrush', brush.value);
  const at = (e) => { const r = cv.getBoundingClientRect(); return [(e.clientX - r.left) / r.width * MASK_W, (e.clientY - r.top) / r.height * MASK_H]; };
  // A round brush, `size` cells across, at (x, y) in cells.
  const dab = (x, y, val) => {
    const size = +brush.value || 3, rad = size / 2;
    let changed = false;
    for (let cy = Math.floor(y - rad); cy <= Math.ceil(y + rad); cy++) for (let cx = Math.floor(x - rad); cx <= Math.ceil(x + rad); cx++) {
      if (cx < 0 || cy < 0 || cx >= MASK_W || cy >= MASK_H) continue;
      if (size > 1 && (cx + 0.5 - x) ** 2 + (cy + 0.5 - y) ** 2 > rad * rad) continue;
      if (size === 1 && (cx !== Math.floor(x) || cy !== Math.floor(y))) continue;
      const i = cy * MASK_W + cx;
      if (maskCells[i] !== val) { maskCells[i] = val; changed = true; }
    }
    return changed;
  };
  let val = null, last = null;
  // Fill along the line from the last point so fast strokes leave no gaps.
  const stroke = (e) => {
    const p = at(e), from = last || p;
    const steps = Math.max(1, Math.ceil(Math.hypot(p[0] - from[0], p[1] - from[1]) / 0.5));
    let changed = false;
    for (let k = 1; k <= steps; k++) changed = dab(from[0] + (p[0] - from[0]) * k / steps, from[1] + (p[1] - from[1]) * k / steps, val) || changed;
    last = p;
    if (changed) paint();
  };
  cv.oncontextmenu = (e) => e.preventDefault();
  cv.onpointerdown = (e) => { e.preventDefault(); val = !(maskTool === 'erase' || e.button === 2 || e.shiftKey); last = null; cv.setPointerCapture(e.pointerId); stroke(e); };
  cv.onpointermove = (e) => { if (val !== null) stroke(e); };
  cv.onpointerup = cv.onpointercancel = () => { val = null; last = null; };
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
// Package delivery spot (camera → AI): drag a box on the camera picture.
let pkgArea = [], pkgUnbind = null;
function setupPackageArea(c, camId) {
  pkgArea = (c.detect.package_area || []).map(p => [...p]);
  const surf = $('d-pkg-area'), svg = $('d-pkg-svg');
  const draw = () => {
    svg.innerHTML = pkgArea.length >= 3 ? `<polygon points="${pkgArea.map(p => p.join(',')).join(' ')}" fill="rgba(255,176,32,.25)" stroke="#ffb020" stroke-width="0.006" vector-effect="non-scaling-stroke"/>` : '';
    $('d-pkg-hint').textContent = pkgArea.length >= 3 ? 'Package spot set. Drag again to redraw it.' : 'Drag a box on the picture around where parcels are left (doorstep, porch).';
  };
  // The camera's live picture (light stream 2, as for the motion mask): a
  // full-size snapshot can take long or fail on big / H.265 streams.
  if (pkgUnbind) { pkgUnbind(); pkgUnbind = null; }
  if (camId) {
    const img = $('d-pkg-img'), m = surf.querySelector('.msg');
    if (m) { m.textContent = 'Connecting…'; m.hidden = false; }
    img.onload = () => { if (m) m.hidden = true; };
    pkgUnbind = Live.bind(img, camId, 'sub');
  }
  let start = null;
  const at = (e) => { const r = surf.getBoundingClientRect(); return [Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (e.clientY - r.top) / r.height))]; };
  surf.onpointerdown = (e) => { start = at(e); surf.setPointerCapture(e.pointerId); e.preventDefault(); };
  surf.onpointermove = (e) => {
    if (!start) return;
    const p = at(e), [x0, x1] = [Math.min(start[0], p[0]), Math.max(start[0], p[0])], [y0, y1] = [Math.min(start[1], p[1]), Math.max(start[1], p[1])];
    pkgArea = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(q => q.map(v => Math.round(v * 1000) / 1000));
    draw();
  };
  surf.onpointerup = () => {
    if (start && pkgArea.length >= 3 && (pkgArea[1][0] - pkgArea[0][0] < 0.02 || pkgArea[2][1] - pkgArea[1][1] < 0.02)) pkgArea = [];
    start = null; draw();
    if (pkgArea.length >= 3) $('d-pkg').checked = true;
  };
  $('d-pkg-clear').onclick = () => { pkgArea = []; draw(); };
  draw();
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
  return profileEdit ? saveProfileForm(body) : finishSave(body);
}
// Save the camera window's Recording, Motion and AI settings as the profile
// being edited; cameras following it change too.
async function saveProfileForm(body) {
  const pe = profileEdit, p = {name: pe.name};
  for (const k of PROFILE_KEYS) p[k] = JSON.parse(JSON.stringify(body[k]));
  p.detect.mask = ''; p.detect.packages = false; p.detect.package_area = [];
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
    record_audio: chk('c-audio'), transcode: chk('c-transcode'), record_fps: num('c-fps'), segment_seconds: num('c-seg'),
    storage_dir: val('c-dir').trim(), retention_days: Math.round(num('c-ret')), max_storage_gb: num('c-max'),
    rotate: +val('c-rotate') || 0, profile: camProfile, zones: dlgZones.map(z => ({...z})),
    timeline_copy: $('c-tlcopy') ? chk('c-tlcopy') : true,
    osd: readOsd(),
    view: {grid_quality: val('c-vq') || 'auto', grid_fps: +val('c-vg') || 0, hd_fps: +val('c-vh') || 0},
    ptz: {enabled: chk('p-on'), host: val('p-host').trim(), port: Math.round(num('p-port')) || 80, username: val('p-user').trim(), password: $('p-pass').value,
      speed: num('p-speed') / 100, invert_pan: chk('p-inv-pan'), invert_tilt: chk('p-inv-tilt')},
    detect: {motion: chk('d-motion'), min_contrast: num('d-contrast'), min_object_px: Math.round(num('d-objpx')),
      make_time_ms: Math.round(num('d-make') * 1000), min_travel_px: Math.round(num('d-travel')), mask: maskValue(), ai: chk('d-ai'), objects, min_confidence: num('d-conf') / 100,
      trigger: val('d-trig'), pre_seconds: Math.round(num('d-pre')), post_seconds: Math.round(num('d-post')), fps: Math.round(num('d-fps')), ai_interval: num('d-int'), ignore_parked: chk('d-parked'), ignore_animals: chk('d-animals'), parked_seconds: Math.max(10, Math.round(num('d-parked-s')) || 60),
      parked_objects: [...document.querySelectorAll('#d-parked-objs input:checked')].map(i => i.value), packages: chk('d-pkg') && pkgArea.length >= 3, package_area: pkgArea},
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

// ---------- zones ----------
let dlgZones = [], zoneDraft = null, zoneUnbind = null;
const ZONE_COLORS = ['#22c55e', '#4f8cff', '#f59e0b', '#ec4899', '#a78bfa', '#14b8a6', '#ef4444', '#eab308'];
const ZONE_OBJECTS = ['person', 'car', 'truck', 'bus', 'motorcycle', 'bicycle', 'dog', 'cat', 'bird'];
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
  dlgZones.push({name: d.kind === 'line' ? `Line ${n}` : `Area ${n}`, kind: d.kind, points: d.points, objects: [], required: false, loiter_seconds: 0, direction: 'any', watch_items: false});
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
            <label class="check"><input type="checkbox" class="z-req" ${z.required ? 'checked' : ''}> Only alert for objects inside this area</label>
            <label class="check">Loitering after <input type="number" class="z-loiter" min="0" max="3600" value="${z.loiter_seconds}" style="width:80px"> s <span class="muted">(0 = off)</span></label>
            <label class="check"><input type="checkbox" class="z-items" ${z.watch_items ? 'checked' : ''}> Package delivery: notice parcels left or taken</label><div class="hint">Draw the area around your doorstep. With smart search on, GAZE tells “Package delivered” from “Package picked up”.</div></div>`}`;
    const upd = () => {
      z.name = el.querySelector('.z-name').value.trim() || z.name;
      z.objects = [...el.querySelectorAll('.z-objs input:checked')].map(i => i.value);
      if (z.kind === 'line') z.direction = el.querySelector('.z-dir').value;
      else { z.required = el.querySelector('.z-req').checked; z.loiter_seconds = Math.max(0, Math.round(+el.querySelector('.z-loiter').value || 0)); z.watch_items = el.querySelector('.z-items').checked; }
      drawZones();
    };
    if (z.kind === 'line') el.querySelector('.z-dir').value = z.direction || 'any';
    el.addEventListener('input', upd); el.addEventListener('change', upd);
    el.querySelector('.z-del').onclick = () => { dlgZones.splice(zi, 1); drawZones(); renderZoneList(); };
    box.appendChild(el);
  });
}

// ---------- recording profiles ----------
const PROFILE_KEYS = ['record_mode', 'schedule_enabled', 'schedule', 'record_audio', 'transcode', 'record_fps', 'segment_seconds', 'retention_days', 'max_storage_gb', 'detect'];
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
    : 'Profiles hold recording, motion and AI settings (not the motion mask or camera connection settings).');
  showLink();
  // Editing a profile setting by hand gives the camera its own settings.
  const tabs = $('f-cam').querySelectorAll('[data-tab=recording], [data-tab=detection]');
  tabs.forEach(t => t.addEventListener('input', (e) => {
    if (!camProfile || e.isTrusted === false || e.target.closest('#d-mask')) return;
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
const REC_MODE_NAMES = {continuous: 'Records all the time (full quality)', continuous_sub: 'Records all the time (sub-stream)', sub_plus_events: 'Sub-stream all the time + full-quality alerts', events: 'Records alerts only', off: 'Not recording'};
function renderProfiles(body) {
  const list = settings.profiles || [];
  const followers = (name) => cameraCfgs.filter(c => (c.profile || '').toLowerCase() === name.toLowerCase());
  const summary = (p) => [REC_MODE_NAMES[p.record_mode] || p.record_mode,
    p.detect.motion ? 'motion detection' : 'no motion detection',
    p.detect.ai ? 'AI: ' + p.detect.objects.join(', ') : 'no AI'].join(' · ');
  body.innerHTML = `<div class="card"><div class="row" style="margin-bottom:6px"><h2 style="margin:0;flex:1">Profiles</h2>
      <button type="button" id="pr-new">＋ New profile</button>
      ${cameraCfgs.length ? `<select id="pr-from" title="Start a profile from a camera's recording, motion and AI settings"><option value="">＋ New from a camera…</option>${cameraCfgs.map(c => `<option value="${esc(c.id)}">${esc(c.name)}</option>`).join('')}</select>` : ''}</div>
    <p class="muted" style="margin-top:0">A profile holds recording, motion and AI settings. Cameras following a profile change when it's edited — set it up once for all the cameras that should behave the same. Each camera keeps its own picture-based settings (ignore areas, package spot, zones). Choose which profile a camera follows in its settings (top of the window).</p>
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
  set('d-trig', d.trigger); set('d-pre', d.pre_seconds); set('d-post', d.post_seconds); set('d-fps', d.fps); set('d-int', d.ai_interval); set('d-parked', d.ignore_parked !== false); set('d-animals', d.ignore_animals !== false); set('d-parked-s', d.parked_seconds || 60);
  if (d.parked_objects) document.querySelectorAll('#d-parked-objs input').forEach(i => i.checked = d.parked_objects.includes(i.value));
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
    if (camDlgGen === gen && !$('cam-dialog').open) { $('cam-dialog').showModal(); err(e.message); }
    else toast('Camera not saved: ' + e.message, true);
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
    <label for="s-max">Maximum storage</label><div class="row"><input type="number" id="s-max" min="0" step="0.5" value="${s.max_storage_gb}" style="width:110px"><span class="muted">GB in total (0 = use the whole disk)</span></div>
    <label for="s-free">Keep free on disk</label><div class="row"><input type="number" id="s-free" min="0" step="0.5" value="${s.min_free_gb}" style="width:110px"><span class="muted">GB (oldest recordings are deleted to keep this free)</span></div>
    <span ${TIMELAPSE ? '' : 'hidden'}></span><div ${TIMELAPSE ? '' : 'hidden'}><label class="check"><input type="checkbox" id="s-tl" ${s.prebuild_timelapse !== false ? 'checked' : ''}> Prepare time-lapse previews for the last 24 hours automatically</label>
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
  const corners = SPOTS;
  const custom = !CLOCK_FORMATS.includes(d.clock_format);
  body.innerHTML = `<form class="card" id="f-display"><h2>Live view</h2><div class="form">
    <label for="l-fps">Grid frame rate</label><div class="row"><input type="number" id="l-fps" min="1" max="30" value="${l.fps}" style="width:110px"><span class="muted">fps for the camera grid (sub-streams)</span></div>
    <label for="l-width">Grid picture width</label><div><select id="l-width">${[[480, '480 (low)'], [640, '640'], [960, '960'], [1280, '1280 (HD)'], [1920, '1920'], [0, 'Native']].map(([v, t]) => `<option value="${v}" ${v === l.width ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    <label for="l-hd">Full-screen frame rate</label><div class="row"><input type="number" id="l-hd" min="1" max="30" value="${l.hd_fps}" style="width:110px"><span class="muted">fps when a camera is opened (main stream)</span></div>
    <label for="l-q">Picture quality</label><div><select id="l-q">${[[3, 'Very high'], [6, 'High'], [10, 'Medium'], [16, 'Low (saves bandwidth)']].map(([v, t]) => `<option value="${v}" ${v === l.quality ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    <label for="l-ds">Data saver (this browser)</label><div><select id="l-ds">
        <option value="auto" ${dsMode === 'auto' ? 'selected' : ''}>Automatic — on for phones (mobile data), off on computers</option>
        <option value="on" ${dsMode === 'on' ? 'selected' : ''}>Always on</option><option value="off" ${dsMode === 'off' ? 'selected' : ''}>Off</option></select>
      <div class="hint">Smaller live pictures, fewer frames per second and light video for recordings. Saved in this browser only; now ${dataSaver ? '<b>on</b>' : 'off'} here.</div></div>
    <span></span><div><label class="check"><input type="checkbox" id="l-pre" ${l.preload_hd_streams ? 'checked' : ''}> Keep HD streams ready in the background</label>
      <div class="hint">Off (recommended): a camera's full-quality stream is only decoded while you have it open; until it arrives you see the sub-stream picture. On: opening a camera shows HD instantly, but every camera's HD stream is decoded all the time — a lot of GPU/CPU.</div></div>
    <h3>Overlays on live video</h3>
    <label>Fullscreen button</label><div class="chips" id="g-fs">${corners.map(([k, t]) => `<label><input type="checkbox" value="${k}" ${d.fullscreen_corners.includes(k) ? 'checked' : ''}>${t}</label>`).join('')}</div>
    <label>Preview</label>
    <div class="surface" id="go-prev" style="aspect-ratio:16/9;max-width:560px">${status.cameras[0] ? `<img alt="" style="object-fit:fill" src="live/${enc(status.cameras[0].id)}/frame.jpg">` : ''}</div>
    <span></span><div class="hint" style="margin-top:-4px">For every camera without its own on-screen display (camera settings → On-screen display).</div>
    ${osdItemsHtml('go', d.osd)}
    <label for="g-fmt">Date &amp; time format</label><div><select id="g-fmt">${CLOCK_FORMATS.map(f => `<option ${f === d.clock_format ? 'selected' : ''}>${f}</option>`).join('')}<option value="custom" ${custom ? 'selected' : ''}>Custom…</option></select>
      <input type="text" id="g-custom" value="${esc(d.clock_format)}" style="margin-top:6px" ${custom ? '' : 'hidden'}>
      <div class="hint">Preview: <b id="g-prev"></b><br>Codes: YYYY YY · MMMM MMM MM M · DD D · dddd ddd · HH H (24 h) hh h (12 h) · mm ss · A a (AM/PM)</div></div>
  </div><div class="actions"><button class="primary">Save</button></div></form>`;
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
      <label for="ai-device">Run AI on</label><div><select id="ai-device">
        <option value="gpu" ${a.device !== 'cpu' ? 'selected' : ''}>Graphics card (GPU) — recommended</option>
        <option value="cpu" ${a.device === 'cpu' ? 'selected' : ''}>Processor (CPU)</option></select>
        <div class="hint">For the built-in object recognition and faces &amp; plates. The graphics card is much faster and leaves the processor free for video; it works with any recent NVIDIA, AMD or Intel graphics on Windows (DirectX 12). The GPU runtime (about 25 MB) downloads by itself the first time; until then, or if the graphics card can't be used, the processor takes over.</div>
        <div class="hint" id="ai-device-st"></div></div>
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
  const form = () => ({provider: document.querySelector('input[name=ai-p]:checked').value, builtin_model: val('ai-model'),
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
      <span></span><div><label class="check"><input type="checkbox" id="h-auto" ${h.auto !== false ? 'checked' : ''}> Choose automatically (recommended)</label>
        <div class="hint">At every start GAZE tries the graphics card's video decoder and encoder and uses the first that works — for every stream, so the processor stays free. Untick to choose below yourself.</div></div>
      <label for="h-dec">Video decoding</label><div><select id="h-dec">${Object.entries(DEC_LABELS).map(([k, v]) => { const ok = k === 'none' || k === 'auto' || caps.hwaccels.includes(k); return `<option value="${k}" ${ok ? '' : 'disabled'}>${v}${ok ? '' : ' – not in this ffmpeg'}</option>`; }).join('')}</select>
        <div class="hint">${caps.hwaccels.length ? 'Your ffmpeg supports: ' + esc(caps.hwaccels.join(', ')) + '. Use Test to check the hardware is really there.' : 'Your ffmpeg reports no GPU support.'}</div></div>
      <label for="h-enc">H.264 encoder</label><div><select id="h-enc">${Object.entries(ENC_LABELS).map(([k, v]) => { const ok = k === 'auto' || caps.encoders.includes(k); return `<option value="${k}" ${ok ? '' : 'disabled'}>${v}${ok ? '' : ' – not in this ffmpeg'}</option>`; }).join('')}</select>
        <div class="hint">“Automatic” picks the encoder that matches the decoder.</div></div>
      <span></span><div><label class="check"><input type="checkbox" id="h-sub" ${h.gpu_substreams ? 'checked' : ''}> Also use the GPU for sub-streams</label>
        <div class="hint">Off (recommended): the GPU only decodes full-quality streams you are watching. Small sub-streams (grid, motion detection) are cheaper on the CPU than on the GPU plus copying the pictures back.</div></div>
      <span></span><div><label class="check"><input type="checkbox" id="h-bal" ${h.balance !== false ? 'checked' : ''}> Smart load balancing</label>
        <div class="hint">When the GPU is busy (85% or more, e.g. a game or another program) and the processor has room, new live views, conversions and exports run on the CPU instead, and motion detection moves its decoding to whichever has capacity — and back when the GPU is free again. The AI always runs on the CPU.</div></div>
      <label for="h-dev" class="vaapi">GPU device</label><div class="vaapi"><input type="text" id="h-dev" value="${esc(h.device)}" placeholder="/dev/dri/renderD128"><div class="hint">VAAPI only. Empty = default.</div></div>
    </div>
    <div class="actions"><button class="primary">Save</button><button type="button" id="h-test">Test selected options</button></div>
    <div id="h-result" style="margin-top:12px"></div></form>`;
  $('h-dec').value = h.hwaccel; $('h-enc').value = h.encoder;
  const sync = () => document.querySelectorAll('.vaapi').forEach(e => e.hidden = !(val('h-dec') === 'vaapi' || val('h-enc') === 'h264_vaapi'));
  $('h-dec').onchange = $('h-enc').onchange = sync; sync();
  const form = () => ({auto: chk('h-auto'), hwaccel: val('h-dec'), encoder: val('h-enc'), device: val('h-dev').trim(), gpu_substreams: chk('h-sub'), balance: chk('h-bal')});
  // Choosing by hand switches "automatically" off.
  ['h-dec', 'h-enc', 'h-sub'].forEach(id => $(id).addEventListener('change', () => { $('h-auto').checked = false; }));
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
    <div class="form"><span></span><label class="check"><input type="checkbox" id="cl-on" ${c.enabled ? 'checked' : ''}> Back up</label></div>
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
  body.innerHTML = `<form class="card" id="f-sys"><h2>System</h2><div class="form">
    <label for="y-name">Name of this system</label><div><input type="text" id="y-name" required value="${esc(s.name)}"></div>
    <label>Web address</label><div id="y-addr"></div>
    <label for="y-port">Port</label><div class="row"><input type="number" id="y-port" min="1" max="65535" value="${esc(port)}" style="width:110px"><span class="muted">1–65535 (default 8080)</span></div>
    <label for="y-bind">Reachable from</label><div><select id="y-bind"><option value="0.0.0.0">Every device on my network (all network adapters)</option><option value="127.0.0.1">Only this computer</option>
      ${addrs.map(a => `<option value="${esc(a)}">Only via ${esc(a)}</option>`).join('')}</select><div class="hint">Changing the port or this setting takes effect after the NVR restarts.</div></div>
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
  showAddr();
  $('f-sys').onsubmit = async (e) => {
    e.preventDefault();
    const listen = `${val('y-bind')}:${Math.round(num('y-port'))}`;
    if (await saveSettings({server: {name: val('y-name'), listen, ffmpeg_path: val('y-ffmpeg')}})) { sysinfo = null; renderSystemSettings(body); }
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
      <div class="form" style="margin-top:8px"><span></span><div><label class="check"><input type="checkbox" id="upd-auto" ${settings.update && settings.update.auto ? 'checked' : ''}> Install updates automatically</label>
        <div class="hint">GAZE checks for a new version once a day. Off: a bar at the top tells you, and you choose when (⟳ Updates next to your name checks right away). On: it installs as soon as it's found, when no alert is going on — downloads, checks it, swaps it in and restarts in a few seconds (also as a Windows service).</div></div>
        <label for="upd-repo">Update from</label><div><div class="row"><span class="muted">github.com/</span><input type="text" id="upd-repo" value="${esc((settings.update && settings.update.repo) || '')}" placeholder="owner/repository" style="flex:1"><button type="button" id="upd-repo-save">Save</button></div>
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
        body: JSON.stringify({username: val('auth-user'), password: $('auth-pass').value, remember: chk('auth-remember')})});
      if (!r.ok) return fail((await r.json().catch(() => ({}))).error || 'Could not sign in');
      $('auth-pass').value = $('auth-pass2').value = '';
      boot();
    } catch (err) { fail(err.message); } finally { $('auth-go').disabled = false; }
  };
  setTimeout(() => (val('auth-user') ? $('auth-pass') : $('auth-user')).focus(), 50);
}
async function boot() {
  let st;
  try { st = await (await fetch('api/auth/state')).json(); } catch (e) { setTimeout(boot, 3000); return; }
  $('auth-name').textContent = st.name || 'GAZE';
  if (st.setup_needed) return showAuth('setup');
  if (!st.user) return showAuth('login');
  me = st.user;
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
  route();
}
const isAdmin = () => me && me.role === 'admin';
$('um-btn').onclick = (e) => { e.stopPropagation(); $('um-ds').value = dsMode; $('um-pop').hidden = !$('um-pop').hidden; };
$('um-ds').onchange = () => { setDataSaver(val('um-ds')); toast('Data saver: ' + $('um-ds').selectedOptions[0].textContent); };
$('um-out').onclick = async () => { await fetch('api/auth/logout', {method: 'POST'}); location.hash = '#live'; location.reload(); };
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
      <span></span><div><label class="check"><input type="checkbox" id="k-faces" ${a.faces ? 'checked' : ''}> Look for faces in alerts and name the people below</label>
        <div class="hint">Uses OpenCV's YuNet (finds faces) and SFace (recognises them), about 40 MB downloaded once. Faces need to be reasonably large and facing the camera; it works best on door and entrance cameras.</div></div>
      <span></span><div><label class="check"><input type="checkbox" id="k-plates" ${a.plates ? 'checked' : ''}> Read number plates of vehicles in alerts</label>
        <div class="hint">Uses fast-alpr's plate finder and reader (about 10 MB). Plates must be readable in the full-quality recording.</div></div>
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
const NOTIFY_OBJECTS = [['person', 'People'], ['car', 'Cars'], ['truck', 'Trucks'], ['bus', 'Buses'], ['motorcycle', 'Motorbikes'], ['bicycle', 'Bicycles'], ['package', 'Packages'], ['dog', 'Dogs'], ['cat', 'Cats']];
function randomTopic() {
  const a = new Uint8Array(16); crypto.getRandomValues(a);
  return 'gaze-' + Array.from(a, b => 'abcdefghijkmnpqrstuvwxyz23456789'[b % 32]).join('');
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
      <label for="n-topic">Topic</label><div><div class="row"><input type="text" id="n-topic" value="${esc(n.topic || randomTopic())}" style="flex:1"><button type="button" id="n-new">New private topic</button></div>
        <div class="hint">On the public ntfy.sh server anyone who knows the topic name can read it, so keep this long random name.</div></div>
      <label for="n-server">ntfy server</label><div><input type="text" id="n-server" value="${esc(n.server)}" placeholder="https://ntfy.sh">
        <div class="hint">Pictures pass through this server. For full privacy run your own ntfy server and enter its address (and an access token below).</div></div>
      <label for="n-token">Access token</label><div><input type="password" id="n-token" autocomplete="new-password" placeholder="${n.has_token ? 'Saved – leave empty to keep' : 'Only for a protected topic or your own server'}"></div>
      <label>Notify for</label><div class="checks">${NOTIFY_OBJECTS.map(([k, t]) => `<label class="check"><input type="checkbox" class="n-obj" value="${k}" ${n.objects.includes(k) ? 'checked' : ''}> ${t}</label>`).join('')}</div>
      <label>Cameras</label><div class="checks">${status.cameras.map(c => `<label class="check"><input type="checkbox" class="n-cam" value="${esc(c.id)}" ${!n.cameras.length || n.cameras.includes(c.id) ? 'checked' : ''}> ${esc(c.name)}</label>`).join('')}</div>
      <label for="n-gap">How often</label><div class="row"><span class="muted">at most one every</span><input type="number" id="n-gap" min="0" max="240" value="${n.gap_minutes}" style="width:80px"><span class="muted">minutes per camera</span></div>
      <label for="n-nfrom">At night</label><div class="row"><input type="time" id="n-nfrom" value="${esc(n.night_from)}" style="width:130px"><span class="muted">to</span><input type="time" id="n-nto" value="${esc(n.night_to)}" style="width:130px"><span class="muted">: one every</span><input type="number" id="n-ngap" min="0" max="240" value="${n.night_gap_minutes}" style="width:80px"><span class="muted">minutes — people at night count as suspicious</span></div>
      <span></span><div><label class="check"><input type="checkbox" id="n-sum" ${n.summary ? 'checked' : ''}> Send a summary when the alert is over</label>
        <div class="hint">What was seen and for how long, who was recognised, number plates, zones and parcels.</div></div>
      <label for="n-link">Open GAZE at</label><div><input type="text" id="n-link" value="${esc(n.link)}" placeholder="https://my-pc.tail1234.ts.net">
        <div class="hint">Tapping a notification opens that alert in GAZE. Left empty, this computer's address on your home network is used (works on your Wi-Fi); for away from home use e.g. your computer's Tailscale name.</div></div>
    </div></div>
    <div class="card"><div class="actions" style="margin:0"><button type="button" class="primary" id="n-save">Save</button><button type="button" id="n-test">Send a test</button>
      <span class="muted" style="margin-left:auto">${st.sent ? `${st.sent} sent since GAZE started${st.last_sent ? ', last ' + fmtDate(st.last_sent) : ''}` : ''}${st.last_error ? ` <span class="errc">· ${esc(st.last_error)}</span>` : ''}</span></div></div>`;
  const upd = () => { $('n-sub').textContent = sub(); };
  upd(); $('n-topic').oninput = upd; $('n-server').oninput = upd;
  $('n-new').onclick = () => { $('n-topic').value = randomTopic(); upd(); };
  $('n-copy').onclick = () => { navigator.clipboard && navigator.clipboard.writeText(sub()).then(() => toast('Copied')); };
  const form = () => {
    const cams = [...body.querySelectorAll('.n-cam')], picked = cams.filter(c => c.checked).map(c => c.value);
    return {enabled: chk('n-on'), server: val('n-server').trim() || 'https://ntfy.sh', topic: val('n-topic').trim(), token: val('n-token'),
      objects: [...body.querySelectorAll('.n-obj:checked')].map(c => c.value), cameras: picked.length === cams.length ? [] : picked,
      gap_minutes: Math.max(0, num('n-gap') | 0), night_from: val('n-nfrom') || '22:00', night_to: val('n-nto') || '06:00',
      night_gap_minutes: Math.max(0, num('n-ngap') | 0), summary: chk('n-sum'), link: val('n-link').trim()};
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
      <td style="text-align:right;white-space:nowrap"><button type="button" class="small u-save">Save</button> <button type="button" class="small u-pw">Set password</button> <button type="button" class="small danger u-del" ${me && u.username === me.username ? 'disabled' : ''}>Delete</button></td></tr>`).join('')}
    </tbody></table></div>
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
    tr.querySelector('.u-pw').onclick = async () => {
      const p = prompt(`New password for ${name} (at least 8 characters):`); if (!p) return;
      try { await api(`api/users/${enc(name)}`, {method: 'PUT', json: {password: p}}); toast(`Password set for ${name} (signed out everywhere)`); } catch (e) { toast(e.message, true); }
    };
    tr.querySelector('.u-del').onclick = async () => {
      if (!confirm(`Delete the user ${name}?`)) return;
      try { await api(`api/users/${enc(name)}`, {method: 'DELETE'}); renderUsers(body); } catch (e) { toast(e.message, true); }
    };
  });
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
