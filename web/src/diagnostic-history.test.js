// @vitest-environment happy-dom
import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, expect, it, vi } from 'vitest'
import TaskRunHistory from './components/task/TaskRunHistory.vue'
import { api } from './api'

vi.mock('./api', () => ({ api: { listRunHistory: vi.fn() } }))
vi.mock('./store', () => ({ devicesData: { value: [] } }))
let wrapper
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks() })

it('offers an exact-run native download and distinguishes expired details from missing runs', async () => {
  api.listRunHistory.mockResolvedValue([{run_id:'run-one',task_id:'daily',device_id:'d',entrypoint:'p/daily',state:'failed',started_at:'2026-10-18T04:01:00Z',finished_at:'2026-10-18T04:02:00Z',diagnostics:{details_status:'expired'}}])
  wrapper=mount(TaskRunHistory,{props:{taskId:'daily'}})
  await flushPromises()
  expect(wrapper.get('a').attributes('href')).toBe('/api/runs/run-one/diagnostics')
  expect(wrapper.get('a').attributes('download')).toBe('gamer-diagnostics-run-one.zip')
  expect(wrapper.text()).toContain('详细步骤已到期，运行结果仍保留')
})

it('does not export an active run as a completed support package', async () => {
  api.listRunHistory.mockResolvedValue([{run_id:'active',task_id:'daily',state:'running',started_at:'2026-10-18T04:01:00Z'}])
  wrapper=mount(TaskRunHistory,{props:{taskId:'daily'}})
  await flushPromises()
  expect(wrapper.find('a').exists()).toBe(false)
  expect(wrapper.text()).toContain('运行结束后可下载诊断包')
})
