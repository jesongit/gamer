<template>
  <div v-if="ctx.store.running" ref="logBox" class="live-logs script-logs mono">
    <div v-for="(l, i) in ctx.liveLogs" :key="i" class="ll" :class="l.level"><span class="ll-time">{{ l.time }}</span><span class="ll-msg">{{ l.msg }}</span></div>
  </div>
</template>

<script setup>
import { nextTick, onMounted, reactive, ref, watch } from 'vue'
const props = defineProps({ context: { type: Object, required: true }, onMounted: { type: Function, required: true } })
const ctx = reactive(props.context)
const logBox = ref(null)
async function emitRef() { await nextTick(); props.onMounted(logBox.value) }
watch(() => ctx.store.running, emitRef)
onMounted(emitRef)
</script>

<style scoped>
.live-logs{max-height:180px;overflow:auto;background:var(--bg-0);border:1px solid var(--border);border-radius:var(--radius-sm);padding:8px;display:flex;flex-direction:column;gap:3px}.live-logs.script-logs{max-height:none;flex:1;min-height:120px}.ll{display:flex;gap:8px;font-size:11px;line-height:1.5}.ll-time{color:var(--text-2);flex-shrink:0}.ll.info .ll-msg{color:var(--text-1)}.ll.success .ll-msg{color:var(--ok)}.ll.warn .ll-msg{color:var(--warn)}.ll.error .ll-msg{color:var(--danger)}.mono{font-family:var(--mono);font-size:11px;color:var(--text-1)}
</style>
