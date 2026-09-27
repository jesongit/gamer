import { inspectPackageExport } from '../package-export-preview'
import { operationReporter } from '../workspace/operation-feedback'
// Package 上下文条（右侧顶部，plan §28）的逻辑收敛：当前 Package 下拉 +
// 导入/导出/新建/复制/删除。动作全部作用于 PackageStore（§38：Current Package
// 由 Core Store 统一管理）；资源面板经注入的刷新回调在包切换/导入后全量重拉。
//
// §40：旧的「读取应用 + 导入 + 导出 + 编辑」同栏设计已删除——Android 应用
// 控制归左侧设备区域，Installed/Editable 双层语义不复存在。
import { computed, reactive, ref, watch } from 'vue'
import { api as defaultApi } from '../api'
import { sha256Hex } from '../workspace/plugin-center/registry-client'
import {
  packageStore, loadPackages, selectPackage, refreshPackages, currentPackageId,
} from '../package-store'

// 配置包 manifest 的默认版本，与服务端产品版本独立。
const PACKAGE_INITIAL_VERSION = '1.0.0'
const PACKAGE_EMPTY_VERSION = '0.1.0'

/** Package id 输入规整：小写域 [a-z0-9._-]，与服务端 validate_scope_id 对齐。 */
export function normalizePackageId(value) {
  return String(value || '').trim().toLowerCase()
}

export function isValidPackageId(value) {
  const id = normalizePackageId(value)
  return /^[a-z0-9][a-z0-9._-]*$/.test(id) && !/^[.]+$/.test(id)
}

/**
 * 依赖注入：toast / refreshAll()（包切换或
 * 导入后重拉资源面板）/ download(blob, filename) / api。均可替换以便测试。
 * currentApp 提供当前设备的 Android 应用包名与显示名；loadCurrentApps 用于在
 * 显示名尚未读取时补读应用列表。
 */
export function usePackageContext({
  api = defaultApi,
  toast,
  feedback,
  refreshAll,
  beforePackageChange,
  download,
  currentApp,
  currentTargetId,
  loadCurrentApps,
} = {}) {
  const beginReport = operationReporter(feedback, '', toast)
  const busy = ref(false)

  const currentId = currentPackageId
  const packages = computed(() => packageStore.packages)
  const pkgOptions = computed(() => packages.value.map(p => p.id))

  function optionLabel(id) {
    const p = packages.value.find(x => x.id === id)
    return p?.name && p.name !== p.id ? `${p.name} · ${p.id}` : p?.id || id
  }

  let changingPackage = false
  async function onPackageChange(e) {
    const next = e?.target?.value || null
    if (e?.target) e.target.value = currentId.value || ''
    if (changingPackage) return
    changingPackage = true
    try {
      if (await beforePackageChange?.(next) === false) return
    } finally { changingPackage = false }
    feedback?.setCore(null)
    selectPackage(next)
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
    summary: { id: '', name: '', version: '', author: '', androidTargets: [], webUrlPrefixes: [], plugins: [] },
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
      webUrlPrefixes: summary?.targets?.web?.url_prefixes || [],
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
    const toast = beginReport()
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
    const toast = beginReport()
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

  // ---------- 导出：先准备归档并展示实际清单，确认后下载同一份文件 ----------
  const exportModal = reactive({
    open: false, loading: false, ready: false, submitting: false, error: '',
    packageId: '', filename: '', files: [], groups: [], entries: [],
    totalBytes: 0, archiveBytes: 0, includeMedia: false,
  })
  let exportGeneration = 0
  let preparedExport = null

  async function refreshExport() {
    if (!exportModal.open || exportModal.submitting) return
    const generation = ++exportGeneration
    const id = exportModal.packageId
    const includeMedia = exportModal.includeMedia
    preparedExport = null
    exportModal.loading = true
    exportModal.ready = false
    exportModal.error = ''
    try {
      const archive = await api.exportPackageArchive(id, { includeMedia })
      const preview = await inspectPackageExport(archive.blob)
      if (generation !== exportGeneration || !exportModal.open) return
      preparedExport = { ...archive, includeMedia }
      Object.assign(exportModal, preview, { filename: archive.filename || `${id}.gamerpkg`, ready: true })
    } catch (e) {
      if (generation === exportGeneration) exportModal.error = e?.message || '准备导出失败，请重试'
    } finally {
      if (generation === exportGeneration) exportModal.loading = false
    }
  }

  async function exportPackage() {
    const id = currentId.value
    if (!id || busy.value || exportModal.open) return
    Object.assign(exportModal, {
      open: true, packageId: id, filename: '', includeMedia: false,
      files: [], groups: [], entries: [], totalBytes: 0, archiveBytes: 0,
    })
    await refreshExport()
  }

  function closeExport() {
    if (exportModal.submitting) return
    ++exportGeneration
    preparedExport = null
    Object.assign(exportModal, { open: false, loading: false, ready: false, error: '', files: [], groups: [], entries: [] })
  }

  async function confirmExport() {
    if (!exportModal.open || !exportModal.ready || exportModal.loading || exportModal.submitting
      || !preparedExport || preparedExport.includeMedia !== exportModal.includeMedia) return
    const toast = beginReport()
    exportModal.submitting = true
    exportModal.error = ''
    try {
      const { blob, sha256 } = preparedExport
      await saveBlob(blob, exportModal.filename)
      toast(`已导出 ${exportModal.filename}（${sha256 ? `SHA-256 ${sha256.slice(0, 12)}…` : '下载已开始'}）`, 'success')
      exportModal.submitting = false
      closeExport()
    } catch (e) {
      exportModal.error = e?.message || '下载失败，请重试'
    } finally {
      exportModal.submitting = false
    }
  }

  // ---------- 新建 / 复制（同一表单弹窗） ----------
  const formModal = reactive({
    open: false, mode: 'create', submitting: false, error: '',
    form: { id: '', name: '', version: PACKAGE_INITIAL_VERSION, androidPackagesText: '', webUrlPrefixesText: '' },
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
    // 由用户明确选择适用目标，不默认扩大范围。
    formModal.form = { id: '', name: '', version: PACKAGE_INITIAL_VERSION, androidPackagesText: '', webUrlPrefixesText: '' }
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
      version: src?.version || PACKAGE_INITIAL_VERSION,
      androidPackagesText: (src?.targets?.android?.packages || []).join(', '),
      webUrlPrefixesText: (src?.targets?.web?.url_prefixes || []).join('\n'),
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
    const toast = beginReport()
    if (formModal.submitting) return
    const id = normalizePackageId(formModal.form.id)
    const version = formModal.form.version.trim() || PACKAGE_EMPTY_VERSION
    const name = formModal.form.name.trim()
    const androidPackages = parseAndroidPackages(formModal.form.androidPackagesText)
    if (!isValidPackageId(id)) {
      formModal.error = '配置 ID 非法：小写字母/数字开头，仅含小写字母、数字、点、下划线、连字符'
      return
    }
    if (formModal.mode === 'create' && !androidPackages.length && !parseWebPrefixes(formModal.form.webUrlPrefixesText).length) {
      formModal.error = '请至少填写一个适用目标'
      return
    }
    formModal.submitting = true
    formModal.error = ''
    try {
      if (formModal.mode === 'create') {
        await api.createPackage({
          id, version,
          ...(name ? { name } : {}),
          targets: { android: { packages: androidPackages }, web: { url_prefixes: parseWebPrefixes(formModal.form.webUrlPrefixesText) } },
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
    const toast = beginReport()
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
    form: { name: '', version: '', author: '', androidPackagesText: '', webUrlPrefixesText: '', plugins: [] },
    compat: { input: '', kind: 'android', checking: false, error: '', result: null },
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
          web: { url_prefixes: pkg.targets?.web?.url_prefixes || [] },
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
    ++compatGeneration
    detailModal.compat = { input: '', kind: 'android', checking: false, error: '', result: null }
    detailModal.open = true
    await reloadDetail()
  }

  function closeDetail() {
    ++compatGeneration
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
      webUrlPrefixesText: (p.targets?.web?.url_prefixes || []).join('\n'),
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
    const toast = beginReport()
    if (detailModal.saving || !detailModal.detail?.package) return
    const body = {
      targets: { android: { packages: parseAndroidPackages(detailModal.form.androidPackagesText) }, web: { url_prefixes: parseWebPrefixes(detailModal.form.webUrlPrefixesText) } },
      plugins: buildPluginsPayload(),
    }
    if (!body.targets.android.packages.length && !body.targets.web.url_prefixes.length) {
      detailModal.editError = '请至少填写一个适用目标'
      return
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
      detailModal.compat.result = null
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
  let compatGeneration = 0
  watch(() => unwrap(currentTargetId), () => {
    ++compatGeneration
    Object.assign(detailModal.compat, { result: null, error: '', checking: false })
  }, { flush: 'sync' })
  watch(() => [detailModal.compat.input, detailModal.compat.kind], () => {
    ++compatGeneration
    Object.assign(detailModal.compat, { result: null, error: '', checking: false })
  }, { flush: 'sync' })
  async function checkCompatibility(androidPackage, useCurrent = false) {
    const input = String(androidPackage ?? (detailModal.compat.input || '')).trim()
    const id = unwrap(currentTargetId)
    if ((!useCurrent && !input) || detailModal.compat.checking) return
    if (useCurrent && !id) { detailModal.compat.error = '请先选择运行目标'; return }
    detailModal.compat.input = input
    const generation = ++compatGeneration
    detailModal.compat.checking = true
    detailModal.compat.error = ''
    detailModal.compat.result = null
    try {
      const query = useCurrent ? { target_id: id } : detailModal.compat.kind === 'web' ? { url: input } : input
      const result = await api.packageCompatibility(detailModal.packageId, query)
      if (generation === compatGeneration) detailModal.compat.result = result
    } catch (e) {
      if (generation === compatGeneration) detailModal.compat.error = e?.message || '兼容性检查失败'
    } finally {
      if (generation === compatGeneration) detailModal.compat.checking = false
    }
  }

  function parseWebPrefixes(text) {
    return [...new Set(String(text || '').split(/\r?\n/).map(x => x.trim()).filter(Boolean))]
  }
  const addingTarget = ref(false)
  async function addCurrentTarget(form, errorOwner) {
    if (addingTarget.value) return
    const id = unwrap(currentTargetId)
    if (!id) { errorOwner[errorOwner === detailModal ? 'editError' : 'error'] = '请先选择运行目标'; return }
    addingTarget.value = true
    try {
      const identity = await api.targetIdentity(id)
      if (id !== unwrap(currentTargetId)) throw new Error('当前目标已变化，请重新添加')
      if (!identity.value) throw new Error(identity.note || '无法读取当前目标')
      if (identity.kind === 'web') {
        const url = new URL(identity.value)
        if (!['http:', 'https:'].includes(url.protocol)) throw new Error('当前页面不是 HTTP(S) 网页')
        // 添加的是站点和路径规则，明确在提示中告知不包含参数。
        const prefix = url.origin + url.pathname
        form.webUrlPrefixesText = [...new Set([...parseWebPrefixes(form.webUrlPrefixesText), prefix])].join('\n')
        toast?.(`已添加网页前缀（不含查询参数与片段）。${identity.note}`, 'success')
      } else {
        form.androidPackagesText = [...new Set([...parseAndroidPackages(form.androidPackagesText), identity.value])].join(', ')
      }
      errorOwner[errorOwner === detailModal ? 'editError' : 'error'] = ''
    } catch (e) {
      errorOwner[errorOwner === detailModal ? 'editError' : 'error'] = e?.message || '读取目标失败'
    } finally { addingTarget.value = false }
  }

  return {
    busy, packages, pkgOptions, currentId, optionLabel, onPackageChange,
    loadPackages, refreshPackages,
    pickImportFile, onImportPicked, importPackage,
    exportPackage, exportModal, refreshExport, confirmExport, closeExport,
    formModal, openCreate, openDuplicate, fillCurrentApp, submitForm, closeForm,
    overwriteModal, confirmOverwrite, closeOverwrite,
    deleteModal, openDelete, confirmDelete, closeDelete,
    detailModal, openDetail, closeDetail, reloadDetail,
    startEdit, cancelEdit, saveEdit, retryEdit, addPluginDep, removePluginDep,
    checkCompatibility, resolveOverwriteMissingRequired,
    addingTarget,
    addCurrentFormTarget: () => addCurrentTarget(formModal.form, formModal),
    addCurrentEditTarget: () => addCurrentTarget(detailModal.form, detailModal),
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
