// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, h, reactive } from 'vue'
import { mediaDragTarget, useMediaGestures } from './components/console/useMediaGestures'

let wrapper
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks() })
function setup() {
  const props = reactive({ stage: { kind: 'media', mediaId: 'clip', stageReady: true, duration: 300, currentTime: 100, playing: false, seek: vi.fn(), togglePlay: vi.fn() }, selectionMode: false, selecting: false, onMouseDown: vi.fn(), onMouseMove: vi.fn(), onMouseUp: vi.fn() })
  let gestures
  wrapper = mount(defineComponent({ setup() { gestures = useMediaGestures(props); return () => h('div') } }))
  return { props, gestures }
}
const event = (x, y = 20) => ({ clientX: x, clientY: y, button: 0, preventDefault: vi.fn() })
const dispatch = (type, x, y = 20) => window.dispatchEvent(new MouseEvent(type, event(x, y)))
it('同距离快划跨越更多时间，正反方向正确且不越过首尾', () => {
  expect(mediaDragTarget(100, 100, 100, 300)).toBeGreaterThan(mediaDragTarget(100, 100, 1000, 300))
  expect(mediaDragTarget(100, -100, 100, 300)).toBeLessThan(100)
  expect(mediaDragTarget(0, -100, 100, 300)).toBe(0)
  expect(mediaDragTarget(299, 100, 100, 300)).toBe(300)
})
it('轻点只切换播放，横拖只跳转，在画面外松开也能收尾', () => {
  const {props, gestures} = setup()
  gestures.down(event(10)); dispatch('mouseup', 12)
  expect(props.stage.togglePlay).toHaveBeenCalledOnce()
  gestures.down(event(10)); dispatch('mousemove', 110); dispatch('mouseup', 110)
  expect(props.stage.seek).toHaveBeenCalled()
  expect(props.stage.togglePlay).toHaveBeenCalledOnce()
  props.stage.seek.mockClear(); dispatch('mousemove', 120)
  expect(props.stage.seek).not.toHaveBeenCalled()
})
it('框选/取点优先处理；纵向移动不误播放，窗口失焦取消手势', () => {
  const {props, gestures} = setup()
  props.selectionMode = true
  gestures.down(event(10)); gestures.hover(event(30)); gestures.release(event(30))
  expect(props.onMouseDown).toHaveBeenCalledOnce()
  expect(props.onMouseUp).toHaveBeenCalledOnce()
  expect(props.stage.seek).not.toHaveBeenCalled()
  props.selectionMode = false
  gestures.down(event(10)); dispatch('mousemove', 10, 100); dispatch('mouseup', 10, 100)
  expect(props.stage.togglePlay).not.toHaveBeenCalled()
  gestures.down(event(10)); window.dispatchEvent(new Event('blur')); dispatch('mouseup', 10)
  expect(props.stage.togglePlay).not.toHaveBeenCalled()
})
it('来源变化或卸载后不处理遗留拖动', async () => {
  const {props, gestures} = setup()
  gestures.down(event(10)); props.stage.mediaId = 'other'; dispatch('mousemove', 100)
  expect(props.stage.seek).not.toHaveBeenCalled()
  gestures.down(event(10)); wrapper.unmount(); dispatch('mouseup', 10)
  expect(props.stage.togglePlay).not.toHaveBeenCalled()
})
