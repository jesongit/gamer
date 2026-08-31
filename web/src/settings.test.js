// @vitest-environment happy-dom
// WEB-005 Settings 页测试：静态原型与「已保存（原型）」假交互移除后，页面挂载
// SystemInfoCard / UpdateStatusCard / UpdateConfirmModal 三组件，全部对接 system/update 真实 API。
// 响应 fixture 读自 release/contracts/fixtures/system-api/（与 system-components.test.js 同源）。
// 覆盖：三卡片挂载、策略保存走真实 PUT（含客户端校验与 400 回显）、能力全 false 降级、
// 安装确认完整流（202 → 断连容忍 → 重连按版本/boot_id 判定）、§4.2 矩阵门禁、卸载停轮询。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { flushPromises, mount } from '@vue/test-utils'
import Settings from './views/Settings.vue'

const HERE = dirname(fileURLToPath(import.meta.url))
const FIX_DIR = resolve(HERE, '../../release/contracts/fixtures/system-api')

/** 读取契约 fixture 的 response 段（{status, body}） */
const fix = (name) => JSON.parse(readFileSync(resolve(FIX_DIR, name), 'utf8')).response

const INFO = fix('system-info.success.json').body // launcher 模式，capabilities 全 true
const UPD = fix('system-update.success.json').body // state=staged，policy notify 02:00–06:00/30

/** direct 模式降级：capabilities 全 false（契约：direct→unsupported，安装类按钮禁用） */
const INFO_DIRECT = {
  ...INFO,
  deployment: { mode: 'direct', update_strategy: 'unsupported' },
  capabilities: { check: false, download: false, install: false, rollback: false },
}

const res = (status, body) => ({
  ok: status >= 200 && status < 300,
  status,
  headers: { get: (h) => (/^content-type$/i.test(h) ? 'application/json' : null) },
  json: async () => body,
})

let fetchMock

/** 按 "METHOD /path" 分发；handler 接收 fetch 的 opt（可读 body），未命中一律 404 */
function installFetchMock(handlers) {
  fetchMock.mockImplementation(async (url, opt = {}) => {
    const h = handlers[`${(opt.method || 'GET').toUpperCase()} ${url}`]
    if (h) return h(opt)
    return res(404, { error: 'not_found' })
  })
}

function mountSettings() {
  return mount(Settings)
}

/** 挂载并等首轮 info+update 加载完成（fake timers 下轮询挂起在 30s 慢周期上） */
async function mountSettled(handlers) {
  installFetchMock(handlers)
  const w = mountSettings()
  await vi.advanceTimersByTimeAsync(1)
  await flushPromises()
  return w
}

beforeEach(() => {
  fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  document.body.innerHTML = '' // 清掉 useToast 挂到 body 的 toast
})

describe('Settings：三卡片挂载（WEB-005）', () => {
  it('加载期显示读取状态，不出现任何原型假交互文案', () => {
    vi.useFakeTimers()
    fetchMock.mockImplementation(() => new Promise(() => {})) // 挂起
    const w = mountSettings()

    expect(w.get('[role="status"]').text()).toContain('正在读取系统状态')
    expect(w.text()).not.toContain('设置已保存')
    expect(w.text()).not.toContain('（原型）')
    w.unmount()
  })

  it('挂载 SystemInfoCard / UpdateStatusCard / 策略卡，策略表单按服务端 policy 回填', async () => {
    vi.useFakeTimers()
    const w = await mountSettled({
      'GET /api/system/info': () => res(200, INFO),
      'GET /api/system/update': () => res(200, UPD),
    })

    // SystemInfoCard：版本与部署模式来自 /api/system/info
    expect(w.text()).toContain('GameBot 0.2.0')
    expect(w.text()).toContain('便携托管（launcher）')
    // UpdateStatusCard：staged 状态标签
    expect(w.get('[data-testid="state-tag"]').text()).toBe('已就绪待安装')
    // 策略卡：表单回填 notify / 02:00 / 06:00 / 30，未保存前无「已保存」note
    expect(w.get('[data-testid="policy-card"]').exists()).toBe(true)
    expect(w.find('input[value="notify"]').element.checked).toBe(true)
    expect(w.get('[data-testid="window-start"]').element.value).toBe('02:00')
    expect(w.get('[data-testid="window-end"]').element.value).toBe('06:00')
    expect(w.get('[data-testid="freeze-window"]').element.value).toBe('30')
    expect(w.find('[data-testid="policy-note"]').exists()).toBe(false)
    w.unmount()
  })

  it('选 auto 显示维护窗口与门禁说明；保存走真实 PUT（整对象替换）并回显已保存', async () => {
    vi.useFakeTimers()
    let lastPut = null
    const w = await mountSettled({
      'GET /api/system/info': () => res(200, INFO),
      'GET /api/system/update': () => res(200, UPD),
      'PUT /api/system/update/policy': (opt) => {
        lastPut = { body: JSON.parse(opt.body), headers: opt.headers }
        return res(200, JSON.parse(opt.body)) // 契约 §6：200 回显保存后的策略
      },
    })

    // off/notify 不强调窗口；切到 auto 后门禁说明出现
    expect(w.find('[data-testid="gate-note"]').exists()).toBe(false)
    await w.find('input[value="auto"]').setValue(true)
    expect(w.get('[data-testid="gate-note"]').text()).toContain('门禁')
    expect(w.get('[data-testid="gate-note"]').text()).toContain('维护窗口')

    await w.get('[data-testid="freeze-window"]').setValue('45')
    await w.get('[data-testid="policy-save"]').trigger('click')
    await flushPromises()

    expect(lastPut.body).toEqual({
      strategy: 'auto',
      maintenance_window: { start: '02:00', end: '06:00' },
      freeze_window_minutes: 45,
    })
    expect(lastPut.headers['Content-Type']).toBe('application/json')
    expect(w.get('[data-testid="policy-note"]').text()).toBe('已保存')

    // 允许跨午夜窗口（契约 §6：23:00–05:00 合法），二次保存整对象替换
    await w.get('[data-testid="window-start"]').setValue('23:00')
    await w.get('[data-testid="window-end"]').setValue('05:00')
    await w.get('[data-testid="policy-save"]').trigger('click')
    await flushPromises()
    expect(lastPut.body.maintenance_window).toEqual({ start: '23:00', end: '05:00' })
    w.unmount()
  })

  it('客户端校验拦截非法窗口不发请求；服务端 400 invalid_argument 回显错误', async () => {
    vi.useFakeTimers()
    let putCalls = 0
    const w = await mountSettled({
      'GET /api/system/info': () => res(200, INFO),
      'GET /api/system/update': () => res(200, UPD),
      'PUT /api/system/update/policy': () => {
        putCalls++
        return res(400, { code: 'invalid_argument', message: '维护窗口非法', details: { field: 'maintenance_window' } })
      },
    })

    // start == end 视为非法：客户端直接拦截，不发 PUT
    await w.get('[data-testid="window-end"]').setValue('02:00')
    await w.get('[data-testid="policy-save"]').trigger('click')
    await flushPromises()
    expect(putCalls).toBe(0)
    expect(w.get('[data-testid="policy-error"]').text()).toContain('不能相同')

    // 窗口合法但服务端 400：错误 message 回显，无「已保存」
    await w.get('[data-testid="window-end"]').setValue('06:00')
    await w.get('[data-testid="policy-save"]').trigger('click')
    await flushPromises()
    expect(putCalls).toBe(1)
    expect(w.get('[data-testid="policy-error"]').text()).toContain('维护窗口非法')
    expect(w.find('[data-testid="policy-note"]').exists()).toBe(false)
    w.unmount()
  })

  it('更新接口不可用（后端未升级 404）：info 正常展示，更新卡空态、策略卡给出重试', async () => {
    vi.useFakeTimers()
    const w = await mountSettled({
      'GET /api/system/info': () => res(200, INFO),
      'GET /api/system/update': () => res(404, { error: 'not_found' }),
    })

    expect(w.text()).toContain('GameBot 0.2.0') // info 卡不受影响
    expect(w.text()).toContain('暂无更新状态')
    expect(w.get('[data-testid="policy-card"]').text()).toContain('暂不可用')
    expect(w.get('[data-testid="policy-card"]').text()).toContain('not_found')
    expect(w.findAll('button').some((b) => b.text() === '重试')).toBe(true)
    w.unmount()
  })
})

describe('Settings：能力降级与动作流', () => {
  it('capabilities 全 false（direct）：动作按钮全禁用 + update_not_managed 说明，策略仍可保存', async () => {
    vi.useFakeTimers()
    let lastPut = null
    const w = await mountSettled({
      'GET /api/system/info': () => res(200, INFO_DIRECT),
      'GET /api/system/update': () => res(200, UPD),
      'PUT /api/system/update/policy': (opt) => {
        lastPut = JSON.parse(opt.body)
        return res(200, JSON.parse(opt.body))
      },
    })

    for (const a of ['check', 'download', 'install', 'rollback']) {
      expect(w.find(`[data-action="${a}"]`).attributes('disabled')).toBeDefined()
    }
    expect(w.text()).toContain('update_not_managed')
    // 策略保存不受能力降级影响（契约 §6：docker/direct 允许保存策略）
    expect(w.get('[data-testid="policy-save"]').attributes('disabled')).toBeUndefined()
    await w.get('[data-testid="policy-save"]').trigger('click')
    await flushPromises()
    expect(lastPut.strategy).toBe('notify')
    w.unmount()
  })

  it('check 被同步拒绝（launcher_unreachable 502）→ toast 显示契约错误文案', async () => {
    vi.useFakeTimers()
    const w = await mountSettled({
      'GET /api/system/info': () => res(200, INFO),
      'GET /api/system/update': () => res(200, UPD),
      'POST /api/system/update/check': () => res(502, { code: 'launcher_unreachable', message: 'ipc down', details: null }),
    })

    await w.get('[data-action="check"]').trigger('click')
    await flushPromises()
    expect(document.querySelector('.toast-wrap').textContent).toContain('无法连接升级器')
    expect(document.querySelector('.toast-wrap').textContent).toContain('launcher_unreachable')
    w.unmount()
  })

  it('§4.2 矩阵门禁：idle 态点「安装更新」不开确认弹窗，提示先检查更新', async () => {
    vi.useFakeTimers()
    const idle = { ...UPD, state: 'idle', detail: 'idle', candidate: null, progress: null }
    const w = await mountSettled({
      'GET /api/system/info': () => res(200, INFO),
      'GET /api/system/update': () => res(200, idle),
    })

    await w.findAll('button').find((b) => b.text() === '安装更新').trigger('click')
    expect(w.find('.modal-mask').exists()).toBe(false)
    expect(document.querySelector('.toast-wrap').textContent).toContain('请先检查更新')
    w.unmount()
  })

  it('安装完整流：确认弹窗 → 202 受理 → 断连容忍不误报失败 → 重连按新版本/boot_id 判定成功', async () => {
    vi.useFakeTimers()
    let down = false
    const INFO_AFTER = {
      ...INFO,
      app: { ...INFO.app, version: '0.3.0' },
      startup: { stage: 'ready', boot_id: '9c27c1af-0000-4000-8000-000000000001' },
    }
    const UPD_AFTER = { ...UPD, state: 'idle', detail: 'committed', candidate: null, progress: null }
    let infoBody = INFO
    let updBody = UPD
    let installCalls = 0
    const w = await mountSettled({
      'GET /api/system/info': () => (down ? Promise.reject(new TypeError('network down')) : res(200, infoBody)),
      'GET /api/system/update': () => (down ? Promise.reject(new TypeError('network down')) : res(200, updBody)),
      'POST /api/system/update/install': () => {
        installCalls++
        down = true // 安装协调器接管：服务即将重启，连接断开（断连是正常路径）
        return res(202, { update_id: 'upd-t1', state: 'installing' })
      },
    })

    // staged → install 在受理矩阵内，点击弹确认
    await w.get('[data-action="install"]').trigger('click')
    expect(w.get('.modal-mask').text()).toContain('安装更新确认')

    // 确认 → 202 受理 → 等待期弹窗保持打开，不显示任何失败
    await w.get('[data-testid="confirm-btn"]').trigger('click')
    await flushPromises()
    expect(installCalls).toBe(1)
    expect(w.get('[data-testid="confirm-btn"]').text()).toBe('提交中…')
    expect(w.find('.modal-mask .err-box').exists()).toBe(false)

    // 断连轮询：两轮失败仍等待（有界重连，不判失败）
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()
    expect(w.find('.modal-mask').exists()).toBe(true)

    // 服务恢复：新版本 + 新 boot_id + idle/committed → 判定成功，弹窗关闭并刷新
    down = false
    infoBody = INFO_AFTER
    updBody = UPD_AFTER
    await vi.advanceTimersByTimeAsync(2000)
    await flushPromises()

    expect(w.find('.modal-mask').exists()).toBe(false)
    expect(document.querySelector('.toast-wrap').textContent).toContain('安装完成')
    expect(w.get('[data-testid="state-tag"]').text()).toBe('空闲') // 刷新后新状态上屏
    w.unmount()
  })

  it('轮询纪律：页面卸载后不再发起任何请求（useSystemStatus 接线不被破坏）', async () => {
    vi.useFakeTimers()
    const w = await mountSettled({
      'GET /api/system/info': () => res(200, INFO),
      'GET /api/system/update': () => res(200, UPD),
    })

    w.unmount()
    const calls = fetchMock.mock.calls.length
    await vi.advanceTimersByTimeAsync(120000)
    expect(fetchMock.mock.calls.length).toBe(calls)
  })
})
