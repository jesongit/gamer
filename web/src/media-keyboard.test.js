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

function setupPlayer() {
  const stage = reactive({
    kind: 'media', mediaId: 'video', mediaSrc: '/clip.mp4', stageReady: true,
    currentTime: 30, duration: 120, playing: false,
    mediaOptions: [{ id: 'video', name: 'clip.mp4' }], rate: 1, rateOptions: [1, 2],
    seek: vi.fn(time => { stage.currentTime = Number(time) }),
    togglePlay: vi.fn(() => { stage.playing = !stage.playing }),
    stepFrames: vi.fn(), setRate: vi.fn(), onMediaPick: vi.fn(),
    backToLive: vi.fn(() => { stage.kind = 'live' }),
  })
  const noop = vi.fn()
  wrapper = mount(ConsoleVideoStage, { attachTo: document.body, props: {
    stage,
    scriptFx: { tap: { show: false }, swipe: { show: false }, hit: { show: false } },
    loupe: { show: false },
    onMouseDown: noop, onMouseMove: noop, onMouseUp: noop, onWheel: noop,
    onVideoMouseLeave: noop, flushAndConnect: noop, fullscreen: noop,
  } })
  const press = code => {
    const target = document.activeElement
    const event = new KeyboardEvent('keydown', { code, bubbles: true, cancelable: true })
    target.dispatchEvent(event)
    target.dispatchEvent(new KeyboardEvent('keyup', { code, bubbles: true }))
    return event
  }
  return { stage, press }
}

it.each(['.mc-seek', '.mc-play', '[aria-label="上一帧"]', '[aria-label="下一帧"]', '[aria-label="视频全屏"]'])(
  '鼠标操作 %s 后，焦点回到播放器且方向键 / J / K / 空格继续生效', async selector => {
    const { stage, press } = setupPlayer()
    const control = wrapper.get(selector)
    // 浏览器会先把焦点交给鼠标点击的原生控件；DOM 模拟器需显式执行。
    control.element.focus()
    if (selector === '.mc-seek') {
      await control.setValue('60')
      expect(stage.seek).toHaveBeenLastCalledWith('60')
    }
    const clickTarget = control.find('svg').exists() ? control.get('svg') : control
    await clickTarget.trigger('click', { detail: 1 })
    if (selector === '.mc-play') expect(stage.togglePlay).toHaveBeenCalledTimes(1)
    if (selector.includes('上一帧')) expect(stage.stepFrames).toHaveBeenCalledWith(-1)
    if (selector.includes('下一帧')) expect(stage.stepFrames).toHaveBeenCalledWith(1)
    expect(document.activeElement).toBe(wrapper.element)
    const start = stage.currentTime
    stage.seek.mockClear(); stage.togglePlay.mockClear()
    for (const code of ['ArrowLeft', 'ArrowRight', 'KeyJ', 'KeyK', 'Space']) {
      expect(press(code).defaultPrevented).toBe(true)
    }
    expect(stage.seek.mock.calls).toEqual([[start - 5], [start], [start - 5], [start]])
    expect(stage.togglePlay).toHaveBeenCalledTimes(1)
  },
)

it.each(['.media-pick', '.mc-rate'])('鼠标打开 %s 保留下拉框焦点和原生按键', async selector => {
  const { stage, press } = setupPlayer()
  const select = wrapper.get(selector)
  select.element.focus()
  await select.trigger('click', { detail: 1 })
  expect(document.activeElement).toBe(select.element)
  expect(press('ArrowRight').defaultPrevented).toBe(false)
  expect(press('Space').defaultPrevented).toBe(false)
  expect(stage.seek).not.toHaveBeenCalled()
  expect(stage.togglePlay).not.toHaveBeenCalled()
})

it('Tab 聚焦控制条后保留控件原生键盘操作，不把键盘激活当作鼠标点击', async () => {
  const { stage, press } = setupPlayer()
  const seek = wrapper.get('.mc-seek')
  seek.element.focus()
  expect(press('ArrowRight').defaultPrevented).toBe(false)
  expect(document.activeElement).toBe(seek.element)
  const play = wrapper.get('.mc-play')
  play.element.focus()
  expect(press('Space').defaultPrevented).toBe(false)
  await play.trigger('click', { detail: 0 })
  expect(document.activeElement).toBe(play.element)
  expect(stage.togglePlay).toHaveBeenCalledTimes(1)
  expect(stage.seek).not.toHaveBeenCalled()
})

it('返回实时投屏后不抢回媒体焦点或处理播放快捷键', async () => {
  const { stage, press } = setupPlayer()
  const live = wrapper.get('.media-live-btn')
  live.element.focus()
  await live.trigger('click', { detail: 1 })
  expect(stage.backToLive).toHaveBeenCalledTimes(1)
  expect(document.activeElement).not.toBe(wrapper.element)
  press('Space'); press('ArrowRight')
  expect(stage.togglePlay).not.toHaveBeenCalled()
  expect(stage.seek).not.toHaveBeenCalled()
})
