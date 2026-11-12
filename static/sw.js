// GAZE as an installed app. Everything comes fresh from GAZE; when the
// computer running it can't be reached, the app shows a friendly message
// instead of the browser's error page.
const SHELL = 'gaze-shell-v2';
const OFFLINE = `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>GAZE</title></head>
<body style="margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;background:#0f1218;color:#e6e9f0;font:16px system-ui,sans-serif;text-align:center">
<div style="padding:24px;max-width:420px"><img src="icon-192.png" alt="" width="72" height="72"><h2>GAZE can’t be reached</h2>
<p style="color:#9aa3b5">Check that the computer running GAZE is on, and that this device is on your home network (or connected to it remotely).</p>
<button onclick="location.reload()" style="font:inherit;padding:10px 20px;border-radius:8px;border:0;background:#3b82f6;color:#fff">Try again</button></div></body></html>`;

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
