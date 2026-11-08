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
const FILTER_TYPES = [['recording','Recordings'],['motion','Motion'],['person','Person'],['car','Car'],['truck','Truck'],['bus','Bus'],
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
function camState(c) {
  if (!c.enabled) return {cls: 'disabled', text: 'Disabled'};
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
    for (const img of this.imgs.get(k) || []) img.src = url;
    // The HD image also shows sub frames until its first HD frame arrives.
    if (q === 'sub') for (const img of this.imgs.get(this.key(id, 'main')) || []) if (!this.urls.has(this.key(id, 'main'))) img.src = url;
  },
  fps(id, q) { const s = this.stats.get(this.key(id, q)); return s && performance.now() - s.last < 3000 ? s.fps : 0; },
  // Show a camera in an <img>; returns an unbind function.
  bind(img, id, q) {
    const k = this.key(id, q);
    if (!this.imgs.has(k)) this.imgs.set(k, new Set());
    this.imgs.get(k).add(img);
    const cur = this.urls.get(k) || (q === 'main' && this.urls.get(this.key(id, 'sub')));
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
function updateSubscriptions() {
  if (!status) return;
  const l = status.live, subs = [];
  for (const c of status.cameras) {
    if (!c.enabled) continue;
    subs.push({camera: c.id, q: 'sub', fps: l.fps});
    const focused = page === 'live' && focusId === c.id && !focusPlayback;
    if (focused) subs.push({camera: c.id, q: 'main', fps: l.hd_fps});
    else if (l.preload_hd && c.has_sub) subs.push({camera: c.id, q: 'main', fps: 1});
  }
  Live.setWanted(subs);
}

// Overlays (name, clock, fps, status, fullscreen) for a live surface.
function overlayHtml(c, q) {
  const d = status.display;
  const parts = {tl: [], tr: [], bl: [], br: []};
  if (d.show_name) parts.bl.push(`<span class="ovtext name">${esc(c.name)}</span>`);
  parts.tr.push(`<span class="ov-pill">${pillHtml(c)}</span>`);
  parts.bl.push('<span class="ov-alert"></span>');
  if (d.show_clock) parts[d.clock_corner].push('<span class="ovtext ov-clock"></span>');
  if (d.show_fps) parts.tl.push('<span class="ovtext ov-fps"></span>');
  for (const corner of d.fullscreen_corners) parts[corner].push('<button type="button" class="fsbtn" title="Fullscreen">⛶</button>');
  return Object.entries(parts).filter(([, v]) => v.length).map(([k, v]) => `<div class="ov ${k}">${v.join('')}</div>`).join('');
}
function wireOverlay(surface) {
  surface.querySelectorAll('.fsbtn').forEach(b => b.onclick = (e) => { e.stopPropagation(); fullscreen(surface); });
}
function updateOverlay(surface, c, q) {
  const clock = surface.querySelector('.ov-clock');
  if (clock) clock.textContent = fmtDate(new Date());
  const p = surface.querySelector('.ov-pill');
  if (p) p.innerHTML = pillHtml(c);
  const fps = surface.querySelector('.ov-fps');
  if (fps) {
    const info = c.info && (q === 'main' ? c.info.main : c.info.sub);
    const shown = Live.fps(c.id, q) || (q === 'main' ? Live.fps(c.id, 'sub') : 0);
    fps.textContent = `${shown.toFixed(1)} fps` + (info ? ` · ${info.width}×${info.height} ${String(info.codec).toUpperCase()} @ ${info.fps || '?'} fps` : '');
  }
  const a = surface.querySelector('.ov-alert');
  if (a) {
    const labels = c.detect && c.detect.event_active ? c.detect.active_labels : [];
    a.innerHTML = labels.length ? `<span class="alertbadge">${labels.map(l => esc(l.label)).join(', ')}</span>`
      : (c.detect && c.detect.motion ? '<span class="alertbadge" style="background:rgba(245,158,11,.85)">motion</span>' : '');
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
  document.querySelectorAll('nav a').forEach(a => a.classList.toggle('active', a.dataset.page === page));
  for (const s of PAGES) $('page-' + s).hidden = s !== page;
  if (page !== 'recordings') pauseAllPlayback();
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

// ======================================================================
// Shared recording player: custom scrubber with frame previews, speed,
// clip start/end handles and clip download, auto-advance to next file.
// ======================================================================
class Player {
  constructor(video, controls, msgEl) {
    this.video = video; this.box = controls; this.msg = msgEl;
    this.item = null; this.sprite = null; this.inT = null; this.outT = null; this.marks = [];
    video.controls = false;
    video.addEventListener('timeupdate', () => this.paint());
    video.addEventListener('play', () => this.paintButtons());
    video.addEventListener('pause', () => this.paintButtons());
    video.addEventListener('loadedmetadata', () => { this.paint(); });
    video.addEventListener('ended', () => this.next());
    video.addEventListener('error', () => {
      if (!video.getAttribute('src') || !this.msg) return;
      this.msg.hidden = false;
      this.msg.textContent = 'This browser cannot play this recording (it may be H.265). Use Download and play it in VLC, or turn on “Convert recordings to H.264” for this camera.';
    });
    video.addEventListener('playing', () => { if (this.msg) this.msg.hidden = true; });
    video.addEventListener('click', () => video.paused ? video.play() : video.pause());
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
    q('.p-back').onclick = () => this.video.currentTime = Math.max(0, this.video.currentTime - 10);
    q('.p-fwd').onclick = () => this.video.currentTime = Math.min(this.dur(), this.video.currentTime + 10);
    q('.p-speed').onchange = (e) => this.video.playbackRate = +e.target.value;
    q('.p-in').onclick = () => { this.inT = this.video.currentTime; if (this.outT != null && this.outT <= this.inT) this.outT = null; this.paint(); };
    q('.p-out').onclick = () => { this.outT = this.video.currentTime; if (this.inT == null || this.inT >= this.outT) this.inT = Math.max(0, this.outT - 30); this.paint(); };
    q('.p-mute').onclick = (e) => { this.video.muted = !this.video.muted; e.target.textContent = this.video.muted ? '🔇' : '🔊'; };
    q('.p-fs').onclick = () => fullscreen(this.video.parentElement);
    this.el.clip.onclick = () => this.downloadClip();
    this.dl = q('.p-dl');
    this.wireScrub();
  }
  dur() { const d = this.video.duration; return isFinite(d) && d > 0 ? d : (this.sprite ? this.sprite.info.duration : 0); }
  wallAt(t) { return new Date(new Date(this.item.file_start).getTime() + t * 1000); }
  async load(item, autoplay = true) {
    if (!this.el) this.build();
    this.item = item; this.inT = this.outT = null; this.sprite = null;
    if (this.msg) this.msg.hidden = true;
    this.video.src = item.url;
    this.video.playbackRate = +this.box.querySelector('.p-speed').value;
    const start = item.offset || 0;
    const seek = () => { this.video.currentTime = start; if (autoplay) this.video.play().catch(() => {}); };
    if (this.video.readyState >= 1) seek(); else this.video.addEventListener('loadedmetadata', seek, {once: true});
    this.dl.href = item.url + '?download=1';
    this.marks = item.marks || [];
    this.paint();
    const kind = item.kind === 'clip' ? 'clip' : 'rec';
    try { this.sprite = await api(`api/sprite/${enc(item.camera)}/${kind}/${enc(item.name)}`); } catch (e) { this.sprite = null; }
    if (this.sprite) { const im = new Image(); im.src = this.sprite.url; }
  }
  stop() { this.video.pause(); this.video.removeAttribute('src'); this.video.load(); this.item = null; }
  paintButtons() { if (this.el) this.el.play.textContent = this.video.paused ? '▶' : '❚❚'; }
  paint() {
    if (!this.el || !this.item) return;
    const d = this.dur() || 1, t = this.video.currentTime, pct = (x) => Math.min(100, Math.max(0, x / d * 100)) + '%';
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
    if (s) {
      const i = s.info, n = Math.min(i.count - 1, Math.max(0, Math.floor(t / i.interval)));
      const col = n % i.columns, row = Math.floor(n / i.columns), k = 192 / i.tile_width;
      const rows = Math.ceil(i.count / i.columns);
      this.el.pimg.style.backgroundImage = `url("${s.url}")`;
      this.el.pimg.style.backgroundSize = `${i.columns * i.tile_width * k}px ${rows * i.tile_height * k}px`;
      this.el.pimg.style.backgroundPosition = `-${col * 192}px -${row * 108}px`;
    }
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
        if (performance.now() - lastSeek > 120) { this.video.currentTime = t; lastSeek = performance.now(); }
        this.el.head.style.left = (t / this.dur() * 100) + '%';
      } else if (drag === 'in') { this.inT = Math.min(t, (this.outT ?? this.dur()) - 1); this.paint(); }
      else if (drag === 'out') { this.outT = Math.max(t, (this.inT ?? 0) + 1); this.paint(); }
    });
    sc.addEventListener('pointerleave', () => { if (!drag) this.el.prev.hidden = true; });
    sc.addEventListener('pointerdown', (e) => {
      if (!this.item) return;
      sc.setPointerCapture(e.pointerId);
      drag = e.target.classList.contains('in') ? 'in' : e.target.classList.contains('out') ? 'out' : 'head';
      if (drag === 'head') { this.wasPlaying = !this.video.paused; this.video.pause(); this.video.currentTime = tAt(e)[0]; }
    });
    const end = (e) => {
      if (!drag) return;
      if (drag === 'head') { this.video.currentTime = tAt(e)[0]; if (this.wasPlaying) this.video.play().catch(() => {}); }
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
let focusId = null, focusPlayback = null, editing = false, layout = [], layoutBackup = null, dashIds = '';
const unbinders = new Map();

function defaultLayout(cams) {
  const n = cams.length, cols = n <= 1 ? 1 : n <= 4 ? 2 : n <= 9 ? 3 : 4, w = 12 / cols;
  return cams.map((c, i) => ({camera: c.id, x: (i % cols) * w, y: Math.floor(i / cols) * w, w, h: w}));
}
function currentLayout() {
  const cams = status.cameras;
  let tiles = (status.dashboard.tiles || []).filter(t => cams.some(c => c.id === t.camera)).map(t => ({...t}));
  if (!tiles.length) return defaultLayout(cams);
  let maxY = tiles.reduce((m, t) => Math.max(m, t.y + t.h), 0);
  for (const c of cams) if (!tiles.some(t => t.camera === c.id)) { tiles.push({camera: c.id, x: 0, y: maxY, w: 4, h: 4}); maxY += 4; }
  return tiles;
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
  const W = $('dash').clientWidth || 800;
  const narrow = W < 640;
  const colW = W / 12, rowH = colW * 9 / 16;
  return {W, colW, rowH, narrow};
}

function renderDash() {
  const dash = $('dash');
  if (!status) return;
  const cams = status.cameras;
  $('no-cameras').hidden = cams.length > 0;
  $('dash-edit').hidden = !cams.length || editing;
  dashIds = cams.map(c => c.id + ':' + c.name + ':' + c.enabled).join(',') + JSON.stringify(status.display);
  if (!editing) layout = currentLayout();
  for (const u of unbinders.values()) u();
  unbinders.clear();
  dash.innerHTML = '';
  dash.classList.toggle('editing', editing);
  for (const t of layout) {
    const c = cam(t.camera);
    if (!c) continue;
    const el = document.createElement('div');
    el.className = 'surface tile';
    el.dataset.id = c.id;
    el.innerHTML = `<img alt=""><div class="msg">${c.enabled ? 'Connecting…' : 'Camera disabled'}</div>${overlayHtml(c, 'sub')}${editing ? '<div class="rz"></div>' : ''}`;
    const img = el.querySelector('img');
    img.onload = () => { el.querySelector('.msg').hidden = true; };
    unbinders.set(c.id, Live.bind(img, c.id, 'sub'));
    wireOverlay(el);
    el.onclick = () => { if (!editing) location.hash = '#live/' + enc(c.id); };
    if (editing) wireTileEdit(el, t);
    dash.appendChild(el);
  }
  placeTiles();
  $('dash-info').textContent = cams.length ? `${cams.length} camera${cams.length > 1 ? 's' : ''}` + (editing ? ' — drag to move, drag the corner to resize' : '') : '';
}

function placeTiles() {
  const m = dashMetrics(), dash = $('dash');
  let maxB = 0, y = 0;
  for (const el of dash.querySelectorAll('.tile')) {
    const t = layout.find(x => x.camera === el.dataset.id);
    if (!t) continue;
    if (m.narrow && !editing) {
      const h = (m.W - 8) * 9 / 16;
      Object.assign(el.style, {left: '4px', top: y + 4 + 'px', width: m.W - 8 + 'px', height: h + 'px'});
      y += h + 8; maxB = y;
    } else {
      Object.assign(el.style, {left: t.x * m.colW + 4 + 'px', top: t.y * m.rowH + 4 + 'px', width: t.w * m.colW - 8 + 'px', height: t.h * m.rowH - 8 + 'px'});
      maxB = Math.max(maxB, (t.y + t.h) * m.rowH);
    }
  }
  dash.style.height = maxB + 'px';
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
      if (resize) return {x: t.x, y: t.y, w: Math.max(2, Math.min(12 - t.x, Math.round(start.tw + dx))), h: Math.max(2, Math.round(start.th + dy))};
      return {x: Math.max(0, Math.min(12 - t.w, Math.round(start.tx + dx))), y: Math.max(0, Math.round(start.ty + dy)), w: t.w, h: t.h};
    };
    const move = (ev) => {
      const g = calc(ev);
      Object.assign(ghost.style, {left: g.x * m.colW + 'px', top: g.y * m.rowH + 'px', width: g.w * m.colW + 'px', height: g.h * m.rowH + 'px'});
      if (resize) { el.style.width = (start.tw * m.colW - 8 + ev.clientX - start.x) + 'px'; el.style.height = (start.th * m.rowH - 8 + ev.clientY - start.y) + 'px'; }
      else { el.style.left = start.tx * m.colW + 4 + ev.clientX - start.x + 'px'; el.style.top = start.ty * m.rowH + 4 + ev.clientY - start.y + 'px'; }
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

$('dash-edit').onclick = () => { editing = true; layoutBackup = JSON.stringify(layout); toggleEditButtons(); renderDash(); };
$('dash-cancel').onclick = () => { editing = false; toggleEditButtons(); renderDash(); };
$('dash-auto').onclick = () => { layout = defaultLayout(status.cameras); renderDash(); };
$('dash-save').onclick = async () => {
  try {
    await api('api/dashboard', {method: 'PUT', json: {tiles: layout}});
    status.dashboard.tiles = layout.map(t => ({...t}));
    editing = false; toggleEditButtons(); renderDash(); toast('Layout saved');
  } catch (e) { toast(e.message, true); }
};
function toggleEditButtons() {
  $('dash-edit').hidden = editing;
  for (const id of ['dash-auto', 'dash-cancel', 'dash-save']) $(id).hidden = !editing;
}

// Keep the dashboard current (it runs in the background on every page).
function dashSync() {
  if (!status) return;
  const ids = status.cameras.map(c => c.id + ':' + c.name + ':' + c.enabled).join(',') + JSON.stringify(status.display);
  if (ids !== dashIds && !editing) renderDash();
  for (const el of document.querySelectorAll('#dash .tile')) {
    const c = cam(el.dataset.id);
    if (c) updateOverlay(el, c, 'sub');
  }
}
setInterval(() => {
  // Clocks on overlays tick every second.
  document.querySelectorAll('.ov-clock').forEach(e => e.textContent = fmtDate(new Date()));
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
  surf.querySelectorAll('.ov').forEach(o => o.remove());
  surf.insertAdjacentHTML('beforeend', overlayHtml(c, 'main'));
  wireOverlay(surf);
  if (focusUnbind) focusUnbind();
  const img = $('focus-img');
  img.removeAttribute('src');
  $('focus-msg').hidden = false; $('focus-msg').textContent = 'Connecting…';
  img.onload = () => { $('focus-msg').hidden = true; };
  focusUnbind = Live.bind(img, c.id, 'main');
  if (img.getAttribute('src')) $('focus-msg').hidden = true;
  $('focus-snap').href = 'snapshot/' + enc(c.id);
  $('focus-recs').href = '#recordings/' + enc(c.id);
  $('focus-edit').onclick = () => openCameraDialog(c.id);
  updateFocus();
  loadActivity();
}
function stopFocus() { if (focusUnbind) { focusUnbind(); focusUnbind = null; } stopFocusPlayback(false); }

function updateFocus() {
  const c = cam(focusId);
  if (!c) { location.hash = '#live'; return; }
  updateOverlay($('focus-surface'), c, 'main');
  const r = c.recorder || c.buffer;
  const btn = $('focus-rec');
  btn.hidden = !r || !c.enabled;
  if (r) {
    btn.textContent = r.paused ? 'Resume recording' : 'Pause recording';
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
    <dt>Live view</dt><dd>${Live.fps(c.id, 'main').toFixed(1)} fps</dd>
    <dt>Detection</dt><dd>${!c.detection ? 'Off' : d ? (d.running ? `${d.motion ? 'Motion now' : 'Watching'}${c.ai ? ' + AI' : ''}` : 'Starting…') : 'Starting…'}</dd>
    ${d && d.last_event ? `<dt>Last alert</dt><dd>${fmtDate(d.last_event)}</dd>` : ''}
    <dt>Reconnects</dt><dd>${r ? r.restarts : 0}</dd>`;
  const errs = [];
  if (r && r.last_error && r.state !== 'recording') errs.push(r.last_error);
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
  const types = f === 'events' ? 'motion,' + FILTER_TYPES.map(t => t[0]).filter(t => t !== 'recording').join(',') : f;
  const from = new Date(Date.now() - 7 * 86400000).toISOString();
  let items = [];
  try { items = await api(`api/recordings?cameras=${enc(id)}&from=${enc(from)}&limit=80${types ? '&types=' + enc(types) : ''}`); } catch (e) {}
  if (focusId !== id) return;
  box.innerHTML = items.length ? '' : '<div class="empty" style="padding:20px">Nothing recorded yet.</div>';
  for (const it of items) {
    const row = document.createElement('div');
    row.className = 'act';
    const dur = (new Date(it.end) - new Date(it.start)) / 1000;
    row.innerHTML = `<img loading="lazy" src="${esc(it.thumb || '')}" alt="" onerror="this.style.visibility='hidden'">
      <div class="meta"><div>${it.kind === 'event' ? labelTags(it.event.labels, it.event.motion) : '<span class="tag">recording</span>'}</div>
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
  const v = $('focus-video'); v.hidden = false;
  if (!focusPlayer) focusPlayer = new Player(v, $('focus-player-controls'), $('focus-msg'));
  $('focus-player-controls').hidden = false;
  focusPlayer.load({...it.play, camera: it.camera, marks: it.event ? [it.event.start] : []});
  $('focus-live').hidden = false;
}
function stopFocusPlayback(resub = true) {
  if (!focusPlayback) return;
  focusPlayback = null;
  if (focusPlayer) focusPlayer.stop();
  $('focus-video').hidden = true; $('focus-img').hidden = false;
  $('focus-player-controls').hidden = true;
  $('focus-surface').querySelectorAll('.ov').forEach(o => o.hidden = false);
  $('focus-live').hidden = true;
  $('focus-activity').querySelectorAll('.act').forEach(a => a.classList.remove('active'));
  if (resub) updateSubscriptions();
}
$('focus-live').onclick = () => stopFocusPlayback();
$('focus-back').onclick = () => location.hash = '#live';

// ======================================================================
// Recordings
// ======================================================================
const startOfDay = (d) => { const x = new Date(d); x.setHours(0, 0, 0, 0); return x; };
const endOfDay = (d) => { const x = new Date(d); x.setHours(23, 59, 59, 999); return x; };
const rec = {mode: 'list', cameras: new Set(), from: startOfDay(new Date()), to: endOfDay(new Date()), todFrom: '', todTo: '', types: new Set(), items: []};
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
$('f-refresh').onclick = () => rec.mode === 'list' ? loadRecordings() : loadTimeline();

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
    (rec.types.size ? '<button class="small" id="f-types-clear">Show everything</button>' : '');
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
  if (rec.types.size) q.set('types', [...rec.types].join(','));
  if (rec.todFrom) q.set('tod_from', rec.todFrom);
  if (rec.todTo) q.set('tod_to', rec.todTo);
  $('rec-count').textContent = 'Loading…';
  try { rec.items = await api('api/recordings?' + q); } catch (e) { rec.items = []; toast(e.message, true); }
  renderRecordings();
}

function renderRecordings() {
  const box = $('rec-cards'), items = rec.items;
  const ev = items.filter(i => i.kind === 'event').length;
  $('rec-count').textContent = items.length ? `${items.length} items · ${ev} alerts · ${items.length - ev} recordings` : '';
  if (!items.length) { box.innerHTML = '<div class="empty" style="grid-column:1/-1">Nothing found for these filters.</div>'; return; }
  box.innerHTML = '';
  let day = '';
  for (const it of items) {
    const d = fmtDay(it.start);
    if (d !== day) { day = d; box.insertAdjacentHTML('beforeend', `<div class="dayhead">${esc(d)}</div>`); }
    const card = document.createElement('div');
    card.className = 'rc';
    const dur = (new Date(it.end) - new Date(it.start)) / 1000;
    const tags = it.kind === 'event' ? labelTags(it.event.labels, it.event.motion) : '<span class="tag">recording</span>';
    card.innerHTML = `<div class="thumb">${it.thumb ? `<img loading="lazy" src="${esc(it.thumb)}" alt="" onerror="this.remove()">` : ''}<span class="dur">${fmtDur(dur)}</span>
        <span class="kind">${it.kind === 'event' && it.event.clip ? '<span class="tag" style="background:#000a">HD clip</span>' : ''}</span></div>
      <div class="body"><div>${tags}</div><div><b>${esc(it.camera_name)}</b> · ${fmtTime(it.start)}</div>
      <div class="muted">${it.size ? fmtBytes(it.size) : ''}</div></div>
      <div class="acts"><button type="button" class="small primary" data-a="play" ${it.play ? '' : 'disabled'} title="${it.pending ? 'Clip still recording' : ''}">${it.pending ? '⏳ Saving' : '▶ Play'}</button>
        ${it.download ? `<a class="btn small" href="${esc(it.download)}" title="Download">⇩</a>` : ''}
        <span class="spacer"></span><button type="button" class="small" data-a="del" title="Delete">✕</button></div>`;
    card.querySelector('[data-a=play]').onclick = (e) => { e.stopPropagation(); playItem(it, card); };
    card.querySelector('.thumb').onclick = () => playItem(it, card);
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
    ${it.kind === 'event' && it.thumb ? `<img src="${esc(it.thumb)}" style="width:100%;border-radius:6px;margin-bottom:8px" alt="">` : ''}
    <dl><dt>Camera</dt><dd>${esc(it.camera_name)}</dd><dt>Time</dt><dd>${fmtDate(it.start)}</dd><dt>Length</dt><dd>${fmtDur(dur)}</dd>
    ${it.kind === 'event' ? `<dt>Detected</dt><dd>${labelTags(it.event.labels, it.event.motion) || '–'}</dd><dt>Video</dt><dd>${it.event.clip ? 'Full-quality clip' : 'From continuous recording'}</dd>` : ''}
    ${it.size ? `<dt>Size</dt><dd>${fmtBytes(it.size)}</dd>` : ''}</dl>
    <div class="hint" style="margin-top:10px">Hover over the bar under the video to preview any moment. Use <b>[ Start</b> and <b>End ]</b> (or drag the orange handles) to cut out a clip.</div>
    <div class="actions"><a class="btn small" href="#recordings/${enc(it.camera)}/timeline" onclick="tl.jump='${it.start}'">Open in timeline</a></div>`;
  $('rec-player').scrollIntoView({behavior: 'smooth', block: 'start'});
}

// ---------- multi-camera timeline ----------
const tl = {data: [], T: null, playing: false, speed: 1, zoom: 3600, winStart: null, inT: null, outT: null, sprites: new Map(), last: 0, jump: null, day: null};

async function loadTimeline() {
  const q = new URLSearchParams({from: rec.from.toISOString(), to: rec.to.toISOString()});
  if (rec.cameras.size) q.set('cameras', [...rec.cameras].join(','));
  try { tl.data = await api('api/timeline?' + q); } catch (e) { tl.data = []; toast(e.message, true); }
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
}
function centerWindow() { tl.winStart = tl.T - tl.zoom / 2 * 1000; }

function buildTimelineVideos() {
  const box = $('tl-videos');
  box.innerHTML = tl.data.length ? '' : '<div class="empty">No cameras selected.</div>';
  for (const c of tl.data) {
    const el = document.createElement('div');
    el.className = 'surface';
    el.dataset.cam = c.camera;
    el.innerHTML = `<video playsinline muted></video><div class="sprite" hidden></div><div class="msg" hidden>No recording</div>
      <div class="ov tl"><span class="ovtext name">${esc(c.name)}</span><span class="ovtext tl-t"></span></div>
      <div class="ov tr"><button type="button" class="fsbtn">⛶</button></div>`;
    el.querySelector('.fsbtn').onclick = () => fullscreen(el);
    box.appendChild(el);
  }
}

function segAt(c, T) {
  return c.segments.find(s => new Date(s.start).getTime() <= T && T < new Date(s.end).getTime() + 1000 && s.kind === 'recording')
    || c.segments.find(s => new Date(s.start).getTime() <= T && T < new Date(s.end).getTime() + 1000);
}

// Keep each camera's video at the shared time `tl.T`.
function tlSync(force) {
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const c = tl.data.find(x => x.camera === el.dataset.cam);
    const v = el.querySelector('video'), msg = el.querySelector('.msg');
    el.querySelector('.tl-t').textContent = fmtDate(new Date(tl.T));
    const s = c && segAt(c, tl.T);
    if (!s) { msg.hidden = false; v.pause(); v.style.visibility = 'hidden'; continue; }
    msg.hidden = true; v.style.visibility = '';
    const want = (tl.T - new Date(s.start).getTime()) / 1000;
    if (v.dataset.url !== s.url) {
      v.dataset.url = s.url; v.src = s.url;
      v.addEventListener('loadedmetadata', () => { v.currentTime = want; v.playbackRate = tl.speed; if (tl.playing) v.play().catch(() => {}); }, {once: true});
      continue;
    }
    if (force || Math.abs(v.currentTime - want) > Math.max(0.8, tl.speed * 0.5)) v.currentTime = want;
    v.playbackRate = tl.speed;
    if (tl.playing && v.paused) v.play().catch(() => {});
    if (!tl.playing && !v.paused) v.pause();
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
$('tl-zoom').onchange = (e) => { tl.zoom = +e.target.value; centerWindow(); drawTimeline(); };
$('tl-left').onclick = () => { tl.winStart -= tl.zoom * 500; drawTimeline(); };
$('tl-right').onclick = () => { tl.winStart += tl.zoom * 500; drawTimeline(); };
$('tl-back10').onclick = () => { tl.T -= 10000; tlSync(true); };
$('tl-fwd10').onclick = () => { tl.T += 10000; tlSync(true); };
function allEvents() { return tl.data.flatMap(c => c.events).map(e => new Date(e.start).getTime()).sort((a, b) => a - b); }
$('tl-prev-ev').onclick = () => { const e = allEvents().filter(t => t < tl.T - 4000).pop(); if (e) jumpTo(e - 3000); };
$('tl-next-ev').onclick = () => { const e = allEvents().find(t => t > tl.T + 4000); if (e) jumpTo(e - 3000); };
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
      lane += `<div class="ev ${e.labels.length ? '' : 'motion'}" style="left:${Math.max(0, a)}%;width:${Math.max(0.3, b - a)}%" title="${esc(tip)} ${fmtTime(e.start)}" data-t="${new Date(e.start).getTime()}"></div>`;
    }
    rows += `<div class="tl-row"><div class="label" title="${esc(c.name)}">${esc(c.name)}</div><div class="lane">${lane}</div></div>`;
  }
  const sel = tl.inT != null && tl.outT != null ? `<div class="tl-sel" style="left:calc(120px + (100% - 120px) * ${pct(tl.inT) / 100});width:calc((100% - 120px) * ${(pct(tl.outT) - pct(tl.inT)) / 100})"></div>` : '';
  box.innerHTML = `<div class="tl-axis">${axis}</div>${rows}${sel}<div class="tl-head" id="tl-head"></div>`;
  box.querySelectorAll('.ev').forEach(e => e.onclick = (ev) => { ev.stopPropagation(); jumpTo(+e.dataset.t - 3000); });
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
    const t = tAt(e); if (t == null || e.clientX < box.querySelector('.lane').getBoundingClientRect().left) return;
    drag = true; was = tl.playing; if (was) tlPlay(false);
    box.setPointerCapture(e.pointerId);
    tl.T = t; drawHead(); previewAll();
  });
  box.addEventListener('pointermove', (e) => { if (!drag) return; tl.T = tAt(e); drawHead(); previewAll(); });
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
    tl.sprites.set(key, api(`api/sprite/${enc(cam)}/${seg.kind === 'clip' ? 'clip' : 'rec'}/${enc(seg.name)}`).then(s => { new Image().src = s.url; return s; }).catch(() => null));
  }
  return tl.sprites.get(key);
}
function previewAll() {
  for (const el of document.querySelectorAll('#tl-videos .surface')) {
    const c = tl.data.find(x => x.camera === el.dataset.cam);
    el.querySelector('.tl-t').textContent = fmtDate(new Date(tl.T));
    const s = c && segAt(c, tl.T), sp = el.querySelector('.sprite');
    el.querySelector('.msg').hidden = !!s;
    if (!s) { sp.hidden = true; continue; }
    const T = tl.T;
    spriteFor(c.camera, s).then(info => {
      if (!info || tl.T !== T) return;
      const i = info.info, off = (T - new Date(s.start).getTime()) / 1000;
      const n = Math.min(i.count - 1, Math.max(0, Math.floor(off / i.interval)));
      const W = el.clientWidth, H = el.clientHeight, k = Math.min(W / i.tile_width, H / i.tile_height);
      const rows = Math.ceil(i.count / i.columns);
      sp.hidden = false;
      sp.style.backgroundImage = `url("${info.url}")`;
      sp.style.backgroundSize = `${i.columns * i.tile_width * k}px ${rows * i.tile_height * k}px`;
      const ox = (W - i.tile_width * k) / 2, oy = (H - i.tile_height * k) / 2;
      sp.style.backgroundPosition = `${ox - (n % i.columns) * i.tile_width * k}px ${oy - Math.floor(n / i.columns) * i.tile_height * k}px`;
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
  ['ram', 'Memory', '#f59e0b'],
];

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
    card('Memory', s.ram.toFixed(0) + '%', `${fmtBytes(c.ram_used)} of ${fmtBytes(c.ram_total)}`, s.ram) +
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
  g += `<path d="${h.map((p, i) => (i ? 'L' : 'M') + X(p.t) + ',' + Y(p.cpu_max)).join('')}L${X(t1)},${Y(0)}L${X(t0)},${Y(0)}Z" fill="rgba(79,140,255,.12)"/>`;
  for (const [k, , color] of SERIES) {
    const pts = h.filter(p => p[k] != null);
    if (pts.length < 2) continue;
    g += `<path d="${pts.map((p, i) => (i ? 'L' : 'M') + X(p.t).toFixed(1) + ',' + Y(p[k]).toFixed(1)).join('')}" fill="none" stroke="${color}" stroke-width="1.6"/>`;
  }
  g += '<line id="g-cursor" y1="0" y2="' + (H - 18) + '" stroke="#fff" stroke-opacity=".4" hidden/>';
  svg.innerHTML = g;
  $('sys-legend').innerHTML = SERIES.filter(([k]) => h.some(p => p[k] != null)).map(([, n, c]) => `<span><i style="background:${c}"></i>${n}</span>`).join('') + '<span><i style="background:rgba(79,140,255,.3)"></i>CPU peaks</span>';
  svg.onmousemove = (e) => {
    const r = svg.getBoundingClientRect(), x = (e.clientX - r.left) / r.width * W;
    const t = t0 + (x - 36) / (W - 40) * (t1 - t0);
    const p = h.reduce((a, b) => Math.abs(b.t - t) < Math.abs(a.t - t) ? b : a);
    const cur = svg.querySelector('#g-cursor'); cur.removeAttribute('hidden'); cur.setAttribute('x1', X(p.t)); cur.setAttribute('x2', X(p.t));
    $('sys-hover').textContent = `${fmtDate(new Date(p.t * 1000))} — CPU ${p.cpu.toFixed(0)}% (peak ${p.cpu_max.toFixed(0)}%), NVR ${p.nvr_cpu.toFixed(1)}%, memory ${p.ram.toFixed(0)}%` + (p.gpu != null ? `, GPU ${p.gpu.toFixed(0)}%` : '');
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
  const r = c.recorder || c.buffer, d = c.detect, out = [];
  if (r && r.last_error && r.state !== 'recording') out.push(r.last_error);
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
    detect: {motion: true, sensitivity: 50, ai: false, objects: ['person','car','truck','dog','cat'], min_confidence: 0.5, trigger: 'motion', pre_seconds: 30, post_seconds: 15, fps: 5, ai_interval: 1}};
  if (clone) c.name = c.name + ' (copy)';
  const q = parseUrl(c.url) || {};
  const objs = (sysinfo && sysinfo.ai_objects) || ['person','car','truck','bus','motorcycle','bicycle','dog','cat','bird'];
  const aiOn = settings && settings.ai.provider !== 'none';
  const brandOpts = '<option value="">Format…</option>' + BRANDS.map(b => `<option value="${b[0]}">${esc(b[1])}</option>`).join('');
  const segOpts = [[0, 'Default (' + fmtDur(settings ? settings.storage.segment_seconds : 300) + ')'], [300, '5 minutes'], [900, '15 minutes'], [1800, '30 minutes'], [3600, '1 hour'], [7200, '2 hours'], [21600, '6 hours'], [86400, 'One long file per day']];
  $('f-cam').innerHTML = `
    <div class="head"><h2>${clone ? 'Clone camera' : id ? 'Edit camera' : 'Add camera'}</h2><button type="button" class="small" data-close>✕</button></div>
    <div class="body">
      <div class="tabs" id="c-tabs"><button type="button" data-t="general" class="active">General</button><button type="button" data-t="recording">Recording</button><button type="button" data-t="detection">Motion &amp; AI</button></div>
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
        <div><input type="text" id="c-dir" value="${esc(c.storage_dir)}" placeholder="Default: ${esc(settings ? settings.storage.dir : 'recordings')}">
          <div class="hint">Any folder this computer can write to: another drive (<code>D:\\NVR</code>), a NAS (<code>\\\\NAS\\cameras</code> or <code>/mnt/nas</code>), or a synced cloud folder (<code>G:\\My Drive\\NVR</code> with Google Drive for desktop). A sub-folder named after the camera is created.</div></div>
        <label for="c-ret">Keep recordings for</label>
        <div class="row"><input type="number" id="c-ret" min="0" value="${c.retention_days}" style="width:110px"><span class="muted">days (0 = use the default, ${settings ? settings.storage.retention_days : 14})</span></div>
        <label for="c-max">Space for this camera</label>
        <div class="row"><input type="number" id="c-max" min="0" step="0.5" value="${c.max_storage_gb}" style="width:110px"><span class="muted">GB (0 = no limit; oldest recordings are deleted above it)</span></div>
      </div>

      <div data-tab="detection" class="form" hidden>
        <span></span><label class="check"><input type="checkbox" id="d-motion" ${c.detect.motion ? 'checked' : ''}> Motion detection</label>
        <label for="d-sens">Motion sensitivity</label>
        <div class="row"><input type="range" id="d-sens" min="1" max="100" value="${c.detect.sensitivity}" style="flex:1"><span id="d-sens-v" class="muted" style="width:40px">${c.detect.sensitivity}</span></div>
        <span></span><div><label class="check"><input type="checkbox" id="d-ai" ${c.detect.ai ? 'checked' : ''}> Object recognition (AI)</label>
          <div class="hint">${aiOn ? `Uses <b>${esc(settings.ai.provider)}</b>. Pictures are checked when motion is seen.` : '<span class="warnc">No AI is set up yet.</span> Choose one in Settings → Object recognition.'}</div></div>
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
  });
  const syncSched = () => f.querySelectorAll('.sched').forEach(e => e.hidden = !chk('c-sched-on'));
  $('c-sched-on').onchange = syncSched; syncSched();
  $('d-sens').oninput = () => $('d-sens-v').textContent = val('d-sens');
  $('d-conf').oninput = () => $('d-conf-v').textContent = val('d-conf') + '%';
  $('q-brand').onchange = () => {
    const b = val('q-brand'); if (!b) return;
    const brand = BRANDS.find(x => x[0] === b);
    if (!val('q-port')) $('q-port').placeholder = 'port (' + brand[2] + ')';
    $('c-url').value = brandUrl(b, 'main'); $('c-live').value = brandUrl(b, 'sub');
  };
  for (const k of ['q-ip', 'q-port', 'q-ch', 'q-user', 'q-pass']) $(k).oninput = () => { if (val('q-brand')) $('q-brand').onchange(); };
  $('c-url-fmt').onchange = (e) => { if (e.target.value) $('c-url').value = brandUrl(e.target.value, 'main'); e.target.value = ''; };
  $('c-live-fmt').onchange = (e) => { if (e.target.value) $('c-live').value = brandUrl(e.target.value, 'sub'); e.target.value = ''; };
  $('c-test').onclick = testCamera;
  f.onsubmit = saveCamera;
  $('cam-dialog').showModal();
}
function closeCameraDialog() { stopPreviews(); $('cam-dialog').close(); }
$('cam-dialog').addEventListener('close', stopPreviews);

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
      if (!r.ok) { msg.innerHTML = `<span class="err">✗ ${esc(r.error || 'Could not connect')}</span>`; info.innerHTML = `<b>${label}</b>`; return; }
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
  const body = {
    name: val('c-name').trim(), enabled: chk('c-enabled'), url: val('c-url').trim(), live_url: val('c-live').trim(),
    rtsp_transport: val('c-transport'), record_mode: mode, schedule_enabled: chk('c-sched-on'),
    schedule: {days, start: val('c-start') || '00:00', end: val('c-end') || '00:00'},
    record_audio: chk('c-audio'), transcode: chk('c-transcode'), record_fps: num('c-fps'), segment_seconds: num('c-seg'),
    storage_dir: val('c-dir').trim(), retention_days: Math.round(num('c-ret')), max_storage_gb: num('c-max'),
    detect: {motion: chk('d-motion'), sensitivity: num('d-sens'), ai: chk('d-ai'), objects, min_confidence: num('d-conf') / 100,
      trigger: val('d-trig'), pre_seconds: Math.round(num('d-pre')), post_seconds: Math.round(num('d-post')), fps: Math.round(num('d-fps')), ai_interval: num('d-int')},
  };
  const err = (m) => { $('c-err').hidden = false; $('c-err').textContent = m; };
  if (body.schedule_enabled && !days.length) return err('Choose at least one day for the schedule.');
  if (body.detect.ai && !objects.length) return err('Choose at least one object to alert on.');
  const btn = $('c-save'); btn.disabled = true; btn.textContent = 'Saving…';
  try {
    if (editingId) await api(`api/cameras/${enc(editingId)}`, {method: 'PUT', json: body});
    else await api('api/cameras', {method: 'POST', json: body});
    closeCameraDialog();
    toast(editingId ? 'Camera updated' : cloneMode ? 'Camera cloned' : 'Camera added');
    await loadSettings(); await refreshStatus();
    if (page === 'settings') showSettings(section); else route();
  } catch (e) { err(e.message); }
  finally { btn.disabled = false; btn.textContent = 'Save camera'; }
}

// ---------- storage ----------
async function renderStorage(body) {
  const s = settings.storage;
  const segs = [[300, '5 minutes'], [600, '10 minutes'], [900, '15 minutes'], [1800, '30 minutes'], [3600, '1 hour'], [7200, '2 hours'], [21600, '6 hours'], [43200, '12 hours'], [86400, 'One long file per day']];
  if (!segs.some(x => x[0] === s.segment_seconds)) segs.unshift([s.segment_seconds, fmtDur(s.segment_seconds)]);
  body.innerHTML = `<form class="card" id="f-storage"><h2>Recording &amp; storage</h2><div class="form">
    <label for="s-dir">Default recordings folder</label><div><input type="text" id="s-dir" required value="${esc(s.dir)}">
      <div class="hint">Cameras record into sub-folders here, unless a camera has its own storage folder (camera settings → Recording). Existing recordings are not moved.</div></div>
    <label for="s-seg">Default file length</label><div><select id="s-seg">${segs.map(([v, t]) => `<option value="${v}" ${v === s.segment_seconds ? 'selected' : ''}>${t}</option>`).join('')}</select>
      <div class="hint">Recordings are split into files of this length. Cameras can override it.</div></div>
    <label for="s-ret">Keep recordings for</label><div class="row"><input type="number" id="s-ret" min="0" value="${s.retention_days}" style="width:110px"><span class="muted">days (0 = forever)</span></div>
    <label for="s-max">Maximum storage</label><div class="row"><input type="number" id="s-max" min="0" step="0.5" value="${s.max_storage_gb}" style="width:110px"><span class="muted">GB in total (0 = use the whole disk)</span></div>
    <label for="s-free">Keep free on disk</label><div class="row"><input type="number" id="s-free" min="0" step="0.5" value="${s.min_free_gb}" style="width:110px"><span class="muted">GB (oldest recordings are deleted to keep this free)</span></div>
  </div><div class="actions"><button class="primary">Save</button><a class="btn" href="#system">See space &amp; days of recording →</a></div></form>`;
  $('f-storage').onsubmit = (e) => { e.preventDefault(); saveSettings({storage: {dir: val('s-dir').trim(), segment_seconds: num('s-seg'), retention_days: Math.round(num('s-ret')), max_storage_gb: num('s-max'), min_free_gb: num('s-free')}}); };
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
    <span></span><div><label class="check"><input type="checkbox" id="l-pre" ${l.preload_hd ? 'checked' : ''}> Keep HD streams ready in the background</label>
      <div class="hint">Opening a camera is then instant (its HD picture is already loaded). Uses more CPU/GPU; turn off on slow computers.</div></div>
    <h3>Overlays on live video</h3>
    <label>Fullscreen button</label><div class="chips" id="o-fs">${corners.map(([k, t]) => `<label><input type="checkbox" value="${k}" ${d.fullscreen_corners.includes(k) ? 'checked' : ''}>${t}</label>`).join('')}</div>
    <span></span><label class="check"><input type="checkbox" id="o-clock" ${d.show_clock ? 'checked' : ''}> Show date and time</label>
    <label for="o-corner">Date &amp; time position</label><div><select id="o-corner">${corners.map(([k, t]) => `<option value="${k}" ${k === d.clock_corner ? 'selected' : ''}>${t}</option>`).join('')}</select></div>
    <label for="o-fmt">Date &amp; time format</label><div><select id="o-fmt">${CLOCK_FORMATS.map(f => `<option ${f === d.clock_format ? 'selected' : ''}>${f}</option>`).join('')}<option value="custom" ${custom ? 'selected' : ''}>Custom…</option></select>
      <input type="text" id="o-custom" value="${esc(d.clock_format)}" style="margin-top:6px" ${custom ? '' : 'hidden'}>
      <div class="hint">Preview: <b id="o-prev"></b><br>Codes: YYYY YY · MMMM MMM MM M · DD D · dddd ddd · HH H (24 h) hh h (12 h) · mm ss · A a (AM/PM)</div></div>
    <span></span><label class="check"><input type="checkbox" id="o-name" ${d.show_name ? 'checked' : ''}> Show camera name</label>
    <span></span><label class="check"><input type="checkbox" id="o-fps" ${d.show_fps ? 'checked' : ''}> Show frames per second and stream details</label>
  </div><div class="actions"><button class="primary">Save</button></div></form>`;
  const fmt = () => val('o-fmt') === 'custom' ? val('o-custom') : val('o-fmt');
  const prev = () => $('o-prev').textContent = fmtDate(new Date(), fmt());
  $('o-fmt').onchange = () => { $('o-custom').hidden = val('o-fmt') !== 'custom'; prev(); };
  $('o-custom').oninput = prev; prev();
  $('f-display').onsubmit = (e) => {
    e.preventDefault();
    saveSettings({
      live: {fps: num('l-fps'), width: num('l-width'), quality: num('l-q'), hd_fps: num('l-hd'), preload_hd: chk('l-pre')},
      display: {fullscreen_corners: [...document.querySelectorAll('#o-fs input:checked')].map(i => i.value), show_clock: chk('o-clock'),
        clock_corner: val('o-corner'), clock_format: fmt() || 'DD/MM/YYYY HH:mm:ss', show_name: chk('o-name'), show_fps: chk('o-fps')},
    }, 'Saved').then(() => renderDash());
  };
}

// ---------- AI ----------
const PROVIDERS = [
  ['none', 'Off', 'No object recognition (motion detection still works).'],
  ['builtin', 'Built-in (this computer)', 'Free and private. Runs a YOLOX model on this computer\'s CPU. The model (4–36 MB) downloads automatically the first time.'],
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
        <option value="yolox_s" ${a.builtin_model === 'yolox_s' ? 'selected' : ''}>Small — most accurate (≈300 ms)</option></select></div>
      <label class="p-url" for="ai-url">Server address</label><div class="p-url"><input type="text" id="ai-url" value="${esc(a.url)}"><div class="hint" id="ai-url-hint"></div></div>
      <label class="p-key" for="ai-key">API key</label><div class="p-key"><input type="password" id="ai-key" autocomplete="new-password" placeholder="${a.has_key ? 'Saved – leave empty to keep' : ''}"><div class="hint" id="ai-key-hint"></div></div>
      <label class="p-model" for="ai-mname">Model name</label><div class="p-model"><input type="text" id="ai-mname" value="${esc(a.model)}"><div class="hint" id="ai-model-hint"></div></div>
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
    gemini: ['', '', 'gemini-2.5-flash', 'Leave empty for gemini-2.5-flash.'],
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
    api_key: val('ai-key'), model: val('ai-mname').trim(), max_per_minute: Math.round(num('ai-rate')), timeout_seconds: Math.round(num('ai-timeout')) || 20});
  const showStatus = () => {
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
      <label for="h-dev" class="vaapi">GPU device</label><div class="vaapi"><input type="text" id="h-dev" value="${esc(h.device)}" placeholder="/dev/dri/renderD128"><div class="hint">VAAPI only. Empty = default.</div></div>
    </div>
    <div class="actions"><button class="primary">Save</button><button type="button" id="h-test">Test selected options</button></div>
    <div id="h-result" style="margin-top:12px"></div></form>`;
  $('h-dec').value = h.hwaccel; $('h-enc').value = h.encoder;
  const sync = () => document.querySelectorAll('.vaapi').forEach(e => e.hidden = !(val('h-dec') === 'vaapi' || val('h-enc') === 'h264_vaapi'));
  $('h-dec').onchange = $('h-enc').onchange = sync; sync();
  const form = () => ({hwaccel: val('h-dec'), encoder: val('h-enc'), device: val('h-dev').trim()});
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
      <label for="cl-what">What to upload</label><div><select id="cl-what"><option value="events" ${c.upload === 'events' ? 'selected' : ''}>Alerts only (clips and pictures)</option><option value="all" ${c.upload === 'all' ? 'selected' : ''}>Everything (needs a fast upload)</option></select></div>
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
function renderSystemSettings(body) {
  const s = settings.server;
  body.innerHTML = `<form class="card" id="f-sys"><h2>System &amp; login</h2><div class="form">
    <label for="y-name">NVR name</label><div><input type="text" id="y-name" required value="${esc(s.name)}"></div>
    <label for="y-listen">Web address and port</label><div><input type="text" id="y-listen" required value="${esc(s.listen)}"><div class="hint">0.0.0.0 = reachable from other devices on your network. Takes effect after a restart.</div></div>
    <label for="y-user">Login username</label><div><input type="text" id="y-user" value="${esc(s.username)}" autocomplete="off"><div class="hint">Leave empty to turn off the login.</div></div>
    <label for="y-pass">Login password</label><div><input type="password" id="y-pass" autocomplete="new-password"><div class="hint">${s.has_password ? 'A password is set. Leave empty to keep it.' : 'Required when a username is set.'}</div></div>
    <label for="y-ffmpeg">ffmpeg program</label><div><input type="text" id="y-ffmpeg" required value="${esc(s.ffmpeg_path)}"><div class="hint">Name or full path of ffmpeg (ffprobe must be next to it).</div></div>
  </div><div class="actions"><button class="primary">Save</button></div></form>`;
  $('f-sys').onsubmit = async (e) => {
    e.preventDefault();
    const user = val('y-user').trim();
    if (user && !val('y-pass') && !settings.server.has_password) return toast('Enter a password for the login.', true);
    if (await saveSettings({server: {name: val('y-name'), listen: val('y-listen'), username: user, password: val('y-pass'), ffmpeg_path: val('y-ffmpeg')}})) { sysinfo = null; renderSystemSettings(body); }
  };
}

async function renderInfo(body) {
  sysinfo = await api('api/system').catch(() => null);
  if (!sysinfo) return;
  const c = sysinfo.capabilities;
  body.innerHTML = `<div class="card"><h2>About</h2><dl>
    <dt>NVR version</dt><dd>${esc(sysinfo.version)}</dd><dt>Operating system</dt><dd>${esc(sysinfo.os)} (${esc(sysinfo.arch)})</dd>
    <dt>Uptime</dt><dd>${fmtDur(sysinfo.uptime_seconds)}</dd><dt>Web address</dt><dd>${esc(sysinfo.listen)}</dd>
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
