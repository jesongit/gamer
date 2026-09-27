// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive } from 'vue'
import { mount } from '@vue/test-utils'
import { useMediaKeyboard } from './components/console/useMediaKeyboard'
import ConsoleVideoStage from './components/console/ConsoleVideoStage.vue'

let wrapper
beforeEach(() => vi.useFakeTimers())
afterEach(() => { wrapper?.unmount(); vi.useRealTimers(); vi.restoreAllMocks() })
function setup() {
  const state = reactive({ blocked: false, focused: true, kind: 'media', mediaId: 'video', stageReady: true, currentTime: 30, duration: 120, playing: false })
  const seek = vi.fn(t => { state.currentTime = t }), togglePlay = vi.fn()
  let keys
  wrapper = mount(defineComponent({ setup() {
    keys = useMediaKeyboard({ stage: () => ({...state, seek, togglePlay}), blocked: () => state.blocked, focused: () => state.focused, report: vi.fn() })
    return () => h('div')
  }}))
  const down = (code, extra = {}) => keys.keydown({ code, preventDefault: vi.fn(), stopPropagation: vi.fn(), ...extra })
  return {state, seek, togglePlay, keys, down}
}
it.each([['KeyJ',25],['ArrowLeft',25],['KeyK',35],['ArrowRight',35]])('%s 单次跳转 5 秒，释放后停止', (key,target) => {
  const {down, state, keys, seek} = setup()
  down(key); expect(state.currentTime).toBe(target)
  keys.keyup({code:key}); vi.advanceTimersByTime(1000)
  expect(seek).toHaveBeenCalledTimes(1)
})
it('长按自行连续跳转，浏览器 repeat 不叠加；范围限制在首尾', () => {
  const {down, state, keys, seek} = setup()
  down('KeyK'); down('KeyK',{repeat:true})
  expect(seek).toHaveBeenCalledTimes(1)
  vi.advanceTimersByTime(650)
  expect(state.currentTime).toBe(50)
  keys.keyup({code:'KeyK'}); vi.advanceTimersByTime(1000)
  expect(state.currentTime).toBe(50)
  state.currentTime=119; down('ArrowRight'); expect(state.currentTime).toBe(120)
  keys.keyup({code:'ArrowRight'}); state.currentTime=1; down('ArrowLeft'); expect(state.currentTime).toBe(0)
})
it('长按空格只切换一次，再次按下才切换', () => {
  const {down, keys, togglePlay} = setup()
  down('Space'); down('Space',{repeat:true}); vi.advanceTimersByTime(1500)
  expect(togglePlay).toHaveBeenCalledTimes(1)
  keys.keyup({code:'Space'}); down('Space'); expect(togglePlay).toHaveBeenCalledTimes(2)
})
it('编辑输入、修饰键、输入法、失焦和框选不触发播放快捷键', () => {
  const {down, state, seek, togglePlay} = setup()
  down('KeyJ',{target:document.createElement('input')})
  down('KeyK',{ctrlKey:true}); down('Space',{isComposing:true})
  state.blocked=true; down('KeyK'); state.blocked=false
  state.focused=false; down('Space'); state.focused=true
  expect(seek).not.toHaveBeenCalled(); expect(togglePlay).not.toHaveBeenCalled()
  down('KeyK'); state.focused=false; vi.advanceTimersByTime(1000)
  expect(seek).toHaveBeenCalledTimes(1)
})
it('失焦、换素材或卸载均终止长按', () => {
  const {down, seek, state} = setup()
  down('KeyK'); window.dispatchEvent(new Event('blur')); vi.advanceTimersByTime(1000)
  expect(seek).toHaveBeenCalledTimes(1)
  down('KeyK'); state.kind='live'; vi.advanceTimersByTime(1000)
  expect(seek).toHaveBeenCalledTimes(2)
  state.kind='media'; down('KeyK'); wrapper.unmount(); vi.advanceTimersByTime(1000)
  expect(seek).toHaveBeenCalledTimes(3)
})

function mountPlayer() {
  const stage = reactive({
    kind: 'media', mediaId: 'test', mediaSrc: '', stageReady: true,
    currentTime: 30, duration: 120, playing: false,
    mediaOptions: [], rateOptions: [1], rate: 1,
    seek: vi.fn(t => { stage.currentTime = Number(t) }),
    togglePlay: vi.fn(), stepFrames: vi.fn(), setRate: vi.fn(),
    onMediaPick: vi.fn(), backToLive: vi.fn(),
  })
  const noop = () => {}
  wrapper = mount(ConsoleVideoStage, { attachTo: document.body, props: {
    stage, scriptFx: {tap: {show: false}, swipe: {show: false}, hit: {show: false}},
    loupe: {show: false}, onMouseDown: noop, onMouseMove: noop, onMouseUp: noop,
    onWheel: noop, onVideoMouseLeave: noop, flushAndConnect: noop, fullscreen: noop,
  } })
  return {stage, root: wrapper.element, slider: wrapper.get('.mc-seek').element}
}
function press(code) {
  document.activeElement.dispatchEvent(new KeyboardEvent('keydown', {code, bubbles: true}))
  document.activeElement.dispatchEvent(new KeyboardEvent('keyup', {code, bubbles: true}))
}
it('鼠标点击播放按钮后，空格和方向键继续控制播放器', async () => {
  const {stage, root} = mountPlayer()
  const button = wrapper.get('.mc-play')
  button.element.focus()
  await button.trigger('click', {detail: 1})
  expect(document.activeElement).toBe(root)
  press('Space'); press('ArrowRight')
  expect(stage.togglePlay).toHaveBeenCalledTimes(2)
  expect(stage.currentTime).toBe(35)
})
it('进度条拖动在控件外松手且没有 click 时，恢复焦点和视频快捷键', async () => {
  const {stage, root, slider} = mountPlayer()
  slider.focus()
  slider.dispatchEvent(new PointerEvent('pointerdown', {button: 0, pointerId: 7, bubbles: true}))
  slider.value = '60'
  slider.dispatchEvent(new Event('input', {bubbles: true}))
  document.body.dispatchEvent(new PointerEvent('pointerup', {pointerId: 7, bubbles: true}))
  await Promise.resolve()
  expect(document.activeElement).toBe(root)
  press('KeyJ'); press('Space')
  expect(stage.currentTime).toBe(55)
  expect(stage.togglePlay).toHaveBeenCalledOnce()
})
it('键盘操作进度条、播放按钮和鼠标点击下拉框不被抢走焦点', async () => {
  const {stage, slider} = mountPlayer()
  for (const selector of ['.mc-seek', '.mc-play', '.mc-rate', '.media-pick']) {
    const control = wrapper.get(selector)
    control.element.focus()
    await control.trigger('click', {detail: selector.includes('pick') || selector.includes('rate') ? 1 : 0})
    expect(document.activeElement).toBe(control.element)
  }
  slider.focus()
  press('KeyJ')
  expect(stage.seek).not.toHaveBeenCalled()
})
it('取消拖动或焦点已转移时，迟到的松手事件不抢焦点', async () => {
  const {slider} = mountPlayer()
  const down = () => slider.dispatchEvent(new PointerEvent('pointerdown', {button: 0, pointerId: 7, bubbles: true}))
  const up = () => window.dispatchEvent(new PointerEvent('pointerup', {pointerId: 7}))
  slider.focus(); down()
  window.dispatchEvent(new PointerEvent('pointercancel', {pointerId: 7}))
  up(); await Promise.resolve()
  expect(document.activeElement).toBe(slider)
  down()
  const select = wrapper.get('.mc-rate').element
  select.focus(); up(); await Promise.resolve()
  expect(document.activeElement).toBe(select)
})
