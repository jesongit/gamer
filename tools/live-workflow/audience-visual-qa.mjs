// Isolated visual fixtures only. Never connects to Gamer, a platform or a device.
// For compact-only evidence: --output-dir server/target/<new-directory>
// --prefix compact --compact --audience-only. Each case also saves a tight
// main-element PNG with omitBackground, independent of the diagnostic viewport.
import assert from 'node:assert/strict';
import { createServer as createHttpServer } from 'node:http';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import { dirname, join, resolve, relative, isAbsolute } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repo = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
function argument(name, fallback) {
  const index = process.argv.indexOf(name);
  if (index < 0) return fallback;
  assert(process.argv[index + 1] && !process.argv[index + 1].startsWith('--'), `${name} needs a value`);
  return process.argv[index + 1];
}
const out = resolve(repo, argument('--output-dir', 'server/target/audience-feedback-20261015'));
const targetRoot = resolve(repo, 'server/target');
assert(!isAbsolute(relative(targetRoot, out)) && !relative(targetRoot, out).startsWith('..'), 'QA evidence must stay under server/target');
const prefix = argument('--prefix', '');
assert(!prefix || /^[a-z0-9][a-z0-9-]{0,47}$/.test(prefix), 'Use a short lowercase filename prefix');
const artifact = name => join(out, prefix ? `${prefix}-${name}` : name);
const compact = process.argv.includes('--compact');
const audienceOnly = process.argv.includes('--audience-only');
const require = createRequire(import.meta.url);
const { chromium } = require('C:/Users/Jeson/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright');
const chrome = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const mode = process.argv.includes('--before-only') ? 'before' : process.argv.includes('--confirm') ? 'confirm' : 'after';
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
await mkdir(out, { recursive: true });
const beforePath = join(out, 'before.html');
if (!compact) {
try {
  await readFile(beforePath);
} catch (error) {
  if (error.code !== 'ENOENT') throw error;
  const before = execFileSync('git', ['-C', join(repo, 'plugins'), 'show', 'HEAD:gamer-live/host/audience.html']);
  await writeFile(beforePath, before, { flag: 'wx' });
  await writeFile(join(out, 'before-source.json'), JSON.stringify({
    plugin_head: execFileSync('git', ['-C', join(repo, 'plugins'), 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim(),
    source: 'gamer-live/host/audience.html', sha256: sha(before), frozen_at: new Date().toISOString(),
  }, null, 2), { flag: 'wx' });
}
}

const now = Date.now();
const item = (number, state = 'waiting', extra = {}) => ({ number, name: '云游戏日常', viewer: `观众${number}`, state, test: false, position: 1, changed_at: now - 2000, started_at: state === 'running' ? now - 65000 : null, finished_at: ['success', 'failed'].includes(state) ? now - 2000 : null, ...extra });
const base = () => ({ time: now, paused: false, configured: true, connection: 'connected', device_busy: false, current: null, featured: null, waiting: [], waiting_count: 0, notices: [], text: '' });
const running = () => ({ ...base(), current: item(101, 'running', { viewer: '正在扫码的观众' }), waiting: [1, 2, 3].map((n) => item(101 + n, 'waiting', { position: n })), waiting_count: 3, text: '发送“日常”加入排队\n请准备好扫码登录' });
const injected = '<img src=x onerror="window.__qaInjected=1"> & <script>window.__qaInjected=2</script>';
const fixtures = {
  running: running(),
  long: { ...running(), current: item(201, 'running', { name: '云游戏日常任务名称很长时仍然需要完整可读', viewer: '这是一位名字非常长的中文观众不能被横向截断' }), waiting: Array.from({ length: 12 }, (_, n) => item(202 + n, 'waiting', { position: n + 1, viewer: n % 2 ? 'AnExtremelyLongEnglishViewerNameWithoutSpacesForOverflowChecks' : '正在等待登录的中文观众名字很长很长', name: n % 2 ? 'DailyCloudGameRoutineWithLongEnglishTaskName' : '排队中的云游戏日常任务' })), waiting_count: 12 },
  success: { ...base(), featured: item(301, 'success', { viewer: '刚刚完成的观众' }), text: '完成后请等待下一位观众' },
  failure: { ...base(), featured: item(302, 'failed', { viewer: '本次未完成的观众' }) },
  priority: { ...running(), featured: item(399, 'success', { name: '上一个任务不应取代当前任务', viewer: '上一个观众' }) },
  empty: base(),
  injection: { ...running(), current: item(401, 'running', { name: injected, viewer: injected }), text: injected },
  stale: running(),
};
const requestCounts = new Map();
const fixtureRequests = [];
let staticServer, viteServer, browser;
const report = { mode, prefix, compact, synthetic: true, real_platform_tested: false, production_accessed: false, screenshots: [], checks: [], failures: [], blocked_requests: [], fixture_requests: fixtureRequests };
const baseline = compact ? JSON.parse(await readFile(join(repo, 'server/target/audience-feedback-20261015/visual-qa-after.json'), 'utf8')) : null;
if (baseline) {
  report.baseline = { report: 'server/target/audience-feedback-20261015/visual-qa-after.json', source_sha256: baseline.audience_source_sha256, main_height_360: baseline.screenshots.find(entry => entry.scenario === 'running' && entry.width === 360).metrics.nodes.main.height };
  if (!process.argv.includes('--resume')) {
    try { await writeFile(artifact('baseline-before.json'), JSON.stringify(report.baseline, null, 2), { flag: 'wx' }); }
    catch (error) {
      if (error.code !== 'EEXIST') throw error;
      assert.deepEqual(JSON.parse(await readFile(artifact('baseline-before.json'), 'utf8')), report.baseline, 'Keep the original compact-layout baseline immutable');
    }
  }
}
if (process.argv.includes('--resume')) {
  assert(mode !== 'before', 'Before evidence is immutable');
  const prior = JSON.parse(await readFile(artifact(`visual-qa-${mode}.json`), 'utf8'));
  assert(prior.fatal && !prior.passed, 'Resume only an incomplete driver round');
  await writeFile(artifact(`visual-qa-${mode}-driver-partial.json`), JSON.stringify(prior, null, 2), { flag: 'wx' });
  Object.assign(report, prior);
  delete report.fatal;
  fixtureRequests.push(...prior.fixture_requests);
  report.fixture_requests = fixtureRequests;
  report.completed_in_same_frozen_round = true;
  report.driver_note = 'Completed missing cases after correcting the stale-state selector; prior screenshots were not repeated.';
}
const check = (name, passed, details) => { report.checks.push({ name, passed: !!passed, details }); if (!passed) report.failures.push(name); };

function listen(server) {
  return new Promise((resolveListen, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', () => resolveListen(`http://127.0.0.1:${server.address().port}`)); });
}
function closeServer(server) { return server ? new Promise(resolveClose => server.close(resolveClose)) : Promise.resolve(); }
function documentMetrics() {
  const selectors = ['html', 'body', 'main', '#activity', '#featured', '#waiting', '#custom-text', '#connection', '#current', '#status', '.task-title', '.waiting-title', '#featured .viewer', '.waiting-row'];
  const nodes = Object.fromEntries(selectors.map(selector => {
    const el = document.querySelector(selector);
    if (!el) return [selector, null];
    const style = getComputedStyle(el), rect = el.getBoundingClientRect();
    return [selector, { text: ['html', 'body'].includes(selector) ? undefined : el.textContent, display: style.display, visibility: style.visibility, whiteSpace: style.whiteSpace, fontSize: style.fontSize, lineHeight: style.lineHeight, color: style.color, background: style.backgroundColor, opacity: style.opacity, x: rect.x, y: rect.y, bottom: rect.bottom, width: rect.width, height: rect.height, scrollWidth: el.scrollWidth, clientWidth: el.clientWidth, scrollHeight: el.scrollHeight, clientHeight: el.clientHeight, overflowY: style.overflowY }];
  }));
  return { viewport: innerWidth, viewportHeight: innerHeight, documentWidth: document.documentElement.scrollWidth, bodyWidth: document.body.scrollWidth, bodyClass: document.body.className, text: document.body.innerText, nodes, listRows: document.querySelectorAll('#waiting li').length, injected: window.__qaInjected ?? null, unexpectedImages: document.querySelectorAll('img').length };
}
async function contextFor(width, height = 760) {
  const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 1, colorScheme: 'dark', reducedMotion: 'reduce' });
  await context.route('**/*', route => {
    const target = new URL(route.request().url());
    if (['data:', 'blob:'].includes(target.protocol) || (target.hostname === '127.0.0.1' && [new URL(staticBase).port, settingsBase && new URL(settingsBase).port].includes(target.port))) return route.continue();
    report.blocked_requests.push({ protocol: target.protocol, hostname: target.hostname, path: target.pathname });
    return route.abort();
  });
  return context;
}
async function captureAudience(name, width, file = 'audience.html') {
  const context = await contextFor(width);
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(`${staticBase}/${file}?case=${name}#fixture-read-token`, { waitUntil: 'domcontentloaded' });
  await page.waitForFunction(() => {
    const activity = document.querySelector('#activity');
    if (activity) return /任务中|空闲中/.test(activity.textContent);
    return !/正在连接|连接中…/.test(document.querySelector('#status')?.textContent || '正在连接');
  }, undefined, { timeout: 8000 });
  if (name === 'stale') await page.waitForFunction(() => document.body.classList.contains('stale') || /中断|过期|无法更新|等待恢复/.test(document.querySelector('#activity,#connection,#status')?.textContent || ''), undefined, { timeout: 12000 });
  await page.evaluate(() => document.fonts.ready);
  let queueScroll = null;
  if (compact && name === 'long') {
    await page.locator('#waiting').evaluate(el => { el.scrollTop = el.scrollHeight; });
    await page.waitForTimeout(1100);
    queueScroll = await page.locator('#waiting').evaluate(el => ({ scrollTop: el.scrollTop, scrollHeight: el.scrollHeight, clientHeight: el.clientHeight }));
    await page.locator('#waiting').evaluate(el => { el.scrollTop = 0; });
  }
  const screenshot = artifact(`${mode}-${name}-${width}.png`);
  await page.screenshot({ path: screenshot, fullPage: true, omitBackground: true });
  const mainScreenshot = artifact(`${mode}-${name}-${width}-main.png`);
  await page.locator('main').screenshot({ path: mainScreenshot, omitBackground: true });
  const metrics = await page.evaluate(documentMetrics);
  report.screenshots.push({ scenario: name, width, path: screenshot, main_path: mainScreenshot, metrics, errors, queueScroll });
  check(`${name}-${width}: no script errors`, errors.length === 0, errors);
  if (mode !== 'before') {
    check(`${name}-${width}: no horizontal page overflow`, metrics.documentWidth <= width && metrics.bodyWidth <= width, { document: metrics.documentWidth, body: metrics.bodyWidth });
    if (name === 'running') {
      check(`${name}-${width}: current task and all three waiting viewers`, metrics.listRows === 3 && metrics.nodes['#featured']?.text?.includes('101'), { rows: metrics.listRows, primary: metrics.nodes['#featured']?.text });
      check(`${name}-${width}: custom instructions retain both lines`, metrics.nodes['#custom-text']?.text?.includes('\n') && /pre|break-spaces/.test(metrics.nodes['#custom-text']?.whiteSpace || ''), metrics.nodes['#custom-text']);
    }
    if (name === 'long') check('long-320: waiting list is not truncated', metrics.listRows === 12, { rows: metrics.listRows });
    if (['success', 'failure'].includes(name)) check(`${name}: exactly one featured result`, await page.locator('#featured').count() === 1 && metrics.nodes['#featured']?.text?.includes(name === 'success' ? '301' : '302'), metrics.nodes['#featured']);
    if (name === 'priority') check('current task takes priority over latest result', metrics.nodes['#featured']?.text?.includes('101') && !metrics.nodes['#featured']?.text?.includes('399'), metrics.nodes['#featured']);
    if (name === 'empty') check('empty: honest idle state', /等待|暂无/.test(metrics.text) && metrics.listRows === 0, metrics.text);
    if (name === 'injection') check('untrusted text stays plain text', metrics.injected === null && metrics.unexpectedImages === 0 && metrics.nodes['#custom-text']?.text?.includes(injected), { injected: metrics.injected, images: metrics.unexpectedImages, text: metrics.nodes['#custom-text']?.text });
    if (name === 'stale') check('stale: old snapshot clearly marked non-live', /stale|offline|error/.test(metrics.bodyClass) || /上次|已过期|非实时|中断/.test(metrics.text), { bodyClass: metrics.bodyClass, text: metrics.text });
    if (compact) {
      const transparent = color => color === 'transparent' || /^rgba\([^)]*,\s*0\)$/.test(color || '');
      check(`${name}-${width}: html and body remain transparent`, transparent(metrics.nodes.html?.background) && transparent(metrics.nodes.body?.background), { html: metrics.nodes.html?.background, body: metrics.nodes.body?.background });
      check(`${name}-${width}: content starts at upper left`, metrics.nodes.main.x <= 8 && metrics.nodes.main.y <= 8, metrics.nodes.main);
      if (name === 'running') {
        const wanted = { '#activity': 24, '.task-title': 22, '.waiting-title': 20, '#featured .viewer': 17, '#custom-text': 18 };
        for (const [selector, size] of Object.entries(wanted)) check(`${name}-${width}: larger type ${selector}`, parseFloat(metrics.nodes[selector]?.fontSize || '0') >= size, { actual: metrics.nodes[selector]?.fontSize, minimum: size });
        if (width === 360) check('running-360: main is shorter than prior layout', metrics.nodes.main.height < report.baseline.main_height_360, { previous: report.baseline.main_height_360, current: metrics.nodes.main.height });
      }
      if (['running', 'long'].includes(name)) check(`${name}-${width}: bottom instructions stay visible`, metrics.nodes['#custom-text']?.display !== 'none' && metrics.nodes['#custom-text']?.height > 0 && metrics.nodes['#custom-text']?.bottom <= metrics.viewportHeight + 1, metrics.nodes['#custom-text']);
      if (name === 'long') check('long-320: complete queue scrolls and retains position across poll', metrics.nodes['#waiting'].scrollHeight > metrics.nodes['#waiting'].clientHeight && /auto|scroll/.test(metrics.nodes['#waiting'].overflowY) && queueScroll.scrollTop > 0, queueScroll);
    }
  }
  await context.close();
}

let staticBase, settingsBase;
try {
  staticServer = createHttpServer(async (req, res) => {
    try {
      const url = new URL(req.url, 'http://127.0.0.1');
      if (url.pathname === '/state') {
        const name = new URL(req.headers.referer || '/?case=running', 'http://127.0.0.1').searchParams.get('case') || 'running';
        const requests = (requestCounts.get(name) || 0) + 1; requestCounts.set(name, requests);
        fixtureRequests.push({ scenario: name, request: requests });
        res.setHeader('Content-Type', 'application/json'); res.setHeader('Cache-Control', 'no-store');
        if (name === 'stale' && requests > 1) { res.writeHead(503); return res.end('{"error":"fixture offline"}'); }
        return res.end(JSON.stringify(fixtures[name] || fixtures.running));
      }
      if (url.pathname === '/favicon.ico') { res.writeHead(204); return res.end(); }
      const path = url.pathname === '/before.html' ? beforePath : url.pathname === '/audience.html' ? join(repo, 'plugins/gamer-live/host/audience.html') : null;
      if (!path) { res.writeHead(404); return res.end('Fixture route not found'); }
      res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(await readFile(path));
    } catch { res.writeHead(500); res.end('Fixture error'); }
  });
  staticBase = await listen(staticServer);
  browser = await chromium.launch({ executablePath: chrome, headless: true, args: ['--disable-background-networking', '--disable-component-update', '--no-first-run'] });
  if (mode === 'before') {
    await captureAudience('running', 360, 'before.html');
  } else {
    const source = await readFile(join(repo, 'plugins/gamer-live/host/audience.html'));
    if (report.audience_source_sha256) assert.equal(sha(source), report.audience_source_sha256, 'Do not resume against changed audience source');
    report.audience_source_sha256 = sha(source);
    let settingsFile;
    if (!audienceOnly) {
    const webRequire = createRequire(join(repo, 'web/package.json'));
    const vitePath = join(dirname(webRequire.resolve('vite/package.json')), 'dist/node/index.js');
    const { createServer } = await import(pathToFileURL(vitePath));
    const vuePluginModule = await import(pathToFileURL(webRequire.resolve('@vitejs/plugin-vue')));
    const vuePlugin = vuePluginModule.default;
    settingsFile = join(repo, 'plugins/gamer-live/ui/AudienceSettings.vue');
    if (report.settings_source_sha256) assert.equal(sha(await readFile(settingsFile)), report.settings_source_sha256, 'Do not resume against changed settings source');
    report.settings_source_sha256 = sha(await readFile(settingsFile));
    const mockApi = `export const api={async callExtension(id,action,values={}){window.__qaCalls.push({id,action,values});const s=window.__qaState;if(action==='audience.read')return structuredClone(s);if(action==='audience.open'){s.enabled=true;s.url=window.__qaUrl;return structuredClone(s)}if(action==='audience.close'){s.enabled=false;s.url=null;return structuredClone(s)}if(action==='audience.save'){if(values.expected_version!==s.version)throw Error('fixture version conflict');s.text=values.text;s.version='fixture-saved';return structuredClone(s)}throw Error('Unexpected fixture action '+action)}};`;
    const entry = `import {createApp} from ${JSON.stringify('/@fs/' + webRequire.resolve('vue/dist/vue.esm-bundler.js').replaceAll('\\','/'))};import AudienceSettings from ${JSON.stringify('/@fs/' + settingsFile.replaceAll('\\','/'))};createApp(AudienceSettings).mount('#app');`;
    await writeFile(join(out, 'qa-settings-entry.mjs'), entry);
    const settingsSeed = { enabled: true, url: staticBase + '/audience.html#fixture-read-token', text: '发送日常加入排队\n请准备扫码登录', version: 'fixture-v1' };
    const settingsHtml = `<!doctype html><html lang="zh-CN"><meta charset="UTF-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>:root{--bg-0:#17191c;--bg-1:#202226;--bg-2:#282a2f;--bg-3:#34363b;--field:#191b1f;--border:#3b3d42;--text-0:#eeece6;--text-1:#cecac1;--text-2:#b6b6ae;--accent:#dda951;--accent-ink:#201d16;--danger:#ed9696;--ok:#8fc9ae}*{box-sizing:border-box}body{margin:0;padding:16px;background:var(--bg-0);color:var(--text-0);font-family:'Microsoft YaHei','Segoe UI',sans-serif}#app{max-width:900px;margin:auto}</style><div id="app"></div><script>window.__qaCalls=[];window.__qaOpened=[];window.__qaState=${JSON.stringify(settingsSeed)};window.__qaUrl=window.__qaState.url;window.open=(...args)=>{window.__qaOpened.push(args);return null};Object.defineProperty(navigator,'clipboard',{value:{writeText:async value=>{window.__qaClipboard=value}}});</script><script type="module" src="/qa-settings-entry.mjs"></script></html>`;
    await writeFile(join(out, 'qa-settings.html'), settingsHtml);
    viteServer = await createServer({ root: out, configFile: false, logLevel: 'error', resolve: { alias: { vue: webRequire.resolve('vue/dist/vue.esm-bundler.js') } }, plugins: [{ name: 'isolated-audience-api', enforce: 'pre', resolveId(id) { if (id === '../../../web/src/api') return '\0audience-qa-api'; }, load(id) { if (id === '\0audience-qa-api') return mockApi; } }, vuePlugin()], server: { host: '127.0.0.1', port: 0, strictPort: false, fs: { allow: [repo] } }, optimizeDeps: { include: [] } });
    await viteServer.listen(); settingsBase = `http://127.0.0.1:${viteServer.httpServer.address().port}`;
    }
    const scenarios = [['running', 960], ['running', 360], ...(compact ? [['running', 320]] : []), ['long', 320], ['success', 360], ['failure', 360], ['priority', 360], ['empty', 360], ['injection', 360], ['stale', 360]].filter(([name, width]) => !report.screenshots.some(entry => entry.scenario === name && entry.width === width));
    // One bounded round: all shipped widths and states share the same source.
    for (let start = 0; start < scenarios.length; start += 3) await Promise.all(scenarios.slice(start, start + 3).map(([name, width]) => captureAudience(name, width)));
    for (const width of audienceOnly ? [] : [960, 360]) {
      const context = await contextFor(width), page = await context.newPage(), errors = [];
      page.on('pageerror', error => errors.push(error.message));
      await page.goto(`${settingsBase}/qa-settings.html`, { waitUntil: 'networkidle' });
      await page.waitForFunction(() => window.__qaCalls?.some(call => call.action === 'audience.read'));
      const checkbox = page.locator('input[type=checkbox]');
      check(`settings-${width}: restores enabled state from read`, await checkbox.count() === 1 && await checkbox.isChecked(), await page.evaluate(() => window.__qaCalls));
      check(`settings-${width}: only switch and text configuration`, await page.locator('select,input[type=number]').count() === 0 && await page.locator('textarea').count() === 1, { selects: await page.locator('select').count(), numberFields: await page.locator('input[type=number]').count(), textareas: await page.locator('textarea').count() });
      const text = await page.locator('textarea').inputValue();
      check(`settings-${width}: restores saved instruction`, text.includes('发送日常加入排队') && text.includes('请准备扫码登录'), text);
      const preview = page.getByRole('link').filter({ hasText: /预览|打开/ });
      check(`settings-${width}: service exposes URL without a window-opening link`, await preview.count() === 0);
      const copy = page.getByRole('button', { name: /复制/ }).first();
      if (await copy.count()) await copy.click();
      check(`settings-${width}: URL can be copied`, await page.evaluate(() => window.__qaClipboard === window.__qaUrl));
      await checkbox.uncheck(); await page.waitForFunction(() => window.__qaCalls.some(call => call.action === 'audience.close'));
      await checkbox.check(); await page.waitForFunction(() => window.__qaCalls.some(call => call.action === 'audience.open'));
      await page.locator('textarea').fill('发送“日常”加入排队\n请准备好扫码登录');
      await page.getByRole('button', { name: /保存/ }).click();
      await page.waitForFunction(() => window.__qaCalls.some(call => call.action === 'audience.save'));
      check(`settings-${width}: saves text with optimistic version and keeps enabled`, await page.evaluate(() => window.__qaState.enabled && window.__qaState.text.includes('\n') && window.__qaCalls.find(call => call.action === 'audience.save').values.expected_version === 'fixture-v1'));
      check(`settings-${width}: opening is never automatic`, await page.evaluate(() => window.__qaOpened.length === 0), await page.evaluate(() => window.__qaOpened));
      check(`settings-${width}: only audience management APIs are called`, await page.evaluate(() => window.__qaCalls.every(call => call.id === 'gamer-live' && ['audience.read', 'audience.open', 'audience.close', 'audience.save'].includes(call.action))));
      const screenshot = artifact(`${mode}-settings-${width}.png`); await page.screenshot({ path: screenshot, fullPage: true });
      const metrics = await page.evaluate(documentMetrics);
      check(`settings-${width}: no horizontal page overflow`, metrics.documentWidth <= width && metrics.bodyWidth <= width, { document: metrics.documentWidth, body: metrics.bodyWidth });
      check(`settings-${width}: no script errors`, errors.length === 0, errors);
      report.screenshots.push({ scenario: 'settings', width, path: screenshot, metrics, errors, calls: await page.evaluate(() => window.__qaCalls) });
      await context.close();
    }
    check('audience source stayed frozen for the batch', sha(await readFile(join(repo, 'plugins/gamer-live/host/audience.html'))) === report.audience_source_sha256);
    if (settingsFile) check('settings source stayed frozen for the batch', sha(await readFile(settingsFile)) === report.settings_source_sha256);
  }
  report.passed = report.failures.length === 0 && report.blocked_requests.length === 0;
} catch (error) {
  report.passed = false; report.fatal = error.stack || String(error);
} finally {
  await browser?.close(); await viteServer?.close(); await closeServer(staticServer);
  await writeFile(artifact(`visual-qa-${mode}.json`), JSON.stringify(report, null, 2));
}
console.log(JSON.stringify({ mode, passed: report.passed, screenshots: report.screenshots.length, checks: report.checks.length, failures: report.failures, fatal: report.fatal, report: artifact(`visual-qa-${mode}.json`) }, null, 2));
if (!report.passed) process.exitCode = 1;
