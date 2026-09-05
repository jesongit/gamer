<template>
  <div v-if="events.list.length" class="run-events" data-testid="run-events">
    <div ref="listBox" class="re-list mono">
      <div v-for="(e, i) in events.list" :key="i" class="re" :class="rowClass(e)">
        <span class="re-time">{{ e.time }}</span>
        <span class="re-tag">{{ tag(e) }}</span>
        <span class="re-msg">{{ text(e) }}</span>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * 运行事件 feed（P12.6 / 契约 §6）：滚动展示最近 N 条 se 运行结构事件
 * （step_start/step_end 带路径与 desc，call_start 带目标与深度，vision 带
 * 匹配结果，budget/run_end 带终止原因；budget 与失败行高亮）。数据源是
 * useRunEvents 单例（Console 壳 onControlMessage → pushRunEvent 分发）。
 * 新事件在顶部追加（与 RunLogPanel 同向），vision 静默期不占行。
 */
import { nextTick, reactive, ref, watch } from 'vue'

const props = defineProps({ events: { type: Object, required: true } })
const events = reactive(props.events)
const listBox = ref(null)

function rowClass(e) {
  if (e.ev === 'budget') return 'warn'
  if (e.ev === 'run_end' && !e.ok) return 'error'
  if (e.ev === 'step_end' && !e.ok) return 'error'
  if (e.ev === 'run_start') return 'info'
  return ''
}

function tag(e) {
  switch (e.ev) {
    case 'run_start': return '运行'
    case 'run_end': return '运行'
    case 'step_start': return '步骤'
    case 'step_end': return '步骤'
    case 'call_start': return '调用'
    case 'vision': return '匹配'
    case 'budget': return '预算'
    default: return e.ev
  }
}

function text(e) {
  switch (e.ev) {
    case 'run_start': return '运行开始'
    case 'run_end': return e.ok ? '运行结束' : `运行失败：${e.error || '未知错误'}`
    case 'step_start': return `${e.path || ''} ${e.desc || ''}`.trim()
    case 'step_end': return e.ok ? `完成 ${e.path}` : `失败 ${e.path}：${e.error || ''}`
    case 'call_start': return `${e.target}（深度 ${e.depth}）`
    case 'vision': {
      if (!e.found) return `${e.template} 未命中`
      const score = Number(e.score || 0).toFixed(2)
      return `${e.template} 命中 score=${score}`
    }
    case 'budget': return `预算终止：${e.kind}`
    default: return ''
  }
}

watch(() => events.list.length, () => {
  nextTick(() => {
    if (listBox.value) listBox.value.scrollTop = 0
  })
})
</script>

<style scoped>
.run-events{flex:none;max-height:150px;min-height:0;display:flex;flex-direction:column;background:var(--bg-0);border:1px solid var(--border);border-radius:var(--radius-sm);padding:6px 8px}
.re-list{flex:1;min-height:0;overflow:auto;display:flex;flex-direction:column;gap:2px}
.re{display:flex;gap:8px;font-size:11px;line-height:1.5;align-items:baseline}
.re-time{color:var(--text-2);flex-shrink:0}
.re-tag{flex-shrink:0;color:var(--text-2);border:1px solid var(--border);border-radius:3px;padding:0 4px;font-size:10px;line-height:1.4}
.re-msg{flex:1;min-width:0;color:var(--text-1);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.re.warn .re-msg,.re.warn .re-tag{color:var(--warn);border-color:var(--warn)}
.re.error .re-msg,.re.error .re-tag{color:var(--danger);border-color:var(--danger)}
.mono{font-family:var(--mono)}
</style>
