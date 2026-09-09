import { computed, ref } from 'vue'
import { load as loadYaml } from 'js-yaml'
import { api } from '../../api'

/** 归一化 action 坐标点（数组或 {x,y}）→ {x,y}；非法返回 null */
function normalizedPoint(value) {
  if (Array.isArray(value) && value.length >= 2) {
    const x = Number(value[0]); const y = Number(value[1])
    return Number.isFinite(x) && Number.isFinite(y) ? { x, y } : null
  }
  if (value && typeof value === 'object') {
    const x = Number(value.x); const y = Number(value.y)
    return Number.isFinite(x) && Number.isFinite(y) ? { x, y } : null
  }
  return null
}

/**
 * 按键映射面板：映射方案列表/选择/保存/删除、当前激活模型、
 * 投屏画面上的映射可视化（keymapOverlay）与状态徽标（keymapStatus）。
 * 自 Console.vue 原样拆出，行为零变化。
 */
export function useConsoleKeymap({
  api,
  toast,
  packageId,
  keyboardMode,
  // 键盘/映射控制器与按压集合（Console 持有）
  keymap,
  keymapPressed,
  // 投屏几何（templates composable）
  videoElement,
  videoWrap,
  deviceRectStyle,
  // 步骤编辑器选点（templates composable）
  pickCoord,
}) {
  const keymaps = ref([])
  const activeKeymapName = ref('')
  const activeKeymapDisplayName = ref('')
  const activeKeymapModel = ref(null)
  const keymapLoading = ref(false)
  const keymapError = ref('')
  const remoteKeymapRunning = ref(false)
  const keymapOptions = computed(() => Array.isArray(keymaps.value) ? keymaps.value : [])
  let keymapLoadSerial = 0
  let keymapDetailSerial = 0
  let keymapSaveSerial = 0
  let keymapPackageId = null
  let activeKeymapPackageId = ''
  let keymapListLoading = false
  let keymapDetailLoading = false
  const keymapSaving = ref(false)

  function refValue(value) {
    return value && typeof value === 'object' && 'value' in value ? value.value : value
  }

  function currentPackageId() {
    return String(refValue(packageId) || '')
  }

  function keymapItemId(item) {
    return String(item && (item.id || item.file || item.name) || '')
  }

  function syncKeymapLoading() {
    keymapLoading.value = keymapListLoading || keymapDetailLoading
  }

  function invalidateKeymapDetail() {
    keymapDetailSerial += 1
    keymapDetailLoading = false
    syncKeymapLoading()
  }

  /**
   * keymap GET 返回通用资源条目 JSON（content 原文 + 注记 name/binding_count/valid，
   * P11.6 后不再携带解析模型）；这里按需解析为输入控制器/可视化消费的
   * {name, bindings} 模型。注记 valid=false（服务端 schema 校验失败）时抛出带
   * 诊断的错误，避免把坏方案静默装进输入链路。
   */
  function keymapModelFromResponse(rep) {
    if (!rep || typeof rep !== 'object') return null
    if (rep.valid === false) {
      const diagnostics = Array.isArray(rep.diagnostics) ? rep.diagnostics.join('；') : ''
      throw new Error(`映射方案无效${diagnostics ? `：${diagnostics}` : ''}`)
    }
    let parsed
    try {
      parsed = loadYaml(String(rep.content ?? ''))
    } catch {
      return null
    }
    if (!parsed || typeof parsed !== 'object' || !Array.isArray(parsed.bindings)) return null
    return {
      version: 1,
      name: parsed.name || rep.name || '',
      bindings: parsed.bindings,
    }
  }

  function clearKeymapSelection({ invalidate = true } = {}) {
    if (invalidate) invalidateKeymapDetail()
    activeKeymapName.value = ''
    activeKeymapDisplayName.value = ''
    activeKeymapPackageId = ''
    activeKeymapModel.value = null
    keymapError.value = ''
    keymapPressed.clear()
  }

  function resetKeymapSelection() {
    clearKeymapSelection()
  }

  async function loadKeymaps(pkg) {
    const requestedPkg = String(pkg || '')
    const serial = ++keymapLoadSerial
    const scopeChanged = keymapPackageId !== requestedPkg
    keymapPackageId = requestedPkg
    if (scopeChanged) {
      clearKeymapSelection()
      keymaps.value = []
    }
    if (!requestedPkg) {
      keymapListLoading = false
      syncKeymapLoading()
      return
    }
    keymapListLoading = true
    syncKeymapLoading()
    try {
      const list = await api.listKeymaps(requestedPkg)
      if (serial !== keymapLoadSerial) return
      const nextKeymaps = Array.isArray(list)
        ? list
        : (Array.isArray(list?.keymaps) ? list.keymaps : [])
      keymaps.value = nextKeymaps

      // 同一 Package 刷新只替换列表，不清除当前已应用方案；但如果方案已经
      // 被删除/重命名，则必须停用旧模型，避免列表与实际输入状态不一致。
      if (activeKeymapName.value && activeKeymapPackageId === requestedPkg) {
        const selected = nextKeymaps.find(item => keymapItemId(item) === activeKeymapName.value)
        if (!selected) clearKeymapSelection()
        else activeKeymapDisplayName.value = selected.name || selected.file || activeKeymapDisplayName.value
      }
    } catch (e) {
      if (serial === keymapLoadSerial) keymapError.value = `读取映射失败：${e.message}`
    } finally {
      if (serial === keymapLoadSerial) {
        keymapListLoading = false
        syncKeymapLoading()
      }
    }
  }

  async function onKeymapChange(item = null) {
    const requestSerial = ++keymapDetailSerial
    const requestedPkg = currentPackageId()
    keymap.releaseAll()
    activeKeymapModel.value = null
    keymapError.value = ''
    if (item && typeof item === 'object') {
      activeKeymapName.value = keymapItemId(item)
      activeKeymapDisplayName.value = item.name || item.file || item.id || ''
    } else {
      const selected = keymapOptions.value.find(candidate =>
        (candidate.id || candidate.file || candidate.name) === activeKeymapName.value)
      activeKeymapDisplayName.value = selected?.name || selected?.file || activeKeymapName.value || ''
    }
    activeKeymapPackageId = requestedPkg
    const requestedName = activeKeymapName.value
    if (!requestedName || !requestedPkg) {
      keymapDetailLoading = false
      syncKeymapLoading()
      return
    }
    keymapDetailLoading = true
    syncKeymapLoading()
    const isCurrentRequest = () => (
      requestSerial === keymapDetailSerial
      && requestedPkg === currentPackageId()
      && requestedName === activeKeymapName.value
      && activeKeymapPackageId === requestedPkg
    )
    try {
      const rep = await api.getKeymap(requestedName, requestedPkg)
      if (!isCurrentRequest()) return
      const model = keymapModelFromResponse(rep)
      if (!model) throw new Error('服务端返回的映射结构无效')
      activeKeymapModel.value = model
      activeKeymapDisplayName.value = model.name || activeKeymapDisplayName.value
    } catch (e) {
      if (!isCurrentRequest()) return
      clearKeymapSelection({ invalidate: false })
      keymapError.value = `加载映射失败：${e.message}`
      keymapDetailLoading = false
      syncKeymapLoading()
      toast(keymapError.value, 'error')
    } finally {
      if (isCurrentRequest()) {
        keymapDetailLoading = false
        syncKeymapLoading()
      }
    }
  }

  const keymapOverlay = computed(() => {
    const bindings = activeKeymapModel.value?.bindings
    if (!Array.isArray(bindings)) return []
    const vw = videoElement.value?.videoWidth || 1920
    const vh = videoElement.value?.videoHeight || 1080
    return bindings.map((binding, index) => {
      const action = binding?.action || {}
      const type = String(action.type || 'raw_key')
      const label = String(binding?.key || `键 ${index + 1}`)
      const active = keymapPressed.has(binding?.key)
      if (type === 'swipe') {
        const from = normalizedPoint(action.from)
        const to = normalizedPoint(action.to)
        if (!from || !to) return null
        const start = deviceRectStyle(from.x * vw, from.y * vh)
        const dx = (to.x - from.x) * vw
        const dy = (to.y - from.y) * vh
        const scale = Math.min(
          (videoWrap.value?.getBoundingClientRect?.().width || 0) / vw || 1,
          (videoWrap.value?.getBoundingClientRect?.().height || 0) / vh || 1,
        )
        return {
          id: `${label}-${index}`,
          type: 'swipe',
          label,
          active,
          style: { ...start, '--keymap-w': `${Math.hypot(dx, dy) * scale}px`, '--keymap-angle': `${Math.atan2(dy, dx) * 180 / Math.PI}deg` },
        }
      }
      const at = normalizedPoint(action.at)
      if (at) {
        return { id: `${label}-${index}`, type: type === 'hold' ? 'hold' : 'tap', label, active, style: deviceRectStyle(at.x * vw, at.y * vh) }
      }
      return { id: `${label}-${index}`, type: 'raw_key', label, active, style: { left: '12px', top: `${52 + index * 24}px`, transform: 'none' } }
    }).filter(Boolean)
  })

  const keymapStatus = computed(() => ({
    name: activeKeymapModel.value?.name || activeKeymapDisplayName.value,
    inactive: keyboardMode.value === 'text',
  }))

  function keymapFileName(name) {
    const value = String(name || '').trim().replace(/[\\/:*?"<>|]/g, '_').replace(/\s+/g, '_')
    return value || `keymap_${Date.now()}`
  }

  async function onKeymapSave(payload = {}) {
    if (keymapSaving.value || !payload.pkg || !payload.model || !payload.yaml) return false
    const saveSerial = ++keymapSaveSerial
    const savePkg = String(payload.pkg)
    keymapSaving.value = true
    keymapError.value = ''
    try {
      const source = payload.source
      if (source?.id) {
        await api.updateKeymap(source.id, savePkg, {
          content: payload.yaml,
          expected_version: payload.expected_version,
        })
      } else {
        await api.createKeymap({ pkg: savePkg, name: keymapFileName(payload.name), content: payload.yaml })
      }
      // 保存只负责持久化和刷新列表，不隐式把编辑结果应用到输入控制器。
      // Package 已切换或已有更新请求时，不把旧请求的结果写回当前上下文。
      if (saveSerial !== keymapSaveSerial || savePkg !== currentPackageId()) return true
      await loadKeymaps(savePkg)
      toast(source ? '映射方案已保存' : '映射方案已创建', 'success')
      return true
    } catch (e) {
      if (savePkg === currentPackageId()) {
        keymapError.value = `保存映射失败：${e.message}`
        toast(keymapError.value, 'error')
      }
      return false
    } finally {
      keymapSaving.value = false
    }
  }

  async function onKeymapDelete(payload = {}) {
    const id = payload.source?.id || payload.id || payload.name
    const requestedPkg = String(payload.pkg || '')
    if (!id || !requestedPkg) return
    try {
      await api.deleteKeymap(id, requestedPkg)
      if (id === activeKeymapName.value || payload.name === activeKeymapDisplayName.value) resetKeymapSelection()
      await loadKeymaps(requestedPkg)
      toast('映射方案已删除', 'success')
    } catch (e) {
      toast(`删除映射失败：${e.message}`, 'error')
    }
  }

  const keymapPanelContext = {
    api,
    toast,
    pkg: packageId,
    packageId,
    keymaps,
    keymapOptions,
    selectedName: activeKeymapDisplayName,
    activeKeymapName,
    usedName: activeKeymapDisplayName,
    model: activeKeymapModel,
    activeKeymapModel,
    loading: keymapLoading,
    keymapLoading,
    saving: keymapSaving,
    keymapSaving,
    error: keymapError,
    keymapError,
    refresh: () => loadKeymaps(currentPackageId()),
    onRefresh: () => loadKeymaps(currentPackageId()),
    select: onKeymapChange,
    onSelect: onKeymapChange,
    onSave: onKeymapSave,
    onRequestPoint: () => pickCoord(),
    onDelete: onKeymapDelete,
    onSaved: () => loadKeymaps(currentPackageId()),
    onDeleted: (name) => {
      if (!name || name === activeKeymapName.value) resetKeymapSelection()
      return loadKeymaps(currentPackageId())
    },
  }

  return {
    keymaps, keymapOptions, activeKeymapName, activeKeymapDisplayName,
    activeKeymapModel, keymapLoading, keymapSaving, keymapError, remoteKeymapRunning,
    keymapOverlay, keymapStatus,
    loadKeymaps, onKeymapChange, onKeymapSave, resetKeymapSelection,
    keymapPanelContext,
  }
}
