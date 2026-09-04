<template>
  <div ref="consoleEl" class="console" :class="{ 'is-panel-resizing': panelResizing }">
    <!-- 左：工具条与投屏画面共用一个键盘焦点区域 -->
    <div
      ref="stageFocusEl"
      class="stage"
      :class="{ 'keyboard-active': keyboardFocused }"
      tabindex="0"
      role="region"
      aria-label="投屏控制区，可接收键盘控制"
      @focusin="onStageFocusIn"
      @focusout="onStageFocusOut"
      @keydown="onStageKeyDown"
      @keyup="onStageKeyUp"
      @click="onStageClick"
    >
      <!-- 顶部工具条：设备管理与常用投屏控制合并为一行，次要控制收进「更多」 -->
      <div ref="toolbarEl" class="toolbar" data-keyboard-ignore="true" @click="onToolbarClick">
        <div class="tb-row">
          <select v-model="store.deviceId" class="select mono tb-dev-select" @change="onDeviceSelect">
            <option value="">选择设备…</option>
            <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }} · {{ d.status === 'online' ? '在线' : '离线' }}</option>
          </select>
          <button v-if="!connected" class="btn btn-sm btn-primary" :disabled="!store.deviceId || connecting" @click="flushAndConnect">{{ connecting ? '连接中…' : '🔌 连接' }}</button>
          <button v-else class="btn btn-sm" @click="disconnect">⏹ 断开</button>
          <button class="btn btn-sm" :disabled="scanning" @click="refreshDevices">🔄 刷新</button>
          <button class="btn btn-sm" @click="startAdd">＋ 新增</button>
          <button class="btn btn-sm" :disabled="!current" @click="openSettings">⚙️ 设置</button>
          <button class="btn btn-sm btn-danger" :disabled="!current" @click="removeDevice">🗑 删除</button>
          <div class="tb-sep"></div>
          <button class="btn btn-sm" @click="shot">📷 截图</button>
          <button class="btn btn-sm" @click="key('HOME')">🏠 Home</button>
          <button class="btn btn-sm" @click="key('BACK')">⬅ 返回</button>
          <div class="tb-more-wrap">
            <button
              ref="toolbarMoreButton"
              class="btn btn-sm"
              :class="{ active: toolbarMoreOpen }"
              aria-haspopup="menu"
              :aria-expanded="toolbarMoreOpen"
              @click.stop="toggleToolbarMore"
            >更多 ▾</button>
          </div>
          <button class="btn btn-sm" @click="launchGame" :title="'启动到虚拟屏：' + (activePkg || '未选择包名')">🚀 启动应用</button>
          <button class="btn btn-sm" @click="clipboard">📋 粘贴</button>
          <button
            class="btn btn-sm keyboard-mode-btn"
            :class="{ active: keyboardMode === 'text' }"
            :title="keyboardMode === 'text' ? '当前为文本模式，字母和空格按文本发送' : '当前为游戏模式，保留按下/释放按键语义'"
            @click="toggleKeyboardMode"
          >{{ keyboardMode === 'text' ? '⌨ 文本模式' : '🎮 游戏模式' }}</button>
          <select
            v-model="activeKeymapName"
            class="select mono keymap-select"
            :disabled="!activePkg || keymapLoading"
            title="游戏模式下选择当前按键映射；文本模式保留选择但不生效"
            @change="onKeymapChange"
          >
            <option value="">无映射</option>
            <option v-for="item in keymapOptions" :key="item.id || item.file || item.name" :value="item.id || item.file || item.name">{{ item.name || item.file || item.id }}</option>
          </select>
        </div>
      </div>

      <!-- 菜单脱离横向滚动行挂到 body，避免窄窗口下被工具条裁掉 -->
      <Teleport to="body">
        <span v-if="toolbarMoreOpen" class="tb-more-mask" @click.stop="closeToolbarMore"></span>
        <div v-if="toolbarMoreOpen" class="tb-more-dropdown tb-more-dropdown-fixed" :style="toolbarMoreStyle" role="menu">
          <button class="tb-more-item" role="menuitem" @click="closeToolbarMore(); rotate()">🔄 旋转</button>
          <button class="tb-more-item" role="menuitem" @click="closeToolbarMore(); key('APP_SWITCH')">🪟 最近</button>
          <button class="tb-more-item" role="menuitem" @click="closeToolbarMore(); key('VOL_UP')">🔊＋ 音量加</button>
          <button class="tb-more-item" role="menuitem" @click="closeToolbarMore(); key('VOL_DOWN')">🔊－ 音量减</button>
          <button class="tb-more-item" role="menuitem" :title="audioMuted ? '取消静音（听游戏声音）' : '静音'" @click="closeToolbarMore(); toggleAudio()">{{ audioMuted ? '🔊 取消静音' : '🔇 静音' }}</button>
        </div>
      </Teleport>

      <DeviceStage
        :bridge="deviceStageBridge"
        :connected="connected"
        :connecting="connecting"
        :error-msg="errorMsg"
        :current-name="currentName"
        :audio-muted="audioMuted"
        :fps="fps"
        :delay="delay"
        :res="res"
        :bitrate="bitrate"
        :show-hit="showHit"
        :hit-miss="hitMiss"
        :hit-style="hitStyle"
        :hit-label="hitLabel"
        :selecting="selecting"
        :sel-style="selStyle"
        :script-fx="scriptFx"
        :keymap-overlay="keymapOverlay"
        :keymap-status="keymapStatus"
        :bridge-overlays="bridgeOverlayView"
        :fx-tap-style="fxTapStyle"
        :fx-swipe-style="fxSwipeStyle"
        :fx-hit-style="fxHitStyle"
        :loupe="loupe"
        :on-mouse-down="onMouseDown"
        :on-mouse-move="onMouseMove"
        :on-mouse-up="onMouseUp"
        :on-wheel="onWheel"
        :on-video-mouse-leave="onVideoMouseLeave"
        :keyboard-focused="keyboardFocused"
        :flush-and-connect="flushAndConnect"
        :fullscreen="fullscreen"
        @video-mounted="onVideoMounted"
        @wrap-mounted="onVideoWrapMounted"
        @loupe-mounted="onLoupeMounted"
      />

      <!-- 未启动应用提示：连接不再自动启动应用，画面停在桌面/黑屏时容易被误以为卡住。
           纯提示无按钮；显示几秒自动消失，应用已启动（手动/脚本拉起）或脚本运行中不出现 -->
      <div v-if="connected && !appHintDismissed" class="app-hint">
        <span>已连接。未启动应用时画面停在桌面/黑屏</span>
      </div>
    </div>

    <!-- 左右分区拖拽条：拖动可手动调整画面区与功能区宽度 -->
    <div
      class="panel-resizer"
      :class="{ active: panelResizing }"
      role="separator"
      aria-orientation="vertical"
      aria-label="调整画面区与功能区宽度"
      :aria-valuenow="panelWidth"
      :aria-valuemin="PANEL_MIN_WIDTH"
      :aria-valuemax="PANEL_MAX_WIDTH"
      tabindex="0"
      @pointerdown="startPanelResize"
      @pointermove="onPanelResize"
      @pointerup="stopPanelResize"
      @pointercancel="stopPanelResize"
      @lostpointercapture="stopPanelResize"
      @keydown="onPanelResizeKeydown"
    ></div>
    <!-- 右：动态 Extension Workspace；二次裁切弹窗仍挂在面板层级，任何页签可见。 -->
    <aside class="panel" :style="{ width: `${panelWidth}px` }">
      <WorkspaceContextBar :context="workspaceContextBarContext" />
      <PluginWorkspace
        :registry="panelRegistry"
        :active-panel="activePanelKey"
        :context="workspaceContext"
        :lifecycle="workspaceLifecycle"
        @select="openPanel"
        @fallback="fallbackPanel"
        @extensions-changed="refreshServerExtensions"
      />
      <!-- CorePanelHost now mounts these registry contributions. Kept in this
           migration comment to document the old contracts for maintainers:
           <div class="func-pkg-row"><select v-model="activePkg"></select>
           <button @click="loadApps"></button></div>
           <TemplateCapture :context="templateCaptureContext" />
           <ScriptRunner :context="scriptRunnerContext" />
           <KeymapPanel :context="keymapPanelContext" />
           <LogsPanel /> <TaskBoard :active-pkg="activePkg" /> <SystemPanel />
           panelTab === 'tpl'; panelTab === 'script'; panelTab === 'keymap';
           panelTab === 'logs'; panelTab === 'tasks'; panelTab === 'settings'.
           The real mounts are registry-driven above. -->
      <!-- 二次裁切弹窗：挂面板层级（不在模板页签 v-show 内），从脚本编辑发起框选时不切页签 -->
      <TemplateCropModal :context="templateCaptureContext" :on-crop-mounted="onCropMounted" />
    </aside>
    <!-- 设备设置 / 新增设备弹窗 -->
    <DeviceSettingsModal :context="deviceSettingsContext" />

    <!-- 运行参数弹窗（脚本声明 params 时点运行/从此运行弹出；稀疏 args 提交、400 诊断回填标红） -->
    <RunParamsModal
      :open="runArgsFlow.modal.open"
      :title="runArgsFlow.modal.title"
      :desc="runArgsFlow.modal.desc"
      submit-label="▶ 运行"
      :params="runArgsFlow.modal.params"
      :initial-args="runArgsFlow.modal.initialArgs"
      :suggestions="runArgsFlow.modal.suggestions"
      :templates="runArgsFlow.modal.templates"
      :field-errors="runArgsFlow.modal.fieldErrors"
      :general-errors="runArgsFlow.modal.generalErrors"
      :submitting="runArgsFlow.modal.submitting"
      @submit="onRunArgsSubmit"
      @close="runArgsFlow.close()"
    />

    <!-- 设备占用冲突 409 提示（对方脚本/来源/开始时间；仍要查看日志 → 跳控制台对应设备） -->
    <RunConflictModal />
  </div>
</template>

<script setup>
// Console 壳：模板装配 + 各拆分模块接线。逻辑按域拆分至 components/console/ 下的
// composables（设备管理 / 模板裁切 / bridge overlay / 脚本运行 / 按键映射 /
// 传输统计 / workspace 面板接线），本文件保留投屏连接、输入控制与跨模块 glue。
import { computed, nextTick, onMounted, onUnmounted, provide, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { store, devicesData, scriptsData, templatesData, useToast, appStartedDevices } from '../store'
import { api } from '../api'
import DeviceStage from '../workspace/DeviceStage.vue'
import WorkspaceContextBar from '../workspace/WorkspaceContextBar.vue'
import PluginWorkspace from '../workspace/PluginWorkspace.vue'
import { createPanelRegistry, DEFAULT_PANEL_KEY } from '../workspace/registry'
import { createWorkspaceContext, PANEL_REGISTRY_KEY, WORKSPACE_CONTEXT_KEY } from '../workspace/context'
import { createWorkspaceLifecycle } from '../workspace/lifecycle'
import { registerCoreContributions } from '../workspace/core-contributions'
import { registerKeymapExtension } from '../workspace/keymap-extension'
import { createServerUiContributionAdapter } from '../workspace/plugin-center/adapter/server-ui'
import DeviceSettingsModal from '../components/console/DeviceSettingsModal.vue'
import TemplateCapture from '../components/console/TemplateCapture.vue'
import TemplateCropModal from '../components/console/TemplateCropModal.vue'
import ScriptRunner from '../components/console/ScriptRunner.vue'
import KeymapPanel from '../components/console/KeymapPanel.vue'
import LogsPanel from '../components/LogsPanel.vue'
import TaskBoard from '../components/TaskBoard.vue'
import SystemPanel from '../components/SystemPanel.vue'
import RunConflictModal from '../components/RunConflictModal.vue'
import RunParamsModal from '../components/RunParamsModal.vue'
import { useConsoleRuntime } from '../composables/useConsoleRuntime'
import { useWebRtcLifecycle } from '../composables/useWebRtcLifecycle'
import { useWorkspacePackages } from '../composables/useWorkspacePackages'
import { createKeyboardController, shouldIgnoreKeyboardTarget } from '../keyboard-control'
import { buildTouchPhase, createKeymapController } from '../keymap-control'
import { useConsolePanelResize } from '../components/console/useConsolePanelResize'
import { useConsoleDeviceManager } from '../components/console/useConsoleDeviceManager'
import { useConsoleTemplates } from '../components/console/useConsoleTemplates'
import { useConsoleBridgeOverlays } from '../components/console/useConsoleBridgeOverlays'
import { useConsoleScriptRunner } from '../components/console/useConsoleScriptRunner'
import { useConsoleKeymap } from '../components/console/useConsoleKeymap'
import { useWebrtcStats } from '../components/console/useWebrtcStats'
import { useConsoleWorkspacePanels } from '../components/console/useConsoleWorkspacePanels'
import { createPluginCallAdapter } from '../components/console/current-api-adapters'

const toast = useToast()
const route = useRoute()
const router = useRouter()

// ---------- 共享基础状态（跨拆分模块的连接/画面/包名状态，由本壳统一持有） ----------
// 侧边栏已移除：右侧面板默认 340px，宽度由分隔条手动调整。
const superseded = ref(false)
const manualClose = ref(false)
const connected = ref(false)
const connecting = ref(false)
const errorMsg = ref('')
const fps = ref(0)
const delay = ref(0)
const res = ref('—')
const bitrate = ref('—')
const audioMuted = ref(true)
const consoleEl = ref(null)
const stageFocusEl = ref(null)
const toolbarEl = ref(null)
const videoWrap = ref(null)
const videoElement = ref(null)
const keyboardFocused = ref(false)
const keyboardMode = ref('game')
const keymapPressed = reactive(new Set())
// 当前包名：右侧下拉是唯一选择入口；模板、脚本、函数库和启动/匹配等后续操作都使用它。
// 初始值仍可从旧设备记录的 pkg 恢复，兼容已有配置；切换 activePkg 不会启动应用。
const activePkg = ref('')
// 远端 keymap 扩展运行中：鼠标/滚轮/手柄输入改经 keymap 控制器（workspace 轮询写、输入层读）
const remoteKeymapRunning = ref(false)
// 传输统计看门狗所需的时间戳（连接生命周期的一部分，留在本壳）
let videoConnectTs = 0
let lastDragInputAt = 0
let keyboardChannelWarned = false

// ---------- 未启动应用提示（app-hint）：连接时出现、显示几秒自动消失（纯提示无按钮）。
// 应用已启动（手动点过启动按钮 / 脚本 str_app 拉起——按设备记入共享 store，
// 跨 SPA 切页存活）或脚本运行中则完全不出现
const APP_HINT_AUTO_CLOSE_MS = 5000
const appHintDismissed = ref(false)
let appHintTimer = null
watch(connected, (on) => {
  if (appHintTimer) { clearTimeout(appHintTimer); appHintTimer = null }
  if (!on) return
  appHintTimer = setTimeout(() => {
    appHintTimer = null
    appHintDismissed.value = true
  }, APP_HINT_AUTO_CLOSE_MS)
})
watch(() => store.running, (running) => {
  if (!running) return
  if (store.deviceId) appStartedDevices.add(store.deviceId)
  appHintDismissed.value = true
})

// ---------- 左右分区拖拽（面板宽度） ----------
const {
  panelWidth, panelResizing, PANEL_MIN_WIDTH, PANEL_MAX_WIDTH,
  startPanelResize, onPanelResize, stopPanelResize, onPanelResizeKeydown,
} = useConsolePanelResize({ consoleEl })

// ---------- 运行时数据加载/重连编排 ----------
const consoleRuntime = useConsoleRuntime({
  api,
  devicesData,
  scriptsData,
  templatesData,
  toast,
  deviceIdRef: computed(() => store.deviceId),
})

async function loadData() {
  await consoleRuntime.loadData()
}

// ---------- 设备管理（工具条设备控件 + 设置弹窗 + 工具条快捷动作） ----------
const {
  devices, current, currentName, pkgOptions,
  mode, form, scanning, configApplying, settingsOpen,
  kindInfo, screenSummary, formDirty,
  startAdd, openSettings, cancelSettings, onDeviceSelect, refreshDeviceStatus, refreshDevices,
  saveSettings, flushAndConnect, addDevice, removeDevice, disconnect, loadApps,
  key, toolbarMoreOpen, toolbarMoreButton, toolbarMoreStyle,
  closeToolbarMore, toggleToolbarMore, shot, rotate, clipboard, launchGame,
  deviceSettingsContext, workspaceContextBarContext,
} = useConsoleDeviceManager({
  toast,
  store,
  devicesData,
  scriptsData,
  templatesData,
  consoleRuntime,
  connected,
  errorMsg,
  activePkg,
  appHintDismissed,
  loadData,
  connect,
  cleanup,
  sendControl,
})

// ---------- 键盘与按键映射控制器（映射层命中时消费事件，未命中才交给 keyboard） ----------
const keyboard = createKeyboardController({
  send: sendKeyboardControl,
  onText: sendControl,
  mode: keyboardMode,
})
// 通过回调读取当前活动模型，因此切换方案不需要重建控制器或刷新页面。
const keymap = createKeymapController({
  getKeymap: () => activeKeymapModel.value,
  sendControl,
  send: sendControl,
  remote: remoteKeymapRunning,
  sendInputEvent: sendControl,
  getVideoSize: () => ({
    width: videoElement.value?.videoWidth || 1920,
    height: videoElement.value?.videoHeight || 1080,
  }),
  getKeyMetaState: () => keyboard.getMetaState(),
  mode: keyboardMode,
})

function syncKeymapPressed() {
  const codes = typeof keymap.getPressedCodes === 'function' ? keymap.getPressedCodes() : []
  keymapPressed.clear()
  for (const code of codes || []) keymapPressed.add(code)
}

// ---------- 模板页签（列表/框选/二次裁切/放大镜/测试匹配/取值工具） ----------
const {
  picking, selecting, selStart, selEnd, showHit, hitLabel, hitMiss, hitStyle, selStyle,
  testThreshold, testRegion, tplSearch, templates, templateNames,
  viewTpl, confirmDelTpl, renaming, renameVal, crop, cropSize, cropZoomPct, saving,
  loupe,
  onLoupeMounted, onCropMounted, setRenameInputEl,
  togglePick, openCrop, selToDeviceRect, hideLoupe, updateLoupe, toDeviceCoord, deviceRectStyle,
  cropMouseDown, cropMouseMove, cropMouseUp, cropMouseLeave, cropWheel,
  saveTemplate, overwriteTemplate, backToCrop, cancelCrop, repick,
  onTplRowClick, onTplThumbClick, onTplNameClick, confirmRename, cancelRename, startRename,
  onTplDeleteClick, onTplMatchClick, onTplUpload, replaceTemplateImage,
  tplShortName, tplRegionBadge, tplThumbUrl, testMatch,
  selectRegionForBridge, beginCellPick, cancelCellPick, finishCellPick, cellPick,
  bridgeRegionSelected, finishBridgeRegionSelect, cancelBridgeRegionSelect, closeTplView,
  templateCaptureContext,
} = useConsoleTemplates({
  toast,
  store,
  templatesData,
  activePkg,
  connected,
  videoElement,
  videoWrap,
  current,
  pkgOptions,
  loadData,
  // 脚本运行 composable 的能力经懒解析箭头注入（规避组合顺序）
  editorMatchThreshold: () => editorMatchThreshold(),
  clearCallParamsCache: () => clearCallParamsCache(),
})

// ---------- bridge overlay（sandbox UI 申请的画面叠加框） ----------
const { bridgeOverlayView, showBridgeOverlay, clearBridgeOverlay } = useConsoleBridgeOverlays({
  videoElement,
  deviceRectStyle,
})

// ---------- 脚本运行/编辑（运行区、编辑外壳、运行参数流程、运行日志与轮询） ----------
const {
  scriptShell, rawEditor, fnLib,
  scriptMode, selScript, selFnFile, runKind, scriptDeleteConfirmId,
  canRunTarget, selTargetId, showYaml, editFocusFn, resourcePreview,
  liveLogs, startPending, runStopping, runArgsFlow,
  funcFnViews, funcSummaryError, summaryModel, summaryError,
  startLogPolling, stopLogPolling, pushLog, autoSaveDebounced,
  clearCallParamsCache, editorMatchThreshold,
  cancelEditScript, startNewScript, editCurrentTarget, editRawCurrentTarget, saveRawScript, cancelRawScript,
  startNewTarget, deleteCurrentTarget, addFunctionToCurrentFile, renameEditingFunction, deleteFunction,
  editCurrentScript, deleteCurrentScript, saveEditScript,
  onConflictReload, onConflictOverwrite, onConflictDismiss,
  runFromStep, openScriptTarget, closeResourcePreview, jumpBack,
  runScript, onRunArgsSubmit, stopScript,
  startRunStatusPoll, restoreRunState, onBeforeUnload, onLogBoxMounted,
  scriptRunnerContext,
} = useConsoleScriptRunner({
  toast,
  activePkg,
  consoleRuntime,
  templateNames,
  tplShortName,
  loadData,
})

// ---------- 按键映射面板（方案选择/保存/导入导出 + 映射可视化） ----------
const {
  keymapOptions, activeKeymapName, activeKeymapDisplayName, activeKeymapModel,
  keymapLoading, keymapError, keymapOverlay, keymapStatus,
  loadKeymaps, onKeymapChange,
  keymapPanelContext,
} = useConsoleKeymap({
  api,
  toast,
  activePkg,
  keyboardMode,
  keymap,
  keymapPressed,
  videoElement,
  videoWrap,
  deviceRectStyle,
  pickCoord: () => beginCellPick('coord'),
})

watch(activePkg, pkg => {
  fnLib.refresh(pkg)
  loadKeymaps(pkg)
})

// ---------- 游戏包三入口（导入/导出/编辑）：逻辑在 composable，context 并入右侧上下文条 ----------
// WorkspaceContextBar 只透传；导入/编辑替换当前分区资源后经注入的刷新回调全量重拉
//（activePkg 未变，watch 不触发，必须显式刷新 fnLib/keymap）
const workspacePackages = useWorkspacePackages({
  toast,
  activePkg,
  loadData,
  refreshFnLib: pkg => fnLib.refresh(pkg),
  refreshKeymaps: pkg => loadKeymaps(pkg),
})
Object.assign(workspaceContextBarContext, workspacePackages.context)

// ---------- 传输统计与画面自愈看门狗 ----------
const { startStats, stopStats, resetWatchdogs, resetBlackWatchdog } = useWebrtcStats({
  getPeerConnection: () => webrtcLifecycle.getPeerConnection(),
  connected,
  videoElement,
  fps,
  delay,
  bitrate,
  sendControl,
  handleVideoSilence,
  getVideoConnectTs: () => videoConnectTs,
  getLastDragInputAt: () => lastDragInputAt,
})

// ---------- Frontend Plugin Workspace ----------
// Core panels are contributions too: one plugin may register multiple panels,
// and the workspace never needs to know a panel's component implementation.
const workspaceLifecycle = createWorkspaceLifecycle()
const panelRegistry = createPanelRegistry({ defaultPanelKey: DEFAULT_PANEL_KEY })
// Workspace 以稳定的 pluginId:panelId 作为 URL key；panelTab 保留为旧上下文
// 的兼容字段，实际导航由 activePanelKey + PanelRegistry 负责。
const panelTab = ref('script')
const activePanelKey = ref(DEFAULT_PANEL_KEY)
registerCoreContributions(panelRegistry, {
  TemplateCapture, ScriptRunner, LogsPanel, TaskBoard, SystemPanel,
}, {
  templateCapture: templateCaptureContext,
  scriptRunner: scriptRunnerContext,
  activePkg,
})
const keymapExtension = registerKeymapExtension(panelRegistry, workspaceLifecycle, {
  component: KeymapPanel,
  context: keymapPanelContext,
  // The browser controller is the transport-facing part of this extension;
  // its runtime is independent from whether the panel tab is mounted.
  runtime: {
    start: () => keymap.setEnabled(true),
    stop: () => {
      keymap.releaseAll()
      keymap.setEnabled(false)
    },
  },
})
void keymapExtension.start()
const serverUiAdapter = createServerUiContributionAdapter(panelRegistry, {
  load: () => api.listExtensions(),
})
const {
  refreshServerExtensions, startExtensionPolling, openPanel, fallbackPanel,
} = useConsoleWorkspacePanels({
  route,
  router,
  panelRegistry,
  keymapExtension,
  serverUiAdapter,
  remoteKeymapRunning,
  keymap,
  connected,
  panelTab,
  activePanelKey,
})

const workspaceContext = createWorkspaceContext({
  device: current,
  deviceId: computed(() => store.deviceId),
  activePackage: activePkg,
  connected,
  stage: {
    selectRegion: selectRegionForBridge,
    pickPoint: () => beginCellPick('coord'),
    overlay: { show: showBridgeOverlay, clear: clearBridgeOverlay },
  },
  openPanel,
  toast,
  dialogConfirm: message => window.confirm(message),
  // iframe 面板 plugin.call → UI Bridge → 这里转发到 REST /api/extensions/:id/call
  //（declarative 面板不经 bridge，已直连 callExtension）
  pluginCall: createPluginCallAdapter(api),
  core: {
    templateCapture: templateCaptureContext,
    scriptRunner: scriptRunnerContext,
    keymap: keymapPanelContext,
    activePkg,
  },
})
const deviceStageBridge = workspaceContext.stage
provide(PANEL_REGISTRY_KEY, panelRegistry)
provide(WORKSPACE_CONTEXT_KEY, workspaceContext)

// ---------- WebRTC 连接 ----------
const webrtcLifecycle = useWebRtcLifecycle({
  // 包一层 connectDevice：捕获服务端 app_started（建会话探测/会话内启动过应用），
  // 手动连接与自动重连两条路径都能及时把设备记入「已启动」，抑制未启动提示
  api: {
    ...api,
    connectDevice: async (id) => {
      const rep = await api.connectDevice(id)
      if (rep?.app_started && id) appStartedDevices.add(id)
      return rep
    },
  },
  deviceIdRef: computed(() => store.deviceId),
  connectedRef: connected,
  connectingRef: connecting,
  errorMsgRef: errorMsg,
  supersededRef: superseded,
  manualCloseRef: manualClose,
  toast,
  onConnectStart() {
    errorMsg.value = ''
    connecting.value = true
  },
  onConnectSuccess() {
    startStats()
    startLogPolling()
    // 连接成功（手动/自动重连/接管同路径）即拉一次设备列表：下拉里的
    // 「在线/离线」标签随建链更新，不再停留「离线」等用户手动刷新
    refreshDeviceStatus()
  },
  onDisconnect() {
    keymap.releaseAll()
    syncKeymapPressed()
    keyboard.releaseAll()
    keyboardFocused.value = false
    stopStats()
    stopLogPolling()
    connected.value = false
    resetWatchdogs()
    if (videoElement.value) videoElement.value.srcObject = null
    hideLoupe()
  },
  onChannelOpen() {
    connected.value = true
    connecting.value = false
    controlChannel = webrtcLifecycle.getControlChannel()
    keyboardChannelWarned = false
    keyboardFocused.value = document.activeElement === stageFocusEl.value
    videoConnectTs = Date.now()
    resetBlackWatchdog()
    // 该设备本会话已启动过应用（手动/脚本拉起/脚本仍在运行）→ 重连不再弹提示
    appHintDismissed.value = store.running
      || (store.deviceId ? appStartedDevices.has(store.deviceId) : false)
    sendControl({ type: 'audio', on: !audioMuted.value })
    toast('WebRTC 连接建立', 'success')
  },
  onChannelClose() {
    keymap.releaseAll()
    syncKeymapPressed()
    keyboard.releaseAll()
    connected.value = false
    controlChannel = null
  },
  onRemoteTrack({ event, pc: currentPc }) {
    if (event.target !== currentPc) return
    mediaStream = event.streams[0] || new MediaStream([event.track])
    if (event.track.kind === 'audio') event.track.enabled = !audioMuted.value
    if (videoElement.value) {
      videoElement.value.srcObject = mediaStream
      videoElement.value.play().catch(() => {})
    }
    const v = event.track
    v.addEventListener('unmute', () => {
      setTimeout(() => {
        const w = videoElement.value?.videoWidth || 0
        const h = videoElement.value?.videoHeight || 0
        if (w) res.value = `${w}x${h}`
      }, 200)
    })
  },
  onControlMessage(e) {
    onControlMessage(e)
  },
  // taken_over 处理收敛在 useWebRtcLifecycle 内部（superseded 置位 + toast +
  // 错误栏持久文案「已被其它页面接管」），此处不再重复挂 onSignalMessage——
  // 旧的双份 toast/置位会让提示重复且不落持久横幅
  onPeerDisposed() {
    controlChannel = null
    mediaStream = null
  },
})

function scheduleReconnect() {
  webrtcLifecycle.scheduleReconnect({ superseded })
}

async function connect(manual = false) {
  await webrtcLifecycle.connect(manual)
}

/** 释放 WebRTC 资源；manual=true 表示主动关闭（不触发自动重连） */
function cleanup(manual = false) {
  keymap.releaseAll()
  syncKeymapPressed()
  keyboard.releaseAll()
  webrtcLifecycle.cleanup(manual)
  consoleRuntime.cleanup()
}

function handleVideoSilence() {
  if (manualClose.value || !connected.value || !store.deviceId) return
  console.warn('[webrtc] video stream silent, treating as disconnected')
  connected.value = false
  scheduleReconnect()
}

// ---------- 控制（走 DataChannel） ----------

const REST_FALLBACK_CONTROL_TYPES = new Set([
  'tap', 'swipe', 'text', 'press', 'home', 'back', 'recents', 'start_app', 'rotate', 'clipboard',
])

/** 键盘是有状态的 DOWN/UP 流，只允许走 DataChannel；不能复用 sendControl 的
 * REST fallback，否则通道断开时一次 keydown 会被错误降级为不兼容的 press。 */
function sendKeyboardControl(obj) {
  const channel = webrtcLifecycle.getControlChannel() || controlChannel
  if (channel && channel.readyState === 'open') {
    channel.send(JSON.stringify(obj))
    keyboardChannelWarned = false
    return true
  }
  if (!keyboardChannelWarned) {
    keyboardChannelWarned = true
    toast('键盘控制通道未连接', 'warn')
  }
  return false
}

function sendControl(obj) {
  // 拖动/滚轮类输入打标（画面停滞看门狗用）：这类操作预期画面变化，
  // 若随后渲染指纹持续冻结则流已病态（见 startStats 处注释）
  if ((obj.type === 'touch' && obj.action === 'move') || obj.type === 'scroll' || obj.type === 'swipe') {
    lastDragInputAt = Date.now()
  }
  const channel = webrtcLifecycle.getControlChannel() || controlChannel
  if (channel && channel.readyState === 'open') {
    channel.send(JSON.stringify(obj))
    return true
  }
  // 有状态消息必须保持在 DataChannel 内：REST 只有一次性动作语义，
  // 不能把 touch down/up 或 key down/up 降级成 press，否则会留下半截状态
  // 或把虚拟触控误发成 Android 物理按键。
  const stateful = obj?.type === 'touch'
    || obj?.type === 'input_event'
    || (obj?.type === 'key' && (obj?.action === 0 || obj?.action === 1))
  if (stateful) {
    console.warn('[control] channel not open, stateful control dropped', JSON.stringify(obj))
    if (!keyboardChannelWarned) {
      keyboardChannelWarned = true
      toast('控制通道未连接', 'warn')
    }
    return false
  }
  if (!REST_FALLBACK_CONTROL_TYPES.has(obj?.type)) {
    console.warn('[control] channel not open, unsupported control dropped', JSON.stringify(obj))
    return false
  }
  console.warn('[control] channel not open, fallback REST', JSON.stringify(obj))
  // fallback：REST API
  api.control(store.deviceId, obj).catch(e => toast('控制失败：' + e.message, 'error'))
  // REST 请求已经接管了一次性动作；返回 true 避免映射层误把同一按键
  // 再交给原始 Android key 控制器。
  return true
}

/** 统一构造触控阶段消息。pointer_id=0 保留给投屏鼠标。 */
function sendTouchPhase(action, pointerId, x, y) {
  return sendControl(buildTouchPhase(action, pointerId, x, y))
}

// let controlChannel/mediaStream：与 WebRTC 生命周期同寿（由上方回调赋值）。
let controlChannel = null
let mediaStream = null

// ---------- 键盘焦点区域与工具条 ----------

function onStageFocusIn(e) {
  if (!connected.value || shouldIgnoreKeyboardTarget(e?.target)) return
  keyboardFocused.value = true
}

function onStageFocusOut(e) {
  const next = e?.relatedTarget
  if (next && stageFocusEl.value?.contains(next)) return
  keyboardFocused.value = false
  keymap.releaseAll()
  syncKeymapPressed()
  keyboard.releaseAll()
}

function onStageKeyDown(e) {
  if (!connected.value || picking.value || selecting.value || cellPick.mode || isGlobalEscapeConsumed(e)) return
  if (keyboardMode.value === 'game') {
    const mapped = keymap.handleKeyDown(e)
    syncKeymapPressed()
    if (mapped?.handled || mapped === true) return
  }
  // 控制器只对已映射且未被 UI 过滤的按键 preventDefault；未知按键保留浏览器行为。
  keyboard.handleKeyDown(e)
}

function onStageKeyUp(e) {
  if (!connected.value) return
  const mapped = keymap.handleKeyUp(e)
  syncKeymapPressed()
  if (mapped?.handled || mapped === true) return
  keyboard.handleKeyUp(e)
}

function onStageClick(e) {
  const target = e?.target
  if (target?.closest?.('button, input, select, textarea, a, [contenteditable], [role="button"], [role="link"], [role="menuitem"]')) return
  stageFocusEl.value?.focus()
}

function onToolbarClick(e) {
  const target = e?.target
  const button = target?.closest?.('button')
  if (!button || !toolbarEl.value?.contains(button)) return
  // 工具栏按钮执行完动作后把焦点还给组合区域，点击模式切换后可以直接输入。
  nextTick(() => stageFocusEl.value?.focus())
}

function toggleKeyboardMode() {
  keyboardMode.value = keyboardMode.value === 'game' ? 'text' : 'game'
  nextTick(() => stageFocusEl.value?.focus())
}

watch(keyboardMode, mode => {
  if (mode === 'text') {
    keymap.releaseAll()
    syncKeymapPressed()
  }
})

function onWindowBlur() {
  keymap.releaseAll()
  syncKeymapPressed()
  keyboard.releaseAll()
}

function onVisibilityChange() {
  if (document.hidden) {
    keymap.releaseAll()
    syncKeymapPressed()
    keyboard.releaseAll()
  }
}

/** 全局 Escape 关闭页面 UI 优先；只有没有待关闭 UI 时才把 Escape 转发给设备。 */
function isGlobalEscapeConsumed(e) {
  if (e?.code !== 'Escape' && e?.key !== 'Escape') return false
  return !!(
    cellPick.mode
    || toolbarMoreOpen.value
    || settingsOpen.value
    || viewTpl.value
    || resourcePreview.open
    || confirmDelTpl.value
  )
}

/** 全局按键：Esc 关闭工具条菜单 / 设备设置弹窗 / 模板大图 / 资源预览 / 取消删除确认 */
function onGlobalKeydown(e) {
  if (e.key !== 'Escape') return
  if (cancelBridgeRegionSelect()) {
    // bridge 框选被 Esc 取消
  } else if (cellPick.mode) {
    cancelCellPick()
  } else if (toolbarMoreOpen.value) {
    closeToolbarMore()
  } else if (settingsOpen.value) {
    cancelSettings()
  } else if (viewTpl.value) {
    closeTplView()
  } else if (resourcePreview.open) {
    closeResourcePreview()
  } else if (confirmDelTpl.value) {
    confirmDelTpl.value = null
  }
}

// ---------- 脚本运行可视化效果 ----------

// 服务端经 control DataChannel 推送 tap/swipe/hit/miss 事件（设备像素坐标），
// 与手动 alt 反馈状态独立（脚本运行时用户仍可手动操作，两类效果互不覆盖）
const scriptFx = reactive({
  tap: { show: false, x: 0, y: 0 },
  swipe: { show: false, x: 0, y: 0, w: 0, h: 0 },
  hit: { show: false, x: 0, y: 0, w: 0, h: 0, label: '', miss: false },
})
let fxTapTimer = null
let fxSwipeTimer = null
let fxHitTimer = null

/** 服务端→浏览器脚本可视化事件（{"type":"se","ev":"tap"|"swipe"|"hit"|"miss", ...}，设备像素坐标）：
 *  引擎执行 tap/swipe、模板匹配命中/未命中时推送到投屏画面
 *  （样式复用 alt 反馈/测试匹配命中框；miss 显示搜索区域，虚线红框）
 *  同一轮匹配的多个模板事件会互相顶替，显示的是最新一次 */
function onControlMessage(e) {
  let msg
  try { msg = JSON.parse(e.data) } catch (err) { return }
  if (!msg || msg.type !== 'se') return
  if (msg.ev === 'tap') {
    scriptFx.tap.x = msg.x || 0
    scriptFx.tap.y = msg.y || 0
    scriptFx.tap.show = true
    if (fxTapTimer) clearTimeout(fxTapTimer)
    fxTapTimer = setTimeout(() => { scriptFx.tap.show = false }, 2000)
  } else if (msg.ev === 'swipe') {
    const { x1 = 0, y1 = 0, x2 = 0, y2 = 0 } = msg
    scriptFx.swipe.x = Math.min(x1, x2)
    scriptFx.swipe.y = Math.min(y1, y2)
    scriptFx.swipe.w = Math.abs(x2 - x1)
    scriptFx.swipe.h = Math.abs(y2 - y1)
    scriptFx.swipe.show = true
    if (fxSwipeTimer) clearTimeout(fxSwipeTimer)
    fxSwipeTimer = setTimeout(() => { scriptFx.swipe.show = false }, 2000)
  } else if (msg.ev === 'hit') {
    scriptFx.hit.x = msg.x || 0
    scriptFx.hit.y = msg.y || 0
    scriptFx.hit.w = msg.w || 0
    scriptFx.hit.h = msg.h || 0
    scriptFx.hit.label = `${msg.tpl || ''} ${Number(msg.score || 0).toFixed(2)}`
    scriptFx.hit.miss = false
    scriptFx.hit.show = true
    if (fxHitTimer) clearTimeout(fxHitTimer)
    fxHitTimer = setTimeout(() => { scriptFx.hit.show = false }, 3000)
  } else if (msg.ev === 'miss') {
    // 未命中：显示本次搜索区域（引擎无 #后缀回退全屏时推 [0,0,w,h] 全屏框）
    scriptFx.hit.x = msg.x || 0
    scriptFx.hit.y = msg.y || 0
    scriptFx.hit.w = msg.w || 0
    scriptFx.hit.h = msg.h || 0
    scriptFx.hit.label = `${msg.tpl || ''} 未命中`
    scriptFx.hit.miss = true
    scriptFx.hit.show = true
    if (fxHitTimer) clearTimeout(fxHitTimer)
    fxHitTimer = setTimeout(() => { scriptFx.hit.show = false }, 3000)
  }
}

/** 脚本运行可视化效果位置（tap 圆点居中偏移由 .alt-tap 的 transform 处理） */
const fxTapStyle = computed(() => (scriptFx.tap.show ? deviceRectStyle(scriptFx.tap.x, scriptFx.tap.y) : {}))
const fxSwipeStyle = computed(() => (scriptFx.swipe.show
  ? deviceRectStyle(scriptFx.swipe.x, scriptFx.swipe.y, scriptFx.swipe.w, scriptFx.swipe.h)
  : {}))
const fxHitStyle = computed(() => (scriptFx.hit.show
  ? deviceRectStyle(scriptFx.hit.x, scriptFx.hit.y, scriptFx.hit.w, scriptFx.hit.h)
  : {}))

// ---------- 鼠标/滚轮输入（触控、框选、取点、映射输入路由） ----------

// 触控状态
const touchState = reactive({ active: false, lastX: 0, lastY: 0 })

// 拖动 move 事件合并：鼠标高频事件（数百 Hz）逐条发送会打爆 DataChannel/服务端日志，
// 这里按 rAF（约 60Hz）合并发送，拖拽手感不受影响，但延迟和负载大幅下降。
let pendingMove = null
let moveRaf = 0
function flushPendingMove() {
  moveRaf = 0
  if (pendingMove) {
    const p = pendingMove
    pendingMove = null
    sendControl(p)
  }
}
function scheduleMove(x, y) {
  pendingMove = { type: 'touch', action: 'move', pointer_id: 0, x, y }
  if (!moveRaf) moveRaf = requestAnimationFrame(flushPendingMove)
}
function cancelPendingMove() {
  if (moveRaf) { cancelAnimationFrame(moveRaf); moveRaf = 0 }
  pendingMove = null
}

function onMouseDown(e) {
  // 步骤编辑器取点/取色模式：本次画面点击被消费（不透传触控）
  if (cellPick.mode && connected.value) {
    finishCellPick(e)
    return
  }
  if (picking.value && connected.value) {
    const rect = videoWrap.value.getBoundingClientRect()
    selStart.x = e.clientX - rect.left
    selStart.y = e.clientY - rect.top
    selEnd.x = selStart.x; selEnd.y = selStart.y
    selecting.value = true
    return
  }
  if (!connected.value) return
  cancelPendingMove()
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  if (remoteKeymapRunning.value) {
    keymap.handleInputEvent({ type: 'mousedown', button: e.button, x, y }, 'down', e)
    return
  }
  touchState.active = true
  touchState.lastX = x; touchState.lastY = y
  // 按下：发 DOWN（拖动时后续 move 事件组成轨迹，up 时收尾）
  sendTouchPhase('down', 0, x, y)
}

function onMouseMove(e) {
  if (selecting.value) {
    const rect = videoWrap.value.getBoundingClientRect()
    selEnd.x = e.clientX - rect.left
    selEnd.y = e.clientY - rect.top
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 2.5, [selToDeviceRect()])
    return
  }
  if (cellPick.mode) {
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 3, [])
    return
  }
  if (picking.value) {
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 2.5, [])
    return
  }
  if (remoteKeymapRunning.value && connected.value) {
    const { x, y } = toDeviceCoord(e.clientX, e.clientY)
    keymap.handleInputEvent({
      type: 'mousemove', x, y, movementX: e.movementX, movementY: e.movementY,
    }, 'move', e)
    return
  }
  if (!touchState.active || !connected.value) return
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  if (Math.abs(x - touchState.lastX) + Math.abs(y - touchState.lastY) > 6) {
    touchState.lastX = x; touchState.lastY = y
    scheduleMove(x, y)
  }
}

function onMouseUp(e) {
  if (selecting.value) {
    selecting.value = false
    picking.value = false
    hideLoupe()
    const rect = selToDeviceRect()
    if (bridgeRegionSelected()) {
      finishBridgeRegionSelect(rect)
      return
    }
    if (rect.w >= 8 && rect.h >= 8) openCrop(rect)
    else toast('框选区域太小，请重新框选', 'warn')
    return
  }  if (remoteKeymapRunning.value && connected.value) {
    const { x, y } = toDeviceCoord(e.clientX, e.clientY)
    keymap.handleInputEvent({ type: 'mouseup', button: e.button, x, y }, 'up', e)
    return
  }
  if (!touchState.active) return
  cancelPendingMove()
  touchState.active = false
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  sendTouchPhase('up', 0, x, y)
}

/** 鼠标离开投屏区域时隐藏取点/框选辅助层。 */
function onVideoMouseLeave() {
  hideLoupe()
}

function onWheel(e) {
  if (!connected.value) return
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  if (remoteKeymapRunning.value) {
    keymap.handleInputEvent({
      type: 'wheel', x, y, deltaX: e.deltaX, deltaY: e.deltaY,
    }, 'wheel', e)
    return
  }
  sendControl({ type: 'scroll', x, y, scroll_x: e.deltaX, scroll_y: e.deltaY })
}

function fullscreen() {
  if (videoWrap.value?.requestFullscreen) videoWrap.value.requestFullscreen()
}

function onVideoMounted(el) { videoElement.value = el }
function onVideoWrapMounted(el) { videoWrap.value = el }

// ---------- 生命周期 ----------

onMounted(async () => {
  // SPA 内跳转（store 存活）→ 自动重连恢复画面；页面刷新 → localStorage 恢复设备选择；
  // 首次进入仅选中第一台设备，等待用户点连接（不主动建会话，尊重空闲低功耗）
  const spaPreselected = !!store.deviceId
  await loadData()
  await refreshServerExtensions()
  startExtensionPolling()
  if (!store.deviceId) {
    const saved = localStorage.getItem('gb_device_id')
    store.deviceId = (saved && devices.value.find(d => d.id === saved)) ? saved : (devices.value[0]?.id || null)
  }
  const d = current.value
  if (d) loadForm(d, { syncPkg: true })
  else { mode.value = 'edit'; store.deviceId = null }
  window.addEventListener('keydown', onGlobalKeydown)
  window.addEventListener('beforeunload', onBeforeUnload)

  // 刷新恢复运行态：刷新前发起的脚本在服务端继续执行——按设备查询当前活动 run。
  // 当前契约为 active:false 或 active:true + 嵌套完整 RunRecord，含来源标签；
  // 恢复运行状态/选中脚本/状态轮询与日志（不依赖投屏连接是否恢复成功）
  if (store.deviceId) await restoreRunState()
  // 画面恢复：SPA 内返回（store 存活）或刷新后脚本运行中/设备会话在线（此前正在
  // 投屏）→ 自动连接；设备空闲离线则保持首次进入行为；遇 conflict 不抢（connect 内处理）
  if (store.deviceId && (spaPreselected || store.running || current.value?.status === 'online')) connect(false)
  // 其他页面已启动脚本时，本页接管状态轮询（脚本结束后复位运行状态）
  if (store.running && store.runId) startRunStatusPoll()
  window.addEventListener('blur', onWindowBlur)
  document.addEventListener('visibilitychange', onVisibilityChange)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onGlobalKeydown)
  window.removeEventListener('beforeunload', onBeforeUnload)
  window.removeEventListener('blur', onWindowBlur)
  document.removeEventListener('visibilitychange', onVisibilityChange)
  keymap.releaseAll()
  syncKeymapPressed()
  keyboard.releaseAll()
  consoleRuntime.cancelReconnect()
  if (appHintTimer) { clearTimeout(appHintTimer); appHintTimer = null }
  if (fxTapTimer) { clearTimeout(fxTapTimer); fxTapTimer = null }
  if (fxSwipeTimer) { clearTimeout(fxSwipeTimer); fxSwipeTimer = null }
  if (fxHitTimer) { clearTimeout(fxHitTimer); fxHitTimer = null }
  cleanup(true)
})
</script>

<style scoped>
.console {
  display: flex; height: 100%; padding: 14px; gap: 14px;
}

/* ===== 画面区 ===== */
.stage {
  flex: 1; display: flex; flex-direction: column; min-width: 0; min-height: 0;
  position: relative; overflow: hidden;
  border: 1px solid var(--border); border-radius: var(--radius); background: var(--bg-1);
  outline: none;
}
.stage.keyboard-active { outline: 2px solid var(--accent); outline-offset: -2px; }
.app-hint {
  position: absolute; top: 54px; left: 50%; transform: translateX(-50%); z-index: 6;
  display: flex; align-items: center; gap: 8px; padding: 5px 10px; white-space: nowrap;
  background: rgba(4, 6, 10, .85); border: 1px solid var(--border); border-radius: 8px;
  font-size: 12px; color: var(--text-1); backdrop-filter: blur(2px);
}

/* 二次裁切区 */
.crop-stage {
  display: flex; overflow: auto;
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  background: #000;
}
.crop-stage .crop-canvas { margin: auto; }
.crop-canvas {
  border-radius: var(--radius-sm);
  cursor: crosshair; background: #000; touch-action: none;
}
.crop-hint { font-size: 10px; color: var(--text-2); align-self: flex-start; }
.crop-panel {
  display: flex; flex-direction: column; gap: 10px;
  border-top: 1px solid var(--border); padding-top: 12px;
}
.crop-actions { display: flex; gap: 8px; }
.crop-actions .btn-primary { margin-left: auto; }

/* 工具条：设备管理与投屏控制合并为同一横向行，窄窗口时横向滚动 */
.toolbar {
  display: flex; align-items: center;
  flex: 0 0 auto; background: var(--bg-1); border-bottom: 1px solid var(--border);
  padding: 8px 10px;
  box-sizing: border-box;
}
.tb-row {
  display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0;
  min-height: 29px; overflow-x: auto; overflow-y: hidden; scrollbar-width: none;
}
.tb-row::-webkit-scrollbar { display: none; width: 0; height: 0; }
.tb-row > .btn, .tb-row > .select, .tb-more-wrap, .tb-sep { flex-shrink: 0; }
.tb-sep { width: 1px; height: 22px; background: var(--border); margin: 0 4px; }
.tb-more-wrap { position: relative; display: inline-flex; }
.keymap-select { flex: 0 1 150px; min-width: 104px; max-width: 180px; padding: 4px 6px; font-size: 12px; }
.keyboard-mode-btn.active { color: var(--accent-2); }
.tb-more-mask { position: fixed; inset: 0; z-index: 20; }
.tb-more-dropdown {
  display: flex; flex-direction: column; min-width: 168px; padding: 4px; gap: 2px;
  background: var(--bg-2); border: 1px solid var(--border); border-radius: var(--radius-sm);
  box-shadow: 0 8px 24px rgba(0, 0, 0, .4);
}
.tb-more-dropdown-fixed { position: fixed; z-index: 30; }
.tb-more-item {
  display: flex; align-items: center; gap: 6px; text-align: left; white-space: nowrap;
  padding: 6px 10px; border: none; background: none; border-radius: var(--radius-sm);
  color: var(--text-0); font-size: 12px; cursor: pointer;
}
.tb-more-item:hover { background: var(--bg-3); }
.btn.active { border-color: var(--accent-2); color: var(--accent-2); }

/* ===== 左右分区与右侧面板 ===== */
.console.is-panel-resizing,
.console.is-panel-resizing * { cursor: col-resize !important; user-select: none !important; }
.panel-resizer {
  position: relative; z-index: 5; flex: 0 0 8px; width: 8px; margin: 0 -11px;
  cursor: col-resize; touch-action: none; outline: none;
}
.panel-resizer::before {
  content: ''; position: absolute; inset: 0 3px; border-radius: 4px; background: transparent;
  transition: background .15s ease;
}
.panel-resizer:hover::before,
.panel-resizer:focus-visible::before,
.panel-resizer.active::before { background: var(--accent); }
.panel {
  width: 340px; flex-shrink: 0; display: flex; flex-direction: column; gap: 10px;
  overflow: hidden;
}

/* 工具条设备下拉（设备管理收进工具条后的宽度约束） */
.tb-dev-select { flex: 0 1 auto; min-width: 130px; max-width: 200px; padding: 4px 6px; font-size: 12px; }

.panel-sec {
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 14px; display: flex; flex-direction: column; gap: 10px;
  flex-shrink: 0;
}
.mono { font-family: var(--mono); font-size: 11px; color: var(--text-1); }

.auto-run { display: flex; flex-wrap: wrap; gap: 8px; }
.auto-run .spicker { flex: 1 1 auto; }
.auto-run .select { flex: 1; min-width: 120px; }
.run-actions { display: flex; gap: 8px; }
.run-actions .btn { flex: 1; }
.run-actions .more-wrap { position: relative; flex: 1; }
.run-actions .more-wrap .btn { width: 100%; }
.more-mask { position: fixed; inset: 0; z-index: 20; }
.more-dropdown {
  position: absolute; right: 0; top: calc(100% + 4px); z-index: 30;
  display: flex; flex-direction: column; min-width: 120px; padding: 4px; gap: 2px;
  background: var(--bg-2); border: 1px solid var(--border); border-radius: var(--radius-sm);
  box-shadow: 0 8px 24px rgba(0, 0, 0, .4);
}
.more-item {
  display: flex; align-items: center; gap: 6px; text-align: left; white-space: nowrap;
  padding: 6px 10px; border: none; background: none; border-radius: var(--radius-sm);
  color: var(--text-0); font-size: 12px; cursor: pointer;
}
.more-item:hover { background: var(--bg-3); }
.more-item:disabled { color: var(--text-2); opacity: .5; cursor: not-allowed; }
.more-item.danger:hover { color: var(--danger); }

/* 脚本页签 */
.panel-sec.script-tab { flex: 1; min-height: 0; overflow: hidden; }
.panel-sec.tpl-tab { flex: 1; min-height: 0; overflow: hidden; }
.panel-sec.extra-tab { flex: 1; min-height: 0; overflow: hidden; display: flex; flex-direction: column; }
.func-pkg-row { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
.func-pkg-row .func-pkg { flex: 1; min-width: 0; font-size: 12px; }
.func-pkg-row .btn { flex: none; }
.func-tabs { display: flex; flex-shrink: 0; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden; background: var(--bg-2); }
.func-tabs button {
  flex: 1; padding: 7px 0; font-size: 12px; text-align: center; cursor: pointer;
  border: none; background: transparent; color: var(--text-1);
}
.func-tabs button + button { border-left: 1px solid var(--border); }
.func-tabs button.active {
  color: var(--accent); background: rgba(34, 211, 165, .14); font-weight: 600;
}
/* 包名下拉：模板/脚本两页签共用，数据随当前包名切换 */
.script-tpl { flex: 4; min-height: 0; display: flex; flex-direction: column; gap: 8px; border-bottom: 1px solid var(--border); padding-bottom: 10px; }
.tpl-top { display: flex; align-items: center; gap: 8px; }
/* 阈值输入 : 区域下拉 : 搜索框 : 框选按钮 : 上传按钮 = 2:4:5:3:3 */
.tpl-top .input { flex: 2 1 0%; min-width: 0; }
.tpl-top .tpl-region { flex: 4 1 0%; min-width: 0; padding: 4px 6px; font-size: 11px; }
.tpl-top .tpl-search { flex: 5 1 0%; min-width: 0; font-size: 11px; }
.tpl-top .btn { flex: 3 1 0%; min-width: 0; }
.tpl-tools { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.script-run { flex: 6; display: flex; flex-direction: column; gap: 10px; min-height: 0; }
.script-logs { flex: 1; min-height: 120px; max-height: none; }
.run-hint { font-size: 11px; color: var(--text-2); flex-shrink: 0; }
.script-view-wrap { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 6px; }
.script-view {
  flex: 1; min-height: 0; overflow: auto; background: var(--bg-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 10px 12px; font-size: 12px; line-height: 1.65; color: #c9d4e8;
  user-select: none;
}
.sv-line { white-space: pre; border-radius: 4px; padding: 0 6px; margin: 0 -6px; }
.sv-line.selectable { cursor: pointer; }
.sv-line.selectable:hover { background: var(--bg-3); }
.sv-line.sel {
  background: rgba(34,211,165,.12); color: var(--accent);
  box-shadow: inset 2px 0 0 var(--accent);
}
/* call 子脚本名链接：悬停下划线，点击弹窗预览（脚本视图 user-select:none，需单独放开） */
.call-link { color: var(--accent-2); cursor: pointer; }
.call-link:hover { text-decoration: underline; }

/* call 子脚本预览弹窗（modal-mask/.modal/.modal-head/.modal-body 为全局样式） */
.preview-modal { min-width: 520px; width: 520px; }
.preview-code {
  background: var(--bg-0); border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 10px 12px; font-size: 12px; line-height: 1.65; color: #c9d4e8;
  overflow: auto; max-height: 60vh; white-space: pre; margin: 0;
}
.script-view-empty {
  flex: 1; display: flex; align-items: center; justify-content: center;
  color: var(--text-2); font-size: 12px; background: var(--bg-0);
  border: 1px dashed var(--border); border-radius: var(--radius-sm);
}
.script-edit { flex: 6; display: flex; flex-direction: column; gap: 10px; min-height: 0; }
.edit-name-row { display: flex; }
.edit-name-row .input { flex: 1; min-width: 0; width: 100%; }
.edit-actions { display: flex; gap: 8px; }
.edit-actions .btn { flex: 1; justify-content: center; }
.edit-actions .btn.active { border-color: var(--accent-2); color: var(--accent-2); background: rgba(56,189,248,.08); }
.script-editor {
  flex: 1; min-height: 160px; resize: none; background: var(--bg-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  color: #c9d4e8; font-size: 12px; line-height: 1.65; padding: 12px;
  font-family: var(--mono); outline: none;
}

.run-progress { display: flex; flex-direction: column; gap: 6px; }
.rp-head { display: flex; justify-content: space-between; font-size: 12px; }
.rp-script { color: var(--accent); }
.rp-pct { color: var(--text-1); }
.rp-bar { height: 5px; background: var(--bg-3); border-radius: 3px; overflow: hidden; }
.rp-fill { height: 100%; background: linear-gradient(90deg, var(--accent), var(--accent-2)); border-radius: 3px; transition: width .4s; }
.rp-step { font-size: 11px; color: var(--text-1); }

.live-logs {
  max-height: 180px; overflow: auto; background: var(--bg-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 8px; display: flex; flex-direction: column; gap: 3px;
}
.live-logs.script-logs { max-height: none; }
.ll { display: flex; gap: 8px; font-size: 11px; line-height: 1.5; }
.ll-time { color: var(--text-2); flex-shrink: 0; }
.ll.info .ll-msg { color: var(--text-1); }
.ll.success .ll-msg { color: var(--ok); }
.ll.warn .ll-msg { color: var(--warn); }
.ll.error .ll-msg { color: var(--danger); }

/* 模板文件列表 */
.tpl-list-wrap { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 4px; }
.tpl-list-head, .tpl-row { display: flex; align-items: center; gap: 8px; padding: 3px 8px; }
.tpl-list-head { font-size: 11px; color: var(--text-2); border-bottom: 1px solid var(--border); flex-shrink: 0; }
.tpl-list { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 2px; min-height: 0; }
.tpl-row {
  cursor: pointer; border-radius: var(--radius-sm); border: 1px solid transparent;
  transition: background .15s;
}
.tpl-row:hover { background: var(--bg-3); }
.tpl-row.del-confirm { background: rgba(248,113,113,.08); border-color: rgba(248,113,113,.35); }
.tpl-row.renaming { background: rgba(56,189,248,.08); border-color: rgba(56,189,248,.35); }
.tpl-empty { padding: 16px 8px; text-align: center; font-size: 11px; color: var(--text-2); }
.tpl-cell.thumb { width: 40px; flex-shrink: 0; display: flex; align-items: center; }
.tpl-list-head .tpl-cell.thumb { white-space: nowrap; }
.tpl-cell.name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; color: var(--text-0); }
.tpl-cell.ops { display: flex; gap: 6px; flex-shrink: 0; }
.tpl-cell.ops .btn { padding: 2px 8px; font-size: 11px; }
.rename-input { width: 100%; min-width: 0; padding: 2px 6px; font-size: 12px; }
.tpl-region-badge {
  display: inline-block; margin-left: 6px; padding: 0 5px; border-radius: 4px;
  background: var(--bg-3); border: 1px solid var(--border);
  color: var(--accent); font-size: 10px; line-height: 16px; vertical-align: 1px;
  cursor: help; user-select: none;
}
.tpl-thumb { display: inline-flex; }
.tpl-thumb img {
  width: 24px; height: 24px; object-fit: contain;
}
.tpl-del-confirm {
  background: var(--danger); border-color: var(--danger); color: #fff;
}
.tpl-del-confirm:hover { background: #ef4444; color: #fff; }

/* 模板查看大图 */
.tpl-view-mask {
  position: fixed; inset: 0; z-index: 100; display: flex; align-items: center; justify-content: center;
  background: rgba(8,10,16,.78); backdrop-filter: blur(2px);
}
.tpl-view-modal {
  position: relative; display: flex; flex-direction: column; gap: 8px;
  max-width: 92vw; max-height: 92vh;
}
.tpl-view-img { position: relative; align-self: center; }
.tpl-view-img img {
  display: block; max-width: 92vw; max-height: 82vh; object-fit: contain;
  border-radius: var(--radius-sm); border: 1px solid var(--border); background: #000;
}
.tpl-view-close {
  position: absolute; top: 8px; right: 8px; width: 28px; height: 28px;
  display: flex; align-items: center; justify-content: center;
  background: var(--bg-2); border: 1px solid var(--border); border-radius: 50%;
  color: var(--text-1); cursor: pointer; font-size: 13px; z-index: 1;
}
.tpl-view-close:hover { color: var(--danger); border-color: var(--danger); }
.tpl-view-name { text-align: center; font-size: 12px; color: var(--text-1); word-break: break-all; }

/* 二次裁切占满整个模板区域 */
.crop-panel-full { flex: 1; min-height: 0; border-top: none; padding-top: 0; }
.crop-panel-full .crop-stage { flex: 1; min-height: 0; min-width: 0; }
</style>
