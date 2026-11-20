// Timeline previews, end to end: starts GAZE in a scratch folder, plants one
// recording and two hours of time-lapse pictures (the files scrub.rs writes),
// opens Recordings and checks that the blue line shows, that scrubbing puts a
// picture from those pages on every camera, and that a moment in a gap shows
// none. Needs ffmpeg, Node and Playwright; `cargo build` first.
const { chromium } = require('playwright');
const { spawn, execFileSync } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const BIN = process.env.GAZE_BIN || path.join(__dirname, '..', '..', 'target', 'debug', 'nvr');
const PORT = process.env.GAZE_PORT || '18191';
const BASE = `http://127.0.0.1:${PORT}`;
const HOUR = 3600000;
const fail = (m) => { console.error('FAIL: ' + m); process.exitCode = 1; };

(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gaze-scrub-'));
  fs.writeFileSync(path.join(dir, 'nvr.toml'), `[server]\nlisten = "127.0.0.1:${PORT}"\nhttps_port = 0\n\n[[cameras]]\nid = "front"\nname = "Front"\nurl = "rtsp://127.0.0.1:1/front"\n`);
  const cam = path.join(dir, 'recordings', 'front');
  fs.mkdirSync(path.join(cam, '.scrub'), { recursive: true });
  // The recording: the last 100 minutes (a short real video, its time stretched by the file's dates).
  const now = Date.now(), start = new Date(now - 100 * 60000);
  const p2 = (n) => String(n).padStart(2, '0');
  const name = `front_${start.getFullYear()}-${p2(start.getMonth() + 1)}-${p2(start.getDate())}_${p2(start.getHours())}-${p2(start.getMinutes())}-${p2(start.getSeconds())}.mp4`;
  const mp4 = path.join(cam, name);
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=5', '-t', '4', '-pix_fmt', 'yuv420p', mp4]);
  fs.utimesSync(mp4, new Date(now - 60000), new Date(now - 60000));
  // Pictures: the hour of 90 minutes ago (all of it) and the hour before now (its first 20 minutes missing).
  const sheet = path.join(dir, 'sheet.jpg');
  execFileSync('ffmpeg', ['-v', 'error', '-y', '-f', 'lavfi', '-i', 'testsrc2=size=256x144:rate=10', '-vf', 'tile=10x10', '-frames:v', '1', sheet]);
  const h0 = Math.floor((now - 90 * 60000) / HOUR) * HOUR, h1 = h0 + HOUR;
  for (const [h, have] of [[h0, [[0, 359]]], [h1, [[120, 359]]]]) {
    const pv = [1, 2, 3, 4];
    for (let n = 0; n < 4; n++) fs.copyFileSync(sheet, path.join(cam, '.scrub', `${h}-${n}.jpg`));
    fs.writeFileSync(path.join(cam, '.scrub', `${h}.json`), JSON.stringify({ h, interval: 10, tw: 256, th: 144, slots: 360, pages: 4, aspect: 16 / 9, have, pv }));
  }
  const server = spawn(BIN, ['--dir', dir], { stdio: ['ignore', 'pipe', 'pipe'] });
  let log = '';
  server.stdout.on('data', (d) => (log += d));
  server.stderr.on('data', (d) => (log += d));
  const stop = () => { try { server.kill(); } catch {} };
  process.on('exit', stop);
  for (let i = 0; i < 60; i++) {
    try { if ((await fetch(`${BASE}/api/auth/state`)).ok) break; } catch {}
    await new Promise((r) => setTimeout(r, 500));
    if (i === 59) { console.error('GAZE did not start:\n' + log.slice(-800)); stop(); process.exit(2); }
  }
  const browser = await chromium.launch();
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const r = await ctx.request.post(`${BASE}/api/auth/setup`, { data: { username: 'admin', password: 'smoke-test-password-1' } });
  if (!r.ok()) { fail('setup ' + r.status()); stop(); process.exit(2); }
  const api = await ctx.request.get(`${BASE}/api/scrub/front?from=${h0}&to=${h1 + HOUR}`);
  const j = await api.json();
  if (j.hours.length !== 2 || j.per_page !== 100) fail('api/scrub: ' + JSON.stringify(j).slice(0, 200));
  if ((await ctx.request.get(`${BASE}/media/front/scrub/${h0}/0?v=1`)).status() !== 200) fail('a page is not served');
  if ((await ctx.request.get(`${BASE}/media/front/scrub/${h0}/9?v=1`)).status() !== 404) fail('a page that is not there should be 404');
  if ((await ctx.request.get(`${BASE}/media/front/scrub/${h0 + 5}/0`)).status() !== 404) fail('an hour off the grid should be 404');

  const page = await ctx.newPage();
  const problems = [];
  page.on('pageerror', (e) => problems.push('script error: ' + e.message));
  const asked = [];
  page.on('request', (q) => { if (/\/(api\/scrub|media\/front\/scrub)\//.test(q.url())) asked.push(q.url()); });
  await page.goto(`${BASE}/#recordings`);
  await page.waitForSelector('#tl .tlp', { timeout: 20000 });
  await page.waitForFunction(() => document.querySelectorAll('#tl .tlp i').length > 0, null, { timeout: 20000 }).catch(() => fail('no blue line under the camera'));
  const kinds = await page.$$eval('#tl .tlp i', (els) => els.map((e) => e.className));
  console.log('blue line pieces:', kinds.length, [...new Set(kinds)].join(','));
  // Scrub to a moment with a picture, then one inside the missing 20 minutes of the second hour.
  const shows = async (T) => {
    await page.evaluate((T) => { tl.T = T; previewAll(); }, T);
    // (The page has to arrive first: a few seconds at most.)
    for (let i = 0; i < 20; i++) {
      await page.waitForTimeout(250);
      await page.evaluate(() => previewAll());  // (dragging calls it on every pointer move)
      if (await page.$$eval('#tl-videos .surface .sprite', (els) => els.some((e) => !e.hidden && /\/scrub\//.test(e.style.backgroundImage)))) break;
    }
    return page.$$eval('#tl-videos .surface .sprite', (els) => els.map((e) => (e.hidden ? '' : e.style.backgroundImage)));
  };
  const a = await shows(h0 + 30 * 60000);
  if (!a.some((x) => /\/scrub\//.test(x))) fail('no scrub picture while scrubbing: ' + JSON.stringify(a));
  // Loaded pages turn the line solid.
  await page.waitForTimeout(1500);
  const solid = await page.$$eval('#tl .tlp i.s-ready', (e) => e.length);
  if (!solid) fail('nothing turned "in memory" after the pages loaded');
  // A moment 15 minutes from any picture is not pictured; one 3 s from a picture is.
  const found = await page.evaluate(([x, y]) => [!!scrubFind('front', x), !!scrubFind('front', y)], [h1 + 5 * 60000, h0 + 40 * 60000 + 3000]);
  if (found[0] || !found[1]) fail('nearest picture rule: ' + JSON.stringify(found));
  if (problems.length) fail(problems.join('\n'));
  console.log('requests:', asked.length);
  await browser.close();
  stop();
  fs.rmSync(dir, { recursive: true, force: true });
  if (!process.exitCode) console.log('scrub e2e ok');
})();
