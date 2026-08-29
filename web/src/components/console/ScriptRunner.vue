<template>
  <div v-if="ctx.scriptMode === 'run'" class="script-run">
    <div class="auto-run"><ScriptPicker v-model="ctx.selScript" :package="ctx.activePkg" /></div>
    <div class="run-actions">
      <button v-if="!ctx.store.running" class="btn btn-primary" :disabled="!ctx.selScript || !ctx.store.deviceId || ctx.startPending" @click="ctx.runScript()">{{ ctx.startPending ? '提交中…' : '▶ 运行' }}</button>
      <button v-else class="btn btn-danger" :disabled="ctx.runStopping" @click="ctx.stopScript">{{ ctx.runStopping ? '■ 停止中…' : '■ 停止' }}</button>
      <button class="btn" :disabled="!ctx.selScript" @click="ctx.editCurrentScript">编辑</button>
      <div class="more-wrap"><button class="btn" :class="{ active: ctx.moreOpen }" @click="ctx.moreOpen = !ctx.moreOpen">更多 ▾</button><div v-if="ctx.moreOpen" class="more-mask" @click="ctx.moreOpen = false"></div><div v-if="ctx.moreOpen" class="more-dropdown"><button class="more-item" @click="ctx.moreOpen = false; ctx.startNewScript()">＋ 新建</button><button class="more-item danger" :disabled="!ctx.selScript" @click="ctx.moreOpen = false; ctx.deleteCurrentScript()">🗑 删除</button></div></div>
    </div>
    <RunLogPanel :context="ctx" :on-mounted="ctx.onLogBoxMounted" />
    <template v-if="!ctx.store.running">
      <ScriptSummary
        v-if="ctx.selScript"
        :model="ctx.summaryModel"
        :error="ctx.summaryError"
        :run-uuid="ctx.runStartUuid"
        @toggle-run-start="ctx.toggleRunStart"
        @run-from="ctx.runFromStep"
        @open-target="ctx.openScriptTarget"
      />
      <div v-else class="script-view-empty">请选择脚本</div>
    </template>
  </div>
  <div v-else class="script-edit">
    <div class="edit-name-row"><input v-model="ctx.shell.name" class="input mono" placeholder="脚本名称（可省略 .yml 后缀）" @keydown.enter="ctx.saveEditScript" /></div>
    <div class="edit-actions">
      <button class="btn btn-primary" :disabled="ctx.shell.saving || !ctx.shell.hasModel" @click="ctx.saveEditScript">{{ ctx.shell.saving ? '保存中…' : '💾 保存' }}</button>
      <button class="btn" @click="ctx.cancelEditScript">取消</button>
      <button class="btn" :disabled="!ctx.shell.canUndo" title="撤销" @click="ctx.shell.undo()">↶</button>
      <button class="btn" :disabled="!ctx.shell.canRedo" title="重做" @click="ctx.shell.redo()">↷</button>
      <button class="btn" :class="{ active: ctx.showExtras }" title="脚本参数与运行配置" @click="ctx.showExtras = !ctx.showExtras">参数/配置</button>
      <button class="btn" :class="{ active: ctx.showYaml }" title="只读生成 YAML（诊断预览，不可编辑）" @click="ctx.showYaml = !ctx.showYaml">诊断</button>
      <button class="btn" :class="{ active: ctx.altMode }" @click="ctx.toggleAltMode" :title="ctx.altHint || '开启后投屏点击/滑动与模板 Alt 点击生成类型化步骤插入当前锚点'">⌥ alt 模式</button>
      <span v-if="ctx.shell.dirty" class="dirty-badge" title="存在未保存修改">未保存</span>
    </div>
    <button v-if="ctx.shell.canJumpBack" class="btn btn-sm jump-back" @click="ctx.jumpBack()">← 返回 {{ ctx.shell.jumpBackLabel }}</button>
    <div class="canvas-wrap" v-if="ctx.shell.hasModel">
      <StepCanvas
        ref="canvasEl"
        :model="ctx.shell.model"
        :stack="ctx.shell.stack"
        :diagnostics="ctx.shell.diagnostics"
        :context="ctx.shell.editorContext"
        :templates="ctx.templateNames"
        :selected-uuid="ctx.shell.selectedUuid"
        show-error-panel
        @select="(u) => ctx.shell.select(u)"
      />
      <!-- 录制上传进行中：锁定画布交互（占位不可跨分支拖动，plan §11.8「禁用即可」） -->
      <div v-if="ctx.recording && ctx.recording.uploading" class="canvas-lock" title="录制模板上传中，画布暂时锁定…"></div>
      <div v-if="ctx.showExtras && ctx.shell.editorContext === 'script'" class="extras">
        <ParamEditor :model="ctx.shell.model" :stack="ctx.shell.stack" :diagnostics="ctx.shell.diagnostics" />
        <ConfigEditor :model="ctx.shell.model" :stack="ctx.shell.stack" />
      </div>
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
 * - 运行态：ScriptPicker（锁分区）+ 运行/停止/更多 + 实时日志 + 只读步骤摘要列表
 *   （ScriptSummary：顶层卡片选运行起点、call/func 结构化跳转）；
 * - 编辑态：StepCanvas 共享画布（面包屑/专注视图/诊断定位/添加面板均为组件现成能力）+
 *   撤销重做 + 参数/配置折叠区 + 只读 YAML 诊断预览 + 保存 409 冲突弹窗（SaveConflictModal）；
 * - shell（useScriptEditorShell）由 Console 持有并传入，本组件只做编排；
 *   画布挂载后把 anchor 提供者注入 shell（Alt 生成的步骤与「添加步骤」面板同锚点插入）。
 */
import { nextTick, onMounted, reactive, ref, watch } from 'vue'
import ScriptPicker from '../ScriptPicker.vue'
import RunLogPanel from './RunLogPanel.vue'
import ScriptSummary from './ScriptSummary.vue'
import SaveConflictModal from './SaveConflictModal.vue'
import { StepCanvas, ParamEditor, ConfigEditor, YamlPreview } from '../../script-editor/components/index'

const props = defineProps({ context: { type: Object, required: true } })
const ctx = reactive(props.context)
const canvasEl = ref(null)

function syncCanvasRef() {
  nextTick(() => {
    ctx.shell.setAnchorProvider(() => canvasEl.value?.anchor ?? null)
  })
}
watch(canvasEl, syncCanvasRef)
onMounted(syncCanvasRef)
</script>

<style scoped>
.script-run{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.auto-run{display:flex;flex-wrap:wrap;gap:8px}.auto-run .spicker{flex:1 1 auto}.auto-run .select{flex:1;min-width:120px}.run-actions{display:flex;gap:8px}.run-actions .btn{flex:1}.run-actions .more-wrap{position:relative;flex:1}.run-actions .more-wrap .btn{width:100%}.more-mask{position:fixed;inset:0;z-index:20}.more-dropdown{position:absolute;right:0;top:calc(100% + 4px);z-index:30;display:flex;flex-direction:column;min-width:120px;padding:4px;gap:2px;background:var(--bg-2);border:1px solid var(--border);border-radius:var(--radius-sm);box-shadow:0 8px 24px rgba(0,0,0,.4)}.more-item{display:flex;align-items:center;gap:6px;text-align:left;white-space:nowrap;padding:6px 10px;border:none;background:none;border-radius:var(--radius-sm);color:var(--text-0);font-size:12px;cursor:pointer}.more-item:hover{background:var(--bg-3)}.more-item:disabled{color:var(--text-2);opacity:.5;cursor:not-allowed}.more-item.danger:hover{color:var(--danger)}.script-view-empty{flex:1;display:flex;align-items:center;justify-content:center;color:var(--text-2);font-size:12px;background:var(--bg-0);border:1px dashed var(--border);border-radius:var(--radius-sm)}.script-edit{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.edit-name-row{display:flex}.edit-name-row .input{flex:1;min-width:0;width:100%}.edit-actions{display:flex;gap:8px;flex-wrap:wrap;align-items:center}.edit-actions .btn{flex:1;justify-content:center;min-width:0}.edit-actions .btn.active{border-color:var(--accent-2);color:var(--accent-2);background:rgba(56,189,248,.08)}.dirty-badge{flex:none;font-size:11px;color:var(--warn);border:1px solid var(--warn);border-radius:4px;padding:1px 6px}.jump-back{flex:none;align-self:flex-start}.canvas-wrap{flex:1;min-height:0;overflow:auto;display:flex;flex-direction:column;position:relative}.canvas-wrap .canvas-lock{position:absolute;inset:0;z-index:6;cursor:wait;background:transparent}.canvas-wrap :deep(.se-canvas){flex:none}.extras{display:flex;flex-direction:column}.mono{font-family:var(--mono);font-size:11px;color:var(--text-1)}
</style>
