import { onMounted, onUnmounted, watch } from 'vue'
import { shouldIgnoreKeyboardTarget } from '../../keyboard-control'

const DIRECTIONS = { KeyJ: -1, ArrowLeft: -1, KeyK: 1, ArrowRight: 1 }
const STEP_SECONDS = 5

export function useMediaKeyboard({ stage, blocked, focused, report }) {
  let held = '', delay = null, repeat = null
  function release() {
    held = ''
    clearTimeout(delay)
    clearInterval(repeat)
  }
  function ready() {
    return stage()?.kind === 'media' && stage().stageReady && !blocked() && focused() && !document.hidden
  }
  function jump(direction) {
    if (!ready()) return release()
    const player = stage()
    const target = Math.max(0, Math.min(player.duration, player.currentTime + direction * STEP_SECONDS))
    const delta = target - player.currentTime
    player.seek(target)
    report(`${direction < 0 ? '后退' : '前进'} ${Math.abs(delta).toFixed(1)} 秒`)
  }
  function keydown(event) {
    const code = event.code || ({ ' ': 'Space', j: 'KeyJ', k: 'KeyK' }[event.key] || event.key)
    if (code !== 'Space' && !DIRECTIONS[code]) return
    if (!ready() || event.defaultPrevented || event.isComposing || event.ctrlKey || event.altKey || event.metaKey || event.shiftKey
      || shouldIgnoreKeyboardTarget(event.target)) return
    event.preventDefault()
    event.stopPropagation()
    // 自行调度长按，浏览器的 repeat 不叠加跳转；空格每次按下只切换一次。
    if (event.repeat || held === code) return
    release()
    held = code
    if (code === 'Space') {
      report(stage().playing ? '已暂停' : '播放')
      stage().togglePlay()
      return
    }
    jump(DIRECTIONS[code])
    delay = setTimeout(() => {
      if (!held) return
      jump(DIRECTIONS[code])
      if (held) repeat = setInterval(() => jump(DIRECTIONS[code]), 150)
    }, 350)
  }
  function keyup(event) {
    const code = event.code || ({ ' ': 'Space', j: 'KeyJ', k: 'KeyK' }[event.key] || event.key)
    if (code === held) release()
  }
  function visibility() { if (document.hidden) release() }
  watch(() => [stage()?.kind, stage()?.mediaId, blocked()], release)
  onMounted(() => {
    window.addEventListener('keyup', keyup)
    window.addEventListener('blur', release)
    document.addEventListener('visibilitychange', visibility)
  })
  onUnmounted(() => {
    release()
    window.removeEventListener('keyup', keyup)
    window.removeEventListener('blur', release)
    document.removeEventListener('visibilitychange', visibility)
  })
  return { keydown, keyup, release }
}
