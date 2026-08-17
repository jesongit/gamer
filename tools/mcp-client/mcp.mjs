// chrome-devtools-mcp 测试驱动：通过 MCP 协议驱动 Chrome 测试 GameBot 前端
// 用法: node mcp.mjs <command> [args...]
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'
import fs from 'node:fs'
import path from 'node:path'

const OUT_DIR = path.join(import.meta.dirname, 'out')
fs.mkdirSync(OUT_DIR, { recursive: true })

const SERVER_ARGS = ['-y', 'chrome-devtools-mcp@latest', '--browserUrl', 'http://127.0.0.1:9222', '--allowUnrestrictedPaths']

let client = null
let transport = null
let pageId = null

async function connect() {
  if (client) return
  transport = new StdioClientTransport({ command: 'npx', args: SERVER_ARGS, stderr: 'pipe' })
  client = new Client({ name: 'gamer-mcp-test', version: '1.0.0' })
  await client.connect(transport)
}

async function disconnect() {
  if (client) { try { await client.close() } catch (e) {} client = null; transport = null }
}

async function selectPage() {
  const r = await client.callTool({ name: 'list_pages', arguments: {} })
  const text = (r.content.find(c => c.type === 'text') || {}).text || ''
  // markdown 格式: ## Pages\n1: <title> (<url>) [selected]
  const m = text.match(/^\s*(\d+):/m)
  if (!m) throw new Error('cannot parse pages: ' + text.slice(0, 300))
  pageId = m[1].trim()
  await client.callTool({ name: 'select_page', arguments: { pageId, bringToFront: true } })
  return pageId
}

async function selectPageN(n) {
  await client.callTool({ name: 'select_page', arguments: { pageId: String(n), bringToFront: true } })
  pageId = String(n)
  return pageId
}

async function call(name, args = {}) {
  const r = await client.callTool({ name, arguments: args })
  const parts = []
  for (const c of r.content || []) {
    if (c.type === 'text') parts.push(c.text)
    else if (c.type === 'image') {
      const f = path.join(OUT_DIR, `img-${Date.now()}.png`)
      fs.writeFileSync(f, Buffer.from(c.data, 'base64'))
      parts.push(`[image saved: ${f}]`)
    } else parts.push(JSON.stringify(c))
  }
  return { tool: name, ok: !r.isError, result: parts.join('\n') }
}

async function evalInPage(fn) {
  const r = await client.callTool({ name: 'evaluate_script', arguments: { function: fn } })
  const text = (r.content.find(c => c.type === 'text') || {}).text
  return text
}

// evaluate_script 返回 "Script ran on page and returned:\n```json\n<data>\n```"，抽取 JSON 解析
async function evalJson(fn) {
  const t = await evalInPage(fn)
  const m = t.match(/```(?:json)?\s*([\s\S]*?)```/)
  if (m) return JSON.parse(m[1])
  try { return JSON.parse(t) } catch (e) { throw new Error('evalJson parse failed: ' + t.slice(0, 300)) }
}

// 从 a11y 快照里找包含指定文本的元素 uid（优先 button）
async function findUid(text) {
  const r = await client.callTool({ name: 'take_snapshot', arguments: { verbose: true } })
  const snap = (r.content.find(c => c.type === 'text') || {}).text || ''
  fs.writeFileSync(path.join(OUT_DIR, 'snapshot.txt'), snap)
  const lines = snap.split('\n')
  // 匹配形如: uid=12 button "连接控制" 或 ref=12 ...
  for (const line of lines) {
    if (!line.includes(text)) continue
    const m = line.match(/(?:uid|ref)="?([A-Za-z0-9_\-:.]+)"?/)
    if (m) return m[1]
  }
  return null
}

async function clickText(text) {
  const uid = await findUid(text)
  if (!uid) throw new Error(`uid not found for text: ${text}`)
  return call('click', { uid })
}

// ---------- 命令 ----------

async function cmdTools() {
  const tools = await client.listTools()
  console.log(JSON.stringify(tools.tools.map(t => t.name), null, 2))
}

async function cmdPages() {
  const r = await client.callTool({ name: 'list_pages', arguments: {} })
  console.log(JSON.stringify(r, null, 2))
}

async function cmdNav(url) {
  await selectPage()
  console.log(JSON.stringify(await call('navigate_page', { url }), null, 2))
}

async function cmdEval(expr) {
  await selectPage()
  console.log(await evalInPage(expr))
}

async function cmdSnap() {
  await selectPage()
  console.log(JSON.stringify(await call('take_snapshot', { verbose: true }), null, 2))
}

async function cmdClickText(text) {
  await selectPage()
  console.log(JSON.stringify(await clickText(text), null, 2))
}

async function cmdShot(name) {
  await selectPage()
  const f = path.join(OUT_DIR, name || `shot-${Date.now()}.png`)
  console.log(JSON.stringify(await call('take_screenshot', { format: 'png', filePath: f }), null, 2))
}

async function cmdConsole() {
  await selectPage()
  console.log(JSON.stringify(await call('list_console_messages', { pageSize: 200 }), null, 2))
}

async function cmdLogin() {
  await selectPage()
  const href = await evalJson('() => location.href')
  if (!href.includes('/login')) { console.log(JSON.stringify({ step: 'login', skipped: true, href })); return }
  // 用 JS 直接设置 v-model 输入（Vue 监听 input 事件）
  await evalInPage(`(() => {
    const set = (sel, val) => {
      const el = document.querySelector(sel);
      const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, val);
      el.dispatchEvent(new Event('input', { bubbles: true }));
    };
    set('input[autocomplete="username"]', 'admin');
    set('input[autocomplete="current-password"]', 'admin123');
    return 'filled';
  })()`)
  await clickText('登 录')
  await new Promise(r => setTimeout(r, 2500))
  console.log(JSON.stringify({ step: 'login', href: await evalJson('() => location.href') }))
}

async function cmdDiag() {
  await selectPage()
  console.log(await evalInPage(`() => {
    const v = document.querySelector('video');
    const out = {
      url: location.href,
      hasVideo: !!v,
      readyState: v ? v.readyState : null,
      networkState: v ? v.networkState : null,
      videoWidth: v ? v.videoWidth : null,
      videoHeight: v ? v.videoHeight : null,
      currentTime: v ? +v.currentTime.toFixed(2) : null,
      paused: v ? v.paused : null,
      muted: v ? v.muted : null,
      srcObjectTracks: v && v.srcObject ? [...v.srcObject.getTracks()].map(t => ({kind: t.kind, readyState: t.readyState, muted: t.muted})) : null,
      overlayText: document.querySelector('.v-empty-text')?.textContent?.trim() || null,
      connecting: !!document.querySelector('.v-connecting'),
      statsVisible: !!document.querySelector('.v-stats'),
      statsText: document.querySelector('.v-stats')?.innerText || null,
      pcCount: window.__pcCount || 0,
      rtpStatsCount: window.__rtpStatsCount || 0,
      deviceCards: [...document.querySelectorAll('.device-card')].map(c => c.innerText.split('\\n').slice(0,3).join(' | '))
    };
    if (v && v.videoWidth > 0) {
      const c = document.createElement('canvas'); c.width = 64; c.height = 36;
      const ctx = c.getContext('2d');
      try { ctx.drawImage(v, 0, 0, 64, 36); } catch(e) { out.drawError = e.message; }
      const d = ctx.getImageData(0, 0, 64, 36).data;
      let sum = 0, sum2 = 0, n = d.length / 4;
      for (let i = 0; i < d.length; i += 4) { const l = 0.299*d[i] + 0.587*d[i+1] + 0.114*d[i+2]; sum += l; sum2 += l*l; }
      const mean = sum / n;
      out.pixel = { mean: +mean.toFixed(1), stdev: +Math.sqrt(sum2/n - mean*mean).toFixed(1) };
    }
    return JSON.stringify(out);
  }`))
}

async function cmdConnect() {
  await selectPage()
  const href = await evalJson('() => location.href')
  if (!href.includes('/console')) {
    console.log(JSON.stringify(await call('navigate_page', { url: 'http://localhost:5173/#/devices' }), null, 2))
    await new Promise(r => setTimeout(r, 4000))
    console.log('clicking 连接控制...')
    console.log(JSON.stringify(await clickText('连接控制'), null, 2))
  }
  // 等待 WebRTC 建立（最多 20s），每 2s 打点
  for (let i = 0; i < 10; i++) {
    await new Promise(r => setTimeout(r, 2000))
    const s = await evalJson(`() => {
      const v = document.querySelector('video');
      return JSON.stringify({ connecting: !!document.querySelector('.v-connecting'), connected: !!document.querySelector('.v-stats'), vw: v?.videoWidth || 0, vh: v?.videoHeight || 0, rs: v?.readyState ?? null });
    }`)
    console.log(`t+${(i + 1) * 2}s`, JSON.stringify(s))
    if (s.connected && s.vw > 0) break
  }
}

// ---------- main ----------

const [cmd, ...args] = process.argv.slice(2)
try {
  await connect()
  switch (cmd) {
    case 'tools': await cmdTools(); break
    case 'pages': await cmdPages(); break
    case 'nav': await cmdNav(args[0]); break
    case 'sel': await selectPageN(Number(args[0])); console.log(JSON.stringify({ selected: pageId })); break
    case 'eval': await cmdEval(args[0]); break
    case 'snap': await cmdSnap(); break
    case 'click': await cmdClickText(args[0]); break
    case 'shot': await cmdShot(args[0]); break
    case 'console': await cmdConsole(); break
    case 'login': await cmdLogin(); break
    case 'diag': await cmdDiag(); break
    case 'connect': await cmdConnect(); break
    default: console.error('unknown cmd: ' + cmd); process.exitCode = 2
  }
} catch (e) {
  console.error('ERROR:', e.message)
  process.exitCode = 1
} finally {
  await disconnect()
}
