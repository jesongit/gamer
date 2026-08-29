<template>
  <div class="error-summary" data-testid="error-summary">
    <div class="summary-head">校验结果{{ diagnostics.length ? `（${diagnostics.length}）` : '' }}</div>
    <div v-if="diagnostics.length === 0" class="summary-empty">无错误</div>
    <button
      v-for="(d, i) in diagnostics"
      :key="i"
      type="button"
      class="err-row"
      :title="`定位到 ${d.step_path || '(全局)'}`"
      @click.stop="emit('locate', d)"
    >
      <span class="err-code">{{ d.code }}</span>
      <span class="err-path">{{ d.step_path || '(全局)' }}</span>
      <span class="err-msg">{{ d.message }}</span>
    </button>
  </div>
</template>

<script setup lang="ts">
/**
 * 错误摘要（plan §8.3 右侧面板 / 阶段 3 验收项）：
 * - 列出诊断（code + step_path + message），点击行 emit('locate', diagnostic)；
 * - 接受外部传入 diagnostics——客户端 validateScript/validateSource 与服务端结构化错误
 *   同构（契约 §5：code/message/resource/step_path/field），滚动与高亮由宿主（画布/页面）联动；
 * - 禁止解析中文文案定位，只消费结构化字段。
 */
import type { PropType } from 'vue'
import type { Diagnostic } from '../diagnostics'

defineProps({
  diagnostics: { type: Array as PropType<Diagnostic[]>, default: () => [] },
})

const emit = defineEmits(['locate'])
</script>

<style scoped>
.error-summary {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  margin-top: 8px;
  overflow: hidden;
}
.summary-head {
  padding: 6px 10px; font-size: 12px; font-weight: 600; color: var(--text-1);
  border-bottom: 1px solid var(--border);
}
.summary-empty { padding: 10px; font-size: 12px; color: var(--ok); }
.err-row {
  display: flex; align-items: baseline; gap: 8px;
  width: 100%; text-align: left;
  border: none; border-bottom: 1px solid var(--border);
  background: transparent; color: var(--text-0);
  padding: 6px 10px; cursor: pointer; font-size: 12px;
}
.err-row:last-child { border-bottom: none; }
.err-row:hover { background: var(--bg-3); }
.err-code {
  font-family: var(--mono); font-size: 11px; color: var(--danger);
  background: rgba(248, 113, 113, .12); border-radius: 4px; padding: 1px 6px; flex: none;
}
.err-path { font-family: var(--mono); font-size: 11px; color: var(--accent-2); flex: none; }
.err-msg { color: var(--text-1); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
