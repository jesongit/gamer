<template>
  <aside class="journal-inspector" aria-label="所选记录详情">
    <header class="inspector-header"><strong>{{ node.desc || eventText(node) }}</strong><span :class="runRowState(node)">{{ rowStateText(runRowState(node)) }} · {{ runDuration(node, now) }}</span><button v-if="node.trace?.source" class="btn btn-sm" @click="$emit('locate', node.failure || node)">定位步骤</button><button class="btn btn-ghost btn-sm" aria-label="关闭记录详情" @click="$emit('close')">关闭</button></header>
    <div class="inspector-tabs" role="group" aria-label="记录详情分类">
      <button v-for="item in tabs" :key="item.key" class="inspector-tab" :class="{ active: tab === item.key }" :aria-pressed="tab === item.key" @click="tab = item.key">{{ item.label }}</button>
    </div>
    <div class="inspector-body">
      <template v-if="tab === 'overview'">
        <div v-if="node.error" class="inspector-error">{{ node.error }}</div>
        <p v-if="summary" class="detail-summary">{{ summary }}</p>
        <dl class="source-fields">
          <template v-if="row.context"><dt>执行上下文</dt><dd>{{ row.context }}</dd></template>
          <template v-if="node.path"><dt>步骤位置</dt><dd>{{ node.path }}</dd></template>
          <template v-if="node.trace?.source"><dt>来源</dt><dd>{{ node.trace.source.package_id }} / {{ node.trace.source.path }}<span v-if="node.trace.source.function"> · {{ node.trace.source.function }}</span></dd></template>
          <dt>开始时间</dt><dd>{{ timeText(node.time) }}</dd>
        </dl>
        <div v-for="log in node.logs || []" :key="log.id" class="inspector-log" :class="log.data?.level"><time>{{ timeText(log.time) }}</time><span>{{ log.data?.level || 'info' }} · {{ log.data?.message }}</span></div>
      </template>
      <template v-else-if="tab === 'parameters'">
        <section v-for="detail in node.details || []" :key="detail.id" class="parameter-record"><strong>{{ detailText(detail.name) }}</strong><pre>{{ formatRunValue(detail.data) }}</pre></section>
        <p v-if="!node.details?.length" class="inspector-empty">没有参数或返回值记录。</p>
      </template>
      <template v-else>
        <div v-for="match in node.matches || []" :key="match.id" class="match-record"><time>{{ timeText(match.time) }}</time><strong>{{ match.template }}</strong><span :class="match.found ? 'success' : ''">{{ match.found ? `命中 · ${Number(match.score || 0).toFixed(3)}` : '未命中' }}</span><span v-if="match.center">坐标 {{ match.center.join(', ') }}</span></div>
        <p v-if="!node.matches?.length" class="inspector-empty">没有匹配记录。</p>
      </template>
    </div>
  </aside>
</template>
<script setup>
import { computed, ref, watch } from 'vue'
import { detailText, eventText, formatRunValue, rowStateText, runDuration, runRowState, stepSummary, timeText } from './run-journal'
const props = defineProps({ row: { type: Object, required: true }, now: Number })
defineEmits(['close', 'locate'])
const node = computed(() => props.row.node)
const tab = ref('overview')
const tabs = computed(() => [{ key: 'overview', label: '详情' }, { key: 'parameters', label: `参数与返回值 (${node.value.details?.length || 0})` }, { key: 'matches', label: `匹配记录 (${node.value.matches?.length || 0})` }])
const summary = computed(() => node.value.kind === 'step' ? stepSummary(node.value) : eventText(node.value))
watch(() => node.value.id, () => { tab.value = 'overview' })
</script>
<style scoped>
.journal-inspector{display:flex;flex-direction:column;flex:0 1 42%;min-height:130px;max-height:45%;border:1px solid var(--border);border-top:2px solid var(--accent);border-radius:4px;background:var(--bg-0);overflow:hidden}.inspector-header{display:flex;align-items:center;gap:8px;flex-wrap:wrap;padding:8px 10px}.inspector-header strong{flex:1;min-width:80px;font-size:13px;overflow-wrap:anywhere}.inspector-header>span{font-size:12px;color:var(--text-2)}.inspector-tabs{display:flex;gap:12px;border-bottom:1px solid var(--border);padding:0 10px;flex-wrap:wrap}.inspector-tab{font:inherit;font-size:12px;cursor:pointer;background:none;border:0;border-bottom:2px solid transparent;color:var(--text-2);padding:7px 0}.inspector-tab.active{color:var(--accent);border-bottom-color:var(--accent)}.inspector-tab:focus-visible{outline:2px solid var(--accent);outline-offset:2px}.inspector-body{min-height:0;overflow:auto;padding:10px;font-size:12px;line-height:1.7;overflow-wrap:anywhere}.source-fields{display:grid;grid-template-columns:72px minmax(0,1fr);gap:5px 8px;margin:8px 0}.source-fields dt,time{color:var(--text-2)}.source-fields dd{white-space:pre-wrap}.detail-summary,.inspector-error,.inspector-log{white-space:pre-wrap}.inspector-error,.inspector-header .failed,.inspector-header .error,.inspector-log.error{color:var(--danger)}.inspector-log.warn,.inspector-header .warning{color:var(--warn)}.inspector-log{display:flex;gap:8px;padding:5px 0;border-top:1px solid var(--border)}.inspector-log time{flex:none}.parameter-record+.parameter-record{margin-top:10px}.parameter-record strong{color:var(--text-1);font-weight:500}.parameter-record pre{white-space:pre-wrap;overflow-wrap:anywhere;padding:6px 8px;background:var(--bg-1);border-radius:4px;font-size:12px;font-family:var(--mono)}.match-record{display:flex;gap:8px;flex-wrap:wrap;padding:6px 0;border-bottom:1px solid var(--border)}.match-record strong{font-weight:500;flex:1;min-width:80px}.success,.inspector-header .success{color:var(--ok)}.inspector-empty{color:var(--text-2)}
</style>
