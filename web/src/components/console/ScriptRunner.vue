<template>
  <div v-if="ctx.scriptMode === 'run'" class="script-run">
    <div class="auto-run"><ScriptPicker v-model="ctx.selScript" :package="ctx.activePkg" /></div>
    <div class="run-actions">
      <button v-if="!ctx.store.running" class="btn btn-primary" :disabled="!ctx.selScript || !ctx.store.deviceId || ctx.startPending" @click="ctx.runScript">{{ ctx.startPending ? '提交中…' : '▶ 运行' }}</button>
      <button v-else class="btn btn-danger" :disabled="ctx.runStopping" @click="ctx.stopScript">{{ ctx.runStopping ? '■ 停止中…' : '■ 停止' }}</button>
      <button class="btn" :disabled="!ctx.selScript" @click="ctx.editCurrentScript">编辑</button>
      <div class="more-wrap"><button class="btn" :class="{ active: ctx.moreOpen }" @click="ctx.moreOpen = !ctx.moreOpen">更多 ▾</button><div v-if="ctx.moreOpen" class="more-mask" @click="ctx.moreOpen = false"></div><div v-if="ctx.moreOpen" class="more-dropdown"><button class="more-item" @click="ctx.moreOpen = false; ctx.startNewScript()">＋ 新建</button><button class="more-item danger" :disabled="!ctx.selScript" @click="ctx.moreOpen = false; ctx.deleteCurrentScript()">🗑 删除</button></div></div>
    </div>
    <RunLogPanel :context="ctx" :on-mounted="ctx.onLogBoxMounted" />
    <template v-if="!ctx.store.running"><div v-if="!ctx.selScript" class="script-view-empty">请选择脚本</div><div v-else class="script-view-wrap"><div class="run-hint">点击「- 」开头的逻辑行（含函数体内步骤）→ 从该步骤开始运行；点击函数名行 → 从头运行整个函数（先判 cond 再跑函数体）；再次点击选中行取消（从头运行）</div><div class="script-view mono"><div v-for="(line, idx) in ctx.scriptLines" :key="idx" class="sv-line" :class="{ sel: ctx.selectedLine === idx, selectable: !!ctx.runLineMap[idx] }" @click="ctx.onScriptLineClick(idx)"><template v-if="ctx.callLinks[idx]">{{ ctx.callLinks[idx].prefix }}<span class="call-link" title="点击预览脚本内容" @click.stop="ctx.openCallPreview(ctx.callLinks[idx].name)">{{ ctx.callLinks[idx].label || ctx.callLinks[idx].name }}</span>{{ ctx.callLinks[idx].suffix }}</template><template v-else>{{ line || ' ' }}</template></div></div></div></template>
  </div>
  <div v-else class="script-edit">
    <div class="edit-name-row"><input v-model="ctx.editScriptName" class="input mono" placeholder="脚本名称（可省略 .yml 后缀）" @keydown.enter="ctx.saveEditScript" /></div>
    <div class="edit-actions"><button class="btn btn-primary" :disabled="ctx.scriptSaving" @click="ctx.saveEditScript">{{ ctx.scriptSaving ? '保存中…' : '💾 保存' }}</button><button class="btn" @click="ctx.cancelEditScript">取消</button><button class="btn" :class="{ active: ctx.altMode }" @click="ctx.toggleAltMode" title="开启后投屏点击/滑动只生成操作记录，不发送控制指令">⌥ alt 模式</button></div>
    <div class="op-record"><div v-if="!ctx.opRecords.length" class="op-record-empty">请在alt模式下进行操作生成记录</div><div v-for="r in ctx.opRecords" :key="r.id" class="op-record-line mono" @click="ctx.applyOpRecord(r)">{{ r.text }}</div></div>
    <textarea ref="scriptEditor" v-model="ctx.editScriptCode" class="script-editor mono" spellcheck="false" placeholder="# YAML 脚本&#10;config:&#10;  interval: 500ms&#10;&#10;steps:&#10;  - find: 模板名.png&#10;    block: 障碍模板.png" @keydown.tab.prevent="ctx.onEditorTab"></textarea>
  </div>
</template>

<script setup>
import { nextTick, onMounted, reactive, ref, watch } from 'vue'
import ScriptPicker from '../ScriptPicker.vue'
import RunLogPanel from './RunLogPanel.vue'
const props = defineProps({ context: { type: Object, required: true }, onEditorMounted: { type: Function, required: true } })
const ctx = reactive(props.context)
const scriptEditor = ref(null)
async function emitEditorRef() {
  await nextTick()
  props.onEditorMounted(scriptEditor.value)
}
watch(() => ctx.scriptMode, emitEditorRef)
onMounted(emitEditorRef)
</script>

<style scoped>
.script-run{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.auto-run{display:flex;flex-wrap:wrap;gap:8px}.auto-run .spicker{flex:1 1 auto}.auto-run .select{flex:1;min-width:120px}.run-actions{display:flex;gap:8px}.run-actions .btn{flex:1}.run-actions .more-wrap{position:relative;flex:1}.run-actions .more-wrap .btn{width:100%}.more-mask{position:fixed;inset:0;z-index:20}.more-dropdown{position:absolute;right:0;top:calc(100% + 4px);z-index:30;display:flex;flex-direction:column;min-width:120px;padding:4px;gap:2px;background:var(--bg-2);border:1px solid var(--border);border-radius:var(--radius-sm);box-shadow:0 8px 24px rgba(0,0,0,.4)}.more-item{display:flex;align-items:center;gap:6px;text-align:left;white-space:nowrap;padding:6px 10px;border:none;background:none;border-radius:var(--radius-sm);color:var(--text-0);font-size:12px;cursor:pointer}.more-item:hover{background:var(--bg-3)}.more-item:disabled{color:var(--text-2);opacity:.5;cursor:not-allowed}.more-item.danger:hover{color:var(--danger)}.run-hint{font-size:11px;color:var(--text-2);flex-shrink:0}.script-view-wrap{flex:1;min-height:0;display:flex;flex-direction:column;gap:6px}.script-view{flex:1;min-height:0;overflow:auto;background:var(--bg-0);border:1px solid var(--border);border-radius:var(--radius-sm);padding:10px 12px;font-size:12px;line-height:1.65;color:#c9d4e8;user-select:none}.sv-line{white-space:pre;border-radius:4px;padding:0 6px;margin:0 -6px}.sv-line.selectable{cursor:pointer}.sv-line.selectable:hover{background:var(--bg-3)}.sv-line.sel{background:rgba(34,211,165,.12);color:var(--accent);box-shadow:inset 2px 0 0 var(--accent)}.call-link{color:var(--accent-2);cursor:pointer}.call-link:hover{text-decoration:underline}.script-view-empty{flex:1;display:flex;align-items:center;justify-content:center;color:var(--text-2);font-size:12px;background:var(--bg-0);border:1px dashed var(--border);border-radius:var(--radius-sm)}.script-edit{flex:6;display:flex;flex-direction:column;gap:10px;min-height:0}.edit-name-row{display:flex}.edit-name-row .input{flex:1;min-width:0;width:100%}.edit-actions{display:flex;gap:8px}.edit-actions .btn{flex:1;justify-content:center}.edit-actions .btn.active{border-color:var(--accent-2);color:var(--accent-2);background:rgba(56,189,248,.08)}.op-record{flex-shrink:0;height:77px;display:flex;flex-direction:column;background:var(--bg-0);border:1px solid var(--border);border-radius:var(--radius-sm);padding:3px;overflow:hidden}.op-record-line{flex:0 0 auto;height:23px;display:flex;align-items:center;padding:0 8px;font-size:11px;line-height:1.4;color:var(--text-1);cursor:pointer;border-radius:4px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.op-record-line:hover{background:var(--bg-3);color:var(--accent)}.op-record-empty{height:100%;display:flex;align-items:center;justify-content:center;font-size:11px;color:var(--text-2);text-align:center;padding:0 8px}.script-editor{flex:1;min-height:160px;resize:none;background:var(--bg-0);border:1px solid var(--border);border-radius:var(--radius-sm);color:#c9d4e8;font-size:12px;line-height:1.65;padding:12px;font-family:var(--mono);outline:none}.mono{font-family:var(--mono);font-size:11px;color:var(--text-1)}
</style>
