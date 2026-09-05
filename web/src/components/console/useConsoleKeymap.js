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
  activePkg,
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

  function resetKeymapSelection() {
    activeKeymapName.value = ''
    activeKeymapDisplayName.value = ''
    activeKeymapModel.value = null
    keymapError.value = ''
    keymapPressed.clear()
  }

  async function loadKeymaps(pkg) {
    const serial = ++keymapLoadSerial
    resetKeymapSelection()
    keymaps.value = []
    if (!pkg) return
    keymapLoading.value = true
    try {
      const list = await api.listKeymaps(pkg)
      if (serial !== keymapLoadSerial) return
      keymaps.value = Array.isArray(list) ? list : (Array.isArray(list?.keymaps) ? list.keymaps : [])
    } catch (e) {
      if (serial === keymapLoadSerial) keymapError.value = `读取映射失败：${e.message}`
    } finally {
      if (serial === keymapLoadSerial) keymapLoading.value = false
    }
  }

  async function onKeymapChange(item = null) {
    keymap.releaseAll()
    activeKeymapModel.value = null
    keymapError.value = ''
    if (item && typeof item === 'object') {
      activeKeymapName.value = item.id || item.file || item.name || ''
      activeKeymapDisplayName.value = item.name || item.file || item.id || ''
    } else {
      const selected = keymapOptions.value.find(candidate =>
        (candidate.id || candidate.file || candidate.name) === activeKeymapName.value)
      activeKeymapDisplayName.value = selected?.name || selected?.file || activeKeymapName.value || ''
    }
    if (!activeKeymapName.value || !activePkg.value) return
    keymapLoading.value = true
    try {
      const rep = await api.getKeymap(activeKeymapName.value, activePkg.value)
      const model = keymapModelFromResponse(rep)
      if (!model) throw new Error('服务端返回的映射结构无效')
      activeKeymapModel.value = model
      activeKeymapDisplayName.value = model.name || activeKeymapDisplayName.value
    } catch (e) {
      activeKeymapName.value = ''
      keymapError.value = `加载映射失败：${e.message}`
      toast(keymapError.value, 'error')
    } finally {
      keymapLoading.value = false
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
      if (type === 'swipe' || type === 'hold') {
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
        return { id: `${label}-${index}`, type: 'tap', label, active, style: deviceRectStyle(at.x * vw, at.y * vh) }
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
    if (!payload.pkg || !payload.model || !payload.yaml) return
    keymapLoading.value = true
    keymapError.value = ''
    try {
      const source = payload.source
      const rep = source?.id
        ? await api.updateKeymap(source.id, payload.pkg, {
          content: payload.yaml,
          expected_version: payload.expected_version,
        })
        : await api.createKeymap({ pkg: payload.pkg, name: keymapFileName(payload.name), content: payload.yaml })
      await loadKeymaps(payload.pkg)
      activeKeymapName.value = rep?.id || source?.id || ''
      activeKeymapDisplayName.value = rep?.name || payload.name || ''
      await onKeymapChange()
      toast(source ? '映射方案已保存' : '映射方案已创建', 'success')
      return true
    } catch (e) {
      keymapError.value = `保存映射失败：${e.message}`
      toast(keymapError.value, 'error')
      return false
    } finally {
      keymapLoading.value = false
    }
  }

  async function onKeymapDelete(payload = {}) {
    const id = payload.source?.id || payload.id || payload.name
    if (!id || !payload.pkg) return
    try {
      await api.deleteKeymap(id, payload.pkg)
      if (id === activeKeymapName.value || payload.name === activeKeymapDisplayName.value) resetKeymapSelection()
      await loadKeymaps(payload.pkg)
      toast('映射方案已删除', 'success')
    } catch (e) {
      toast(`删除映射失败：${e.message}`, 'error')
    }
  }

  const keymapPanelContext = {
    api,
    toast,
    pkg: activePkg,
    activePkg,
    keymaps,
    keymapOptions,
    selectedName: activeKeymapDisplayName,
    activeKeymapName,
    usedName: activeKeymapDisplayName,
    model: activeKeymapModel,
    activeKeymapModel,
    loading: keymapLoading,
    keymapLoading,
    error: keymapError,
    keymapError,
    refresh: () => loadKeymaps(activePkg.value),
    onRefresh: () => loadKeymaps(activePkg.value),
    select: onKeymapChange,
    onSelect: onKeymapChange,
    onSave: onKeymapSave,
    onRequestPoint: () => pickCoord(),
    onDelete: onKeymapDelete,
    onSaved: async (item) => {
      await loadKeymaps(activePkg.value)
      const name = item?.name || item?.keymap?.name
      if (name) {
        activeKeymapName.value = name
        await onKeymapChange()
      }
    },
    onDeleted: (name) => {
      if (!name || name === activeKeymapName.value) resetKeymapSelection()
      return loadKeymaps(activePkg.value)
    },
  }

  return {
    keymaps, keymapOptions, activeKeymapName, activeKeymapDisplayName,
    activeKeymapModel, keymapLoading, keymapError, remoteKeymapRunning,
    keymapOverlay, keymapStatus,
    loadKeymaps, onKeymapChange, resetKeymapSelection,
    keymapPanelContext,
  }
}
