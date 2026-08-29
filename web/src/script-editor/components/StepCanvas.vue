<template>
  <div ref="rootEl" class="se-canvas" @click.self="deselect">
    <!-- 顶部：函数切换（函数库）+ 面包屑 + 添加入口 -->
    <div class="canvas-toolbar">
      <select
        v-if="isFunction"
        class="fn-select"
        :value="activeFnName"
        aria-label="编辑函数"
        @click.stop
        @change="switchFn(($event.target as HTMLSelectElement).value)"
      >
        <option v-for="name in fnNames" :key="name" :value="name">{{ name }}</option>
      </select>
      <nav class="breadcrumb" aria-label="当前编辑流程">
        <template v-for="(node, i) in breadcrumbNodes" :key="i">
          <span v-if="i" class="crumb-sep">/</span>
          <button
            type="button"
            class="crumb"
            :class="{ current: i === breadcrumbNodes.length - 1 }"
            @click.stop="navigateTo(node)"
          >{{ node.label }}</button>
        </template>
      </nav>
      <button type="button" class="add-btn" @click.stop="panelOpen = !panelOpen">+ 添加步骤</button>
    </div>

    <!-- 插入锚点提示（§8.4/§10：可见「下一条将插入：主流程 / 第 N 步之后」） -->
    <div class="anchor-hint">下一条将插入：{{ anchorLabel }}</div>

    <AddStepPanel
      v-if="panelOpen"
      :context="context"
      :stack="stack"
      :anchor="anchor"
      @inserted="onInserted"
      @close="panelOpen = false"
    />

    <BranchContainer
      :model="model"
      :stack="stack"
      :container-path="activeContainer"
      :base-path="basePathActive"
      :label="activeLabel"
      :depth="0"
      :diagnostics="diagnostics"
      :selected-uuid="sel"
      :highlight-uuid="highlightUuid"
      :expanded-uuids="expandedUuids"
      :params="cellParams"
      :context="context"
      :resolve-target="resolveTarget"
      :templates="templates"
      @select="onSelect"
      @toggle-expand="toggleExpand"
      @focus="enterFocus"
      @add-here="onAddHere"
    />

    <ErrorSummary v-if="showErrorPanel" :diagnostics="diagnostics" @locate="locate" />
  </div>
</template>

<script setup lang="ts">
/**
 * 步骤画布（plan §8.3 中央区 / §8.4）：
 * - 卡片列表渲染（经 BranchContainer，depth 0）+ 点击选中 + 添加步骤入口；
 * - 插入锚点提示「下一条将插入：主流程 / 第 N 步之后」（选中卡之后 / 当前容器末尾）；
 * - 顶部面包屑：有选中卡用 selection.breadcrumb，否则按当前容器构建；点击节点切换/返回；
 * - 专注子流程视图：进入深层分支后整屏编辑该容器，面包屑导航返回，避免无限右缩进；
 * - ErrorSummary 定位联动（showErrorPanel 时）：展开祖先链 → 选中 → 滚动 + 瞬态高亮；
 *   接受外部诊断（服务端错误回填与客户端校验同构）。
 * 选中与展开状态由画布持有（selectedUuid prop 传入则受控于页面）。
 */
import { computed, nextTick, onBeforeUnmount, reactive, ref, watch, type PropType } from 'vue'
import type { EditorModel, Path } from '../commands'
import { resolveStepList } from '../commands'
import { breadcrumb, defaultAnchor, rootContainerPath } from '../selection'
import type { BreadcrumbNode } from '../selection'
import type { Diagnostic } from '../diagnostics'
import type { ParamDecl, ScriptModel } from '../model'
import { containerNesting, basePathOfContainer, breadcrumbForContainer, locateDiagnostic } from './kinds'
import BranchContainer from './BranchContainer.vue'
import AddStepPanel from './AddStepPanel.vue'
import ErrorSummary from './ErrorSummary.vue'

const props = defineProps({
  model: { type: Object as PropType<EditorModel>, required: true },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
  diagnostics: { type: Array as PropType<Diagnostic[]>, default: () => [] },
  /** 受控选中（阶段 4 页面用）；不传则画布内部持有。 */
  selectedUuid: { type: String, default: null },
  context: { type: String as PropType<'script' | 'function'>, default: 'script' },
  resolveTarget: {
    type: Function as PropType<((kind: 'call' | 'func', target: string) => { params: ParamDecl[] } | null) | undefined>,
    default: undefined,
  },
  /** 模板短名候选（tmpl 控件 datalist）。 */
  templates: { type: Array as PropType<string[]>, default: () => [] },
  /** 渲染 ErrorSummary 并接管定位联动。 */
  showErrorPanel: { type: Boolean, default: false },
  /** CellEditor 的参数引用列表；缺省时按模型自动取（脚本 = 文件级，函数库 = 当前函数）。 */
  params: { type: Array as PropType<ParamDecl[]>, default: null as unknown as ParamDecl[] },
})

const emit = defineEmits(['select'])

// ---------- 选中 / 展开 / 高亮 ----------

const innerSelected = ref<string | null>(null)
const sel = computed<string | null>(() => props.selectedUuid ?? innerSelected.value)
const expandedUuids = reactive(new Set<string>())
const highlightUuid = ref<string | null>(null)
let highlightTimer: ReturnType<typeof setTimeout> | null = null

function onSelect(uuid: string | null): void {
  innerSelected.value = uuid
  emit('select', uuid)
}
function deselect(): void {
  onSelect(null)
}
function toggleExpand(uuid: string): void {
  if (expandedUuids.has(uuid)) expandedUuids.delete(uuid)
  else expandedUuids.add(uuid)
}

// ---------- 容器状态（当前容器 + 专注视图） ----------

const rootPath = computed<Path>(() => rootContainerPath(props.model))
const isFunction = computed(() => 'functions' in props.model)
const fnNames = computed<string[]>(() =>
  isFunction.value ? (props.model as { functions: { name: string }[] }).functions.map((f) => f.name) : [],
)

const currentContainer = ref<Path>(rootContainerPath(props.model))
const focusPath = ref<Path | null>(null)

watch(
  () => props.model,
  () => {
    currentContainer.value = rootContainerPath(props.model)
    focusPath.value = null
    innerSelected.value = null
  },
)

/** 路径失效兜底（函数被删/重命名等）。 */
function sanitize(path: Path | null | undefined): Path {
  if (path) {
    if (path[0] === 'functions' && isFunction.value && fnNames.value.includes(String(path[1]))) return path
    if (path[0] === 'steps' && !isFunction.value) return path
  }
  return rootPath.value
}

const activeContainer = computed<Path>(() => sanitize(focusPath.value ?? currentContainer.value))
const activeFnName = computed(() => (isFunction.value ? String(activeContainer.value[1] ?? '') : ''))
const activeLabel = computed(() => {
  const nodes = breadcrumbForContainer(props.model, activeContainer.value)
  return nodes.length ? nodes[nodes.length - 1]!.label : '主流程'
})
const basePathActive = computed(() => basePathOfContainer(activeContainer.value))

function enterFocus(path: Path): void {
  focusPath.value = path
  currentContainer.value = path
}
function navigateTo(node: BreadcrumbNode): void {
  const root = rootPath.value
  const isRoot = node.containerPath.length === root.length && root.every((seg, i) => seg === node.containerPath[i])
  if (isRoot) {
    focusPath.value = null
    currentContainer.value = root
  } else {
    focusPath.value = node.containerPath
    currentContainer.value = node.containerPath
  }
}
function switchFn(name: string): void {
  focusPath.value = null
  currentContainer.value = ['functions', name, 'steps']
}

// ---------- 插入锚点 ----------

function isPrefixPath(prefix: Path, path: Path): boolean {
  return prefix.length <= path.length && prefix.every((seg, i) => seg === path[i])
}

const anchor = computed(() => {
  const a = defaultAnchor(props.model, sel.value, activeContainer.value)
  // 选中卡不在当前视图容器子树内 → 回退当前容器末尾（避免插入到不可见位置）
  if (sel.value && !isPrefixPath(activeContainer.value, a.containerPath)) {
    return { containerPath: activeContainer.value, index: resolveStepList(props.model, activeContainer.value).length }
  }
  return a
})

const anchorLabel = computed(() => {
  const labels = breadcrumbForContainer(props.model, anchor.value.containerPath).map((n) => n.label)
  const len = resolveStepList(props.model, anchor.value.containerPath).length
  const at = anchor.value.index >= len ? '末尾' : `第 ${anchor.value.index} 步之后`
  return `${labels.join(' / ')} / ${at}`
})

// ---------- 添加面板 ----------

const panelOpen = ref(false)

function onAddHere(path: Path): void {
  // 容器级「+ 添加」= 插入该容器末尾：清除选中使锚点落到该容器
  innerSelected.value = null
  emit('select', null)
  currentContainer.value = path
  panelOpen.value = true
}
function onInserted(uuid: string): void {
  panelOpen.value = false
  innerSelected.value = uuid
  emit('select', uuid)
}

// ---------- 面包屑 ----------

const breadcrumbNodes = computed<BreadcrumbNode[]>(() => {
  if (sel.value) {
    const nodes = breadcrumb(props.model, sel.value)
    if (nodes.length) return nodes
  }
  return breadcrumbForContainer(props.model, activeContainer.value)
})

// ---------- 诊断定位 ----------

function locate(diag: Diagnostic): void {
  const hit = locateDiagnostic(props.model, diag)
  if (!hit) return
  expandedUuids.add(hit.uuid)
  for (const u of hit.ancestorUuids) expandedUuids.add(u)
  // 宿主容器嵌套超过一层 → 专注到该容器；否则回根视图
  if (containerNesting(hit.containerPath) > 1) {
    focusPath.value = hit.containerPath
    currentContainer.value = hit.containerPath
  } else if (!isFunction.value) {
    focusPath.value = null
    currentContainer.value = rootPath.value
  }
  innerSelected.value = hit.uuid
  emit('select', hit.uuid)
  highlightUuid.value = hit.uuid
  if (highlightTimer) clearTimeout(highlightTimer)
  highlightTimer = setTimeout(() => {
    highlightUuid.value = null
  }, 1800)
  void nextTick(() => {
    const el = rootEl.value?.querySelector(`[data-step-uuid="${hit.uuid}"]`)
    el?.scrollIntoView?.({ block: 'center' })
  })
}

onBeforeUnmount(() => {
  if (highlightTimer) clearTimeout(highlightTimer)
})

// ---------- 参数列表（CellEditor 引用下拉） ----------

const cellParams = computed<ParamDecl[]>(() => {
  if (props.params && props.params.length) return props.params
  if (isFunction.value) {
    const fns = (props.model as { functions: { name: string; params: ParamDecl[] }[] }).functions
    const fn = fns.find((f) => f.name === activeFnName.value)
    return fn ? fn.params : []
  }
  return (props.model as ScriptModel).params
})

const rootEl = ref<HTMLElement | null>(null)

/**
 * 外壳（阶段 4）专用出口：
 * - anchor：当前插入锚点（Alt 生成 tap/find/color 步骤按此插入，与面板添加同源）；
 * - locate：错误面板独立挂载在画布外（全屏外壳右侧常驻）时由宿主转发定位；
 * - activeFnName：函数库当前编辑的函数名（全屏外壳按它把 ParamEditor 指到函数级 params）。
 */
defineExpose({ anchor, locate, activeFnName })
</script>

<style scoped>
.se-canvas {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-2);
  padding: 8px 10px;
  min-height: 200px;
}
.canvas-toolbar {
  display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
  padding-bottom: 6px; border-bottom: 1px solid var(--border); margin-bottom: 4px;
}
.fn-select {
  background: var(--bg-3); color: var(--text-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 3px 8px; font-size: 12px;
}
.breadcrumb { display: flex; align-items: center; gap: 4px; flex-wrap: wrap; flex: 1; min-width: 0; }
.crumb {
  border: none; background: transparent; color: var(--accent-2);
  font-size: 12px; cursor: pointer; padding: 2px 4px; border-radius: 4px;
}
.crumb:hover { background: var(--bg-3); }
.crumb.current { color: var(--text-0); font-weight: 600; cursor: default; }
.crumb-sep { color: var(--text-2); font-size: 12px; }
.add-btn {
  border: 1px solid var(--accent); background: transparent; color: var(--accent);
  border-radius: var(--radius-sm); font-size: 12px; padding: 4px 10px; cursor: pointer;
}
.add-btn:hover { background: var(--accent); color: #06251c; }
.anchor-hint { font-size: 12px; color: var(--text-2); padding: 4px 2px; }
</style>
