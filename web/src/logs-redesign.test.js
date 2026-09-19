// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import LogsPanel from './components/LogsPanel.vue'
import { api } from './api'
vi.mock('./api', () => ({ api: { listLogs: vi.fn(), listDevices: vi.fn(), clearLogs: vi.fn() } }))
let wrapper
const records = [
  { id: 2, time: '16:20:02', level: 'error', msg: '等待模板超时', entrypoint: 'demo/daily.yaml', device_id: 'device1' },
  { id: 1, time: '16:20:01', level: 'info', msg: '开始运行', entrypoint: 'demo/daily.yaml', device_id: 'device1' },
]
beforeEach(() => {
  vi.useFakeTimers(); vi.clearAllMocks()
  api.listLogs.mockResolvedValue(records)
  api.listDevices.mockResolvedValue([{ id: 'device1', name: '测试设备' }])
  api.clearLogs.mockResolvedValue({})
})
afterEach(() => { wrapper?.unmount(); vi.useRealTimers() })
it('日志时间正序，搜索本次记录，暂停后停止轮询', async () => {
  wrapper = mount(LogsPanel); await flushPromises()
  expect(wrapper.findAll('.lg-msg').map(row => row.text())).toEqual(['开始运行', '等待模板超时'])
  await wrapper.get('[aria-label="搜索日志"]').setValue('超时')
  expect(wrapper.findAll('.log-line')).toHaveLength(1)
  expect(wrapper.find('.log-foot').text()).toContain('1 / 2')
  await wrapper.get('.refresh-toggle').trigger('click')
  const calls = api.listLogs.mock.calls.length
  await vi.advanceTimersByTimeAsync(10000)
  expect(api.listLogs).toHaveBeenCalledTimes(calls)
  await wrapper.get('.refresh-toggle').trigger('click')
  await vi.advanceTimersByTimeAsync(5000)
  expect(api.listLogs).toHaveBeenCalledTimes(calls + 1)
})
it('更换过滤条件后旧请求不能覆盖新结果', async () => {
  let resolveOld
  api.listLogs.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
  wrapper = mount(LogsPanel); await flushPromises()
  api.listLogs.mockResolvedValueOnce([records[0]])
  await wrapper.get('[aria-label="日志级别"]').setValue('error'); await flushPromises()
  resolveOld(records); await flushPromises()
  expect(wrapper.findAll('.log-line')).toHaveLength(1)
  expect(wrapper.find('.lg-msg').text()).toBe('等待模板超时')
})
it('读取失败可见，清空后在途请求不能恢复旧日志', async () => {
  api.listLogs.mockRejectedValueOnce(new Error('连接中断'))
  wrapper = mount(LogsPanel); await flushPromises()
  expect(wrapper.get('[role="alert"]').text()).toContain('连接中断')
  let resolveOld
  api.listLogs.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
  await wrapper.get('[aria-label="刷新日志"]').trigger('click')
  await wrapper.findAll('button').find(button => button.text() === '清空').trigger('click'); await flushPromises()
  resolveOld(records); await flushPromises()
  expect(wrapper.findAll('.log-line')).toHaveLength(0)
})
