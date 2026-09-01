<template>
  <!-- 根容器（depth 0，主流程/专注视图当前层）：去掉外框与标题，步骤直接平铺；
       容器外观只保留给嵌套分支（depth ≥ 1），减少一层多余的视觉嵌套 -->
  <div class="branch-container" :class="{ 'focus-only': depth >= 2, root: depth === 0 }">
    <div v-if="depth > 0" class="branch-head" @click.stop>
      <span class="branch-label">{{ label }}</span>
      <span class="branch-count">{{ list.length }} 步</span>
      <span class="branch-actions">
        <button
          v-if="depth < 2" type="button" class="mini-btn add" title="在此流程末尾添加步骤"
          @click.stop="emit('add-here', containerPath, $event.currentTarget)"
        >+ 添加</button>
        <button
          v-else type="button" class="mini-btn focus-btn" title="全屏编辑该子流程（避免无限缩进）"
          @click.stop="emit('focus', containerPath)"
        >进入专注编辑</button>
      </span>
    </div>

    <template v-if="depth < 2">
      <div v-if="list.length" class="branch-steps">
        <StepCard
          v-for="(st, i) in list"
          :key="st.uuid"
          :model="model"
          :stack="stack"
          :step="st"
          :container-path="containerPath"
          :base-path="basePath"
          :index="i"
          :depth="depth"
          :diagnostics="diagnostics"
          :selected-uuid="selectedUuid"
          :highlight-uuid="highlightUuid"
          :expanded-uuids="expandedUuids"
          :params="params"
          :context="context"
          :resolve-target="resolveTarget"
          :templates="templates"
          :test-from="testFrom"
          @select="(u) => emit('select', u)"
          @toggle-expand="(u) => emit('toggle-expand', u)"
          @focus="(p) => emit('focus', p)"
          @add-here="(p, el) => emit('add-here', p, el)"
          @test-from="(u) => emit('test-from', u)"
        />
      </div>
      <div
        v-else class="branch-empty"
        :class="{ 'drop-active': emptyDropActive }"
        @dragover.prevent.stop="onEmptyDragOver"
        @dragleave.stop="onEmptyDragLeave"
        @drop.prevent.stop="onEmptyDrop"
      >空流程——点「+ 添加」插入步骤</div>
    </template>
    <div v-else class="branch-collapsed-hint">{{ list.length }} 个步骤，进入专注编辑查看与修改</div>
  </div>
</template>

<script setup lang="ts">
/**
 * 分支容器（plan §8.4）：then/else、match/color 候选子流程、loop 循环体的统一渲染。
 *
 * 深度分界（一层内嵌、更深专注）：
 * - depth 0：画布根容器（主流程 / 专注视图的当前层）；
 * - depth 1：卡片内的第一层分支，继续内嵌渲染卡片；
 * - depth ≥ 2：只显示头与「进入专注编辑」，点击后由画布切换专注视图（面包屑导航返回），
 *   避免卡片无限向右缩进。
 * 所有写操作仍由 StepCard / 画布经 CommandStack 完成，本组件只做结构与转发。
 */
import { computed, ref, type PropType } from 'vue'
import type { Path } from '../commands'
import { resolveStepList } from '../commands'
import type { Diagnostic } from '../diagnostics'
import type { ParamDecl, Step } from '../model'
import { getActiveStepDrag, readStepDragPayload, type StepDragPayload } from '../step-dnd'
import StepCard from './StepCard.vue'

const props = defineProps({
  model: { type: Object as PropType<Parameters<typeof resolveStepList>[0]>, required: true },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
  /** 容器路径（resolveStepList 合法输入）。 */
  containerPath: { type: Array as PropType<Path>, required: true },
  /** step_path 字符串基（诊断定位，如 steps[0].then）。 */
  basePath: { type: String, required: true },
  label: { type: String, required: true },
  depth: { type: Number, default: 0 },
  diagnostics: { type: Array as PropType<Diagnostic[]>, default: () => [] },
  selectedUuid: { type: String, default: null },
  highlightUuid: { type: String, default: null },
  expandedUuids: { type: Object as PropType<Set<string> | null>, default: null },
  context: { type: String as PropType<'script' | 'function'>, default: 'script' },
  params: { type: Array as PropType<ParamDecl[]>, default: () => [] },
  resolveTarget: {
    type: Function as PropType<((kind: 'call' | 'func', target: string) => { params: ParamDecl[] } | null) | undefined>,
    default: undefined,
  },
  templates: { type: Array as PropType<string[]>, default: () => [] },
  /** 透传「从此步骤测试函数」入口开关（宿主仅在函数体根容器开启）。 */
  testFrom: { type: Boolean, default: false },
})

const emit = defineEmits(['select', 'toggle-expand', 'focus', 'add-here', 'test-from'])

const list = computed<Step[]>(() => resolveStepList(props.model, props.containerPath))

const emptyDropActive = ref(false)

function emptyDragPayload(event: DragEvent): StepDragPayload | null {
  return readStepDragPayload(event.dataTransfer) ?? getActiveStepDrag()
}

function onEmptyDragOver(event: DragEvent): void {
  emptyDropActive.value = !!emptyDragPayload(event)
  if (emptyDropActive.value && event.dataTransfer) event.dataTransfer.dropEffect = 'move'
}

function onEmptyDragLeave(event: DragEvent): void {
  const current = event.currentTarget as HTMLElement
  const next = event.relatedTarget
  if (next instanceof Node && current.contains(next)) return
  emptyDropActive.value = false
}

function onEmptyDrop(event: DragEvent): void {
  const source = emptyDragPayload(event)
  emptyDropActive.value = false
  if (!source) return
  props.stack.apply(
    {
      type: 'move_step',
      from: { path: source.path, index: source.index },
      to: { path: [...props.containerPath], index: 0 },
    },
    '拖动步骤',
  )
}
</script>

<style scoped>
.branch-container {
  border: 1px dashed var(--border);
  border-radius: var(--radius-sm);
  margin: 4px 0;
  padding: 2px 6px 6px;
  background: rgba(23, 28, 41, .5);
}
.branch-container.root { border: none; margin: 0; padding: 0; background: transparent; }
.branch-container.root .branch-steps { padding-left: 0; border-left: none; }
.branch-container.root .branch-empty { padding: 18px 8px; text-align: center; border: 1px dashed var(--border); border-radius: var(--radius-sm); }
.branch-container.focus-only { border-style: dotted; opacity: .92; }
.branch-head {
  display: flex; align-items: center; gap: 8px;
  padding: 3px 2px;
}
.branch-label {
  font-size: 12px; font-weight: 600; color: var(--accent-2);
  background: var(--bg-3); border-radius: 4px; padding: 1px 8px;
}
.branch-count { font-size: 11px; color: var(--text-2); }
.branch-actions { margin-left: auto; display: inline-flex; gap: 4px; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 6px; cursor: pointer;
}
.mini-btn:hover { color: var(--accent); border-color: var(--accent); }
.mini-btn.add { color: var(--accent-2); }
.mini-btn.focus-btn { color: var(--warn); }
.branch-steps { padding-left: 10px; border-left: 2px solid var(--border); }
.branch-empty {
  font-size: 12px; color: var(--text-2); padding: 6px 2px 2px 10px;
  border-radius: var(--radius-sm);
}
.branch-empty.drop-active { background: rgba(56, 189, 248, .12); outline: 1px dashed var(--accent); }
.branch-collapsed-hint { font-size: 12px; color: var(--text-2); padding: 2px 2px 2px 10px; }
</style>
