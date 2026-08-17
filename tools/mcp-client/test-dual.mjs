// 双页面互踢测试：单次 MCP 连接内操作两个标签页
// 场景：页 A 连接设备 → 页 B 也连接（踢 A）→ A 应检测到断流并因锁被 B 持有而停止重连
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'

const sleep = ms => new Promise(r => setTimeout(r, ms))
const t = new StdioClientTransport({
  command: 'npx',
  args: ['-y', 'chrome-devtools-mcp@latest', '--browserUrl', 'http://127.0.0.1:9222', '--allowUnrestrictedPaths'],
  stderr: 'pipe',
})
const c = new Client({ name: 'dual-test', version: '1.0.0' })
await c.connect(t)

async function pages() {
  const r = await c.callTool({ name: 'list_pages', arguments: {} })
  const text = r.content.find(x => x.type === 'text').text
  return [...text.matchAll(/^(\d+):\s*(.*?)\s*\(([^)]+)\)/gm)].map(m => ({ idx: m[1], title: m[2], url: m[3] }))
}
async function sel(idx) {
  const r = await c.callTool({ name: 'select_page', arguments: { pageId: Number(idx), bringToFront: true } })
  if (r.isError) throw new Error('select failed: ' + JSON.stringify(r.content))
}
async function evalIn(fn) {
  const r = await c.callTool({ name: 'evaluate_script', arguments: { function: fn } })
  const text = r.content.find(x => x.type === 'text')?.text || ''
  const m = text.match(/```(?:json)?\s*([\s\S]*?)```/)
  if (m) { try { return JSON.parse(m[1]) } catch (e) { return text } }
  try { return JSON.parse(text) } catch (e) { return text }
}
async function clickText(text) {
  const r = await c.callTool({ name: 'take_snapshot', arguments: { verbose: true } })
  const snap = r.content.find(x => x.type === 'text')?.text || ''
  const line = snap.split('\n').find(l => l.includes(text))
  if (!line) return { ok: false, why: `text not found: ${text}` }
  const uid = line.match(/(?:uid|ref)="?([A-Za-z0-9_\-:.]+)"?/)
  if (!uid) return { ok: false, why: 'no uid' }
  const cr = await c.callTool({ name: 'click', arguments: { uid: uid[1] } })
  return { ok: !cr.isError }
}
async function videoState(tag) {
  return evalIn(`() => {
    const v = document.querySelector('video');
    return JSON.stringify({
      tag: ${JSON.stringify(tag)},
      connected: !!document.querySelector('.v-stats'),
      connecting: !!document.querySelector('.v-connecting'),
      rs: v ? v.readyState : null,
      vw: v ? v.videoWidth : 0,
      t: v ? +v.currentTime.toFixed(1) : null,
      err: document.querySelector('.v-empty-text')?.textContent?.trim() || null
    });
  }`)
}

const out = {}
const ps = await pages()
out.pages = ps.map(p => `${p.idx}:${p.url.slice(-14)}`)

// 给每页打唯一标记
for (const p of ps) {
  await sel(p.idx)
  await evalIn(`() => { window.__tag = ${JSON.stringify('TAG-' + p.idx)}; return 'ok'; }`)
}

// 页 A（第一个 console 页）：确认已连接
const pageA = ps.find(p => p.url.includes('#/console') && p.title.includes('GameBot'))
const pageB = ps.find(p => p.url.includes('#/devices'))
out.pageA = pageA.idx
out.pageB = pageB.idx

await sel(pageA.idx)
out.A_before = await videoState('A')

// 页 B：连接控制（踢 A）
await sel(pageB.idx)
await sleep(1500)
const cr = await clickText('连接控制')
out.B_click = cr
await sleep(10000)
out.B_connected = await videoState('B')

// 页 A：等待静默检测 + 锁检查（~4s 静默 + 3s 重连延迟 + 余量）
await sel(pageA.idx)
await sleep(3000)
out.A_t1 = await videoState('A')
await sleep(8000)
out.A_t2 = await videoState('A')
await sleep(8000)
out.A_t3 = await videoState('A')

// 最终：锁归属
out.lock = await evalIn(`() => JSON.stringify(JSON.parse(localStorage.getItem('gb_webrtc_lock') || 'null'))`)

console.log(JSON.stringify(out, null, 2))
await c.close()
