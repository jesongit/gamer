// @vitest-environment happy-dom
/**
 * P5-VDRAFT：录制历史事件 → YAML V1 草稿的完整 UI 工作流。
 * 只覆盖 VideoDraft 及其专属工作流模型；不把 VideoWorkbench、Stage 或 API
 * 实现细节混进来。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

const mocks = vi.hoisted(() => ({
  routerPush: vi.fn(async () => {}),
  requestAutomationEditor: vi.fn(),
  recordingEvents: vi.fn(),
  createVideoDraft: vi.fn(),
  saveDraft: vi.fn(),
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: mocks.routerPush, currentRoute: { value: { query: {} } } }),
}))

vi.mock('./components/console/automationEditorBridge', () => ({
  requestAutomationEditor: mocks.requestAutomationEditor,
}))

vi.mock('./components/video/videoApi', () => ({
  videoApi: {
    recordingEvents: mocks.recordingEvents,
    createVideoDraft: mocks.createVideoDraft,
    saveDraft: mocks.saveDraft,
  },
}))

import VideoDraft from './components/video/VideoDraft.vue'
import { videoApi } from './components/video/videoApi'

const EVENTS = [
  { event_id: 'tap-1', kind: 'tap', source: 'manual', status: 'accepted', timeline_us: 100000, payload: { x: 10, y: 20 } },
  { event_id: 'text-1', kind: 'text', source: 'manual', status: 'accepted', timeline_us: 900000, payload: { length: 4 } },
  { event_id: 'swipe-1', kind: 'swipe', source: 'runner', status: 'accepted', timeline_us: 2200000, payload: { x: 1, y: 2, x2: 4, y2: 5 } },
  { event_id: 'unknown-1', kind: 'pinch', source: 'plugin', status: 'accepted', timeline_us: 4000000, payload: { scale: 2 } },
]

function deferred() {
  let resolve
  let reject
  const promise = new Promise((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function mountDraft(props = {}) {
  return mount(VideoDraft, {
    props: {
      recordingId: 'rec-a',
      packageId: 'pkg-a',
      deviceId: 'device-a',
      androidPackageName: 'com.example.game',
      yamlReady: true,
      ...props,
    },
  })
}

async function load(wrapper, events = EVENTS) {
  videoApi.recordingEvents.mockResolvedValueOnce(events)
  await wrapper.find('[data-testid="draft-load"]').trigger('click')
  await flushPromises()
}

beforeEach(() => {
  vi.clearAllMocks()
  videoApi.recordingEvents.mockResolvedValue([])
  videoApi.createVideoDraft.mockResolvedValue({ yaml: '', diagnostics: [] })
  videoApi.saveDraft.mockResolvedValue({ id: 'pkg-a/draft.yaml', package_id: 'pkg-a' })
})

describe('P5-VDRAFT 录制历史事件 UI 工作流', () => {
  it('支持关键词/类型/来源/状态/时间筛选；筛选只影响视图，隐藏的已选事件不丢失', async () => {
    const wrapper = mountDraft()
    await load(wrapper)

    expect(wrapper.findAll('.event-row')).toHaveLength(4)
    await wrapper.find('[data-testid="draft-event-kind"]').setValue('tap')
    expect(wrapper.findAll('.event-row')).toHaveLength(1)
    await wrapper.find('[data-testid="draft-select-all"]').trigger('click')
    expect(wrapper.find('.events-summary').text()).toContain('已选 1')

    await wrapper.find('[data-testid="draft-event-kind"]').setValue('')
    await wrapper.find('[data-testid="draft-event-source"]').setValue('runner')
    expect(wrapper.findAll('.event-row')).toHaveLength(1)
    expect(wrapper.find('.event-row').text()).toContain('swipe')
    expect(wrapper.find('.events-summary').text()).toContain('已选 1')

    await wrapper.find('[data-testid="draft-event-source"]').setValue('')
    await wrapper.find('[data-testid="draft-event-from"]').setValue('2')
    await wrapper.find('[data-testid="draft-event-to"]').setValue('3')
    expect(wrapper.findAll('.event-row')).toHaveLength(1)
    expect(wrapper.find('.event-row').text()).toContain('swipe')
    await wrapper.find('[data-testid="draft-event-filter-clear"]').trigger('click')
    expect(wrapper.findAll('.event-row')).toHaveLength(4)
    expect(wrapper.find('[data-testid="draft-event-check-tap-1"]').element.checked).toBe(true)
    wrapper.unmount()
  })

  it('未支持事件在生成前后都有诊断；生成只调用草稿动作，不创建任务或执行设备输入', async () => {
    const wrapper = mountDraft()
    await load(wrapper)

    const diagnostics = wrapper.find('[data-testid="draft-diagnostics-only"]')
    expect(diagnostics.text()).toContain('text 事件')
    expect(diagnostics.text()).toContain('pinch')

    await wrapper.find('[data-testid="draft-event-search"]').setValue('tap-1')
    await wrapper.find('[data-testid="draft-select-all"]').trigger('click')
    videoApi.createVideoDraft.mockResolvedValueOnce({
      yaml: 'run:\n  - tap: [0.1, 0.2]\n',
      diagnostics: [],
      source: { recording_id: 'rec-a', events: [{ event_id: 'tap-1', selected: true }] },
    })
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()

    expect(videoApi.createVideoDraft).toHaveBeenCalledWith('rec-a', ['tap-1'], {})
    expect(videoApi.saveDraft).not.toHaveBeenCalled()
    expect(mocks.requestAutomationEditor).not.toHaveBeenCalled()
    expect(wrapper.find('[data-testid="draft-yaml"]').text()).toContain('tap: [0.1, 0.2]')
    expect(wrapper.text()).toContain('不会自动执行')
    wrapper.unmount()
  })

  it('生成与保存绑定 Package/设备上下文；保存失败保留草稿并可重试，成功后才打开编辑器', async () => {
    const wrapper = mountDraft()
    await load(wrapper, [EVENTS[0]])
    await wrapper.find('[data-testid="draft-select-all"]').trigger('click')
    videoApi.createVideoDraft.mockResolvedValueOnce({
      yaml: 'run:\n  - tap: [0.1, 0.2]\n',
      diagnostics: [],
      source: { recording_id: 'rec-a', events: [] },
    })
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()

    expect(wrapper.find('[data-testid="draft-context"]').text()).toContain('Package：pkg-a')
    expect(wrapper.find('[data-testid="draft-context"]').text()).toContain('设备：device-a')
    expect(wrapper.find('[data-testid="draft-bound-device"]').text()).toBe('device-a')

    await wrapper.find('[data-testid="draft-save-name"]').setValue('draft')
    videoApi.saveDraft.mockRejectedValueOnce(new Error('资源版本冲突'))
    await wrapper.find('[data-testid="draft-save"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-yaml"]').text()).toContain('tap: [0.1, 0.2]')
    expect(wrapper.find('[data-testid="draft-save-name"]').element.value).toBe('draft')
    expect(wrapper.find('[data-testid="draft-save-failed"]').exists()).toBe(true)
    expect(mocks.requestAutomationEditor).not.toHaveBeenCalled()

    videoApi.saveDraft.mockResolvedValueOnce({ id: 'pkg-a/automations/draft.yaml', package_id: 'pkg-a' })
    await wrapper.find('[data-testid="draft-save"]').trigger('click')
    await flushPromises()
    expect(videoApi.saveDraft).toHaveBeenLastCalledWith({
      packageId: 'pkg-a', name: 'draft', yaml: 'run:\n  - tap: [0.1, 0.2]\n', overwrite: false,
    })
    expect(mocks.requestAutomationEditor).toHaveBeenCalledWith('pkg-a', 'pkg-a/automations/draft.yaml')
    expect(mocks.routerPush).toHaveBeenCalled()
    wrapper.unmount()
  })

  it('旧录制会话响应不能覆盖新会话的事件列表或选择', async () => {
    const oldRequest = deferred()
    const newRequest = deferred()
    videoApi.recordingEvents
      .mockImplementationOnce(() => oldRequest.promise)
      .mockImplementationOnce(() => newRequest.promise)
    const wrapper = mountDraft()
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await wrapper.setProps({ recordingId: 'rec-b' })

    oldRequest.resolve([EVENTS[0]])
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-event-check-tap-1"]').exists()).toBe(false)
    newRequest.resolve([{ ...EVENTS[2], event_id: 'swipe-b' }])
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-event-check-swipe-b"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="draft-event-check-tap-1"]').exists()).toBe(false)
    wrapper.unmount()
  })
})

