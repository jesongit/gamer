<template>
  <div class="add-step-panel" @click.stop>
    <div class="panel-head">
      <span class="panel-title">添加步骤</span>
      <input
        v-model="query"
        class="panel-search"
        type="search"
        placeholder="搜索：点击 / 模板 / 循环…"
        aria-label="搜索步骤类型"
      />
      <button type="button" class="mini-btn" title="关闭" @click.stop="emit('close')">✕</button>
    </div>
    <div class="panel-body">
      <div v-for="group in visibleGroups" :key="group.id" class="panel-group">
        <div class="group-label">{{ group.label }}</div>
        <div class="group-entries">
          <button
            v-for="entry in group.entries"
            :key="entry.kind"
            type="button"
            class="entry-btn"
            :title="KIND_META[entry.kind].hint"
            @click.stop="insert(entry.kind)"
          >
            <span class="entry-icon">{{ KIND_META[entry.kind].icon }}</span>
            {{ entry.label }}
          </button>
        </div>
      </div>
      <div v-if="visibleGroups.length === 0" class="panel-empty">没有匹配「{{ query }}」的步骤类型</div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 添加步骤面板（plan §8.5）：PANEL_GROUPS 六组（应用/操作/识别/流程/复用/函数专用）
 * + 搜索过滤 + 上下文过滤（script 隐藏 return）。
 * 点击条目 → 工厂 makeStep + CommandStack 插入到当前锚点（选中卡之后 / 当前流程末尾），
 * 不直接改模型；插入成功后由画布选中新卡。
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
  /** 插入锚点：{ containerPath, index }（画布按选中卡/当前容器计算后传入）。 */
  anchor: {
    type: Object as PropType<{ containerPath: Path; index: number }>,
    required: true,
  },
})

const emit = defineEmits(['inserted', 'close'])

const query = ref('')

const visibleGroups = computed(() =>
  PANEL_GROUPS
    .filter((g) => props.context === 'function' || g.id !== ('function' satisfies PanelGroupId))
    .map((g) => ({
      ...g,
      entries: g.entries.filter((e) => {
        const q = query.value.trim().toLowerCase()
        if (!q) return true
        return e.label.toLowerCase().includes(q) || e.kind.toLowerCase().includes(q)
      }),
    }))
    .filter((g) => g.entries.length > 0),
)

function insert(kind: StepKind): void {
  const step = makeStep(kind)
  const label = KIND_META[kind].label
  const ok = props.stack.apply(
    { type: 'insert_step', path: props.anchor.containerPath, index: props.anchor.index, step },
    `添加 ${label}`,
  )
  if (ok) emit('inserted', step.uuid)
}
</script>

<style scoped>
.add-step-panel {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  box-shadow: var(--shadow);
  margin: 6px 0;
  max-width: 560px;
}
.panel-head { display: flex; align-items: center; gap: 8px; padding: 8px 10px; border-bottom: 1px solid var(--border); }
.panel-title { font-weight: 600; font-size: 13px; }
.panel-search {
  flex: 1; background: var(--bg-2); color: var(--text-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 4px 8px; font-size: 12px;
}
.panel-search:focus { outline: none; border-color: var(--accent); }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 6px; cursor: pointer;
}
.mini-btn:hover { color: var(--danger); border-color: var(--danger); }
.panel-body { padding: 8px 10px; display: flex; flex-direction: column; gap: 10px; max-height: 320px; overflow: auto; }
.panel-group .group-label { font-size: 11px; color: var(--text-2); margin-bottom: 4px; }
.group-entries { display: flex; flex-wrap: wrap; gap: 6px; }
.entry-btn {
  display: inline-flex; align-items: center; gap: 5px;
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-0);
  border-radius: var(--radius-sm); font-size: 12px; padding: 4px 10px; cursor: pointer;
}
.entry-btn:hover { border-color: var(--accent); color: var(--accent); }
.entry-icon {
  display: inline-flex; align-items: center; justify-content: center;
  width: 16px; height: 16px; border-radius: 3px;
  background: var(--bg-3); color: var(--accent); font-size: 10px;
}
.panel-empty { font-size: 12px; color: var(--text-2); padding: 8px 0; }
</style>
