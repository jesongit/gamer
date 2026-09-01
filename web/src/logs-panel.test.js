// @vitest-environment happy-dom
/**
 * LogsPanel（日志页签）挂载测试：mock ./api 与 store 数据源，
 * 锁定「按设备+脚本连续段插分组分割线」的展示行为——
 * - 服务端 id 倒序返回 → 前端反转为时间正序；
 * - 连续同「设备+脚本」归为一段共用组头，脚本/设备切换处出现新分割线；
 * - 同一脚本再次出现（交替运行）时组头重复出现，来源仍可辨。
 */
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal()),
  api: {
    listLogs: vi.fn(async () => []),
    clearLogs: vi.fn(async () => ({})),
    listDevices: vi.fn(async () => []),
    listScripts: vi.fn(async () => []),
  },
}))

import LogsPanel from './components/LogsPanel.vue'
import { api } from './api'
import { devicesData, scriptsData } from './store'

const DEV = [{ id: 'dev1', name: '设备一' }, { id: 'dev2', name: '设备二' }]
const SCRIPTS = [
  { id: 'com.demo/a.yml', name: 'a.yml', package: 'com.demo' },
  { id: 'com.demo/b.yml', name: 'b.yml', package: 'com.demo' },
]
// 服务端形态：id 倒序（最新在前）。序列 = a×2 → b×1 → a×1（交替运行）
const LOGS = [
  { id: 4, time: '2026-09-01 10:00:03', device_id: 'dev1', script_id: 'com.demo/a.yml', level: 'info', msg: 'a-3' },
  { id: 3, time: '2026-09-01 10:00:02', device_id: 'dev2', script_id: 'com.demo/b.yml', level: 'info', msg: 'b-1' },
  { id: 2, time: '2026-09-01 10:00:01', device_id: 'dev1', script_id: 'com.demo/a.yml', level: 'warn', msg: 'a-2' },
  { id: 1, time: '2026-09-01 10:00:00', device_id: 'dev1', script_id: 'com.demo/a.yml', level: 'info', msg: 'a-1' },
]

beforeEach(() => {
  vi.clearAllMocks()
  devicesData.value = DEV.map(d => ({ ...d }))
  scriptsData.value = SCRIPTS.map(s => ({ ...s }))
  api.listLogs.mockResolvedValue(LOGS.map(l => ({ ...l })))
  // 挂载后面板会用接口响应覆盖 store 数据源，mock 必须返回同样的数据
  api.listDevices.mockResolvedValue(DEV.map(d => ({ ...d })))
  api.listScripts.mockResolvedValue(SCRIPTS.map(s => ({ ...s })))
})

describe('LogsPanel 运行分组', () => {
  it('时间正序展示，连续「设备+脚本」段各带组头（脚本切换/交替运行处插分割线）', async () => {
    const w = mount(LogsPanel)
    await flushPromises()

    const lines = w.findAll('.log-line')
    expect(lines.map(l => l.find('.lg-msg').text())).toEqual(['a-1', 'a-2', 'b-1', 'a-3']) // 正序
    const heads = w.findAll('.run-divider')
    expect(heads).toHaveLength(3) // a段 → b段 → a段（再次出现重复组头）
    expect(heads[0].text()).toContain('com.demo/a.yml')
    expect(heads[0].text()).toContain('设备一')
    expect(heads[1].text()).toContain('com.demo/b.yml')
    expect(heads[1].text()).toContain('设备二')
    expect(heads[2].text()).toContain('com.demo/a.yml')
    // 组内消息不再重复设备/脚本列（组头已承载来源信息）
    expect(lines[0].find('.lg-msg').text()).not.toContain('com.demo')
  })

  it('空日志显示空态', async () => {
    api.listLogs.mockResolvedValue([])
    const w = mount(LogsPanel)
    await flushPromises()
    expect(w.text()).toContain('没有日志记录')
    expect(w.findAll('.run-divider')).toHaveLength(0)
  })
})
