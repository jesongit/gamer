<template>
  <div class="plugin-workspace">
    <WorkspaceTabs
      :panels="panels"
      :active-panel="selected?.key || ''"
      @select="selectPanel"
      @open-plugin-center="centerOpen = true"
    />
    <div class="workspace-panel-slot">
      <template v-if="selected">
        <KeepAlive v-if="selected.keepAlive === 'session'">
          <CorePanelHost
            v-if="selected.runtime === 'core'"
            :key="selected.key"
            :contribution="selected"
            :context="coreContext"
          />
          <PluginPanelHost
            v-else
            :key="selected.key"
            :contribution="selected"
            :bridge="uiBridge"
          />
        </KeepAlive>
        <CorePanelHost
          v-else-if="selected.runtime === 'core'"
          :key="selected.key"
          :contribution="selected"
          :context="coreContext"
        />
        <PluginPanelHost
          v-else
          :key="selected.key"
          :contribution="selected"
          :bridge="uiBridge"
        />
      </template>
      <div v-else class="workspace-empty">没有可用面板</div>
    </div>
    <PluginCenter :open="centerOpen" @close="centerOpen = false" @changed="emit('extensions-changed')" />
  </div>
</template>

<script setup>
import { computed, onUnmounted, ref, watch } from 'vue'
import WorkspaceTabs from './WorkspaceTabs.vue'
import CorePanelHost from './CorePanelHost.vue'
import PluginPanelHost from './PluginPanelHost.vue'
import PluginCenter from './plugin-center/PluginCenter.vue'
import { createWorkspaceLifecycle } from './lifecycle'

const props = defineProps({
  registry: { type: Object, required: true },
  activePanel: { type: String, default: '' },
  context: { type: Object, default: () => ({}) },
  lifecycle: { type: Object, default: null },
})
const emit = defineEmits(['select', 'fallback', 'extensions-changed'])

const lifecycle = props.lifecycle || createWorkspaceLifecycle()
const panels = computed(() => props.registry.getPanels())
const selected = computed(() => props.registry.resolve(props.activePanel) || props.registry.defaultPanel())
const coreContext = computed(() => props.context.core || {})
const uiBridge = computed(() => props.context.uiBridge || props.context.bridge)
const centerOpen = ref(false)

function selectPanel(key) { emit('select', key) }

watch(selected, (panel, previous) => {
  if (previous?.key) lifecycle.ui.close(previous.key)
  if (panel?.key) {
    lifecycle.ui.open(panel.key)
    if (panel.key !== props.activePanel) emit('fallback', panel.key)
  }
}, { immediate: true })

onUnmounted(() => {
  if (selected.value?.key) lifecycle.ui.close(selected.value.key)
})
</script>

<style scoped>
.plugin-workspace { flex:1; min-height:0; display:flex; flex-direction:column; gap:10px; overflow:hidden; }
.workspace-panel-slot { flex:1; min-height:0; display:flex; flex-direction:column; overflow:hidden; }
.workspace-empty { flex:1; display:flex; align-items:center; justify-content:center; color:var(--text-2); border:1px solid var(--border); border-radius:var(--radius); }
</style>
