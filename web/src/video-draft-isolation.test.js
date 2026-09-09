// @vitest-environment happy-dom
/**
 * VideoDraft V05：录制会话、异步请求、草稿和 Package 上下文隔离回归。
 * 该文件只覆盖 VideoDraft.vue，避免把媒体 API/Stage 的行为混入本任务。
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
const { routerPush, requestAutomationEditor } = mocks

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: routerPush, currentRoute: { value: { query: {} } } }),
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

const A_EVENTS = [
  { event_id: 'a-1', kind: 'tap', source: 'manual', timeline_us: 1000, payload: { x: 1, y: 2 } },
]
const B_EVENTS = [
  { event_id: 'b-1', kind: 'swipe', source: 'runner', timeline_us: 2000, payload: { x: 3, y: 4, x2: 5, y2: 6 } },
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
    props: { recordingId: 'rec-a', packageId: 'pkg-a', yamlReady: true, ...props },
  })
}

async function loadCurrent(wrapper, list = A_EVENTS) {
  videoApi.recordingEvents.mockResolvedValueOnce(list)
  await wrapper.find('[data-testid="draft-load"]').trigger('click')
  await flushPromises()
}

beforeEach(() => {
  vi.clearAllMocks()
  videoApi.recordingEvents.mockResolvedValue([])
  videoApi.createVideoDraft.mockResolvedValue({ yaml: '', diagnostics: [] })
  videoApi.saveDraft.mockResolvedValue({ id: 'pkg-a/draft.yaml', package_id: 'pkg-a' })
})

describe('VideoDraft V05 草稿与录制会话隔离', () => {
  it('切换会话前提供保留/放弃/取消；放弃后同步清理选择、注释和草稿并加载新会话', async () => {
    const wrapper = mountDraft()
    await loadCurrent(wrapper)

    await wrapper.find('[data-testid="draft-event-check-a-1"]').setValue(true)
    await wrapper.find('[data-testid="draft-event-comment-a-1"]').setValue('旧会话注释')
    expect(wrapper.find('[data-testid="draft-event-comment-a-1"]').element.value).toBe('旧会话注释')

    await wrapper.setProps({ recordingId: 'rec-b' })
    expect(wrapper.find('[data-testid="draft-switch-protect"]').exists()).toBe(true)
    await wrapper.find('[data-testid="draft-switch-cancel"]').trigger('click')
    expect(wrapper.find('[data-testid="draft-event-check-a-1"]').exists()).toBe(true)
    expect(videoApi.recordingEvents).toHaveBeenCalledTimes(1)

    await wrapper.find('[data-testid="draft-recording-id"]').setValue('rec-b')
    await wrapper.find('[data-testid="draft-recording-id"]').trigger('change')
    const newRequest = deferred()
    videoApi.recordingEvents.mockReturnValueOnce(newRequest.promise)
    await wrapper.find('[data-testid="draft-switch-discard"]').trigger('click')
    expect(wrapper.findAll('.event-row')).toHaveLength(0)
    expect(videoApi.recordingEvents).toHaveBeenLastCalledWith('rec-b')

    newRequest.resolve(B_EVENTS)
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-event-check-b-1"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="draft-event-comment-a-1"]').exists()).toBe(false)
    expect(wrapper.find('.events-summary').text()).toContain('已选 0')
    wrapper.unmount()
  })

  it('旧会话读取返回晚于新会话时不能覆盖新列表', async () => {
    const oldRequest = deferred()
    const newRequest = deferred()
    videoApi.recordingEvents
      .mockImplementationOnce(() => oldRequest.promise)
      .mockImplementationOnce(() => newRequest.promise)

    const wrapper = mountDraft()
    await wrapper.find('[data-testid="draft-load"]').trigger('click')
    await wrapper.setProps({ recordingId: 'rec-b' })
    expect(videoApi.recordingEvents).toHaveBeenNthCalledWith(2, 'rec-b')

    oldRequest.resolve(A_EVENTS)
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-event-list"]').exists()).toBe(false)

    newRequest.resolve(B_EVENTS)
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-event-check-b-1"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="draft-event-check-a-1"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('空会话和失败会话都清除旧 event_id，不能继续生成上一会话草稿', async () => {
    const wrapper = mountDraft()
    await loadCurrent(wrapper)
    await wrapper.find('[data-testid="draft-event-check-a-1"]').setValue(true)

    const emptyRequest = deferred()
    videoApi.recordingEvents.mockReturnValueOnce(emptyRequest.promise)
    await wrapper.setProps({ recordingId: 'rec-b' })
    await wrapper.find('[data-testid="draft-switch-discard"]').trigger('click')
    emptyRequest.resolve([])
    await flushPromises()
    expect(wrapper.findAll('.event-row')).toHaveLength(0)
    expect(wrapper.find('[data-testid="draft-generate"]').exists()).toBe(false)
    expect(videoApi.createVideoDraft).not.toHaveBeenCalled()

    const failedRequest = deferred()
    videoApi.recordingEvents.mockReturnValueOnce(failedRequest.promise)
    await wrapper.setProps({ recordingId: 'rec-c' })
    failedRequest.reject(new Error('recording unavailable'))
    await flushPromises()
    expect(wrapper.findAll('.event-row')).toHaveLength(0)
    expect(wrapper.find('[data-testid="draft-error"]').text()).toContain('recording unavailable')
    expect(videoApi.createVideoDraft).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('旧生成响应不能覆盖新会话；保留草稿选项不会复用旧会话的选择和注释', async () => {
    const wrapper = mountDraft()
    await loadCurrent(wrapper)
    await wrapper.find('[data-testid="draft-event-check-a-1"]').setValue(true)
    const oldDraft = deferred()
    videoApi.createVideoDraft.mockReturnValueOnce(oldDraft.promise)
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')

    await wrapper.setProps({ recordingId: 'rec-b' })
    await wrapper.find('[data-testid="draft-switch-retain"]').trigger('click')
    expect(wrapper.findAll('.event-row')).toHaveLength(0)
    expect(videoApi.recordingEvents).toHaveBeenLastCalledWith('rec-b')

    oldDraft.resolve({
      yaml: 'old-session-yaml',
      diagnostics: [],
      source: { recording_id: 'rec-a', events: [] },
    })
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-yaml"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="draft-event-check-a-1"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('生成冻结 Package；切换 Package 后保存仍提交原 Package，并打开原 Package 编辑器', async () => {
    const wrapper = mountDraft()
    await loadCurrent(wrapper)
    await wrapper.find('[data-testid="draft-event-check-a-1"]').setValue(true)
    videoApi.createVideoDraft.mockResolvedValueOnce({
      yaml: 'draft-for-a', diagnostics: [], source: { recording_id: 'rec-a', events: [] },
    })
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="draft-bound-package"]').text()).toBe('pkg-a')

    await wrapper.find('[data-testid="draft-save-name"]').setValue('draft')
    await wrapper.setProps({ packageId: 'pkg-b' })
    expect(wrapper.find('[data-testid="draft-switch-protect"]').exists()).toBe(true)
    await wrapper.find('[data-testid="draft-save"]').trigger('click')
    await flushPromises()

    expect(videoApi.saveDraft).toHaveBeenCalledWith({
      packageId: 'pkg-a', name: 'draft', yaml: 'draft-for-a', overwrite: false,
    })
    expect(requestAutomationEditor).toHaveBeenCalledWith('pkg-a', 'pkg-a/draft.yaml')
    expect(routerPush).toHaveBeenCalled()
    wrapper.unmount()
  })

  it('保存请求进行中切换会话后，旧保存响应不能打开编辑器或改写新会话', async () => {
    const wrapper = mountDraft()
    await loadCurrent(wrapper)
    await wrapper.find('[data-testid="draft-event-check-a-1"]').setValue(true)
    videoApi.createVideoDraft.mockResolvedValueOnce({
      yaml: 'draft-for-a', diagnostics: [], source: { recording_id: 'rec-a', events: [] },
    })
    await wrapper.find('[data-testid="draft-generate"]').trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="draft-save-name"]').setValue('draft')

    const oldSave = deferred()
    videoApi.saveDraft.mockReturnValueOnce(oldSave.promise)
    await wrapper.find('[data-testid="draft-save"]').trigger('click')
    await wrapper.setProps({ recordingId: 'rec-b' })
    await wrapper.find('[data-testid="draft-switch-discard"]').trigger('click')

    oldSave.resolve({ id: 'pkg-a/old.yaml', package_id: 'pkg-a' })
    await flushPromises()
    expect(requestAutomationEditor).not.toHaveBeenCalled()
    expect(routerPush).not.toHaveBeenCalled()
    expect(wrapper.find('[data-testid="draft-yaml"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('非单调或非法 timeline_us 显示时序诊断，并保持事件可人工确认', async () => {
    const wrapper = mountDraft()
    await loadCurrent(wrapper, [
      { event_id: 'late', kind: 'tap', source: 'manual', timeline_us: 3000, payload: {} },
      { event_id: 'early', kind: 'tap', source: 'manual', timeline_us: 1000, payload: {} },
      { event_id: 'bad', kind: 'tap', source: 'manual', timeline_us: -1, payload: {} },
    ])

    expect(wrapper.find('[data-testid="draft-diagnostics-only"]').text()).toContain('事件时序逆序')
    expect(wrapper.find('[data-testid="draft-diagnostics-only"]').text()).toContain('timeline_us 无效')
    expect(wrapper.findAll('.event-row')[0].text()).toContain('0.001s')
    wrapper.unmount()
  })
})
