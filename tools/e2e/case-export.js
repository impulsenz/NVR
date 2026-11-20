// Evidence export, end to end: a case with an H.265 clip (what many cameras record, and Chrome can't play) is
// exported with and without "play in any web browser"; the review page is opened from the extracted folder and the
// video must play in the first, and say why it can't in the second. Needs ffmpeg (with libx265), Python 3, Node and
// Playwright; `cargo build` first.
const { chromium } = require('playwright');
const { spawn, execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const BIN = process.env.GAZE_BIN || path.join(__dirname, '..', '..', 'target', 'debug', 'nvr');
const PORT = process.env.GAZE_PORT || '18198';
const BASE = `http://127.0.0.1:${PORT}`;
const fail = (m) => { console.error('FAIL: ' + m); process.exitCode = 1; };
(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gaze-case-'));
  fs.writeFileSync(path.join(dir, 'nvr.toml'), `[server]\nlisten = "127.0.0.1:${PORT}"\nhttps_port = 0\n\n[[cameras]]\nid = "front"\nname = "Front"\nurl = "rtsp://127.0.0.1:1/front"\n`);
  const cam = path.join(dir, 'recordings', 'front');
  fs.mkdirSync(cam, { recursive: true });
  const p2 = (n) => String(n).padStart(2, '0');
  const start = new Date(Date.now() - 10 * 60000);
  const f = path.join(cam, `front_${start.getFullYear()}-${p2(start.getMonth() + 1)}-${p2(start.getDate())}_${p2(start.getHours())}-${p2(start.getMinutes())}-${p2(start.getSeconds())}.mp4`);
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=10', '-t', '6', '-c:v', 'libx265', '-pix_fmt', 'yuv420p', '-tag:v', 'hvc1', f]);
  fs.utimesSync(f, new Date(start.getTime() + 8000), new Date(start.getTime() + 8000));
  const server = spawn(BIN, ['--dir', dir], { stdio: ['ignore', 'pipe', 'pipe'] });
  const stop = () => { try { server.kill(); } catch {} };
  process.on('exit', stop);
  for (let i = 0; i < 60; i++) { try { if ((await fetch(`${BASE}/api/auth/state`)).ok) break; } catch {} await new Promise((r) => setTimeout(r, 500)); }
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1300, height: 800 } });
  await ctx.request.post(`${BASE}/api/auth/setup`, { data: { username: 'admin', password: 'smoke-test-password-1' } });
  const c = await (await ctx.request.post(`${BASE}/api/cases`, { data: { name: 'Codec test', notes: '' } })).json();
  const it = await ctx.request.post(`${BASE}/api/cases/${c.id}/items`, { data: { kind: 'clip', camera: 'front', from: new Date(start.getTime() + 1000).toISOString(), to: new Date(start.getTime() + 6000).toISOString(), title: 'Clip' } });
  if (!it.ok()) { fail('adding the clip: ' + (await it.text())); stop(); process.exit(1); }
  const run = async (form, label) => {
    const r = await ctx.request.post(`${BASE}/api/cases/${c.id}/export`, { form, timeout: 120000 });
    if (!r.ok()) { fail(label + ': export ' + r.status()); return null; }
    const out = path.join(dir, label);
    fs.mkdirSync(out);
    fs.writeFileSync(out + '.zip', await r.body());
    execFileSync('python3', ['-I', '-c', 'import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])', out + '.zip', out]);
    return out;
  };
  const check = async (out, label, expectPlay) => {
    const files = fs.readdirSync(out, { recursive: true });
    const web = files.filter((n) => /\.web\.mp4$/.test(n));
    if (expectPlay !== web.length > 0) fail(`${label}: browser copy ${web.length ? 'present' : 'missing'}`);
    const page = await ctx.newPage();
    await page.goto('file://' + path.join(out, 'index.html'));
    await page.click('.card');
    await page.waitForTimeout(2500);
    const st = await page.evaluate(() => { const v = document.querySelector('#pv video'); return v ? { w: v.videoWidth, ready: v.readyState, err: !!v.error, note: !!document.querySelector('.noplay') } : null; });
    console.log(label, JSON.stringify(st), 'files:', files.filter((n) => /\.mp4$/.test(n)).join(', '));
    if (!st) fail(label + ': no video element');
    else if (expectPlay && !(st.w > 0 && !st.err)) fail(label + ': the video did not play ' + JSON.stringify(st));
    else if (!expectPlay && !st.note) fail(label + ': no explanation shown for a video that cannot play ' + JSON.stringify(st));
    await page.close();
  };
  const a = await run({ playable: '1' }, 'with-copy'); if (a) await check(a, 'with copy', true);
  const b = await run({}, 'original-only'); if (b) await check(b, 'original only', false);
  await browser.close(); stop();
  fs.rmSync(dir, { recursive: true, force: true });
  if (!process.exitCode) console.log('case export e2e ok');
})();
