<template>
  <div class="plugin-panel-host panel-sec">
    <iframe
      v-if="contribution.runtime === 'iframe' && !remoteUiBlocked"
      ref="iframeEl"
      class="plugin-panel-frame"
      :src="iframeSrc"
      sandbox="allow-scripts"
      referrerpolicy="no-referrer"
      :title="contribution.iframe?.title || contribution.title"
      @load="connect"
    ></iframe>
    <div v-else-if="contribution.runtime === 'iframe' && remoteUiBlocked" class="declarative-panel-placeholder plugin-remote-blocked" role="alert">
      <div class="workspace-empty-title">已阻止远程插件界面</div>
      <div>插件界面必须来自已安装的本地归档。</div>
    </div>
    <!-- declarative 面板：宿主按 manifest schema 原生渲染表单（不引入 iframe）。
         控件值收集后，按钮点击经 UI Bridge plugin.call 发给插件后端（meta 携带 pluginId/panelId）。 -->
    <form
      v-else-if="fields.length"
      class="declarative-form"
      @submit.prevent
    >
      <p v-if="schemaDescription" class="declarative-description">{{ schemaDescription }}</p>
      <label v-for="field in fields" :key="fieldKey(field)" class="declarative-field" :class="`is-${field.type}`">
        <template v-if="field.type === 'button'">
          <button
            type="button"
            class="btn btn-sm declarative-button"
            :disabled="callingAction"
            @click="runAction(field)"
          >{{ field.label }}</button>
          <span v-if="field.description" class="declarative-field-desc">{{ field.description }}</span>
        </template>
        <template v-else>
          <span class="declarative-label" :title="field.description || ''">{{ field.label }}</span>
          <input
            v-if="field.type === 'text'"
            v-model="values[fieldKey(field)]"
            type="text"
            class="input"
            :placeholder="field.placeholder || ''"
          />
          <input
            v-else-if="field.type === 'number'"
            v-model.number="values[fieldKey(field)]"
            type="number"
            class="input"
            :placeholder="field.placeholder || ''"
          />
          <input
            v-else-if="field.type === 'boolean'"
            v-model="values[fieldKey(field)]"
            type="checkbox"
            class="declarative-checkbox"
          />
          <select v-else-if="field.type === 'select'" v-model="values[fieldKey(field)]" class="select">
            <option v-for="option in field.options || []" :key="String(option.value)" :value="option.value">
              {{ option.label || option.value }}
            </option>
          </select>
          <span v-if="field.description" class="declarative-field-desc">{{ field.description }}</span>
        </template>
      </label>
    </form>
    <div v-else class="declarative-panel-placeholder">
      <div class="workspace-empty-title">{{ contribution.title }}</div>
      <div>该 declarative 面板未声明表单 schema。</div>
    </div>
    <div v-if="bridgeError" class="plugin-bridge-error" role="alert">{{ bridgeError }}</div>
    <div v-else-if="actionResult !== null" class="plugin-action-result">{{ actionResultText }}</div>
  </div>
</template>

<script setup>
import { computed, reactive, ref, watch } from 'vue'
import iframePocUrl from './iframe-poc.html?url'
import {
  BRIDGE_CONNECT_TYPE,
  UI_BRIDGE_VERSION,
  isBridgeRequest,
  replyToBridgeRequest,
} from './bridge'
import { api } from '../api'

const props = defineProps({
  contribution: { type: Object, required: true },
  bridge: { type: Object, required: true },
})

const iframeEl = ref(null)
const bridgeError = ref('')
let channel = null
let port = null

const iframeSrc = computed(() => props.contribution.iframe?.src || iframePocUrl)
const remoteUiBlocked = computed(() => /^https?:\/\//i.test(String(props.contribution.iframe?.src || '')))

// ---------- declarative 表单 Host ----------

const schema = computed(() => (props.contribution.runtime === 'declarative' ? props.contribution.schema : null))
const fields = computed(() => (Array.isArray(schema.value?.fields) ? schema.value.fields : []))
const schemaDescription = computed(() => String(schema.value?.description || ''))
const values = reactive({})
const callingAction = ref(false)
const actionResult = ref(null)

/** 控件键：有 name 用 name；button 无值键，用 action 兜底保证 v-for key 稳定。 */
function fieldKey(field) {
  return field.name || field.action || field.label
}

function resetValues() {
  for (const key of Object.keys(values)) delete values[key]
  for (const field of fields.value) {
    if (field.type === 'button' || !field.name) continue
    if (field.default !== undefined && field.default !== null) {
      values[field.name] = field.default
    } else if (field.type === 'boolean') {
      values[field.name] = false
    } else {
      values[field.name] = ''
    }
  }
}

/** 按钮动作：收集当前控件值，直接调服务端 plugin.call 端点（POST /api/extensions/:id/call）。
 *  服务端校验插件必须 Running 且 action 在 manifest declarative schema 按钮集合内；
 *  guest 返回的 JSON 结果展示在面板内，失败转为面板内错误提示。 */
async function runAction(field) {
  if (!field.action || callingAction.value) return
  callingAction.value = true
  bridgeError.value = ''
  actionResult.value = null
  try {
    const result = await api.callExtension(props.contribution.pluginId, field.action, { ...values })
    if (result !== null && result !== undefined) actionResult.value = result
  } catch (error) {
    bridgeError.value = error?.message || String(error)
  } finally {
    callingAction.value = false
  }
}

watch(() => props.contribution?.key, resetValues, { immediate: true })

/** guest 返回的 JSON 结果摘要；对象/数组折叠为紧凑 JSON，其余原样展示。 */
const actionResultText = computed(() => {
  if (actionResult.value === null || actionResult.value === undefined) return ''
  if (typeof actionResult.value === 'string') return actionResult.value
  try { return JSON.stringify(actionResult.value) } catch { return String(actionResult.value) }
})

function disconnect() {
  if (port) {
    port.onmessage = null
    port.close?.()
  }
  channel = null
  port = null
}

function connect() {
  disconnect()
  bridgeError.value = ''
  if (props.contribution.runtime !== 'iframe') return
  if (typeof MessageChannel === 'undefined') {
    bridgeError.value = '当前浏览器不支持 MessageChannel'
    return
  }
  channel = new MessageChannel()
  port = channel.port1
  port.onmessage = event => {
    const request = event?.data
    if (!isBridgeRequest(request)) return
    void replyToBridgeRequest(request, props.bridge, port, {
      pluginId: props.contribution.pluginId,
      panelId: props.contribution.panelId,
    })
  }
  port.start?.()
  const target = iframeEl.value?.contentWindow
  if (!target) {
    bridgeError.value = '插件 iframe 尚未就绪'
    return
  }
  // sandbox without allow-same-origin yields an opaque origin; bootstrap uses
  // '*' once, while subsequent RPC travels over the transferred MessagePort.
  target.postMessage({
    type: BRIDGE_CONNECT_TYPE,
    version: UI_BRIDGE_VERSION,
    panel: { pluginId: props.contribution.pluginId, panelId: props.contribution.panelId },
  }, '*', [channel.port2])
}

watch(() => props.contribution?.key, () => { if (iframeEl.value) connect() })
</script>

<style scoped>
.plugin-panel-host { flex:1; min-height:0; overflow:hidden; display:flex; flex-direction:column; position:relative; }
.plugin-panel-frame { width:100%; height:100%; flex:1; min-height:0; border:0; background:var(--bg-1); }
.declarative-panel-placeholder { flex:1; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:8px; color:var(--text-2); font-size:12px; }
.workspace-empty-title { color:var(--text-0); font-weight:600; }
.plugin-bridge-error { position:absolute; left:10px; right:10px; bottom:10px; padding:6px 8px; border:1px solid var(--danger); border-radius:var(--radius-sm); background:rgba(8,10,16,.92); color:var(--danger); font-size:11px; }
.plugin-action-result { position:absolute; left:10px; right:10px; bottom:10px; padding:6px 8px; border:1px solid var(--border); border-radius:var(--radius-sm); background:rgba(8,10,16,.92); color:var(--text-1); font-size:11px; word-break:break-all; max-height:30%; overflow:auto; }

/* declarative 表单：紧凑单列布局，适配右侧面板宽度 */
.declarative-form { flex:1; min-height:0; overflow:auto; display:flex; flex-direction:column; gap:10px; padding-right:2px; }
.declarative-description { margin:0; color:var(--text-2); font-size:11px; line-height:1.5; }
.declarative-field { display:flex; flex-direction:column; gap:4px; font-size:12px; min-width:0; }
.declarative-field.is-button { flex-direction:row; align-items:center; gap:8px; flex-wrap:wrap; }
.declarative-field.is-boolean { flex-direction:row; align-items:center; gap:8px; }
.declarative-label { color:var(--text-1); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.declarative-field .input,
.declarative-field .select { width:100%; min-width:0; box-sizing:border-box; }
.declarative-checkbox { width:15px; height:15px; margin:0; }
.declarative-field-desc { color:var(--text-2); font-size:10px; line-height:1.4; }
.declarative-button { flex:0 0 auto; }
</style>
