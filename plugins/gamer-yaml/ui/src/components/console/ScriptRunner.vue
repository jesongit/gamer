<template>
  <section class="runner-workspace">
    <div class="resource-toolbar">
      <ScriptPicker v-if="ctx.runKind === 'script' && !(editing && !ctx.shell.resourceId)" :model-value="ctx.selScript" :package="ctx.packageId" :lock-package="true" :auto-pick="!editing" :disabled="ctx.store.running || ctx.startPending || ctx.shell.saving || ctx.navigationPending" @update:model-value="selectScript" />
      <select v-else-if="isFunction" class="select resource-select" :value="functionPickerValue" aria-label="选择函数" :disabled="loading || ctx.store.running || ctx.startPending || ctx.shell.saving || ctx.navigationPending" @change="selectFunction($event.target.value)">
        <option value="">选择函数…</option><option v-if="editing && !ctx.filteredFnViews.some(v => v.name === ctx.editFocusFn && v.fileId === ctx.shell.resourceId)" :value="functionPickerValue">{{ ctx.editFocusFn }}（未保存）</option><option v-for="view in ctx.filteredFnViews" :key="fnKey(view)" :value="fnKey(view)">{{ view.name }}{{ fnIsDefault(view) ? '' : ` · ${view.category}` }}</option>
      </select>
      <span v-else class="resource-select">{{ ctx.shell.scriptDisplayName || '新脚本' }}</span>
      <button class="btn resource-action" :disabled="!editing || !(ctx.shell.dirty || namePending) || navigationBusy" @click="save">保存</button>
      <button v-if="!ctx.store.running" class="btn btn-primary resource-action" :disabled="!hasTarget || !ctx.store.deviceId || navigationBusy" @click="run()">{{ ctx.startPending ? '提交中' : '运行' }}</button>
      <button v-else class="btn btn-danger resource-action" :disabled="ctx.runStopping" @click="ctx.stopScript">{{ ctx.runStopping ? '停止中' : '停止' }}</button>
      <details ref="moreEl" class="resource-more"><summary class="btn btn-icon menu-summary" title="更多操作" aria-label="更多操作"><UiIcon name="more" /></summary><div class="resource-menu action-menu">
        <button class="action-menu-item" @click="newTarget" :disabled="navigationBusy">新建{{ isFunction ? '函数' : '脚本' }}</button>
        <button class="action-menu-item" @click="openRename" :disabled="!editing || ctx.store.running || ctx.startPending || ctx.shell.saving || ctx.navigationPending">重命名</button>
        <button class="action-menu-item" @click="openRaw" :disabled="!hasTarget || navigationBusy">原文编辑</button>
        <button class="action-menu-item" @click="ctx.showYaml = !ctx.showYaml; closeMore()" :disabled="!editing">查看生成 YAML</button>
        <button class="action-menu-item" @click="showRunDetails = !showRunDetails; closeMore()">日志</button>
        <button class="action-menu-item" @click="ctx.shell.undo(); closeMore()" :disabled="!editing || !ctx.shell.canUndo || navigationBusy">撤销</button>
        <button class="action-menu-item" @click="ctx.shell.redo(); closeMore()" :disabled="!editing || !ctx.shell.canRedo || navigationBusy">重做</button>
        <span class="action-menu-separator" role="separator"></span>
        <button class="action-menu-item danger" @click="removeTarget" :disabled="!hasTarget || navigationBusy">删除</button>
      </div></details>
    </div>
    <div v-show="!showRunDetails" class="editor-toolbar">
      <input v-if="editing" ref="renameInput" v-model="renameDraft" class="input editor-name"
        :aria-label="isFunction ? '函数名称' : '脚本名称'"
        :placeholder="isFunction ? '函数名称' : '脚本名称'"
        :disabled="navigationBusy"
        :aria-invalid="!!nameError" :title="nameError || '修改名称后按回车或移开焦点确认，点击保存写入'"
        @input="nameError = ''" @blur="rename" @keydown.enter.prevent.stop="rename"
        @keydown.esc.prevent.stop="resetName"
      />
      <button v-if="editing" type="button" class="btn btn-sm add-step" :disabled="navigationBusy"
        :class="{ active: canvasEl?.panelOpen }" title="添加步骤" @click.stop="openSteps($event.currentTarget)">步骤</button>
      <button type="button" class="btn btn-sm details-entry"
        title="运行日志与历史记录" @click="showRunDetails = true">日志</button>
      <button v-if="editing && ctx.shell.canJumpBack" type="button" class="btn btn-sm jump-back"
        :disabled="navigationBusy" :title="`保存并返回 ${ctx.shell.jumpBackLabel}`" @click="backToCaller">返回</button>
    </div>
    <span v-if="nameError && !showRunDetails" class="name-error" role="alert">{{ nameError }}</span>
    <div v-show="!showRunDetails" class="editor-view">
    <div v-if="ctx.scriptMode === 'raw'" class="raw-edit">
      <div class="raw-actions"><button class="btn" @click="saveRawAndReturn" :disabled="ctx.raw.loading || ctx.raw.saving || ctx.store.running">保存原文</button><button class="btn" @click="backFromRaw">返回</button></div>
      <textarea v-if="!ctx.raw.loading" v-model="ctx.raw.content" class="raw-editor" :disabled="ctx.raw.saving || ctx.store.running" spellcheck="false" aria-label="YAML 原文编辑区"></textarea>
    </div>
    <div v-else-if="editing" class="script-edit">
      <div v-if="!ctx.shell.resourceId && isFunction" class="edit-name-row">新函数：{{ ctx.editFocusFn }}</div>
      <span v-if="ctx.store.running" class="editor-readonly">运行中 · 编辑已锁定</span>
    <div class="canvas-wrap" v-if="ctx.shell.hasModel" :inert="navigationBusy">
      <!-- 参数/配置常驻在步骤列表上方：脚本 = 文件级 params + 运行配置；函数库 = 当前函数
           params（functionPath 指到 functions.<名>.params，随画布顶部函数下拉联动），
           函数库没有文件级 config -->
      <details class="extras"><summary>参数与变量声明</summary>
        <ParamEditor
          ref="paramEditorEl"
          :model="ctx.shell.model" :stack="ctx.shell.stack" :diagnostics="ctx.shell.diagnostics"
          :function-path="ctx.shell.editorContext === 'function' ? fnParamsPath : null"
          :templates="ctx.templateNames"
          :show-add-button="true"
        />
      </details>
      <StepCanvas
        ref="canvasEl"
        :test-from="!ctx.store.running"
        :show-error-panel="ctx.shell.diagnostics.length > 0"
        compact-toolbar
        @test-from="runFrom"
        :model="ctx.shell.model"
        :stack="ctx.shell.stack"
        :diagnostics="ctx.shell.diagnostics"
        :templates="ctx.templateNames"
        :selected-uuid="ctx.shell.selectedUuid"
        :initial-fn="ctx.editFocusFn"
        :lock-fn="ctx.shell.kind === 'function_library'"
        hide-function-toolbar
        @select="(u) => ctx.shell.select(u)"
      />
    </div>
    <div v-else-if="ctx.store.running && !isFunction && ctx.summaryModel" class="sum-wrap"><ScriptSummary :model="ctx.summaryModel" :readonly="true" :active-top="activeTop" :error-top="errorTop" /></div>
    <div v-else class="script-view-empty">编辑器加载中…</div>
    <YamlPreview v-if="ctx.showYaml && ctx.shell.hasModel" :model="ctx.shell.model" :filename="ctx.shell.name || 'script.yaml'" @close="ctx.showYaml = false" />
    <SaveConflictModal
      :open="!!ctx.shell.conflict"
      :resource="ctx.shell.conflict?.resource || ''"
      :message="ctx.shell.conflict?.message || ''"
      @reload="ctx.onConflictReload"
      @overwrite="ctx.onConflictOverwrite"
      @close="ctx.onConflictDismiss"
    />

    </div>
    <div v-else-if="hasTarget" class="script-view-empty" :aria-busy="loading || ctx.shell.loading">
      <span :role="loadError ? 'alert' : 'status'">{{ loadError || (ctx.store.running ? '运行结束后自动显示编辑器' : '正在加载编辑器…') }}</span>
      <button v-if="loadError && !navigationBusy" class="btn" @click="retryEditor">重新加载</button>
    </div>
    <div v-else class="script-view-empty">选择{{ isFunction ? '函数' : '脚本' }}开始编辑，或从更多菜单新建</div>
    </div>
    <RunDetails v-show="showRunDetails" :visible="showRunDetails && active" :device-id="ctx.store.deviceId" :target="detailTarget" :live-run="detailLiveRun" @edit="showRunDetails = false" @locate="locateJournalStep" />
    <RunErrorLocation v-if="errorLocation" :event="errorLocation" @close="errorLocation = null" />
    <ResourcePreviewModal :preview="ctx.resourcePreview" @close="ctx.closeResourcePreview" />
  </section>
</template>
<script setup>
/**
 * 自动化/函数共用的内联工作台：选择、显式保存、运行、更多管理入口。
 * 画布复用 StepCanvas；原文、版本冲突、参数流程保留独立语义。
 * 运行时冻结编辑，运行前保存失败则中止；详细事件按需打开。
 * 编辑模型由 Console 持有，资源类型由面板上下文锁定。
 */
import { WORKSPACE_CONTEXT_KEY } from '../../../../../../web/src/workspace/context'
import { SE_FUNCTION_NAVIGATION } from '../../script-editor/targets'
import { useConfirmDialog } from '../../../../../../web/src/components/ui/useConfirmDialog'
import { computed, defineAsyncComponent, onMounted, onActivated, onDeactivated, onUnmounted, nextTick, reactive, ref, watch, inject, provide } from 'vue'
import UiIcon from '../../../../../../web/src/components/ui/UiIcon.vue'
import { useOperationStatus } from '../../../../../../web/src/components/ui/useOperationStatus'
import ScriptPicker from '../ScriptPicker.vue'
import ScriptSummary from './ScriptSummary.vue'
import ResourcePreviewModal from './ResourcePreviewModal.vue'
import SaveConflictModal from './SaveConflictModal.vue'
import { StepCanvas, ParamEditor, YamlPreview } from '../../script-editor/components/index'
import RunDetails from './RunDetails.vue'
import { api } from '../../../../../../web/src/api'
import { runEventTopIndex, useRunEvents } from './useRunEvents'
import { findRun, runRegistry } from '../../../../../../web/src/store'

const props = defineProps({ context: { type: Object, required: true } })
const RunErrorLocation = defineAsyncComponent(() => import('./RunErrorLocation.vue'))
const confirmDialog = useConfirmDialog()
const errorLocation = ref(null)
const ctx = reactive(props.context)
const canvasEl = ref(null)
const paramEditorEl = ref(null)

// 运行可视化（P12.6）：事件 feed 数据源（Console 壳分发的 se 运行事件）+
// 当前/失败步骤的顶层卡片序号（嵌套路径映射其顶层祖先，如 steps[2].then[1] → 2）
const runEvents = useRunEvents()
const activeTop = computed(() => runEventTopIndex(runEvents.activePath))
const errorTop = computed(() => runEventTopIndex(runEvents.errorPath))
const currentRunRecord = computed(() => findRun(ctx.store.runId) || (String(runRegistry.last?.device_id || '') === String(ctx.store.deviceId || '') ? runRegistry.last : null))

/** 当前编辑函数名（画布顶部下拉透出）→ 函数级 params 容器路径，ParamEditor 按 it 编辑。 */
const fnParamsPath = computed(() => {
  const fnName = canvasEl.value?.activeFnName
  return fnName ? ['functions', fnName, 'params'] : null
})

const isFunction = computed(() => ctx.runKind === 'func')
const editing = computed(() => ctx.scriptMode === 'edit' && ctx.shell.hasModel && (ctx.shell.kind === 'function_library') === isFunction.value)
const loading = ref(false), loadError = ref(''), selectedFunctionKey = ref(''), moreEl = ref(null), showRunDetails = ref(false)
const workspace = inject(WORKSPACE_CONTEXT_KEY, null)
const navigationBusy = computed(() => !!(ctx.store.running || ctx.startPending || ctx.shell.saving || ctx.shell.loading || ctx.navigationPending || loading.value))
watch(showRunDetails, () => canvasEl.value?.closeAdd())
async function openSteps(target) {
  if (navigationBusy.value) return
  showRunDetails.value = false
  await nextTick()
  canvasEl.value?.openAdd(target)
}
provide(SE_FUNCTION_NAVIGATION, reactive({ busy: navigationBusy, open: jumpToFunction }))
async function openEditorPanel(panel) {
  if (panel) await workspace?.uiBridge.workspace.openPanel(panel)
}
async function jumpToFunction(target, uuid) {
  if (navigationBusy.value || !rename()) return
  await openEditorPanel(await ctx.jumpToFunction(target, uuid))
}
async function backToCaller() {
  if (navigationBusy.value || !rename()) return
  await openEditorPanel(await ctx.jumpBack())
}
const renameDraft = ref(''), renameInput = ref(null), nameError = ref('')
const editingName = computed(() => (isFunction.value ? ctx.editFocusFn : ctx.shell.scriptDisplayName) || '')
const namePending = computed(() => editing.value && renameDraft.value.trim() !== editingName.value)
function resetName() { renameDraft.value = editingName.value; nameError.value = '' }
// 改名撤销/重做仍跟随同一个函数对象，避免焦点落到库中第一个函数。
watch(() => ctx.shell.model?.functions?.find(fn => fn.name === ctx.editFocusFn), (fn, previous) => {
  if (!fn && previous && ctx.shell.model?.functions?.includes(previous)) ctx.editFocusFn = previous.name
}, { flush: 'sync' })
watch([editingName, () => ctx.shell.model, editing], resetName, { immediate: true })
const fnKey = view => `${view.fileId}#${view.name}`
const selectedFunction = computed(() => ctx.filteredFnViews.find(v => fnKey(v) === selectedFunctionKey.value))
const functionPickerValue = computed(() => editing.value && isFunction.value ? `${ctx.shell.resourceId || ''}#${ctx.editFocusFn}` : selectedFunctionKey.value)
const hasTarget = computed(() => editing.value || (isFunction.value ? !!selectedFunction.value : !!ctx.selScript))
const active = ref(true)
const detailTarget = computed(() => isFunction.value ? `${ctx.packageId}#${ctx.editFocusFn || selectedFunction.value?.name || ''}` : ctx.selScript || ctx.shell.resourceId || '')
const detailLiveRun = computed(() => {
  const rec = currentRunRecord.value
  return rec && rec.device_id === ctx.store.deviceId && rec.run_id === ctx.store.runId ? rec.run_id : ''
})
watch([() => ctx.store.runId, detailLiveRun], ([id, matching]) => { if (id && (!currentRunRecord.value?.entrypoint || matching === id)) showRunDetails.value = true }, { immediate: true })
watch(() => ctx.store.deviceId, () => { showRunDetails.value = !!ctx.store.runId && detailLiveRun.value === ctx.store.runId })
async function locateJournalStep(event) {
  const source = event.trace?.source
  if (!source) return
  const fileId = `${source.package_id}/${source.path?.slice('automations/'.length)}`
  if (editing.value && fileId === ctx.shell.resourceId && !ctx.shell.dirty && ctx.shell.version === source.version && (!source.function || source.function === ctx.editFocusFn)) {
    showRunDetails.value = false
    await nextTick()
    let path = event.path || event.trace.path
    if (source.function && !path.startsWith('functions.')) path = path.startsWith(`${source.function}.`) ? `functions.${path}` : `functions.${source.function}.${path}`
    canvasEl.value?.locate({ step_path: path, message: event.error || '运行步骤', field: '' })
  } else if (source.function && source.package_id === ctx.packageId && !navigationBusy.value && !ctx.shell.dirty) {
    try {
      const file = await api.getFunction(fileId)
      if (file.version !== source.version) { errorLocation.value = event; return }
      const panel = await ctx.jumpToFunction(source.function)
      if (!panel) { errorLocation.value = event; return }
      ctx.pendingRunLocation = event
      await openEditorPanel(panel)
    } catch { errorLocation.value = event }
  } else errorLocation.value = event
}
watch([() => ctx.pendingRunLocation, editing, active], async ([event, ready, visible]) => {
  if (!event || !ready || !visible || event.trace?.source?.function !== ctx.editFocusFn || !isFunction.value) return
  ctx.pendingRunLocation = null
  await locateJournalStep(event)
}, { immediate: true })
function closeMore() { if (moreEl.value) moreEl.value.open = false }
async function allowSwitch() {
  if (navigationBusy.value) return false
  const ownsModel = (ctx.shell.kind === 'function_library') === isFunction.value
  if ((ctx.shell.dirty || namePending.value || (ctx.scriptMode === 'raw' && ctx.raw.dirty)) && !await confirmDialog('有未保存修改，放弃后将切换编辑内容。', { title: '放弃修改', confirmText: '放弃并切换', danger: true })) return false
  if (ctx.scriptMode === 'raw') ctx.cancelRawScript()
  if (!ownsModel || ctx.shell.dirty) ctx.shell.reset()
  return true
}
let selectionGeneration = 0
async function loadSelected() {
  if (ctx.store.running || !hasTarget.value) return
  const generation = ++selectionGeneration
  loading.value = true; loadError.value = ''
  try {
    if (isFunction.value) { if (selectedFunction.value) await ctx.editFunction(selectedFunction.value) }
    else if (ctx.selScript) await ctx.editCurrentTarget()
    if (generation === selectionGeneration && !editing.value) loadError.value = '资源未能加载，请重试或重新选择。'
  } catch (e) {
    if (generation === selectionGeneration) loadError.value = `加载失败：${e.message}`
  } finally { if (generation === selectionGeneration) loading.value = false }
}
let autoOpenRequested = true
const selectedEditorKey = computed(() => `${ctx.packageId}:${isFunction.value ? `function:${selectedFunctionKey.value}` : `script:${ctx.selScript}`}`)
const editorMatchesSelection = computed(() => editing.value && (!ctx.shell.resourceId || (isFunction.value
  ? !selectedFunction.value || ctx.shell.resourceId === selectedFunction.value.fileId && ctx.editFocusFn === selectedFunction.value.name
  : ctx.shell.resourceId === ctx.selScript)))
async function ensureSelectedEditor() {
  if (!autoOpenRequested || !active.value || showRunDetails.value || ctx.scriptMode === 'raw') return
  if (editorMatchesSelection.value) { autoOpenRequested = false; return }
  if (!hasTarget.value || navigationBusy.value || loadError.value) return
  const key = selectedEditorKey.value
  autoOpenRequested = false
  if (!await allowSwitch()) {
    if (active.value && key === selectedEditorKey.value) loadError.value = '保留了当前未保存的修改。保存后会自动打开所选资源。'
    return
  }
  // An explicit jump/load may have started while the switch guard was pending.
  if (!active.value || key !== selectedEditorKey.value || editorMatchesSelection.value) return
  if (navigationBusy.value) { autoOpenRequested = true; return }
  await loadSelected()
}
function retryEditor() { autoOpenRequested = true; loadError.value = ''; return ensureSelectedEditor() }
async function selectScript(id) {
  if (id === ctx.selScript && editing.value) return
  if (!await allowSwitch()) return
  ctx.selScript = id
  if (id) await loadSelected()
}
async function selectFunction(key) {
  if (key === selectedFunctionKey.value && editing.value) return
  if (!await allowSwitch()) return
  if (ctx.shell.hasModel) ctx.shell.reset()
  selectedFunctionKey.value = key
  if (key) await loadSelected()
}
async function saveRawAndReturn() { await ctx.saveRawScript(); if (ctx.scriptMode === 'run') await loadSelected() }
async function backFromRaw() { if (ctx.raw.dirty && !await confirmDialog('原文有未保存修改，放弃后将返回可视化编辑。', { title: '放弃修改', confirmText: '放弃修改', danger: true })) return; ctx.cancelRawScript(); await loadSelected() }
async function save() {
  if (!rename()) return false
  const result = await ctx.saveEditScript({ keepOpen: true })
  return result?.ok === true && !ctx.shell.dirty
}
async function run(fromUuid = null) {
  if (navigationBusy.value) return
  if (editing.value && !rename()) return
  const functionName = editing.value ? ctx.editFocusFn : selectedFunction.value?.name
  const steps = isFunction.value ? ctx.shell.model?.functions?.find(f => f.name === functionName)?.run || selectedFunction.value?.model?.run || [] : ctx.shell.model?.run || []
  const startIndex = fromUuid ? steps.findIndex(s => s.uuid === fromUuid) : 0
  if (fromUuid && !(startIndex >= 0)) return
  if (editing.value && (ctx.shell.dirty || !ctx.shell.resourceId) && !await save()) return
  if (isFunction.value) {
    await ctx.runFunction({ fnName: functionName, startIndex })
  } else await ctx.runScript({ startIndex: startIndex || 0 })
}
function runFrom(uuid) { return run(uuid) }
async function newTarget() { closeMore(); if (await allowSwitch()) await ctx.startNewTarget() }
async function openRaw() { closeMore(); if (await allowSwitch()) await ctx.editRawCurrentTarget(selectedFunction.value) }
async function openRename() { closeMore(); showRunDetails.value = false; await nextTick(); renameInput.value?.focus(); renameInput.value?.select() }
function rename() {
  if (!namePending.value) return true
  if (!editing.value || ctx.store.running || ctx.startPending || ctx.shell.saving || ctx.navigationPending) return false
  const name = renameDraft.value.trim()
  if (!name) { nameError.value = '名称不能为空'; return false }
  if (isFunction.value) {
    if (!ctx.renameEditingFunction(ctx.editFocusFn, name)) { nameError.value = '无法使用此函数名，请检查是否重名'; return false }
  }
  else ctx.shell.scriptDisplayName = name
  resetName()
  return true
}
async function removeTarget() {
  closeMore()
  const target = ctx.shell.resourceId, script = ctx.selScript, fn = selectedFunctionKey.value
  if (!await confirmDialog(`删除当前${isFunction.value ? '函数' : '脚本'}？未保存修改也会丢弃。`, { title: '删除资源', confirmText: '删除', danger: true })) return
  if (ctx.shell.resourceId !== target || ctx.selScript !== script || selectedFunctionKey.value !== fn || ctx.store.running) return
  let deleted
  if (isFunction.value) deleted = await ctx.deleteFunction(selectedFunction.value)
  else { deleted = await ctx.deleteCurrentTarget(); if (!deleted && ctx.scriptDeleteConfirmId === ctx.selScript) deleted = await ctx.deleteCurrentTarget() }
  if (deleted) { ctx.shell.reset(); selectedFunctionKey.value = ''; ctx.scriptMode = 'run'; await loadSelected() }
}

function locateError() {
  if (runEvents.errorEvent?.trace?.source) { locateJournalStep(runEvents.errorEvent); return }
  const failure = runEvents.errorEvent
  const source = failure?.trace?.source
  if (source) {
    const sameFile = `${source.package_id}/${source.path?.slice('automations/'.length)}` === ctx.shell.resourceId
    if (editing.value && sameFile && !ctx.shell.dirty && ctx.shell.version === source.version && (!source.function || source.function === ctx.editFocusFn)) {
      const path = source.function && !failure.path.startsWith('functions.')
        ? failure.path.startsWith(`${source.function}.`) ? `functions.${failure.path}` : `functions.${source.function}.${failure.path}` : failure.path
      canvasEl.value?.locate({ step_path: path, message: failure.error || '运行报错', field: '' })
    } else errorLocation.value = failure
    return
  }
  if (!editing.value || errorTop.value === null) { showRunDetails.value = true; return }
  const target = isFunction.value ? `${ctx.shell.pkg}#${ctx.editFocusFn}` : ctx.shell.resourceId
  // 终态会清空 store.runScript，使用保留的运行记录核对完整 Package/资源身份。
  // 事件尚不含完整调用栈：涉及跨函数调用时打开详情，避免跳进错误文档。
  if (currentRunRecord.value?.entrypoint !== target || runEvents.list.some(e => e.ev === 'call_start')) { showRunDetails.value = true; return }
  const path = runEvents.errorPath.replace(/^steps\[/, 'run[')
  canvasEl.value?.locate({ step_path: isFunction.value ? `functions.${ctx.editFocusFn}.${path}` : path, message: '运行报错', field: '' })
}
useOperationStatus(() => {
  if (!active.value) return undefined
  const last = runEvents.list?.at(-1)
  const source = runEvents.errorEvent?.trace?.source
  const samePackage = !source?.package_id || source.package_id === ctx.packageId
  const failure = runEvents.errorEvent?.error || last?.error || runEvents.errorPath
  if (samePackage && (runEvents.errorPath || last?.ev === 'run_end' && last?.ok === false) && !ctx.store.running) return { text: `运行报错${ctx.shell.dirty ? ' · 未保存' : ''}${runEvents.errorPath ? ' · ' + runEvents.errorPath : ''}`, tone: 'error', actions: [...(runEvents.errorPath ? [{ label: '跳转', run: locateError }] : []), { label: '详情', run: () => { showRunDetails.value = true } }, { label: '复制', copy: typeof failure === 'string' ? failure : JSON.stringify(failure) }] }
  if (ctx.store.running) return { text: ctx.store.runStep || '运行中', actions: [{ label: '详情', run: () => { showRunDetails.value = true } }] }
  if (editing.value) return { text: ctx.shell.saving ? '保存中…' : ctx.shell.dirty || namePending.value ? '未保存' : '已保存' }
  return null
})
watch(() => ctx.packageId, () => { selectedFunctionKey.value = '' })
watch([() => ctx.filteredFnViews.map(fnKey).join('|'), editing, () => ctx.editFocusFn, () => ctx.shell.resourceId], () => {
  if (isFunction.value && editing.value) {
    const current = ctx.filteredFnViews.find(v => v.name === ctx.editFocusFn && v.fileId === ctx.shell.resourceId)
    if (current) selectedFunctionKey.value = fnKey(current)
  }
  if (isFunction.value && !selectedFunctionKey.value && !ctx.shell.dirty && !editing.value) {
    selectedFunctionKey.value = ctx.filteredFnViews[0] ? fnKey(ctx.filteredFnViews[0]) : ''
  }
}, { immediate: true })
watch([active, selectedEditorKey, () => ctx.store.running, showRunDetails,
  () => ctx.shell.dirty, () => ctx.shell.loading, () => ctx.shell.saving, () => ctx.navigationPending, loading], (values, previous) => {
  const [enabled, key, running, showingLogs, dirty] = values
  if (!previous || enabled !== previous[0] || key !== previous[1] || previous[2] && !running
    || previous[3] && !showingLogs || previous[4] && !dirty && loadError.value) {
    autoOpenRequested = true; loadError.value = ''
  }
  void ensureSelectedEditor()
}, { immediate: true, flush: 'post' })
onMounted(() => { ctx.refreshNativeDefaults?.() })
onActivated(() => { active.value = true; ctx.refreshNativeDefaults?.() })
onDeactivated(() => { active.value = false; confirmDialog.cancel() })
onUnmounted(() => { active.value = false })

/** 函数来源徽标：默认库 = 「默认」，手动拆分文件 = 文件短名。 */
function fnIsDefault(view) {
  const file = String(view?.category || '')
  return file === '_function'
}
function fnSourceLabel(view) {
  return fnIsDefault(view) ? '默认' : (view?.category || '默认')
}
function fnSourceTitle(view) {
  return fnIsDefault(view)
    ? 'Package 默认函数库 automations/_function.yaml（可编辑）'
    : `手动拆分函数库 automations/${view.category}.yaml（可编辑、可运行）`
}

</script>

<style scoped>
.editor-view{display:flex;flex:1;min-height:0;flex-direction:column}
.editor-toolbar{display:flex;align-items:center;gap:6px;flex:none;min-width:0}
.editor-toolbar .btn{flex:none;white-space:nowrap;height:28px;align-self:center}
.editor-toolbar .btn.active{color:var(--accent);border-color:var(--accent)}
.editor-name{flex:1 1 0;min-width:0;width:0;height:28px;box-sizing:border-box;font-size:13px;padding:4px 8px}
.editor-name[aria-invalid="true"]{border-color:var(--danger)}
.name-error{flex-basis:100%;font-size:12px;color:var(--danger)}
.script-run{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.auto-run{display:flex;flex-wrap:nowrap;gap:8px}.auto-run .spicker{width:auto;flex:1 1 auto;min-width:0}.auto-run .select{flex:1;min-width:0}.auto-run .fn-file{flex:1 1 auto;min-width:0}.auto-run .fn-search{flex:1 1 auto;min-width:0}.sum-wrap{flex:1;min-height:0;overflow:auto;display:flex;flex-direction:column;gap:8px}.sum-wrap .script-summary{flex:none}.sum-wrap .script-view-empty{flex:none;min-height:160px}.fn-sum{display:flex;flex-direction:column;gap:4px}.fn-sum-head{display:flex;align-items:center;gap:8px;font-size:12px;font-weight:600;color:var(--accent);padding:2px 2px 0}.fn-cat{flex:none;max-width:40%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size: 12px;color:var(--text-2);border:1px solid var(--border);border-radius:4px;padding:1px 6px;background:var(--bg-2)}
.fn-cat.default{color:var(--accent);border-color:var(--accent)}.fn-sig{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.fn-actions{display:flex;align-items:center;gap:4px;flex:none}.fn-edit-btn,.fn-delete-btn{flex:none;font-size: 12px;padding:2px 8px;border:1px solid var(--border);background:var(--bg-2);color:var(--text-1);border-radius:var(--radius-sm);cursor:pointer}.fn-edit-btn:hover{color:var(--accent);border-color:var(--accent)}.fn-delete-btn{color:var(--danger)}.fn-delete-btn:hover:not(:disabled){color:var(--danger);border-color:var(--danger)}.run-actions{display:flex;gap:8px}.run-actions .btn{flex:1}.run-actions .more-wrap{position:relative;flex:1}.run-actions .more-wrap .btn{width:100%}.more-mask{position:fixed;inset:0;z-index:20}.script-view-empty{flex:1;display:flex;align-items:center;justify-content:center;color:var(--text-2);font-size:12px;background:var(--bg-0);border:1px dashed var(--border);border-radius:var(--radius-sm)}.script-edit{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.edit-name-row{display:flex}.edit-name-row .input{flex:1;min-width:0;width:100%}.edit-actions{display:flex;gap:8px;flex-wrap:wrap;align-items:center}.edit-actions .btn{flex:1;justify-content:center;min-width:0}.edit-actions .btn.active{border-color:var(--accent-2);color:var(--accent-2);background:color-mix(in srgb, var(--accent) 8%, transparent)}.dirty-badge{flex:none;font-size: 12px;color:var(--warn);border:1px solid var(--warn);border-radius:4px;padding:1px 6px}.function-edit-toolbar{display:flex;align-items:center;gap:8px;min-height:30px}
.function-edit-file{flex:0 1 auto;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px;color:var(--accent);border:1px dashed var(--border);border-radius:4px;padding:4px 10px}
.function-edit-name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px;color:var(--accent)}.function-edit-toolbar .btn{flex:none}.jump-back{flex:none;align-self:flex-start}.canvas-wrap{flex:1;min-height:0;overflow:auto;display:flex;flex-direction:column;position:relative}.canvas-wrap :deep(.se-canvas){flex:none}.extras{display:flex;flex-direction:column;flex:none;margin-bottom:8px}.mono{font-family:var(--mono);font-size: 12px;color:var(--text-1)}
.raw-edit{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.raw-actions{display:flex;gap:8px;flex:none}.raw-actions .btn{flex:1;justify-content:center}.raw-editor{flex:1;min-height:240px;width:100%;box-sizing:border-box;resize:none;padding:12px;border:1px solid var(--border);border-radius:var(--radius-sm);background:var(--bg-0);color:var(--text-0);line-height:1.5;tab-size:2;outline:none}.raw-editor:focus{border-color:var(--accent)}
.fn-sum { gap: 5px; }
.fn-sum-head { gap: 10px; font-size: 14px; padding: 4px 2px 1px; }
.fn-sig { font-size: 14px; line-height: 1.4; }
.fn-actions { gap: 6px; }
.fn-run-btn, .fn-edit-btn, .fn-delete-btn {
  flex: none; font-size: 13px; padding: 5px 10px; white-space: nowrap;
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: var(--radius-sm); cursor: pointer;
}
.fn-run-btn { color: var(--accent); }
.fn-run-btn.danger { color: var(--danger); }
.fn-run-btn:hover, .fn-edit-btn:hover {
  color: var(--accent); border-color: var(--accent);
}
.fn-run-btn.danger:hover { color: var(--danger); border-color: var(--danger); }
.fn-delete-btn { color: var(--danger); }
.fn-delete-btn:hover:not(:disabled) { color: var(--danger); border-color: var(--danger); }
.fn-run-btn:disabled, .fn-edit-btn:disabled, .fn-delete-btn:disabled { opacity: .5; cursor: not-allowed; }
.run-actions { flex-wrap: wrap; }
.run-actions .btn { min-width: 0; flex: 1 1 96px; }
.function-edit-name.input {
  flex: 1; min-width: 0; width: auto; padding: 6px 10px;
  font-size: 16px; font-weight: 600; color: var(--accent);
}
.runner-workspace{display:flex;flex-direction:column;flex:1;min-height:0;gap:9px}.resource-toolbar{display:flex;gap:6px;align-items:center;flex:none}.resource-toolbar .spicker,.resource-select{flex:1;min-width:0}.resource-action{height:28px;min-width:60px;justify-content:center;flex:none}.resource-more{position:relative;flex:none}.resource-menu{position:absolute;right:0;top:calc(100% + 4px);z-index:30}.script-edit{gap:7px}.extras{border-bottom:1px solid var(--border);padding:5px 0;margin-bottom:8px}.extras summary{cursor:pointer;font-size:12px;color:var(--text-1);padding:2px 0 7px}.editor-readonly,.readonly-note{font-size:12px;color:var(--text-1)}.run-details{max-height:35%;overflow:auto;border-top:1px solid var(--border);font-size:12px}.run-details summary{display:flex;justify-content:space-between;align-items:center}.raw-editor{font-family:var(--mono);font-size:13px;color:var(--text-0)}.script-view-empty{gap:12px;flex-direction:column}.rename-modal{width:400px}.mono{font-size:12px}
</style>
