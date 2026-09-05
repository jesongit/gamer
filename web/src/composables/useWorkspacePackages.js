// 游戏包（App Package）导入/导出/编辑入口的界面逻辑，收敛在同一个 composable：
// WorkspaceContextBar 只透传 context（按钮 + 隐藏文件输入 + 三个弹窗），Console.vue
// 注入 toast / activePkg / 刷新回调后装配。导入/编辑替换的是当前分区（activePkg）的
// 本地编辑现场（data/<pkg>/{scripts,functions,templates,keymaps,presets,resources}），
// 成功后必须全量刷新（脚本/模板/函数库/按键映射），否则界面仍显示旧资源。
import { reactive, ref } from 'vue'
import { api as defaultApi } from '../api'
import { sha256Hex } from '../workspace/plugin-center/registry-client'

/** 导出确认弹窗展示的资源统计行（key 与服务端 WorkspaceStats JSON 对齐）。 */
export const PACKAGE_STAT_ROWS = [
  ['scripts', '脚本'],
  ['functions', '函数库'],
  ['templates', '模板'],
  ['keymaps', '按键映射'],
  ['presets', '任务预设'],
  ['resources', '资源'],
]

/** "a, b\nc" → 去重后的包名数组（逗号/中文逗号/换行均可作分隔）。 */
export function parseAndroidPackages(text) {
  const seen = new Set()
  for (const part of String(text || '').split(/[\n,，]/)) {
    const pkg = part.trim()
    if (pkg) seen.add(pkg)
  }
  return [...seen]
}

/**
 * 依赖注入：toast(msg, type) / activePkg(ref) / loadData()（设备）/ 
 * refreshScripts()（脚本列表）/ refreshTemplates()（模板列表）/ refreshFnLib(pkg)（函数库）
 * / refreshKeymaps(pkg)（按键映射）/ download(blob, filename) / api。
 * 全部可替换以便测试；不传时用真实实现。
 */
export function useWorkspacePackages({
  api = defaultApi,
  toast,
  activePkg,
  loadData,
  refreshScripts,
  refreshTemplates,
  refreshFnLib,
  refreshKeymaps,
  download,
} = {}) {
  const busy = ref(false)

  /** 资源替换后的统一全量刷新（导入/编辑共用）；单项失败不阻塞其余刷新。 */
  async function refreshAll() {
    const pkg = activePkg.value
    await Promise.all([
      Promise.resolve(loadData?.()).catch(() => {}),
      Promise.resolve(refreshScripts?.()).catch(() => {}),
      Promise.resolve(refreshTemplates?.()).catch(() => {}),
      Promise.resolve(refreshFnLib?.(pkg)).catch(() => {}),
      Promise.resolve(refreshKeymaps?.(pkg)).catch(() => {}),
    ])
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

  // ---------- 导入 ----------
  function pickImportFile(inputEl) {
    if (busy.value) return
    inputEl?.click()
  }

  /** 409 无机器码（错误体只有 {error} 文本），按服务端 Display 文案区分两种冲突。 */
  function importErrorMessage(e) {
    const msg = e?.message || '请重试'
    if (e?.status === 409) {
      const hint = /已由其他.*激活/.test(msg)
        ? '同一安卓应用已有激活的游戏包，请先卸载或切换'
        : '该版本已存在，请修改版本号后重新导出'
      return `导入失败：${msg}（${hint}）`
    }
    return `导入失败：${msg}`
  }

  async function importPackage(file) {
    if (!file || busy.value) return
    busy.value = true
    try {
      const bytes = await file.arrayBuffer()
      const sha256 = await sha256Hex(bytes)
      const pkg = await api.installAppPackage(bytes, sha256)
      toast(`游戏包已导入：${pkg?.id || '?'}@${pkg?.active_version || '?'}`, 'success')
      await refreshAll()
    } catch (e) {
      toast(importErrorMessage(e), 'error')
    } finally {
      busy.value = false
    }
  }

  /** 隐藏 input change：读文件走导入，完毕清空 value 以便同文件可重复选择。 */
  async function onImportPicked(e) {
    const input = e?.target
    try {
      await importPackage(input?.files?.[0])
    } finally {
      if (input) input.value = ''
    }
  }

  // ---------- 导出（编辑区 → .gamerpkg）----------
  const metaModal = reactive({
    open: false, saving: false, error: '',
    form: { id: '', name: '', version: '1.0.0', androidPackagesText: '' },
  })
  const exportModal = reactive({
    open: false, exporting: false, errorLines: [],
    info: { id: '', name: '', version: '', androidPackages: [], stats: {} },
  })
  // 初始化元数据期间暂存 getWorkspace 的资源统计，PUT 成功后原样带进导出确认弹窗
  let pendingStats = {}

  function openExportConfirm(metadata, stats) {
    exportModal.info = {
      id: metadata?.id || '',
      name: metadata?.name || '',
      version: metadata?.version || '',
      androidPackages: Array.isArray(metadata?.android_packages) ? metadata.android_packages : [],
      stats: stats || {},
    }
    exportModal.errorLines = []
    exportModal.open = true
  }

  async function openExport() {
    if (!activePkg.value || busy.value) return
    busy.value = true
    try {
      const ws = await api.getWorkspace(activePkg.value)
      pendingStats = ws?.stats || {}
      if (!ws?.metadata) {
        // 未初始化：先填包信息（id 与 Android Packages 默认当前分区，版本 1.0.0）
        metaModal.form = {
          id: activePkg.value,
          name: '',
          version: '1.0.0',
          androidPackagesText: activePkg.value,
        }
        metaModal.error = ''
        metaModal.open = true
      } else {
        openExportConfirm(ws.metadata, pendingStats)
      }
    } catch (e) {
      toast(`读取编辑区信息失败：${e?.message || e}`, 'error')
    } finally {
      busy.value = false
    }
  }

  function closeMeta() {
    metaModal.open = false
    metaModal.error = ''
  }

  /** 初始化弹窗提交：客户端兜底校验 → PUT workspace → 直接进入导出确认弹窗。 */
  async function submitMeta() {
    if (metaModal.saving) return
    const id = metaModal.form.id.trim()
    const version = metaModal.form.version.trim()
    const androidPackages = parseAndroidPackages(metaModal.form.androidPackagesText)
    if (!id || !version || !androidPackages.length) {
      metaModal.error = '游戏包 ID、版本、Android Packages 均不能为空'
      return
    }
    metaModal.saving = true
    metaModal.error = ''
    try {
      const rep = await api.saveWorkspace(activePkg.value, {
        id,
        version,
        android_packages: androidPackages,
        ...(metaModal.form.name.trim() ? { name: metaModal.form.name.trim() } : {}),
      })
      metaModal.open = false
      openExportConfirm(rep?.metadata || { id, version, android_packages: androidPackages }, pendingStats)
    } catch (e) {
      metaModal.error = e?.message || '保存失败'
    } finally {
      metaModal.saving = false
    }
  }

  /** preflight 400 的 error 为逐行问题列表；去掉首行「导出 preflight 失败:」标题。 */
  function preflightErrorLines(e) {
    const text = String(e?.data?.error ?? e?.message ?? '')
    return text.split(/\r?\n/).map(s => s.trimEnd()).filter(Boolean)
      .filter(line => !/^导出 preflight 失败[:：]?\s*$/.test(line))
  }

  async function confirmExport() {
    if (exportModal.exporting) return
    exportModal.exporting = true
    exportModal.errorLines = []
    try {
      const { blob, filename, sha256 } = await api.exportAppPackage(activePkg.value)
      const name = filename
        || `${exportModal.info.id || activePkg.value}-${exportModal.info.version || '1.0.0'}.gamerpkg`
      saveBlob(blob, name)
      exportModal.open = false
      toast(`已导出 ${name}${sha256 ? `（SHA-256 ${sha256.slice(0, 12)}…）` : ''}`, 'success')
    } catch (e) {
      exportModal.errorLines = e?.code === 'preflight_failed'
        ? preflightErrorLines(e)
        : [e?.message ? `导出失败：${e.message}` : '导出失败：请重试']
    } finally {
      exportModal.exporting = false
    }
  }

  function closeExport() {
    exportModal.open = false
    exportModal.errorLines = []
  }

  // ---------- 编辑（已激活游戏包 → 当前编辑区）----------
  const editModal = reactive({
    open: false, starting: false,
    id: '', version: '', targets: [], target: '',
    /** 当前分区不在该包 android_packages（理论少见）→ 弹窗内展示 target 单选 */
    showTargetPicker: false,
  })

  async function openEdit() {
    if (!activePkg.value || busy.value) return
    busy.value = true
    try {
      const rep = await api.listAppPackages()
      const packages = Array.isArray(rep?.packages) ? rep.packages : []
      const activeOf = p => p && p.active_version && Array.isArray(p.android_packages) && p.android_packages.length
      // 常规：android_packages 包含当前分区且 active_version 非空
      let found = packages.find(p => activeOf(p) && p.android_packages.includes(activePkg.value))
      if (!found) {
        // 理论少见：存在激活包但都不含当前分区（列表陈旧/跨分区误配）→ 仍进弹窗，
        // 由弹窗内 target 单选指定真正要写入的编辑区
        found = packages.find(activeOf) || null
      }
      if (!found) {
        toast('当前应用没有已激活的 Gamer 游戏包，请先导入', 'info')
        return
      }
      editModal.id = found.id
      editModal.version = found.active_version
      editModal.targets = [...found.android_packages]
      const inTargets = editModal.targets.includes(activePkg.value)
      editModal.showTargetPicker = !inTargets
      editModal.target = inTargets ? activePkg.value : editModal.targets[0]
      editModal.open = true
    } catch (e) {
      toast(`读取游戏包列表失败：${e?.message || e}`, 'error')
    } finally {
      busy.value = false
    }
  }

  function closeEdit() {
    editModal.open = false
  }

  async function confirmEdit() {
    if (editModal.starting) return
    editModal.starting = true
    try {
      const rep = await api.editAppPackage(editModal.id, editModal.version, editModal.target)
      const replaced = rep?.replaced || {}
      const total = Object.values(replaced).reduce((sum, n) => sum + (Number(n) || 0), 0)
      editModal.open = false
      toast(`已将 ${editModal.id}@${editModal.version} 导入 ${editModal.target} 编辑区，替换 ${total} 项资源`, 'success')
      await refreshAll()
    } catch (e) {
      toast(`编辑失败：${e?.message || e}`, 'error')
    } finally {
      editModal.starting = false
    }
  }

  const context = {
    busy,
    // 导入（隐藏 input 由组件持有，DOM 交互留在组件、逻辑在此）
    pickImportFile, onImportPicked,
    // 导出
    openExport, submitMeta, closeMeta, confirmExport, closeExport,
    metaModal, exportModal,
    // 编辑
    openEdit, confirmEdit, closeEdit, editModal,
  }

  return {
    context,
    busy, refreshAll,
    importPackage, pickImportFile, onImportPicked,
    openExport, submitMeta, closeMeta, confirmExport, closeExport, metaModal, exportModal,
    openEdit, confirmEdit, closeEdit, editModal,
  }
}
