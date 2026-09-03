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
    <div v-else class="declarative-panel-placeholder">
      <div class="workspace-empty-title">{{ contribution.title }}</div>
      <div>Declarative 插件面板 Host 预留中。</div>
    </div>
    <div v-if="bridgeError" class="plugin-bridge-error" role="alert">{{ bridgeError }}</div>
  </div>
</template>

<script setup>
import { computed, onUnmounted, ref, watch } from 'vue'
import iframePocUrl from './iframe-poc.html?url'
import {
  BRIDGE_CONNECT_TYPE,
  UI_BRIDGE_VERSION,
  isBridgeRequest,
  replyToBridgeRequest,
} from './bridge'

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
onUnmounted(disconnect)
</script>

<style scoped>
.plugin-panel-host { flex:1; min-height:0; overflow:hidden; display:flex; flex-direction:column; position:relative; }
.plugin-panel-frame { width:100%; height:100%; flex:1; min-height:0; border:0; background:var(--bg-1); }
.declarative-panel-placeholder { flex:1; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:8px; color:var(--text-2); font-size:12px; }
.workspace-empty-title { color:var(--text-0); font-weight:600; }
.plugin-bridge-error { position:absolute; left:10px; right:10px; bottom:10px; padding:6px 8px; border:1px solid var(--danger); border-radius:var(--radius-sm); background:rgba(8,10,16,.92); color:var(--danger); font-size:11px; }
</style>
