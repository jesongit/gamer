<template>
  <Teleport to="body">
    <div v-if="confirmation" class="confirmation-mask" @click.self="finishConfirmation(false)">
      <section ref="dialog" class="confirmation-dialog" role="dialog" aria-modal="true" aria-labelledby="confirmation-title" aria-describedby="confirmation-message" tabindex="-1">
        <header><h2 id="confirmation-title">{{ confirmation.title }}</h2><button type="button" class="btn btn-icon btn-ghost" aria-label="关闭确认框" @click="finishConfirmation(false)"><UiIcon name="close" /></button></header>
        <div class="confirmation-body">
          <p v-if="confirmation.message" id="confirmation-message">{{ confirmation.message }}</p>
          <dl v-if="confirmation.fields?.length" class="confirmation-facts"><template v-for="(field, index) in confirmation.fields" :key="index"><dt>{{ field.label }}</dt><dd>{{ field.value }}</dd></template></dl>
          <section v-for="(section, index) in confirmation.sections || []" :key="index" class="confirmation-section" :class="section.tone"><h3>{{ section.title }}</h3><p>{{ section.text }}</p></section>
          <p v-if="confirmation.warning" class="confirmation-warning">{{ confirmation.warning }}</p>
        </div>
        <footer><button ref="cancelButton" type="button" class="btn" @click="finishConfirmation(false)">{{ confirmation.cancelText || '取消' }}</button><button type="button" class="btn" :class="confirmation.danger ? 'btn-danger' : 'btn-primary'" @click="finishConfirmation(true)">{{ confirmation.confirmText || '确认' }}</button></footer>
      </section>
    </div>
  </Teleport>
</template>

<script setup>
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import UiIcon from './UiIcon.vue'
import { confirmation, finishConfirmation } from './useConfirmDialog'
const dialog = ref(null), cancelButton = ref(null)
let previousFocus = null
let blocked = []
function restoreBackground() {
  for (const [element, inert] of blocked) element.inert = inert
  blocked = []
  if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true })
  previousFocus = null
}
watch(confirmation, async request => {
  if (!request) { restoreBackground(); return }
  previousFocus = document.activeElement
  await nextTick()
  if (confirmation.value !== request || !dialog.value) return
  for (const element of document.body.children) {
    if (element.contains(dialog.value) || ['SCRIPT', 'STYLE', 'LINK'].includes(element.tagName)) continue
    blocked.push([element, element.inert]); element.inert = true
  }
  cancelButton.value?.focus({ preventScroll: true })
}, { flush: 'post', immediate: true })
function keydown(event) {
  if (!confirmation.value) return
  // Stop underlying modal/console keyboard handlers from consuming the same key.
  event.stopImmediatePropagation()
  if (event.key === 'Escape') { event.preventDefault(); finishConfirmation(false) }
  if (event.key === 'Tab') {
    const controls = [...dialog.value.querySelectorAll('button:not(:disabled)')]
    const index = controls.indexOf(document.activeElement)
    if (event.shiftKey && index <= 0) { event.preventDefault(); controls.at(-1)?.focus() }
    else if (!event.shiftKey && (index < 0 || index === controls.length - 1)) { event.preventDefault(); controls[0]?.focus() }
  }
}
function retainFocus(event) { if (confirmation.value && dialog.value && !dialog.value.contains(event.target)) cancelButton.value?.focus() }
onMounted(() => { window.addEventListener('keydown', keydown, true); window.addEventListener('focusin', retainFocus, true) })
onUnmounted(() => { window.removeEventListener('keydown', keydown, true); window.removeEventListener('focusin', retainFocus, true); finishConfirmation(false); restoreBackground() })
</script>

<style scoped>
.confirmation-mask{position:fixed;inset:0;z-index:2000;background:rgba(0,0,0,.64);display:flex;align-items:center;justify-content:center;padding:16px}
.confirmation-dialog{width:480px;max-width:100%;max-height:calc(100dvh - 32px);display:flex;flex-direction:column;background:var(--bg-1);border:1px solid var(--control-border,var(--border));border-radius:4px;box-shadow:0 18px 70px #0008;color:var(--text-0);outline:none}
header{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:10px 16px;border-bottom:1px solid var(--border)}h2{margin:0;font-size:15px;font-weight:600}
.confirmation-body{padding:16px;overflow:auto;min-height:0;display:flex;flex-direction:column;gap:14px;font-size:13px;line-height:1.65;overflow-wrap:anywhere}p{margin:0;white-space:pre-wrap}
.confirmation-facts{display:grid;grid-template-columns:76px minmax(0,1fr);gap:7px 12px;margin:0;padding:12px;background:var(--bg-0);border:1px solid var(--border);border-radius:3px}dt{color:var(--text-2)}dd{margin:0;white-space:pre-wrap;color:var(--text-0)}
.confirmation-section h3{font-size:12px;font-weight:500;color:var(--text-2);margin:0 0 5px}.confirmation-section.warn p{color:var(--warn)}.confirmation-warning{padding:8px 10px;border-left:2px solid var(--warn);background:color-mix(in srgb,var(--warn) 5%,transparent);color:var(--text-1);font-size:12px}
footer{display:flex;justify-content:flex-end;gap:8px;padding:10px 16px;border-top:1px solid var(--border)}footer .btn{min-width:76px;height:28px;justify-content:center;font-size:13px}
</style>
