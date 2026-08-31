// @vitest-environment happy-dom
// WEB-002/003/004 组件测试：SystemInfoCard（正常/降级/dev 构建/依赖损坏）、
// UpdateStatusCard（11 状态全覆盖渲染 + §4.2 动作可用性 + 轮询接线）、
// UpdateConfirmModal（安装/回滚确认、blocking 门禁、错误回显）。
// 响应 fixture 读自 release/contracts/fixtures/system-api/；无 fixture 覆盖的状态用内联最小
// status（字段结构与 system-update.success.json 一致，detail 值取自契约 §5.2 冻结映射）。
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { mount, flushPromises } from '@vue/test-utils'

import SystemInfoCard from './components/SystemInfoCard.vue'
import UpdateStatusCard from './components/UpdateStatusCard.vue'
import UpdateConfirmModal from './components/UpdateConfirmModal.vue'
import { useSystemStatus } from './system/useSystemStatus'

// fixture 目录用 node:path 解析（happy-dom 环境下全局 URL 与 node:fs 配合不可靠，避免 new URL 路径）
const HERE = dirname(fileURLToPath(import.meta.url))
const FIX_DIR = resolve(HERE, '../../release/contracts/fixtures/system-api')

/** 读取契约 fixture 的 response 段（{status, body}）；字段结构见 release/contracts/system-api-v1.md §9 */
const fix = (name) => JSON.parse(readFileSync(resolve(FIX_DIR, name), 'utf8')).response

const res = (status, body) => ({
  ok: status >= 200 && status < 300,
  status,
  headers: { get: (h) => (/^content-type$/i.test(h) ? 'application/json' : null) },
  json: async () => body,
})

const INFO = fix('system-info.success.json').body
const INFO_DOCKER = fix('system-info.degraded-docker.json').body
const UPD_STAGED = fix('system-update.success.json').body
const UPD_FAILED = fix('system-update.failed-signature-invalid.json').body
const UPD_MANUAL = fix('system-update.manual-recovery.json').body

/** 内联最小 update status（契约 §3 字段；detail 取 §5.2 冻结映射） */
function upd(state, extra = {}) {
  return {
    state,
    detail: { idle: 'idle', checking: 'checking', available: 'checked', downloading: 'downloading', staged: 'staged', waiting: 'waiting_idle', installing: 'draining', restarting: 'candidate_starting', failed: 'failed', rolling_back: 'rolling_back', manual_recovery: 'manual_recovery_required' }[state],
    update_id: 'upd-20260831-9f3ab2c1',
    candidate: null,
    progress: null,
    policy: { strategy: 'notify', maintenance_window: { start: '02:00', end: '06:00' }, freeze_window_minutes: 30 },
    last_error: null,
    updated_at: '2026-08-31T12:00:00Z',
    ...extra,
  }
}

const ACTIONS = ['check', 'download', 'install', 'rollback']

/** §4.2 状态×动作受理矩阵（与 system-api.test.js 的矩阵断言互为印证：组件层禁用态） */
const MATRIX = {
  idle:            { check: true,  download: false, install: false, rollback: false },
  checking:        { check: true,  download: false, install: false, rollback: false },
  available:       { check: true,  download: true,  install: false, rollback: false },
  downloading:     { check: true,  download: true,  install: false, rollback: false },
  staged:          { check: true,  download: true,  install: true,  rollback: true },
  waiting:         { check: true,  download: true,  install: true,  rollback: true },
  installing:      { check: false, download: false, install: false, rollback: false },
  restarting:      { check: false, download: false, install: false, rollback: false },
  failed:          { check: true,  download: true,  install: true,  rollback: true },
  rolling_back:    { check: false, download: false, install: false, rollback: false },
  manual_recovery: { check: false, download: false, install: false, rollback: false },
}

const STATE_FIXTURES = {
  staged: UPD_STAGED,
  failed: UPD_FAILED,
  manual_recovery: UPD_MANUAL,
}

function mountStatus(status, props = {}) {
  return mount(UpdateStatusCard, { props: { status, ...props } })
}

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn())
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('SystemInfoCard（WEB-002）', () => {
  it('launcher 正常态：版本/commit/构建时间/target、部署与策略、schema、启动阶段、依赖三行', () => {
    const w = mount(SystemInfoCard, { props: { info: INFO } })
    const t = w.text()
    expect(t).toContain('0.2.0')
    expect(t).toContain('01234567') // commit 截短展示
    expect(t).toContain('x86_64-pc-windows-msvc')
    expect(t).toContain('便携托管（launcher）')
    expect(t).toContain('升级器托管')
    expect(t).toContain('数据库 schema v1')
    expect(t).toContain('文件布局 schema v1')
    expect(t).toContain('自动回滚下限 v1')
    expect(t).toContain('boot 3f2c9a58')
    // 依赖三行：状态/版本/来源/绑定
    expect(t).toContain('adb')
    expect(t).toContain('34.0.5')
    expect(t).toContain('托管')
    expect(t).toContain('运行时组件')
    expect(t).toContain('随应用分发') // scrcpy 绑定
    // 正常构建不显示「开发构建」，能力不全 false 无 update_not_managed 说明
    expect(t).not.toContain('开发构建')
    expect(t).not.toContain('update_not_managed')
    expect(w.findAll('.dep-table tbody tr')).toHaveLength(3)
  })

  it('Docker 降级态（degraded-docker fixture）：能力全禁用 + update_not_managed 说明 + external 策略', async () => {
    const w = mount(SystemInfoCard, { props: { info: INFO_DOCKER } })
    const t = w.text()
    expect(t).toContain('容器（Docker）')
    expect(t).toContain('外部管理')
    expect(t).toContain('update_not_managed')
    // 检查/安装按钮禁用
    const buttons = w.findAll('button').map((b) => ({ text: b.text(), disabled: b.attributes('disabled') !== undefined }))
    expect(buttons.find((b) => b.text === '检查更新').disabled).toBe(true)
    expect(buttons.find((b) => b.text === '安装更新').disabled).toBe(true)
  })

  it('依赖缺失/损坏行有警示视觉态；版本不可得显示占位', () => {
    const broken = {
      ...INFO,
      dependencies: {
        adb: { status: 'broken', version: null, source: 'custom', binding: 'external' },
        ffmpeg: { status: 'missing', version: null, source: 'system', binding: 'external' },
        scrcpy: { status: 'ready', version: '3.3.3', source: 'managed', binding: 'application' },
      },
    }
    const w = mount(SystemInfoCard, { props: { info: broken } })
    const rows = w.findAll('.dep-table tbody tr')
    expect(rows[0].classes()).toContain('degraded')
    expect(rows[0].text()).toContain('损坏')
    expect(rows[1].classes()).toContain('degraded')
    expect(rows[1].text()).toContain('缺失')
    expect(rows[2].classes()).not.toContain('degraded')
    expect(rows[0].text()).toContain('自定义')
    expect(w.text()).toContain('—') // version null 占位
  })

  it('dev/unknown 构建信息：显式「开发构建」标记，unknown 值如实显示不伪装', () => {
    const dev = { ...INFO, app: { version: '0.2.0-dev', commit: 'unknown', built_at: 'unknown', channel: 'dev', target: 'x86_64-pc-windows-msvc' } }
    const w = mount(SystemInfoCard, { props: { info: dev } })
    expect(w.text()).toContain('开发构建')
    expect(w.text()).toContain('unknown（未注入构建信息）')
    expect(w.text()).toContain('未知（开发构建）')
    expect(w.text()).toContain('0.2.0-dev')
  })

  it('info 缺失：空态/错误文案，不渲染依赖表', () => {
    const w = mount(SystemInfoCard, { props: {} })
    expect(w.find('.dep-table').exists()).toBe(false)
    expect(w.text()).toContain('暂无系统信息')
    const w2 = mount(SystemInfoCard, { props: { error: '网络请求失败' } })
    expect(w2.text()).toContain('系统信息加载失败：网络请求失败')
  })

  it('能力按钮点击上抛 check/install 事件', async () => {
    const w = mount(SystemInfoCard, { props: { info: INFO } })
    await w.findAll('button').find((b) => b.text() === '检查更新').trigger('click')
    await w.findAll('button').find((b) => b.text() === '安装更新').trigger('click')
    expect(w.emitted('check')).toHaveLength(1)
    expect(w.emitted('install')).toHaveLength(1)
  })
})

describe('UpdateStatusCard：11 状态全覆盖（WEB-003）', () => {
  // 逐状态：fixture 来源 / 状态标签文案 / 动作按钮禁用矩阵（null = 内联最小 status）
  const CASES = [
    ['idle',            '空闲',          null,       MATRIX.idle],
    ['checking',        '正在检查更新',   null,       MATRIX.checking],
    ['available',       '有可用更新',     null,       MATRIX.available],
    ['downloading',     '正在下载',       null,       MATRIX.downloading],
    ['staged',          '已就绪待安装',   UPD_STAGED, MATRIX.staged],
    ['waiting',         '等待维护窗口',   null,       MATRIX.waiting],
    ['installing',      '正在安装',       null,       MATRIX.installing],
    ['restarting',      '正在重启',       null,       MATRIX.restarting],
    ['failed',          '更新失败',       UPD_FAILED, MATRIX.failed],
    ['rolling_back',    '正在回滚',       null,       MATRIX.rolling_back],
    ['manual_recovery', '需要人工恢复',   UPD_MANUAL, MATRIX.manual_recovery],
  ]

  for (const [state, label, fixture, allowed] of CASES) {
    it(`状态 ${state}：渲染「${label}」+ 描述 + 按钮可用性 ${JSON.stringify(allowed)}${fixture ? '（契约 fixture）' : '（内联最小 status）'}`, () => {
      const status = fixture || upd(state)
      const w = mountStatus(status)
      expect(w.find('[data-testid="state-tag"]').text()).toBe(label)
      expect(w.find('.state-desc').text()).toBeTruthy() // 每个状态都有一句话描述
      // 动作按钮：可用性与 §4.2 矩阵一致
      for (const a of ACTIONS) {
        const btn = w.find(`[data-action="${a}"]`)
        expect(btn.exists()).toBe(true)
        expect(btn.attributes('disabled') !== undefined).toBe(!allowed[a])
      }
    })
  }

  it('installing/restarting 展示「服务即将重启/短暂不可达」警示描述', () => {
    expect(mountStatus(upd('installing')).text()).toContain('服务即将重启')
    expect(mountStatus(upd('restarting')).text()).toContain('短暂不可达')
  })

  it('available 态 detail=checked（§5.2 驻留值）展示「检查完成」', () => {
    expect(mountStatus(upd('available')).text()).toContain('检查完成')
  })

  it('downloading：进度条按 bytes_done/bytes_total 计算，字节人性化', () => {
    const w = mountStatus(upd('downloading', { progress: { bytes_done: 446825600, bytes_total: 893451200 } }))
    expect(w.text()).toContain('50%')
    expect(w.text()).toContain('426 MB') // 446825600 / 1048576 → round
    expect(w.text()).toContain('852 MB') // 893451200 / 1048576 → round
    expect(w.find('.prog-in').attributes('style')).toContain('width: 50%')
  })

  it('failed（fixture）：展示错误码 + 无泄露 message', () => {
    const w = mountStatus(UPD_FAILED)
    expect(w.text()).toContain('signature_invalid')
    expect(w.text()).toContain('发布清单验签失败')
  })

  it('manual_recovery（fixture）：恢复指引占位 + journal 摘要字段（事务/阶段/最后错误/状态时间）', () => {
    const w = mountStatus(UPD_MANUAL)
    expect(w.text()).toContain('请按维护手册执行人工恢复')
    expect(w.text()).toContain('upd-20260831-9f3ab2c1')
    expect(w.text()).toContain('manual_recovery_required')
    expect(w.text()).toContain('2026-08-31')
  })

  it('candidate 展示（staged fixture）：版本/channel/大小/发布说明链接', () => {
    const w = mountStatus(UPD_STAGED)
    expect(w.text()).toContain('可更新到 0.3.0')
    expect(w.text()).toContain('stable')
    expect(w.find('a').attributes('href')).toBe('https://example.invalid/releases/v0.3.0')
  })

  it('能力门禁：capabilities.install=false（Docker/direct）时按钮全禁用 + update_not_managed 说明；动作点击上抛', () => {
    const w = mountStatus(UPD_STAGED, { info: INFO_DOCKER })
    for (const a of ACTIONS) {
      expect(w.find(`[data-action="${a}"]`).attributes('disabled') !== undefined).toBe(true)
    }
    expect(w.text()).toContain('update_not_managed')
    // 正常能力下点击可用动作 → emit('action', name)
    const w2 = mountStatus(UPD_STAGED, { info: INFO })
    w2.find('[data-action="install"]').trigger('click')
    expect(w2.emitted('action')).toEqual([['install']])
    // 矩阵禁用的动作点击不上抛
    const w3 = mountStatus(upd('idle'), { info: INFO })
    w3.find('[data-action="install"]').trigger('click')
    expect(w3.emitted('action')).toBeUndefined()
  })

  it('busy 时全部按钮禁用；无 status 显示空态', () => {
    const w = mountStatus(UPD_STAGED, { busy: true })
    for (const a of ACTIONS) {
      expect(w.find(`[data-action="${a}"]`).attributes('disabled') !== undefined).toBe(true)
    }
    const w2 = mount(UpdateStatusCard, { props: {} })
    expect(w2.text()).toContain('暂无更新状态')
  })

  it('autoPoll：自持轮询活跃态高频，卸载后停止（组件卸载断轮询验收）', async () => {
    vi.useFakeTimers()
    global.fetch.mockImplementation(async (url) =>
      res(200, url === '/api/system/update' ? upd('downloading') : INFO))
    const w = mount(UpdateStatusCard, { props: { autoPoll: true } })
    await vi.advanceTimersByTimeAsync(1)
    await flushPromises()
    expect(w.find('[data-testid="state-tag"]').text()).toBe('正在下载')
    const calls = global.fetch.mock.calls.length
    expect(calls).toBeGreaterThanOrEqual(2)
    await vi.advanceTimersByTimeAsync(2000)
    expect(global.fetch.mock.calls.length).toBeGreaterThan(calls)
    w.unmount()
    const afterUnmount = global.fetch.mock.calls.length
    await vi.advanceTimersByTimeAsync(60000)
    expect(global.fetch.mock.calls.length).toBe(afterUnmount)
  })

  it('useSystemStatus 在组件 setup 内使用：卸载自动停止轮询（WEB-001/003 验收）', async () => {
    vi.useFakeTimers()
    global.fetch.mockImplementation(async (url) =>
      res(200, url === '/api/system/update' ? upd('checking') : INFO))
    let ctlRef = null
    const { defineComponent, h } = await import('vue')
    const Host = defineComponent({
      setup() {
        ctlRef = useSystemStatus({ fastMs: 1000, slowMs: 10000 })
        return () => h('div')
      },
    })
    const w = mount(Host)
    const callsAtMount = global.fetch.mock.calls.length
    expect(callsAtMount).toBeGreaterThanOrEqual(2)
    await vi.advanceTimersByTimeAsync(2000)
    expect(global.fetch.mock.calls.length).toBeGreaterThan(callsAtMount)
    w.unmount()
    const callsAtUnmount = global.fetch.mock.calls.length
    await vi.advanceTimersByTimeAsync(60000)
    expect(global.fetch.mock.calls.length).toBe(callsAtUnmount)
    expect(ctlRef.st.polling).toBe(false)
  })
})

describe('UpdateConfirmModal（WEB-004）', () => {
  it('安装确认：当前/目标版本、channel、schema 与回滚下限、维护窗口提示、重启警示', () => {
    const w = mount(UpdateConfirmModal, { props: { open: true, mode: 'install', info: INFO, status: UPD_STAGED } })
    const t = w.text()
    expect(t).toContain('安装更新确认')
    expect(t).toContain('当前版本')
    expect(t).toContain('0.2.0')
    expect(t).toContain('目标版本')
    expect(t).toContain('0.3.0')
    expect(t).toContain('stable')
    expect(t).toContain('852 MB')
    expect(t).toContain('数据库 v1')
    expect(t).toContain('自动回滚下限 v1')
    expect(t).toContain('02:00')
    expect(t).toContain('06:00')
    expect(t).toContain('冻结窗口 30 分钟')
    expect(t).toContain('服务将重启')
  })

  it('回滚确认：回滚目标、升级前快照警示、危险按钮样式', async () => {
    const w = mount(UpdateConfirmModal, { props: { open: true, mode: 'rollback', info: INFO, status: UPD_STAGED } })
    const t = w.text()
    expect(t).toContain('回滚确认')
    expect(t).toContain('上一个稳定版本')
    expect(t).toContain('升级前快照')
    const btn = w.find('[data-testid="confirm-btn"]')
    expect(btn.classes()).toContain('btn-danger')
    expect(btn.text()).toBe('确认回滚')
    await btn.trigger('click')
    expect(w.emitted('confirm')).toHaveLength(1)
  })

  it('取消/关闭：取消按钮与右上 ✕ 都 emit close；open=false 不渲染', async () => {
    const w = mount(UpdateConfirmModal, { props: { open: true, mode: 'install', info: INFO, status: UPD_STAGED } })
    await w.findAll('button').find((b) => b.text() === '取消').trigger('click')
    expect(w.emitted('close')).toHaveLength(1)
    await w.findAll('button').find((b) => b.text() === '✕').trigger('click')
    expect(w.emitted('close')).toHaveLength(2)
    const closed = mount(UpdateConfirmModal, { props: { open: false, mode: 'install' } })
    expect(closed.find('.modal').exists()).toBe(false)
  })

  it('错误回填：update_not_ready 展示错误码 + blocking 门禁中文列表；submitting 禁用按钮', async () => {
    const w = mount(UpdateConfirmModal, {
      props: {
        open: true, mode: 'install', info: INFO, status: UPD_STAGED,
        error: { code: 'update_not_ready', message: '安装条件未满足：存在运行中的脚本', details: { blocking: ['active_run', 'cron_freeze_window'] } },
      },
    })
    const t = w.text()
    expect(t).toContain('错误码：update_not_ready')
    expect(t).toContain('存在运行中的脚本任务')
    expect(t).toContain('下一次定时任务触发时间在冻结窗口内')
    expect(w.find('[data-testid="confirm-btn"]').text()).toBe('确认安装')

    const w2 = mount(UpdateConfirmModal, {
      props: { open: true, mode: 'install', info: INFO, status: UPD_STAGED, submitting: true },
    })
    expect(w2.find('[data-testid="confirm-btn"]').text()).toBe('提交中…')
    expect(w2.find('[data-testid="confirm-btn"]').attributes('disabled')).toBeDefined()
  })

  it('rollback_unavailable 回填展示错误码与人工降级指引', () => {
    const w = mount(UpdateConfirmModal, {
      props: {
        open: true, mode: 'rollback', info: INFO, status: UPD_STAGED,
        error: { code: 'rollback_unavailable', message: '没有可用的自动回滚点', details: null },
      },
    })
    expect(w.text()).toContain('错误码：rollback_unavailable')
  })
})
