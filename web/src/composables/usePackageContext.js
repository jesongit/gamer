// Package 上下文条（右侧顶部，plan §28）的逻辑收敛：当前 Package 下拉 +
// 导入/导出/新建/复制/删除。动作全部作用于 PackageStore（§38：Current Package
// 由 Core Store 统一管理）；资源面板经注入的刷新回调在包切换/导入后全量重拉。
//
// §40：旧的「读取应用 + 导入 + 导出 + 编辑」同栏设计已删除——Android 应用
// 控制归左侧设备区域，Installed/Editable 双层语义不复存在。
import { computed, reactive, ref } from 'vue'
import { api as defaultApi } from '../api'
import { sha256Hex } from '../workspace/plugin-center/registry-client'
import {
  packageStore, loadPackages, selectPackage, refreshPackages, currentPackageId,
} from '../package-store'

/** Package id 输入规整：小写域 [a-z0-9._-]，与服务端 validate_scope_id 对齐。 */
export function normalizePackageId(value) {
  return String(value || '').trim().toLowerCase()
}

export function isValidPackageId(value) {
  const id = normalizePackageId(value)
  return /^[a-z0-9][a-z0-9._-]*$/.test(id) && !/^[.]+$/.test(id)
}

/**
 * 依赖注入：toast / confirmDialog(message)→boolean / refreshAll()（包切换或
 * 导入后重拉资源面板）/ download(blob, filename) / api。均可替换以便测试。
 * currentApp 提供当前设备的 Android 应用包名与显示名；loadCurrentApps 用于在
 * 显示名尚未读取时补读应用列表。
 */
export function usePackageContext({
  api = defaultApi,
  toast,
  confirmDialog = message => window.confirm(message),
  refreshAll,
  download,
  currentApp,
  loadCurrentApps,
} = {}) {
  const busy = ref(false)

  const currentId = currentPackageId
  const packages = computed(() => packageStore.packages)
  const pkgOptions = computed(() => packages.value.map(p => p.id))

  function optionLabel(id) {
    const p = packages.value.find(x => x.id === id)
    return p?.name && p.name !== p.id ? `${p.name} · ${p.id}` : p?.id || id
  }

  function onPackageChange(e) {
    selectPackage(e?.target?.value || null)
    Promise.resolve(refreshAll?.()).catch(() => {})
  }

  function saveBlob(blob, filename) {
    if (typeof download === 'function') return download(blob, filename)
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = filename
    a.click()
    setTimeout(() => URL.revokeObjectURL(url), 1000)
  }

  // ---------- 导入（.gamerpkg → Local Package；同 id 409 → 覆盖确认） ----------
  function pickImportFile(inputEl) {
    if (busy.value) return
    inputEl?.click()
  }

  /** 覆盖确认弹窗（plan §9/§36）：明示整体替换 + Required Plugin 缺失提示。 */
  const overwriteModal = reactive({
    open: false, submitting: false, error: '',
    summary: { id: '', name: '', version: '', author: '', androidTargets: [], plugins: [] },
    // 缺少的 Required Plugin id 列表（对照 GET /api/extensions；提示但允许继续导入）
    missingRequired: [],
    file: null, bytes: null, sha256: '',
  })

  /**
   * plan §36：导入前对照已安装扩展，缺 Required Plugin 时黄条提示
   * 「缺少插件 X，导入后部分功能不可用，安装插件后自动恢复」，但不阻塞导入
   * （插件数据随包保留，安装后 dormant 数据自动恢复语义）。扩展列表不可得时
   * 宁可不提示，也不阻塞导入。
   */
  async function resolveOverwriteMissingRequired(summary) {
    overwriteModal.missingRequired = []
    const required = (Array.isArray(summary?.plugins) ? summary.plugins : [])
      .filter(dep => dep && dep.id && dep.required !== false)
      .map(dep => dep.id)
    if (!required.length) return
    let installedIds = null
    try {
      const rep = await api.listExtensions()
      installedIds = new Set(
        (Array.isArray(rep?.extensions) ? rep.extensions : []).map(x => x?.id),
      )
    } catch { /* 对照失败不阻塞导入 */ }
    if (!installedIds) return
    overwriteModal.missingRequired = required.filter(id => !installedIds.has(id))
  }

  async function openOverwriteModal(summary, file, bytes, sha256) {
    overwriteModal.summary = {
      id: summary?.id || '',
      name: summary?.name || '',
      version: summary?.version || '',
      author: summary?.author || '',
      androidTargets: Array.isArray(summary?.targets?.android?.packages)
        ? summary.targets.android.packages : [],
      plugins: Array.isArray(summary?.plugins) ? summary.plugins : [],
    }
    overwriteModal.file = file
    overwriteModal.bytes = bytes
    overwriteModal.sha256 = sha256
    overwriteModal.error = ''
    overwriteModal.open = true
    await resolveOverwriteMissingRequired(overwriteModal.summary)
  }

  function closeOverwrite() {
    overwriteModal.open = false
    overwriteModal.error = ''
  }

  async function confirmOverwrite() {
    if (overwriteModal.submitting) return
    overwriteModal.submitting = true
    overwriteModal.error = ''
    try {
      await api.importPackageArchive(overwriteModal.bytes, {
        expectedSha256: overwriteModal.sha256, overwrite: true,
      })
      overwriteModal.open = false
      toast(`配置已覆盖导入：${overwriteModal.summary.id}`, 'success')
      await refreshPackages({ api })
      if (overwriteModal.summary.id) selectPackage(overwriteModal.summary.id)
      await Promise.resolve(refreshAll?.()).catch(() => {})
    } catch (e) {
      overwriteModal.error = e?.message || '覆盖导入失败'
    } finally {
      overwriteModal.submitting = false
    }
  }

  async function importPackage(file) {
    if (!file || busy.value) return
    busy.value = true
    try {
      const bytes = await file.arrayBuffer()
      const sha256 = await sha256Hex(bytes)
      try {
        const rep = await api.importPackageArchive(bytes, { expectedSha256: sha256 })
        toast(`配置已导入：${rep?.id || '?'}@${rep?.version || '?'}`, 'success')
        await refreshPackages({ api })
        if (rep?.id) selectPackage(rep.id)
        await Promise.resolve(refreshAll?.()).catch(() => {})
      } catch (e) {
        // 409 {error:"package_exists", existing:<manifest 摘要>} → 覆盖确认弹窗
        if (e?.status === 409 && e?.data?.existing) {
          await openOverwriteModal(e.data.existing, file, bytes, sha256)
          return
        }
        throw e
      }
    } catch (e) {
      toast(`导入失败：${e?.message || '请重试'}`, 'error')
    } finally {
      busy.value = false
    }
  }

  async function onImportPicked(e) {
    const input = e?.target
    try {
      await importPackage(input?.files?.[0])
    } finally {
      if (input) input.value = ''
    }
  }

  // ---------- 导出（当前 Local Package → .gamerpkg；媒体素材可选携带） ----------
  //
  // Phase 8 §11.1：默认导出不含原始大视频（归档只带 media/index.json 引用
  // 登记）；存在被引用素材时弹确认框——列出素材与总大小 + 隐私提示，由用户
  // 勾选「包含媒体素材」后带 ?include_media=true 导出。
  const exportModal = reactive({
    open: false, submitting: false, error: '',
    packageId: '',
    entries: [],       // [{id,name,size,plugin_id,kind,state}]
    totalBytes: 0,
    includeMedia: false,
  })

  async function doExport(id, includeMedia) {
    const { blob, filename, sha256 } = await api.exportPackageArchive(id, { includeMedia })
    const name = filename || `${id}.gamerpkg`
    saveBlob(blob, name)
    const mediaNote = includeMedia ? '含媒体素材' : '不含媒体素材'
    toast(`已导出 ${name}（${mediaNote}${sha256 ? `，SHA-256 ${sha256.slice(0, 12)}…` : ''}）`, 'success')
  }

  /** 导出入口：先查媒体引用——无素材直接导出；有素材弹确认框。 */
  async function exportPackage() {
    const id = currentId.value
    if (!id || busy.value) return
    busy.value = true
    try {
      let entries = []
      let totalBytes = 0
      try {
        // 查询面：GET /api/packages/:pkg 的 media_refs / media_total_bytes
        //（refs_for_package）；查询失败视为无素材引用，直接按默认导出。
        const detail = await api.getPackage(id)
        entries = Array.isArray(detail?.media_refs) ? detail.media_refs : []
        totalBytes = Number(detail?.media_total_bytes) || 0
      } catch { /* 查询失败不阻塞导出 */ }
      if (!entries.length) {
        await doExport(id, false)
      } else {
        exportModal.packageId = id
        exportModal.entries = entries
        exportModal.totalBytes = totalBytes
          || entries.reduce((sum, e) => sum + (Number(e?.size) || 0), 0)
        exportModal.includeMedia = false
        exportModal.error = ''
        exportModal.open = true
      }
    } catch (e) {
      toast(`导出失败：${e?.message || '请重试'}`, 'error')
    } finally {
      busy.value = false
    }
  }

  function closeExport() {
    exportModal.open = false
    exportModal.error = ''
  }

  async function confirmExport() {
    if (exportModal.submitting) return
    const id = exportModal.packageId
    const includeMedia = exportModal.includeMedia
    exportModal.submitting = true
    exportModal.error = ''
    try {
      await doExport(id, includeMedia)
      exportModal.open = false
    } catch (e) {
      exportModal.error = e?.message || '导出失败'
    } finally {
      exportModal.submitting = false
    }
  }

  // ---------- 新建 / 复制（同一表单弹窗） ----------
  const formModal = reactive({
    open: false, mode: 'create', submitting: false, error: '',
    form: { id: '', name: '', version: '1.0.0', androidPackagesText: '' },
  })

  function unwrap(value) {
    if (typeof value === 'function') return value()
    if (value && typeof value === 'object' && 'value' in value) return value.value
    return value
  }

  function currentAppSnapshot() {
    const value = unwrap(currentApp)
    if (typeof value === 'string') return { pkg: value, label: '' }
    return value && typeof value === 'object' ? value : null
  }

  /** 将当前设备的 Android 应用填入新建配置表单。 */
  async function fillCurrentApp() {
    let app = currentAppSnapshot()
    let packageName = String(app?.pkg || app?.package_name || app?.packageName || '').trim()
    if (!packageName) {
      toast('当前没有可用的应用包名，请先在左侧应用下拉中选择', 'warn')
      return false
    }

    // 连接后通常已有缓存；名称缺失时补读一次，取得 API 返回的真实 label。
    if (!String(app?.label || app?.name || '').trim() && typeof loadCurrentApps === 'function') {
      try { await loadCurrentApps() } catch { /* 名称读取失败时仍可使用包名填充 */ }
      app = currentAppSnapshot()
      packageName = String(app?.pkg || app?.package_name || app?.packageName || packageName).trim()
    }

    formModal.form.id = normalizePackageId(packageName)
    formModal.form.androidPackagesText = packageName
    formModal.form.name = String(app?.label || app?.name || packageName).trim()
    formModal.error = ''
    return true
  }

  function openCreate() {
    formModal.mode = 'create'
    // 默认 Android Targets = `*`（通用配置、零插件依赖）；需要绑定具体应用时
    // 改填包名或用「填入当前应用」。
    formModal.form = { id: '', name: '', version: '1.0.0', androidPackagesText: '*' }
    formModal.error = ''
    formModal.open = true
  }

  function openDuplicate() {
    const id = currentId.value
    if (!id) return
    const src = packages.value.find(p => p.id === id)
    formModal.mode = 'duplicate'
    formModal.form = {
      id: `user.${id.replace(/^user\./, '')}.copy`,
      name: src?.name ? `${src.name}（副本）` : '',
      version: src?.version || '1.0.0',
      androidPackagesText: (src?.targets?.android?.packages || []).join(', '),
    }
    formModal.error = ''
    formModal.open = true
  }

  function closeForm() {
    formModal.open = false
    formModal.error = ''
  }

  /** "a, b\nc" → 去重数组（逗号/中文逗号/换行分隔）。 */
  function parseAndroidPackages(text) {
    const seen = new Set()
    for (const part of String(text || '').split(/[\n,，]/)) {
      const pkg = part.trim()
      if (pkg) seen.add(pkg)
    }
    return [...seen]
  }

  async function submitForm() {
    if (formModal.submitting) return
    const id = normalizePackageId(formModal.form.id)
    const version = formModal.form.version.trim() || '0.1.0'
    const name = formModal.form.name.trim()
    const androidPackages = parseAndroidPackages(formModal.form.androidPackagesText)
    if (!isValidPackageId(id)) {
      formModal.error = '配置 ID 非法：小写字母/数字开头，仅含小写字母、数字、点、下划线、连字符'
      return
    }
    formModal.submitting = true
    formModal.error = ''
    try {
      if (formModal.mode === 'create') {
        await api.createPackage({
          id, version,
          ...(name ? { name } : {}),
          targets: { android: { packages: androidPackages } },
        })
        toast(`配置已创建：${id}`, 'success')
      } else {
        await api.duplicatePackage(currentId.value, id)
        toast(`已复制为 ${id}`, 'success')
      }
      formModal.open = false
      await refreshPackages({ api })
      selectPackage(id)
      await Promise.resolve(refreshAll?.()).catch(() => {})
    } catch (e) {
      formModal.error = e?.message || '保存失败'
    } finally {
      formModal.submitting = false
    }
  }

  // ---------- 删除（确认后；绑定任务由服务端挂起） ----------
  const deleteModal = reactive({ open: false, submitting: false, error: '', target: null })

  function openDelete() {
    const id = currentId.value
    if (!id) return
    deleteModal.target = packages.value.find(p => p.id === id) || { id }
    deleteModal.error = ''
    deleteModal.open = true
  }

  function closeDelete() {
    deleteModal.open = false
    deleteModal.error = ''
  }

  async function confirmDelete() {
    if (deleteModal.submitting) return
    deleteModal.submitting = true
    deleteModal.error = ''
    try {
      await api.deletePackage(deleteModal.target.id)
      deleteModal.open = false
      toast(`配置已删除：${deleteModal.target.id}`, 'success')
      await refreshPackages({ api })
      await Promise.resolve(refreshAll?.()).catch(() => {})
    } catch (e) {
      deleteModal.error = e?.message || '删除失败'
    } finally {
      deleteModal.submitting = false
    }
  }

  // ---------- 详情弹窗（plan §18/§21/§37：manifest 全字段 + 插件五态 +
  // 元数据编辑 + §17 兼容性检查；数据走 GET/PUT /api/packages/:pkg） ----------
  const detailModal = reactive({
    open: false, loading: false, error: '',
    packageId: '',
    detail: null,        // normalizeDetail 形态 {package, stats, pluginStates}
    editing: false, saving: false, editError: '', editConflict: false,
    form: { name: '', version: '', author: '', androidPackagesText: '', plugins: [] },
    compat: { input: '', checking: false, error: '', result: null },
  })

  /** GET /api/packages/:pkg 响应 → 组件友好形态（plugin_states → pluginStates）。 */
  function normalizeDetail(rep) {
    const pkg = rep?.package || {}
    return {
      package: {
        id: pkg.id || '',
        name: pkg.name || '',
        version: pkg.version || '',
        author: pkg.author || '',
        revision: pkg.revision ?? 0,
        targets: {
          android: {
            packages: Array.isArray(pkg.targets?.android?.packages)
              ? pkg.targets.android.packages : [],
          },
        },
        plugins: Array.isArray(pkg.plugins) ? pkg.plugins : [],
      },
      stats: {
        files: rep?.stats?.files ?? 0,
        bytes: rep?.stats?.bytes ?? 0,
        plugins: Array.isArray(rep?.stats?.plugins) ? rep.stats.plugins : [],
      },
      pluginStates: Array.isArray(rep?.plugin_states) ? rep.plugin_states : [],
    }
  }

  async function reloadDetail() {
    detailModal.loading = true
    detailModal.error = ''
    try {
      detailModal.detail = normalizeDetail(await api.getPackage(detailModal.packageId))
    } catch (e) {
      detailModal.error = e?.message || '加载配置详情失败'
    } finally {
      detailModal.loading = false
    }
  }

  async function openDetail(packageId) {
    const id = packageId || currentId.value
    if (!id) return
    detailModal.packageId = id
    detailModal.detail = null
    detailModal.editing = false
    detailModal.editConflict = false
    detailModal.editError = ''
    detailModal.compat = { input: '', checking: false, error: '', result: null }
    detailModal.open = true
    await reloadDetail()
  }

  function closeDetail() {
    detailModal.open = false
    detailModal.error = ''
  }

  function startEdit() {
    const p = detailModal.detail?.package
    if (!p) return
    detailModal.form = {
      name: p.name || '',
      version: p.version || '',
      author: p.author || '',
      androidPackagesText: (p.targets?.android?.packages || []).join(', '),
      plugins: p.plugins.map(dep => ({
        id: dep?.id || '', required: dep?.required !== false,
      })),
    }
    detailModal.editConflict = false
    detailModal.editError = ''
    detailModal.editing = true
  }

  function cancelEdit() {
    detailModal.editing = false
    detailModal.editError = ''
    detailModal.editConflict = false
  }

  function addPluginDep() {
    detailModal.form.plugins.push({ id: '', required: true })
  }

  function removePluginDep(index) {
    detailModal.form.plugins.splice(index, 1)
  }

  function buildPluginsPayload() {
    const map = {}
    for (const dep of detailModal.form.plugins) {
      const id = String(dep?.id || '').trim()
      if (id) map[id] = dep?.required !== false
    }
    return map
  }

  /** 保存元数据（§37）；条件更新 expected_revision，409 冲突后可重试或 force。 */
  async function saveEdit({ force = false } = {}) {
    if (detailModal.saving || !detailModal.detail?.package) return
    const body = {
      targets: { android: { packages: parseAndroidPackages(detailModal.form.androidPackagesText) } },
      plugins: buildPluginsPayload(),
    }
    const name = detailModal.form.name.trim()
    const version = detailModal.form.version.trim()
    const author = detailModal.form.author.trim()
    if (name) body.name = name
    if (version) body.version = version
    if (author) body.author = author
    if (force) body.force = true
    else body.expected_revision = detailModal.detail.package.revision
    detailModal.saving = true
    detailModal.editError = ''
    detailModal.editConflict = false
    try {
      const updated = await api.updatePackage(detailModal.packageId, body)
      detailModal.detail = {
        ...detailModal.detail,
        package: normalizeDetail({ package: updated }).package,
      }
      detailModal.editing = false
      toast(`配置元数据已保存：${detailModal.packageId}`, 'success')
      await refreshPackages({ api })
    } catch (e) {
      if (e?.status === 409 && !force) {
        detailModal.editConflict = true
        detailModal.editError = '元数据已被其他页面修改，可重试（保留当前修改按最新版本重放）或强制覆盖。'
      } else {
        detailModal.editError = e?.message || '保存失败'
      }
    } finally {
      detailModal.saving = false
    }
  }

  /** 409 冲突后的「重试」：拉取最新 revision 并保留用户当前编辑。 */
  async function retryEdit() {
    if (detailModal.loading) return
    const keepForm = { ...detailModal.form }
    await reloadDetail()
    if (detailModal.error) return
    detailModal.form = keepForm
    detailModal.editConflict = false
    detailModal.editError = ''
  }

  /** §17 兼容性检查（warning 语义：不兼容仅提示不禁止）。 */
  async function checkCompatibility(androidPackage) {
    const input = String(androidPackage ?? (detailModal.compat.input || '')).trim()
    if (!input || detailModal.compat.checking) return
    detailModal.compat.input = input
    detailModal.compat.checking = true
    detailModal.compat.error = ''
    detailModal.compat.result = null
    try {
      detailModal.compat.result = await api.packageCompatibility(detailModal.packageId, input)
    } catch (e) {
      detailModal.compat.error = e?.message || '兼容性检查失败'
    } finally {
      detailModal.compat.checking = false
    }
  }

  return {
    busy, packages, pkgOptions, currentId, optionLabel, onPackageChange,
    loadPackages, refreshPackages,
    pickImportFile, onImportPicked, importPackage,
    exportPackage, exportModal, confirmExport, closeExport,
    formModal, openCreate, openDuplicate, fillCurrentApp, submitForm, closeForm,
    overwriteModal, confirmOverwrite, closeOverwrite,
    deleteModal, openDelete, confirmDelete, closeDelete,
    detailModal, openDetail, closeDetail, reloadDetail,
    startEdit, cancelEdit, saveEdit, retryEdit, addPluginDep, removePluginDep,
    checkCompatibility, resolveOverwriteMissingRequired,
  }
}

/** 字节数人性化（详情 stats 展示）。 */
export function formatBytes(value) {
  const v = Number(value)
  if (!Number.isFinite(v) || v <= 0) return '0 B'
  if (v < 1024) return `${v} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let x = v
  let i = -1
  do {
    x /= 1024
    i += 1
  } while (x >= 1024 && i < units.length - 1)
  return `${x >= 100 ? x.toFixed(0) : x.toFixed(1)} ${units[i]}`
}
