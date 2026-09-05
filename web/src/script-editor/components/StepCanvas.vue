<template>
  <div ref="rootEl" class="se-canvas" @click.self="deselect">
    <!-- 函数编辑的紧凑外壳把函数名/添加步骤放在参数区上方；这里仅保留嵌套流程导航。 -->
    <div v-if="!hideFunctionToolbar || !atRoot" class="canvas-toolbar">
      <template v-if="isFunction && !hideFunctionToolbar">
        <!-- 重命名态：下拉变输入框 + 确认按钮（Enter 确认 / Esc 取消） -->
        <input
          v-if="renaming" v-model="renameDraft"
          class="select fn-select fn-input" aria-label="函数新名字"
          placeholder="函数名"
          @click.stop @keydown.enter.stop="confirmRename" @keydown.esc.stop="cancelRename"
        />
        <!-- 锁定态：进入时已指定函数（Console 逐函数「编辑」直达），函数名静态展示，
             不再提供切换下拉——换函数回运行区点对应函数的「编辑」 -->
        <span
          v-else-if="lockFn"
          class="fn-select fn-static"
          :title="`当前编辑函数：${activeFnName}（换函数请返回后点对应函数的「编辑」）`"
        >{{ activeFnName }}</span>
        <select
          v-else
          class="select fn-select"
          :value="activeFnName"
          aria-label="选择要编辑的函数"
          title="切换到所选函数（列出文件内全部函数）"
          @click.stop
          @change="onFnSelectChange(($event.target as HTMLSelectElement).value)"
        >
          <option v-for="name in fnNames" :key="name" :value="name">{{ name }}</option>
        </select>
        <button
          v-if="renaming" type="button"
          class="fn-btn fn-rename"
          title="确认重命名（新名字重命名当前函数；已有函数名则切换到它）"
          @click.stop="confirmRename"
        >确认</button>
        <button
          v-else type="button"
          class="fn-btn fn-rename"
          title="重命名当前函数（输入已有函数名则切换到它）"
          @click.stop="beginRename"
        >重命名</button>
      </template>
      <!-- 非根视图（专注/面包屑导航后）：返回上一个专注视图，而不是固定跳回根流程 -->
      <button
        v-if="!atRoot" type="button" class="fn-btn back-btn"
        :title="focusHistory.length ? '返回上一个专注视图' : `返回${rootLabel}步骤列表`"
        @click.stop="goBack"
      >← 返回上一级</button>
      <button
        v-if="!hideFunctionToolbar" ref="addButtonEl" type="button" class="add-btn"
        :class="{ active: panelOpen }" title="添加步骤（选择后插入到当前锚点）"
        @click.stop="openAdd($event.currentTarget)"
      >+ 步骤</button>
      <template v-if="isFunction && !hideFunctionToolbar">
        <button
          type="button"
          class="fn-btn fn-add"
          title="在文件末尾新增一个空函数（func1/func2… 顺延命名，「重命名」按钮改名），画布切到新函数"
          @click.stop="addFunction"
        >＋ 函数</button>
        <button
          type="button"
          class="fn-btn fn-btn-danger"
          :disabled="fnNames.length <= 1"
          :title="fnNames.length <= 1 ? '至少保留一个函数' : `删除函数 ${activeFnName}（其 params 与 steps 一并移除，可撤销）`"
          @click.stop="removeActiveFn"
        >删除</button>
      </template>
      <!-- 面包屑：进入嵌套分支后显示「函数名 / 命中后 / …」逐层导航；根视图只有
           函数名/主流程一个节点，与函数下拉重复 → 隐藏。
           独占工具条下一行（flex-basis 100% 换行），不再与按钮抢宽度被挤成竖排；
           单行不折行，放不下把中段折叠成 …（尾部=当前层级尽量多留），见 fitBreadcrumb -->
      <nav
        v-if="breadcrumbNodes.length > 1"
        ref="breadcrumbEl"
        class="breadcrumb"
        :class="{ 'clip-tail': crumbClip }"
        aria-label="当前编辑流程"
      >
        <template v-for="(item, i) in visibleCrumbs" :key="item.kind === 'node' ? item.index : 'ellipsis'">
          <span v-if="i" class="crumb-sep">/</span>
          <button
            v-if="item.kind === 'node'"
            type="button"
            class="crumb"
            :class="{ current: item.index === breadcrumbNodes.length - 1 }"
            :title="item.node.label"
            @click.stop="navigateTo(item.node)"
          >{{ item.node.label }}</button>
          <span v-else class="crumb-ellipsis" :title="crumbHiddenTitle">…</span>
        </template>
      </nav>
    </div>

    <!-- 插入锚点提示（§8.4/§10：可见「下一条将插入：主流程 / 第 N 步之后」） -->
    <div class="anchor-hint">下一条将插入：{{ anchorLabel }}</div>

    <!-- 添加步骤紧凑下拉：不遮挡整个画布；嵌套容器「+ 添加」只记住插入目标，
         选择步骤后由同一条 CommandStack 插入。 -->
    <div v-if="panelOpen" class="add-dropdown-wrap" @click.stop>
      <AddStepPanel
        :style="addMenuStyle"
        :context="context"
        :stack="stack"
        :anchor="anchor"
        :target-label="anchorLabel"
        @inserted="onInserted"
        @close="closeAdd"
      />
    </div>

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
      :test-from="testFromActive"
      @select="onSelect"
      @toggle-expand="toggleExpand"
      @focus="enterFocus"
      @add-here="onAddHere"
      @test-from="(u: string) => emit('test-from', u)"
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
    type: Function as PropType<((target: string) => { params: ParamDecl[] } | null) | undefined>,
    default: undefined,
  },
  /** 模板短名候选（tmpl 控件 datalist）。 */
  templates: { type: Array as PropType<string[]>, default: () => [] },
  /** 渲染 ErrorSummary 并接管定位联动。 */
  showErrorPanel: { type: Boolean, default: false },
  /** CellEditor 的参数引用列表；缺省时按模型自动取（脚本 = 文件级，函数库 = 当前函数）。 */
  params: { type: Array as PropType<ParamDecl[]>, default: null as unknown as ParamDecl[] },
  /** 开启「从此步骤测试函数」入口（阶段 5）：仅函数库页签 + 当前容器为函数体根时生效。 */
  testFrom: { type: Boolean, default: false },
  /** 进入时聚焦的函数名（Console 摘要区逐函数「编辑」/ func 跳转直达）；空或不存在回退默认。 */
  initialFn: { type: String, default: '' },
  /** 锁定函数切换：函数名静态展示、不渲染切换下拉（进入时已指定函数，换函数回运行区再进）。 */
  lockFn: { type: Boolean, default: false },
  /** 函数紧凑编辑态：函数名与添加步骤入口由外壳置于参数列表上方。 */
  hideFunctionToolbar: { type: Boolean, default: false },
})

const emit = defineEmits(['select', 'test-from'])

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

const isFunction = computed(() => 'functions' in props.model)
const fnNames = computed<string[]>(() =>
  isFunction.value ? (props.model as { functions: { name: string }[] }).functions.map((f) => f.name) : [],
)

const currentContainer = ref<Path>(rootContainerPath(props.model))
const focusPath = ref<Path | null>(null)
const focusHistory = ref<Path[]>([])

/** 根路径必须是当前编辑函数的函数体，而不是函数库中的第一个函数。 */
const rootPath = computed<Path>(() => {
  if (!isFunction.value) return ['steps']
  const currentFn = String(currentContainer.value[1] ?? '')
  const name = fnNames.value.includes(currentFn) ? currentFn : (fnNames.value[0] ?? '')
  return ['functions', name, 'steps']
})

function samePath(a: Path, b: Path): boolean {
  return a.length === b.length && a.every((segment, i) => segment === b[i])
}

watch(
  () => [props.model, props.initialFn] as const,
  () => {
    currentContainer.value = rootContainerPath(props.model)
    // 外壳指定进入函数：命中则画布直接落在该函数体（Model 复位与聚焦一次完成）
    const want = props.initialFn
    if (want && isFunction.value && fnNames.value.includes(want)) {
      currentContainer.value = ['functions', want, 'steps']
    }
    focusPath.value = null
    focusHistory.value = []
    innerSelected.value = null
  },
  { immediate: true },
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
// 测试入口仅出现在函数体根容器（专注视图进入深层分支后隐藏——start_index 只映射函数体顶层）
const testFromActive = computed(() =>
  props.testFrom && isFunction.value
  && activeContainer.value.length === 3 && activeContainer.value[2] === 'steps')
const activeLabel = computed(() => {
  const nodes = breadcrumbForContainer(props.model, activeContainer.value)
  return nodes.length ? nodes[nodes.length - 1]!.label : '主流程'
})
const basePathActive = computed(() => basePathOfContainer(activeContainer.value))

function enterFocus(path: Path): void {
  const current = activeContainer.value
  if (samePath(current, path)) return
  focusHistory.value.push([...current])
  focusPath.value = [...path]
  currentContainer.value = [...path]
}
function navigateTo(node: BreadcrumbNode): void {
  const root = rootPath.value
  const isRoot = node.containerPath.length === root.length && root.every((seg, i) => seg === node.containerPath[i])
  if (isRoot) {
    goRoot()
  } else {
    const target = node.containerPath
    if (samePath(activeContainer.value, target)) return
    const historyIndex = focusHistory.value.findIndex((path) => samePath(path, target))
    focusHistory.value = historyIndex >= 0
      ? focusHistory.value.slice(0, historyIndex)
      : focusHistory.value.slice(0, -1)
    focusPath.value = [...target]
    currentContainer.value = [...target]
  }
}

/** 当前视图是否在根容器（主流程/函数体）；非根时工具条出「返回」按钮。 */
const atRoot = computed(() => {
  const root = rootPath.value
  const cur = activeContainer.value
  return cur.length === root.length && root.every((seg, i) => seg === cur[i])
})
const rootLabel = computed(() => {
  const nodes = breadcrumbForContainer(props.model, rootPath.value)
  return nodes.length ? nodes[nodes.length - 1]!.label : '主流程'
})
function goRoot(): void {
  focusPath.value = null
  focusHistory.value = []
  currentContainer.value = rootPath.value
}
function goBack(): void {
  const previous = focusHistory.value.pop() ?? rootPath.value
  if (samePath(previous, rootPath.value)) {
    focusPath.value = null
    currentContainer.value = rootPath.value
  } else {
    focusPath.value = [...previous]
    currentContainer.value = [...previous]
  }
}
function switchFn(name: string): void {
  focusPath.value = null
  focusHistory.value = []
  currentContainer.value = ['functions', name, 'steps']
  innerSelected.value = null
  emit('select', null)
}

/** 下拉切换到所选函数（select 值即目标函数名，画布跟到该函数体）。 */
function onFnSelectChange(name: string): void {
  if (!name || name === activeFnName.value) return
  switchFn(name)
}

/** 重命名态：函数下拉原地变输入框，确认按钮提交；Enter 确认 / Esc 取消。 */
const renaming = ref(false)
const renameDraft = ref('')

function beginRename(): void {
  renameDraft.value = activeFnName.value
  renaming.value = true
}
function cancelRename(): void {
  renaming.value = false
}
function confirmRename(): void {
  const to = renameDraft.value.trim()
  const current = activeFnName.value
  if (!renaming.value) return
  renaming.value = false
  if (!to || to === current) return
  if (fnNames.value.includes(to)) {
    switchFn(to)
    return
  }
  if (props.stack.apply({ type: 'rename_function', from: current, to }, `重命名函数 ${current} → ${to}`)) {
    focusPath.value = null
    focusHistory.value = []
    currentContainer.value = ['functions', to, 'steps']
    innerSelected.value = null
    emit('select', null)
  }
}

/** 新增空函数并切到它（func1/func2… 顺延命名，改名走上方输入框）。 */
function addFunction(): void {
  if (!isFunction.value) return
  const fns = (props.model as { functions: { name: string }[] }).functions
  let i = 1
  while (fns.some((f) => f.name === `func${i}`)) i++
  const name = `func${i}`
  if (props.stack.apply({ type: 'insert_function', name }, `新增函数 ${name}`)) {
    focusPath.value = null
    focusHistory.value = []
    currentContainer.value = ['functions', name, 'steps']
    innerSelected.value = null
    emit('select', null)
  }
}

/** 删除当前函数（至少保留一个，命令栈可撤销）；画布回退到首个函数。 */
function removeActiveFn(): void {
  if (!isFunction.value || fnNames.value.length <= 1) return
  const name = activeFnName.value
  if (!name || !window.confirm(`删除函数 ${name}？（其 params 与 steps 一并移除，可撤销）`)) return
  if (props.stack.apply({ type: 'remove_function', name }, `删除函数 ${name}`)) {
    focusPath.value = null
    focusHistory.value = []
    currentContainer.value = rootPath.value
    innerSelected.value = null
    emit('select', null)
  }
}

// ---------- 插入锚点 ----------

function isPrefixPath(prefix: Path, path: Path): boolean {
  return prefix.length <= path.length && prefix.every((seg, i) => seg === path[i])
}

  /** 容器级「+ 添加」记住的插入目标容器；仅影响锚点，不切换视图（下拉开时有效）。 */
const pendingAddPath = ref<Path | null>(null)

const anchor = computed(() => {
  // 容器级「+ 添加」的待插入目标：嵌套容器末尾（不切换视图，下拉关闭即清除）
  if (pendingAddPath.value) {
    const p = sanitize(pendingAddPath.value)
    return { containerPath: p, index: resolveStepList(props.model, p).length }
  }
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

// ---------- 添加面板（紧凑下拉态） ----------

const panelOpen = ref(false)
const addButtonEl = ref<HTMLElement | null>(null)
const addAnchorEl = ref<HTMLElement | null>(null)
const addMenuStyle = ref<Record<string, string>>({})

function updateAddMenuPosition(): void {
  const target = addAnchorEl.value ?? addButtonEl.value
  if (!target) return
  const rect = target.getBoundingClientRect()
  const width = Math.min(320, Math.max(220, window.innerWidth - 32))
  const left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8))
  const panel = rootEl.value?.querySelector('.add-step-panel') as HTMLElement | null
  // 先清掉上一次计算的高度，避免滚动后菜单只能保持此前较小的高度。
  if (panel) panel.style.maxHeight = ''
  const panelHeight = panel?.getBoundingClientRect().height ?? 0
  const viewportHeight = window.innerHeight || document.documentElement.clientHeight
  const margin = 8
  const gap = 4
  const spaceBelow = Math.max(0, viewportHeight - rect.bottom - margin)
  const spaceAbove = Math.max(0, rect.top - margin)
  // 下方放不下时翻到锚点上方；两边都放不下则选空间更大的一侧并滚动菜单内容。
  const openAbove = panelHeight > spaceBelow && spaceAbove > spaceBelow
  const available = openAbove ? spaceAbove : spaceBelow
  const top = openAbove
    ? Math.max(margin, rect.top - Math.min(panelHeight, available) - gap)
    : Math.max(margin, rect.bottom + gap)
  addMenuStyle.value = {
    position: 'fixed',
    left: `${left}px`,
    top: `${top}px`,
    maxHeight: `${Math.max(0, available)}px`,
  }
}

function setAddAnchor(target: unknown): void {
  addAnchorEl.value = target instanceof HTMLElement ? target : addButtonEl.value
  void nextTick(updateAddMenuPosition)
}

function watchAddMenuPosition(): void {
  window.addEventListener('resize', updateAddMenuPosition)
  window.addEventListener('scroll', updateAddMenuPosition, true)
  void nextTick(updateAddMenuPosition)
}

function unwatchAddMenuPosition(): void {
  window.removeEventListener('resize', updateAddMenuPosition)
  window.removeEventListener('scroll', updateAddMenuPosition, true)
}

function openAdd(target?: unknown): void {
  pendingAddPath.value = null
  const next = !panelOpen.value
  panelOpen.value = next
  if (next) {
    setAddAnchor(target)
    watchAddMenuPosition()
  } else {
    unwatchAddMenuPosition()
    addAnchorEl.value = null
    addMenuStyle.value = {}
  }
}
function closeAdd(): void {
  panelOpen.value = false
  pendingAddPath.value = null
  unwatchAddMenuPosition()
  addAnchorEl.value = null
  addMenuStyle.value = {}
}
function onAddHere(path: Path, target?: unknown): void {
  // 容器级「+ 添加」（含嵌套分支）：只记住插入目标并展开下拉，视图不再切进子流程
  // （此前会整屏跳到该容器，只有顶部面包屑能返回，容易被困在子流程里）
  innerSelected.value = null
  emit('select', null)
  pendingAddPath.value = path
  panelOpen.value = true
  setAddAnchor(target)
  watchAddMenuPosition()
}
function onInserted(uuid: string): void {
  closeAdd()
  innerSelected.value = uuid
  emit('select', uuid)
  expandedUuids.add(uuid) // 新卡自动展开：省去每次手动点开再填/选
  flashCard(uuid) // 嵌套插入不切视图：闪烁 + 滚动定位新卡落点
}

/** 瞬态高亮 + 滚动到卡片（诊断定位与插入落点共用）。 */
function flashCard(uuid: string): void {
  highlightUuid.value = uuid
  if (highlightTimer) clearTimeout(highlightTimer)
  highlightTimer = setTimeout(() => {
    highlightUuid.value = null
  }, 1800)
  void nextTick(() => {
    const el = rootEl.value?.querySelector(`[data-step-uuid="${uuid}"]`)
    el?.scrollIntoView?.({ block: 'center' })
  })
}

// ---------- 面包屑 ----------

const breadcrumbNodes = computed<BreadcrumbNode[]>(() => {
  if (sel.value) {
    const nodes = breadcrumb(props.model, sel.value)
    if (nodes.length) return nodes
  }
  return breadcrumbForContainer(props.model, activeContainer.value)
})

// ---------- 面包屑单行折叠：独占一行仍放不下时，中段折叠成 …（尾部尽量多留） ----------
// 收敛顺序：保首节点、从左往右收中段（尾段整体保留）→ 连「首/…/尾」都放不下才丢
// 首节点、从右往左尽量多留尾段 → 仍超长交给 clip-tail（右对齐+左裁剪）兜底，
// 尾部（当前所在层级）始终优先可见，任何情况下不折行。

const breadcrumbEl = ref<HTMLElement | null>(null)
const crumbTailStart = ref(1) // 尾段首下标：其前（首节点之后）的节点折叠为 …
const crumbShowHead = ref(true)
const crumbClip = ref(false)

type CrumbItem = { kind: 'node'; node: BreadcrumbNode; index: number } | { kind: 'ellipsis' }

const visibleCrumbs = computed<CrumbItem[]>(() => {
  const nodes = breadcrumbNodes.value
  const items: CrumbItem[] = []
  if (!nodes.length) return items
  const tailFrom = crumbShowHead.value ? 1 : 0
  if (crumbShowHead.value) items.push({ kind: 'node', node: nodes[0]!, index: 0 })
  if (crumbTailStart.value > tailFrom) items.push({ kind: 'ellipsis' })
  for (let i = Math.max(crumbTailStart.value, tailFrom); i < nodes.length; i++) {
    items.push({ kind: 'node', node: nodes[i]!, index: i })
  }
  return items
})

/** … 悬停提示：被折叠的中段完整路径。 */
const crumbHiddenTitle = computed(() => {
  const nodes = breadcrumbNodes.value
  const from = crumbShowHead.value ? 1 : 0
  return nodes.slice(from, Math.max(crumbTailStart.value, from)).map((n) => n.label).join(' / ')
})

/** 溢出判定：子项 offsetWidth 求和 + gap。不用 scrollWidth——clip-tail 的左侧
 *  溢出不计入 scrollWidth（LTR 不可滚动），会误判为放得下。 */
function crumbOverflows(nav: HTMLElement): boolean {
  const gap = Number.parseFloat(getComputedStyle(nav).columnGap) || 0
  let w = 0
  for (const el of Array.from(nav.children)) w += (el as HTMLElement).offsetWidth
  return w + gap * Math.max(0, nav.children.length - 1) > nav.clientWidth + 1
}

let crumbFitToken = 0

/** 测量收敛循环：每次改折叠状态后等 DOM 提交再量；token 防旧轮覆盖新轮。
 *  测试环境（happy-dom）无布局、宽度恒 0 → 永不折叠，行为与全显示一致。 */
async function fitBreadcrumb(): Promise<void> {
  const nav = breadcrumbEl.value
  if (!nav) return
  const token = ++crumbFitToken
  const dirty = !crumbShowHead.value || crumbTailStart.value !== 1 || crumbClip.value
  crumbShowHead.value = true
  crumbTailStart.value = 1
  crumbClip.value = false
  if (dirty) await nextTick()
  if (token !== crumbFitToken) return
  const total = breadcrumbNodes.value.length
  if (total <= 1 || !crumbOverflows(nav)) return
  if (total === 2) {
    // 只有首尾两个节点：先试丢首节点（…/尾），仍放不下才左裁剪兜底
    crumbShowHead.value = false
    await nextTick()
    if (token !== crumbFitToken) return
    if (!crumbOverflows(nav)) return
    crumbClip.value = true
    return
  }
  // 保首节点：中段从左往右逐个折叠（尾段整体保留）
  for (let start = 2; start <= total - 1; start++) {
    crumbTailStart.value = start
    await nextTick()
    if (token !== crumbFitToken) return
    if (!crumbOverflows(nav)) return
  }
  // 连「首/…/尾」都放不下：丢首节点，从右往左尽量多留尾段
  crumbShowHead.value = false
  crumbTailStart.value = total - 1
  await nextTick()
  if (token !== crumbFitToken) return
  if (crumbOverflows(nav)) { crumbClip.value = true; return }
  for (let start = total - 2; start >= 1; start--) {
    crumbTailStart.value = start
    await nextTick()
    if (token !== crumbFitToken) return
    if (crumbOverflows(nav)) { crumbTailStart.value = start + 1; return }
  }
}

watch(breadcrumbNodes, () => { void fitBreadcrumb() })

// 容器宽度变化（窗口/侧栏拖动）重收敛；RO 首次 observe 也回一次 → 覆盖挂载首轮
let crumbRo: ResizeObserver | null = null
watch(breadcrumbEl, (el) => {
  if (!el) { crumbRo?.disconnect(); return }
  if (!crumbRo && typeof ResizeObserver !== 'undefined') {
    crumbRo = new ResizeObserver(() => { void fitBreadcrumb() })
  }
  crumbRo?.observe(el)
  void fitBreadcrumb()
})

// ---------- 诊断定位 ----------

function locate(diag: Diagnostic): void {
  const hit = locateDiagnostic(props.model, diag)
  if (!hit) return
  expandedUuids.add(hit.uuid)
  for (const u of hit.ancestorUuids) expandedUuids.add(u)
  // 宿主容器嵌套超过一层 → 专注到该容器；否则回根视图
  if (containerNesting(hit.containerPath) > 1) {
    enterFocus(hit.containerPath)
  } else if (!isFunction.value) {
    focusPath.value = null
    currentContainer.value = rootPath.value
  }
  innerSelected.value = hit.uuid
  emit('select', hit.uuid)
  flashCard(hit.uuid)
}

onBeforeUnmount(() => {
  if (highlightTimer) clearTimeout(highlightTimer)
  crumbRo?.disconnect()
  unwatchAddMenuPosition()
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
 * - locate：错误面板独立挂载在画布外（全屏外壳右侧常驻）时由宿主转发定位；
 * - activeFnName：函数库当前编辑的函数名（全屏外壳按它把 ParamEditor 指到函数级 params）。
 */
defineExpose({ locate, activeFnName, openAdd })
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
/* 全局 .select 是 width:100% 大表单形态；工具条内收窄为内联尺寸 */
.fn-select {
  width: auto; max-width: 180px;
  padding: 4px 8px; font-size: 12px;
  font-family: var(--mono);
}
.fn-input { min-width: 120px; max-width: 180px; }
/* 锁定态函数名：与下拉同占位，静态展示（强调色 + 加粗，不可切换） */
.fn-static {
  display: inline-flex; align-items: center;
  color: var(--accent); font-weight: 600;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.fn-btn {
  border: 1px solid var(--border); background: transparent; color: var(--text-1);
  border-radius: var(--radius-sm); font-size: 12px; padding: 4px 10px; cursor: pointer;
  white-space: nowrap;
}
.fn-btn:hover:not(:disabled) { color: var(--accent); border-color: var(--accent); }
.fn-btn-danger:hover:not(:disabled) { color: var(--danger); border-color: var(--danger); }
.fn-btn:disabled { opacity: .4; cursor: not-allowed; }
/* 面包屑独占工具条下一行（flex-basis 100% 借 toolbar 换行），单行不折行：
   放不下按「中段折叠成 …」收敛（fitBreadcrumb），clip-tail 兜底右对齐+左裁剪 */
.breadcrumb {
  flex: 0 0 100%; min-width: 0;
  display: flex; align-items: center; gap: 4px;
  white-space: nowrap; overflow: hidden;
}
.breadcrumb.clip-tail { justify-content: flex-end; }
.crumb {
  flex: none;
  border: none; background: transparent; color: var(--accent-2);
  font-size: 12px; cursor: pointer; padding: 2px 4px; border-radius: 4px;
}
.crumb:hover { background: var(--bg-3); }
.crumb.current { color: var(--text-0); font-weight: 600; cursor: default; }
.crumb-sep { flex: none; color: var(--text-2); font-size: 12px; }
.crumb-ellipsis { flex: none; color: var(--text-2); font-size: 12px; padding: 2px; }
.add-btn {
  border: 1px solid var(--accent); background: transparent; color: var(--accent);
  border-radius: var(--radius-sm); font-size: 12px; padding: 4px 10px; cursor: pointer;
}
.add-btn:hover { background: var(--accent); color: #06251c; }
.anchor-hint { font-size: 12px; color: var(--text-2); padding: 4px 2px; }
.add-dropdown-wrap { position: static; min-height: 0; margin: 0; }
.back-btn { color: var(--accent-2); }
</style>
