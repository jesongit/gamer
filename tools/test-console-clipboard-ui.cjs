// Isolated HTTP clipboard acceptance; no game, account or device APIs.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const fs = require('node:fs'), path = require('node:path'), net = require('node:net');
const { spawn } = require('node:child_process');
(async () => {
  const root = path.resolve(__dirname, '..');
  const listener = net.createServer();
  await new Promise(resolve => listener.listen(0, '127.0.0.1', resolve));
  const port = listener.address().port;
  await new Promise(resolve => listener.close(resolve));
  const fixture = path.join(root, 'web', `.clipboard-qa-${process.pid}.html`);
  fs.writeFileSync(fixture, `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><div id="app"></div><script type="module">
  import {createApp, ref, reactive} from 'vue/dist/vue.esm-bundler.js';
  import Dialog from '/src/components/console/ConsoleClipboardDialog.vue';
  import {useConsoleClipboard} from '/src/components/console/useConsoleClipboard.js';
  import {useConsoleDeviceManager} from '/src/components/console/useConsoleDeviceManager.js';
  import {installDialogAccessibility} from '/src/components/ui/dialog-accessibility.js';
  import '/src/styles/theme.css';
  createApp({components:{Dialog}, setup() {
    const stage = ref(null), store=reactive({deviceId:'phone'}), sent=ref([]), allowed=ref(true);
    const focus = () => stage.value?.focus();
    const manager=useConsoleDeviceManager({store, devicesData:ref([]), connected:ref(true), consoleRuntime:{scanning:ref(false)},
      toast:()=>{}, guardManualInput:()=>allowed.value, sendControl:msg=>{sent.value.push(msg); return true}, requestPasteText:id=>clip.openPaste(id)});
    const clip=useConsoleClipboard({deviceId:()=>store.deviceId, context:()=>store.deviceId, canUse:()=>allowed.value,
      copyFromDevice:async()=>({text:'游戏复制 中文🙂'}), pasteText:manager.pasteText, releaseKeys:()=>{}, focusStage:focus, toast:()=>{}});
    window.fixture={sent,allowed};
    return {stage,clip,capture:clip.capture,manager,sent};
  }, template:\`<main style="padding:24px;max-width:900px;margin:auto"><h1>剪贴板隔离验证</h1><label>本地文本<textarea id="local" class="input" rows="2"></textarea></label><div id="stage" ref="stage" tabindex="0" style="padding:32px;margin:20px 0;border:1px solid var(--border)" @keydown="clip.handleKeyDown" @keyup="clip.handleKeyUp" @paste="clip.handlePaste">游戏画面焦点<textarea ref="capture" style="position:absolute;width:1px;height:1px;clip-path:inset(50%)" tabindex="-1" /></div><button class="btn" @click="manager.clipboard()">粘贴</button><output id="sent">{{JSON.stringify(sent)}}</output><Dialog :state="clip.dialog" @close="clip.close" @submit="clip.submit" /></main>\`}).mount('#app');
  installDialogAccessibility();
  </script></html>`, { flag: 'wx' });
  const vite = spawn(process.execPath, [path.join(root, 'web/node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'],
    { cwd: path.join(root, 'web'), stdio: 'ignore', windowsHide: true, env: { ...process.env, __VITE_ADDITIONAL_SERVER_ALLOWED_HOSTS: 'clipboard.test' } });
  let browser;
  try {
    for (let i = 0; ; i++) { try { if ((await fetch(`http://127.0.0.1:${port}/${path.basename(fixture)}`)).ok) break; } catch {} if (i > 80) throw Error('Vite startup failed'); await new Promise(resolve => setTimeout(resolve, 250)); }
    browser = await chromium.launch({ channel: 'msedge', headless: true, args: ['--host-resolver-rules=MAP clipboard.test 127.0.0.1', '--no-proxy-server'] });
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } }), errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    page.on('requestfailed', request => errors.push(`${request.url()}: ${request.failure()?.errorText}`));
    await page.route('**/api/**', route => route.abort());
    const navigation = await page.goto(`http://clipboard.test:${port}/${path.basename(fixture)}`);
    try { await page.locator('#stage').waitFor({ timeout: 5000 }); }
    catch { throw Error(JSON.stringify({ errors, status: navigation.status(), url: page.url(), html: (await page.content()).slice(0, 1500) })); }
    if (await page.evaluate(() => isSecureContext || !!navigator.clipboard)) throw Error('Fixture must be insecure HTTP without Clipboard API');
    const local = page.locator('#local'), text = '粘贴 中文🙂';
    await local.fill(text); await local.press('Control+A'); await local.press('Control+C');
    await page.locator('#stage').focus(); await page.keyboard.press('Control+V');
    await page.waitForFunction(() => window.fixture.sent.value.length === 1);
    const sent = await page.evaluate(() => window.fixture.sent.value[0]);
    if (JSON.stringify(sent) !== JSON.stringify({ type: 'clipboard', text, paste: true })) throw Error('Native HTTP paste lost text');
    await page.locator('#stage').focus(); await page.keyboard.press('Control+C');
    // Copy is asynchronous. Wait for its fallback field to be removed.
    await page.waitForTimeout(100);
    await local.fill(''); await local.press('Control+V');
    if (await local.inputValue() !== '游戏复制 中文🙂') throw Error('Native HTTP copy did not update clipboard');
    await page.getByRole('button', { name: '粘贴', exact: true }).click();
    const dialog = page.getByRole('dialog'); await dialog.waitFor();
    await page.locator('#clipboard-text').press('Control+V');
    fs.mkdirSync(path.join(root, 'server/target'), { recursive: true });
    await page.screenshot({ path: path.join(root, 'server/target/clipboard-desktop.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({ path: path.join(root, 'server/target/clipboard-mobile.png'), fullPage: true });
    if (await page.evaluate(() => document.documentElement.scrollWidth > innerWidth)) throw Error('Horizontal overflow');
    await page.getByRole('button', { name: '发送到游戏', exact: true }).click();
    await page.waitForFunction(() => window.fixture.sent.value.length === 2);
    await page.locator('#stage').focus();
    await page.evaluate(() => { window.fixture.allowed.value = false });
    await page.keyboard.press('Control+V');
    if (await page.evaluate(() => window.fixture.sent.value.length) !== 2) throw Error('Locked paste reached game');
    if (errors.length) throw Error(errors.join('\n'));
    console.log(JSON.stringify({ insecure_http: true, native_copy: true, native_paste: true, toolbar_paste: true, locked_paste_blocked: true, desktop: '1280x900', mobile: '390x844', page_errors: errors }));
  } finally { await browser?.close(); vite.kill(); fs.unlinkSync(fixture); }
})().catch(error => { console.error(error); process.exitCode = 1 });
