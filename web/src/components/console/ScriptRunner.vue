<template>
  <div v-if="ctx.scriptMode === 'run'" class="script-run">
    <div class="auto-run">
      <!-- 资源类型随面板锁定（console.scripts=脚本 / console.functions=函数），
           不再提供面板内切换；ctx.runKind 为锁定的面板类型 -->
      <ScriptPicker v-if="ctx.runKind === 'script'" v-model="ctx.selScript" :package="ctx.activePkg" :lock-package="true" />
      <select v-else v-model="ctx.selFnFile" class="select mono fn-file" title="函数库文件（data/<应用分区>/functions/）">
        <option value="" disabled>选择函数库文件…</option>
        <option v-for="f in ctx.fnLib.list" :key="f.id" :value="f.id">{{ f.file }}</option>
      </select>
    </div>
    <div class="run-actions">
      <template v-if="ctx.runKind === 'func'">
        <button class="btn" @click="ctx.startNewTarget">新建文件</button>
        <button class="btn" :disabled="!ctx.selTargetId" @click="ctx.addFunctionToCurrentFile">添加函数</button>
        <button class="btn" :disabled="!ctx.selTargetId" @click="ctx.editRawCurrentTarget">原文编辑</button>
        <button class="btn btn-danger" :disabled="!ctx.selTargetId" @click="ctx.deleteCurrentTarget">删除文件</button>
      </template>
      <template v-else>
        <button v-if="!ctx.store.running" class="btn btn-primary" :disabled="!ctx.canRunTarget || !ctx.store.deviceId || ctx.startPending" @click="ctx.runScript()">{{ ctx.startPending ? '提交中…' : '运行脚本' }}</button>
        <button v-else class="btn btn-danger" :disabled="ctx.runStopping" @click="ctx.stopScript">{{ ctx.runStopping ? '停止中…' : '停止脚本' }}</button>
        <button class="btn" @click="ctx.startNewTarget">新建脚本</button>
        <button class="btn" :disabled="!ctx.selTargetId" @click="ctx.editCurrentTarget()">编辑脚本</button>
        <button class="btn" :disabled="!ctx.selTargetId" @click="ctx.editRawCurrentTarget">原文编辑</button>
        <button
          class="btn btn-danger" :disabled="!ctx.selTargetId"
          @click="ctx.deleteCurrentTarget"
        >{{ ctx.scriptDeleteConfirmId === ctx.selScript ? '确认删除' : '删除脚本' }}</button>
      </template>
    </div>
    <RunLogPanel :context="ctx" :on-mounted="ctx.onLogBoxMounted" />
    <!-- 摘要区独立滚动：panel-sec 是 overflow:hidden，内容超高（多函数分组/长脚本）必须在这里滚动 -->
    <div v-if="!ctx.store.running || ctx.runKind === 'func'" class="sum-wrap">
      <!-- 函数模式：逐函数分组摘要（每组=函数签名 + 该函数体顶层卡片 + 直达编辑按钮） -->
      <template v-if="ctx.runKind === 'func' && ctx.selTargetId">
        <template v-if="ctx.funcFnViews.length">
          <div v-for="view in ctx.funcFnViews" :key="view.name" class="fn-sum">
            <div class="fn-sum-head mono">
              <span class="fn-sig" :title="fnSignature(view)">{{ fnSignature(view) }}</span>
              <span class="fn-actions">
                <button
                  v-if="!ctx.store.running || !isRunningFunction(view.name)"
                  type="button" class="fn-run-btn" :disabled="ctx.store.running || !ctx.canRunTarget || !ctx.store.deviceId || ctx.startPending"
                  :title="`运行函数 ${view.name}`" @click="ctx.runScript({ fnName: view.name })"
                >{{ ctx.startPending ? '提交中…' : '▶ 运行' }}</button>
                <button
                  v-else type="button" class="fn-run-btn danger" :disabled="ctx.runStopping"
                  title="停止当前函数" @click="ctx.stopScript"
                >{{ ctx.runStopping ? '停止中…' : '■ 停止' }}</button>
                <button type="button" class="fn-edit-btn" :disabled="ctx.store.running" :title="`编辑函数 ${view.name}`" @click="ctx.editCurrentTarget(view.name)">编辑</button>
                <button type="button" class="fn-delete-btn" :disabled="ctx.store.running" :title="`删除函数 ${view.name}`" @click="deleteFn(view.name)">删除</button>
              </span>
            </div>
            <ScriptSummary
              :model="view.model"
              :error="ctx.funcSummaryError"
              @run-from="ctx.runFromStep"
              @open-target="ctx.openScriptTarget"
            />
          </div>
        </template>
        <div v-else class="script-view-empty">{{ ctx.funcSummaryError || '该函数库文件没有函数（编辑态画布「＋ 函数」添加）' }}</div>
      </template>
      <ScriptSummary
        v-else-if="ctx.selTargetId"
        :model="ctx.summaryModel"
        :error="ctx.summaryError"
        @run-from="ctx.runFromStep"
        @open-target="ctx.openScriptTarget"
      />
      <div v-else class="script-view-empty">请选择脚本</div>
    </div>
    <ResourcePreviewModal :preview="ctx.resourcePreview" @close="ctx.closeResourcePreview" />
  </div>
  <div v-else-if="ctx.scriptMode === 'raw'" class="raw-edit">
    <div class="raw-actions">
      <button class="btn btn-primary" :disabled="ctx.raw.loading || ctx.raw.saving" @click="ctx.saveRawScript">{{ ctx.raw.saving ? '保存中…' : '💾 保存' }}</button>
      <button class="btn" :disabled="ctx.raw.loading || ctx.raw.saving" @click="ctx.cancelRawScript">取消</button>
    </div>
    <textarea
      v-if="!ctx.raw.loading"
      v-model="ctx.raw.content"
      class="raw-editor mono"
      spellcheck="false"
      autofocus
      aria-label="YAML 原文编辑区"
    ></textarea>
    <div v-else class="script-view-empty">原文加载中…</div>
  </div>
  <div v-else class="script-edit" @focusout="ctx.autoSaveDebounced()">
    <div class="edit-name-row"><input v-model="ctx.shell.name" class="input mono" :autofocus="ctx.shell.kind === 'function_library'" :placeholder="ctx.shell.kind === 'function_library' ? '函数库文件短名（缺省 .yaml 自动补）' : '脚本名称（可省略 .yml 后缀）'" @keydown.enter="ctx.saveEditScript" /></div>
    <div class="edit-actions">
      <button class="btn btn-primary" :disabled="ctx.shell.saving || !ctx.shell.hasModel" @click="ctx.saveEditScript">{{ ctx.shell.saving ? '保存中…' : '💾 保存' }}</button>
      <button class="btn" @click="ctx.cancelEditScript">取消</button>
      <button class="btn" :disabled="!ctx.shell.canUndo" title="撤销" @click="ctx.shell.undo()">↶</button>
      <button class="btn" :disabled="!ctx.shell.canRedo" title="重做" @click="ctx.shell.redo()">↷</button>
      <button class="btn" :class="{ active: ctx.showYaml }" title="只读生成 YAML（诊断预览，不可编辑）" @click="ctx.showYaml = !ctx.showYaml">诊断</button>
      <span v-if="ctx.shell.dirty" class="dirty-badge" title="存在未保存修改">未保存</span>
    </div>
    <button v-if="ctx.shell.canJumpBack" class="btn btn-sm jump-back" @click="ctx.jumpBack()">← 返回 {{ ctx.shell.jumpBackLabel }}</button>
    <div v-if="ctx.shell.kind === 'function_library'" class="function-edit-toolbar">
      <input
        v-model="functionNameDraft"
        class="function-edit-name input mono"
        :title="`当前编辑函数：${editFunctionName}`"
        placeholder="函数名"
        autofocus
        @focus="beginFunctionNameEdit"
        @blur="commitFunctionName"
        @keydown.enter.prevent="commitFunctionName"
      />
      <button type="button" class="btn btn-sm" title="在当前函数参数列表末尾添加参数" @click="paramEditorEl?.addParam?.()">＋ 添加参数</button>
      <button type="button" class="btn btn-sm" title="在当前函数末尾添加步骤" @click="openFunctionStepPicker">＋ 添加步骤</button>
    </div>
    <div class="canvas-wrap" v-if="ctx.shell.hasModel">
      <!-- 参数/配置常驻在步骤列表上方：脚本 = 文件级 params + 运行配置；函数库 = 当前函数
           params（functionPath 指到 functions.<名>.params，随画布顶部函数下拉联动），
           函数库没有文件级 config -->
      <div class="extras">
        <ParamEditor
          ref="paramEditorEl"
          :model="ctx.shell.model" :stack="ctx.shell.stack" :diagnostics="ctx.shell.diagnostics"
          :function-path="ctx.shell.editorContext === 'function' ? fnParamsPath : null"
          :templates="ctx.templateNames"
          :show-add-button="ctx.shell.editorContext !== 'function'"
        />
        <DefaultsEditor v-if="ctx.shell.editorContext === 'script'" :model="ctx.shell.model" :stack="ctx.shell.stack" />
      </div>
      <StepCanvas
        ref="canvasEl"
        :model="ctx.shell.model"
        :stack="ctx.shell.stack"
        :diagnostics="ctx.shell.diagnostics"
        :context="ctx.shell.editorContext"
        :templates="ctx.templateNames"
        :selected-uuid="ctx.shell.selectedUuid"
        :resolve-target="ctx.resolveTargetSync"
        :initial-fn="ctx.editFocusFn"
        :lock-fn="ctx.shell.kind === 'function_library'"
        :hide-function-toolbar="ctx.shell.kind === 'function_library'"
        @select="(u) => ctx.shell.select(u)"
      />
    </div>
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
</template>

<script setup>
/**
 * Console 紧凑脚本外壳（阶段 4，plan §10.1）：替换旧 textarea 文本编辑区。
 * 同一组件渲染两个扩展面板（console.scripts / console.functions），资源类型由
 * 面板作用域上下文锁定（ctx.runKind，互不串台）：
 * - 运行态：文件下拉（脚本 = ScriptPicker 锁分区；函数 = 函数库文件）+ 运行/停止/
 *   编辑/文件操作（Target 系列按类型分发，新建/删除/编辑函数库文件与脚本同形）+
 *   实时日志 + 只读步骤摘要列表（脚本/函数体通用：「从此运行」选起点、call/func 结构化只读预览）；
 * - 结构化编辑态：StepCanvas 共享画布（面包屑/专注视图/诊断定位/添加面板均为组件现成能力）+
 *   撤销重做 + 参数/配置折叠区 + 只读 YAML 诊断预览 + 保存 409 冲突弹窗（SaveConflictModal）；
 * - 原文编辑态：原始 YAML textarea + 保存/取消，用于直接修复或保留文本内容；
 * - shell（useScriptEditorShell）由 Console 持有并传入，本组件只做编排。
 */
import { computed, reactive, ref, watch } from 'vue'
import ScriptPicker from '../ScriptPicker.vue'
import RunLogPanel from './RunLogPanel.vue'
import ScriptSummary from './ScriptSummary.vue'
import ResourcePreviewModal from './ResourcePreviewModal.vue'
import SaveConflictModal from './SaveConflictModal.vue'
import { StepCanvas, ParamEditor, DefaultsEditor, YamlPreview } from '../../script-editor/components/index'

const props = defineProps({ context: { type: Object, required: true } })
const ctx = reactive(props.context)
const canvasEl = ref(null)
const paramEditorEl = ref(null)

/** 当前编辑函数名（画布顶部下拉透出）→ 函数级 params 容器路径，ParamEditor 按 it 编辑。 */
const fnParamsPath = computed(() => {
  const fnName = canvasEl.value?.activeFnName
  return fnName ? ['functions', fnName, 'params'] : null
})

/** 函数签名展示：登录(account:账号模板, x) —— 参数名:备注（无备注只显示参数名），逗号分割 */
function fnSignature(view) {
  const ps = Array.isArray(view.model?.params) ? view.model.params : []
  const sig = ps.map(p => (p.remark ? `${p.name}:${p.remark}` : p.name)).join(', ')
  return `${view.name}(${sig})`
}

const editFunctionName = computed(() => canvasEl.value?.activeFnName || ctx.editFocusFn || '')
const functionNameDraft = ref('')
const functionNameEditing = ref(false)

watch(editFunctionName, (name) => {
  if (!functionNameEditing.value) functionNameDraft.value = name || ''
}, { immediate: true })

function beginFunctionNameEdit() {
  functionNameEditing.value = true
  functionNameDraft.value = editFunctionName.value || functionNameDraft.value
}

function commitFunctionName() {
  if (!functionNameEditing.value) return
  const current = editFunctionName.value
  const next = functionNameDraft.value.trim()
  functionNameEditing.value = false
  if (!next || next === current) {
    functionNameDraft.value = current
    return
  }
  if (ctx.renameEditingFunction(current, next)) functionNameDraft.value = next
  else functionNameDraft.value = current
}

function openFunctionStepPicker(event) {
  canvasEl.value?.openAdd?.(event?.currentTarget)
}

async function deleteFn(name) {
  await ctx.deleteFunction(name)
}

function isRunningFunction(name) {
  return ctx.store.running && String(ctx.store.runScript || '').endsWith(`· ${name}()`)
}

</script>

<style scoped>
.script-run{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.auto-run{display:flex;flex-wrap:nowrap;gap:8px}.auto-run .spicker{width:auto;flex:1 1 auto;min-width:0}.auto-run .select{flex:1;min-width:0}.auto-run .fn-file{flex:1 1 auto;min-width:0}.sum-wrap{flex:1;min-height:0;overflow:auto;display:flex;flex-direction:column;gap:8px}.sum-wrap .script-summary{flex:none}.sum-wrap .script-view-empty{flex:none;min-height:160px}.fn-sum{display:flex;flex-direction:column;gap:4px}.fn-sum-head{display:flex;align-items:center;gap:8px;font-size:12px;font-weight:600;color:var(--accent);padding:2px 2px 0}.fn-sig{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.fn-actions{display:flex;align-items:center;gap:4px;flex:none}.fn-edit-btn,.fn-delete-btn{flex:none;font-size:11px;padding:2px 8px;border:1px solid var(--border);background:var(--bg-2);color:var(--text-1);border-radius:var(--radius-sm);cursor:pointer}.fn-edit-btn:hover{color:var(--accent);border-color:var(--accent)}.fn-delete-btn{color:var(--danger)}.fn-delete-btn:hover:not(:disabled){color:var(--danger);border-color:var(--danger)}.run-actions{display:flex;gap:8px}.run-actions .btn{flex:1}.run-actions .more-wrap{position:relative;flex:1}.run-actions .more-wrap .btn{width:100%}.more-mask{position:fixed;inset:0;z-index:20}.more-dropdown{position:absolute;right:0;top:calc(100% + 4px);z-index:30;display:flex;flex-direction:column;min-width:120px;padding:4px;gap:2px;background:var(--bg-2);border:1px solid var(--border);border-radius:var(--radius-sm);box-shadow:0 8px 24px rgba(0,0,0,.4)}.more-item{display:flex;align-items:center;gap:6px;text-align:left;white-space:nowrap;padding:6px 10px;border:none;background:none;border-radius:var(--radius-sm);color:var(--text-0);font-size:12px;cursor:pointer}.more-item:hover{background:var(--bg-3)}.more-item:disabled{color:var(--text-2);opacity:.5;cursor:not-allowed}.more-item.danger:hover{color:var(--danger)}.script-view-empty{flex:1;display:flex;align-items:center;justify-content:center;color:var(--text-2);font-size:12px;background:var(--bg-0);border:1px dashed var(--border);border-radius:var(--radius-sm)}.script-edit{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.edit-name-row{display:flex}.edit-name-row .input{flex:1;min-width:0;width:100%}.edit-actions{display:flex;gap:8px;flex-wrap:wrap;align-items:center}.edit-actions .btn{flex:1;justify-content:center;min-width:0}.edit-actions .btn.active{border-color:var(--accent-2);color:var(--accent-2);background:rgba(56,189,248,.08)}.dirty-badge{flex:none;font-size:11px;color:var(--warn);border:1px solid var(--warn);border-radius:4px;padding:1px 6px}.function-edit-toolbar{display:flex;align-items:center;gap:8px;min-height:30px}.function-edit-name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:13px;color:var(--accent)}.function-edit-toolbar .btn{flex:none}.jump-back{flex:none;align-self:flex-start}.canvas-wrap{flex:1;min-height:0;overflow:auto;display:flex;flex-direction:column;position:relative}.canvas-wrap :deep(.se-canvas){flex:none}.extras{display:flex;flex-direction:column;flex:none;margin-bottom:8px}.mono{font-family:var(--mono);font-size:11px;color:var(--text-1)}
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
</style>
