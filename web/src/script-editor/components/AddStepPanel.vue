<template>
  <div class="add-step-panel" @click.stop>
    <div class="panel-head">
      <span class="panel-title">添加步骤</span>
      <select v-model="selectedKind" class="step-select" aria-label="选择步骤类型" @change="insertSelected">
        <option value="">选择步骤类型…</option>
        <optgroup v-for="group in visibleGroups" :key="group.id" :label="group.label">
          <option v-for="entry in group.entries" :key="entry.kind" :value="entry.kind">
            {{ entry.label }}
          </option>
        </optgroup>
      </select>
      <button type="button" class="mini-btn" title="关闭" @click.stop="emit('close')">✕</button>
    </div>
    <div v-if="targetLabel" class="panel-target">插入到：{{ targetLabel }}</div>
  </div>
</template>

<script setup lang="ts">
/**
 * 添加步骤下拉（PANEL_GROUPS 六组：应用/操作/识别/流程/复用/函数专用）+
 * 上下文过滤（script 隐藏 return）。选择条目 → 工厂 makeStep + CommandStack 插入到当前
 * 锚点（选中卡之后 / 当前流程末尾），不直接改模型；插入成功后由画布选中新卡。
 */
import { computed, ref, type PropType } from 'vue'
import type { Path } from '../commands'
import { makeStep, PANEL_GROUPS, type PanelGroupId } from '../factories'
import type { Step, StepKind } from '../model'
import { KIND_META } from './kinds'

const props = defineProps({
  /** 编辑上下文：script 隐藏「函数专用」组（return 仅函数）。 */
  context: { type: String as PropType<'script' | 'function'>, default: 'script' },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
  /** 插入锚点：{ containerPath, index }（画布按选中卡/当前容器/容器级添加计算后传入）。 */
  anchor: {
    type: Object as PropType<{ containerPath: Path; index: number }>,
    required: true,
  },
  /** 插入位置提示（面包屑标签 + 末尾/第 N 步之后），下拉条头部展示。 */
  targetLabel: { type: String, default: '' },
})

const emit = defineEmits(['inserted', 'close'])

const visibleGroups = computed(() =>
  PANEL_GROUPS
    .filter((g) => props.context === 'function' || g.id !== ('function' satisfies PanelGroupId))
    .map((g) => ({ ...g, entries: g.entries })),
)

const selectedKind = ref<StepKind | ''>('')

function insert(kind: StepKind): void {
  const step = makeStep(kind)
  const label = KIND_META[kind].label
  const ok = props.stack.apply(
    { type: 'insert_step', path: props.anchor.containerPath, index: props.anchor.index, step },
    `添加 ${label}`,
  )
  if (ok) emit('inserted', step.uuid)
}

function insertSelected(event: Event): void {
  const kind = (event.target as HTMLSelectElement).value as StepKind | ''
  if (!kind) return
  insert(kind)
  selectedKind.value = ''
}
</script>

<style scoped>
.add-step-panel {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  box-shadow: var(--shadow);
  min-width: 250px;
  max-width: min(360px, calc(100vw - 32px));
}
.panel-head { display: flex; align-items: center; gap: 8px; padding: 8px 10px; border-bottom: 1px solid var(--border); }
.step-select {
  flex: 1; min-width: 0; width: auto; padding: 4px 8px; font-size: 12px;
  background: var(--bg-2); color: var(--text-0); border: 1px solid var(--border);
  border-radius: var(--radius-sm);
}
.step-select:focus { outline: none; border-color: var(--accent); }
.panel-target {
  padding: 5px 10px; font-size: 12px; color: var(--accent-2);
  border-bottom: 1px solid var(--border); background: var(--bg-2);
}
.panel-title { font-weight: 600; font-size: 13px; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 6px; cursor: pointer;
}
.mini-btn:hover { color: var(--danger); border-color: var(--danger); }
</style>
