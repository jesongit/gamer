<template>
  <div class="console">
    <!-- 左：画面区 -->
    <div class="stage">
      <!-- 顶部工具条 -->
      <div class="toolbar">
        <button class="btn btn-sm" @click="shot">📷 截图</button>
        <button class="btn btn-sm" @click="rotate">🔄 旋转</button>
        <button class="btn btn-sm" @click="key('HOME')">🏠 Home</button>
        <button class="btn btn-sm" @click="key('BACK')">⬅ 返回</button>
        <button class="btn btn-sm" @click="key('APP_SWITCH')">🪟 最近</button>
        <button class="btn btn-sm" @click="key('VOL_UP')">🔊＋</button>
        <button class="btn btn-sm" @click="key('VOL_DOWN')">🔊－</button>
        <button class="btn btn-sm" @click="toggleAudio" :title="audioMuted ? '取消静音（听游戏声音）' : '静音'">{{ audioMuted ? '🔇' : '🔊' }}</button>
        <button class="btn btn-sm" @click="launchGame" :title="'启动到虚拟屏：' + (currentPkg || '未配置包名')">🚀 启动游戏</button>
        <div class="tb-sep"></div>
        <button class="btn btn-sm" :class="{ active: picking }" @click="togglePick">✂️ 框选模板</button>
        <button class="btn btn-sm" @click="clipboard">📋 剪贴板</button>
        <span class="tb-tip">鼠标左键=触控 · 滚轮=滑动 · 支持多点触控</span>
      </div>

      <div class="video-wrap" ref="videoWrap">
        <video
          ref="videoElement"
          autoplay
          playsinline
          :muted="audioMuted"
          class="video-stream"
          @mousedown="onMouseDown"
          @mousemove="onMouseMove"
          @mouseup="onMouseUp"
          @wheel.prevent="onWheel"
          @contextmenu.prevent
          @mouseleave="hideLoupe"
        ></video>

        <!-- 找图命中框演示（模板测试） -->
        <div v-if="showHit" class="hit-box" :style="hitStyle">
          <span class="hit-label">{{ hitLabel }}</span>
        </div>

        <!-- 框选模板 -->
        <div v-if="selecting" class="select-box" :style="selStyle"></div>

        <!-- 放大预览镜 -->
        <div class="loupe" v-show="loupe.show" :style="{ left: loupe.x + 'px', top: loupe.y + 'px' }">
          <canvas ref="loupeCanvas" width="300" height="300"></canvas>
          <span class="loupe-tag mono">{{ loupe.zoom }}×</span>
        </div>

        <div class="v-overlay" v-if="!connected">
          <div class="v-connecting" v-if="connecting">
            <span class="dot run"></span> 正在建立 WebRTC 连接…
          </div>
          <div v-else>
            <div class="v-empty-icon">📴</div>
            <div class="v-empty-text">{{ errorMsg || '未连接设备' }}</div>
            <button class="btn btn-primary" @click="connect(true)">连接 {{ currentName }}</button>
          </div>
        </div>

        <div class="v-stats" v-if="connected">
          <span class="st">{{ fps }} fps</span>
          <span class="st">延迟 {{ delay }}ms</span>
          <span class="st">{{ res }}</span>
          <span class="st">H.264 · WebRTC</span>
        </div>

        <button class="v-fs" @click="fullscreen" title="全屏">⛶</button>
      </div>
    </div>

    <!-- 右：控制面板（页签切换） -->
    <aside class="panel">
      <div class="panel-tabs">
        <button v-for="t in tabs" :key="t.key" class="tab-btn" :class="{ active: activeTab === t.key, hidden: t.key === 'crop' && !crop.active }" @click="activeTab = t.key">
          {{ t.icon }}<span class="tab-label">{{ t.label }}</span>
        </button>
      </div>

      <div class="tab-body">
        <!-- 设备信息 -->
        <div v-show="activeTab === 'info'" class="panel-sec">
          <div class="ps-head">
            <span class="dot" :class="connected ? 'ok' : 'off'"></span>
            <span class="ps-title">{{ currentName }}</span>
            <span class="tag" :class="connected ? 'info' : ''">{{ connected ? '已连接' : '离线' }}</span>
          </div>
          <div class="ps-sub mono">{{ currentAddr }}</div>
          <div class="ps-sub" v-if="connected">
            🖥️ {{ currentScreenMode === 'virtual' ? `虚拟屏 ${currentVdRes} · 模板通用分辨率` : '镜像主屏' }}
          </div>
          <div class="ps-stats" v-if="connected">
            <span class="st">🖥️ {{ res }}</span>
            <span class="st">{{ fps }} fps</span>
            <span class="st">延迟 {{ delay }}ms</span>
          </div>

          <!-- 配置修改（下拉框，选项与设备列表预设一致） -->
          <div class="cfg-box">
            <div class="form-item" v-if="currentScreenMode === 'virtual'">
              <label>分辨率</label>
              <select v-model="selRes" class="select mono" @change="applyDeviceConfig" :disabled="connecting || configApplying">
                <option v-for="v in vdResOptions" :key="v" :value="v">{{ v.replace('@', ' · ') }}dpi</option>
              </select>
            </div>
            <div class="form-item">
              <label>帧率</label>
              <select v-model="selFps" class="select mono" @change="applyDeviceConfig" :disabled="connecting || configApplying">
                <option :value="''">自动</option>
                <option v-for="f in fpsPresets" :key="f" :value="f">{{ f }} fps</option>
              </select>
            </div>
            <div class="cfg-hint">修改后自动重连设备生效</div>
          </div>
        </div>

        <!-- 自动化 -->
        <div v-show="activeTab === 'auto'" class="panel-sec">
          <div class="ps-head">
            <span class="ps-title">🤖 自动化</span>
            <button class="btn btn-sm btn-ghost" @click="openScripts">管理脚本 →</button>
          </div>
          <div class="auto-run">
            <select v-model="selScript" class="select mono">
              <option value="">选择要运行的脚本…</option>
              <option v-for="s in scripts" :key="s.id" :value="s.id">{{ s.name }}</option>
            </select>
            <button v-if="!store.running" class="btn btn-primary" :disabled="!selScript" @click="runScript">▶ 运行</button>
            <button v-else class="btn btn-danger" @click="stopScript">■ 停止</button>
          </div>

          <!-- 运行状态 -->
          <div v-if="store.running" class="run-progress">
            <div class="rp-head">
              <span class="mono rp-script">{{ store.runScript }}</span>
              <span class="rp-pct">{{ store.runProgress }}%</span>
            </div>
            <div class="rp-bar"><div class="rp-fill" :style="{ width: store.runProgress + '%' }"></div></div>
            <div class="rp-step mono">{{ store.runStep }}</div>
          </div>

          <!-- 实时日志 -->
          <div class="live-logs mono">
            <div v-for="(l, i) in liveLogs" :key="i" class="ll" :class="l.level">
              <span class="ll-time">{{ l.time }}</span>
              <span class="ll-msg">{{ l.msg }}</span>
            </div>
          </div>
        </div>

        <!-- 模板 -->
        <div v-show="activeTab === 'tpl'" class="panel-sec">
          <div class="ps-head"><span class="ps-title">🖼️ 模板</span></div>
          <div class="tpl-quick">
            <div v-for="t in templates" :key="t.name" class="tpl-chip" @click="testMatch(t.name)">
              <span class="tpl-thumb"><img :src="tplThumbUrl(t.name)" alt="" loading="lazy" @error="e => e.target.style.visibility = 'hidden'" /></span>
              <span>{{ t.name }}</span>
            </div>
          </div>
          <div class="ps-sub">点击模板 → 在当前画面测试匹配</div>
        </div>

        <!-- 二次裁切（框选后出现） -->
        <div v-show="activeTab === 'crop'" class="panel-sec" v-if="crop.active" ref="cropSec">
          <div class="ps-head">
            <span class="ps-title">✂️ 二次裁切</span>
            <span class="ps-sub mono">{{ cropSize }}</span>
          </div>
          <div class="crop-stage">
            <canvas ref="cropCanvas" class="crop-canvas" @mousedown="cropMouseDown" @mousemove="cropMouseMove" @mouseup="cropMouseUp" @mouseleave="cropMouseLeave"></canvas>
            <div class="crop-hint">拖动边框/角二次裁切（向外拖扩大范围）· 拖框内移动位置</div>
          </div>
          <input v-model="crop.name" class="input mono" placeholder="模板名称（默认自动生成）" @keydown.enter="saveTemplate" />
          <div class="crop-actions">
            <button class="btn btn-sm" @click="cancelCrop">取消</button>
            <button class="btn btn-sm btn-ghost" @click="repick">重新框选</button>
            <button class="btn btn-sm btn-primary" :disabled="saving" @click="saveTemplate">{{ saving ? '保存中…' : '💾 保存模板' }}</button>
          </div>
        </div>
      </div>
    </aside>
  </div>
</template>

<script setup>
import { ref, reactive, computed, watch, nextTick, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { store, devicesData, scriptsData, templatesData, useToast } from '../store'
import { api } from '../api'

const router = useRouter()
const toast = useToast()

const videoWrap = ref(null)
const videoElement = ref(null)

const connected = ref(false)
const connecting = ref(false)
const errorMsg = ref('')
const fps = ref(0)
const delay = ref(0)
const res = ref('—')
const selScript = ref('')
const picking = ref(false)
const selecting = ref(false)
const selStart = reactive({ x: 0, y: 0 })
const selEnd = reactive({ x: 0, y: 0 })
const showHit = ref(false)
const hit = reactive({ x: 0, y: 0, w: 0, h: 0 })
const hitLabel = ref('')
const liveLogs = ref([])
// 二次裁切（右侧面板）
const crop = reactive({ active: false, imgW: 0, imgH: 0, rect: { x: 0, y: 0, w: 0, h: 0 }, preview: '', name: '' })
const cropCanvas = ref(null)
const cropSec = ref(null)
const cropDrag = reactive({ mode: null, startX: 0, startY: 0, orig: null })
const saving = ref(false)
// 放大预览镜
const loupe = reactive({ show: false, x: 0, y: 0, zoom: 2.5 })
const loupeCanvas = ref(null)

// WebRTC 状态
let ws = null
let pc = null
let controlChannel = null
let mediaStream = null
let statsTimer = null
let logTimer = null
// 连接同步锁：防止并发 connect() 创建多个 PeerConnection（双连接 → 串流 → 画面定格）
let connectLock = false

// ---------- 多页面互斥锁 + 自动重连 ----------
// 同一设备同一时刻只允许一个页面持有 WebRTC 连接（服务端单 viewer 设计）。
// 多个浏览器页面/标签页同时操作时会互相踢连接导致黑屏：
//  - 连接成功即持有 localStorage 锁并心跳续期
//  - 被踢（被动断开）时检查锁：他人持有 → 提示且不重连（避免互踢死循环）；
//    锁在自己手里/已过期 → 自动重连（3/6/12s 退避，上限 3 次）
//  - 用户手动操作（点连接 / 切换配置）→ 强制抢锁
const LOCK_KEY = 'gb_webrtc_lock'
const LOCK_TTL = 15000
const lock = reactive({ pageId: Math.random().toString(36).slice(2, 10), deviceId: null, ts: 0 })
let lockTimer = null
let reconnectTimer = null
let reconnectAttempts = 0
let manualClose = false

function readLock() {
  try { return JSON.parse(localStorage.getItem(LOCK_KEY) || 'null') } catch (e) { return null }
}

function acquireLock(force = false) {
  const cur = readLock()
  const heldByOther = cur && cur.deviceId === store.deviceId && cur.pageId !== lock.pageId && Date.now() - cur.ts < LOCK_TTL
  if (heldByOther && !force) return false
  const l = { pageId: lock.pageId, deviceId: store.deviceId, ts: Date.now() }
  localStorage.setItem(LOCK_KEY, JSON.stringify(l))
  lock.deviceId = store.deviceId
  lock.ts = l.ts
  return true
}

function releaseLock() {
  const cur = readLock()
  if (cur && cur.pageId === lock.pageId) localStorage.removeItem(LOCK_KEY)
  lock.deviceId = null
}

function startLockHeartbeat() {
  stopLockHeartbeat()
  lockTimer = setInterval(() => {
    if (connected.value) acquireLock(false)
  }, 8000)
}

function stopLockHeartbeat() {
  if (lockTimer) { clearInterval(lockTimer); lockTimer = null }
}

/** 被动断开后的自动重连调度：他人持锁则不重连，否则按退避时间重连 */
function scheduleReconnect() {
  if (reconnectTimer || !store.deviceId) return
  const cur = readLock()
  if (cur && cur.deviceId === store.deviceId && cur.pageId !== lock.pageId && Date.now() - cur.ts < LOCK_TTL) {
    errorMsg.value = '设备正在其他页面使用，画面已断开'
    return
  }
  const delay = [3000, 6000, 12000][Math.min(reconnectAttempts, 2)]
  reconnectAttempts++
  toast(`连接已断开，${delay / 1000} 秒后自动重连…`, 'warn')
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null
    // 延迟后二次检查锁：被踢后其他页面可能已抢锁（连接竞态窗口），
    // 此时不再自动重连，避免多页面互踢死循环
    const cur2 = readLock()
    if (cur2 && cur2.deviceId === store.deviceId && cur2.pageId !== lock.pageId && Date.now() - cur2.ts < LOCK_TTL) {
      errorMsg.value = '设备正在其他页面使用，画面已断开'
      return
    }
    connect(false) // 自动重连不强抢锁（锁在自己手里会成功；在别人手里已在上方拦截）
  }, delay)
}

function onChannelOpen() {
  connected.value = true
  connecting.value = false
  reconnectAttempts = 0
  acquireLock(true) // 能建立连接说明服务端已踢掉旧 viewer，锁归本页
  startLockHeartbeat()
  toast('WebRTC 连接建立', 'success')
}

function onChannelClose() {
  connected.value = false
  stopLockHeartbeat()
  if (!manualClose) scheduleReconnect()
  manualClose = false
}

// 右侧面板页签
const activeTab = ref('info')
const tabs = [
  { key: 'info', icon: 'ℹ️', label: '设备' },
  { key: 'auto', icon: '🤖', label: '自动化' },
  { key: 'tpl', icon: '🖼️', label: '模板' },
  { key: 'crop', icon: '✂️', label: '裁切' }
]

// 设备配置修改（下拉框，选项与设备列表预设一致）
const vdPresets = [
  { res: '1920x1080', dpi: 420 },
  { res: '1080x1920', dpi: 420 },
  { res: '1280x720', dpi: 320 },
  { res: '2340x1080', dpi: 440 }
]
const fpsPresets = [15, 30, 60, 120]
const selRes = ref('1920x1080@420')
const selFps = ref('')
// 配置修改串行化标志：防止下拉框连续变更叠加触发多次重连
const configApplying = ref(false)

const devices = computed(() => devicesData.value)
const scripts = computed(() => scriptsData.value)
const templates = computed(() => templatesData.value)

const current = computed(() => devices.value.find(d => d.id === store.deviceId) || null)
const currentName = computed(() => current.value?.name || '未选择设备')
const currentAddr = computed(() => current.value?.addr || '—')
const currentPkg = computed(() => current.value?.pkg || '')
const currentVdRes = computed(() => current.value?.vd_res || '1920x1080')
const currentScreenMode = computed(() => current.value?.screen_mode || 'mirror')

// 当前设备分辨率若不在预设中，追加一个选项展示（只读当前值，仍只能从预设里选）
const vdResOptions = computed(() => {
  const presets = vdPresets.map(p => `${p.res}@${p.dpi}`)
  const cur = `${current.value?.vd_res}@${current.value?.vd_dpi}`
  return cur && cur !== '@' && !presets.includes(cur) ? [...presets, cur] : presets
})

watch(current, (d) => {
  if (!d) return
  selRes.value = d.vd_res && d.vd_dpi ? `${d.vd_res}@${d.vd_dpi}` : '1920x1080@420'
  selFps.value = d.fps || ''
}, { immediate: true })

/** 应用分辨率/帧率修改：更新配置 → 服务端自动断开旧会话 → 重新连接 → 重连 WebRTC 画面 */
async function applyDeviceConfig() {
  if (configApplying.value) return
  const d = current.value
  if (!d) return
  const [res, dpi] = selRes.value.split('@')
  const fps = selFps.value === '' ? null : Number(selFps.value)
  // 无变化则不动作
  if (d.vd_res === res && String(d.vd_dpi) === String(dpi) && (d.fps || null) === fps) return
  const payload = {
    name: d.name,
    kind: d.kind,
    addr: d.addr,
    screen_mode: d.screen_mode,
    vd_res: res,
    vd_dpi: Number(dpi),
    pkg: d.pkg,
    fps
  }
  configApplying.value = true
  try {
    toast('配置已更新，正在自动重连…', 'info')
    await api.updateDevice(d.id, payload)
    await loadData()
    // 服务端已踢掉本页 viewer（config changed, kicked viewer）→ 触发 onclose →
    // 自动重连逻辑（scheduleReconnect）重新建立会话与 WebRTC，无需在此手动重连，
    // 避免与自动重连并发导致双连接
  } catch (e) {
    toast('配置更新失败：' + e.message, 'error')
  } finally {
    configApplying.value = false
  }
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

async function loadData() {
  try {
    devicesData.value = await api.listDevices()
  } catch (e) { console.warn('load devices:', e.message) }
  try {
    scriptsData.value = await api.listScripts()
  } catch (e) { console.warn('load scripts:', e.message) }
  try {
    templatesData.value = await api.listTemplates()
  } catch (e) { console.warn('load templates:', e.message) }
}

// ---------- WebRTC 连接 ----------

// 声音默认静音（虚拟屏音频已接入 WebRTC，用户可点工具栏 🔊 按钮开启）
const audioMuted = ref(true)
function toggleAudio() {
  audioMuted.value = !audioMuted.value
  const v = videoElement.value
  if (v) {
    v.muted = audioMuted.value
    // 取消静音时浏览器要求用户手势后播放（已处于点击事件内，直接 play 即可）
    if (!audioMuted.value) v.play().catch(() => {})
  }
}

async function connect(force = false) {
  // 幂等：同步锁 + 状态检查，杜绝并发/重复调用创建多个 PC
  // （服务端会因多连接出现多推流，video.srcObject 被串流覆盖 → 画面定格）
  if (connectLock || connecting.value || connected.value) {
    console.warn('[webrtc] connect ignored (lock/connecting/connected)')
    return
  }
  // force=true 仅限用户手动操作（点连接按钮）：强制抢锁；
  // 自动重连（force=false）不抢锁——锁在他人手里时已由 scheduleReconnect 拦截
  if (!acquireLock(force)) {
    errorMsg.value = '设备正在其他页面使用'
    return
  }
  connectLock = true
  console.log('[webrtc] connect called (pc exists:', !!pc, ')')
  try {
    await doConnect()
  } finally {
    connectLock = false
  }
}

async function doConnect() {
  if (!store.deviceId) return toast('请先在设备列表选择设备', 'error')
  // 重连场景：若有残留 pc（连接失败但未清理干净），先释放（主动关闭，不触发自动重连）
  if (pc) cleanup(true)
  errorMsg.value = ''
  connecting.value = true

  try {
    // 1. 服务端建立 scrcpy 会话
    await api.connectDevice(store.deviceId)
  } catch (e) {
    connecting.value = false
    errorMsg.value = '设备连接失败：' + e.message
    return
  }

  try {
    // 2. 信令 WebSocket
    const wsProto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    ws = new WebSocket(`${wsProto}//${location.host}/ws/device/${store.deviceId}`)

    await new Promise((resolve, reject) => {
      ws.onopen = resolve
      ws.onerror = () => reject(new Error('信令连接失败'))
    })

    // 3. 创建 PeerConnection（接收视频轨 + 控制 DataChannel）
    pc = new RTCPeerConnection({ iceServers: [{ urls: 'stun:stun.l.google.com:19302' }] })
    // 调试：统计本页创建的 PC 数量（验证无双连接）
    window.__pcCount = (window.__pcCount || 0) + 1
    console.log('[webrtc] PC #' + window.__pcCount)
    pc.addTransceiver('video', { direction: 'recvonly' })
    pc.addTransceiver('audio', { direction: 'recvonly' })

    // 控制 DataChannel 必须由 offerer 创建：否则 offer 里没有 m=application，
    // answer 也不会有（webrtc-rs 只镜像 offer 的 media section），SCTP 永不建立
    controlChannel = pc.createDataChannel('control')
    controlChannel.onopen = onChannelOpen
    controlChannel.onclose = onChannelClose

    pc.ontrack = (e) => {
      // 只接受当前 pc 的轨道：残留/旧连接的 ontrack 不得覆盖 srcObject（串流 → 定格）
      if (e.target !== pc) return
      // 兜底：对端 SDP 无 a=msid 时 e.streams 可能为空，用 track 自建 MediaStream
      mediaStream = e.streams[0] || new MediaStream([e.track])
      if (videoElement.value) {
        videoElement.value.srcObject = mediaStream
        videoElement.value.play().catch(() => {})
      }
      console.log('[webrtc] ontrack', e.track.kind, 'streams=', e.streams.length, 'codec=', e.track.getSettings?.())
      // 视频元信息
      const v = e.track
      v.addEventListener('unmute', () => {
        setTimeout(() => {
          const w = videoElement.value?.videoWidth || 0
          const h = videoElement.value?.videoHeight || 0
          if (w) res.value = `${w}x${h}`
        }, 200)
      })
    }

    pc.ondatachannel = (e) => {
      controlChannel = e.channel
      controlChannel.onopen = onChannelOpen
      controlChannel.onclose = onChannelClose
    }

    // 4. offer 交换
    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    const answer = await new Promise((resolve, reject) => {
      ws.onmessage = (evt) => {
        try {
          const msg = JSON.parse(evt.data)
          if (msg.type === 'answer') resolve(msg.sdp)
          else if (msg.type === 'error') reject(new Error(msg.error || '信令错误'))
        } catch (e) { reject(e) }
      }
      ws.send(JSON.stringify({ type: 'offer', sdp: offer }))
      setTimeout(() => reject(new Error('信令超时')), 10000)
    })
    await pc.setRemoteDescription(new RTCSessionDescription(answer))

    // 5. 统计定时器
    startStats()
    startLogPolling()
  } catch (e) {
    console.error('webrtc connect:', e)
    connecting.value = false
    errorMsg.value = e.message
    cleanup(true)
  }
}

/** 释放 WebRTC 资源；manual=true 表示主动关闭（不触发自动重连） */
function cleanup(manual = false) {
  if (statsTimer) { clearInterval(statsTimer); statsTimer = null }
  if (logTimer) { clearInterval(logTimer); logTimer = null }
  stopLockHeartbeat()
  if (pc) {
    manualClose = manual // 主动关闭时标记：pc.close() 触发的 onclose 不触发自动重连
    try { pc.close() } catch (e) {}
    pc = null
  }
  if (ws) { try { ws.close() } catch (e) {} ws = null }
  controlChannel = null
  mediaStream = null
  connected.value = false
  hadVideo = false
  stillFrames = 0
  // 清空画面：断开后避免旧帧定格残留（overlay 会显示连接提示）
  if (videoElement.value) videoElement.value.srcObject = null
  hideLoupe()
}

// ---------- 视频静默检测 ----------
// 被踢/断流时服务端不再向本页推帧（或已推给其他页面），video.currentTime 停止推进
// → 判定断流，走与 onclose 相同的自动重连逻辑（带页面锁检查）。
// 服务端有静止补帧（RTP 时间戳单调推进），正常连接下 currentTime 持续前进，
// 静止画面不会误判；仅真正无帧流入（被踢/断流/看门狗重建窗口）才会触发。
let lastVideoTime = 0
let stillFrames = 0
let hadVideo = false

function handleVideoSilence() {
  if (manualClose || !connected.value || !store.deviceId) return
  console.warn('[webrtc] video stream silent, treating as disconnected')
  connected.value = false
  stopLockHeartbeat()
  scheduleReconnect()
}

function startStats() {
  if (statsTimer) clearInterval(statsTimer)
  statsTimer = setInterval(async () => {
    if (!pc) return
    // 视频静默检测：仅在见过画面后启用（连接初期 currentTime=0 不误判）
    const v = videoElement.value
    if (connected.value && v && v.videoWidth > 0) {
      hadVideo = true
      if (Math.abs(v.currentTime - lastVideoTime) < 0.001) {
        if (++stillFrames >= 2) { // 连续 ~4s 无新帧
          stillFrames = 0
          handleVideoSilence()
        }
      } else {
        stillFrames = 0
        lastVideoTime = v.currentTime
      }
    }
    try {
      const stats = await pc.getStats()
      let fpsCount = 0
      stats.forEach(s => {
        if (s.type === 'inbound-rtp' && s.kind === 'video') {
          if (s.framesPerSecond) fpsCount = Math.round(s.framesPerSecond)
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
  }, 2000)
}

function startLogPolling() {
  if (logTimer) clearInterval(logTimer)
  logTimer = setInterval(async () => {
    try {
      const logs = await api.listLogs(store.deviceId, null, 5)
      if (logs && logs.length) {
        const newest = logs[0]
        if (liveLogs.value[0]?.time !== newest.time) {
          liveLogs.value = logs.map(l => ({ time: l.time.slice(11, 23), level: l.level, msg: l.msg })).reverse()
        }
      }
    } catch (e) {}
  }, 3000)
}

// ---------- 控制（走 DataChannel） ----------

function sendControl(obj) {
  if (controlChannel && controlChannel.readyState === 'open') {
    controlChannel.send(JSON.stringify(obj))
    console.log('[control] sent', JSON.stringify(obj))
    return true
  }
  console.warn('[control] channel not open, fallback REST', JSON.stringify(obj))
  // fallback：REST API
  api.control(store.deviceId, obj).catch(e => toast('控制失败：' + e.message, 'error'))
  return false
}

/** 鼠标坐标 → 设备坐标（object-fit: contain 换算） */
function toDeviceCoord(clientX, clientY) {
  const video = videoElement.value
  const rect = video.getBoundingClientRect()
  const vw = video.videoWidth || 1920
  const vh = video.videoHeight || 1080
  const ratio = Math.min(rect.width / vw, rect.height / vh)
  const dispW = vw * ratio, dispH = vh * ratio
  const offX = (rect.width - dispW) / 2, offY = (rect.height - dispH) / 2
  const x = Math.round((clientX - rect.left - offX) / dispW * vw)
  const y = Math.round((clientY - rect.top - offY) / dispH * vh)
  return { x: Math.max(0, Math.min(vw, x)), y: Math.max(0, Math.min(vh, y)) }
}

// 触控状态
const touchState = reactive({ active: false, lastX: 0, lastY: 0 })

function onMouseDown(e) {
  if (picking.value && connected.value) {
    const rect = videoWrap.value.getBoundingClientRect()
    selStart.x = e.clientX - rect.left
    selStart.y = e.clientY - rect.top
    selEnd.x = selStart.x; selEnd.y = selStart.y
    selecting.value = true
    return
  }
  if (!connected.value) return
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  touchState.active = true
  touchState.lastX = x; touchState.lastY = y
  // 按下：发 DOWN（拖动时后续 move 事件组成轨迹，up 时收尾）
  sendControl({ type: 'touch', action: 'down', x, y })
}

function onMouseMove(e) {
  if (selecting.value) {
    const rect = videoWrap.value.getBoundingClientRect()
    selEnd.x = e.clientX - rect.left
    selEnd.y = e.clientY - rect.top
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 2.5, [selToDeviceRect()])
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
    sendControl({ type: 'touch', action: 'move', x, y })
  }
}

function togglePick() {
  picking.value = !picking.value
  if (!picking.value) hideLoupe()
}

function onMouseUp(e) {
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
  touchState.active = false
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  sendControl({ type: 'touch', action: 'up', x, y })
}

// ---------- 框选保存模板 ----------

/** 框选矩形（容器 CSS 坐标）→ 设备像素坐标，自动裁剪 letterbox 黑边并夹取到画面内 */
function selToDeviceRect() {
  const video = videoElement.value
  const vw = video?.videoWidth || 1920
  const vh = video?.videoHeight || 1080
  const rect = videoWrap.value.getBoundingClientRect()
  const ratio = Math.min(rect.width / vw, rect.height / vh)
  const dispW = vw * ratio, dispH = vh * ratio
  const offX = (rect.width - dispW) / 2, offY = (rect.height - dispH) / 2
  const toDev = p => ({ x: (p.x - offX) / dispW * vw, y: (p.y - offY) / dispH * vh })
  const p1 = toDev(selStart), p2 = toDev(selEnd)
  const x = Math.round(Math.min(p1.x, p2.x)), y = Math.round(Math.min(p1.y, p2.y))
  const w = Math.round(Math.abs(p2.x - p1.x)), h = Math.round(Math.abs(p2.y - p1.y))
  const cx = Math.max(0, Math.min(vw, x)), cy = Math.max(0, Math.min(vh, y))
  return { x: cx, y: cy, w: Math.min(w, vw - cx), h: Math.min(h, vh - cy) }
}

function defaultTplName() {
  const d = new Date()
  const p = n => String(n).padStart(2, '0')
  return `tpl_${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}_${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}.png`
}

// ---------- 二次裁切 ----------

const cropSize = computed(() => `${Math.round(crop.rect.w)}×${Math.round(crop.rect.h)} px`)

/** 框选完成后打开右侧裁切区 */
function openCrop(rect) {
  const video = videoElement.value
  if (!video?.videoWidth) return toast('无法截取画面，请稍后重试', 'error')
  crop.imgW = video.videoWidth
  crop.imgH = video.videoHeight
  crop.rect = { x: Math.round(rect.x), y: Math.round(rect.y), w: Math.round(rect.w), h: Math.round(rect.h) }
  crop.name = defaultTplName()
  crop.active = true
  activeTab.value = 'crop'
  nextTick(() => {
    renderCropFrame()
    refreshCropPreview()
    cropSec.value?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  })
}

function cancelCrop() {
  crop.active = false
  hideLoupe()
  if (activeTab.value === 'crop') activeTab.value = 'info'
}

function repick() {
  crop.active = false
  picking.value = true
  if (activeTab.value === 'crop') activeTab.value = 'info'
  toast('在画面上重新框选', 'info')
}

/** 画布适配尺寸：只显示框选区域，小图适当放大、大图缩到面板宽度内 */
function cropFit() {
  const w = Math.max(1, crop.rect.w), h = Math.max(1, crop.rect.h)
  const scale = Math.min(260 / w, 220 / h, 3)
  return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)), scale: Math.round(w * scale) / w }
}

/** 在裁切画布上绘制框选区域本身（放大显示）+ 边框手柄 + 尺寸标注 */
function renderCropFrame() {
  const canvas = cropCanvas.value
  const video = videoElement.value
  if (!canvas || !video?.videoWidth || crop.rect.w < 1 || crop.rect.h < 1) return
  const fit = cropFit()
  canvas.width = fit.w
  canvas.height = fit.h
  canvas.style.width = fit.w + 'px'
  canvas.style.height = fit.h + 'px'
  const ctx = canvas.getContext('2d')
  ctx.clearRect(0, 0, fit.w, fit.h)
  ctx.drawImage(video, crop.rect.x, crop.rect.y, crop.rect.w, crop.rect.h, 0, 0, fit.w, fit.h)
  // 边框
  ctx.strokeStyle = 'rgba(34,211,165,.95)'
  ctx.lineWidth = 1.5
  ctx.strokeRect(0.5, 0.5, fit.w - 1, fit.h - 1)
  // 角点手柄
  ctx.fillStyle = '#fff'
  const hs = 5
  for (const [hx, hy] of [[0, 0], [fit.w, 0], [0, fit.h], [fit.w, fit.h]]) {
    ctx.fillRect(hx - hs / 2, hy - hs / 2, hs, hs)
  }
  // 尺寸标注
  ctx.fillStyle = 'rgba(34,211,165,.95)'
  ctx.font = '10px monospace'
  ctx.fillText(cropSize.value, 6, 12)
}

/** 按当前裁切框重新生成裁剪结果预览（全分辨率） */
function refreshCropPreview() {
  const video = videoElement.value
  if (!video?.videoWidth) return
  const r = crop.rect
  if (r.w < 1 || r.h < 1) return
  const canvas = document.createElement('canvas')
  canvas.width = r.w
  canvas.height = r.h
  canvas.getContext('2d').drawImage(video, r.x, r.y, r.w, r.h, 0, 0, r.w, r.h)
  crop.preview = canvas.toDataURL('image/png')
}

/** 鼠标事件 → 设备像素坐标（相对裁切框显示区域换算） */
function cropEventDev(e) {
  const canvas = cropCanvas.value
  const rect = canvas.getBoundingClientRect()
  const scale = canvas.width / crop.rect.w
  return {
    x: crop.rect.x + (e.clientX - rect.left) / scale,
    y: crop.rect.y + (e.clientY - rect.top) / scale
  }
}

function cropMouseDown(e) {
  const p = cropEventDev(e)
  const r = crop.rect
  const HIT = 12 / (cropCanvas.value.width / crop.rect.w) // 设备像素命中半径
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
  updateLoupe(e.clientX, e.clientY, cropEventDev(e), 3, [crop.rect])
  if (!cropDrag.mode) return
  const p = cropEventDev(e)
  const o = cropDrag.orig
  const r = crop.rect
  const MIN = 8
  const dx = p.x - cropDrag.startX
  const dy = p.y - cropDrag.startY
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v))
  switch (cropDrag.mode) {
    case 'move':
      r.x = clamp(o.x + dx, 0, crop.imgW - o.w)
      r.y = clamp(o.y + dy, 0, crop.imgH - o.h)
      break
    case 'nw':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.w = o.x + o.w - r.x; r.h = o.y + o.h - r.y
      break
    case 'ne':
      r.w = clamp(o.w + dx, MIN, crop.imgW - o.x)
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.h = o.y + o.h - r.y
      break
    case 'sw':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.w = o.x + o.w - r.x
      r.h = clamp(o.h + dy, MIN, crop.imgH - o.y)
      break
    case 'se':
      r.w = clamp(o.w + dx, MIN, crop.imgW - o.x)
      r.h = clamp(o.h + dy, MIN, crop.imgH - o.y)
      break
    case 'n':
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.h = o.y + o.h - r.y
      break
    case 's':
      r.h = clamp(o.h + dy, MIN, crop.imgH - o.y)
      break
    case 'w':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.w = o.x + o.w - r.x
      break
    case 'e':
      r.w = clamp(o.w + dx, MIN, crop.imgW - o.x)
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

async function saveTemplate() {
  const raw = crop.name.trim()
  if (!raw) return toast('请输入模板名称', 'warn')
  const name = raw.toLowerCase().endsWith('.png') ? raw : raw + '.png'
  saving.value = true
  try {
    await api.uploadTemplate(name, crop.preview.split(',')[1])
    templatesData.value = await api.listTemplates()
    crop.active = false
    hideLoupe()
    if (activeTab.value === 'crop') activeTab.value = 'tpl'
    toast(`模板 ${name} 已保存`, 'success')
    pushLog('success', `模板 ${name} 已保存（${cropSize.value}）`)
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
  if (!currentPkg.value) return toast('该设备未配置游戏包名', 'warn')
  sendControl({ type: 'start_app', app: currentPkg.value })
  toast(`正在启动 ${currentPkg.value}…`, 'info')
}

function openScripts() { router.push('/scripts') }

function tplThumbUrl(name) { return `/api/templates/${encodeURIComponent(name)}/image` }

function pushLog(level, msg) {
  const t = new Date().toTimeString().slice(0, 8)
  liveLogs.value.push({ time: t, level, msg })
  if (liveLogs.value.length > 30) liveLogs.value.shift()
}

function runScript() {
  if (!selScript.value) return
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return
  store.running = true
  store.runScript = s.name
  store.runProgress = 10
  store.runStep = '启动中…'
  pushLog('info', `开始执行脚本：${s.name}`)
  api.runScript(s.id, store.deviceId).then(() => {
    store.runProgress = 100
    setTimeout(() => {
      store.running = false
      store.runStep = ''
    }, 1500)
    pushLog('success', '脚本执行完成')
  }).catch(e => {
    store.running = false
    pushLog('error', `执行失败：${e.message}`)
    toast('脚本执行失败', 'error')
  })
}

function stopScript() {
  if (!selScript.value) return
  api.stopScript(selScript.value).catch(() => {})
  store.running = false
  pushLog('warn', '脚本已停止')
  toast('脚本已停止', 'warn')
}

async function testMatch(name) {
  if (!connected.value) return toast('请先连接设备', 'error')
  showHit.value = false
  try {
    const r = await api.testTemplate(name, store.deviceId, 0.8, null)
    if (r.hit) {
      hit.x = r.x; hit.y = r.y; hit.w = r.width; hit.h = r.height
      hitLabel.value = `${name} ${r.score.toFixed(2)}`
      showHit.value = true
      toast(`匹配成功：${name} 置信度 ${r.score.toFixed(2)}`, 'success')
      pushLog('success', `模板 ${name} 命中 @ (${r.x},${r.y}) 置信度 ${r.score.toFixed(2)}`)
    } else {
      toast(`未找到：${name}`, 'warn')
      pushLog('warn', `模板 ${name} 未命中`)
    }
  } catch (e) {
    toast('匹配失败：' + e.message, 'error')
  }
}

function fullscreen() {
  if (videoWrap.value?.requestFullscreen) videoWrap.value.requestFullscreen()
}

onMounted(() => {
  loadData()
  // 页面关闭时释放页面锁（其他页面才能接管）
  window.addEventListener('beforeunload', releaseLock)
  if (store.deviceId) connect(false)
})

onUnmounted(() => {
  window.removeEventListener('beforeunload', releaseLock)
  if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null }
  releaseLock()
  cleanup(true)
})
</script>

<style scoped>
.console { display: flex; height: 100%; padding: 14px; gap: 14px; }

/* ===== 画面区 ===== */
.stage { flex: 1; display: flex; flex-direction: column; gap: 10px; min-width: 0; }

.video-wrap {
  flex: 1; position: relative; background: #000;
  border: 1px solid var(--border); border-radius: var(--radius);
  overflow: hidden; min-height: 300px;
}

.video-stream { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; user-select: none; }

.hit-box {
  position: absolute; border: 2px solid var(--accent);
  box-shadow: 0 0 12px rgba(34,211,165,.5); border-radius: 4px;
  pointer-events: none; z-index: 5;
}
.hit-label {
  position: absolute; top: -22px; left: 0; background: var(--accent); color: #06251c;
  font-size: 10px; font-weight: 700; padding: 1px 6px; border-radius: 4px; white-space: nowrap;
}

.select-box {
  position: absolute; border: 2px dashed var(--accent-2);
  background: rgba(56,189,248,.12); pointer-events: none; z-index: 5;
}

/* 放大预览镜 */
.loupe {
  position: fixed; z-index: 200; width: 150px; height: 150px;
  border: 1px solid rgba(34,211,165,.5); border-radius: 10px; overflow: hidden;
  background: #000; box-shadow: 0 8px 30px rgba(0,0,0,.6);
  pointer-events: none;
}
.loupe canvas { width: 100%; height: 100%; display: block; }
.loupe-tag {
  position: absolute; right: 6px; bottom: 4px; font-size: 10px;
  color: #fff; background: rgba(0,0,0,.55); padding: 1px 5px; border-radius: 6px;
}

/* 二次裁切区 */
.crop-stage { display: flex; flex-direction: column; align-items: center; gap: 6px; }
.crop-canvas {
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  cursor: crosshair; background: #000; touch-action: none;
}
.crop-hint { font-size: 10px; color: var(--text-2); align-self: flex-start; }
.crop-actions { display: flex; gap: 8px; }
.crop-actions .btn-primary { margin-left: auto; }

.v-overlay {
  position: absolute; inset: 0; z-index: 10; display: flex;
  align-items: center; justify-content: center;
  background: rgba(8,10,16,.72); backdrop-filter: blur(2px);
}
.v-connecting { display: flex; align-items: center; gap: 10px; color: var(--accent); font-size: 14px; }
.v-empty-icon { font-size: 44px; text-align: center; opacity: .6; }
.v-empty-text { color: var(--text-1); margin: 10px 0 16px; max-width: 320px; text-align: center; }

.v-stats {
  position: absolute; left: 12px; top: 12px; z-index: 6;
  display: flex; gap: 8px; background: rgba(8,10,16,.6);
  border: 1px solid rgba(255,255,255,.08); border-radius: 20px; padding: 4px 10px;
}
.st { font-size: 11px; color: var(--text-1); font-family: var(--mono); }

.v-fs {
  position: absolute; right: 12px; top: 12px; z-index: 6;
  background: rgba(8,10,16,.6); border: 1px solid rgba(255,255,255,.08);
  color: var(--text-1); border-radius: 8px; width: 30px; height: 30px; cursor: pointer;
}
.v-fs:hover { color: var(--accent); border-color: var(--accent); }

/* 工具条 */
.toolbar {
  display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 8px 10px;
  min-height: 45px; box-sizing: border-box;
}
.tb-sep { width: 1px; height: 22px; background: var(--border); margin: 0 4px; }
.tb-tip { margin-left: auto; font-size: 11px; color: var(--text-2); }
.btn.active { border-color: var(--accent-2); color: var(--accent-2); }

/* ===== 右侧面板 ===== */
.panel {
  width: 320px; flex-shrink: 0; display: flex; flex-direction: column; gap: 10px;
  overflow: hidden;
}
.panel-tabs {
  display: flex; gap: 4px; flex-shrink: 0;
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 4px 6px;
  height: 45px; box-sizing: border-box;
}
.tab-btn {
  flex: 1; display: flex; align-items: center; justify-content: center; gap: 4px;
  padding: 7px 2px; border: none; border-radius: var(--radius-sm);
  background: transparent; color: var(--text-2); font-size: 12px; cursor: pointer;
  transition: all .15s; white-space: nowrap;
}
.tab-btn:hover { color: var(--text-0); background: var(--bg-3); }
.tab-btn.active { background: rgba(34,211,165,.1); color: var(--accent); font-weight: 600; }
.tab-btn.hidden { display: none; }
.tab-body { flex: 1; display: flex; flex-direction: column; gap: 12px; overflow: auto; min-height: 0; }
.ps-stats { display: flex; gap: 8px; flex-wrap: wrap; }

/* 设备配置修改区 */
.cfg-box {
  display: flex; flex-direction: column; gap: 8px;
  border-top: 1px solid var(--border); padding-top: 10px;
}
.cfg-box .form-item { display: flex; align-items: center; gap: 8px; }
.cfg-box label { font-size: 12px; color: var(--text-1); flex-shrink: 0; width: 44px; }
.cfg-box .select { flex: 1; padding: 5px 8px; font-size: 12px; }
.cfg-hint { font-size: 10px; color: var(--text-2); }

.panel-sec {
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 14px; display: flex; flex-direction: column; gap: 10px;
  flex-shrink: 0;
}
.ps-head { display: flex; align-items: center; gap: 8px; }
.ps-title { font-size: 13px; font-weight: 600; }
.ps-sub { font-size: 11px; color: var(--text-2); }
.mono { font-family: var(--mono); font-size: 11px; color: var(--text-1); }

.auto-run { display: flex; gap: 8px; }
.auto-run .select { flex: 1; }

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
.ll { display: flex; gap: 8px; font-size: 11px; line-height: 1.5; }
.ll-time { color: var(--text-2); flex-shrink: 0; }
.ll.info .ll-msg { color: var(--text-1); }
.ll.success .ll-msg { color: var(--ok); }
.ll.warn .ll-msg { color: var(--warn); }
.ll.error .ll-msg { color: var(--danger); }

.tpl-quick { display: flex; flex-wrap: wrap; gap: 8px; }
.tpl-chip {
  display: flex; align-items: center; gap: 6px; padding: 5px 10px;
  background: var(--bg-3); border: 1px solid var(--border); border-radius: 20px;
  font-size: 12px; color: var(--text-1); cursor: pointer; transition: all .15s;
}
.tpl-chip:hover { border-color: var(--accent); color: var(--accent); }
.tpl-thumb { font-size: 12px; position: relative; display: inline-flex; }
.tpl-thumb::before { content: '▦'; }
.tpl-thumb img {
  position: relative; z-index: 1; width: 14px; height: 14px; object-fit: contain;
}
</style>
