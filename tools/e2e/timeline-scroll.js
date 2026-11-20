// Moving back along the timeline without limit: the data loads days before the date filter as the window
// approaches its start, the window only ever moves back (it never snaps to a place it was before), and it stops
// where there is no more footage. Needs ffmpeg, Node and Playwright; `cargo build` first.
const { chromium } = require('playwright');
const { spawn, execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');
const BIN = process.env.GAZE_BIN || path.join(__dirname, '..', '..', 'target', 'debug', 'nvr');
const PORT = process.env.GAZE_PORT || '18197';
const BASE = `http://127.0.0.1:${PORT}`;
const fail = (m) => { console.error('FAIL: ' + m); process.exitCode = 1; };
(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gaze-tls-'));
  fs.writeFileSync(path.join(dir, 'nvr.toml'), `[server]\nlisten = "127.0.0.1:${PORT}"\nhttps_port = 0\n\n[[cameras]]\nid = "front"\nname = "Front"\nurl = "rtsp://127.0.0.1:1/front"\n`);
  const cam = path.join(dir, 'recordings', 'front');
  fs.mkdirSync(cam, { recursive: true });
  const p2 = (n) => String(n).padStart(2, '0');
  const mk = (ago) => {
    const t = new Date(Date.now() - ago);
    const f = path.join(cam, `front_${t.getFullYear()}-${p2(t.getMonth() + 1)}-${p2(t.getDate())}_${p2(t.getHours())}-${p2(t.getMinutes())}-${p2(t.getSeconds())}.mp4`);
    execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=160x90:rate=5', '-t', '2', '-pix_fmt', 'yuv420p', f]);
    fs.utimesSync(f, new Date(t.getTime() + 600000), new Date(t.getTime() + 600000));
    return t.getTime();
  };
  const oldest = mk(5 * 864e5);
  mk(60 * 3600e3); mk(30 * 3600e3); mk(2 * 3600e3);
  const server = spawn(BIN, ['--dir', dir], { stdio: ['ignore', 'pipe', 'pipe'] });
  const stop = () => { try { server.kill(); } catch {} };
  process.on('exit', stop);
  for (let i = 0; i < 60; i++) { try { if ((await fetch(`${BASE}/api/auth/state`)).ok) break; } catch {} await new Promise((r) => setTimeout(r, 500)); }
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1500, height: 900 } });
  await ctx.request.post(`${BASE}/api/auth/setup`, { data: { username: 'admin', password: 'smoke-test-password-1' } });
  const page = await ctx.newPage();
  const errs = [];
  page.on('pageerror', (e) => errs.push(e.message));
  await page.goto(`${BASE}/#recordings/timeline`);
  await page.waitForSelector('#tl-videos .surface', { timeout: 20000 });
  await page.waitForTimeout(1500);
  await page.evaluate(() => { store.set('tl-vertical', true); applyTlOrient(); });
  await page.evaluate(() => { $('tl-zoom').value = '86400'; $('tl-zoom').dispatchEvent(new Event('change')); });
  await page.waitForTimeout(1200);
  const from0 = await page.evaluate(() => rec.from.getTime());
  const box = await page.$eval('#vtl', (e) => { const r = e.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; });
  await page.mouse.move(box[0], box[1]);
  // Scroll back a long way (wheel up = earlier), watching that the window never goes forward.
  let prev = await page.evaluate(() => tl.winStart), wentForward = 0;
  for (let i = 0; i < 400; i++) {
    await page.mouse.wheel(0, -300);
    await page.waitForTimeout(25);
    const w = await page.evaluate(() => tl.winStart);
    if (w > prev + 1000) wentForward++;
    prev = w;
  }
  await page.waitForTimeout(2500);
  const r = await page.evaluate(() => ({ from: rec.from.getTime(), floor: tl.floor, win: tl.winStart, segs: tl.data.flatMap((c) => c.segments.map((s) => Date.parse(s.start))), label: document.getElementById('f-date-btn').textContent }));
  if (wentForward) fail(`the window went forward ${wentForward} times while scrolling back`);
  if (!(r.from < from0)) fail('the loaded range did not grow back: ' + JSON.stringify({ from0, from: r.from }));
  if (!r.segs.some((t) => Math.abs(t - oldest) < 2000)) fail('the oldest footage (5 days ago) was not loaded: ' + r.segs.length + ' segments');
  if (r.floor == null) fail('it did not find the end of the footage');
  if (r.floor != null && r.win < r.floor - 1) fail('the window went past the floor');
  console.log('days loaded back:', ((from0 - r.from) / 864e5).toFixed(1), 'floor set:', r.floor != null, 'label:', r.label, 'segments:', r.segs.length);
  if (errs.length) fail(errs.join('\n'));
  await browser.close(); stop();
  fs.rmSync(dir, { recursive: true, force: true });
  if (!process.exitCode) console.log('timeline scroll e2e ok');
})();
