<template>
  <footer ref="bar" class="operation-bar" aria-label="当前操作反馈" :style="{ gridTemplateColumns: `minmax(0, ${split}fr) 6px minmax(0, ${100 - split}fr)` }">
    <OperationStatusZone side="core" :message="core" :statuses="coreStatuses" @perform="perform" />
    <div class="operation-divider" role="separator" tabindex="0" aria-label="调整操作反馈分区，双击恢复均分" aria-orientation="vertical" :aria-valuenow="split" :aria-valuemin="20" :aria-valuemax="80" @pointerdown="start" @dblclick="resetSplit" @keydown="keyAdjust"></div>
    <OperationStatusZone side="plugin" :message="plugin" @perform="perform" />
    <span v-if="copyError" class="copy-error" role="alert">复制失败，请检查剪贴板权限</span>
    <span v-else-if="copied" class="copy-confirm" role="status">已复制</span>
    <Teleport to="body"><div v-if="detail !== null" class="modal-mask" @click.self="detail = null" @keydown.esc.stop="detail = null"><section ref="detailPanel" class="modal status-detail" role="dialog" aria-modal="true" aria-label="操作详情" tabindex="-1"><div class="modal-head"><span class="title">操作详情</span><button class="btn btn-icon" aria-label="关闭操作详情" @click="detail = null">×</button></div><div class="modal-body"><pre>{{ detail }}</pre></div><div class="modal-foot"><button class="btn" @click="perform({ copy: detail })">复制</button><button class="btn" @click="detail = null">关闭</button></div></section></div></Teleport>
  </footer>
</template>
<script setup>
import { nextTick, onUnmounted, ref, watch } from 'vue'
import OperationStatusZone from './OperationStatusZone.vue'
defineProps({ core: Object, coreStatuses: { type: Array, default: () => [] }, plugin: Object })
const STORAGE_KEY = 'gamer.operationStatusSplit'
const clamp = value => Math.max(20, Math.min(80, value))
function initialSplit() { try { const raw = localStorage.getItem(STORAGE_KEY); const value = Number(raw); return raw !== null && Number.isFinite(value) ? clamp(value) : 50 } catch { return 50 } }
const bar = ref(null), split = ref(initialSplit()), copyError = ref(false)
const copied = ref(false)
const detail = ref(null)
const detailPanel = ref(null)
let detailTrigger
watch(detail, async (value, previous) => {
  if (value !== null) {
    if (previous === null) detailTrigger = document.activeElement
    await nextTick(); detailPanel.value?.focus()
  } else { detailTrigger?.focus?.(); detailTrigger = null }
})
let timer, pointer = null
function saveSplit() { try { localStorage.setItem(STORAGE_KEY, String(split.value)) } catch {} }
function resetSplit() { split.value = 50; saveSplit() }
async function perform(action) {
  if (action.copy !== undefined) {
    try { await navigator.clipboard.writeText(action.copy); copyError.value = false; copied.value = true; clearTimeout(timer); timer = setTimeout(() => { copied.value = false }, 1500) }
    catch { copied.value = false; copyError.value = true; clearTimeout(timer); timer = setTimeout(() => { copyError.value = false }, 3000) }
  } else if (action.detail !== undefined) detail.value = action.detail
  else { try { await action.run?.() } catch (error) { detail.value = `操作失败：${error?.message || error}` } }
}
function move(event) {
  if (event.pointerId !== pointer) return
  const rect = bar.value.getBoundingClientRect()
  split.value = clamp((event.clientX - rect.left) / rect.width * 100)
}
function stop(event) { if (event?.pointerId !== undefined && pointer !== event.pointerId) return; if (pointer !== null) saveSplit(); pointer = null; window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', stop); window.removeEventListener('pointercancel', stop) }
function start(event) {
  if (event.button !== 0) return
  event.currentTarget?.focus?.()
  pointer = event.pointerId; event.preventDefault()
  window.addEventListener('pointermove', move); window.addEventListener('pointerup', stop); window.addEventListener('pointercancel', stop)
}
function keyAdjust(event) {
  if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
  event.preventDefault()
  split.value = event.key === 'Home' ? 20 : event.key === 'End' ? 80 : clamp(split.value + (event.key === 'ArrowLeft' ? -2 : 2))
  saveSplit()
}
onUnmounted(() => { stop(); clearTimeout(timer) })
</script>
<style scoped>
.operation-bar{height:29px;flex:none;display:grid;background:var(--chrome);border-top:1px solid var(--border);position:relative;font-size:12px;color:var(--text-1)}
button{border:0;background:none;color:inherit;font:inherit;padding:2px;cursor:pointer;text-decoration:underline;text-decoration-color:var(--border);text-underline-offset:3px}button:hover{color:var(--accent)}.operation-divider{cursor:col-resize;touch-action:none;border-left:1px solid var(--border);margin:5px 0}.operation-divider:hover,.operation-divider:focus{border-color:var(--accent)}.copy-error{position:absolute;bottom:34px;left:10px;background:var(--bg-2);padding:6px 9px;border:1px solid var(--danger);color:var(--danger)}
.status-detail{width:560px}.status-detail pre{white-space:pre-wrap;overflow-wrap:anywhere;font:13px/1.6 var(--mono);color:var(--text-0)}
.copy-confirm{position:absolute;bottom:32px;left:10px;background:var(--bg-2);padding:3px 8px;border:1px solid var(--border);color:var(--ok);pointer-events:none}
</style>
