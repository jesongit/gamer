<template>
  <div class="plugin-workspace">
    <!-- 主导航（plan §29）：任务 | 日志 | 市场▾ | 插件▾ | 设置。市场/插件为下拉
         二级菜单：市场分插件市场/配置市场两分区，插件下拉列出已启用插件（显示名）；
         插件 Panel 不再占据主导航（§40）。 -->
    <WorkspaceTabs
      :panels="topTabs"
      :active-panel="activeTab"
      :active-child="activeChild"
      @select="selectTop"
      @open-plugin-center="centerOpen = true"
    />
    <!-- 插件功能子页签：选中插件后以其贡献的 Panel 平铺一排页签（样式对齐主导航）；
         单面板插件直接渲染面板本体，不显示该行 -->
    <div v-if="activePluginGroup && activePluginGroup.panels.length > 1" class="plugin-subnav-tabs">
      <button
        v-for="panel in activePluginGroup.panels"
        :key="panel.key"
        type="button"
        class="workspace-subtab"
        :class="{ active: panel.key === activePanel }"
        @click="selectPanel(panel.key)"
      >{{ panel.title }}</button>
    </div>

    <div class="workspace-panel-slot">
      <!-- 市场（§30）：下拉二级菜单选分区（插件市场/配置市场），一次只渲染所选分区 -->
      <MarketView v-if="activeTop === 'market'" :section="marketSection" @extensions-changed="emit('extensions-changed')" />
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
          @click="selectPlugin(group.pluginId)"
        >
          <span class="plugin-card-title">{{ group.pluginName }}</span>
          <span class="plugin-card-panels">{{ group.pluginId }} · {{ group.panels.map(p => p.title).join(' · ') }}</span>
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
  // {pluginId → 显示名}（扩展快照 name），插件下拉/插件列表用插件名而非 id
  pluginNames: { type: Object, default: () => ({}) },
  context: { type: Object, default: () => ({}) },
  lifecycle: { type: Object, default: null },
})
const emit = defineEmits(['select', 'fallback', 'extensions-changed'])

const lifecycle = props.lifecycle || createWorkspaceLifecycle()
const allPanels = computed(() => props.registry.getPanels())

// 主导航五项（§29）：core 三面板 + 市场/插件两个虚拟视图。
// 市场收成下拉二级菜单：插件市场/配置市场两分区；插件下拉列出已启用插件本体
// （显示名，hint 标其功能清单），选中插件后由主导航下一行的功能子页签承接
// （单面板插件无子页签行，面板直接渲染）。子项 key：市场分区用 `market:<section>`、
// 插件用 `plugin:<pluginId>`（均为 selectTop 拦截翻译的 UI key，不进路由）。
const MARKET_CHILDREN = [
  { key: 'market:plugin', title: '插件市场' },
  { key: 'market:package', title: '配置市场' },
]
const topTabs = computed(() => {
  const tabs = []
  for (const key of ['gamer.core:tasks', 'gamer.core:logs']) {
    const panel = allPanels.value.find(p => p.key === key)
    if (panel) tabs.push(panel)
  }
  tabs.push({ key: 'market', title: '市场', icon: '🛒', children: MARKET_CHILDREN })
  const pluginChildren = pluginGroups.value.map(group => ({
    key: `plugin:${group.pluginId}`,
    title: group.pluginName,
    hint: group.panels.map(panel => panel.title).join(' · '),
  }))
  tabs.push(pluginChildren.length
    ? { key: 'plugins', title: '插件', icon: '🧩', children: pluginChildren }
    : { key: 'plugins', title: '插件', icon: '🧩' })
  const settings = allPanels.value.find(p => p.key === 'gamer.core:settings')
  if (settings) tabs.push(settings)
  return tabs
})

/** 市场当前分区（下拉选择决定；panel=market 的 URL 不带分区，默认插件市场）。 */
const marketSection = ref('plugin')

/** 业务插件面板按 pluginId 分组（gamer.core 除外），附插件显示名。 */
const pluginGroups = computed(() => {
  void props.pluginNames
  const groups = new Map()
  for (const panel of allPanels.value) {
    if (panel.pluginId === 'gamer.core') continue
    if (!groups.has(panel.pluginId)) groups.set(panel.pluginId, [])
    groups.get(panel.pluginId).push(panel)
  }
  return [...groups.entries()]
    .map(([pluginId, panels]) => ({
      pluginId,
      pluginName: String(props.pluginNames?.[pluginId] || '').trim() || pluginId,
      panels,
    }))
    .sort((a, b) => a.pluginId.localeCompare(b.pluginId))
})

const activeTop = computed(() => {
  if (props.activePanel === 'market' || props.activePanel === 'plugins') return props.activePanel
  // 其余 key（gamer.core:* 与业务面板 gamer.<plugin>:<panel>）原样透传：
  // selected 按它解析面板组件，页签高亮经 activeTab 映射回所在主导航页签。
  return props.activePanel
})

/** WorkspaceTabs 的高亮 key：业务面板挂靠「插件」页签，其余原样。 */
const activeTab = computed(() => {
  if (activeTop.value === 'market' || activeTop.value === 'plugins') return activeTop.value
  if (props.activePanel.startsWith('gamer.core:')) return activeTop.value
  return 'plugins'
})

/** 当前处于某插件二级导航时的分组（业务面板 key 命中）。 */
const activePluginGroup = computed(() => {
  if (activeTop.value === 'market' || activeTop.value === 'plugins') return null
  const key = props.activePanel
  if (!key || key.startsWith('gamer.core:')) return null
  return pluginGroups.value.find(g => g.panels.some(p => p.key === key)) || null
})

const selected = computed(() => {
  if (activeTop.value === 'market' || activeTop.value === 'plugins') return null
  return props.registry.resolve(props.activePanel) || props.registry.defaultPanel()
})

const coreContext = computed(() => props.context.core || {})
const uiBridge = computed(() => props.context.uiBridge || props.context.bridge)
const centerOpen = ref(false)

function selectTop(key) {
  // 市场下拉子项（market:<section>）翻译成 market 视图 + 分区状态，不进路由
  if (key === 'market:plugin' || key === 'market:package') {
    marketSection.value = key.slice('market:'.length)
    emit('select', 'market')
    return
  }
  // 插件下拉子项（plugin:<pluginId>）翻译成该插件的目标面板（真实 panel key 进路由）
  if (key.startsWith('plugin:')) {
    selectPlugin(key.slice('plugin:'.length))
    return
  }
  emit('select', key)
}
function selectPanel(key) { emit('select', key) }

/** 每插件最后访问的面板：再次从下拉选中该插件时回到它，而不是永远跳第一个。 */
const lastPanelByPlugin = new Map()
watch(() => props.activePanel, key => {
  const group = pluginGroups.value.find(g => g.panels.some(p => p.key === key))
  if (group) lastPanelByPlugin.set(group.pluginId, key)
}, { immediate: true })

/** 选中插件：保持当前面板 → 上次访问的面板 → 第一个面板。 */
function selectPlugin(pluginId) {
  const group = pluginGroups.value.find(g => g.pluginId === pluginId)
  if (!group) return
  const target = group.panels.find(p => p.key === props.activePanel)
    || group.panels.find(p => p.key === lastPanelByPlugin.get(pluginId))
    || group.panels[0]
  if (target) emit('select', target.key)
}

/** 下拉菜单项高亮：市场跟随所选分区，插件跟随当前业务面板所属插件。 */
const activeChild = computed(() => {
  if (activeTop.value === 'market') return `market:${marketSection.value}`
  if (activePluginGroup.value) return `plugin:${activePluginGroup.value.pluginId}`
  return props.activePanel
})

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
/* 插件功能子页签条：样式对齐主导航（WorkspaceTabs）的整条页签框 */
.plugin-subnav-tabs { display:flex; flex-shrink:0; border:1px solid var(--border); border-radius:var(--radius-sm); background:var(--bg-2); overflow-x:auto; }
.workspace-subtab { flex:1; min-width:0; padding:7px 3px; border:0; border-left:1px solid var(--border); background:transparent; color:var(--text-1); font-size:12px; cursor:pointer; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
.workspace-subtab:first-child { border-left:0; }
.workspace-subtab:hover { color:var(--text-0); background:var(--bg-3); }
.workspace-subtab.active { color:var(--accent); background:rgba(34,211,165,.14); font-weight:600; }
.workspace-panel-slot { flex:1; min-height:0; display:flex; flex-direction:column; overflow:hidden; }
.workspace-empty { flex:1; display:flex; align-items:center; justify-content:center; color:var(--text-2); border:1px solid var(--border); border-radius:var(--radius); padding:0 16px; text-align:center; }
.plugin-picker { flex:1; min-height:0; overflow-y:auto; display:flex; flex-direction:column; gap:8px; padding:4px; }
.plugin-card { display:flex; flex-direction:column; align-items:flex-start; gap:4px; text-align:left; border:1px solid var(--border); background:var(--bg-2); border-radius:var(--radius); padding:12px; cursor:pointer; }
.plugin-card:hover { border-color:var(--accent); }
.plugin-card-title { font-size:13px; font-weight:600; color:var(--text-0); }
.plugin-card-panels { font-size:12px; color:var(--text-2); }
</style>
