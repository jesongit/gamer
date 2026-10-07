// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive, ref } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { api } from './api'
import { useConsoleStage } from './components/console/useConsoleStage'
import { useConsoleTemplates } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleTemplates'
let wrapper
beforeEach(() => {
  vi.useFakeTimers()
  vi.spyOn(api, 'listTemplates').mockResolvedValue([])
  vi.spyOn(api, 'listMedia').mockResolvedValue([])
  vi.spyOn(api, 'activeRecording').mockResolvedValue(null)
})
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks(); vi.clearAllTimers(); vi.useRealTimers() })
function setup(captureLiveFrame) {
  const store = reactive({ deviceId: 'browser-a' }), connected = ref(true)
  const video = ref({ naturalWidth: 1920, naturalHeight: 1080 })
  let stage, templates
  wrapper = mount(defineComponent({ setup() {
    stage = useConsoleStage({ toast: vi.fn(), deviceId: () => store.deviceId, connected, liveVideoEl: () => video.value, captureLiveFrame })
    templates = useConsoleTemplates({ toast: vi.fn(), store, connected, packageId: ref('same-package'), templatesData: ref([]),
      videoElement: video, videoWrap: ref(null), current: ref(null), stage: stage.templateBridge, editorMatchThreshold: () => .8 })
    return () => h('div')
  } }))
  const selectB = () => { stage.onTargetChanged(); store.deviceId = 'browser-b'; stage.onDeviceChanged() }
  return { store, stage, templates, selectB }
}

describe('target changes invalidate vision and recording ownership', () => {
  it('same-package target switch discards late frozen frame before opening crop editor', async () => {
    let finish
    const { templates, selectB } = setup(() => new Promise(resolve => { finish = resolve }))
    const pending = templates.openCrop({ x: 10, y: 20, w: 300, h: 200 })
    selectB()
    finish({ source: {}, width: 1920, height: 1080, label: 'A frame' })
    await pending
    expect(templates.crop.active).toBe(false)
    expect(templates.crop.sourceLabel).not.toBe('A frame')
  })

  it('same-package target switch discards late match and retains full-resolution coordinates', async () => {
    let finish
    const match = vi.spyOn(api, 'testTemplate').mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const { templates, selectB, stage } = setup()
    const pending = templates.testMatch('image.png', { stepSemantics: true, matchOptions: { region: [.25, .25, .5, .5] } })
    expect(stage.displaySize()).toEqual({ width: 1920, height: 1080 })
    expect(match).toHaveBeenCalledWith('image.png', 'browser-a', .8, [480, 270, 960, 540], 'same-package')
    selectB()
    finish({ hit: true, x: 100, y: 100, width: 50, height: 50, score: .99 })
    await pending
    expect(templates.showHit.value).toBe(false)
    expect(templates.hitLabel.value).toBe('')
  })

  it('late recording lookup for old target cannot replace current target recording', async () => {
    const requests = []
    api.activeRecording.mockImplementation(id => new Promise(resolve => requests.push({ id, resolve })))
    const { stage, selectB } = setup()
    selectB()
    const b = requests.find(request => request.id === 'browser-b')
    b.resolve({ id: 'recording-b', state: 'recording' })
    await flushPromises()
    for (const request of requests.filter(request => request.id === 'browser-a')) request.resolve({ id: 'recording-a', state: 'recording' })
    await flushPromises()
    const stop = vi.spyOn(api, 'recordingStop').mockResolvedValue({ id: 'recording-b', state: 'completed', segments: [] })
    await stage.view.toggleRecording()
    expect(stop).toHaveBeenCalledWith('recording-b')
  })
  it('late recording start result cannot label or block the newly selected target', async () => {
    let finish
    vi.spyOn(api, 'recordingStart').mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const { stage, selectB } = setup()
    await flushPromises()
    const pending = stage.view.toggleRecording()
    expect(stage.view.recordingBusy).toBe(true)
    selectB()
    await flushPromises()
    finish({ id: 'recording-a', state: 'recording' })
    await pending
    expect(stage.view.recordingActive).toBe(false)
    expect(stage.view.recordingBusy).toBe(false)
  })

  it('late recording stop result cannot open the old target recording in the new tile', async () => {
    api.activeRecording.mockImplementation(async id => id === 'browser-a' ? { id: 'recording-a', state: 'recording' } : null)
    let finish
    vi.spyOn(api, 'recordingStop').mockImplementation(() => new Promise(resolve => { finish = resolve }))
    const getMedia = vi.spyOn(api, 'getMedia').mockResolvedValue({ id: 'clip-a', width: 1920, height: 1080 })
    const { stage, selectB } = setup()
    await flushPromises()
    const pending = stage.view.toggleRecording()
    selectB()
    await flushPromises()
    finish({ id: 'recording-a', state: 'completed', segments: [{ media_id: 'clip-a' }] })
    await pending
    expect(stage.view.kind).toBe('live')
    expect(stage.view.recordingBusy).toBe(false)
    expect(getMedia).not.toHaveBeenCalled()
  })

})
