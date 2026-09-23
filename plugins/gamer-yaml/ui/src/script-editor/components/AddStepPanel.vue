<template>
  <div class="add-step-panel" @click.stop @keydown.esc.stop.prevent="emit('close')">
    <span class="add-step-mask" aria-hidden="true" @click.stop="emit('close')"></span>
    <div class="panel-head">
      <span class="panel-title">添加步骤</span>
      <button type="button" class="mini-btn" title="关闭" @click.stop="emit('close')">✕</button>
    </div>
    <div v-if="targetLabel" class="panel-target">插入到：{{ targetLabel }}</div>
    <input
      ref="searchInput" v-model="filter" class="fn-filter" type="search" placeholder="搜索名称、函数名或说明…"
      aria-label="搜索函数"
    />
    <div class="source-filters" role="group" aria-label="函数来源">
      <button v-for="source in sources" :key="source.id" type="button" :aria-pressed="sourceFilter === source.id"
        :class="{ active: sourceFilter === source.id }" @click="sourceFilter = source.id">
        {{ source.label }}<span>{{ source.count }}</span>
      </button>
    </div>
    <div class="step-menu" role="menu" aria-label="选择步骤类型">
      <div v-if="controlEntries.length && sourceFilter === 'all'" class="step-group control-group">
        <div class="step-group-label">流程</div>
        <div class="control-grid">
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
      </div>
      <div v-for="g in functionGroups" :key="g.id" class="step-group">
        <div class="step-group-label">{{ g.label }}</div>
        <div class="function-grid">
        <button
          v-for="o in g.options" :key="o.target"
          type="button"
          class="step-menu-item fn-item"
          role="menuitem"
          :data-kind="`call:${o.target}`"
          :aria-label="`调用 ${o.target}`"
          :title="`${o.target}${o.hint ? ' · ' + o.hint : ''}`"
          @click.stop="insertCall(o.target)"
        >
          <span class="fn-name">{{ optionLabel(o) }}</span>
          <span v-if="optionLabel(o) !== o.target" class="fn-code">{{ o.target }}</span>
        </button>
        </div>
      </div>
      <div v-if="!functionGroups.length && (!controlEntries.length || sourceFilter !== 'all')" class="step-group-empty">没有匹配选项，试试其他关键词或来源</div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 添加步骤下拉（V1）：流程（if/repeat/return/match_templates/break）+ 函数目录（插件函数 /
 * 配置包函数，宿主经 provide(SE_TARGET_OPTIONS) 注入）。选择条目 → 工厂创建 +
 * CommandStack 插入到当前锚点（选中卡之后 / 当前流程末尾），不直接改模型；
 * 插入成功后由画布选中新卡。
 */
import { computed, inject, onMounted, ref, type PropType } from 'vue'
import type { Path } from '../commands'
import { createCall, createCallFromSchema, createControl } from '../factories'
import type { Step } from '../model'
import { KIND_META } from './kinds'
import { SE_TARGET_OPTIONS } from '../targets'
import type { SeTargetOption } from '../targets'
import { NATIVE_CALL_NAMES } from '../call-names'

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
const sourceFilter = ref('all')
const searchInput = ref<HTMLInputElement | null>(null)
onMounted(() => searchInput.value?.focus({ preventScroll: true }))
const targetOptions = inject(SE_TARGET_OPTIONS, null)
function optionLabel(option: SeTargetOption): string {
  if (option.label && option.label !== option.target) return option.label
  return (option.group === 'plugin' && NATIVE_CALL_NAMES[option.target]) || option.target
}
const sources = computed(() => {
  const all = targetOptions?.targets ?? []
  return [
    { id: 'all', label: '全部', count: all.length + CONTROL_ALL.length },
    { id: 'plugin', label: '插件函数', count: all.filter(o => o.group === 'plugin').length },
    { id: 'package', label: '配置包函数', count: all.filter(o => o.group !== 'plugin').length },
  ]
})

const controlEntries = computed(() => {
  const q = filter.value.trim().toLowerCase()
  if (!q) return CONTROL_ALL
  return CONTROL_ALL.filter((e) => e.label.toLowerCase().includes(q) || e.kind.includes(q))
})

const CONTROL_ALL = [
  { kind: 'match_templates' as const, label: '模板分支', hint: '按顺序匹配模板，执行首个命中分支' },
  { kind: 'if' as const, label: '条件分支', hint: 'if $x → then / else' },
  { kind: 'repeat' as const, label: '固定循环', hint: 'repeat N 次 → do' },
  { kind: 'break' as const, label: '跳出循环', hint: 'break → 退出最近一层 repeat 循环' },
  { kind: 'return' as const, label: '返回值', hint: '结束并返回一个值' },
]

const functionGroups = computed(() => {
  const q = filter.value.trim().toLowerCase()
  const all = targetOptions?.targets ?? []
  const match = (o: { target: string; label?: string; hint?: string }): boolean =>
    !q
    || o.target.toLowerCase().includes(q)
    || optionLabel(o).toLowerCase().includes(q)
    || (o.hint ?? '').toLowerCase().includes(q)
  return [
    { id: 'plugin', label: '插件函数', options: all.filter((o) => o.group === 'plugin' && match(o)) },
    { id: 'package', label: '配置包函数', options: all.filter((o) => o.group !== 'plugin' && match(o)) },
  ].filter((g) => g.options.length > 0 && (sourceFilter.value === 'all' || sourceFilter.value === g.id))
})

function insert(step: Step, label: string): void {
  const ok = props.stack.apply(
    { type: 'insert_step', path: props.anchor.containerPath, index: props.anchor.index, step },
    `添加 ${label}`,
  )
  if (ok) emit('inserted', step.uuid)
}

function insertControl(kind: 'if' | 'repeat' | 'return' | 'match_templates' | 'break'): void {
  insert(createControl(kind), KIND_META[kind].label)
}

async function insertCall(fn: string): Promise<void> {
  let step = createCall(fn)
  try {
    const decls = await targetOptions?.resolveParams(fn)
    if (decls) step = createCallFromSchema(fn, decls)
  } catch {
    // Schema 获取失败时仍允许插入函数；保存/校验阶段会给出正式诊断。
  }
  insert(step, `调用 ${fn}`)
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
  min-width: min(240px, calc(100vw - 16px));
  width: min(660px, calc(100vw - 16px));
  max-width: calc(100vw - 16px);
  max-height: min(560px, calc(100vh - 16px));
}
.add-step-mask {
  position: fixed; inset: 0; z-index: 0;
}
.panel-head, .panel-target, .fn-filter, .source-filters, .step-menu { position: relative; z-index: 1; }
.panel-head, .panel-target, .fn-filter, .source-filters { flex-shrink: 0; }
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
  display: flex; flex-direction: column;
  gap: 10px; padding: 8px 10px 10px; overflow: auto;
  overscroll-behavior: contain;
}
.step-group { min-width: 0; flex: none; }
.function-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 4px; }
.control-group { display: flex; align-items: center; gap: 12px; padding-bottom: 8px; border-bottom: 1px solid var(--border); }
.control-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); flex: 1; gap: 4px; }
.source-filters { display: flex; gap: 4px; padding: 8px 10px 0; }
.source-filters button { display: inline-flex; gap: 6px; padding: 4px 8px; border: 1px solid transparent; border-radius: 3px; color: var(--text-1); background: transparent; font-size: 12px; cursor: pointer; }
.source-filters button span { color: var(--text-2); font-variant-numeric: tabular-nums; }
.source-filters button.active { color: var(--accent); background: var(--bg-3); border-color: var(--border); }
.source-filters button:hover { background: var(--bg-3); }
.step-group-label {
  padding: 2px 6px 4px; color: var(--text-2); font-size: 12px;
}
.step-group-empty { padding: 2px 6px 4px; color: var(--text-2); font-size: 12px; font-style: italic; }
.step-menu-item {
  display: block; width: 100%; min-width: 0; padding: 6px 8px; border: 1px solid var(--border);
  border-radius: var(--radius-sm); background: var(--bg-2); color: var(--text-0);
  font-size: 12px; text-align: left; cursor: pointer;
}
.step-menu-item:hover, .step-menu-item:focus-visible {
  outline: none; background: var(--bg-3); color: var(--accent);
}
.fn-item { min-height: 43px; }
.fn-item .fn-name,.fn-code { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.fn-code { color: var(--text-2); font: 11px var(--mono); margin-top: 2px; }
@media (max-width: 516px) { .function-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@media (max-width: 316px) { .function-grid { grid-template-columns: minmax(0, 1fr); } }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 12px; padding: 2px 6px; cursor: pointer;
}
.mini-btn:hover { color: var(--danger); border-color: var(--danger); }
</style>
