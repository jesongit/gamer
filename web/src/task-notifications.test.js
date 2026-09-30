// @vitest-environment happy-dom
import { beforeEach, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { taskNotificationPolicy, updateTaskNotificationPolicy } from './components/task/task-notifications'
const mocks = vi.hoisted(() => ({ list: vi.fn(), call: vi.fn() }))
vi.mock('./api', () => ({ api: { listExtensions: mocks.list, callExtension: mocks.call } }))
import TaskNotifications from './components/task/TaskNotifications.vue'
beforeEach(() => { mocks.list.mockReset().mockResolvedValue([]); mocks.call.mockReset() })
it('默认关闭通知，启用后默认通知成功和失败，保留其他扩展配置', () => {
  const policy = taskNotificationPolicy()
  expect(policy.enabled).toBe(false)
  expect(policy.results.success.enabled).toBe(true)
  expect(policy.results.failed.enabled).toBe(true)
  expect(policy.results.cancelled.enabled).toBe(false)
  expect(policy.results.skipped.enabled).toBe(false)
  expect(updateTaskNotificationPolicy({ other: { x: 1 } }, policy).other).toEqual({ x: 1 })
})
it('没有插件仍可编辑通道和文案，且不调用发送能力', async () => {
  const original = { other: { x: 1 }, 'gamer-notify': { enabled: true, results: { success: { channels: ['saved'] } } } }
  const w = mount(TaskNotifications, { props: { modelValue: original } }); await flushPromises()
  expect(w.text()).toContain('配置保留')
  await w.findAll('input.input')[0].setValue('saved, future, saved')
  const updated = w.emitted('update:modelValue').at(-1)[0]
  expect(updated['gamer-notify'].results.success.channels).toEqual(['saved', 'future'])
  expect(updated.other).toEqual({ x: 1 })
  expect(original['gamer-notify'].results.success.channels).toEqual(['saved'])
  expect(mocks.call).not.toHaveBeenCalled()
  w.unmount()
})
it('通道删除后引用仍可显示和保存', async () => {
  mocks.list.mockResolvedValue([{ id: 'gamer-notify', state: 'running' }])
  mocks.call.mockResolvedValue({ channels: [{ id: 'current', name: '当前微信', enabled: true }] })
  const policy = taskNotificationPolicy(); policy.enabled = true; policy.results.success.channels = ['missing']
  const w = mount(TaskNotifications, { props: { modelValue: { 'gamer-notify': policy } } }); await flushPromises()
  expect(w.findAll('select')[0].text()).toContain('missing（不存在）')
  expect(w.findAll('select')[0].findAll('option').find(o => o.element.value === 'missing').element.selected).toBe(true)
  w.unmount()
})
