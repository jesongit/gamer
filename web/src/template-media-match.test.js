// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive, ref } from 'vue'
import { mount } from '@vue/test-utils'
import { api } from './api'
import { useConsoleTemplates } from '../../plugins/gamer-yaml/ui/src/components/console/useConsoleTemplates'

let wrapper
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks() })
function setup() {
  vi.spyOn(api, 'listTemplates').mockResolvedValue([])
  const match = vi.spyOn(api, 'testTemplate').mockResolvedValue({hit: true, x: 1, y: 2, width: 20, height: 10, score: 1})
  const frame = ref({mediaId: 'clip', index: 29, ptsUs: 966666})
  const kind = ref('media'), generation = ref(1), toast = vi.fn()
  const stage = {
    kind: () => kind.value, generation: () => generation.value, ready: () => true,
    surfaceEl: () => ({videoWidth: 640, videoHeight: 360}), frameAt: () => frame.value,
    captureFrame: vi.fn(),
    capturePreviewFrame: vi.fn(async () => {
      const at = frame.value
      return {png: 'cG5n', isCurrent: () => frame.value === at}
    }),
  }
  let panel
  wrapper = mount(defineComponent({setup() {
    panel = useConsoleTemplates({stage, toast, store: reactive({deviceId: 'live-device'}),
      templatesData: ref([]), packageId: ref('pkg'), connected: ref(false),
      videoElement: ref(null), videoWrap: ref(null), current: ref(null), editorMatchThreshold: () => .8})
    return () => h('div')
  }}))
  return {panel, match, frame, kind, generation, toast, stage}
}
it('暂停视频的模板列表与步骤直接匹配浏览器像素，不调用确定帧抽取', async () => {
  const {panel, match, stage} = setup()
  await panel.testMatch('shot.png')
  expect(stage.captureFrame).not.toHaveBeenCalled()
  expect(stage.capturePreviewFrame).toHaveBeenCalledOnce()
  expect(match).toHaveBeenLastCalledWith('shot.png', null, .8, null, 'pkg', expect.objectContaining({png: 'cG5n'}))
  expect(panel.showHit.value).toBe(true)
  await panel.testMatch('shot.png', {stepSemantics: true})
  expect(match).toHaveBeenLastCalledWith('shot.png', null, .8, undefined, 'pkg', expect.objectContaining({png: 'cG5n'}))
  await panel.testMatch('shot.png', {stepSemantics: true, matchOptions: {threshold: .93, region: [.1, .2, .5, .5]}})
  expect(match).toHaveBeenLastCalledWith('shot.png', null, .93, [64, 72, 320, 180], 'pkg', expect.objectContaining({png: 'cG5n'}))
})
it('无法确定步骤引用参数时不发起使用其他默认值的测试', async () => {
  const {panel, match, toast} = setup()
  await panel.testMatch('shot.png', {stepSemantics: true, matchOptions: {error: '无法确定运行时变量'}})
  expect(match).not.toHaveBeenCalled()
  expect(toast).toHaveBeenCalledWith('无法确定运行时变量', 'error')
})
it('匹配请求期间切换视频位置，迟到命中不能画在新画面上', async () => {
  const {panel, match, frame, toast} = setup()
  let resolve
  match.mockReturnValue(new Promise(done => { resolve = done }))
  const pending = panel.testMatch('shot.png')
  await Promise.resolve()
  frame.value = {mediaId: 'clip', index: 40, ptsUs: 1500000}
  resolve({hit: true, x: 1, y: 2, width: 20, height: 10, score: 1})
  await pending
  expect(panel.showHit.value).toBe(false)
  expect(toast).not.toHaveBeenCalled()
})
