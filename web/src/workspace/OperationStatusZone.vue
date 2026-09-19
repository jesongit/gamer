<template>
  <div ref="zone" class="operation-zone" :class="`${side}-zone`" :tabindex="message || statuses.length ? 0 : -1" :aria-label="side === 'core' ? '内核状态' : '插件状态'" :title="expanded ? '' : description" @mouseenter="reveal" @mouseleave="hideUnlessFocused" @focusin="reveal" @focusout="leaveFocus" @keydown.esc.stop="expanded = false">
    <div ref="clip" class="zone-clip">
      <div ref="content" class="zone-content">
        <OperationMessage :message="message" @perform="$emit('perform', $event)" />
        <span v-for="(status, index) in statuses" :key="index" class="core-status">{{ status }}</span>
      </div>
    </div>
    <div v-if="expanded && overflowing" class="status-expanded">
      <OperationMessage :message="message" @perform="$emit('perform', $event)" />
      <span v-for="(status, index) in statuses" :key="index" class="core-status">{{ status }}</span>
    </div>
  </div>
</template>
<script setup>
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import OperationMessage from './OperationMessage.vue'
const props = defineProps({ side: String, message: Object, statuses: { type: Array, default: () => [] } })
defineEmits(['perform'])
const zone = ref(null), clip = ref(null), content = ref(null), expanded = ref(false), overflowing = ref(false)
const description = computed(() => [props.message?.text, ...(props.message?.actions || []).map(a => a.label), ...props.statuses].filter(Boolean).join(' · '))
let observer
function measure() { overflowing.value = !!clip.value && Math.max(clip.value.scrollWidth, content.value?.scrollWidth || 0) > clip.value.clientWidth + 1; if (!overflowing.value) expanded.value = false }
function reveal() { measure(); expanded.value = overflowing.value }
function hideUnlessFocused() { if (!zone.value?.contains(document.activeElement)) expanded.value = false }
function leaveFocus(event) { if (!zone.value?.contains(event.relatedTarget)) expanded.value = false }
watch(() => [props.message, props.statuses], async () => { await nextTick(); measure() }, { deep: true })
onMounted(() => { if (typeof ResizeObserver !== 'undefined') { observer = new ResizeObserver(measure); observer.observe(clip.value); observer.observe(content.value) } measure() })
onUnmounted(() => observer?.disconnect())
</script>
<style scoped>
.operation-zone{min-width:0;position:relative;padding:0 10px;display:flex;align-items:center}.zone-clip{overflow:hidden;min-width:0;width:100%}.zone-content{display:flex;align-items:center;gap:12px;width:max-content;min-width:100%}.plugin-zone .zone-content{justify-content:flex-end;float:right}.core-status{white-space:nowrap;flex:none;color:var(--text-2);font-variant-numeric:tabular-nums}.zone-content>*+*::before{content:'·';margin-right:12px;color:var(--text-2)}
.status-expanded{position:absolute;bottom:100%;left:0;z-index:40;display:flex;align-items:center;flex-wrap:wrap;gap:8px 12px;width:max-content;max-width:min(640px,calc(100vw - 24px));max-height:35vh;overflow:auto;padding:7px 10px;background:var(--bg-2);border:1px solid var(--border);box-shadow:0 4px 16px #0005}.plugin-zone .status-expanded{left:auto;right:0}.status-expanded :deep(.operation-content){flex-wrap:wrap;white-space:normal;overflow-wrap:anywhere;min-width:0;max-width:100%}.status-expanded .core-status{white-space:normal}
</style>
