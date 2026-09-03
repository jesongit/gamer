import { computed, ref } from 'vue'
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
 * 按键映射面板：映射方案列表/选择/保存/删除/导入导出、当前激活模型、
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

  function keymapModelFromResponse(rep) {
    const candidate = rep?.model || rep?.keymap || rep?.data?.model || rep?.data || rep
    return candidate && typeof candidate === 'object' && Array.isArray(candidate.bindings)
      ? candidate
      : null
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

  async function onKeymapExport() {
    if (!activePkg.value) return toast('请先选择应用分区', 'warn')
    try {
      const { blob, filename } = await api.exportKeymaps(activePkg.value)
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = filename || `${activePkg.value}-keymaps.zip`
      a.click()
      setTimeout(() => URL.revokeObjectURL(url), 0)
      toast('映射方案已导出', 'success')
    } catch (e) {
      toast(`导出映射失败：${e.message}`, 'error')
    }
  }

  async function onKeymapImport(file) {
    if (!file || !activePkg.value) return
    let preview
    try {
      preview = await api.importKeymaps(file, false, activePkg.value)
    } catch (e) {
      return toast(`导入映射失败：${e.message}`, 'error')
    }
    const invalid = Array.isArray(preview?.invalid) ? preview.invalid : []
    if (invalid.length) {
      const message = invalid.slice(0, 5).map(item => `${item.path || '文件'}（${item.diagnostics?.[0]?.message || '校验失败'}）`).join('；')
      return toast(`导入被阻止：${invalid.length} 个文件未通过校验：${message}`, 'error')
    }
    const overwrite = Array.isArray(preview?.overwrite) ? preview.overwrite : []
    if (overwrite.length && !window.confirm(`导入到 ${activePkg.value} 将覆盖 ${overwrite.length} 个映射方案，确认继续？`)) return
    try {
      await api.importKeymaps(file, true, activePkg.value)
      await loadKeymaps(activePkg.value)
      toast(`映射导入完成：新增 ${preview?.add?.length || 0} 个，覆盖 ${overwrite.length} 个`, 'success')
    } catch (e) {
      toast(`导入映射失败：${e.message}`, 'error')
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
    onExport: onKeymapExport,
    onImport: onKeymapImport,
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
