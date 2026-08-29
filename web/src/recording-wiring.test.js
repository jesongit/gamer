// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { ref } from 'vue'
import { altScopeFlags, PROJECTION_ALT_HINT, useRecording } from './composables/useRecording'
import { useScriptEditorShell } from './composables/useScriptEditorShell'
import { serialize } from './script-editor/codec'
import { composeRegionName } from './api'

/**
 * 阶段 6 后半接线回归（plan §11 / §16.3）：
 * - Alt 作用域三态（投屏 Alt 录制中暂停，模板/取色 Alt 保留）；
 * - 录制入口/状态栏/离开保护的静态接线断言；
 * - 真实 recording/ 模块 + 真实编辑器外壳的集成：指针时序（freeze 先于 down）、
 *   手势分类 → find vs match→swipe、上传定稿/失败重试/坐标降级/丢弃、
 *   停止排空解锁保存、多指提示一次、录制锚点保序。
 */

const read = (path) => readFileSync(join(process.cwd(), 'src', path.replace(/^\.\//, '')), 'utf8')

const SCRIPT_YAML = `steps:
  - log: start
  - tap: [0.5, 0.5]
`

const FRAME_URL = 'data:image/png;base64,QUJD'

const flush = () => new Promise(r => setTimeout(r, 0))
const pt = (relX, relY) => ({ relX, relY, frameW: 1920, frameH: 1080 })

async function makeHarness({ templates = [], cropImpl, start = true } = {}) {
  const notifications = []
  const controlMsgs = []
  const freezeCalls = []
  const api = {
    uploadTemplateRegion: vi.fn(async (name, b64, pkg, region) => ({ ok: true, name })),
    listTemplates: vi.fn(async () => [...templates]),
  }
  const shell = useScriptEditorShell({
    api: { getScript: async () => ({ id: 'com.demo/main.yaml', name: 'main.yaml', package: 'com.demo', version: 'v1', content: SCRIPT_YAML }) },
  })
  await shell.loadScript('com.demo/main.yaml')
  const templatesData = ref([...templates])
  let t = 1_700_000_000_000
  const rec = useRecording({
    shell,
    activePkg: ref('com.demo'),
    connected: ref(true),
    videoElement: ref({ videoWidth: 1920, videoHeight: 1080 }),
    templatesData,
    api,
    notify: (msg, type) => notifications.push({ msg, type }),
    sendControl: (msg) => controlMsgs.push(msg),
    clientToDevice: (x, y) => ({ x: Math.round(x), y: Math.round(y) }),
    freezeFrame: () => { freezeCalls.push(1); return { dataUrl: FRAME_URL, width: 1920, height: 1080 } },
    cropPng: cropImpl || (async () => 'Zml4ZWQ='),
    now: () => t,
  })
  const el = {
    rec, shell, api, templatesData, notifications, controlMsgs, freezeCalls,
    advanceTime: (ms) => { t += ms },
  }
  if (start) el.start = rec.start()
  return el
}

describe('Alt 作用域拆分（plan §11.2）', () => {
  it('三态真值表：altMode 开 + 非录制全开；录制中仅投屏 Alt 暂停；altMode 关全关', () => {
    expect(altScopeFlags(true, false)).toEqual({ projection: true, template: true, crop: true })
    expect(altScopeFlags(true, true)).toEqual({ projection: false, template: true, crop: true })
    expect(altScopeFlags(false, false)).toEqual({ projection: false, template: false, crop: false })
    expect(altScopeFlags(false, true)).toEqual({ projection: false, template: false, crop: false })
  })

  it('投屏 Alt 暂停的文案不是「已禁用」，而是「模板添加与取色仍可用」', () => {
    expect(PROJECTION_ALT_HINT).toBe('投屏 Alt 暂停；模板添加与取色仍可用')
  })

  it('Console 静态接线：录制分支先于 Alt 分支，作用域 computed 与提示文案到位', () => {
    const src = read('./views/Console.vue')
    expect(src).toContain("import { useRecording, altScopeFlags, PROJECTION_ALT_HINT } from '../composables/useRecording'")
    expect(src).toContain('const projectionAltEnabled = computed(() => altScopeFlags(altMode.value, recording.active).projection)')
    expect(src).toContain('const templateAltEnabled = computed(() => altScopeFlags(altMode.value, recording.active).template)')
    expect(src).toContain('const cropAltEnabled = computed(() => altScopeFlags(altMode.value, recording.active).crop)')
    // 投屏 mouse 链路：录制分支在最前（录制中 Alt 特殊语义被透传取代）
    const downFn = src.slice(src.indexOf('function onMouseDown'), src.indexOf('function onMouseMove'))
    expect(downFn.indexOf('recording.active')).toBeGreaterThan(-1)
    expect(downFn.indexOf('recording.active')).toBeLessThan(downFn.indexOf('isAltAction(e)'))
    expect(downFn).toContain('e.button !== 0')
  })
})

describe('录制入口与状态栏 / 离开保护（静态接线）', () => {
  const src = read('./views/Console.vue')
  const template = src.slice(0, src.indexOf('</template>'))

  it('录制按钮位于「启动应用」右侧，可用条件与禁用文案齐全', () => {
    const ctrlRow = template.slice(template.indexOf('tb-row-ctrl'), template.indexOf('tb-sep'))
    expect(ctrlRow.indexOf('启动应用')).toBeLessThan(ctrlRow.indexOf('data-test="recording-toggle"'))
    expect(ctrlRow).toContain('recording.toggle()')
    expect(ctrlRow).toContain('recording.available')
    expect(ctrlRow).toContain('recording.buttonTitle')
    expect(ctrlRow).toContain('⏺ 录制')
  })

  it('录制状态栏：状态 · 录制目标 · 待处理数量 · 停止按钮', () => {
    expect(template).toContain('data-test="recording-bar"')
    expect(template).toContain('录制到：')
    expect(template).toContain('待处理 {{ recording.pendingCount }}')
    expect(template).toContain('data-test="recording-stop"')
    expect(template).toContain("recording.phase === 'recording' ? '录制中' : '停止中…'")
    expect(template).toContain('<RecordingCropPanel v-if="recording.panelDraft"')
  })

  it('离开保护：保存/退出/跳转/分区切换/关页全覆盖，录制中阻断直至排空', () => {
    expect(src).toContain("import RecordingCropPanel from '../components/console/RecordingCropPanel.vue'")
    // 保存与退出编辑
    expect(src).toContain("if (recording.busy) return toast(recording.phase === 'stopping'")
    expect(src).toContain("if (recording.busy) return toast('录制未完成：请先停止录制，并重试或丢弃未完成草稿', 'warn')")
    // 跳转双向 + 分区锁定 + 关页
    expect(src).toContain("if (recording.busy) return toast('录制中不能切换资源', 'warn')")
    expect(src).toContain('录制中不能切换应用分区')
    expect(src).toContain('function onBeforeUnload(e)')
    expect(src).toContain('if (recording.busy || (scriptMode.value === \'edit\' && scriptShell.hasModel && scriptShell.dirty))')
    // 投屏链路丢失 → 取消手势并停止
    expect(src).toContain('recording.onLinkLost()')
    // Alt 录制中走 onAltAdd 保序（buildOnly 构建纯步骤）
    expect(src).toContain('recording.altAdd(')
  })

  it('shell 扩展与 ScriptRunner 画布锁到位', () => {
    const shellSrc = read('./composables/useScriptEditorShell.js')
    expect(shellSrc).toContain('function insertStepWithAnchor(')
    expect(shellSrc).toContain('function replaceStepFields(')
    const runner = read('./components/console/ScriptRunner.vue')
    expect(runner).toContain('ctx.recording.uploading')
    expect(runner).toContain('class="canvas-lock"')
    expect(runner).toContain('ctx.altHint')
  })
})

describe('composeRegionName（api.js 模板上传封装）', () => {
  it('短名 + 相对区域 → #×1000 三位整数后缀；越界钳取；无区域回退普通名', () => {
    expect(composeRegionName('record_click_20260829_001', [0.1, 0.2, 0.3, 0.4]))
      .toBe('record_click_20260829_001#100_200_300_400.png')
    expect(composeRegionName('a.png', [0, 0, 1, 1])).toBe('a#000_000_999_999.png')
    expect(composeRegionName('b.png')).toBe('b.png')
    expect(composeRegionName('c.png', null)).toBe('c.png')
  })

  it('api.uploadTemplateRegion 走 POST /api/templates（完整名服务端灰度重编码）', () => {
    const src = read('./api.js')
    expect(src).toContain('uploadTemplateRegion:')
    expect(src).toContain('composeRegionName(shortName, region)')
  })
})

describe('指针时序与手势分类（真实 recording/service）', () => {
  it('DOWN：先冻结帧再透传 down（不等待编码/上传）；坐标按帧原始尺寸换算', async () => {
    const h = await makeHarness()
    const stepsBefore = h.shell.model.steps.length
    h.rec.onPointerDown(pt(0.5, 0.25))
    expect(h.freezeCalls).toHaveLength(1)
    const downIdx = h.controlMsgs.findIndex(m => m.action === 'down')
    expect(downIdx).toBeGreaterThanOrEqual(0)
    expect(h.controlMsgs[downIdx]).toEqual({ type: 'touch', action: 'down', x: 960, y: 270 })
    // DOWN 阶段还没有占位插入（抬起才分类并插入）
    expect(h.shell.model.steps).toHaveLength(stepsBefore)
    expect(h.rec.available).toBe(true)
    h.rec.stop()
  })

  it('点击 → find 占位；A=50×50 中心矩形，S=100×100；busy 因待处理草稿置位', async () => {
    const h = await makeHarness()
    h.rec.onPointerDown(pt(0.5, 0.5))
    h.rec.onPointerUp(pt(0.5, 0.5))
    const steps = h.shell.model.steps
    const last = steps[steps.length - 1]
    expect(last.kind).toBe('find')
    expect(last.template).toEqual({ lit: '' })
    expect(h.rec.timeline).toHaveLength(1)
    const view = h.rec.timeline[0]
    expect(view.state).toBe('pending')
    expect(view.draft.aRect).toEqual({ x: 935, y: 515, w: 50, h: 50 })
    expect(view.draft.searchRect).toEqual({ x: 910, y: 490, w: 100, h: 100 })
    expect(view.draft.shortName).toMatch(/^record_click_\d{8}_001\.png$/)
    expect(h.rec.busy).toBe(true)
    expect(h.rec.pendingCount).toBe(1)
    h.rec.stop()
  })

  it('滑动 → match 占位（relEnd/durationMs 入草稿）；长按 → 失败草稿不静默转点击', async () => {
    const h = await makeHarness()
    h.rec.onPointerDown(pt(0.5, 0.8))
    h.rec.onPointerMove(pt(0.5, 0.2))
    h.advanceTime(800)
    h.rec.onPointerUp(pt(0.5, 0.2))
    let view = h.rec.timeline[0]
    expect(view.kind).toBe('swipe')
    expect(view.draft.relEnd).toEqual([0.5, 0.2])
    expect(view.draft.durationMs).toBe(800)
    // 滑动过程 move 已透传
    expect(h.controlMsgs.some(m => m.action === 'move')).toBe(true)

    // 长按：位移在阈值内但 > 600ms → 失败草稿
    h.rec.onPointerDown(pt(0.3, 0.3))
    h.advanceTime(1000)
    h.rec.onPointerUp(pt(0.3, 0.3))
    view = h.rec.timeline[1]
    expect(view.state).toBe('failed')
    expect(view.failure).toContain('长按')
    expect(view.kind).toBe('click')
    h.rec.stop()
  })
})

describe('上传定稿 / 失败重试 / 坐标降级 / 丢弃', () => {
  it('确认裁切 → 上传（相对区域参数）→ find 定稿（一次事务替换占位）', async () => {
    const h = await makeHarness()
    h.rec.onPointerDown(pt(0.5, 0.5))
    h.rec.onPointerUp(pt(0.5, 0.5))
    const view = h.rec.panelDraft
    const cropRects = []
    h.rec2cropRects = cropRects
    const ok = h.rec.confirmCrop(view, { name: 'login_btn.png', rect: view.draft.aRect, adjusted: false })
    expect(ok).toBe(true)
    await flush()
    expect(h.api.uploadTemplateRegion).toHaveBeenCalledTimes(1)
    const [name, b64, pkg, region] = h.api.uploadTemplateRegion.mock.calls[0]
    expect(name).toBe('login_btn.png')
    expect(b64).toBe('Zml4ZWQ=')
    expect(pkg).toBe('com.demo')
    // S = 以 A 为中心的 100×100 → 相对坐标
    expect(region[0]).toBeCloseTo(910 / 1920, 6)
    expect(region[3]).toBeCloseTo(590 / 1080, 6)
    const steps = h.shell.model.steps
    expect(steps[steps.length - 1].kind).toBe('find')
    expect(steps[steps.length - 1].template).toEqual({ lit: 'login_btn.png' })
    expect(h.rec.timeline[0].state).toBe('ready')
    // 录制仍在进行（busy），但队列已无待处理；停止后 busy 解除
    expect(h.rec.pendingCount).toBe(0)
    expect(h.rec.busy).toBe(true)
    await h.rec.stop()
    expect(h.rec.busy).toBe(false)
    expect(serialize(h.shell.model)).toContain('- find: login_btn.png')
  })

  it('滑动定稿 → match 候选[短名]{swipe fm/to/time} + else throw + timeout 30s', async () => {
    const h = await makeHarness()
    h.rec.onPointerDown(pt(0.5, 0.8))
    h.rec.onPointerMove(pt(0.5, 0.2))
    h.advanceTime(800)
    h.rec.onPointerUp(pt(0.5, 0.2))
    const view = h.rec.panelDraft
    h.rec.confirmCrop(view, { name: 'swipe_origin.png', rect: view.draft.aRect, adjusted: false })
    await flush()
    const yaml = serialize(h.shell.model)
    expect(yaml).toContain('swipe_origin.png')
    expect(yaml).toContain('swipe:')
    expect(yaml).toContain('fm: [0.5, 0.8]')
    expect(yaml).toContain('to: [0.5, 0.2]')
    expect(yaml).toContain('time: 800ms')
    expect(yaml).toContain('throw: 未找到滑动起点')
    expect(yaml).toContain('timeout: 30s')
    // 绝不能生成 find → swipe（find 命中会点击模板中心）
    expect(yaml).not.toContain('- find: swipe_origin.png')
  })

  it('短名校验：非法名与重名要求改名，不覆盖', async () => {
    const h = await makeHarness({
      templates: [
        { name: 'dup.png', pkg: 'com.demo' },
        { name: 'dup#100_100_300_300.png', pkg: 'com.demo' },
        { name: 'dup_001#000_000_100_100.png', pkg: 'com.demo' },
      ],
    })
    h.rec.onPointerDown(pt(0.5, 0.5))
    h.rec.onPointerUp(pt(0.5, 0.5))
    const view = h.rec.panelDraft
    expect(h.rec.confirmCrop(view, { name: 'bad name!.png', rect: view.draft.aRect, adjusted: false })).toBe(false)
    expect(h.rec.confirmCrop(view, { name: 'dup.png', rect: view.draft.aRect, adjusted: false })).toBe(false)
    expect(h.api.uploadTemplateRegion).not.toHaveBeenCalled()
    expect(h.notifications.some(n => n.type === 'warn' && n.msg.includes('已存在'))).toBe(true)
    // 改名后通过；短名与带 # 后缀文件都算冲突（shortNameTaken）
    expect(h.rec.shortNameTaken('dup.png')).toBe(true)
    expect(h.rec.shortNameTaken('other.png')).toBe(false)
    // 默认短名冲突时按同前缀顺延序号（dup_001.png 已被占用 → _002）
    expect(h.rec.nextShortName('click', 'dup_001.png')).toBe('dup_002.png')
    expect(h.rec.nextShortName('click', 'fresh.png')).toBe('fresh.png')
    h.rec.discard(view)
  })

  it('上传失败保留草稿可重试；降级为 tap；丢弃移除占位', async () => {
    let fail = true
    const h = await makeHarness({ cropImpl: () => fail ? Promise.reject(new Error('网络断开')) : Promise.resolve('Zml4ZWQ=') })
    h.rec.onPointerDown(pt(0.25, 0.75))
    h.rec.onPointerUp(pt(0.25, 0.75))
    let view = h.rec.panelDraft
    h.rec.confirmCrop(view, { name: 'retry_case.png', rect: view.draft.aRect, adjusted: false })
    await flush()
    view = h.rec.timeline[0]
    expect(view.state).toBe('failed')
    expect(view.failure).toContain('网络断开')
    expect(h.rec.busy).toBe(true)
    // 重试成功（首次上传在 cropPng 即失败，未到达 api；重试 api 恰好一次）
    fail = false
    expect(h.rec.retry(view)).toBe(true)
    await flush()
    expect(h.rec.timeline[0].state).toBe('ready')
    expect(h.api.uploadTemplateRegion).toHaveBeenCalledTimes(1)

    // 降级：新草稿 → tap（占位整体替换，坐标取按下点）
    h.rec.onPointerDown(pt(0.1, 0.9))
    h.rec.onPointerUp(pt(0.1, 0.9))
    const v2 = h.rec.panelDraft
    expect(h.rec.downgrade(v2)).toBe(true)
    const steps = h.shell.model.steps
    const tap = steps[steps.length - 1]
    expect(tap.kind).toBe('tap')
    expect(tap.at).toEqual({ lit: [0.1, 0.9] })
    expect(h.rec.timeline[1].state).toBe('ready')

    // 丢弃：占位一并移除
    h.rec.onPointerDown(pt(0.5, 0.5))
    h.rec.onPointerUp(pt(0.5, 0.5))
    const v3 = h.rec.panelDraft
    const before = h.shell.model.steps.length
    expect(h.rec.discard(v3)).toBe(true)
    expect(h.shell.model.steps).toHaveLength(before - 1)
    expect(h.rec.timeline[2].state).toBe('discarded')
  })
})

describe('停止排空与保存解锁（plan §11.3）', () => {
  it('停止时待裁切草稿按 A+自动搜索区定稿，排空后回到 idle、busy 解除', async () => {
    const h = await makeHarness()
    h.rec.onPointerDown(pt(0.5, 0.5))
    h.rec.onPointerUp(pt(0.5, 0.5))
    expect(h.rec.pendingCount).toBe(1)
    const done = h.rec.stop()
    await flush()
    await done
    expect(h.rec.phase).toBe('idle')
    expect(h.rec.busy).toBe(false)
    // 自动定稿用了控制器建议的默认短名
    const uploaded = h.api.uploadTemplateRegion.mock.calls[0]
    expect(uploaded[0]).toMatch(/^record_click_\d{8}_001\.png$/)
    const steps = h.shell.model.steps
    expect(steps[steps.length - 1].template).toEqual({ lit: uploaded[0] })
  })

  it('失败条目阻塞排空：停止中保持 stopping，用户处理后解锁', async () => {
    let fail = true
    const h = await makeHarness({ cropImpl: () => fail ? Promise.reject(new Error('x')) : Promise.resolve('Zml4ZWQ=') })
    h.rec.onPointerDown(pt(0.5, 0.5))
    h.rec.onPointerUp(pt(0.5, 0.5))
    h.rec.confirmCrop(h.rec.panelDraft, { name: 'blocked.png', rect: h.rec.panelDraft.draft.aRect, adjusted: false })
    await flush()
    expect(h.rec.timeline[0].state).toBe('failed')
    const done = h.rec.stop()
    await flush()
    expect(h.rec.phase).toBe('stopping')
    expect(h.rec.busy).toBe(true)
    // 用户重试成功 → 排空 → idle
    fail = false
    expect(h.rec.retry(h.rec.timeline[0])).toBe(true)
    await flush()
    await done
    expect(h.rec.phase).toBe('idle')
    expect(h.rec.busy).toBe(false)
  })
})

describe('多指透传与取消清理（真实 window 事件）', () => {
  function fire(type, init) {
    const ev = new window.Event(type)
    Object.assign(ev, init)
    window.dispatchEvent(ev)
    return ev
  }

  it('第二只指针只透传并提示一次；pointercancel 取消当前手势生成失败草稿', async () => {
    const h = await makeHarness()
    // 主指针开始一个手势
    h.rec.onPointerDown(pt(0.5, 0.5))
    expect(h.rec.active).toBe(true)
    // 第二只指针（非 primary）→ 透传 down + 提示一次
    fire('pointerdown', { isPrimary: false, pointerId: 7, clientX: 100, clientY: 100 })
    fire('pointerdown', { isPrimary: false, pointerId: 8, clientX: 120, clientY: 120 })
    const passthroughDowns = h.controlMsgs.filter(m => m.action === 'down')
    expect(passthroughDowns).toHaveLength(3) // 主指针 1 + 透传 2
    const warns = h.notifications.filter(n => n.msg.includes('不支持多指录制'))
    expect(warns).toHaveLength(1)
    // 第二只指针抬起
    fire('pointerup', { isPrimary: false, pointerId: 7, clientX: 110, clientY: 110 })
    expect(h.controlMsgs.some(m => m.action === 'up')).toBe(true)
    // 主指针 pointercancel → 手势取消：补发 UP + 失败草稿（不悄悄漏步）
    fire('pointercancel', { isPrimary: true, pointerId: 1 })
    await flush()
    const view = h.rec.timeline[0]
    expect(view.state).toBe('failed')
    expect(view.failure).toContain('取消')
    expect(h.shell.model.steps.at(-1).kind).toBe('find')
    expect(h.controlMsgs.at(-1).action).toBe('up')
  })
})

describe('录制插入目标锁定与 Alt 保序（plan §11.8）', () => {
  it('状态栏显示锚点路径；多次插入按序推进；Alt 插入同锚点保序且不进上传流程', async () => {
    const h = await makeHarness({ start: false })
    h.shell.select(h.shell.model.steps[1].uuid)
    expect(h.rec.start()).toBe(true)
    expect(h.rec.targetLabel).toBe('主流程 · 第 2 步之后')
    // 两次录制手势：依次插入选中步骤之后
    h.rec.onPointerDown(pt(0.5, 0.5))
    h.rec.onPointerUp(pt(0.5, 0.5))
    h.rec.onPointerDown(pt(0.2, 0.2))
    h.rec.onPointerUp(pt(0.2, 0.2))
    expect(h.shell.model.steps).toHaveLength(4)
    expect(h.shell.model.steps[2].kind).toBe('find')
    expect(h.shell.model.steps[3].kind).toBe('find')
    // Alt 模板添加：同锚点保序（第 5 位），登记为 alt 条目但不产生待处理
    expect(h.rec.altAdd(h.rec.buildFindStep('alt_tpl.png'), '等待并点击 alt_tpl.png')).toBe(true)
    expect(h.shell.model.steps[4].template).toEqual({ lit: 'alt_tpl.png' })
    expect(h.rec.pendingCount).toBe(2) // 仅两次录制手势待处理
    const altEntries = h.rec.timeline.filter(e => e.kind === 'alt')
    expect(altEntries).toHaveLength(1)
    expect(altEntries[0].state).toBe('ready')
  })

  it('插入目标不可用时队列仍登记（哨兵 uuid），不悄悄漏步', async () => {
    const h = await makeHarness({ start: false })
    expect(h.rec.start()).toBe(true)
    // 退出编辑态（模型卸载）后手势仍透传，占位插入失败但队列登记失败草稿
    h.shell.reset()
    h.rec.onPointerDown(pt(0.5, 0.5))
    h.rec.onPointerUp(pt(0.5, 0.5))
    expect(h.rec.timeline).toHaveLength(1)
    expect(h.notifications.some(n => n.msg.includes('插入失败'))).toBe(true)
  })
})
