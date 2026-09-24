import { onUnmounted, ref, watch } from 'vue'

// 像素/毫秒决定每像素跨越秒数；同样距离快划比慢拖跨越更多时间。
export function mediaDragTarget(start, dx, elapsed, duration) {
  const speed = Math.min(2, Math.abs(dx) / Math.max(16, elapsed))
  const delta = Math.max(-120, Math.min(120, dx * (0.025 + speed * 0.1)))
  return Math.max(0, Math.min(duration, start + delta))
}

export function useMediaGestures(props) {
  const feedback = ref('')
  let drag = null, timer = null
  const blocked = () => props.selectionMode || props.selecting
  function clearDrag() {
    drag = null
    window.removeEventListener('mousemove', move)
    window.removeEventListener('mouseup', up)
    window.removeEventListener('blur', cancel)
  }
  function cancel() { clearDrag(); feedback.value = ''; clearTimeout(timer) }
  function show(text) {
    clearTimeout(timer)
    feedback.value = text
    timer = setTimeout(() => { feedback.value = '' }, 900)
  }
  function move(e) {
    if (!drag) return
    if (blocked() || props.stage?.mediaId !== drag.id) return cancel()
    const dx = e.clientX - drag.x, dy = e.clientY - drag.y
    if (!drag.moved && Math.hypot(dx, dy) < 6) return
    if (!drag.moved) drag.horizontal = Math.abs(dx) > Math.abs(dy)
    drag.moved = true
    if (!drag.horizontal) return
    e.preventDefault()
    const target = mediaDragTarget(drag.start, dx, performance.now() - drag.time, props.stage.duration)
    props.stage.seek(target)
    const delta = target - drag.start
    show(`${delta < 0 ? '后退' : '前进'} ${Math.abs(delta).toFixed(1)} 秒`)
  }
  function up(e) {
    if (!drag || e.button !== 0) return
    const click = !drag.moved && Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 6
    const valid = !blocked() && props.stage?.mediaId === drag.id
    clearDrag()
    if (click && valid) {
      show(props.stage.playing ? '已暂停' : '播放')
      props.stage.togglePlay()
    }
  }
  function down(e) {
    if (blocked()) return props.onMouseDown(e)
    if (e.button !== 0 || props.stage?.kind !== 'media' || !props.stage.stageReady) return
    cancel()
    drag = { id: props.stage.mediaId, x: e.clientX, y: e.clientY, time: performance.now(), start: props.stage.currentTime, moved: false }
    window.addEventListener('mousemove', move)
    window.addEventListener('mouseup', up)
    window.addEventListener('blur', cancel)
    e.preventDefault()
  }
  function hover(e) { if (!drag) props.onMouseMove(e) }
  function release(e) { if (!drag) props.onMouseUp(e) }
  watch(() => [props.stage?.kind, props.stage?.mediaId, blocked()], cancel)
  onUnmounted(cancel)
  return { feedback, show, down, hover, release }
}
