<template>
  <div class="console" :class="{ 'sb-collapsed': sidebarCollapsed }">
    <!-- 左：画面区 -->
    <div class="stage">
      <!-- 顶部工具条：两行布局——上行设备管理（删除归设备组），下行投屏控制 -->
      <div class="toolbar">
        <div class="tb-row tb-row-dev">
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
        </div>
        <div class="tb-row tb-row-ctrl">
          <button class="btn btn-sm" @click="shot">📷 截图</button>
          <button class="btn btn-sm" @click="rotate">🔄 旋转</button>
          <button class="btn btn-sm" @click="key('HOME')">🏠 Home</button>
          <button class="btn btn-sm" @click="key('BACK')">⬅ 返回</button>
          <button class="btn btn-sm" @click="key('APP_SWITCH')">🪟 最近</button>
          <button class="btn btn-sm" @click="key('VOL_UP')">🔊＋</button>
          <button class="btn btn-sm" @click="key('VOL_DOWN')">🔊－</button>
          <button class="btn btn-sm" @click="toggleAudio" :title="audioMuted ? '取消静音（听游戏声音）' : '静音'">{{ audioMuted ? '🔇' : '🔊' }}</button>
          <button class="btn btn-sm" @click="launchGame" :title="'启动到虚拟屏：' + (currentPkg || '未配置应用')">🚀 启动应用</button>
          <button
            class="btn btn-sm"
            :class="{ active: recording.phase !== 'idle', 'rec-btn': recording.phase !== 'idle' }"
            :disabled="recording.phase === 'stopping' || (recording.phase === 'idle' && !recording.available)"
            :title="recording.buttonTitle"
            data-test="recording-toggle"
            @click="recording.toggle()"
          >{{ recording.phase === 'idle' ? '⏺ 录制' : '■ 停止录制' }}</button>
          <div class="tb-sep"></div>
          <button class="btn btn-sm" @click="clipboard">📋 剪贴板</button>
        </div>
      </div>

      <!-- 录制状态栏（plan §10.1/§11.8）：状态 · 录制目标 · 待处理数量 · 停止按钮 -->
      <div v-if="recording.phase !== 'idle'" class="rec-bar" data-test="recording-bar">
        <span class="rec-dot" :class="{ stopping: recording.phase === 'stopping' }"></span>
        <strong>{{ recording.phase === 'recording' ? '录制中' : '停止中…' }}</strong>
        <span class="mono">录制到：{{ recording.targetLabel }}</span>
        <span class="mono">待处理 {{ recording.pendingCount }}</span>
        <span v-if="recording.failedCount" class="rec-bar-err">失败 {{ recording.failedCount }}</span>
        <button class="btn btn-sm" :disabled="recording.phase === 'stopping'" data-test="recording-stop" @click="recording.stop()">■ 停止</button>
      </div>

      <ConsoleVideoStage
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
        :fx-tap-style="fxTapStyle"
        :fx-swipe-style="fxSwipeStyle"
        :fx-hit-style="fxHitStyle"
        :loupe="loupe"
        :on-mouse-down="onMouseDown"
        :on-mouse-move="onMouseMove"
        :on-mouse-up="onMouseUp"
        :on-wheel="onWheel"
        :on-video-mouse-leave="onVideoMouseLeave"
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

    <!-- 右：功能区（顶部 模板/脚本 页签切换整块内容；二次裁切为弹窗，见 TemplateCapture） -->
    <aside class="panel">
      <RecordingCropPanel v-if="recording.panelDraft" :context="recordingCropContext" />
      <select v-model="activePkg" class="select mono func-pkg" title="应用分区：脚本/函数库/模板按应用包名分区存储（两页签共用）">
        <option v-if="!pkgOptions.length" value="">（未配置应用包名）</option>
        <option v-for="p in pkgOptions" :key="p" :value="p">{{ p }}</option>
      </select>
      <div class="func-tabs">
        <button type="button" :class="{ active: panelTab === 'tpl' }" @click="panelTab = 'tpl'">🖼️ 模板</button>
        <button type="button" :class="{ active: panelTab === 'script' }" @click="panelTab = 'script'">📜 脚本</button>
      </div>
      <div v-show="panelTab === 'tpl'" class="panel-sec tpl-tab">
        <TemplateCapture :context="templateCaptureContext" :on-crop-mounted="onCropMounted" />
      </div>
      <div v-show="panelTab === 'script'" class="panel-sec script-tab">
        <ScriptRunner :context="scriptRunnerContext" />
      </div>
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

<script>
// 应用列表缓存：设备/地址 -> { list, ts }，应用列表不常变，避免每次重复读取
const appCache = new Map()
const APP_CACHE_TTL = 5 * 60 * 1000
</script>

<script setup>
import { ref, reactive, computed, watch, nextTick, onMounted, onUnmounted, inject, provide } from 'vue'
import { pinyin } from 'pinyin-pro'
import { useRouter } from 'vue-router'
import { store, devicesData, scriptsData, templatesData, useToast, applyRunRecord, findRun, beginCancel, resetStoreRunState, pushRunConflict, appStartedDevices } from '../store'
import { api, runPartitionImport } from '../api'
import {
  sourceLabel, terminalLabel,
  isDeviceBusyConflict, isTerminalRunState,
} from '../runs'
import ConsoleVideoStage from '../components/console/ConsoleVideoStage.vue'
import DeviceSettingsModal from '../components/console/DeviceSettingsModal.vue'
import TemplateCapture from '../components/console/TemplateCapture.vue'
import ScriptRunner from '../components/console/ScriptRunner.vue'
import RecordingCropPanel from '../components/console/RecordingCropPanel.vue'
import RunConflictModal from '../components/RunConflictModal.vue'
import RunParamsModal from '../components/RunParamsModal.vue'
import { createEditorShellApi, createRecordingApi } from '../components/console/current-api-adapters'
import { useConsoleRuntime } from '../composables/useConsoleRuntime'
import { useWebRtcLifecycle } from '../composables/useWebRtcLifecycle'
import { useScriptEditorShell } from '../composables/useScriptEditorShell'
import { useFunctionLibrary } from '../composables/useFunctionLibrary'
import { useRunArgsFlow } from '../composables/useRunArgsFlow'
import { useRecording } from '../composables/useRecording'
import { parseScript, parseFunctionLibrary } from '../script-editor/codec'
import { SE_TARGET_OPTIONS } from '../script-editor/targets'
import { startIndexOf } from '../script-editor/selection'
import {
  defaultTemplateName,
  deviceRectStyle as mapDeviceRectStyle,
  selectionToDeviceRect,
  toDeviceCoord as mapToDeviceCoord,
} from '../console/geometry'
import { formatScreenSummary } from '../console/device-summary'

const router = useRouter()
const toast = useToast()

// 侧边栏收起状态（MainLayout provide）：收起时释放的宽度让给右侧操作区，投屏区保持不变
const sidebarCollapsed = inject('sidebarCollapsed', ref(false))
const superseded = ref(false)
const manualClose = ref(false)
const connected = ref(false)
const connecting = ref(false)
const errorMsg = ref('')
const fps = ref(0)
const delay = ref(0)
let delaySpikes = 0
const res = ref('—')
const bitrate = ref('—')
const audioMuted = ref(true)
const videoWrap = ref(null)
const videoElement = ref(null)

function onVideoMounted(el) { videoElement.value = el }
function onVideoWrapMounted(el) { videoWrap.value = el }
function onLoupeMounted(el) { loupeCanvas.value = el }

const consoleRuntime = useConsoleRuntime({
  api,
  devicesData,
  scriptsData,
  templatesData,
  toast,
  deviceIdRef: computed(() => store.deviceId),
})

let statsTimer = null
let logTimer = null
let connectLock = false
let forceTakeover = false
let ws = null
let pc = null
let controlChannel = null
let mediaStream = null
let hadVideo = false
let videoBytesAdvanced = false
let lastVideoTime = 0
let stillFrames = 0
let lastBytesReceived = 0
let lastBitrateTs = 0
let lastJbd = 0
let lastJbe = 0
let videoConnectTs = 0
let lastPliCount = 0
let lastPliResetAt = 0
let pliResetStreak = 0
let renderFpLast = ''
let renderFpFrozen = 0
let lastDragInputAt = 0
let stallResetSent = false
let blackResetSent = false
// 未启动应用提示（app-hint）：连接时出现、显示几秒自动消失（纯提示无按钮）。
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
let fpCanvas = null
let fpCtx = null
const selScript = ref('')
// 脚本页签：运行/编辑模式
// 脚本页签当前应用分区（= 应用包名）：默认/自动跟随设备页签配置的 pkg，可手动切换；
// 模板列表、脚本选择、模板/脚本读写都按该分区进行（后端 data/<pkg>/tmpl|yaml）
const activePkg = ref('')
const scriptMode = ref('run')
// 右侧功能区页签：模板 / 脚本（整块切换，各自独占面板高度）
const panelTab = ref('script')
// ---------- 共享脚本编辑器外壳（阶段 4） ----------
// 模型/命令栈/dirty/保存/409 冲突/校验/跳转全部收敛在 useScriptEditorShell，
// Console 与独立脚本页共用同一编辑核心（script-editor/*）。
// resolvers 提供模板存在性校验（call/func 资源与 args 绑定检查需要目标参数表，客户端暂缺、由服务端权威校验）
const scriptShell = useScriptEditorShell({
  api: createEditorShellApi(api),
  getContext: () => ({
    resolveTemplate: (n) => {
      const list = templatesData.value.filter(t => t.pkg === activePkg.value)
      return list.some(t => t.name === n || tplShortName(t.name) === n)
    },
  }),
})
// 函数库列表与 func 目标解析（func 步骤「打开函数定义」跳转用）
const fnLib = useFunctionLibrary({ api })
// 运行区资源类型：脚本（yaml/）/ 函数（func/）。函数模式经函数测试接口运行单个函数
const runKind = ref('script')
const selFnFile = ref('')
/** 运行按钮可用性：脚本模式看脚本选择，函数模式看函数库文件选择 */
const canRunTarget = computed(() => (runKind.value === 'script' ? !!selScript.value : !!selFnFile.value))
// 函数文件默认选中第一个：切类型/切分区/列表刷新后当前选择失效时回退第一个（与脚本下拉同形）
watch([() => fnLib.list, runKind], () => {
  if (runKind.value !== 'func') return
  if (!fnLib.list.some(f => f.id === selFnFile.value)) selFnFile.value = fnLib.list[0]?.id || ''
}, { immediate: true })
/** 运行区当前选择 id（脚本 id / 函数库文件 id）：编辑、删除按钮与摘要区共用 */
const selTargetId = computed(() => (runKind.value === 'script' ? selScript.value : selFnFile.value))
/**
 * 函数模式：整个函数库文件的解析模型（全部函数）。摘要区逐函数分组渲染
 * （每组一个 ScriptSummary，steps 带稳定 uuid 供运行起点定位）。
 * parseFunctionFile 返回 {model, diagnostics} 包装。
 */
const funcParsed = computed(() => {
  if (runKind.value !== 'func') return null
  const f = fnLib.list.find(x => x.id === selFnFile.value)
  if (!f) return null
  try {
    const parsed = fnLib.parseFunctionFile(f.content ?? '', f.file || '')
    const model = parsed && parsed.model
    return model && Array.isArray(model.functions) ? model : null
  } catch {
    return null
  }
})
/** 每个函数一个伪脚本模型（params + steps），复用 ScriptSummary 顶层卡片交互 */
const funcFnViews = computed(() => {
  const model = funcParsed.value
  if (!model) return []
  return model.functions.map(fn => ({ name: fn.name, model: { params: fn.params || [], steps: fn.steps || [] } }))
})
const funcSummaryError = computed(() => {
  if (runKind.value !== 'func') return ''
  const f = fnLib.list.find(x => x.id === selFnFile.value)
  if (!f) return '请选择函数库文件'
  return funcParsed.value ? '' : '函数库解析失败（可能含旧语法），请进编辑态查看诊断'
})
// 编辑态辅助 UI 开关
const showYaml = ref(false)
// 模板短名候选（画布 tmpl 控件 datalist）
const templateNames = computed(() =>
  templatesData.value.filter(t => t.pkg === activePkg.value).map(t => tplShortName(t.name)))
watch(activePkg, pkg => { fnLib.refresh(pkg) })

// ---------- call/func 目标候选与参数解析（编辑态画布下拉 + args 自动生成，同独立脚本页） ----------
// func 参数直接从 fnLib.list 的文件内容解析（按内容版本 memo）；call 拉脚本内容按 id 缓存。

const callScriptOptions = computed(() => {
  if (!activePkg.value) return []
  return scriptsData.value
    .filter(s => s.package === activePkg.value && !(s.id === scriptShell.resourceId && scriptShell.kind === 'script'))
    .map(s => ({ target: s.name, label: String(s.name || '').replace(/\.(ya?ml)$/i, '') }))
})

/** func 候选文件：正在编辑的函数库文件以画布实时模型为准（未保存的新增/改名函数立即可选），
 *  其余文件用磁盘顶层函数名清单（fnLib 列表快照）。 */
const funcFileOptions = computed(() => {
  const live = scriptShell.kind === 'function_library' && scriptShell.hasModel && Array.isArray(scriptShell.model.functions)
    ? scriptShell.model.functions.map(f => f.name)
    : null
  return fnLib.list.map(f => ({
    file: f.file,
    functions: live && f.id === scriptShell.resourceId ? live : (Array.isArray(f.functions) ? f.functions : []),
  }))
})

const callParamsCache = new Map() // script id → ParamDecl[] | null
const fnParamsMemo = new Map() // `<file>@<内容版本>` → Map(函数名 → ParamDecl[])

function funcParamsFor(target) {
  const s = String(target || '')
  const i = s.indexOf('/')
  if (i <= 0 || i === s.length - 1) return null
  const file = s.slice(0, i)
  const fn = s.slice(i + 1)
  const entry = fnLib.list.find(f => f.file === file)
  if (!entry || !entry.content) return null
  const memoKey = `${file}@${entry.version || entry.content.length}`
  let byName = fnParamsMemo.get(memoKey)
  if (!byName) {
    const parsed = parseFunctionLibrary(entry.content, { file })
    byName = new Map((parsed.model?.functions || []).map(f => [f.name, f.params || []]))
    fnParamsMemo.set(memoKey, byName)
  }
  return byName.has(fn) ? byName.get(fn) : null
}

function resolveTargetParamsSync(kind, target) {
  if (!target) return null
  if (kind === 'func') return funcParamsFor(target)
  return callParamsCache.get(target) ?? null
}

async function resolveTargetParams(kind, target) {
  if (!target) return null
  if (kind === 'func') return funcParamsFor(target)
  if (callParamsCache.has(target)) return callParamsCache.get(target)
  const script = scriptsData.value.find(x => x.package === activePkg.value && x.name === target)
  if (!script) return null
  try {
    const full = await api.getScript(script.id)
    const parsed = parseScript(full.content ?? '')
    const params = parsed.model?.params || []
    callParamsCache.set(script.id, params)
    return params
  } catch {
    return null
  }
}

function clearCallParamsCache() {
  callParamsCache.clear()
}
function resolveTargetSync(kind, target) {
  const params = resolveTargetParamsSync(kind, target)
  return params ? { params } : null
}

provide(SE_TARGET_OPTIONS, reactive({
  callScripts: callScriptOptions,
  funcFiles: funcFileOptions,
  resolveParams: resolveTargetParams,
  resolveParamsSync: resolveTargetParamsSync,
}))

// ---------- 录制接线（阶段 6，plan §11）：核心服务在 web/src/recording/，本层只做接入 ----------
// 指针坐标换算（帧原始尺寸基准）、触控发送（move 走 rAF 合并）、冻结帧注入 useRecording；
// 占位插入/最终替换经 shell 命令栈事务，草稿队列/裁切/上传/命名收敛在 composable。
function recordingClientToDevice(clientX, clientY) {
  const video = videoElement.value
  const wrap = videoWrap.value
  if (!video?.videoWidth || !wrap) return null
  const rect = wrap.getBoundingClientRect()
  const vw = video.videoWidth
  const vh = video.videoHeight
  const ratio = Math.min(rect.width / vw, rect.height / vh)
  const offX = (rect.width - vw * ratio) / 2
  const offY = (rect.height - vh * ratio) / 2
  // 画面 letterbox 区域之外不透传（多指第二指在面板上滑动不应触达设备）
  if (clientX < rect.left + offX || clientX > rect.right - offX) return null
  if (clientY < rect.top + offY || clientY > rect.bottom - offY) return null
  return toDeviceCoord(clientX, clientY)
}

const recording = useRecording({
  shell: scriptShell,
  activePkg,
  connected,
  scriptRunning: computed(() => store.running),
  videoElement,
  templatesData,
  api: createRecordingApi(api),
  notify: toast,
  sendControl,
  sendTouchMove: (x, y) => scheduleMove(x, y),
  clientToDevice: recordingClientToDevice,
})
// 录制中分区锁定：busy（录制/停止中/有未完成草稿）时禁止切换应用分区
watch(activePkg, (v, old) => {
  if (recording.busy && old && v !== recording.lockPkg) {
    activePkg.value = old
    toast('录制中不能切换应用分区', 'warn')
  }
})
// 二次裁切录制草稿面板上下文（getter 保响应式）
const recordingCropContext = {
  get draft() { return recording.panelDraft },
  confirm: (item, payload) => recording.confirmCrop(item, payload),
  downgrade: item => recording.downgrade(item),
  discard: item => recording.discard(item),
  retry: item => recording.retry(item),
  shortNameTaken: name => recording.shortNameTaken(name),
  defaultNameFor: (kind, suggested) => recording.nextShortName(kind, suggested),
  computeSearchRect: (item, rect, adjusted) => recording.computeSearchRect(item, rect, adjusted),
}

// 脚本运行可视化效果：服务端经 control DataChannel 推送 tap/swipe/hit/miss 事件（设备像素坐标），
// 与手动 alt 反馈状态独立（脚本运行时用户仍可手动操作，两类效果互不覆盖）
const scriptFx = reactive({
  tap: { show: false, x: 0, y: 0 },
  swipe: { show: false, x: 0, y: 0, w: 0, h: 0 },
  hit: { show: false, x: 0, y: 0, w: 0, h: 0, label: '', miss: false },
})
let fxTapTimer = null
let fxSwipeTimer = null
let fxHitTimer = null
// 日志原始数据（未过滤），用于按级别切换显示
let rawLogs = []
// 本次运行开始时间：清空日志区后只显示本次运行产生的日志
let runStartTime = 0
const picking = ref(false)
const testThreshold = ref(0.8)
// 模板匹配区域：'' = 默认（按模板名自动识别），否则 a/u/d/l/r/ul/ur/dl/dr（测试匹配与生成记录共用）
const testRegion = ref('')
// 模板列表：查看大图 / 删除二次确认 / 重命名
const viewTpl = ref(null)
const confirmDelTpl = ref(null)
const renaming = ref(null)   // 正在重命名的模板名（null=不在重命名）
const renameVal = ref('')    // 重命名输入框内容
let renameInputEl = null     // 重命名输入框元素（自动聚焦/全选）
const selecting = ref(false)
const selStart = reactive({ x: 0, y: 0 })
const selEnd = reactive({ x: 0, y: 0 })
const showHit = ref(false)
const hit = reactive({ x: 0, y: 0, w: 0, h: 0 })
const hitLabel = ref('')
// true = 展示的是未命中的搜索区域框（虚线红），false = 命中框（实线绿）
const hitMiss = ref(false)
let hitTimer = null
const liveLogs = ref([])
const logBox = ref(null)
// 二次裁切（右侧面板）
const crop = reactive({ active: false, imgW: 0, imgH: 0, baseW: 0, baseH: 0, originX: 0, originY: 0, rect: { x: 0, y: 0, w: 0, h: 0 }, preview: '', name: '', zoom: 1 })
const cropCanvas = ref(null)
const cropSec = ref(null)
// 二次裁切底图：框选时冻结的初始画面，拖动时只动遮罩框
let cropBaseCanvas = null
const cropDrag = reactive({ mode: null, startX: 0, startY: 0, orig: null })
const saving = ref(false)
// 放大预览镜
const loupe = reactive({ show: false, x: 0, y: 0, zoom: 2.5 })
const loupeCanvas = ref(null)

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
  },
  onDisconnect() {
    stopStats()
    stopLogPolling()
    connected.value = false
    hadVideo = false
    stillFrames = 0
    renderFpLast = ''
    renderFpFrozen = 0
    stallResetSent = false
    blackResetSent = false
    lastBytesReceived = 0
    lastBitrateTs = 0
    bitrate.value = '—'
    if (videoElement.value) videoElement.value.srcObject = null
    hideLoupe()
    // viewer 断开/被接管：取消录制手势并停止录制（草稿队列继续处理，plan §16.4）
    recording.onLinkLost()
  },
  onChannelOpen() {
    connected.value = true
    connecting.value = false
    controlChannel = webrtcLifecycle.getControlChannel()
    videoConnectTs = Date.now()
    blackResetSent = false
    // 该设备本会话已启动过应用（手动/脚本拉起/脚本仍在运行）→ 重连不再弹提示
    appHintDismissed.value = store.running
      || (store.deviceId ? appStartedDevices.has(store.deviceId) : false)
    sendControl({ type: 'audio', on: !audioMuted.value })
    toast('WebRTC 连接建立', 'success')
  },
  onChannelClose() {
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
  onSignalMessage({ type, message, channel }) {
    if (type === 'signal' && message?.type === 'taken_over') {
      superseded.value = true
      toast('连接已被其他页面接管', 'warn')
    }
  },
  onPeerDisposed() {
    controlChannel = null
    mediaStream = null
  },
})

function scheduleReconnect() {
  webrtcLifecycle.scheduleReconnect({ superseded })
}

function onChannelOpen() {}
function onChannelClose() {}

// 设备设置弹窗开关（新增/编辑共用一个弹窗，由 mode 区分）
const settingsOpen = ref(false)

// ---------- 设备管理（工具条设备控件 + 设置弹窗；原右侧设备页签已收编） ----------
const vdPresets = [
  { res: '1920x1080', dpi: 420 },
  { res: '1080x1920', dpi: 420 },
  { res: '1280x720', dpi: 320 },
  { res: '2340x1080', dpi: 440 }
]
// 帧率仅提供具体数值（无"自动"选项），默认 30
const fpsPresets = [15, 30, 60, 120]
const types = [
  { key: 'redroid', label: 'redroid 容器', icon: '🐳' },
  { key: 'usb', label: 'USB 直连', icon: '🔌' },
  { key: 'wifi', label: '无线 adb', icon: '📶' },
  { key: 'emu', label: '模拟器', icon: '🖥️' }
]
// 表单状态：'edit' 编辑现有设备 / 'add' 手动新增（均在设置弹窗内完成）
// 默认配置：分辨率 1920x1080 · 帧率 30 · DPI 自动（0）
const mode = ref('edit')
const form = reactive({ name: '', kind: 'redroid', addr: '', screen_mode: 'virtual', vd_res: '1920x1080', vd_dpi: 0, pkg: '', fps: 30 })
const scanning = consoleRuntime.scanning
// 配置保存进行中标志：防止重复提交
const configApplying = ref(false)

// 应用下拉（应用选择）
const appList = ref([])
const pkgDraft = ref('')
const appLoading = ref(false)
const appOpen = ref(false)
const appHint = ref('')

const devices = computed(() => devicesData.value)
const scripts = computed(() => scriptsData.value)
// 模板模糊搜索词（短名/带 #后缀 全名均可命中）
const tplSearch = ref('')
// 模板名拼音首字母缓存（非汉字字符原样保留）：「日常遗器.png」→ "rcyq.png"，供搜索匹配
const tplPyCache = new Map()
function tplPinyinInitials(name) {
  let s = tplPyCache.get(name)
  if (s === undefined) {
    s = pinyin(name, { pattern: 'first', toneType: 'none', type: 'array' })
      .join('').replace(/\s+/g, '').toLowerCase()
    tplPyCache.set(name, s)
  }
  return s
}

// 模板列表：当前应用分区过滤（templatesData 为跨分区全量，条目带 pkg 字段）；
// 有搜索词时三口径并列匹配（全名/短名子串 + 中文名拼音首字母），任一命中即展示，
// 排序按最早命中位置（拼音命中加偏移恒排文字命中之后），同级按修改时间倒序；无搜索词按修改时间倒序
const templates = computed(() => {
  let list = templatesData.value.filter(t => t.pkg === activePkg.value)
  const q = tplSearch.value.trim().toLowerCase()
  if (q) {
    // 首字母串不含中文，查询词含中文时跳过该口径（必然无交集）
    const pyAble = !/[\u4e00-\u9fff]/.test(q)
    const PY_OFFSET = 1e4
    list = list.map(t => {
      let idx = t.name.toLowerCase().indexOf(q)
      const si = tplShortName(t.name).toLowerCase().indexOf(q)
      if (idx === -1 || (si !== -1 && si < idx)) idx = si
      if (idx === -1 && pyAble) {
        const pi = tplPinyinInitials(t.name).indexOf(q)
        if (pi !== -1) idx = PY_OFFSET + pi
      }
      return idx === -1 ? null : { t, idx }
    }).filter(Boolean).sort((a, b) => a.idx - b.idx || (b.t.mtime || 0) - (a.t.mtime || 0)).map(x => x.t)
  } else {
    list = list.sort((a, b) => (b.mtime || 0) - (a.mtime || 0))
  }
  return list
})

/** 应用分区下拉选项：设备页签配置的包名 ∪ 脚本分区 ∪ 模板分区（字典序） */
const pkgOptions = computed(() => {
  const set = new Set()
  const dp = (form.pkg || '').trim()
  if (dp) set.add(dp)
  for (const s of scripts.value) if (s.package) set.add(s.package)
  for (const t of templatesData.value) if (t.pkg) set.add(t.pkg)
  return [...set].sort((a, b) => a.localeCompare(b))
})

// 设备页签应用包名变化（含未保存草稿、切换设备）→ 分区自动跟随；
// 清空包名时保持当前分区（磁盘分区仍在），仅从未选择时兜底选第一个分区
watch(() => form.pkg, v => {
  const t = (v || '').trim()
  if (t) activePkg.value = t
  else if (!activePkg.value) activePkg.value = pkgOptions.value[0] || ''
})
watch(pkgOptions, list => {
  if (!activePkg.value) activePkg.value = list[0] || ''
  else if (!list.includes(activePkg.value)) activePkg.value = list[0] || ''
})

const current = computed(() => devices.value.find(d => d.id === store.deviceId) || null)
const currentName = computed(() => current.value?.name || '未选择设备')
const currentPkg = computed(() => current.value?.pkg || '')

/** 接入方式展示（新增时可选，编辑时只读徽章） */
function kindInfo(k) {
  return types.find(t => t.key === k) || { key: k, label: k || '未知', icon: '📱' }
}

/** 编辑模式概览里的屏幕摘要（与配置表单区分开，避免重复） */
const screenSummary = computed(() => {
  return formatScreenSummary(current.value)
})

const appFiltered = computed(() => {
  const q = (pkgDraft.value || '').trim().toLowerCase()
  return appList.value
    .filter(a => !q || a.label.toLowerCase().includes(q) || a.pkg.toLowerCase().includes(q))
    .slice(0, 50)
})

/** 当前应用列表缓存 key（编辑按设备 id，新增按 ADB 地址） */
function appCacheKey() {
  return mode.value === 'edit' && store.deviceId ? `device:${store.deviceId}` : `addr:${form.addr.trim()}`
}

/** 从缓存恢复应用列表（切换设备时避免重新读取） */
function restoreAppCache(id) {
  const cached = appCache.get(`device:${id}`)
  appList.value = cached?.list || []
  appOpen.value = false
  appHint.value = cached ? `已缓存 ${cached.list.length} 个应用` : ''
}

/** 把设备记录载入表单（编辑模式） */
function loadForm(d) {
  mode.value = 'edit'
  form.name = d.name || ''
  form.kind = d.kind || 'redroid'
  form.addr = d.addr || ''
  form.screen_mode = d.screen_mode || 'virtual'
  form.vd_res = d.vd_res || '1920x1080'
  form.vd_dpi = d.vd_dpi || 0
  form.pkg = d.pkg || ''
  pkgDraft.value = d.pkg || ''
  form.fps = d.fps || 30
  restoreAppCache(d.id)
}

/** 表单相对已保存配置是否有未保存修改 */
const formDirty = computed(() => {
  const d = current.value
  if (!d || mode.value !== 'edit') return false
  const norm = (v, fb) => (v === '' || v === null || v === undefined ? fb : v)
  // 接入方式 / ADB 地址是新增时确定的连接属性，编辑时只读，不参与 dirty 判断
  return !(
    d.name === norm(form.name, '') &&
    (d.screen_mode || 'virtual') === norm(form.screen_mode, 'virtual') &&
    (d.vd_res || '1920x1080') === norm(form.vd_res, '1920x1080') &&
    Number(d.vd_dpi || 0) === Number(norm(form.vd_dpi, 0)) &&
    (d.pkg || '') === norm(form.pkg, '') &&
    (d.fps || 30) === Number(norm(form.fps, 30))
  )
})

/** 手动新增：重置为默认配置（1920x1080 / 30fps / DPI 自动）并打开设置弹窗 */
function startAdd() {
  mode.value = 'add'
  form.name = ''
  form.kind = 'redroid'
  form.addr = ''
  form.screen_mode = 'virtual'
  form.vd_res = '1920x1080'
  form.vd_dpi = 0
  form.pkg = ''
  pkgDraft.value = ''
  form.fps = 30
  appList.value = []
  appOpen.value = false
  appHint.value = ''
  errorMsg.value = ''
  settingsOpen.value = true
}

/** 打开设备设置弹窗（编辑当前选中设备） */
function openSettings() {
  const d = current.value
  if (!d) return
  loadForm(d)
  settingsOpen.value = true
}

/** 关闭设置弹窗：丢弃未保存修改，恢复当前设备的已保存配置 */
function cancelSettings() {
  settingsOpen.value = false
  const d = current.value
  if (d) loadForm(d)
  else {
    mode.value = 'edit'
    store.deviceId = null
    pkgDraft.value = ''
    appList.value = []
    appHint.value = ''
  }
  errorMsg.value = ''
}

/** 下拉框切换设备：手动断开旧连接（不自动重连），等待用户点连接 */
function onDeviceSelect() {
  if (connected.value || consoleRuntime.reconnectTimer.value) {
    consoleRuntime.cancelReconnect()
    cleanup(true)
  }
  reconnectAttempts = 0
  errorMsg.value = ''
  const d = current.value
  if (d) loadForm(d)
  else { mode.value = 'edit'; pkgDraft.value = ''; appList.value = []; appHint.value = '' }
}

/** 刷新：扫描 adb 自动入库新设备，再拉列表 */
async function refreshDevices() {
  if (scanning.value) return
  scanning.value = true
  try {
    const r = await api.scanDevices()
    const list = r.devices && Array.isArray(r.devices) ? r.devices : await api.listDevices()
    devicesData.value = list
    // 当前设备已不存在（被删）→ 选中第一台
    if (!list.some(x => x.id === store.deviceId)) {
      store.deviceId = list[0]?.id || null
    }
    const d = current.value
    // 仅编辑模式重新载入表单（不覆盖进行中的"新增"表单）
    if (d && mode.value === 'edit') loadForm(d)
    else if (!d) { mode.value = 'edit'; pkgDraft.value = ''; appList.value = []; appHint.value = '' }
    toast(r.added > 0 ? `扫描到 ${r.added} 台新设备，已自动添加` : '已刷新设备状态', 'success')
  } catch (e) {
    toast('刷新失败：' + e.message, 'error')
  } finally {
    scanning.value = false
  }
}

/** 表单 → 保存 payload（镜像模式不使用虚拟屏参数） */
function buildPayload() {
  return {
    name: form.name.trim(),
    kind: form.kind,
    addr: form.addr.trim(),
    screen_mode: form.screen_mode,
    vd_res: form.screen_mode === 'virtual' ? form.vd_res.trim() : null,
    vd_dpi: form.screen_mode === 'virtual' ? Number(form.vd_dpi) || 0 : null,
    pkg: form.screen_mode === 'virtual' ? (form.pkg.trim() || null) : null,
    fps: Number(form.fps) || 30
  }
}

/** 判断本次保存的 payload 相对旧配置是否触碰投屏会话参数（与服务端
 *  session_affecting_change 同口径：kind/addr/screen_mode/vd_res/vd_dpi/fps）。
 *  仅名称/应用变更时服务端保持会话，前端据此前提示「不断开投屏」。 */
function castingParamsChanged(d, p) {
  const normRes = s => String(s || '').trim().toLowerCase() || '1920x1080'
  return d.kind !== p.kind
    || (d.addr || '').trim() !== p.addr
    || (d.screen_mode || 'virtual') !== p.screen_mode
    || normRes(d.vd_res) !== normRes(p.vd_res)
    || Number(d.vd_dpi || 0) !== Number(p.vd_dpi || 0)
    || Number(d.fps || 30) !== Number(p.fps || 30)
}

/** 设置弹窗保存：编辑模式 PUT 更新配置，成功后关闭弹窗。
 *  投屏相关参数变更且已连接时，服务端踢 viewer → onclose → 自动重连生效，
 *  前端无需手动重连（避免与自动重连并发导致双连接）；仅改名称/应用时
 *  服务端保持会话，投屏不中断。 */
async function saveSettings() {
  if (mode.value === 'add') return addDevice()
  const d = current.value
  if (!d || configApplying.value) return
  const payload = buildPayload()
  if (!payload.name) return toast('请填写设备名称', 'error')
  const wasConnected = connected.value
  const castingChanged = castingParamsChanged(d, payload)
  configApplying.value = true
  try {
    await api.updateDevice(d.id, payload)
    await loadData()
    const nd = devices.value.find(x => x.id === d.id)
    if (nd) loadForm(nd)
    settingsOpen.value = false
    toast(wasConnected && castingChanged ? '配置已保存，投屏参数变更，自动重连中…' : '配置已保存', 'success')
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  } finally {
    configApplying.value = false
  }
}

/** 建立连接（配置统一在设置弹窗内显式保存，连接时无待保存修改） */
async function flushAndConnect() {
  if (mode.value === 'add') return toast('请先完成或取消「新增设备」', 'warn')
  if (!store.deviceId) return
  connect(true)
}

/** 手动新增设备（POST 返回 id，创建后自动选中） */
async function addDevice() {
  const payload = buildPayload()
  if (!payload.name) return toast('请填写设备名称', 'error')
  try {
    const r = await api.createDevice(payload)
    await loadData()
    // 新增成功后切换到新设备：先断开旧设备连接，避免画面/控制仍指向旧设备
    if (connected.value || consoleRuntime.reconnectTimer.value) {
      consoleRuntime.cancelReconnect()
      cleanup(true)
    }
    store.deviceId = r.id
    const nd = devices.value.find(x => x.id === r.id)
    if (nd) loadForm(nd)
    settingsOpen.value = false
    toast('设备已添加，点击连接开始投屏', 'success')
  } catch (e) {
    toast('添加失败：' + e.message, 'error')
  }
}

async function removeDevice() {
  const d = current.value
  if (!d) return
  if (!confirm(`确定删除设备 ${d.name}？`)) return
  try {
    await api.deleteDevice(d.id)
    if (connected.value || consoleRuntime.reconnectTimer.value) {
      consoleRuntime.cancelReconnect()
      cleanup(true)
    }
    devicesData.value = devices.value.filter(x => x.id !== d.id)
    if (devices.value.length) {
      store.deviceId = devices.value[0].id
      loadForm(devices.value[0])
    } else {
      store.deviceId = null
      mode.value = 'edit'
      pkgDraft.value = ''
      appList.value = []
      appHint.value = ''
    }
    toast('设备已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

/** 主动断开（只停本页 WebRTC，不拆服务端↔设备会话，不触发自动重连；
 *  设备会话由服务端空闲低功耗统一管理：无 viewer 无脚本 5 分钟后
 *  虚拟屏拆会话/镜像关屏） */
function disconnect() {
  if (!store.deviceId) return
  consoleRuntime.cancelReconnect()
  cleanup(true)
  toast('已断开投屏（设备会话保留）', 'info')
}

/** 从设备读取已安装应用（scrcpy list_apps，带真实软件名），带缓存避免重复读取 */
async function loadApps() {
  if (appLoading.value) return
  const key = appCacheKey()
  const cached = appCache.get(key)
  // 5 分钟内直接用缓存，应用列表不是经常变
  if (cached && Date.now() - cached.ts < APP_CACHE_TTL) {
    appList.value = cached.list
    appHint.value = cached.list.length ? `已加载缓存（共 ${cached.list.length} 个应用）` : '设备上未发现第三方应用（缓存）'
    return
  }
  appLoading.value = true
  appHint.value = '正在读取设备应用…'
  try {
    const list = mode.value === 'edit' && store.deviceId
      ? await api.listApps(store.deviceId)
      : await api.listAppsByAddr(form.addr.trim())
    appList.value = list || []
    appCache.set(key, { list: appList.value, ts: Date.now() })
    appHint.value = appList.value.length ? `共 ${appList.value.length} 个应用，输入关键字搜索` : '设备上未发现第三方应用'
  } catch (e) {
    appList.value = []
    appHint.value = '读取失败：' + e.message + '（可直接手动输入包名后回车确认）'
  } finally {
    appLoading.value = false
  }
}

function pickApp(a) {
  form.pkg = a.pkg
  pkgDraft.value = a.pkg
  appOpen.value = false
}

/** 手动输入包名不会自动保存；按回车确认后才写入配置并触发保存 */
function commitPkg() {
  const pkg = pkgDraft.value.trim()
  form.pkg = pkg
  appOpen.value = false
}

const selStyle = computed(() => ({
  left: Math.min(selStart.x, selEnd.x) + 'px',
  top: Math.min(selStart.y, selEnd.y) + 'px',
  width: Math.abs(selEnd.x - selStart.x) + 'px',
  height: Math.abs(selEnd.y - selStart.y) + 'px'
}))

const hitStyle = computed(() => {
  const vw = videoWrap.value
  if (!vw) return {}
  const rect = vw.getBoundingClientRect()
  const vw_ = rect.width, vh = rect.height
  const sw = videoElement.value?.videoWidth || 1920
  const sh = videoElement.value?.videoHeight || 1080
  const ratio = Math.min(vw_ / sw, vh / sh)
  const w = hit.w * ratio, h = hit.h * ratio
  const x = (hit.x * ratio) + (vw_ - sw * ratio) / 2
  const y = (hit.y * ratio) + (vh - sh * ratio) / 2
  return { left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' }
})



/** 设备像素矩形 → 显示坐标样式（object-fit: contain 的 letterbox 映射；脚本事件效果用） */
function deviceRectStyle(x, y, w = 0, h = 0) {
  const vw = videoWrap.value
  if (!vw) return {}
  const rect = vw.getBoundingClientRect()
  return mapDeviceRectStyle(x, y, w, h, rect, videoElement.value?.videoWidth, videoElement.value?.videoHeight)
}

/** 脚本运行可视化效果位置（tap 圆点居中偏移由 .alt-tap 的 transform 处理） */
const fxTapStyle = computed(() => (scriptFx.tap.show ? deviceRectStyle(scriptFx.tap.x, scriptFx.tap.y) : {}))
const fxSwipeStyle = computed(() => (scriptFx.swipe.show
  ? deviceRectStyle(scriptFx.swipe.x, scriptFx.swipe.y, scriptFx.swipe.w, scriptFx.swipe.h)
  : {}))
const fxHitStyle = computed(() => (scriptFx.hit.show
  ? deviceRectStyle(scriptFx.hit.x, scriptFx.hit.y, scriptFx.hit.w, scriptFx.hit.h)
  : {}))

async function loadData() {
  await consoleRuntime.loadData()
}

// ---------- WebRTC 连接 ----------

function toggleAudio() {
  audioMuted.value = !audioMuted.value
  const v = videoElement.value
  if (v) {
    v.muted = audioMuted.value
    // 静音时禁用音频轨（关键）：音频轨参与浏览器 A/V 同步（主时钟），scrcpy
    // 虚拟屏音频流在 Chrome 侧播放时钟异常会把视频 jitter buffer 目标延迟单调
    // 拉高——挂机（静止画面）时延迟从 ~87ms 累积到 3s+ 且不回落（见 AGENTS.md
    // 已知坑）。禁用该轨后视频独立播放，延迟不再累积；要听声音再启用。
    if (mediaStream) {
      for (const t of mediaStream.getAudioTracks()) t.enabled = !audioMuted.value
    }
    // 取消静音时浏览器要求用户手势后播放（已处于点击事件内，直接 play 即可）
    if (!audioMuted.value) v.play().catch(() => {})
  }
  // 同步服务端音频转发开关（默认不发音频，开启后才开始转发）
  if (connected.value) sendControl({ type: 'audio', on: !audioMuted.value })
}

async function connect(manual = false) {
  await webrtcLifecycle.connect(manual)
}

/** 释放 WebRTC 资源；manual=true 表示主动关闭（不触发自动重连） */
function cleanup(manual = false) {
  webrtcLifecycle.cleanup(manual)
  consoleRuntime.cleanup()
}

function handleVideoSilence() {
  if (manualClose.value || !connected.value || !store.deviceId) return
  console.warn('[webrtc] video stream silent, treating as disconnected')
  connected.value = false
  scheduleReconnect()
}

/** 格式化传输码率 */
function formatBitrate(bps) {
  if (!bps || bps <= 0) return '—'
  if (bps >= 1000000) return (bps / 1000000).toFixed(1) + ' Mbps'
  if (bps >= 1000) return Math.round(bps / 1000) + ' Kbps'
  return Math.round(bps) + ' bps'
}

function startStats() {
  if (statsTimer) clearInterval(statsTimer)
  statsTimer = setInterval(async () => {
    if (!webrtcLifecycle.getPeerConnection()) return
    const v = videoElement.value
    // 两级黑屏处理（静态屏 MTK 编码器对 reset 响应极慢，实测要多次才吐 IDR）：
    // 8s 仍无可解码帧 → 先补一次 reset_video 要 IDR，继续等到 16s；仍黑屏才重连。
    // 旧实现 8s 直接重连：编码器还没来得及响应就被拆链，静态屏下形成重连风暴
    if (connected.value && v && !hadVideo && v.videoWidth === 0 && Date.now() - videoConnectTs > 8000) {
      const blackFor = Date.now() - videoConnectTs
      if (!blackResetSent && blackFor < 16000) {
        blackResetSent = true
        console.warn('[webrtc] no decodable video after 8s, requesting IDR via reset_video')
        sendControl({ type: 'reset_video' })
        return
      }
      if (blackFor >= 16000) {
        console.warn('[webrtc] no decodable video after 16s, reconnecting')
        handleVideoSilence()
        return
      }
    }
    // 视频静默检测：仅在见过画面后启用（连接初期 currentTime=0 不误判）
    if (connected.value && v && v.videoWidth > 0) {
      hadVideo = true
      if (Math.abs(v.currentTime - lastVideoTime) < 0.001 && !videoBytesAdvanced) {
        if (++stillFrames >= 2) { // 连续 ~4s：currentTime 冻结且零新增字节
          stillFrames = 0
          handleVideoSilence()
        }
      } else {
        stillFrames = 0
        lastVideoTime = v.currentTime
      }
      // 画面停滞看门狗（原理见变量处注释）：渲染像素指纹连续未变 + 近期有
      // 拖动/滚轮输入（画面本应变化）。先 reset_video，5s 后仍冻结才重连，
      // 避免对"拖不动/本就静止"的界面频繁误重连
      if (connected.value && hadVideo) {
        let fp = ''
        try {
          if (!fpCanvas) {
            fpCanvas = document.createElement('canvas')
            fpCanvas.width = 24; fpCanvas.height = 14
            fpCtx = fpCanvas.getContext('2d', { willReadFrequently: true })
          }
          fpCtx.drawImage(v, 0, 0, 24, 14)
          const d = fpCtx.getImageData(0, 0, 24, 14).data
          let h = 5381
          for (let i = 0; i < d.length; i += 4) h = ((h * 33) ^ (d[i] + d[i+1] + d[i+2])) >>> 0
          fp = String(h)
        } catch (err) { /* drawImage 失败（如视频未就绪）跳过本轮 */ }
        if (fp && fp === renderFpLast) {
          renderFpFrozen++
        } else {
          renderFpFrozen = 0
          if (fp) stallResetSent = false
        }
        renderFpLast = fp
        if (renderFpFrozen >= 5 && Date.now() - lastDragInputAt < 8000) {
          renderFpFrozen = 0
          if (!stallResetSent) {
            stallResetSent = true
            console.warn('[webrtc] picture frozen after drag/scroll input, requesting IDR via reset_video')
            sendControl({ type: 'reset_video' })
          } else {
            stallResetSent = false
            console.warn('[webrtc] picture still frozen after reset_video, reconnecting to rebuild jitter buffer')
            handleVideoSilence()
          }
        }
      }
    }
    try {
      const stats = await webrtcLifecycle.getPeerConnection().getStats()
      let fpsCount = 0
      stats.forEach(s => {
        if (s.type === 'inbound-rtp' && s.kind === 'video') {
          if (s.framesPerSecond) fpsCount = Math.round(s.framesPerSecond)
          // 画面延迟：jitterBufferDelay 规范单位为秒（个别 Chromium 版本报 ms，自适应：
          // 单帧均值 >50s 视为 ms 直读，否则按秒换算）。只统计增量窗口，避免累计均值失真
          if (typeof s.jitterBufferDelay === 'number' && s.jitterBufferEmittedCount > 0) {
            if (lastJbe > 0 && s.jitterBufferEmittedCount > lastJbe) {
              const perFrame = (s.jitterBufferDelay - lastJbd) / (s.jitterBufferEmittedCount - lastJbe)
              if (perFrame >= 0 && perFrame < 50) {
                delay.value = Math.round(perFrame * 1000)
              }
              // 延迟看门狗：音频轨 A/V 同步异常会把 jitter buffer 目标延迟单调拉高
              // （挂机静止画面 87ms → 3s+ 且不回落，见 AGENTS.md 已知坑）。连续两次
              // 采样超阈值（~4s）→ 走断流重连路径重置缓冲（含页面锁二次检查）
              if (delay.value > 1500) {
                if (++delaySpikes >= 2) {
                  delaySpikes = 0
                  console.warn('[webrtc] latency watchdog: delay=' + delay.value + 'ms, reconnecting')
                  handleVideoSilence()
                  return
                }
              } else {
                delaySpikes = 0
              }
            }
            lastJbd = s.jitterBufferDelay
            lastJbe = s.jitterBufferEmittedCount
          }
          // 传输码率：按字节增量 / 时间增量估算
          if (typeof s.bytesReceived === 'number') {
            const now = Date.now()
            if (lastBytesReceived > 0 && lastBitrateTs > 0) {
              const dt = (now - lastBitrateTs) / 1000
              if (dt > 0) bitrate.value = formatBitrate(((s.bytesReceived - lastBytesReceived) * 8) / dt)
            }
            // 链路活性（静默检测双条件用，见检测处注释）；重连后计数回退
            videoBytesAdvanced = s.bytesReceived > lastBytesReceived
            if (s.bytesReceived < lastBytesReceived) lastBytesReceived = 0
            lastBytesReceived = s.bytesReceived
            lastBitrateTs = now
          }
          // 花屏自愈：解码器失步（PLI 增量）→ 请求设备立即出关键帧。
          // 限频 2s：持续丢包（WiFi 差）时最多每 2s 重置一次，不会打爆编码器
          if (typeof s.pliCount === 'number') {
            if (s.pliCount < lastPliCount) lastPliCount = s.pliCount // 重连后回退
            if (s.pliCount > lastPliCount) {
              lastPliCount = s.pliCount
              // 连接初期（~6s 内）Chrome 加入流时会例行发 PLI 请求关键帧，不是失步：
              // 静态屏（无应用/挂机静止）编码器对 reset 响应极慢（MTK 要多次才吐
              // IDR），reset 反而打断静止补帧 → 浏览器断供 4s 被静默检测杀掉 →
              // "连上一会儿就断"死循环。真失步（解码中突发花屏）不受此窗口限制
              const joinWindow = Date.now() - videoConnectTs < 6000
              const now = Date.now()
              const backoff = pliResetStreak >= 4 ? 60000 : pliResetStreak >= 2 ? 15000 : 2000
              if (!joinWindow && connected.value && now - lastPliResetAt > backoff) {
                lastPliResetAt = now
                pliResetStreak++
                console.warn('[webrtc] decoder desync (pliCount=' + s.pliCount + ', streak=' + pliResetStreak + '), requesting IDR via reset_video')
                sendControl({ type: 'reset_video' })
              }
            } else if (s.pliCount === lastPliCount && lastPliCount > 0) {
              // 一整个统计周期无新 PLI：解码器已满足，退避复位
              pliResetStreak = 0
            }
          }
          // 诊断：每 3 次打印一次接收统计
          if (!window.__rtpStatsCount) window.__rtpStatsCount = 0
          if (++window.__rtpStatsCount % 3 === 0) {
            const v = videoElement.value
            console.log('[webrtc] inbound-rtp', JSON.stringify({
              bytesReceived: s.bytesReceived, packetsReceived: s.packetsReceived,
              framesDecoded: s.framesDecoded, framesDropped: s.framesDropped,
              framesPerSecond: s.framesPerSecond, keyFramesDecoded: s.keyFramesDecoded,
              pliCount: s.pliCount, nackCount: s.nackCount,
              codecId: s.codecId, decoder: s.decoderImplementation,
              videoWidth: v?.videoWidth, videoHeight: v?.videoHeight, readyState: v?.readyState
            }))
          }
        }
      })
      if (fpsCount) fps.value = fpsCount
    } catch (e) {}
    // 1s 轮询：比 2s 更快发现花屏（PLI 自愈延迟减半）与延迟/静默异常
  }, 1000)
}

function stopStats() {
  if (statsTimer) {
    clearInterval(statsTimer)
    statsTimer = null
  }
}

function parseLogTime(s) {
  if (!s) return 0
  const d = new Date(s.replace(' ', 'T'))
  return d.getTime() || 0
}

function scrollLogsToBottom() {
  nextTick(() => {
    const el = logBox.value
    if (el) el.scrollTop = el.scrollHeight
  })
}

function applyLogFilter() {
  // 日志级别由脚本顶层 log_level 在服务端过滤（debug/info），前端只按运行开始时间截取
  const filtered = (rawLogs || []).filter(l => {
    if (runStartTime && parseLogTime(l.time) < runStartTime) return false
    return true
  })
  liveLogs.value = filtered.map(l => ({ time: l.time.slice(11, 23), level: l.level, msg: l.msg })).reverse()
  scrollLogsToBottom()
}

async function refreshLogs() {
  try {
    const logs = await consoleRuntime.refreshLogs()
    rawLogs = logs || []
    applyLogFilter()
  } catch (e) {}
}

function startLogPolling() {
  consoleRuntime.startLogPolling(refreshLogs)
}

// ---------- 控制（走 DataChannel） ----------

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
  console.warn('[control] channel not open, fallback REST', JSON.stringify(obj))
  // fallback：REST API
  api.control(store.deviceId, obj).catch(e => toast('控制失败：' + e.message, 'error'))
  return false
}

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

/** 鼠标坐标 → 设备坐标（object-fit: contain 换算） */
function toDeviceCoord(clientX, clientY) {
  const video = videoElement.value
  const rect = video.getBoundingClientRect()
  return mapToDeviceCoord(clientX, clientY, rect, video.videoWidth, video.videoHeight)
}

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
  pendingMove = { type: 'touch', action: 'move', x, y }
  if (!moveRaf) moveRaf = requestAnimationFrame(flushPendingMove)
}
function cancelPendingMove() {
  if (moveRaf) { cancelAnimationFrame(moveRaf); moveRaf = 0 }
  pendingMove = null
}

/** 录制指针换算：设备像素 → 相对坐标（帧原始尺寸基准，plan §11.6）；画面未就绪返回 null */
function recordingPointerMeta(e) {
  const v = videoElement.value
  if (!v || !v.videoWidth || !v.videoHeight) return null
  const pt = toDeviceCoord(e.clientX, e.clientY)
  return { relX: pt.x / v.videoWidth, relY: pt.y / v.videoHeight, frameW: v.videoWidth, frameH: v.videoHeight }
}

function onMouseDown(e) {
  // 录制透传（plan §11）：DOWN 冻结帧→立即发送触控（不等待编码/上传）；右键/中键忽略
  if (recording.active) {
    if (e.button !== 0) return
    if (!connected.value) return
    const m = recordingPointerMeta(e)
    if (!m) return toast('设备画面不可用，无法录制', 'warn')
    recording.onPointerDown(m)
    return
  }
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
  touchState.active = true
  touchState.lastX = x; touchState.lastY = y
  // 按下：发 DOWN（拖动时后续 move 事件组成轨迹，up 时收尾）
  sendControl({ type: 'touch', action: 'down', x, y })
}

function onMouseMove(e) {
  // 录制透传：MOVE 实时发送（useRecording 侧经 rAF 合并，不等待编码/上传）
  if (recording.active) {
    const m = recordingPointerMeta(e)
    if (m) recording.onPointerMove(m)
    return
  }
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
  if (!touchState.active || !connected.value) return
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  if (Math.abs(x - touchState.lastX) + Math.abs(y - touchState.lastY) > 6) {
    touchState.lastX = x; touchState.lastY = y
    scheduleMove(x, y)
  }
}

function togglePick() {
  confirmDelTpl.value = null
  if (!connected.value) return toast('请先连接设备', 'error')
  picking.value = !picking.value
  if (!picking.value) hideLoupe()
}

function onMouseUp(e) {
  // 录制透传：UP 记录终点/时长 → 手势分类 → 占位插入（控制早已在 DOWN/MOVE 发出）
  if (recording.active) {
    const m = recordingPointerMeta(e)
    if (m) recording.onPointerUp(m)
    return
  }
  if (selecting.value) {
    selecting.value = false
    picking.value = false
    hideLoupe()
    const rect = selToDeviceRect()
    if (rect.w >= 8 && rect.h >= 8) openCrop(rect)
    else toast('框选区域太小，请重新框选', 'warn')
    return
  }
  if (!touchState.active) return
  cancelPendingMove()
  touchState.active = false
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  sendControl({ type: 'touch', action: 'up', x, y })
}

/** 鼠标离开投屏区域时终止未完成的 alt 手势/录制手势，避免卡在记录模式 */
function onVideoMouseLeave() {
  hideLoupe()
  if (recording.active) recording.cancelGesture('leave') // pointercancel/失焦同路径：UP 兜底 + 清理
}

// ---------- 框选保存模板 ----------

/** 框选矩形（容器 CSS 坐标）→ 设备像素坐标，自动裁剪 letterbox 黑边并夹取到画面内 */
function selToDeviceRect() {
  const video = videoElement.value
  const rect = videoWrap.value.getBoundingClientRect()
  return selectionToDeviceRect(selStart, selEnd, rect, video?.videoWidth, video?.videoHeight)
}

/** 生成默认模板名：随机名字#x1_y1_x2_y2（相对坐标 0~1，×1000 存 3 位整数，如 0.123→123，不带 .png 后缀） */
function defaultTplName(rect) {
  return defaultTemplateName(rect, videoElement.value?.videoWidth, videoElement.value?.videoHeight)
}

// ---------- 二次裁切 ----------

const cropSize = computed(() => `${Math.round(crop.rect.w)}×${Math.round(crop.rect.h)} px`)
/** 当前显示缩放（100% = 自适应适配），滚轮调整 */
const cropZoomPct = computed(() => `${Math.round(crop.zoom * 100)}%`)

/** 框选完成后打开右侧裁切区 */
function openCrop(rect) {
  confirmDelTpl.value = null
  const video = videoElement.value
  if (!video?.videoWidth) return toast('无法截取画面，请稍后重试', 'error')
  crop.imgW = video.videoWidth
  crop.imgH = video.videoHeight
  crop.originX = Math.round(rect.x)
  crop.originY = Math.round(rect.y)
  crop.baseW = Math.round(rect.w)
  crop.baseH = Math.round(rect.h)
  crop.zoom = 1
  // 冻结初始框选画面，二次裁切时底图不动，只动遮罩框
  cropBaseCanvas = document.createElement('canvas')
  cropBaseCanvas.width = crop.baseW
  cropBaseCanvas.height = crop.baseH
  cropBaseCanvas.getContext('2d').drawImage(video, crop.originX, crop.originY, crop.baseW, crop.baseH, 0, 0, crop.baseW, crop.baseH)
  crop.rect = { x: 0, y: 0, w: crop.baseW, h: crop.baseH }
  crop.name = defaultTplName(rect)
  crop.active = true
  nextTick(() => {
    renderCropFrame()
    refreshCropPreview()
    cropSec.value?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  })
}

function cancelCrop() {
  crop.active = false
  cropBaseCanvas = null
  crop.zoom = 1
  hideLoupe()
}

function repick() {
  crop.active = false
  cropBaseCanvas = null
  crop.zoom = 1
  picking.value = true
  toast('在画面上重新框选', 'info')
}

/** 画布适配尺寸：展示冻结的初始框选画面，可适当放大（再乘滚轮缩放 crop.zoom） */
function cropFit() {
  const w = Math.max(1, crop.baseW)
  const h = Math.max(1, crop.baseH)
  const scale = Math.min(260 / w, 220 / h, 3) * crop.zoom
  return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)), scale: Math.round(w * scale) / w }
}

/** 滚轮缩放裁切底图：以光标下的图像点为锚点放大/缩小，缩放后画布超出区域可滚动查看 */
function cropWheel(e) {
  const canvas = cropCanvas.value
  const stage = cropSec.value?.querySelector('.crop-stage')
  if (!canvas || !stage) return
  e.preventDefault()
  const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15
  const next = Math.max(0.5, Math.min(8, crop.zoom * factor))
  if (next === crop.zoom) return
  const cr = canvas.getBoundingClientRect()
  const sr = stage.getBoundingClientRect()
  // 光标在画布内的位置（画布 CSS 像素 = 画布像素）
  const mx = e.clientX - cr.left
  const my = e.clientY - cr.top
  // 画布原点在滚动内容中的位置
  const ox = cr.left - sr.left + stage.scrollLeft
  const oy = cr.top - sr.top + stage.scrollTop
  const oldW = canvas.width
  const oldH = canvas.height
  crop.zoom = next
  renderCropFrame()
  const kx = canvas.width / oldW
  const ky = canvas.height / oldH
  // 保持光标下的图像点不动：margin:auto 居中时原点 = max(0, (区域宽 - 画布宽)/2)
  const ox1 = Math.max(0, (stage.clientWidth - canvas.width) / 2)
  const oy1 = Math.max(0, (stage.clientHeight - canvas.height) / 2)
  stage.scrollLeft += ox1 + mx * kx - ox - mx
  stage.scrollTop += oy1 + my * ky - oy - my
}

/** 在裁切画布上绘制冻结的框选画面 + 可拖动的遮罩框（拖动时只改框，不动底图） */
function renderCropFrame() {
  const canvas = cropCanvas.value
  const base = cropBaseCanvas
  if (!canvas || !base || base.width < 1 || crop.rect.w < 1 || crop.rect.h < 1) return
  const bw = base.width
  const bh = base.height
  const fit = cropFit()
  canvas.width = fit.w
  canvas.height = fit.h
  canvas.style.width = fit.w + 'px'
  canvas.style.height = fit.h + 'px'
  const ctx = canvas.getContext('2d')
  ctx.clearRect(0, 0, fit.w, fit.h)
  ctx.drawImage(base, 0, 0, bw, bh, 0, 0, fit.w, fit.h)

  const sx = fit.w / bw
  const sy = fit.h / bh
  const rx = crop.rect.x * sx
  const ry = crop.rect.y * sy
  const rw = crop.rect.w * sx
  const rh = crop.rect.h * sy

  // 遮罩框外压暗，拖动时只改变这个遮罩
  ctx.fillStyle = 'rgba(0,0,0,.45)'
  ctx.fillRect(0, 0, fit.w, ry)
  ctx.fillRect(0, ry + rh, fit.w, Math.max(0, fit.h - ry - rh))
  ctx.fillRect(0, ry, rx, rh)
  ctx.fillRect(rx + rw, ry, Math.max(0, fit.w - rx - rw), rh)

  // 边框
  ctx.strokeStyle = 'rgba(34,211,165,.95)'
  ctx.lineWidth = 1.5
  ctx.strokeRect(rx, ry, rw, rh)

  // 角点手柄
  ctx.fillStyle = '#fff'
  const hs = 5
  for (const [hx, hy] of [[rx, ry], [rx + rw, ry], [rx, ry + rh], [rx + rw, ry + rh]]) {
    ctx.fillRect(hx - hs / 2, hy - hs / 2, hs, hs)
  }

  // 尺寸标注
  ctx.fillStyle = 'rgba(34,211,165,.95)'
  ctx.font = '10px monospace'
  ctx.fillText(cropSize.value, rx + 6, ry + 14)
}

/** 按当前遮罩框从冻结底图重新生成裁剪结果预览（全分辨率） */
function refreshCropPreview() {
  const base = cropBaseCanvas
  if (!base || base.width < 1) return
  const r = crop.rect
  if (r.w < 1 || r.h < 1) return
  const canvas = document.createElement('canvas')
  canvas.width = Math.round(r.w)
  canvas.height = Math.round(r.h)
  canvas.getContext('2d').drawImage(base, r.x, r.y, r.w, r.h, 0, 0, Math.round(r.w), Math.round(r.h))
  crop.preview = canvas.toDataURL('image/png')
}

/** 鼠标事件 → 冻结底图上的像素坐标 */
function cropEventDev(e) {
  const canvas = cropCanvas.value
  const rect = canvas.getBoundingClientRect()
  const scale = canvas.width / crop.baseW
  return {
    x: (e.clientX - rect.left) / scale,
    y: (e.clientY - rect.top) / scale
  }
}


function cropMouseDown(e) {
  const p = cropEventDev(e)
  const r = crop.rect
  const HIT = 12 / (cropCanvas.value.width / crop.baseW) // 底图像素命中半径
  const corners = { nw: [r.x, r.y], ne: [r.x + r.w, r.y], sw: [r.x, r.y + r.h], se: [r.x + r.w, r.y + r.h] }
  let mode = null
  for (const [k, [hx, hy]] of Object.entries(corners)) {
    if (Math.hypot(p.x - hx, p.y - hy) <= HIT) { mode = k; break }
  }
  if (!mode) {
    const edges = {
      n: [r.x + r.w / 2, r.y], s: [r.x + r.w / 2, r.y + r.h],
      w: [r.x, r.y + r.h / 2], e: [r.x + r.w, r.y + r.h / 2]
    }
    for (const [k, [hx, hy]] of Object.entries(edges)) {
      const onSeg = (k === 'n' || k === 's') ? (p.x >= r.x && p.x <= r.x + r.w) : (p.y >= r.y && p.y <= r.y + r.h)
      if (Math.hypot(p.x - hx, p.y - hy) <= HIT && onSeg) { mode = k; break }
    }
  }
  if (!mode && p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h) mode = 'move'
  if (!mode) return
  cropDrag.mode = mode
  cropDrag.startX = p.x
  cropDrag.startY = p.y
  cropDrag.orig = { ...r }
  e.preventDefault()
}

function cropMouseMove(e) {
  const p = cropEventDev(e)
  // 放大镜仍按完整画面坐标显示（底图坐标 + 初始框选偏移）
  updateLoupe(e.clientX, e.clientY, { x: p.x + crop.originX, y: p.y + crop.originY }, 3, [{ x: crop.rect.x + crop.originX, y: crop.rect.y + crop.originY, w: crop.rect.w, h: crop.rect.h }])
  if (!cropDrag.mode) return
  const o = cropDrag.orig
  const r = crop.rect
  const MIN = 8
  const dx = p.x - cropDrag.startX
  const dy = p.y - cropDrag.startY
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v))
  switch (cropDrag.mode) {
    case 'move':
      r.x = clamp(o.x + dx, 0, crop.baseW - o.w)
      r.y = clamp(o.y + dy, 0, crop.baseH - o.h)
      break
    case 'nw':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.w = o.x + o.w - r.x; r.h = o.y + o.h - r.y
      break
    case 'ne':
      r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.h = o.y + o.h - r.y
      break
    case 'sw':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.w = o.x + o.w - r.x
      r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
      break
    case 'se':
      r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
      r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
      break
    case 'n':
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.h = o.y + o.h - r.y
      break
    case 's':
      r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
      break
    case 'w':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.w = o.x + o.w - r.x
      break
    case 'e':
      r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
      break
  }
  renderCropFrame()
}

function cropMouseUp() {
  if (!cropDrag.mode) return
  cropDrag.mode = null
  refreshCropPreview()
}

function cropMouseLeave() {
  hideLoupe()
  if (cropDrag.mode) { cropDrag.mode = null; refreshCropPreview() }
}

/** 上传响应的体积提示：823KB → 96KB（服务端灰度 PNG 重编码） */
function tplSizeHint(rep) {
  if (!rep?.size || !rep?.orig_size) return ''
  const fmt = n => n >= 1024 * 1024 ? (n / 1024 / 1024).toFixed(1) + 'MB' : n >= 1024 ? Math.round(n / 1024) + 'KB' : n + 'B'
  return `（${fmt(rep.orig_size)} → ${fmt(rep.size)}）`
}

async function saveTemplate() {
  const raw = crop.name.trim()
  if (!raw) return toast('请输入模板名称', 'warn')
  if (!activePkg.value) return toast('请先选择应用分区', 'warn')
  const name = raw.toLowerCase().endsWith('.png') ? raw : raw + '.png'
  const shortName = name.replace(/#[^#]+\.png$/i, '.png')
  const region = [
    crop.originX / crop.imgW,
    crop.originY / crop.imgH,
    (crop.originX + crop.baseW) / crop.imgW,
    (crop.originY + crop.baseH) / crop.imgH,
  ]
  saving.value = true
  try {
    const rep = await api.createTemplate(shortName, crop.preview.split(',')[1], activePkg.value, region)
    templatesData.value = await api.listTemplates()
    crop.active = false
    cropBaseCanvas = null
    hideLoupe()
    toast(`模板 ${rep?.name || shortName} 已保存${tplSizeHint(rep)}`, 'success')
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  } finally {
    saving.value = false
  }
}

// ---------- 放大预览镜 ----------

/** 以光标为中心放大当前视频帧：devPt 为放大中心（设备像素），rects 为要叠加显示的选区（设备像素坐标） */
function updateLoupe(clientX, clientY, devPt, zoom, rects) {
  const video = videoElement.value
  const canvas = loupeCanvas.value
  if (!video?.videoWidth || !canvas) return
  const c = devPt
  const L = canvas.width
  const half = L / zoom / 2
  const ctx = canvas.getContext('2d')
  ctx.clearRect(0, 0, L, L)
  ctx.imageSmoothingEnabled = true
  ctx.drawImage(video, c.x - half, c.y - half, half * 2, half * 2, 0, 0, L, L)
  // 十字准星：贯穿全幅的长线
  ctx.strokeStyle = 'rgba(255,255,255,.3)'
  ctx.lineWidth = 1
  ctx.beginPath()
  ctx.moveTo(L / 2, 0); ctx.lineTo(L / 2, L)
  ctx.moveTo(0, L / 2); ctx.lineTo(L, L / 2)
  ctx.stroke()
  // 中心点
  ctx.fillStyle = 'rgba(255,255,255,.95)'
  ctx.beginPath()
  ctx.arc(L / 2, L / 2, 2, 0, Math.PI * 2)
  ctx.fill()
  // 选区轮廓
  ctx.strokeStyle = 'rgba(34,211,165,.95)'
  ctx.lineWidth = 1.5
  for (const r of rects || []) {
    ctx.strokeRect((r.x - (c.x - half)) * zoom, (r.y - (c.y - half)) * zoom, r.w * zoom, r.h * zoom)
  }
  // 定位：跟随光标，越界自动翻转
  loupe.zoom = zoom
  loupe.show = true
  const W = 160, G = 14
  let x = clientX + G, y = clientY + G
  if (x + W > window.innerWidth - 6) x = clientX - W - G
  if (y + W > window.innerHeight - 6) y = clientY - W - G
  loupe.x = Math.max(6, x)
  loupe.y = Math.max(6, y)
}

function hideLoupe() { loupe.show = false }

function onWheel(e) {
  if (!connected.value) return
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  sendControl({ type: 'scroll', x, y, scroll_x: e.deltaX, scroll_y: e.deltaY })
}

function key(k) {
  if (!connected.value) return
  const codes = { HOME: 3, BACK: 4, APP_SWITCH: 187, VOL_UP: 24, VOL_DOWN: 25 }
  sendControl({ type: 'press', keycode: codes[k] || 0 })
}

function shot() {
  if (!connected.value) return toast('请先连接设备', 'error')
  api.screenshot(store.deviceId).then(dataUrl => {
    const a = document.createElement('a')
    a.href = dataUrl
    a.download = `screenshot-${Date.now()}.png`
    a.click()
    toast('截图已保存', 'success')
  }).catch(e => toast('截图失败：' + e.message, 'error'))
}

function rotate() { if (connected.value) sendControl({ type: 'rotate' }) }
function clipboard() {
  if (!connected.value) return
  const text = prompt('输入要发送到设备的剪贴板内容')
  if (text !== null) sendControl({ type: 'clipboard', text, paste: true })
}

function launchGame() {
  if (!connected.value) return toast('请先连接设备', 'error')
  if (!currentPkg.value) return toast('该设备未配置应用包名', 'warn')
  sendControl({ type: 'start_app', app: currentPkg.value })
  if (store.deviceId) appStartedDevices.add(store.deviceId)
  appHintDismissed.value = true
  toast(`正在启动 ${currentPkg.value}…`, 'info')
}

function openScripts() { router.push('/scripts') }

function tplThumbUrl(name) { return api.tplImageUrl(name, activePkg.value) }

/** 模板列表：行空白区点击 → 查看大图（缩略图/文件名单元格有各自的交互） */
function onTplRowClick(e, t) {
  confirmDelTpl.value = null
  openTplView(t.name)
}

/** 模板列表缩略图：alt（按住 Alt / alt 模式）→ 复制模板名；普通 → 查看大图 */
async function onTplThumbClick(e, t) {
  confirmDelTpl.value = null
  openTplView(t.name)
}

// ---------- 画布模板下拉悬停缩略图（CellEditor inject；短名 → 当前分区图片 URL） ----------
provide('tplPreviewUrl', (short) => {
  const full = templatesData.value.find(t => t.pkg === activePkg.value && tplShortName(t.name) === short)?.name
  return api.tplImageUrl(full || short, activePkg.value)
})

// ---------- 步骤编辑器取值工具（CellEditor inject('seCellTools')）：投屏选点/选色/框选 ----------
// 选点/选色为单次点击模式：进入后下一次画面点击解析坐标/颜色（选色走放大镜），Esc 取消；
// 框选复用模板页签的既有流程（进入框选 → 二次裁切 → 上传），完成后模板下拉自动可见新模板。

const cellPick = reactive({ mode: null, resolve: null }) // mode: 'coord' | 'color'

function beginCellPick(mode) {
  if (!connected.value) {
    toast('请先连接设备', 'error')
    return Promise.resolve(null)
  }
  if (cellPick.mode) cellPick.resolve?.(null)
  return new Promise((resolve) => {
    cellPick.mode = mode
    cellPick.resolve = resolve
    toast(mode === 'color' ? '在画面上点击取色（Esc 取消）' : '在画面上点击选点（Esc 取消）', 'info')
  })
}

function cancelCellPick() {
  if (cellPick.mode) {
    cellPick.resolve?.(null)
    cellPick.mode = null
    cellPick.resolve = null
  }
  hideLoupe()
}

/** 从当前视频帧采样设备像素颜色 → 6 位 hex（画面不可用返回 null） */
function samplePixelHex(devX, devY) {
  const v = videoElement.value
  if (!v?.videoWidth) return null
  const c = document.createElement('canvas')
  c.width = 1
  c.height = 1
  const ctx = c.getContext('2d')
  ctx.drawImage(v, Math.round(devX), Math.round(devY), 1, 1, 0, 0, 1, 1)
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data
  return [r, g, b].map(n => n.toString(16).padStart(2, '0')).join('')
}

/** 视频画面点击 → 结束取点模式：coord 回相对坐标，color 回采样 hex */
function finishCellPick(e) {
  const v = videoElement.value
  const pt = toDeviceCoord(e.clientX, e.clientY)
  const mode = cellPick.mode
  cellPick.mode = null
  const resolve = cellPick.resolve
  cellPick.resolve = null
  hideLoupe()
  if (!v?.videoWidth) {
    resolve?.(null)
    toast('设备画面不可用', 'warn')
    return
  }
  if (mode === 'coord') {
    resolve?.({ x: Number((pt.x / v.videoWidth).toFixed(4)), y: Number((pt.y / v.videoHeight).toFixed(4)) })
  } else if (mode === 'color') {
    const hex = samplePixelHex(pt.x, pt.y)
    if (hex) resolve?.({ hex, x: pt.x, y: pt.y })
    else { resolve?.(null); toast('取色失败：画面不可用', 'warn') }
  }
}

provide('seCellTools', {
  pickCoord: () => beginCellPick('coord'),
  pickColor: () => beginCellPick('color'),
  /** 框选生成新模板：切到模板页签并进入框选，用户走既有 二次裁切→上传 流程，
   *  上传成功后 templatesData 刷新，CellEditor 模板下拉即可选到新模板 */
  captureTemplate: () => {
    if (!connected.value) {
      toast('请先连接设备', 'error')
      return Promise.resolve(null)
    }
    panelTab.value = 'tpl'
    picking.value = true
    toast('在画面上框选模板区域，完成后保存模板', 'info')
    return Promise.resolve(null)
  },
})

/** 模板列表文件名点击：查看大图（alt 生成 find 已随可视化编辑移除，插入步骤走编辑器） */
function onTplNameClick(e, t) {
  if (renaming.value === t.name) return
  confirmDelTpl.value = null
  openTplView(t.name)
}

/** 复制文本到剪贴板：navigator.clipboard 需安全上下文（localhost），
 *  LAN http 访问时回退 execCommand（临时 textarea） */
async function copyText(text) {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch { /* 回退 execCommand */ }
  const ta = document.createElement('textarea')
  ta.value = text
  ta.style.position = 'fixed'
  ta.style.opacity = '0'
  document.body.appendChild(ta)
  ta.select()
  const ok = document.execCommand('copy')
  ta.remove()
  return ok
}

/** 模板列表：查看大图 */
function openTplView(name) {
  confirmDelTpl.value = null
  viewTpl.value = name
}
function closeTplView() {
  viewTpl.value = null
}

// 模板查看：悬停坐标读数已随 until 的 click 参数删除一并移除（命中恒点模板中心）

// ---------- 模板重命名 ----------

/** 重命名输入框初始值：去掉图片后缀 */
function renameBase(name) {
  return name.replace(/\.(png|jpe?g)$/i, '')
}

function startRename(t) {
  confirmDelTpl.value = null
  renaming.value = t.name
  renameVal.value = renameBase(t.name)
  nextTick(() => renameInputEl?.select())
}

/** 输入框失焦 / Esc → 取消重命名（不保存） */
function cancelRename() {
  renaming.value = null
}

/** 确认重命名：名称去空格、自动补 .png 后缀、重名校验，成功后刷新列表 */
async function confirmRename(t) {
  const raw = renameVal.value.trim()
  if (!raw) return toast('名称不能为空', 'warn')
  const newName = /\.(png|jpe?g)$/i.test(raw) ? raw : raw + '.png'
  renaming.value = null
  if (newName === t.name) return
  if (templatesData.value.some(x => x.pkg === activePkg.value && x.name === newName)) return toast(`已存在同名模板：${newName}`, 'warn')
  try {
    await api.renameTemplate(t.name, newName, activePkg.value)
    templatesData.value = await api.listTemplates()
    toast(`模板已重命名为 ${newName}`, 'success')
  } catch (e) {
    toast('重命名失败：' + e.message, 'error')
  }
}

/** 模板列表：匹配按钮（测试匹配） */
function onTplMatchClick(t) {
  confirmDelTpl.value = null
  testMatch(t.name)
}

/** 模板列表：删除按钮（第一次变确认，第二次删除；其他操作自动取消） */
async function onTplDeleteClick(t) {
  if (confirmDelTpl.value === t.name) {
    confirmDelTpl.value = null
    try {
      await api.deleteTemplate(t.name, activePkg.value)
      templatesData.value = await api.listTemplates()
      if (viewTpl.value === t.name) viewTpl.value = null
      toast('模板已删除', 'success')
    } catch (e) {
      toast('删除失败：' + e.message, 'error')
    }
  } else {
    confirmDelTpl.value = t.name
  }
}

/** 模板列表：上传图片模板 */
async function onTplUpload(e) {
  confirmDelTpl.value = null
  const file = e.target.files[0]
  e.target.value = ''
  if (!file) return
  const name = /\.png$/i.test(file.name)
    ? file.name
    : file.name.replace(/\.[^.]+$/, '') + '.png'
  try {
    const b64 = await fileToBase64(file)
    const rep = await api.createTemplate(name, b64, activePkg.value)
    templatesData.value = await api.listTemplates()
    toast(`模板已新建${tplSizeHint(rep)}`, 'success')
  } catch (err) {
    toast('新建失败：' + err.message, 'error')
  }
}

/** 替换已有模板图片：名称/分区来自当前模板，图片替换使用独立当前端点。 */
async function replaceTemplateImage(t, file) {
  if (!t || !file) return
  try {
    const b64 = await fileToBase64(file)
    await api.replaceTemplateImage(t.name, b64, t.pkg || activePkg.value)
    templatesData.value = await api.listTemplates()
    toast(`模板 ${t.name} 图片已替换`, 'success')
  } catch (err) {
    toast('替换失败：' + err.message, 'error')
  }
}

function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const fr = new FileReader()
    fr.onload = () => resolve(fr.result.split(',')[1])
    fr.onerror = reject
    fr.readAsDataURL(file)
  })
}

/** 全局按键：Esc 关闭设备设置弹窗 / 模板大图 / 取消删除确认 */
function onGlobalKeydown(e) {
  if (e.key !== 'Escape') return
  if (cellPick.mode) {
    cancelCellPick()
  } else if (settingsOpen.value) {
    cancelSettings()
  } else if (viewTpl.value) {
    closeTplView()
  } else if (confirmDelTpl.value) {
    confirmDelTpl.value = null
  }
}

function pushLog(level, msg) {
  const now = new Date()
  const t = now.toTimeString().slice(0, 8) + '.' + String(now.getMilliseconds()).padStart(3, '0')
  liveLogs.value.push({ time: t, level, msg })
  if (liveLogs.value.length > 30) liveLogs.value.shift()
  scrollLogsToBottom()
}





/** 从模板名解析 #x1_y1_x2_y2（相对坐标 ×1000 存 3 位整数，如 123→0.123），返回 [x1,y1,x2,y2] 或 null */
function parseTplRegion(name) {
  const base = name.replace(/\.(png|jpe?g)$/i, '')
  const idx = base.lastIndexOf('#')
  if (idx < 0) return null
  const parts = base.slice(idx + 1).split('_')
  if (parts.length !== 4) return null
  const nums = parts.map(s => /^\d{1,3}$/.test(s) ? Number(s) / 1000 : NaN)
  if (!nums.every(n => Number.isFinite(n) && n >= 0 && n <= 1) || !(nums[2] > nums[0]) || !(nums[3] > nums[1])) return null
  return nums
}

/** 从模板名解析半区代码后缀（#a/#u/#d/#l/#r/#ul/#ur/#dl/#dr），如 task_item#l.png → 'l'；无 → null */
function parseTplRegionCode(name) {
  const base = name.replace(/\.(png|jpe?g)$/i, '')
  const idx = base.lastIndexOf('#')
  if (idx < 0) return null
  const code = base.slice(idx + 1).toLowerCase()
  return ['a', 'u', 'd', 'l', 'r', 'ul', 'ur', 'dl', 'dr'].includes(code) ? code : null
}

/** 模板短名：去掉 #区域后缀（login#0_0_500_500.png → login.png），无后缀原样返回。
 *  脚本里写短名即可，引擎自动解析到唯一匹配的带后缀文件（区域照常生效） */
function tplShortName(name) {
  return name.replace(/#[^#./\\]+(\.(png|jpe?g))$/i, '$1')
}

/** 模板名区域徽标文本：半区码直接显示码字（l/r/dr…），数字坐标显示 ◧（悬停看全名） */
function tplRegionBadge(name) {
  const base = name.replace(/\.(png|jpe?g)$/i, '')
  const idx = base.lastIndexOf('#')
  if (idx < 0) return ''
  const s = base.slice(idx + 1).toLowerCase()
  if (['a', 'u', 'd', 'l', 'r', 'ul', 'ur', 'dl', 'dr'].includes(s)) return s
  if (/^\d{1,3}(_\d{1,3}){3}$/.test(s)) return '◧'
  return ''
}

/** 模板名半区代码 → 设备像素搜索区域 [x, y, w, h] */
function regionCodePixels(code, vw, vh) {
  const hw = Math.round(vw / 2)
  const hh = Math.round(vh / 2)
  const map = {
    a: null,
    u: [0, 0, vw, hh],
    d: [0, vh - hh, vw, hh],
    l: [0, 0, hw, vh],
    r: [vw - hw, 0, hw, vh],
    ul: [0, 0, hw, hh],
    ur: [vw - hw, 0, hw, hh],
    dl: [0, vh - hh, hw, hh],
    dr: [vw - hw, vh - hh, hw, hh]
  }
  return map[code] ?? null
}

/** 测试匹配的搜索区域：下拉框手动选择优先，否则按模板名自动识别
 *  （#x1_y1_x2_y2 → 对应矩形区域；#l/#r/... → 对应半区；无 → 全屏） */
function templateRegionPixels(name) {
  // 实际视频尺寸优先：虚拟屏分辨率/方向会被游戏改变，设备配置里的 width/height 可能过期
  const vw = videoElement.value?.videoWidth || current.value?.width || 1920
  const vh = videoElement.value?.videoHeight || current.value?.height || 1080
  if (testRegion.value) return regionCodePixels(testRegion.value, vw, vh)
  const nums = parseTplRegion(name)
  if (nums) {
    const x = Math.round(nums[0] * vw)
    const y = Math.round(nums[1] * vh)
    const w = Math.round((nums[2] - nums[0]) * vw)
    const h = Math.round((nums[3] - nums[1]) * vh)
    return [x, y, w, h]
  }
  const code = parseTplRegionCode(name)
  if (code) return regionCodePixels(code, vw, vh)
  return null
}




/** 退出编辑（脏模型需确认丢弃）；若处于跳转栈中先返回上一资源。
 *  录制忙（录制中/停止中/有未完成草稿）时阻止退出——占位与草稿锚定当前模型（plan §11.3）。
 *  注意 shell 是 reactive 包装：ref/computed 属性访问即解包，不能再取 .value */
async function cancelEditScript() {
  if (recording.busy) return toast('录制未完成：请先停止录制，并重试或丢弃未完成草稿', 'warn')
  if (scriptShell.hasModel && scriptShell.dirty && !window.confirm('有未保存修改，确认放弃？')) return
  if (scriptShell.canJumpBack) {
    await jumpBack()
    return
  }
  scriptShell.reset()
  scriptMode.value = 'run'
  showYaml.value = false
}

/** 新建脚本：空 ScriptModel（保存时落盘到当前应用分区） */
function startNewScript() {
  if (!activePkg.value) return toast('请先选择应用分区（设备页签配置应用包名）', 'warn')
  scriptMode.value = 'edit'
  showYaml.value = false
  scriptShell.newScript({ name: '新脚本.yml', pkg: activePkg.value })
}

/** 编辑当前选择（运行区资源类型分发）：脚本 = 脚本编辑上下文；函数 = 函数库编辑上下文 */
async function editCurrentTarget() {
  if (runKind.value !== 'func') return editCurrentScript()
  const f = fnLib.list.find(x => x.id === selFnFile.value)
  if (!f) return toast('请先选择函数库文件', 'error')
  scriptMode.value = 'edit'
  showYaml.value = false
  try {
    await scriptShell.loadFunctionFile(f.id)
  } catch (e) {
    scriptShell.reset()
    scriptMode.value = 'run'
    toast('函数库加载失败：' + e.message, 'error')
  }
}

/** 新建当前类型（运行区「更多」）：脚本 = 新建脚本；函数 = 新建函数库文件 */
function startNewTarget() {
  if (runKind.value !== 'func') return startNewScript()
  if (!activePkg.value) return toast('请先选择应用分区（设备页签配置应用包名）', 'warn')
  const raw = window.prompt('函数库文件短名（缺省 .yaml 自动补）', 'functions')
  if (!raw || !raw.trim()) return
  scriptMode.value = 'edit'
  showYaml.value = false
  scriptShell.newFunctionFile({ file: raw.trim(), pkg: activePkg.value })
}

/** 删除当前选择（运行区「更多」）：脚本 / 函数库文件 */
async function deleteCurrentTarget() {
  if (runKind.value !== 'func') return deleteCurrentScript()
  const f = fnLib.list.find(x => x.id === selFnFile.value)
  if (!f) return toast('请先选择函数库文件', 'error')
  if (!window.confirm(`删除函数库文件 ${f.file}？（引用它的 func 步骤将失效）`)) return
  try {
    await api.deleteFunction(f.id)
    await fnLib.refresh(activePkg.value)
    if (selFnFile.value === f.id) selFnFile.value = ''
    toast('函数库文件已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

/** 运行模式：编辑当前选中的脚本（getScript 读取最新内容与版本短码） */
async function editCurrentScript() {
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return toast('请先选择脚本', 'error')
  scriptMode.value = 'edit'
  showYaml.value = false
  try {
    await scriptShell.loadScript(s.id)
  } catch (e) {
    scriptShell.reset()
    scriptMode.value = 'run'
    toast('脚本加载失败：' + e.message, 'error')
  }
}

/** 运行模式：删除当前选中的脚本 */
async function deleteCurrentScript() {
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return toast('请先选择脚本', 'error')
  if (!confirm(`删除脚本 ${s.name}？`)) return
  try {
    await api.deleteScript(s.id)
    await loadData()
    clearCallParamsCache()
    if (selScript.value === s.id) selScript.value = ''
    toast('脚本已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

// ---------- 更多菜单（新建 / 删除）；导入/导出在脚本页签顶部应用下拉旁 ----------
const moreOpen = ref(false)

/** 导出当前应用分区快照（yaml/ + tmpl/ 全量）→ zip 下载 */
async function exportPartition() {
  if (!activePkg.value) return toast('请先选择应用分区', 'warn')
  try {
    const { blob, filename } = await api.exportPartition(activePkg.value)
    const name = filename || `${activePkg.value}.zip`
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = name
    a.click()
    setTimeout(() => URL.revokeObjectURL(a.href), 5000)
    toast(`已导出 ${name}`, 'success')
  } catch (e) {
    toast('导出失败：' + e.message, 'error')
  }
}

/** 导入分区快照 zip（模板+脚本）到当前应用分区：先 dry-run 解析报告；invalid 条目直接阻止
 *  （服务端 confirm 模式遇任一非法文件整体拒绝），同名覆盖弹二次确认后 confirm 落盘。
 *  流程实现抽离至 api.js runPartitionImport（依赖注入，node 单测覆盖）。 */
async function onImportFile(e) {
  const file = e.target.files?.[0]
  e.target.value = ''
  if (!file) return
  if (!activePkg.value) return toast('请先选择应用分区', 'warn')
  await runPartitionImport({
    file,
    pkg: activePkg.value,
    importScripts: api.importScripts,
    confirmDialog: msg => window.confirm(msg),
    notify: toast,
    refresh: loadData,
  })
}

/** 脚本校验（结构化字段级）由 useScriptEditorShell.diagnostics 提供（validateScript + 解析期诊断） */

/** 保存编辑中的脚本：shell.save() 序列化模型并携带 expected_version；
 *  校验失败 → 提示前 3 条诊断；409 version_conflict → shell.conflict 置位，SaveConflictModal 弹出。
 *  录制忙时阻断保存直至队列排空（stopping 中同样拦截，plan §11.3） */
async function saveEditScript() {
  if (!scriptShell.hasModel) return
  if (recording.busy) return toast(recording.phase === 'stopping' ? '停止中：等待录制队列排空后再保存' : '录制尚未结束：请先停止录制', 'warn')
  if (!String(scriptShell.name || '').trim()) return toast('请填写脚本名称', 'error')
  if (!scriptShell.pkg && !activePkg.value) return toast('请先选择应用分区', 'warn')
  const r = await scriptShell.save()
  if (r.ok) {
    clearCallParamsCache()
    await afterScriptSaved(r.result)
  } else if (r.reason === 'invalid') {
    toast('校验未通过：' + r.diagnostics.slice(0, 3).map(d => d.message).join('；'), 'error')
  } else if (r.reason === 'conflict') {
    // shell.conflict 已置位，弹窗由 ScriptRunner 渲染（重载 / 覆盖）
  } else {
    toast('保存失败：' + (r.error?.message || r.error), 'error')
  }
}

// ---------- 自动保存（编辑区失焦即存）：600ms 防抖合并连续失焦；成功静默，
// 校验不通过 / 版本冲突 / 失败 toast 提示（不弹冲突窗、不退出编辑态） ----------
let autoSaveTimer = null
function autoSaveDebounced() {
  if (scriptMode.value !== 'edit' || !scriptShell.hasModel) return
  if (autoSaveTimer) clearTimeout(autoSaveTimer)
  autoSaveTimer = setTimeout(autoSave, 600)
}
async function autoSave() {
  autoSaveTimer = null
  if (scriptMode.value !== 'edit' || !scriptShell.hasModel || !scriptShell.dirty || scriptShell.saving) return
  if (recording.busy) return // 录制队列排空前不落盘（与手动保存同一闸门）
  const wasNew = !scriptShell.resourceId
  const r = await scriptShell.save({ suppressConflict: true })
  if (r.ok) {
    clearCallParamsCache()
    // 函数库落盘后刷新文件清单（call/func 下拉与运行区函数下拉共用）；
    // 新建脚本落盘后刷新脚本列表（call 目标下拉候选）
    if (scriptShell.kind === 'function_library') await fnLib.refresh(activePkg.value)
    else if (wasNew) await loadData()
    if (wasNew) selScript.value = scriptShell.resourceId // 首次落盘：运行区选择跟随
  } else if (r.reason === 'invalid') {
    toast('自动保存未通过：' + (r.diagnostics?.[0]?.message || '存在校验问题'), 'warn')
  } else if (r.reason === 'conflict') {
    toast('自动保存遇到版本冲突，请点「💾 保存」手动处理', 'warn')
  } else if (r.reason !== 'empty') {
    toast('自动保存失败：' + (r.error?.message || '未知错误'), 'warn')
  }
}

/** 保存成功后置：刷新列表、选中保存后的资源（脚本/函数库文件）、退出编辑回到运行视图 */
async function afterScriptSaved(rep) {
  await loadData()
  if (rep?.id) {
    if (scriptShell.kind === 'function_library') {
      runKind.value = 'func'
      selFnFile.value = rep.id
      await fnLib.refresh(activePkg.value)
    } else {
      selScript.value = rep.id
    }
  }
  scriptShell.reset()
  scriptMode.value = 'run'
  showYaml.value = false
  toast('脚本已保存', 'success')
}

/** 409 冲突弹窗：重载磁盘版本（放弃本地修改） */
async function onConflictReload() {
  try {
    const r = await scriptShell.reload()
    if (r.ok) toast('已恢复磁盘版本', 'success')
  } catch (e) {
    toast('重载失败：' + e.message, 'error')
  }
}

/** 409 冲突弹窗：强制覆盖（不带 expected_version 重存），成功后同保存收尾 */
async function onConflictOverwrite() {
  const r = await scriptShell.overwrite()
  if (r.ok) await afterScriptSaved(r.result)
  else if (r.reason === 'error') toast('覆盖失败：' + (r.error?.message || r.error), 'error')
}

/** 409 冲突弹窗：关闭（留在编辑态，可继续改后重试保存） */
function onConflictDismiss() {
  scriptShell.dismissConflict()
}

// 运行状态轮询：以当前 run_id 单次查询 GET /api/runs/:run_id，
// 按 record.state 驱动状态机（stopping→停止中、终态→复位空闲并归档）。
let runStatusTimer = null

function startRunStatusPoll() {
  if (runStatusTimer) clearInterval(runStatusTimer)
  checkRunStatus()
  runStatusTimer = setInterval(checkRunStatus, 1000)
}

function stopRunStatusPoll() {
  if (runStatusTimer) { clearInterval(runStatusTimer); runStatusTimer = null }
}

async function checkRunStatus() {
  if (!store.running) { stopRunStatusPoll(); return }
  const rid = store.runId
  if (!rid) { stopRunStatusPoll(); resetStoreRunState(); return }
  let rec
  try {
    rec = await api.getRun(rid)
  } catch (e) { return } // 网络抖动等：下轮再试，不提前复位运行态
  const m = applyRunRecord(rec)
  if (m && isTerminalRunState(m.state)) {
    stopRunStatusPoll()
    const detail = `：${terminalLabel(m.state)}${m.error ? `（${m.error}）` : ''}`
    toast(`脚本已结束${detail}`, m.state === 'success' ? 'info' : 'warn')
  }
}

// ---------- 运行模式：只读步骤摘要 + 从此步骤运行（plan §10「只读源码展示/从某行运行」行） ----------
// 非编辑态不再展示源码文本：选中脚本解析为 ScriptModel，ScriptSummary 逐顶层卡片给出
// 图标 + 中文动作名 + 自然语言摘要；运行起点只经卡片「▶ 从此运行」直发（2026-08-30 用户
// 决策：去掉点击卡片选中/取消，顶部「运行」按钮恒从头跑）。解析失败（旧语法残留等）→
// summaryError 提示，主视图不给摘要。
const summaryModel = computed(() => {
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return null
  try {
    return parseScript(s.content ?? '').model
  } catch {
    return null
  }
})
const summaryError = computed(() => {
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return ''
  if (!summaryModel.value) return '脚本解析失败（可能含旧语法），请进编辑态查看诊断'
  return ''
})

/** 摘要卡片「▶ 从此运行」：直接以该步骤为起点启动（不选中、不留驻，起点即发即用） */
function runFromStep(uuid) {
  return runScript({ fromUuid: uuid })
}

// ---------- 结构化跳转（plan §10「调用文本链接预览」行：正则扫描源码 → 结构化引用） ----------

/** call 步骤目标（短名或含扩展名）→ 同分区脚本 id（缺扩展名自动补全，与引擎一致） */
function resolveCallTargetId(target) {
  const raw = String(target || '').trim()
  if (!raw) return null
  const names = [raw]
  if (!/\.(ya?ml)$/i.test(raw)) names.push(`${raw}.yaml`, `${raw}.yml`)
  for (const n of names) {
    const hit = scripts.value.find(x => x.package === activePkg.value && x.name === n)
    if (hit) return hit.id
  }
  return null
}

/** 摘要 call/func 卡片「↗ 打开子脚本/函数定义」：进入编辑态并把目标载入共享外壳。
 *  录制忙时阻止（同 jumpBack：跳转会整体替换模型） */
async function openScriptTarget({ kind, target }) {
  if (recording.busy) return toast('录制中不能跳转资源', 'warn')
  const id = kind === 'call' ? resolveCallTargetId(target) : fnLib.resolveTargetId(target)
  if (!id) return toast(`跳转目标不存在：${target}`, 'warn')
  scriptMode.value = 'edit'
  showYaml.value = false
  try {
    if (kind === 'call') await scriptShell.jumpToScript(id)
    else await scriptShell.jumpToFunctionFile(id)
  } catch (e) {
    scriptShell.reset()
    scriptMode.value = 'run'
    toast('目标加载失败：' + e.message, 'error')
  }
}

/** 编辑态跳转返回（call/func 打开目标后）：载回上一资源；栈空时按钮不显示。
 *  录制忙时阻止（跳转会整体替换模型，录制占位会丢失） */
async function jumpBack() {
  if (recording.busy) return toast('录制中不能切换资源', 'warn')
  try {
    await scriptShell.jumpBack()
  } catch (e) {
    toast('返回失败：' + e.message, 'error')
  }
}

// 启动提交中（202 快速返回前的防重复点击位）；run_id 在启动成功那一刻即登记为主键
const startPending = ref(false)
// 当前展示实例是否处于 stopping（cancel 已发、终态未达）：停止按钮转为禁用「停止中…」，
// 避免旧实现立即回空闲导致可再次点运行与停"两个实例"交叠
const runStopping = computed(() => {
  const rec = store.runId ? findRun(store.runId) : null
  return !!rec && rec.state === 'stopping'
})

/** 设备占用冲突（409 device_busy）：入队弹窗展示对方脚本/来源/本地化开始时间，
 *  提供「仍要查看日志」跳控制台对应设备；不打断本页其他功能 */
function openRunConflict(d) {
  console.warn('[run] device busy (409)', d)
  pushRunConflict({ ...(d || {}), device_id: store.deviceId })
}

// ---------- 运行参数流程（阶段 5）：脚本声明 params 时先弹参数表单，稀疏 args 提交 ----------
// exec 完成 API 调用与 run_id 登记；flow 负责表单开关/400 诊断回填字段/覆盖建议缓存/摘要
const runArgsFlow = useRunArgsFlow({
  exec: async ({ id, name, kind, fnName, startIndex, args }) => {
    startPending.value = true
    // 每次运行清空日志区域，只显示本次运行产生的日志
    runStartTime = Date.now()
    rawLogs = []
    liveLogs.value = []
    try {
      // 函数模式（运行区资源类型=函数）：走函数测试接口运行单个函数
      const rep = kind === 'function_library'
        ? await api.runFunction(id, store.deviceId, { function: fnName || undefined, start_index: startIndex, args })
        : await api.runScript(id, store.deviceId, startIndex, args)
      // 当前运行响应固定含 run_id；启动即登记实例，后续查询只按该主键进行。
      applyRunRecord({ ...rep, device_id: store.deviceId, script_id: id, source: 'manual', display: name })
      return rep
    } finally {
      startPending.value = false
    }
  },
  notify: ({ summary }) => {
    toast('脚本已开始运行', 'success')
    // resolved_args 摘要（默认继承/显式覆盖来源标注）进运行日志区，说明本次实际使用的参数
    if (summary) pushLog('info', summary)
    // POST 成功（服务端已登记条目）后才开始轮询，避免设备离线时 connect_device 耗时较长、
    // 查询先于登记返回导致状态被提前复位
    startLogPolling()
    startRunStatusPoll()
  },
})

/** 运行启动失败统一处理：409 设备占用 → 冲突弹窗；其余写日志 + toast（400 诊断由 flow 消化不经过此） */
function handleRunStartError(e) {
  if (isDeviceBusyConflict(e)) {
    openRunConflict({ ...(e.data || {}), device_id: store.deviceId })
  } else {
    pushLog('error', `执行失败：${e.message}`)
    toast('脚本执行失败', 'error')
  }
}

/** 运行/从此步骤运行入口：解析当前脚本 params → 无参数直接运行，有参数弹参数表单；
 *  函数模式：按函数 params 弹表单，经函数测试接口运行所选函数（缺省 = 文件第一个函数）。
 *  opts.fromUuid（从此运行）→ 脚本取顶层 steps 序号 / 函数定位目标函数与步序；
 *  顶部「运行」按钮不传 → 从头跑。守卫失败一律 toast 说明原因，不做静默 no-op */
async function runScript(opts = {}) {
  if (startPending.value || store.running) return
  if (!store.deviceId) return toast('请先在上方选择设备再运行', 'warn')
  if (runKind.value === 'func') {
    const f = fnLib.list.find(x => x.id === selFnFile.value)
    if (!f) return toast('请先选择函数库文件', 'warn')
    // 运行目标：从此运行落在某函数的某步 → 该函数从该步；顶部运行 → 第一个函数从头
    let fnName = (f.functions || [])[0] || ''
    let startIndex = 0
    if (opts.fromUuid && funcParsed.value) {
      for (const fn of funcParsed.value.functions) {
        const idx = fn.steps.findIndex(s => s.uuid === opts.fromUuid)
        if (idx >= 0) { fnName = fn.name; startIndex = idx; break }
      }
    }
    if (!fnName) return toast('该函数库文件没有可运行的函数', 'warn')
    try {
      await runArgsFlow.begin({
        id: f.id,
        name: `${f.file} · ${fnName}()`,
        kind: 'function_library',
        fnName,
        yaml: f.content ?? '',
        startIndex,
        templates: templateNames.value,
        title: '函数参数',
        submitLabel: '▶ 运行',
        desc: `运行函数 ${f.file}/${fnName}()${startIndex ? `（从第 ${startIndex + 1} 步）` : ''}`,
      })
    } catch (e) {
      handleRunStartError(e)
    }
    return
  }
  if (!selScript.value || !scripts.value.find(x => x.id === selScript.value)) return toast('请先选择脚本', 'warn')
  const s = scripts.value.find(x => x.id === selScript.value)
  // 运行起点：从此运行 → 顶层 steps 序号（找不到回退 0 从头跑）；顶部运行 → 从头
  const startIndex = opts.fromUuid && summaryModel.value
    ? (startIndexOf(summaryModel.value, opts.fromUuid) ?? 0)
    : 0
  try {
    await runArgsFlow.begin({
      id: s.id,
      name: s.name,
      yaml: s.content ?? '',
      startIndex,
      templates: templateNames.value,
      desc: `运行脚本 ${s.name}${startIndex ? `（从第 ${startIndex + 1} 步）` : ''}`,
    })
  } catch (e) {
    handleRunStartError(e)
  }
}

/** RunParamsModal 提交（客户端校验已过）：稀疏 args 提交；400 invalid_args 由 flow 回填表单标红 */
function onRunArgsSubmit({ args }) {
  runArgsFlow.confirm(args).catch(handleRunStartError)
}

function stopScript() {
  // 取消只按当前 run_id 寻址；本地先行迁 stopping，终态以轮询为准。
  const rid = store.runId
  if (!rid) return
  beginCancel(rid)
  api.cancelRun(rid).catch(e => {
    pushLog('error', `停止失败：${e.message}`)
    toast('停止失败：' + e.message, 'error')
  })
  pushLog('warn', '已发送停止指令，等待脚本退出…')
  toast('已发送停止指令', 'warn')
}

async function testMatch(name) {
  if (!connected.value) return toast('请先连接设备', 'error')
  if (hitTimer) { clearTimeout(hitTimer); hitTimer = null }
  showHit.value = false
  try {
    const region = templateRegionPixels(name)
    const r = await api.testTemplate(name, store.deviceId, Number(testThreshold.value) || 0.8, region, activePkg.value)
    if (r.hit) {
      hit.x = r.x; hit.y = r.y; hit.w = r.width; hit.h = r.height
      hitLabel.value = `${name} ${r.score.toFixed(2)}`
      hitMiss.value = false
      showHit.value = true
      // 匹配框只展示 3 秒，避免一直留在画面上
      hitTimer = setTimeout(() => { showHit.value = false }, 3000)
      toast(`匹配成功：${name} 置信度 ${r.score.toFixed(2)}`, 'success')
    } else {
      // 未命中也画框：显示本次搜索区域（与引擎 miss 可视化同语义，便于发现区域配错）
      const vw2 = videoElement.value?.videoWidth || current.value?.width || 1920
      const vh2 = videoElement.value?.videoHeight || current.value?.height || 1080
      const [rx, ry, rw2, rh2] = region || [0, 0, vw2, vh2]
      hit.x = rx; hit.y = ry; hit.w = rw2; hit.h = rh2
      hitLabel.value = `${name} 未命中`
      hitMiss.value = true
      showHit.value = true
      hitTimer = setTimeout(() => { showHit.value = false }, 3000)
      toast(`未找到：${name}`, 'warn')
    }
  } catch (e) {
    toast('匹配失败：' + e.message, 'error')
  }
}

function fullscreen() {
  if (videoWrap.value?.requestFullscreen) videoWrap.value.requestFullscreen()
}

// 视觉组件只接收这两个上下文对象；所有状态、动作和清理仍由 Console 统一持有。
function onCropMounted({ canvas, section }) {
  cropCanvas.value = canvas
  cropSec.value = section
  if (crop.active) {
    renderCropFrame()
    refreshCropPreview()
  }
}
function onLogBoxMounted(el) { logBox.value = el }
function setRenameInputEl(el) { renameInputEl = el }

const deviceSettingsContext = {
  settingsOpen, mode, form, types, vdPresets, fpsPresets, formDirty,
  pkgDraft, appOpen, appFiltered, appLoading, loadApps, commitPkg, pickApp, appHint,
  configApplying, saveSettings, cancelSettings, current, connected, kindInfo, screenSummary,
}
const templateCaptureContext = {
  activePkg, pkgOptions, exportPartition, onImportFile, crop, testThreshold, testRegion, tplSearch,
  picking, connected, togglePick, templates, confirmDelTpl, renaming, onTplRowClick, onTplThumbClick,
  tplThumbUrl, onTplNameClick, setRenameInputEl, renameVal, confirmRename, cancelRename, startRename,
  onTplDeleteClick, onTplMatchClick, tplShortName, tplRegionBadge, cropSize, cropZoomPct,
  cropMouseDown, cropMouseMove, cropMouseUp, cropMouseLeave, cropWheel, saveTemplate, cancelCrop,
  repick, saving, viewTpl, closeTplView, replaceTemplateImage,
}
const scriptRunnerContext = {
  scriptMode, selScript, activePkg, store, startPending, runScript, runStopping, stopScript,
  // 运行区资源类型（脚本/函数）；Target 系列按类型分发编辑/新建/删除
  runKind, selFnFile, canRunTarget, selTargetId, fnLib, autoSaveDebounced,
  // 函数模式摘要：逐函数分组视图（每组一个 ScriptSummary）+ 解析失败文案
  funcFnViews, funcSummaryError,
  editCurrentTarget, startNewTarget, deleteCurrentTarget,
  editCurrentScript, moreOpen, startNewScript, deleteCurrentScript, liveLogs, onLogBoxMounted,
  // 运行视图：只读摘要 + 运行起点 + call/func 结构化跳转（替代旧源码行点击/文本预览）
  summaryModel, summaryError, runFromStep, openScriptTarget,
  // 运行参数表单（阶段 5）：脚本声明 params 时点运行/从此运行弹出
  runArgsFlow, onRunArgsSubmit,
  // 编辑视图：共享编辑器外壳 + 保存/取消/409 冲突回调 + Alt 录制开关
  shell: scriptShell, saveEditScript, cancelEditScript,
  showYaml, templateNames, jumpBack,
  onConflictReload, onConflictOverwrite, onConflictDismiss,
  // call/func 目标实参类型回显（同步缓存命中形态），ScriptRunner 经 ctx 传给画布
  resolveTargetSync,
  // 录制（阶段 6）：上传进行中锁画布（占位不可跨分支拖动）+ 投屏 Alt 暂停提示
  recording,
}

/** 关页保护：录制忙（录制/停止中/未完成草稿）或有未保存修改时浏览器弹出确认 */
function onBeforeUnload(e) {
  if (recording.busy || (scriptMode.value === 'edit' && scriptShell.hasModel && scriptShell.dirty)) {
    e.preventDefault()
    e.returnValue = ''
  }
}

onMounted(async () => {
  // SPA 内跳转（store 存活）→ 自动重连恢复画面；页面刷新 → localStorage 恢复设备选择；
  // 首次进入仅选中第一台设备，等待用户点连接（不主动建会话，尊重空闲低功耗）
  const spaPreselected = !!store.deviceId
  await loadData()
  if (!store.deviceId) {
    const saved = localStorage.getItem('gb_device_id')
    store.deviceId = (saved && devices.value.find(d => d.id === saved)) ? saved : (devices.value[0]?.id || null)
  }
  const d = current.value
  if (d) loadForm(d)
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
})

/** 页面刷新 / 设备列表就绪后恢复该设备的活动 run：
 * GET /api/devices/:id/run → {active:true,run:RunRecord}；无活动/请求失败静默跳过。 */
async function restoreRunState() {
  if (!store.deviceId || store.running) return
  let rep = null
  try {
    rep = await api.deviceRun(store.deviceId)
  } catch (e) { /* 恢复失败不影响进入页面 */ return }
  if (!rep.active) return // {active:false}：无活动 run，保持空闲展示
  const rec = rep.run
  if (!rec?.run_id) return
  const s = scripts.value.find(x => x.id === rec.script_id)
  const baseName = rec.script_name || s?.name || rec.script_id
  const srcTag = sourceLabel(rec.source)
  applyRunRecord({ ...rec, device_id: store.deviceId, display: srcTag ? `${baseName}（${srcTag}）` : baseName })
  selScript.value = rec.script_id
  scriptMode.value = 'run'
  runStartTime = 0   // 不按开始时间过滤，恢复最近日志
  startLogPolling()
  startRunStatusPoll()
  toast(`检测到 ${baseName}${srcTag ? `（${srcTag}）` : ''} 正在运行，已恢复状态`, 'info')
}

// 设备选择持久化：刷新后自动恢复选中设备（运行态/画面恢复的前提）
watch(() => store.deviceId, id => {
  if (id) localStorage.setItem('gb_device_id', id)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onGlobalKeydown)
  window.removeEventListener('beforeunload', onBeforeUnload)
  consoleRuntime.cancelReconnect()
  if (appHintTimer) { clearTimeout(appHintTimer); appHintTimer = null }
  if (hitTimer) { clearTimeout(hitTimer); hitTimer = null }
  if (fxTapTimer) { clearTimeout(fxTapTimer); fxTapTimer = null }
  if (fxSwipeTimer) { clearTimeout(fxSwipeTimer); fxSwipeTimer = null }
  if (fxHitTimer) { clearTimeout(fxHitTimer); fxHitTimer = null }
  if (autoSaveTimer) { clearTimeout(autoSaveTimer); autoSaveTimer = null }
  stopRunStatusPoll()
  cleanup(true)
})
</script>

<style scoped>
.console {
  display: flex; height: 100%; padding: 14px; gap: 14px;
  /* 侧边栏收起时释放的宽度（展开 200px - 收起 52px，见 MainLayout.vue） */
  --sb-free-w: 148px;
}

/* ===== 画面区 ===== */
.stage { flex: 1; display: flex; flex-direction: column; gap: 10px; min-width: 0; position: relative; }
.app-hint {
  position: absolute; top: 8px; left: 50%; transform: translateX(-50%); z-index: 6;
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

/* 工具条：两行布局（上行设备管理 / 下行投屏控制），行内各自 wrap 不出横向滚动 */
.toolbar {
  display: flex; flex-direction: column; gap: 6px;
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 8px 10px;
  box-sizing: border-box;
}
.tb-row {
  display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
  min-height: 29px; /* 单行高度（按钮 29px），wrap 换行后自然撑开 */
}
.tb-sep { width: 1px; height: 22px; background: var(--border); margin: 0 4px; }
.btn.active { border-color: var(--accent-2); color: var(--accent-2); }
.btn.rec-btn { border-color: var(--danger); color: var(--danger); }

/* ===== 录制状态栏（plan §10.1：录制状态/插入位置/待处理数量/停止按钮） ===== */
.rec-bar {
  flex: none; display: flex; align-items: center; gap: 10px; flex-wrap: wrap;
  background: rgba(239,68,68,.08); border: 1px solid rgba(239,68,68,.4);
  border-radius: var(--radius); padding: 6px 10px;
}
.rec-bar strong { font-size: 12px; color: var(--danger); }
.rec-bar .mono { font-size: 11px; color: var(--text-1); }
.rec-bar-err { font-size: 11px; color: var(--danger); font-weight: 600; }
.rec-alt-hint { color: var(--text-2); }
.rec-dot {
  width: 10px; height: 10px; border-radius: 50%; background: var(--danger);
  animation: rec-blink 1.2s ease-in-out infinite; flex-shrink: 0;
}
.rec-dot.stopping { background: var(--warn); animation: none; }
@keyframes rec-blink { 0%, 100% { opacity: 1; } 50% { opacity: .25; } }
.rec-bar .btn { margin-left: auto; }

/* ===== 右侧面板 ===== */
.panel {
  width: 340px; flex-shrink: 0; display: flex; flex-direction: column; gap: 10px;
  overflow: hidden; transition: width .18s ease;
}
/* 侧边栏收起：释放宽度全部给右侧操作区（340 + 148），中间投屏区宽度保持不变 */
.console.sb-collapsed .panel { width: calc(340px + var(--sb-free-w)); }

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
.func-pkg { flex-shrink: 0; font-size: 12px; }
.func-tabs { display: flex; flex-shrink: 0; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden; background: var(--bg-2); }
.func-tabs button {
  flex: 1; padding: 7px 0; font-size: 12px; text-align: center; cursor: pointer;
  border: none; background: transparent; color: var(--text-1);
}
.func-tabs button + button { border-left: 1px solid var(--border); }
.func-tabs button.active {
  color: var(--accent); background: rgba(34, 211, 165, .14); font-weight: 600;
}
/* 应用分区下拉：模板/脚本两页签共用，数据随分区切换（默认跟随设备页签的应用包名） */
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
.op-record {
  flex-shrink: 0; height: 77px; display: flex; flex-direction: column;
  background: var(--bg-0); border: 1px solid var(--border);
  border-radius: var(--radius-sm); padding: 3px; overflow: hidden;
}
.op-record-line {
  flex: 0 0 auto; height: 23px; display: flex; align-items: center; padding: 0 8px;
  font-size: 11px; line-height: 1.4; color: var(--text-1); cursor: pointer;
  border-radius: 4px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
.op-record-line:hover { background: var(--bg-3); color: var(--accent); }
.op-record-empty {
  height: 100%; display: flex; align-items: center; justify-content: center;
  font-size: 11px; color: var(--text-2); text-align: center; padding: 0 8px;
}
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
