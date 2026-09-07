import { computed, onUnmounted, reactive, ref, shallowRef } from 'vue'
import { api } from '../../api'
import { videoApi } from '../video/videoApi'

/**
 * 统一舞台来源 StageSource（视频工作台 V1，实施合同 §6 / 计划 §4.2）：
 * `kind: 'live' | 'media'`、`sourceId`、`generation`（来源切换/媒体切换时递增）、
 * `displaySize/referenceSize`、`canDeviceInput`、`frameAt{mediaId,ptsUs,index}`。
 *
 * 安全红线：媒体模式是离线只读来源——一切**由舞台产生**的设备输入（鼠标触控、
 * 键盘、按键映射、滚轮、启停应用）必须在统一输入路由处拒绝，`guardDeviceInput`
 * 即该路由门禁（Console 的 sendControl / sendKeyboardControl 入口调用）。
 * canDeviceInput 只是前端 UI 提示，真正的输入授权仍在服务端。
 *
 * 指定帧：媒体模式播放器定位（浏览器 currentTime）仅用于预览；制作模板等需要
 * 精确帧的场景经 `captureFrame()` 走服务端确定帧 PNG（按展示序索引寻址，可
 * 重复、不依赖 seek 后的旧画面，计划 §4.3/§6.1；Phase 5 起帧身份 = 服务端
 * 真实展示帧表，前端无固定步长假设）。结果携带 generation，来源切换后过期不应用。
 *
 * 来源切换不改变真实设备连接、不停止运行中的任务；切回实时时由 Console 侧
 * watch(kind) 清理指针/键盘焦点/旧来源叠加层（见 Console.vue）。
 */

/** 媒体控制条倍速档位 */
export const STAGE_RATE_OPTIONS = [0.25, 0.5, 1, 2, 4]
const ACTIVE_POLL_MS = 5000

/** 需要在媒体模式下统一拒绝的设备输入消息类型（sendControl 词表 + 键盘 key） */
const DEVICE_INPUT_TYPES = new Set([
  'touch', 'swipe', 'scroll', 'text', 'press', 'key', 'input_event',
  'start_app', 'stop_app', 'rotate', 'clipboard',
])

/** 秒 → "mm:ss.d" 展示（媒体控制条时间显示；负/非法值归 0） */
export function formatStageClock(seconds) {
  const total = Math.max(0, Number(seconds) || 0)
  const m = Math.floor(total / 60)
  const s = total - m * 60
  return `${String(m).padStart(2, '0')}:${s.toFixed(1).padStart(4, '0')}`
}

function defaultLoadImage(url) {
  return new Promise((resolve, reject) => {
    const img = new Image()
    img.onload = () => resolve(img)
    img.onerror = () => reject(new Error('frame image load failed'))
    img.src = url
  })
}

export function useConsoleStage({
  toast,
  /** 设备 id（computed/ref/函数均可）；live 来源 id */
  deviceId,
  /** WebRTC 连接态 ref（live 模式可用性；不影响录制轮询——服务端权威） */
  connected,
  /** 实时画面元素访问器（live 坐标系/帧冻结源） */
  liveVideoEl,
  /** 测试注入：帧图片加载器 */
  loadImage = defaultLoadImage,
} = {}) {
  // ---------- StageSource 状态 ----------
  const kind = ref('live')
  const sourceId = ref('')
  const generation = ref(0)
  const mediaList = ref([])
  const mediaMeta = ref(null)

  // ---------- 媒体播放器状态（<video> 元素由舞台组件挂载后注入；
  // shallowRef 持有 DOM 元素，不做深度响应式代理） ----------
  const mediaVideoEl = shallowRef(null)
  const playing = ref(false)
  const currentTimeSec = ref(0)
  const durationSec = ref(0)
  const playbackRate = ref(1)
  const frameReady = ref(false)
  /** 当前锁定帧身份 {index, pts_us}（服务端展示帧表；null = 按预览时间解析） */
  const stageFrame = ref(null)
  /** programmaticSeek：stepFrames 引发的 seek 不清帧身份（用户手动 seek 才清） */
  let programmaticSeek = false

  // ---------- 录制按钮态（activeRecording 轮询驱动；start/stop 经 REST） ----------
  const activeSession = ref(null)
  const recordingBusy = ref(false)

  let mediaBinding = null
  let pollTimer = null
  let inputWarned = false

  const devId = () => {
    const v = typeof deviceId === 'function' ? deviceId() : deviceId?.value
    return typeof v === 'string' ? v : ''
  }

  const canDeviceInput = computed(() => kind.value === 'live')
  const mediaReady = computed(() => kind.value === 'media' && frameReady.value)
  const stageReady = computed(() => (kind.value === 'live' ? !!connected.value : mediaReady.value))

  const displaySize = computed(() => {
    if (kind.value === 'media') {
      const el = mediaVideoEl.value
      return {
        width: el?.videoWidth || mediaMeta.value?.width || 0,
        height: el?.videoHeight || mediaMeta.value?.height || 0,
      }
    }
    const el = liveVideoEl?.()
    return { width: el?.videoWidth || 0, height: el?.videoHeight || 0 }
  })
  // V1：参考尺寸 = 来源原始画面尺寸（外部素材的显式校准为后续能力）
  const referenceSize = computed(() => ({ ...displaySize.value }))

  const mediaSrc = computed(() => (kind.value === 'media' && mediaMeta.value ? api.mediaFileUrl(mediaMeta.value.id) : ''))

  /** 指定帧选择器：媒体模式且画面就绪时给出当前帧。帧身份优先取服务端展示帧
   *  表锁定的 {index, pts_us}；未锁定时 pts 取预览时间（粗定位，index 未知）——
   *  精确帧由服务端按 pts/索引解码保证可重复，浏览器 currentTime 只作预览定位。 */
  const frameAt = computed(() => {
    if (kind.value !== 'media' || !mediaMeta.value || !frameReady.value) return null
    return {
      mediaId: mediaMeta.value.id,
      ptsUs: stageFrame.value
        ? stageFrame.value.pts_us
        : Math.max(0, Math.round(currentTimeSec.value * 1e6)),
      index: stageFrame.value ? stageFrame.value.index : null,
    }
  })

  // ---------- 输入门禁（安全红线的唯一裁决点） ----------
  /** 统一输入路由门禁：媒体模式拒绝一切舞台产生的设备输入（warn 一次）。 */
  function guardDeviceInput(obj) {
    if (canDeviceInput.value) return true
    if (obj && DEVICE_INPUT_TYPES.has(String(obj?.type))) {
      if (!inputWarned) {
        inputWarned = true
        toast?.('视频来源模式为只读，不向设备发送输入', 'warn')
      }
      return false
    }
    return true
  }

  // ---------- 媒体库 ----------
  async function refreshMedia() {
    try {
      mediaList.value = await api.listMedia()
      return true
    } catch (e) {
      toast?.('读取媒体库失败：' + e.message, 'error')
      return false
    }
  }

  async function loadMedia(id) {
    const wanted = String(id || '')
    if (!wanted) return false
    const meta = mediaList.value.find(m => m.id === wanted)
      || await api.getMedia(wanted).catch(e => {
        toast?.('读取素材失败：' + e.message, 'error')
        return null
      })
    if (!meta) return false
    if (kind.value !== 'media') toMediaKind()
    mediaMeta.value = meta
    sourceId.value = meta.id
    // 媒体切换同属来源切换：generation 递增（旧来源的异步帧/裁切结果过期）
    generation.value += 1
    playing.value = false
    currentTimeSec.value = 0
    durationSec.value = Number(meta.duration_us || 0) / 1e6
    frameReady.value = (mediaVideoEl.value?.videoWidth || 0) > 0
    stageFrame.value = null
    return true
  }

  /** 进入媒体模式且尚无选中素材：默认选最新一条（列表创建时间倒序首位） */
  async function ensureMediaSelected() {
    await refreshMedia()
    const current = mediaMeta.value?.id
    if (current && mediaList.value.some(m => m.id === current)) return
    const first = mediaList.value[0]
    if (first) await loadMedia(first.id)
    else {
      mediaMeta.value = null
      sourceId.value = ''
      toast?.('媒体库为空：请先在视频工作台导入素材或完成一次录制', 'warn')
    }
  }

  function toMediaKind() {
    if (kind.value === 'media') return
    kind.value = 'media'
    generation.value += 1
  }

  function setKind(next) {
    const target = next === 'media' ? 'media' : 'live'
    if (target === kind.value) return
    kind.value = target
    generation.value += 1
    if (target === 'live') pauseMedia()
    else if (!mediaMeta.value) void ensureMediaSelected()
    else void refreshMedia()
  }

  // ---------- 媒体播放控制 ----------
  function attachMediaVideo(el) {
    detachMediaVideo()
    mediaVideoEl.value = el || null
    if (!el) return
    const syncTime = () => { currentTimeSec.value = Math.max(0, Number(el.currentTime) || 0) }
    const syncMeta = () => {
      if (Number.isFinite(el.duration)) durationSec.value = el.duration
      frameReady.value = (el.videoWidth || 0) > 0
    }
    const syncPlay = () => { playing.value = true }
    const syncPause = () => { playing.value = false }
    const syncRate = () => { playbackRate.value = el.playbackRate || 1 }
    // seeked：stepFrames 的程序性 seek 保持帧身份；用户手动 seek 使其失效
    const syncSeeked = () => {
      syncTime()
      if (programmaticSeek) {
        programmaticSeek = false
        return
      }
      stageFrame.value = null
    }
    const onError = () => {
      frameReady.value = false
      playing.value = false
      toast?.('视频加载失败：素材可能暂不受支持', 'error')
    }
    const pairs = [
      ['timeupdate', syncTime], ['seeked', syncSeeked],
      ['loadedmetadata', syncMeta], ['resize', syncMeta],
      ['play', syncPlay], ['pause', syncPause], ['ratechange', syncRate],
      ['error', onError],
    ]
    for (const [name, fn] of pairs) el.addEventListener(name, fn)
    mediaBinding = { el, pairs }
    syncMeta()
    syncTime()
  }

  function detachMediaVideo() {
    if (mediaBinding) {
      for (const [name, fn] of mediaBinding.pairs) mediaBinding.el.removeEventListener(name, fn)
      mediaBinding = null
    }
    mediaVideoEl.value = null
    frameReady.value = false
  }

  function pauseMedia() {
    try { mediaVideoEl.value?.pause?.() } catch { /* 未加载完成时静默 */ }
    playing.value = false
  }

  function togglePlay() {
    const el = mediaVideoEl.value
    if (kind.value !== 'media' || !el) return
    if (el.paused) {
      el.play?.().catch(() => {})
    } else {
      el.pause?.()
    }
  }

  /** 逐帧 ±n（Phase 5）：服务端真实展示帧表相邻定位（prev/next），无固定步长
   *  假设。首步按预览时间解析当前帧，之后沿相邻帧链走；自动暂停，预览 seek 到
   *  目标帧时刻（程序性 seek 不清帧身份）。帧表不可用时提示并保持现状。 */
  async function stepFrames(n) {
    const el = mediaVideoEl.value
    if (kind.value !== 'media' || !el || !mediaMeta.value) return
    const count = Math.round(Number(n) || 0)
    if (!count) return
    const dir = count > 0 ? 1 : -1
    const id = mediaMeta.value.id
    try {
      let position = stageFrame.value
      if (!position) {
        const meta = await videoApi.mediaFrames(id, {
          ptsUs: Math.max(0, Math.round(currentTimeSec.value * 1e6)),
        })
        position = meta?.current || null
      }
      if (!position) return // 空素材（0 帧）
      let target = position
      for (let i = 0; i < Math.abs(count); i++) {
        const neighbors = await videoApi.mediaFrameNeighbors(id, target.index)
        const nextTarget = dir < 0 ? neighbors?.prev : neighbors?.next
        if (!nextTarget) break // 首/末帧边界
        target = nextTarget
      }
      if (target === position) return
      stageFrame.value = target
      if (!el.paused) el.pause?.()
      programmaticSeek = true
      try { el.currentTime = target.pts_us / 1e6 } catch { /* 元数据未就绪时静默 */ }
      currentTimeSec.value = target.pts_us / 1e6
    } catch (e) {
      toast?.('逐帧定位失败：' + (e?.message || e), 'warn')
    }
  }

  function setRate(rate) {
    const el = mediaVideoEl.value
    const v = Number(rate)
    if (kind.value !== 'media' || !el || !Number.isFinite(v)) return
    try { el.playbackRate = v } catch { /* 非法倍速由浏览器拒绝 */ }
    playbackRate.value = el.playbackRate || 1
  }

  // ---------- 画面对象与指定帧捕获（裁切/放大镜数据源） ----------
  /** 舞台当前活动画面元素：live = WebRTC video；media = 媒体 <video>。 */
  function surfaceEl() {
    if (kind.value === 'media') return mediaVideoEl.value || null
    return liveVideoEl?.() || null
  }

  /** 冻结当前画面帧：live = 现有视频元素（既有截图路径）；media = 服务端确定帧
   *  PNG（mediaFrameUrl 按当前 ptsUs，绝不在保存时重抓最新设备画面）。
   *  返回 {source,width,height,generation,label}；画面不可用返回 null。 */
  async function captureFrame() {
    if (kind.value === 'live') {
      const el = liveVideoEl?.()
      if (!el?.videoWidth) return null
      return { source: el, width: el.videoWidth, height: el.videoHeight, generation: generation.value, label: '实时画面当前帧' }
    }
    const frame = frameAt.value
    const meta = mediaMeta.value
    if (!frame || !meta) return null
    // 帧身份已知（stageFrame 锁定）→ 按展示序索引寻址（字节级可重复）；
    // 未锁定 → 按预览 pts 粗定位（服务端解析为首个 pts ≥ 目标的展示帧）
    const url = frame.index !== null && frame.index !== undefined
      ? api.mediaFrameUrl(meta.id, { index: frame.index })
      : api.mediaFrameUrl(meta.id, { ptsUs: frame.ptsUs })
    let img = null
    try { img = await loadImage(url) } catch { img = null }
    if (!img || !img.naturalWidth) return null
    return {
      source: img,
      width: img.naturalWidth,
      height: img.naturalHeight,
      generation: generation.value,
      // 帧身份随裁切底图走：label 携带展示序索引与真实 PTS（可追溯）
      label: frame.index !== null && frame.index !== undefined
        ? `视频帧 #${frame.index} @ ${formatStageClock(frame.ptsUs / 1e6)}`
        : `视频帧 @ ${formatStageClock(frame.ptsUs / 1e6)}`,
    }
  }

  // ---------- 录制按钮态 ----------
  const TERMINAL_STATES = new Set(['completed', 'interrupted', 'failed', 'cancelled'])

  async function pollActiveSession() {
    const id = devId()
    if (!id) {
      activeSession.value = null
      return
    }
    try {
      const rep = await api.activeRecording(id)
      // 404（无活动会话）已在 api 层归一为 null；非会话形态的响应同样视为无会话
      activeSession.value = rep && typeof rep === 'object' && typeof rep.id === 'string' ? rep : null
    } catch { /* 轮询失败静默：按钮态保持上一次结果 */ }
  }

  function startPolling() {
    stopPolling()
    pollTimer = setInterval(() => { void pollActiveSession() }, ACTIVE_POLL_MS)
  }

  function stopPolling() {
    if (pollTimer) { clearInterval(pollTimer); pollTimer = null }
  }

  async function startRecording() {
    const id = devId()
    if (!id) return toast?.('请先选择设备', 'warn')
    if (!canDeviceInput.value || !connected?.value) return toast?.('请先连接设备再开始录制', 'warn')
    recordingBusy.value = true
    try {
      activeSession.value = await api.recordingStart(id)
      toast?.('录制已开始（服务端执行，浏览器可关闭）', 'success')
    } catch (e) {
      toast?.('开始录制失败：' + e.message, 'error')
    } finally {
      recordingBusy.value = false
    }
  }

  async function stopRecording() {
    const session = activeSession.value
    if (!session?.id) return
    recordingBusy.value = true
    try {
      const done = await api.recordingStop(session.id)
      activeSession.value = null
      await refreshMedia()
      // 录制产出已入媒体库：切到视频来源打开新素材（segments 时间轴分段保序）
      const mediaId = done?.segments?.length ? done.segments[done.segments.length - 1].media_id : ''
      if (mediaId) {
        await loadMedia(mediaId)
        toast?.('录制完成，已切换到视频来源查看新素材', 'success')
      } else {
        toast?.(`录制结束（${done?.state || 'completed'}）`, done && TERMINAL_STATES.has(done.state) ? 'info' : 'warn')
      }
    } catch (e) {
      toast?.('停止录制失败：' + e.message, 'error')
      void pollActiveSession()
    } finally {
      recordingBusy.value = false
    }
  }

  function toggleRecording() {
    if (recordingBusy.value) return
    if (activeSession.value && !TERMINAL_STATES.has(activeSession.value.state)) return stopRecording()
    return startRecording()
  }

  // 设备切换后旧会话态作废（录制归属设备，服务端权威；下一轮轮询恢复按钮态）
  function onDeviceChanged() {
    activeSession.value = null
    void pollActiveSession()
  }

  // 按钮态轮询：挂载即启动（录制属服务端权威，与浏览器/WebRTC 连接态无关）
  void pollActiveSession()
  startPolling()

  onUnmounted(() => {
    stopPolling()
    detachMediaVideo()
  })

  const view = reactive({
    // StageSource（计划 §4.2）
    kind,
    sourceId,
    generation,
    displaySize,
    referenceSize,
    canDeviceInput,
    stageReady,
    // 媒体来源展示/控制
    mediaId: computed(() => mediaMeta.value?.id || ''),
    mediaName: computed(() => mediaMeta.value?.name || ''),
    mediaSizeLabel: computed(() => {
      const { width, height } = displaySize.value
      return width && height ? `${width}×${height}` : ''
    }),
    mediaOptions: computed(() => mediaList.value.map(m => ({ id: m.id, name: m.name }))),
    mediaSrc,
    playing,
    timeText: computed(() => formatStageClock(currentTimeSec.value)),
    durationText: computed(() => formatStageClock(durationSec.value)),
    rate: playbackRate,
    rateOptions: STAGE_RATE_OPTIONS,
    // 录制按钮态
    recordingActive: computed(() => !!activeSession.value && !TERMINAL_STATES.has(activeSession.value.state)),
    recordingBusy,
    recordingState: computed(() => activeSession.value?.state || ''),
    // 动作（模板直接经 view 调用）
    toggleKind: () => setKind(kind.value === 'live' ? 'media' : 'live'),
    backToLive: () => setKind('live'),
    onMediaPick: id => loadMedia(id),
    togglePlay,
    stepFrames,
    setRate,
    toggleRecording,
    refreshMedia,
  })

  return {
    view,
    guardDeviceInput,
    attachMediaVideo,
    detachMediaVideo,
    surfaceEl,
    frameAt: () => frameAt.value,
    displaySize: () => ({ ...displaySize.value }),
    captureFrame,
    togglePlay,
    stepFrames,
    setRate,
    pollActiveSession,
    onDeviceChanged,
    /** 指定帧裁切桥（useConsoleTemplates 注入）：kind/ready/generation 访问器 +
     *  captureFrame 指定帧冻结。generation 供调用方做过期判定。 */
    templateBridge: {
      kind: () => kind.value,
      ready: () => stageReady.value,
      generation: () => generation.value,
      surfaceEl,
      captureFrame,
    },
  }
}
