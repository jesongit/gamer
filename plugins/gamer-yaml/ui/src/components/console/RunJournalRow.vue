<template>
  <tr class="journal-row" :class="[state, { selected }]" @click="$emit('select', node.id)">
    <td class="row-time"><time>{{ timeText(node.time) }}</time></td>
    <td class="row-content">
      <button class="row-action" :aria-pressed="selected" @click.stop="$emit('select', node.id)">{{ title }}</button>
      <span v-if="node.iteration" class="iteration-label">第 {{ node.iteration }} 轮</span>
      <p v-if="result" class="row-result" :title="result">{{ result }}</p>
    </td>
    <td class="row-state"><span :class="state">{{ rowStateText(state) }}</span></td>
    <td class="row-duration">{{ runDuration(node, now) }}</td>
  </tr>
</template>
<script setup>
import { computed } from 'vue'
import { eventText, rowStateText, runDuration, runRowState, stepSummary, timeText } from './run-journal'
const props = defineProps({ node: { type: Object, required: true }, selected: Boolean, now: Number })
defineEmits(['select'])
const state = computed(() => runRowState(props.node))
const title = computed(() => props.node.kind === 'step' ? props.node.desc || props.node.path : eventText(props.node))
const result = computed(() => props.node.kind === 'step' ? props.node.error || stepSummary(props.node) : '')
</script>
<style scoped>
.journal-row{cursor:pointer}.journal-row td{padding:10px 8px;border-bottom:1px solid var(--border);vertical-align:top;background:var(--bg-1)}.journal-row:nth-child(even) td{background:var(--bg-0)}.journal-row:hover td{background:var(--bg-2)}.journal-row.selected td{background:color-mix(in srgb,var(--accent) 10%,var(--bg-1))}.journal-row.selected td:first-child{box-shadow:inset 3px 0 var(--accent)}.row-time,.row-duration{font-size:11px;color:var(--text-2);font-variant-numeric:tabular-nums;white-space:nowrap;line-height:22px}.row-duration{text-align:right}.row-action{border:0;background:transparent;padding:0;color:var(--text-0);font:inherit;font-size:13px;line-height:22px;text-align:left;cursor:pointer;overflow-wrap:anywhere;max-width:100%}.row-action:focus-visible{outline:2px solid var(--accent);outline-offset:2px}.row-result{font-size:12px;line-height:1.6;color:var(--text-1);white-space:pre-wrap;overflow-wrap:anywhere;display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:2;overflow:hidden;margin-top:2px}.failed .row-result,.error .row-result{color:var(--danger)}.row-state span{display:inline-block;white-space:nowrap;font-size:11px;line-height:22px;color:var(--text-2)}.row-state .success{color:var(--ok)}.row-state .failed,.row-state .error{color:var(--danger)}.row-state .running,.row-state .warning{color:var(--warn)}.iteration-label{font-size:11px;color:var(--text-2);margin-left:8px;white-space:nowrap}
</style>
