// GAZE as an installed app. Everything comes fresh from GAZE; when the
// computer running it can't be reached, the app shows a friendly message
// instead of the browser's error page.
const SHELL = 'gaze-shell-v4';
// "Try again" shows that it's trying (a spinning ring, "Connecting…"), and
// the page keeps trying by itself every few seconds: GAZE opens as soon as
// it answers.
const OFFLINE = `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>GAZE</title>
<style>
body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#0f1218;color:#e6e9f0;font:16px system-ui,sans-serif;text-align:center}
.box{padding:24px;max-width:420px} p{color:#9aa3b5}
button{font:inherit;padding:10px 22px;border-radius:8px;border:0;background:#3b82f6;color:#fff;cursor:pointer;display:inline-flex;align-items:center;gap:10px;min-width:150px;justify-content:center;transition:background .2s}
button[disabled]{background:#2b4f8a;cursor:default}
.ring{width:16px;height:16px;border-radius:50%;border:2px solid rgba(255,255,255,.35);border-top-color:#fff;animation:spin .8s linear infinite;display:none}
button[disabled] .ring{display:inline-block}
.note{font-size:13px;color:#7c869a;min-height:18px;margin-top:12px}
@keyframes spin{to{transform:rotate(360deg)}}
</style></head>
<body><div class="box"><img src="icon-192.png" alt="" width="72" height="72"><h2>GAZE can’t be reached</h2>
<p>Check that the computer running GAZE is on, and that this device is on your home network (or connected to it remotely).</p>
<button id="go"><span class="ring"></span><span id="lbl">Try again</span></button>
<div class="note" id="note">Trying again by itself every 10 seconds…</div></div>
<script>
const go = document.getElementById('go'), lbl = document.getElementById('lbl'), note = document.getElementById('note');
let busy = false;
async function attempt(byHand) {
  if (busy) return; busy = true;
  go.disabled = true; lbl.textContent = 'Connecting…';
  const started = Date.now();
  let ok = false;
  try { const r = await fetch('api/auth/state?t=' + Date.now(), {cache: 'no-store'}); ok = r.ok; } catch (e) {}
  // (Long enough to see it trying.)
  await new Promise(r => setTimeout(r, Math.max(0, 900 - (Date.now() - started))));
  if (ok) { lbl.textContent = 'Connected — opening…'; location.reload(); return; }
  go.disabled = false; lbl.textContent = 'Try again'; busy = false;
  note.textContent = (byHand ? 'Still can’t reach GAZE. ' : '') + 'Trying again by itself every 10 seconds…';
}
go.onclick = () => attempt(true);
setInterval(() => { if (!document.hidden) attempt(false); }, 10000);
</script></body></html>`;

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(SHELL).then(c => c.add('icon-192.png')).then(() => self.skipWaiting()));
});
self.addEventListener('activate', (e) => {
  e.waitUntil(caches.keys().then(keys => Promise.all(keys.filter(k => k !== SHELL).map(k => caches.delete(k)))).then(() => self.clients.claim()));
});
self.addEventListener('fetch', (e) => {
  const u = new URL(e.request.url);
  if (e.request.method !== 'GET' || u.origin !== location.origin) return;
  if (e.request.mode === 'navigate') {
    e.respondWith(fetch(e.request).catch(() => new Response(OFFLINE, {headers: {'Content-Type': 'text/html; charset=utf-8'}})));
  } else if (u.pathname.endsWith('/icon-192.png')) {
    e.respondWith(fetch(e.request).catch(() => caches.match('icon-192.png')));
  }
});
