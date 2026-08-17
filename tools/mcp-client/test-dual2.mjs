// 强抢场景：页 B 手动点连接（connect(true) 强抢锁）→ 踢页 A → 页 A 应收敛（提示且不互踢）
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js'

const sleep = ms => new Promise(r => setTimeout(r, ms))
const t = new StdioClientTransport({
  command: 'npx',
  args: ['-y', 'chrome-devtools-mcp@latest', '--browserUrl', 'http://127.0.0.1:9222', '--allowUnrestrictedPaths'],
  stderr: 'pipe',
})
const c = new Client({ name: 'dual-test2', version: '1.0.0' })
await c.connect(t)

async function pages() {
  const r = await c.callTool({ name: 'list_pages', arguments: {} })
  const text = r.content.find(x => x.type === 'text').text
  return [...text.matchAll(/^(\d+):\s*(.*?)\s*\(([^)]+)\)/gm)].map(m => ({ idx: Number(m[1]), url: m[3] }))
}
async function sel(idx) {
  const r = await c.callTool({ name: 'select_page', arguments: { pageId: Number(idx), bringToFront: true } })
  if (r.isError) throw new Error('select failed')
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
  const cr = await c.callTool({ name: 'click', arguments: { uid: uid[1] } })
  return { ok: !cr.isError }
}
const videoState = tag => evalIn(`() => {
  const v = document.querySelector('video');
  return JSON.stringify({
    connected: !!document.querySelector('.v-stats'),
    connecting: !!document.querySelector('.v-connecting'),
    rs: v ? v.readyState : null,
    vw: v ? v.videoWidth : 0,
    t: v ? +v.currentTime.toFixed(1) : null,
    err: document.querySelector('.v-empty-text')?.textContent?.trim() || null,
    btn: [...document.querySelectorAll('.v-overlay button')].map(b => b.textContent.trim())
  });
}`)

const out = {}
let ps = await pages()
const pageA = ps.find(p => p.url.includes('#/console'))
let pageB = ps.find(p => p.url.includes('#/devices'))
if (!pageB) {
  await c.callTool({ name: 'new_page', arguments: { url: 'http://localhost:5173/#/devices' } })
  await sleep(3000)
  ps = await pages()
  pageB = ps.find(p => p.url.includes('#/devices'))
}
out.pageA = pageA.idx
out.pageB = pageB.idx

// 页 A 当前状态（应仍连接中）
await sel(pageA.idx)
out.A_before = await videoState()

// 页 B：点连接按钮（强抢）
await sel(pageB.idx)
await sleep(1500)
out.B_click = await clickText('连接')
await sleep(10000)
out.B_after = await videoState()

// 页 A：等待静默检测（~4s）+ 锁检查放弃
await sel(pageA.idx)
await sleep(4000)
out.A_t1 = await videoState()
await sleep(8000)
out.A_t2 = await videoState()
await sleep(8000)
out.A_t3 = await videoState()

out.lock = await evalIn(`() => JSON.stringify(JSON.parse(localStorage.getItem('gb_webrtc_lock') || 'null'))`)

console.log(JSON.stringify(out, null, 2))
await c.close()
