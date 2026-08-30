<template>
  <div class="script-summary" data-testid="script-summary">
    <div class="sum-head mono">{{ headLabel }}</div>
    <div v-if="!model" class="sum-empty">{{ error || '请选择脚本' }}</div>
    <div v-else-if="!topSteps.length" class="sum-empty">空脚本（无步骤）</div>
    <div v-else class="sum-list">
      <div
        v-for="(row, i) in topSteps"
        :key="row.uuid"
        class="sum-row"
      >
        <span class="idx mono">{{ i + 1 }}</span>
        <span class="icon" :title="row.meta.hint">{{ row.meta.icon }}</span>
        <span class="label">{{ row.meta.label }}</span>
        <span class="summary mono">{{ row.summary }}</span>
        <span v-if="row.target" class="row-ops">
          <button class="mini-btn link" type="button" :title="row.kind === 'call' ? '打开子脚本' : '打开函数定义'" @click.stop="emit('open-target', { kind: row.kind, target: row.target })">↗ {{ row.kind === 'call' ? '子脚本' : '函数' }}</button>
        </span>
        <button class="mini-btn run" type="button" title="从此步骤运行（顶层）" @click.stop="emit('run-from', row.uuid)">▶ 从此运行</button>
      </div>
    </div>
    <div class="run-hint">▶ 从此运行：直接从该步骤开始运行（顶部「运行」按钮从头跑）。call/func 卡片可打开目标。</div>
  </div>
</template>

<script setup>
/**
 * 只读步骤摘要列表（plan §10.1 Console 紧凑外壳非编辑态）：替代旧「只读源码 + 行点击」。
 * - 逐顶层卡片显示动作图标 + 中文动作名 + 自然语言摘要（kinds.stepSummary 同源）；
 * - 运行起点只经卡片「▶ 从此运行」发起（2026-08-30 用户决策：去掉点击卡片选中/取消，
 *   从此运行按钮已覆盖该场景）；嵌套分支不展开、不提供运行入口；
 * - call/func 卡片提供「打开子脚本/打开函数定义」结构化跳转入口（emit open-target）。
 */
import { computed } from 'vue'
import { KIND_META, stepSummary } from '../../script-editor/components/kinds'

const props = defineProps({
  model: { type: Object, default: null }, // ScriptModel（已分配 uuid；解析失败时可能为空壳）
  /** 解析失败等摘要不可用原因（显示在空态，替代旧源码视图的报错入口）。 */
  error: { type: String, default: '' },
})

const emit = defineEmits(['run-from', 'open-target'])

const topSteps = computed(() => {
  const m = props.model
  if (!m || !Array.isArray(m.steps)) return []
  return m.steps.map((step) => ({
    uuid: step.uuid,
    kind: step.kind,
    meta: KIND_META[step.kind] || { icon: '?', label: step.kind, hint: '' },
    summary: stepSummary(step),
    target: step.kind === 'call' || step.kind === 'func' ? step.target : '',
  }))
})

const headLabel = computed(() => {
  const m = props.model
  if (!m) return ''
  const parts = []
  if (Array.isArray(m.params) && m.params.length) {
    parts.push(`参数 ${m.params.length} 个`)
  }
  if (m.config) {
    parts.push(`轮询 ${m.config.interval}`, `阈值 ${m.config.threshold}`, String(m.config.log_level))
  }
  parts.push(`步骤 ${topSteps.value.length} 个`)
  return parts.join(' · ')
})
</script>

<style scoped>
.script-summary { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 6px; }
.sum-head { font-size: 11px; color: var(--text-2); flex-shrink: 0; }
.sum-empty {
  flex: 1; display: flex; align-items: center; justify-content: center;
  color: var(--text-2); font-size: 12px; background: var(--bg-0);
  border: 1px dashed var(--border); border-radius: var(--radius-sm);
}
.sum-list { flex: 1; min-height: 0; overflow: auto; display: flex; flex-direction: column; gap: 3px; }
.sum-row {
  display: flex; align-items: center; gap: 7px;
  background: var(--bg-0); border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 4px 8px;
}
.idx { color: var(--text-2); width: 18px; text-align: right; flex: none; }
.icon {
  display: inline-flex; align-items: center; justify-content: center;
  width: 18px; height: 18px; border-radius: 4px; flex: none;
  background: var(--bg-3); color: var(--accent); font-size: 11px;
}
.label { font-size: 12px; color: var(--text-0); flex: none; }
.summary { flex: 1; min-width: 0; font-size: 11px; color: var(--text-1); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.row-ops { flex: none; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 7px; cursor: pointer; flex: none;
}
.mini-btn.link { color: var(--accent-2); }
.mini-btn.link:hover { border-color: var(--accent-2); color: var(--accent-2); }
.mini-btn.run { color: var(--accent); }
.mini-btn.run:hover { background: var(--accent); color: #06251c; }
.run-hint { font-size: 11px; color: var(--text-2); flex-shrink: 0; }
.mono { font-family: var(--mono); font-size: 11px; }
</style>
