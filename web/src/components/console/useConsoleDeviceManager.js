import { computed, nextTick, reactive, ref, watch } from 'vue'
import { api, runPartitionImport } from '../../api'
import { appStartedDevices } from '../../store'
import { formatScreenSummary } from '../../console/device-summary'

// 应用列表缓存：设备 id -> { list, ts }，应用列表不常变，避免每次重复读取
const appCache = new Map()
const APP_CACHE_TTL = 5 * 60 * 1000

/**
 * 设备管理（工具条设备控件 + 设置弹窗；原右侧设备页签已收编）：
 * 设备 CRUD/扫描/连接动作、已安装应用列表、应用分区下拉候选、
 * 分区快照导入导出，以及工具条「更多」菜单与快捷投屏动作。
 * 自 Console.vue 原样拆出，行为零变化。
 */
export function useConsoleDeviceManager({
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
  /** 控制消息发送（Console 的 DataChannel 链路，函数声明提升后传入） */
  sendControl,
}) {
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
  const form = reactive({ name: '', kind: 'redroid', addr: '', screen_mode: 'virtual', vd_res: '1920x1080', vd_dpi: 0, fps: 30 })
  const scanning = consoleRuntime.scanning
  // 配置保存进行中标志：防止重复提交
  const configApplying = ref(false)

  // 已安装应用列表：读取后合并到右侧包名下拉；选择包名本身不触发任何启动动作。
  const appList = ref([])
  const appLoading = ref(false)

  const devices = computed(() => devicesData.value)
  const scripts = computed(() => scriptsData.value)
  const current = computed(() => devices.value.find(d => d.id === store.deviceId) || null)
  const currentName = computed(() => current.value?.name || '未选择设备')

  /** 应用包名下拉选项：旧设备记录包名 ∪ 已安装应用 ∪ 脚本分区 ∪ 模板分区（字典序） */
  const pkgOptions = computed(() => {
    const set = new Set()
    const dp = (current.value?.pkg || '').trim()
    if (dp) set.add(dp)
    for (const a of appList.value) if (a.pkg) set.add(a.pkg)
    for (const s of scripts.value) if (s.package) set.add(s.package)
    for (const t of templatesData.value) if (t.pkg) set.add(t.pkg)
    return [...set].sort((a, b) => a.localeCompare(b))
  })

  const appLabelByPkg = computed(() => new Map(
    appList.value.filter(a => a && a.pkg).map(a => [a.pkg, a.label || a.pkg])
  ))

  function packageOptionLabel(pkg) {
    const label = appLabelByPkg.value.get(pkg)
    return label && label !== pkg ? `${label} · ${pkg}` : pkg
  }

  watch(pkgOptions, list => {
    if (!activePkg.value) activePkg.value = list[0] || ''
    else if (!list.includes(activePkg.value)) activePkg.value = list[0] || ''
  })

  /** 接入方式展示（新增时可选，编辑时只读徽章） */
  function kindInfo(k) {
    return types.find(t => t.key === k) || { key: k, label: k || '未知', icon: '📱' }
  }

  /** 编辑模式概览里的屏幕摘要（与配置表单区分开，避免重复） */
  const screenSummary = computed(() => {
    return formatScreenSummary(current.value)
  })

  /** 当前应用列表缓存 key（按设备 id） */
  function appCacheKey() {
    return store.deviceId ? `device:${store.deviceId}` : ''
  }

  /** 从缓存恢复应用列表（切换设备时避免重新读取） */
  function restoreAppCache(id) {
    const cached = appCache.get(`device:${id}`)
    appList.value = cached?.list || []
  }

  /** 把设备记录载入表单（编辑模式）；syncPkg 仅在切换设备/初始化时恢复默认包名。 */
  function loadForm(d, { syncPkg = false } = {}) {
    mode.value = 'edit'
    form.name = d.name || ''
    form.kind = d.kind || 'redroid'
    form.addr = d.addr || ''
    form.screen_mode = d.screen_mode || 'virtual'
    form.vd_res = d.vd_res || '1920x1080'
    form.vd_dpi = d.vd_dpi || 0
    form.fps = d.fps || 30
    restoreAppCache(d.id)
    if (syncPkg) activePkg.value = (d.pkg || '').trim()
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
    form.fps = 30
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
      appList.value = []
    }
    errorMsg.value = ''
  }

  /** 下拉框切换设备：手动断开旧连接（不自动重连），等待用户点连接 */
  function onDeviceSelect() {
    if (connected.value || consoleRuntime.reconnectTimer.value) {
      consoleRuntime.cancelReconnect()
      cleanup(true)
    }
    // 重置自动重连退避计数（原代码误写隐式全局 `reconnectAttempts = 0`，严格模式下抛
    // ReferenceError；拆分时改为正确的运行时字段，行为恢复设计意图）
    consoleRuntime.reconnectAttempts.value = 0
    errorMsg.value = ''
    const d = current.value
    if (d) loadForm(d, { syncPkg: true })
    else { mode.value = 'edit'; appList.value = [] }
  }

  /** 连接成功后只拉设备列表（不扫描）：更新下拉「在线/离线」状态标签。
   * 失败静默——状态刷新属附带增强，不打扰投屏主流程。 */
  async function refreshDeviceStatus() {
    try {
      devicesData.value = await api.listDevices()
    } catch (e) { /* 状态刷新失败不提示 */ }
  }

  /** 刷新：扫描 adb 自动入库新设备，再拉列表 */
  async function refreshDevices() {
    if (scanning.value) return
    const previousDeviceId = store.deviceId
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
      if (d && mode.value === 'edit') {
        // 刷新同一设备时保留用户手动切换的当前包名；仅在扫描导致设备选择改变时恢复该设备旧配置。
        loadForm(d, { syncPkg: previousDeviceId !== store.deviceId })
      }
      else if (!d) { mode.value = 'edit'; appList.value = [] }
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
      // pkg 是旧版本设备配置字段；设备设置不再编辑它，保留旧值避免保存投屏参数时清空兼容数据。
      pkg: mode.value === 'edit' ? (current.value?.pkg || null) : null,
      fps: Number(form.fps) || 30
    }
  }

  /** 判断本次保存的 payload 相对旧配置是否触碰投屏会话参数（与服务端
   *  session_affecting_change 同口径：kind/addr/screen_mode/vd_res/vd_dpi/fps）。
   *  仅名称变更时服务端保持会话，前端据此前提示「不断开投屏」。 */
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
   *  前端无需手动重连（避免与自动重连并发导致双连接）；仅改名称时
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
      if (nd) loadForm(nd, { syncPkg: true })
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
        loadForm(devices.value[0], { syncPkg: true })
      } else {
        store.deviceId = null
        mode.value = 'edit'
        appList.value = []
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

  /** 从设备读取已安装应用（scrcpy list_apps，带真实软件名），合并到右侧包名下拉。 */
  async function loadApps() {
    if (appLoading.value) return
    if (!store.deviceId) return toast('请先选择设备', 'warn')
    const key = appCacheKey()
    const cached = appCache.get(key)
    // 5 分钟内直接用缓存，应用列表不是经常变
    if (cached && Date.now() - cached.ts < APP_CACHE_TTL) {
      appList.value = cached.list
      return
    }
    appLoading.value = true
    try {
      const list = await api.listApps(store.deviceId)
      appList.value = list || []
      appCache.set(key, { list: appList.value, ts: Date.now() })
    } catch (e) {
      appList.value = []
      toast('读取应用失败：' + e.message, 'error')
    } finally {
      appLoading.value = false
    }
  }

  // ---------- 分区导入/导出在右侧面板顶部应用分区下拉旁 ----------
  const impFile = ref(null) // 分区快照 zip 选择（应用分区行「⬇ 导入」触发）

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

  // ---------- 工具条快捷动作与「更多」菜单 ----------

  function key(k) {
    if (!connected.value) return
    const codes = { HOME: 3, BACK: 4, APP_SWITCH: 187, VOL_UP: 24, VOL_DOWN: 25 }
    sendControl({ type: 'press', keycode: codes[k] || 0 })
  }

  const toolbarMoreOpen = ref(false)
  const toolbarMoreButton = ref(null)
  const toolbarMoreStyle = reactive({ top: '0px', left: '0px' })

  function closeToolbarMore() { toolbarMoreOpen.value = false }
  function positionToolbarMore() {
    const rect = toolbarMoreButton.value?.getBoundingClientRect()
    if (!rect) return
    const menuWidth = 168
    toolbarMoreStyle.top = `${Math.round(rect.bottom + 4)}px`
    toolbarMoreStyle.left = `${Math.round(Math.max(8, Math.min(rect.left, window.innerWidth - menuWidth - 8)))}px`
  }
  function toggleToolbarMore() {
    toolbarMoreOpen.value = !toolbarMoreOpen.value
    if (toolbarMoreOpen.value) nextTick(positionToolbarMore)
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

  function splitTextForScrcpy(text, maxBytes = 300) {
    const encoder = new TextEncoder()
    const chunks = []
    let current = ''
    let currentBytes = 0
    for (const char of text) {
      const charBytes = encoder.encode(char).length
      if (current && currentBytes + charBytes > maxBytes) {
        chunks.push(current)
        current = ''
        currentBytes = 0
      }
      current += char
      currentBytes += charBytes
    }
    if (current) chunks.push(current)
    return chunks
  }

  async function clipboard() {
    if (!connected.value) return toast('请先连接设备', 'error')
    if (!navigator.clipboard?.readText) {
      return toast('当前浏览器不允许读取系统剪贴板，请使用 HTTPS 或 localhost', 'warn')
    }
    let text
    try {
      text = await navigator.clipboard.readText()
    } catch (e) {
      return toast('读取系统剪贴板失败，请允许浏览器访问剪贴板', 'error')
    }
    if (!text) return toast('系统剪贴板为空', 'warn')

    // scrcpy 文本控制消息单条上限为 300 字节；按 UTF-8 字符切块，避免中文
    // 或 emoji 被截断。DataChannel 本身有序，多个 text 消息会按原顺序提交。
    const chunks = splitTextForScrcpy(text)
    for (const chunk of chunks) sendControl({ type: 'text', text: chunk })
    toast(`已粘贴 ${text.length} 个字符`, 'success')
  }

  function launchGame() {
    if (!connected.value) return toast('请先连接设备', 'error')
    if (!activePkg.value) return toast('请先在右侧选择包名', 'warn')
    sendControl({ type: 'start_app', app: activePkg.value })
    if (store.deviceId) appStartedDevices.add(store.deviceId)
    appHintDismissed.value = true
    toast(`正在启动 ${activePkg.value}…`, 'info')
  }

  // 设备选择持久化：刷新后自动恢复选中设备（运行态/画面恢复的前提）
  watch(() => store.deviceId, id => {
    if (id) localStorage.setItem('gb_device_id', id)
  })

  const deviceSettingsContext = {
    settingsOpen, mode, form, types, vdPresets, fpsPresets, formDirty,
    configApplying, saveSettings, cancelSettings, current, connected, kindInfo, screenSummary,
  }
  const workspaceContextBarContext = {
    activePkg, pkgOptions, current, appLoading, loadApps, packageOptionLabel,
    exportPartition, openImport: () => impFile.value?.click(), onImportFile,
  }

  return {
    // 设备与设置弹窗
    vdPresets, fpsPresets, types, mode, form, scanning, configApplying,
    appList, appLoading, devices, current, currentName, pkgOptions, packageOptionLabel,
    kindInfo, screenSummary, formDirty, settingsOpen,
    loadForm,
    startAdd, openSettings, cancelSettings, onDeviceSelect, refreshDeviceStatus, refreshDevices,
    saveSettings, flushAndConnect, addDevice, removeDevice, disconnect, loadApps,
    // 分区导入导出
    impFile, exportPartition, onImportFile,
    // 工具条快捷动作与菜单
    key, toolbarMoreOpen, toolbarMoreButton, toolbarMoreStyle,
    closeToolbarMore, toggleToolbarMore, shot, rotate, clipboard, launchGame,
    // 上下文对象
    deviceSettingsContext, workspaceContextBarContext,
  }
}
