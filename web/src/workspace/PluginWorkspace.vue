<template>
  <div class="plugin-workspace">
    <!-- 全局主导航；工作台内平铺实际启用插件贡献的功能。 -->
    <Teleport :to="navigationTarget || 'body'" :disabled="!navigationTarget">
    <WorkspaceTabs
      :panels="topTabs"
      :active-panel="activeTab"
      @select="selectTop"
    />
    </Teleport>
    <!-- 功能项来自注册表，保持应用兼容过滤。 -->
    <div v-if="isWorkbench" class="plugin-subnav-tabs" aria-label="工作台功能，可拖动排序">
      <button
        v-for="panel in workbenchPanels"
        :key="panel.key"
        type="button"
        class="workspace-subtab"
        :class="{ active: panel.key === activePanel, 'tab-dragging': draggedTab === panel.key, 'tab-drop-before': dropTab === panel.key && !dropAfter, 'tab-drop-after': dropTab === panel.key && dropAfter }"
        draggable="true"
        :title="`${panel.title} · 拖动排序，或 Alt+Shift+方向键调整位置`"
        @dragstart="startTabDrag($event, panel.key)"
        @dragover.prevent="overTab($event, panel.key)"
        @drop.prevent.stop="dropOnTab($event, panel.key)"
        @dragend="endTabDrag"
        @keydown="reorderTabKey($event, panel.key)"
        @click="selectPanel(panel.key)"
      >{{ panel.title }}</button>
      <button v-if="tabOrder.customized.value" class="reset-tab-order btn btn-icon btn-ghost" title="恢复默认页签顺序" aria-label="恢复默认页签顺序" @click="tabOrder.reset()"><UiIcon name="refresh" /></button>
    </div>
    <span class="tab-order-announcement" role="status">{{ orderAnnouncement }}</span>

    <div class="workspace-panel-slot">
      <div v-if="activeTop === 'packages'" class="package-page">
        <section class="package-installed" aria-label="已安装配置包">
          <PackageContextBar :context="packageContext" :dialogs="false" management />
          <div class="package-section-head"><h3>本地配置</h3><span>{{ (packageView.pkgOptions || []).length }} 个</span><span class="package-section-note">选中后作为工作台的数据上下文</span></div>
          <div class="package-list"><button v-for="id in packageView.pkgOptions || []" :key="id" class="package-card" :class="{ active: id === packageView.currentId }" :aria-pressed="id === packageView.currentId" @click="packageView.onPackageChange({ target: { value: id } })">
            <span class="package-identity"><strong>{{ packageInfo(id).name || id }}</strong><span class="mono">{{ id }}</span></span>
            <span class="package-version"><small>版本</small><span class="mono">{{ packageInfo(id).version || '—' }}</span></span>
            <span class="package-author"><small>作者</small><span>{{ packageInfo(id).author || '未填写' }}</span></span>
            <span class="package-selection"><span v-if="id === packageView.currentId" class="selected-dot"></span>{{ id === packageView.currentId ? '当前使用' : '切换使用' }}</span>
          </button></div>
          <p v-if="!packageView.pkgOptions?.length" class="package-note">尚未安装配置包，可从下方市场安装，或通过上方操作导入、新建。</p>
        </section>
        <MarketView class="package-market" />
      </div>
      <div v-else-if="activeTop === 'workbench'" class="workspace-empty">启用插件后，其功能面板会显示在工作台。<button class="btn" @click="selectTop('plugins')">管理插件</button></div>
      <PluginCenter v-else-if="activeTop === 'plugins'" @changed="emit('extensions-changed')" />
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
  </div>
</template>

<script setup>
import { computed, defineAsyncComponent, nextTick, onUnmounted, reactive, ref, watch } from 'vue'
import UiIcon from '../components/ui/UiIcon.vue'
import { useWorkbenchTabOrder } from './useWorkbenchTabOrder'
import PackageContextBar from './PackageContextBar.vue'
import WorkspaceTabs from './WorkspaceTabs.vue'
import CorePanelHost from './CorePanelHost.vue'
import PluginPanelHost from './PluginPanelHost.vue'
const PluginCenter = defineAsyncComponent(() => import('./plugin-center/PluginCenter.vue'))
const MarketView = defineAsyncComponent(() => import('./MarketView.vue'))
import { createWorkspaceLifecycle } from './lifecycle'

const props = defineProps({
  navigationTarget: { type: String, default: '' },
  packageContext: { type: Object, default: null },
  registry: { type: Object, required: true },
  activePanel: { type: String, default: '' },
  // {pluginId → 显示名}（扩展快照 name），插件下拉/插件列表用插件名而非 id
  pluginNames: { type: Object, default: () => ({}) },
  // {pluginId → Android Targets 数组}（扩展快照 targets.android.packages）
  pluginTargets: { type: Object, default: () => ({}) },
  // 当前设备的 Android 应用包名（运行目标）；空 = 未选择应用
  androidPackageName: { type: String, default: '' },
  context: { type: Object, default: () => ({}) },
  lifecycle: { type: Object, default: null },
})
const emit = defineEmits(['select', 'fallback', 'extensions-changed'])

const packageView = computed(() => reactive(props.packageContext || {}))
function packageInfo(id) { return packageView.value.packages?.find(item => item.id === id) || {} }
const lifecycle = props.lifecycle || createWorkspaceLifecycle()
const allPanels = computed(() => props.registry.getPanels())

// 六个主页面；已安装资源与市场同页展示。
const topTabs = computed(() => {
  const tabs = [{ key: 'workbench', title: '工作台' }]
  for (const key of ['gamer.core:tasks', 'gamer.core:logs']) {
    const panel = allPanels.value.find(p => p.key === key)
    if (panel) tabs.push({ ...panel, icon: '' })
  }
  tabs.push({ key: 'packages', title: '配置包' })
  tabs.push({ key: 'plugins', title: '插件' })
  const settings = allPanels.value.find(p => p.key === 'gamer.core:settings')
  if (settings) tabs.push({ ...settings, icon: '' })
  return tabs
})

/**
 * 插件 Android Targets 匹配（manifest `[targets.android].packages`）：
 * 未声明/空 = 通用（`*` 语义）恒显示；声明含 `*` 恒显示；其余需当前设备
 * 应用包名精确命中——不命中（含未选择应用）则插件功能不出现在工作台。
 */
function pluginSupportsApp(pluginId) {
  const list = props.pluginTargets?.[pluginId]
  if (!Array.isArray(list) || !list.length || list.includes('*')) return true
  const app = String(props.androidPackageName || '').trim()
  return app !== '' && list.includes(app)
}

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
    .filter(([pluginId]) => pluginSupportsApp(pluginId))
    .map(([pluginId, panels]) => ({
      pluginId,
      pluginName: String(props.pluginNames?.[pluginId] || '').trim() || pluginId,
      panels,
    }))
    .sort((a, b) => a.pluginId.localeCompare(b.pluginId))
})

const tabOrder = useWorkbenchTabOrder(computed(() => pluginGroups.value.flatMap(group => group.panels)))
const workbenchPanels = tabOrder.sortedPanels
const draggedTab = ref(''), dropTab = ref(''), dropAfter = ref(false), orderAnnouncement = ref('')
let suppressTabClickUntil = 0
function startTabDrag(event, key) {
  draggedTab.value = key
  if (event.dataTransfer) { event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', key) }
}
function overTab(event, key) {
  if (!draggedTab.value) return
  const rect = event.currentTarget.getBoundingClientRect()
  dropTab.value = key
  dropAfter.value = event.clientX > rect.left + rect.width / 2
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'
}
function endTabDrag() {
  draggedTab.value = ''; dropTab.value = ''; suppressTabClickUntil = Date.now() + 250
}
function announceTab(key) {
  const index = workbenchPanels.value.findIndex(panel => panel.key === key)
  orderAnnouncement.value = `${workbenchPanels.value[index]?.title || ''} 已移至第 ${index + 1} 位`
}
function dropOnTab(event, key) {
  if (!draggedTab.value) return
  overTab(event, key)
  const source = draggedTab.value
  if (tabOrder.move(source, key, dropAfter.value)) announceTab(source)
  endTabDrag()
}
async function reorderTabKey(event, key) {
  if (!event.altKey || !event.shiftKey || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return
  event.preventDefault()
  const direction = event.key === 'ArrowRight' ? 1 : -1
  const index = workbenchPanels.value.findIndex(panel => panel.key === key)
  const target = workbenchPanels.value[index + direction]
  if (!target) return
  const element = event.currentTarget
  if (tabOrder.move(key, target.key, direction > 0)) { announceTab(key); await nextTick(); element.focus() }
}
const isWorkbench = computed(() => props.activePanel === 'workbench' || !!workbenchPanels.value.find(p => p.key === props.activePanel))
let lastWorkbench = ''
watch(() => props.activePanel, key => { if (workbenchPanels.value.some(p => p.key === key)) lastWorkbench = key })

const activeTop = computed(() => {
  if (['plugins', 'packages', 'workbench'].includes(props.activePanel)) return props.activePanel
  // 其余 key（gamer.core:* 与业务面板 gamer.<plugin>:<panel>）原样透传：
  // selected 按它解析面板组件，页签高亮经 activeTab 映射回所在主导航页签。
  return props.activePanel
})

/** WorkspaceTabs 的高亮 key：业务面板挂靠「工作台」页签，其余原样。 */
const activeTab = computed(() => {
  if (['plugins', 'packages', 'workbench'].includes(activeTop.value)) return activeTop.value
  if (props.activePanel.startsWith('gamer.core:')) return activeTop.value
  return 'workbench'
})

const selected = computed(() => {
  // resolve 使用普通 Map；订阅贡献列表，确保冷启动/启停时重新解析同一个路由。
  void allPanels.value
  if (['plugins', 'packages', 'workbench'].includes(activeTop.value)) return null
  return props.registry.resolve(props.activePanel) || (!props.activePanel ? props.registry.defaultPanel() : null)
})

const coreContext = computed(() => props.context.core || {})
const uiBridge = computed(() => props.context.uiBridge || props.context.bridge)
function selectTop(key) {
  if (key === 'workbench') {
    emit('select', workbenchPanels.value.find(p => p.key === lastWorkbench)?.key || workbenchPanels.value[0]?.key || 'workbench')
    return
  }
  emit('select', key)
}
function selectPanel(key) { if (Date.now() >= suppressTabClickUntil) emit('select', key) }

// 首次进入工作台时，扩展贡献到达后打开第一个可用功能；无插件保持工作台空态。
watch([() => props.activePanel, workbenchPanels], ([key, panels]) => {
  if (key === 'workbench' && panels.length) emit('fallback', panels[0].key)
}, { immediate: true })

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
.package-page { padding:18px 24px; overflow:auto; min-height:0; background:var(--bg-0); container-type:inline-size; }
.package-section-head { display:flex; align-items:center; gap:10px; margin:22px 0 10px; }
.package-section-head h3 { margin:0; font-size:13px; font-weight:600; }.package-section-head>span { font-size:12px; color:var(--text-2); }.package-section-note { margin-left:auto; }
.package-market { margin-top:28px; }
.package-list { border:1px solid var(--border); border-radius:3px; overflow:hidden; }
.package-card { display:grid; grid-template-columns:minmax(0,1fr) 140px 180px 110px; gap:24px; align-items:center; width:100%; padding:14px 16px; text-align:left; background:var(--bg-1); border:0; border-bottom:1px solid color-mix(in srgb,var(--border) 65%,transparent); color:var(--text-0); cursor:pointer; }
.package-card:last-child { border-bottom:0; }.package-card:hover { background:var(--bg-2); }.package-card.active { box-shadow:inset 2px 0 var(--accent); background:color-mix(in srgb,var(--accent) 3%,var(--bg-1)); }
.package-identity,.package-version,.package-author { min-width:0; display:flex; flex-direction:column; gap:5px; overflow-wrap:anywhere; }.package-identity strong { font-size:14px; font-weight:600; }.package-identity>.mono { color:var(--text-2); font-size:12px; }.package-version>span,.package-author>span { color:var(--text-1); font-size:13px; }.package-version small,.package-author small { color:var(--text-2); font-size:11px; }
.package-selection { display:flex; align-items:center; justify-content:flex-end; gap:7px; font-size:12px; color:var(--text-2); }.active .package-selection { color:var(--accent); }.selected-dot { width:5px; height:5px; background:currentColor; border-radius:50%; }.package-note { padding:24px 12px; color:var(--text-2); font-size:13px; text-align:center; }
@container (max-width:700px) { .package-card { grid-template-columns:minmax(0,1fr) 80px; gap:12px; }.package-author { display:none; }.package-version { grid-column:1; }.package-selection { grid-column:2; grid-row:1; }.package-section-note { display:none; } }
@media (max-width:700px) { .package-page { padding:12px; } }

.plugin-workspace { flex:1; min-height:0; display:flex; flex-direction:column; gap:10px; overflow:hidden; }
/* 插件功能子页签条：样式对齐主导航（WorkspaceTabs）的整条页签框 */
.plugin-subnav-tabs { display:flex; flex-shrink:0; border:1px solid var(--border); border-radius:var(--radius-sm); background:var(--bg-2); overflow-x:auto; }
.workspace-subtab { flex:1; min-width:0; padding:7px 3px; border:0; border-left:1px solid var(--border); background:transparent; color:var(--text-1); font-size:12px; cursor:pointer; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
.workspace-subtab:first-child { border-left:0; }
.workspace-subtab:hover { color:var(--text-0); background:var(--bg-3); }
.workspace-subtab.active { color:var(--accent); background:color-mix(in srgb, var(--accent) 14%, transparent); font-weight:600; }
.workspace-panel-slot { flex:1; min-height:0; display:flex; flex-direction:column; overflow:hidden; }
.workspace-empty { flex:1; display:flex; align-items:center; justify-content:center; color:var(--text-2); border:1px solid var(--border); border-radius:var(--radius); padding:0 16px; text-align:center; }
.plugin-workspace{gap:0}.plugin-subnav-tabs{border:0;border-bottom:1px solid var(--border);border-radius:0;min-height:39px;background:var(--bg-1)}.workspace-subtab{flex:0 0 auto;min-width:70px;padding:8px 14px;font-size:13px;border:0;border-bottom:2px solid transparent}.workspace-subtab.active{background:var(--bg-2);border-bottom-color:var(--accent);color:var(--text-0)}.workspace-empty{gap:12px}
.workspace-subtab{position:relative;cursor:grab;flex-shrink:0}.workspace-subtab:active{cursor:grabbing}.workspace-subtab.tab-dragging{opacity:.55}.tab-drop-before::before,.tab-drop-after::after{content:"";position:absolute;top:5px;bottom:5px;width:2px;background:var(--accent)}.tab-drop-before::before{left:0}.tab-drop-after::after{right:0}.reset-tab-order{flex:none;align-self:center;margin:0 5px 0 auto}.tab-order-announcement{position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%);white-space:nowrap}
</style>
