<template>
  <div class="add-step-panel" @click.stop>
    <span class="add-step-mask" aria-hidden="true" @click.stop="emit('close')"></span>
    <div class="panel-head">
      <span class="panel-title">添加步骤</span>
      <button type="button" class="mini-btn" title="关闭" @click.stop="emit('close')">✕</button>
    </div>
    <div v-if="targetLabel" class="panel-target">插入到：{{ targetLabel }}</div>
    <input
      v-model="filter" class="fn-filter" type="search" placeholder="搜索函数…"
      aria-label="搜索函数"
    />
    <div class="step-menu" role="menu" aria-label="选择步骤类型">
      <div class="step-group">
        <div class="step-group-label">流程</div>
        <button
          v-for="entry in controlEntries" :key="entry.kind"
          type="button"
          class="step-menu-item"
          role="menuitem"
          :data-kind="entry.kind"
          :aria-label="`添加${entry.label}`"
          :title="entry.hint"
          @click.stop="insertControl(entry.kind)"
        >{{ entry.label }}</button>
      </div>
      <div v-for="g in functionGroups" :key="g.id" class="step-group">
        <div class="step-group-label">{{ g.label }}</div>
        <button
          v-for="o in g.options" :key="o.target"
          type="button"
          class="step-menu-item fn-item"
          role="menuitem"
          :data-kind="`call:${o.target}`"
          :aria-label="`调用 ${o.target}`"
          :title="o.hint || o.target"
          @click.stop="insertCall(o.target)"
        >
          <span class="fn-name">{{ o.label || o.target }}</span>
          <span v-if="o.hint" class="fn-hint">{{ o.hint }}</span>
        </button>
        <div v-if="g.options.length === 0" class="step-group-empty">无匹配函数</div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 添加步骤下拉（V1）：流程（if/repeat/return）+ 函数目录（插件函数 /
 * 配置包函数，宿主经 provide(SE_TARGET_OPTIONS) 注入）。选择条目 → 工厂创建 +
 * CommandStack 插入到当前锚点（选中卡之后 / 当前流程末尾），不直接改模型；
 * 插入成功后由画布选中新卡。
 */
import { computed, inject, ref, type PropType } from 'vue'
import type { Path } from '../commands'
import { createCall, createControl } from '../factories'
import type { Step } from '../model'
import { KIND_META } from './kinds'
import { SE_TARGET_OPTIONS } from '../targets'

const props = defineProps({
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

const filter = ref('')
const targetOptions = inject(SE_TARGET_OPTIONS, null)

const controlEntries = computed(() => {
  const q = filter.value.trim().toLowerCase()
  if (!q) return CONTROL_ALL
  return CONTROL_ALL.filter((e) => e.label.toLowerCase().includes(q) || e.kind.includes(q))
})

const CONTROL_ALL = [
  { kind: 'if' as const, label: '条件分支', hint: 'if $x → then / else' },
  { kind: 'repeat' as const, label: '固定循环', hint: 'repeat N 次 → do' },
  { kind: 'return' as const, label: '返回值', hint: '结束并返回一个值' },
]

const functionGroups = computed(() => {
  const q = filter.value.trim().toLowerCase()
  const all = targetOptions?.targets ?? []
  const match = (o: { target: string; label?: string; hint?: string }): boolean =>
    !q
    || o.target.toLowerCase().includes(q)
    || (o.label ?? '').toLowerCase().includes(q)
    || (o.hint ?? '').toLowerCase().includes(q)
  return [
    { id: 'plugin', label: '插件函数', options: all.filter((o) => o.group === 'plugin' && match(o)) },
    { id: 'package', label: '配置包函数', options: all.filter((o) => o.group !== 'plugin' && match(o)) },
  ].filter((g) => g.options.length > 0 || g.id === 'package')
})

function insert(step: Step, label: string): void {
  const ok = props.stack.apply(
    { type: 'insert_step', path: props.anchor.containerPath, index: props.anchor.index, step },
    `添加 ${label}`,
  )
  if (ok) emit('inserted', step.uuid)
}

function insertControl(kind: 'if' | 'repeat' | 'return'): void {
  insert(createControl(kind), KIND_META[kind].label)
}

function insertCall(fn: string): void {
  insert(createCall(fn), `调用 ${fn}`)
}
</script>

<style scoped>
.add-step-panel {
  position: absolute; top: 0; left: 0; z-index: 30;
  display: flex; flex-direction: column; box-sizing: border-box;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  box-shadow: var(--shadow);
  min-width: 240px;
  max-width: min(340px, calc(100vw - 32px));
  max-height: min(480px, calc(100vh - 64px));
}
.add-step-mask {
  position: fixed; inset: 0; z-index: 0;
}
.panel-head, .panel-target, .fn-filter, .step-menu { position: relative; z-index: 1; }
.panel-head, .panel-target, .fn-filter { flex-shrink: 0; }
.panel-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 8px 10px; border-bottom: 1px solid var(--border); }
.panel-target {
  padding: 5px 10px; font-size: 12px; color: var(--accent-2);
  border-bottom: 1px solid var(--border); background: var(--bg-2);
}
.panel-title { font-weight: 600; font-size: 13px; }
.fn-filter {
  margin: 8px 10px 0; padding: 5px 8px;
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  background: var(--bg-2); color: var(--text-0); font-size: 12px;
}
.fn-filter:focus { outline: none; border-color: var(--accent); }
.step-menu {
  flex: 1 1 auto; min-height: 0;
  display: grid; grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px; padding: 8px; overflow: auto;
}
.step-group { min-width: 0; }
.step-group-label {
  padding: 2px 6px 4px; color: var(--text-2); font-size: 11px;
}
.step-group-empty { padding: 2px 6px 4px; color: var(--text-2); font-size: 11px; font-style: italic; }
.step-menu-item {
  display: block; width: 100%; padding: 6px 8px; border: none;
  border-radius: var(--radius-sm); background: transparent; color: var(--text-0);
  font-size: 12px; text-align: left; cursor: pointer;
}
.step-menu-item:hover, .step-menu-item:focus-visible {
  outline: none; background: var(--bg-3); color: var(--accent);
}
.fn-item .fn-name { display: block; }
.fn-item .fn-hint {
  display: block; color: var(--text-2); font-size: 11px;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 140px;
}
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 6px; cursor: pointer;
}
.mini-btn:hover { color: var(--danger); border-color: var(--danger); }
</style>
