// Keep the Live page open (all cameras) for N seconds, signed in.
const { chromium } = require('playwright');
(async () => {
  const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1600, height: 900 } });
  await p.goto('http://localhost:18080/'); await p.waitForTimeout(800);
  if (await p.isVisible('#auth-pass')) { await p.fill('#auth-user', 'admin'); await p.fill('#auth-pass', 'testpass123'); await p.keyboard.press('Enter'); }
  await p.waitForTimeout(1500); await p.goto('http://localhost:18080/#live');
  await p.waitForTimeout(+(process.argv[2] || 60) * 1000); await b.close();
})();
