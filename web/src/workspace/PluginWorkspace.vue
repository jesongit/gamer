<template>
  <div class="plugin-workspace">
    <!-- 主导航（plan §29）：任务 | 日志 | 市场 | 插件 | 设置。插件贡献的 Panel
         不再直接占据主导航（§40），收在「插件」二级（§31-§32）。 -->
    <WorkspaceTabs
      :panels="topTabs"
      :active-panel="activeTop"
      @select="selectTop"
      @open-plugin-center="centerOpen = true"
    />
    <!-- 插件二级导航：选中插件后展示其贡献的 Panel（§32） -->
    <div v-if="activePluginGroup" class="plugin-subnav">
      <button type="button" class="plugin-back" title="返回插件列表" @click="selectTop('plugins')">← 插件</button>
      <span class="plugin-subnav-title">{{ activePluginGroup.pluginId }}</span>
      <div class="plugin-subnav-tabs">
        <button
          v-for="panel in activePluginGroup.panels"
          :key="panel.key"
          type="button"
          class="workspace-subtab"
          :class="{ active: panel.key === activePanel }"
          @click="selectPanel(panel.key)"
        >{{ panel.title }}</button>
      </div>
    </div>

    <div class="workspace-panel-slot">
      <!-- 市场（§30）：插件市场 + Package 市场 -->
      <MarketView v-if="activeTop === 'market'" @extensions-changed="emit('extensions-changed')" />
      <!-- 插件列表（§31）：当前已注册贡献的插件 -->
      <div v-else-if="activeTop === 'plugins'" class="plugin-picker">
        <div v-if="!pluginGroups.length" class="workspace-empty">
          当前没有已启用插件贡献的面板。可在「市场」安装插件。
        </div>
        <button
          v-for="group in pluginGroups"
          :key="group.pluginId"
          type="button"
          class="plugin-card"
          @click="selectPanel(group.panels[0].key)"
        >
          <span class="plugin-card-title">{{ group.pluginId }}</span>
          <span class="plugin-card-panels">{{ group.panels.map(p => p.title).join(' · ') }}</span>
        </button>
      </div>
      <!-- 面板渲染（Core 自有 + 扩展贡献同机制） -->
      <template v-else-if="selected">
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
import MarketView from './MarketView.vue'
import { createWorkspaceLifecycle } from './lifecycle'

const props = defineProps({
  registry: { type: Object, required: true },
  activePanel: { type: String, default: '' },
  context: { type: Object, default: () => ({}) },
  lifecycle: { type: Object, default: null },
})
const emit = defineEmits(['select', 'fallback', 'extensions-changed'])

const lifecycle = props.lifecycle || createWorkspaceLifecycle()
const allPanels = computed(() => props.registry.getPanels())

// 主导航五项（§29）：core 三面板 + 市场/插件两个虚拟视图
const topTabs = computed(() => {
  const tabs = []
  for (const key of ['gamer.core:tasks', 'gamer.core:logs']) {
    const panel = allPanels.value.find(p => p.key === key)
    if (panel) tabs.push(panel)
  }
  tabs.push({ key: 'market', title: '市场', icon: '🛒' })
  tabs.push({ key: 'plugins', title: '插件', icon: '🧩' })
  const settings = allPanels.value.find(p => p.key === 'gamer.core:settings')
  if (settings) tabs.push(settings)
  return tabs
})

/** 业务插件面板按 pluginId 分组（gamer.core 除外）。 */
const pluginGroups = computed(() => {
  const groups = new Map()
  for (const panel of allPanels.value) {
    if (panel.pluginId === 'gamer.core') continue
    if (!groups.has(panel.pluginId)) groups.set(panel.pluginId, [])
    groups.get(panel.pluginId).push(panel)
  }
  return [...groups.entries()]
    .map(([pluginId, panels]) => ({ pluginId, panels }))
    .sort((a, b) => a.pluginId.localeCompare(b.pluginId))
})

const activeTop = computed(() => {
  if (props.activePanel === 'market' || props.activePanel === 'plugins') return props.activePanel
  if (props.activePanel.startsWith('gamer.core:')) return props.activePanel
  return 'plugins'
})

/** 当前处于某插件二级导航时的分组（业务面板 key 命中）。 */
const activePluginGroup = computed(() => {
  if (activeTop.value !== 'plugins') return null
  const key = props.activePanel
  if (!key || key === 'plugins') return null
  return pluginGroups.value.find(g => g.panels.some(p => p.key === key)) || null
})

const selected = computed(() => {
  if (activeTop.value === 'market' || activeTop.value === 'plugins') return null
  return props.registry.resolve(props.activePanel) || props.registry.defaultPanel()
})

const coreContext = computed(() => props.context.core || {})
const uiBridge = computed(() => props.context.uiBridge || props.context.bridge)
const centerOpen = ref(false)

function selectTop(key) { emit('select', key) }
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
.plugin-subnav { display:flex; align-items:center; gap:8px; flex-shrink:0; }
.plugin-back { border:1px solid var(--border); background:var(--bg-2); color:var(--text-1); border-radius:var(--radius-sm); padding:4px 10px; font-size:12px; cursor:pointer; }
.plugin-back:hover { color:var(--text-0); background:var(--bg-3); }
.plugin-subnav-title { font-size:12px; font-weight:600; color:var(--text-2); }
.plugin-subnav-tabs { display:flex; gap:4px; overflow-x:auto; }
.workspace-subtab { border:1px solid var(--border); background:transparent; color:var(--text-1); border-radius:var(--radius-sm); padding:4px 12px; font-size:12px; cursor:pointer; white-space:nowrap; }
.workspace-subtab.active { color:var(--accent); background:rgba(34,211,165,.14); font-weight:600; border-color:rgba(34,211,165,.4); }
.workspace-panel-slot { flex:1; min-height:0; display:flex; flex-direction:column; overflow:hidden; }
.workspace-empty { flex:1; display:flex; align-items:center; justify-content:center; color:var(--text-2); border:1px solid var(--border); border-radius:var(--radius); padding:0 16px; text-align:center; }
.plugin-picker { flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column; gap:8px; padding:4px; }
.plugin-card { display:flex; flex-direction:column; align-items:flex-start; gap:4px; text-align:left; border:1px solid var(--border); background:var(--bg-2); border-radius:var(--radius); padding:12px; cursor:pointer; }
.plugin-card:hover { border-color:var(--accent); }
.plugin-card-title { font-size:13px; font-weight:600; color:var(--text-0); }
.plugin-card-panels { font-size:12px; color:var(--text-2); }
</style>
