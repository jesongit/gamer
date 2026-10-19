// Isolated browser acceptance for account settings. Never contacts deployed Gamer.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs');
const path = require('node:path');
const net = require('node:net');
const { spawn } = require('node:child_process');
(async () => {
  const root = path.resolve(__dirname, '..');
  const listener = net.createServer();
  await new Promise(resolve => listener.listen(0, '127.0.0.1', resolve));
  const port = listener.address().port;
  await new Promise(resolve => listener.close(resolve));
  const fixture = path.join(root, 'web', `.account-qa-${process.pid}.html`);
  fs.writeFileSync(fixture, `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app" style="max-width:1000px;margin:24px auto;padding:0 12px"></div><script type="module">import {createApp} from 'vue';import Component from '/src/components/AdminAccountSettings.vue';import '/src/styles/theme.css';createApp(Component).mount('#app')</script></html>`, { flag: 'wx' });
  const vite = spawn(process.execPath, [path.join(root, 'web/node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: path.join(root, 'web'), stdio: 'ignore', windowsHide: true });
  let browser;
  try {
    const url = `http://127.0.0.1:${port}/${path.basename(fixture)}`;
    for (let attempt = 0; ; attempt++) {
      try { if ((await fetch(url)).ok) break; } catch {}
      if (attempt > 80) throw new Error('Isolated Vite did not start');
      await new Promise(resolve => setTimeout(resolve, 250));
    }
    browser = await chromium.launch({ ...(process.platform === 'win32' ? { channel: 'msedge' } : {}), headless: true });
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
    const errors = [], submitted = [];
    page.on('pageerror', error => errors.push(error.message));
    let reject = true;
    await page.route('**/api/auth/account', route => {
      const put = route.request().method() === 'PUT';
      if (put) submitted.push(route.request().postDataJSON());
      return route.fulfill({ status: put && reject ? 403 : 200, contentType: 'application/json', body: JSON.stringify(put ? (reject ? { error: 'invalid_current_password' } : { ok: true }) : { username: 'admin', revision: 'r1', password_editable: true }) });
    });
    await page.goto(url);
    const inputs = page.locator('.account-fields input');
    await inputs.nth(0).fill('new-owner');
    await inputs.nth(1).fill('isolated-fixture-password');
    await page.screenshot({ path: path.join(root, 'server/target/account-ui-desktop.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.getByRole('button', { name: '保存并重新登录' }).click();
    await page.getByRole('alert').filter({ hasText: '当前密码不正确' }).waitFor();
    if (await inputs.nth(0).inputValue() !== 'new-owner' || await inputs.nth(1).inputValue() !== '') throw new Error('Rejected password must clear; username must remain');
    if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw new Error('Mobile overflow');
    await page.screenshot({ path: path.join(root, 'server/target/account-ui-mobile.png'), fullPage: true });
    reject = false;
    await inputs.nth(1).fill('isolated-fixture-password');
    await page.getByRole('button', { name: '保存并重新登录' }).click();
    await page.waitForURL(url => url.hash === '#/login?account_changed=1');
    if (submitted.length !== 2 || submitted[1].new_password !== '' || await inputs.nth(1).inputValue() !== '') throw new Error('Username-only submission or secret clearing failed');
    if (errors.length) throw new Error(errors.join('\n'));
    console.log(JSON.stringify({ desktop: '1280x900', mobile: '390x844', username_only: true, error_preserves_edits: true, success_requires_login: true, page_errors: errors }));
  } finally {
    if (browser) await browser.close();
    vite.kill();
    fs.unlinkSync(fixture);
  }
})().catch(error => { console.error(error.message); process.exitCode = 1; });
