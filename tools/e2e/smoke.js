// Page smoke test: starts GAZE in a scratch folder, signs in, opens every
// page and every Settings section in the day and the night theme, and fails
// on a script error, a console error, a failed request (other than the ones
// listed as expected) or a page that shows nothing.
//
//   cargo build            # the program under test (target/debug/nvr, or set GAZE_BIN)
//   node tools/e2e/smoke.js            # needs Node + Playwright
//
// Options (environment): GAZE_BIN, GAZE_PORT (default 18181), SHOTS=<folder>
// to keep a screenshot of every page and theme.
const { chromium } = require('playwright');
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const BIN = process.env.GAZE_BIN || path.join(__dirname, '..', '..', 'target', 'debug', 'nvr');
const PORT = process.env.GAZE_PORT || '18181';
const BASE = `http://127.0.0.1:${PORT}`;
const SHOTS = process.env.SHOTS;
const PASS = 'smoke-test-password-1';
const PAGES = ['live', 'alerts', 'recordings', 'plan', 'evidence', 'system', 'settings'];
// Keep in step with SECTIONS in static/app.js (the test reads it from there).
const sections = () => {
  const js = fs.readFileSync(path.join(__dirname, '..', '..', 'static', 'app.js'), 'utf8');
  const m = js.match(/const SECTIONS = \[([^\]]*)\]/);
  return m[1].split(',').map((s) => s.trim().replace(/['"]/g, '')).filter(Boolean);
};
// Requests that legitimately fail on an empty install (no cameras, no internet).
const EXPECTED = [/\/live\/[^/]+\/(frame\.jpg|stream\.mjpeg)/, /\/snapshot\// /* the test cameras can't be reached */, /\/api\/plan\/image/, /\/api\/watchdog/, /favicon/, /\/api\/update/, /\/api\/smart/, /\/overview\.jpg/ /* no cameras: no overview picture */];

(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gaze-smoke-'));
  // Two cameras that can't be reached: enough for the pages to show their players
  // (a page with no cameras hides layout bugs that only appear beside them).
  const cam = (id, name) => `\n[[cameras]]\nid = "${id}"\nname = "${name}"\nurl = "rtsp://127.0.0.1:1/${id}"\n`;
  fs.writeFileSync(path.join(dir, 'nvr.toml'), `[server]\nlisten = "127.0.0.1:${PORT}"\nhttps_port = 0\n` + cam('front', 'Front Door') + cam('garage', 'Garage'));
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
  const r = await ctx.request.post(`${BASE}/api/auth/setup`, { data: { username: 'admin', password: PASS } });
  if (!r.ok()) { console.error('setup failed', r.status(), await r.text()); stop(); process.exit(2); }

  const problems = [];
  const page = await ctx.newPage();
  let where = '';
  page.on('pageerror', (e) => problems.push(`${where}: script error: ${e.message}`));
  page.on('console', (m) => { if (m.type() === 'error' && !/Failed to load resource/.test(m.text())) problems.push(`${where}: console: ${m.text()}`); });
  page.on('response', (res) => {
    if (res.status() >= 400 && !EXPECTED.some((x) => x.test(res.url()))) problems.push(`${where}: ${res.status()} ${res.url().replace(BASE, '')}`);
  });

  const targets = [...PAGES.map((p) => `#${p}`), ...sections().map((s) => `#settings/${s}`)];
  let visited = 0;
  for (const theme of ['dark', 'light']) {
    await page.emulateMedia({ colorScheme: theme });
    for (const t of targets) {
      where = `${theme} ${t}`;
      await page.goto(`${BASE}/${t}`);
      await page.waitForTimeout(700);
      const text = (await page.locator('main').innerText().catch(() => '')).trim();
      if (!text) problems.push(`${where}: the page is empty`);
      if (SHOTS) { fs.mkdirSync(SHOTS, { recursive: true }); await page.screenshot({ path: path.join(SHOTS, `${theme}-${t.replace(/[#/]/g, '_')}.png`) }); }
      visited++;
    }
  }
  // The Recordings date picker must open above everything beside and below it
  // (it was once drawn under the players, with its right half hidden).
  for (const theme of ['dark', 'light']) {
    await page.emulateMedia({ colorScheme: theme });
    where = `${theme} recordings date picker`;
    await page.goto(`${BASE}/?picker=${theme}#recordings`);
    await page.waitForTimeout(1200);
    await page.locator('#f-date-btn').click();
    await page.waitForTimeout(300);
    const covered = await page.evaluate(() => {
      const pop = document.getElementById('f-date-pop');
      if (!pop || pop.hidden) return 'the date picker did not open';
      const r = pop.getBoundingClientRect();
      // Points across the picker, including its far right and bottom edges.
      for (const [fx, fy] of [[0.1, 0.1], [0.5, 0.1], [0.95, 0.1], [0.95, 0.5], [0.5, 0.95], [0.95, 0.95]]) {
        const hit = document.elementFromPoint(r.left + r.width * fx, r.top + r.height * fy);
        if (!hit || !pop.contains(hit)) return `something covers the date picker at ${Math.round(fx * 100)}% across, ${Math.round(fy * 100)}% down: ${hit && (hit.tagName + '.' + hit.className)}`;
      }
      return '';
    });
    if (covered) problems.push(`${where}: ${covered}`);
    visited++;
  }
  await browser.close();
  stop();
  fs.rmSync(dir, { recursive: true, force: true });
  const uniq = [...new Set(problems)];
  console.log(`${visited} page views (${targets.length} pages x 2 themes), ${uniq.length} problem(s)`);
  uniq.forEach((p) => console.log('  - ' + p));
  process.exit(uniq.length ? 1 : 0);
})();
