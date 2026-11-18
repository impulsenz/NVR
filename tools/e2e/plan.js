// Floor-plan page test: starts GAZE in a scratch folder, makes a plan from the
// satellite map picker (tiles are stubbed in the browser, no internet), then
// checks wall clipping (`planSight`), mouse-wheel zoom, wall drawing and that
// the sidebar stays compact.
//
//   cargo build && node tools/e2e/plan.js        # GAZE_BIN, GAZE_PORT (18182)
const { chromium } = require('playwright');
const { spawn } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

const BIN = process.env.GAZE_BIN || path.join(__dirname, '..', '..', 'target', 'debug', 'nvr');
const PORT = process.env.GAZE_PORT || '18182';
const BASE = `http://127.0.0.1:${PORT}`;
// A 1x1 green PNG: the picker stretches it over each tile.
const TILE = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPj/HwADBwIAMCbHYQAAAABJRU5ErkJggg==', 'base64');

(async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gaze-plan-'));
  fs.writeFileSync(path.join(dir, 'nvr.toml'), `[server]\nlisten = "127.0.0.1:${PORT}"\nhttps_port = 0\n`);
  const server = spawn(BIN, ['--dir', dir], { stdio: 'ignore' });
  const stop = () => { try { server.kill(); } catch {} };
  process.on('exit', stop);
  for (let i = 0; ; i++) {
    try { if ((await fetch(`${BASE}/api/auth/state`)).ok) break; } catch {}
    if (i > 60) { console.error('GAZE did not start'); stop(); process.exit(2); }
    await new Promise((r) => setTimeout(r, 500));
  }
  const browser = await chromium.launch();
  const problems = [];
  const check = (ok, what) => { if (!ok) problems.push(what); };
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  const r = await ctx.request.post(`${BASE}/api/auth/setup`, { data: { username: 'admin', password: 'plan-test-password-1' } });
  if (!r.ok()) { console.error('setup failed', r.status()); stop(); process.exit(2); }
  const page = await ctx.newPage();
  page.on('pageerror', (e) => problems.push('script error: ' + e.message));
  page.on('dialog', (d) => d.accept());
  await page.route('http://tiles.test/**', (rt) => rt.fulfill({ status: 200, contentType: 'image/png', headers: { 'access-control-allow-origin': '*' }, body: TILE }));

  // 1. The map picker makes a plan picture and a scale.
  await page.goto(`${BASE}/#plan`);
  await page.waitForTimeout(1200);
  await page.locator('#pl-sat').click();
  await page.selectOption('#mp-src', 'custom');
  await page.fill('#mp-url', 'http://tiles.test/{z}/{x}/{y}.png');
  await page.fill('#mp-credit', 'Test tiles');
  await page.fill('#mp-q', '-41.2865, 174.7762');
  await page.keyboard.press('Enter');
  await page.waitForTimeout(1500);
  const v = await page.locator('#mp-view').boundingBox();
  await page.mouse.move(v.x + v.width / 2, v.y + v.height / 2);
  await page.mouse.wheel(0, -300);
  await page.locator('#mp-rot').fill('20');
  await page.waitForTimeout(1000);
  await page.locator('#mp-use').click();
  await page.waitForFunction(() => !document.getElementById('mp-view'), null, { timeout: 60000 }).catch(() => problems.push('map picker did not close'));
  await page.waitForTimeout(1500);
  const plan = await (await ctx.request.get(`${BASE}/api/plan`)).json();
  check(plan.image_w > 0 && plan.px_per_m > 0, 'the map picture did not give a plan with a scale');

  // 2. Wall clipping (planSight is the page's ray-cast).
  const los = await page.evaluate(() => {
    const cx = 500, cy = 500, R = 100, out = {};
    const far = (p) => Math.max(...p.map((q) => Math.hypot(q[0] - cx, q[1] - cy)));
    const area = (p) => { let a = 0; const all = [[cx, cy], ...p]; for (let i = 0; i < all.length; i++) { const [x1, y1] = all[i], [x2, y2] = all[(i + 1) % all.length]; a += x1 * y2 - x2 * y1; } return Math.abs(a) / 2; };
    const open = planSight(cx, cy, R, 0, 90, []);
    out.openFar = far(open); out.openArea = area(open);
    out.sector = Math.PI * R * R / 4;
    const wall = planSight(cx, cy, R, 0, 90, [[380, 460, 620, 460]]);
    out.wallStops = Math.min(...wall.map((q) => q[1])) >= 459.5;
    out.behind = Math.abs(area(planSight(cx, cy, R, 0, 90, [[380, 540, 620, 540]])) - out.openArea) < 1;
    // Doorway: beyond the wall only inside the wedge seen through the gap (x 480..520 at y 460).
    const gap = planSight(cx, cy, R, 0, 90, [[380, 460, 480, 460], [520, 460, 620, 460]]);
    out.doorway = gap.some((q) => q[1] < 455 && q[0] > 478 && q[0] < 522);
    out.nothingElseBeyond = gap.every((q) => q[1] >= 459.9 || Math.abs(q[0] - 500) <= 20 * (500 - q[1]) / 40 + 0.5);
    const room = planSight(cx, cy, R, 0, 360, [[450, 450, 550, 450], [550, 450, 550, 550], [550, 550, 450, 550], [450, 550, 450, 450]]);
    out.room = room.every((q) => q[0] >= 449.9 && q[0] <= 550.1 && q[1] >= 449.9 && q[1] <= 550.1);
    out.beyondRange = Math.abs(area(planSight(cx, cy, R, 0, 90, [[380, 300, 620, 300]])) - out.openArea) < 1;
    return out;
  });
  check(Math.abs(los.openFar - 100) < 0.5, 'open view should reach its range');
  check(Math.abs(los.openArea - los.sector) / los.sector < 0.03, 'open view is not a sector');
  check(los.wallStops, 'a wall does not stop the view');
  check(los.behind, 'a wall behind the camera changes the view');
  check(los.doorway, 'the view does not pass through a gap');
  check(los.nothingElseBeyond, 'the view leaks past a wall beside the gap');
  check(los.room, 'the view leaves a closed room');
  check(los.beyondRange, 'a wall beyond range changes the view');

  // 3. Wall drawing, then wheel zoom and a compact sidebar.
  await page.reload(); await page.waitForTimeout(1200);
  await page.locator('#pl-edit').click();
  await page.locator('.pl-tabs [data-tab="walls"]').first().click();
  await page.waitForTimeout(200);
  const st = await page.locator('#pl-stage').boundingBox();
  await page.locator('#wl-draw').click();
  await page.mouse.click(st.x + st.width * 0.3, st.y + st.height * 0.3);
  await page.mouse.click(st.x + st.width * 0.6, st.y + st.height * 0.3);
  await page.keyboard.press('Escape');
  await page.waitForTimeout(200);
  const walls = await page.locator('#pl-svg .pl-wall').count();
  check(walls >= 1, 'drawing a wall made nothing');
  const scale = () => page.evaluate(() => planUI.k);
  const k0 = await scale();
  await page.mouse.move(st.x + st.width / 2, st.y + st.height / 2);
  await page.mouse.wheel(0, -400);
  await page.waitForTimeout(300);
  const k1 = await scale();
  check(k1 > k0, 'the mouse wheel does not zoom in');
  await page.mouse.wheel(0, 800);
  await page.waitForTimeout(300);
  check((await scale()) < k1, 'the mouse wheel does not zoom out');
  const side = await page.locator('#pl-side').boundingBox();
  check(side && side.height <= 1000, `the sidebar is taller than the window (${side && Math.round(side.height)} px)`);

  await browser.close();
  stop();
  fs.rmSync(dir, { recursive: true, force: true });
  console.log(`plan page: ${problems.length} problem(s)`);
  problems.forEach((p) => console.log('  - ' + p));
  process.exit(problems.length ? 1 : 0);
})();
