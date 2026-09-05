import { computed, nextTick, onUnmounted, provide, reactive, ref, watch } from 'vue'
import { api } from '../../api'
import { GAMER_YAML_RUNNER_ID, runYamlFunction, runYamlScript } from '../../gamer-yaml-runner'
import {
  applyRunRecord, beginCancel, findRun, pushRunConflict, resetStoreRunState,
  scriptsData, store, templatesData,
} from '../../store'
import { isDeviceBusyConflict, isTerminalRunState, sourceLabel, terminalLabel } from '../../runs'
import { useScriptEditorShell } from '../../composables/useScriptEditorShell'
import { useRawYamlEditor } from '../../composables/useRawYamlEditor'
import { useFunctionLibrary } from '../../composables/useFunctionLibrary'
import { useRunArgsFlow } from '../../composables/useRunArgsFlow'
import { createEditorShellApi } from './current-api-adapters'
import { parseScript, parseFunctionLibrary, serialize } from '../../script-editor/codec'
import { SE_TARGET_OPTIONS } from '../../script-editor/targets'
import { startIndexOf } from '../../script-editor/selection'

/**
 * gamer.yaml 面板运行器（console.scripts / console.functions 两个扩展面板的
 * 共享实现）：运行区（目标选择、只读摘要、从此运行）、编辑外壳
 * （useScriptEditorShell/rawEditor/fnLib）、call/func 目标参数解析、
 * 运行参数流程、运行日志与运行状态轮询。
 *
 * 面板上下文按资源类型拆分为两份独立作用域（scriptPanel / functionsPanel）：
 * 编辑模式、目标选择、删除确认等互不串台；编辑器外壳/函数库快照/日志与
 * 运行轮询是同一设备的同一份机制，保持单例共享。
 */

// 脚本列表加载（gamer.yaml 面板实现自持，Console 壳不再预拉业务资源）：
// inflight 去重 + 共享 store（与任务表单贡献的懒加载互通——store 非空即跳过）。
let scriptsInflight = null

export function useConsoleScriptRunner({
  toast,
  activePkg,
  consoleRuntime,
  templateNames,
  tplShortName,
  loadData,
}) {
  // 面板作用域：每个面板锁定自己的资源类型与编辑模式
  function createPanelScope(kind) {
    return {
      kind,
      runKind: ref(kind),        // 锁定（面板类型即资源类型；模板分支沿用）
      scriptMode: ref('run'),    // run | edit | raw（面板独立）
    }
  }
  const scriptScope = createPanelScope('script')
  const funcScope = createPanelScope('func')

  async function refreshScripts() {
    if (!scriptsInflight) {
      scriptsInflight = api.listScripts()
        .then(list => { scriptsData.value = Array.isArray(list) ? list : [] })
        .finally(() => { scriptsInflight = null })
    }
    return scriptsInflight
  }
  refreshScripts().catch(() => { /* 拉取失败：面板内提示「（无脚本）」等空态 */ })

  // ---------- 共享脚本编辑器外壳（阶段 4） ----------
  // 模型/命令栈/dirty/保存/409 冲突/校验/跳转全部收敛在 useScriptEditorShell，
  // 两个面板的编辑态共用同一外壳（任一时刻只有一个面板可见）。
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
  const rawEditor = useRawYamlEditor({ api })
  // 函数库列表与 func 目标解析（func 步骤「打开函数定义」跳转用）
  const fnLib = useFunctionLibrary({ api })
  // 各面板目标选择（面板独立）
  const selScript = ref('')
  const selFnFile = ref('')
  const scriptDeleteConfirmId = ref('')
  /** 运行按钮可用性：脚本面板看脚本选择，函数面板看函数库文件选择 */
  const canRunTargetScript = computed(() => !!selScript.value)
  const canRunTargetFunc = computed(() => !!selFnFile.value)
  // 函数文件默认选中第一个：切分区/列表刷新后当前选择失效时回退第一个（与脚本下拉同形）
  watch(() => fnLib.list, () => {
    if (!fnLib.list.some(f => f.id === selFnFile.value)) selFnFile.value = fnLib.list[0]?.id || ''
  }, { immediate: true })
  /** 运行区当前选择 id（脚本 id / 函数库文件 id）：编辑、删除按钮与摘要区共用 */
  const selTargetIdScript = computed(() => selScript.value)
  const selTargetIdFunc = computed(() => selFnFile.value)
  watch([selScript, activePkg], () => { scriptDeleteConfirmId.value = '' })
  /**
   * 函数面板：整个函数库文件的解析模型（全部函数）。摘要区逐函数分组渲染
   * （每组一个 ScriptSummary，steps 带稳定 uuid 供运行起点定位）。
   * parseFunctionFile 返回 {model, diagnostics} 包装。
   */
  const funcParsed = computed(() => {
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
    const f = fnLib.list.find(x => x.id === selFnFile.value)
    if (!f) return '请选择函数库文件'
    return funcParsed.value ? '' : '函数库解析失败（可能含旧语法），请进编辑态查看诊断'
  })
  // 编辑态辅助 UI 开关（编辑视图共享外壳，开关随外壳共享）
  const showYaml = ref(false)
  /** 进入函数库编辑态时聚焦的函数名（摘要区逐函数「编辑」直达；空 = 默认第一个） */
  const editFocusFn = ref('')
  // 子脚本/函数跳转只打开只读预览，不切换当前运行/编辑资源。
  const resourcePreview = reactive({
    open: false,
    kind: 'script',
    title: '',
    resource: '',
    model: null,
    error: '',
  })
  const scripts = computed(() => scriptsData.value)

  // ---------- call/func 目标候选与参数解析（编辑态画布下拉 + args 自动生成，同独立脚本页） ----------
  // func 参数直接从 fnLib.list 的文件内容解析（按内容版本 memo）；call 拉脚本内容按 id 缓存。

  // v3 call 目标 = 命名空间串（script:<资源id> / function:<文件短路径>/<函数名>），
  // 候选与参数解析按 target 串寻址；正在编辑的脚本自身不进候选（自引用排除）。
  const callTargets = computed(() => {
    if (!activePkg.value) return []
    const scriptOpts = scriptsData.value
      .filter(s => s.package === activePkg.value && !(s.id === scriptShell.resourceId && scriptShell.kind === 'script'))
      .map(s => {
        const path = String(s.name || '').replace(/\.(ya?ml)$/i, '')
        return { target: `script:${path}`, label: path }
      })
    const live = scriptShell.kind === 'function_library' && scriptShell.hasModel && Array.isArray(scriptShell.model.functions)
      ? scriptShell.model.functions.map(f => f.name)
      : null
    const fnOpts = fnLib.list.flatMap(f => {
      const names = live && f.id === scriptShell.resourceId ? live : (Array.isArray(f.functions) ? f.functions : [])
      return names.map(n => ({ target: `function:${f.file}/${n}`, label: `${f.file}/${n}` }))
    })
    return [...scriptOpts, ...fnOpts]
  })

  const callParamsCache = new Map() // call 目标（命名空间串/脚本 id）→ ParamDecl[] | null
  const fnParamsMemo = new Map() // `<file>@<内容版本>` → Map(函数名 → ParamDecl[])

  /** target = 'function:<文件短路径>/<函数名>'（文件短路径可含目录，按最后一个 / 分割）。 */
  function funcParamsFor(target) {
    const s = String(target || '')
    const prefix = 'function:'
    if (!s.startsWith(prefix)) return null
    const rest = s.slice(prefix.length)
    const i = rest.lastIndexOf('/')
    if (i <= 0 || i === rest.length - 1) return null
    const file = rest.slice(0, i)
    const fn = rest.slice(i + 1)
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

  /** target = 'script:<资源id>'（分区相对路径去扩展名）。 */
  function scriptParamsFor(target) {
    if (!target.startsWith('script:')) return null
    const path = target.slice('script:'.length)
    if (callParamsCache.has(target)) return callParamsCache.get(target)
    // 脚本列表已带 content；优先同步解析，保证已有 call 步骤首次渲染时
    // 就能按目标声明选择正确的 CellEditor 类型，不会先退化成 text。
    const script = scriptsData.value.find(x => x.package === activePkg.value
      && String(x.name || '').replace(/\.(ya?ml)$/i, '') === path)
    if (!script?.content) return null
    try {
      const params = parseScript(script.content).model?.params || []
      callParamsCache.set(target, params)
      return params
    } catch {
      return null
    }
  }

  function resolveTargetParamsSync(target) {
    if (!target) return null
    if (target.startsWith('function:')) return funcParamsFor(target)
    return scriptParamsFor(target)
  }

  async function resolveTargetParams(target) {
    if (!target || target.startsWith('function:')) return resolveTargetParamsSync(target)
    if (callParamsCache.has(target)) return callParamsCache.get(target)
    const path = target.slice('script:'.length)
    const script = scriptsData.value.find(x => x.package === activePkg.value
      && String(x.name || '').replace(/\.(ya?ml)$/i, '') === path)
    if (!script) return null
    try {
      const full = await api.getScript(script.id)
      const parsed = parseScript(full.content ?? '')
      const params = parsed.model?.params || []
      callParamsCache.set(target, params)
      callParamsCache.set(script.id, params)
      return params
    } catch {
      return null
    }
  }

  function clearCallParamsCache() {
    callParamsCache.clear()
  }
  function resolveTargetSync(target) {
    const params = resolveTargetParamsSync(target)
    return params ? { params } : null
  }

  provide(SE_TARGET_OPTIONS, reactive({
    targets: callTargets,
    resolveParams: resolveTargetParams,
    resolveParamsSync: resolveTargetParamsSync,
  }))

  // 日志原始数据（未过滤），用于按级别切换显示
  let rawLogs = []
  // 本次运行开始时间：清空日志区后只显示本次运行产生的日志
  let runStartTime = 0
  const liveLogs = ref([])
  const logBox = ref(null)

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

  function stopLogPolling() {
    consoleRuntime.stopLogPolling()
  }

  function pushLog(level, msg) {
    const now = new Date()
    const t = now.toTimeString().slice(0, 8) + '.' + String(now.getMilliseconds()).padStart(3, '0')
    liveLogs.value.push({ time: t, level, msg })
    if (liveLogs.value.length > 30) liveLogs.value.shift()
    scrollLogsToBottom()
  }

  /** 退出编辑（脏模型需确认丢弃）；若处于跳转栈中先返回上一资源。
   *  注意 shell 是 reactive 包装：ref/computed 属性访问即解包，不能再取 .value */
  async function cancelEditScript(scope) {
    if (scriptShell.hasModel && scriptShell.dirty && !window.confirm('有未保存修改，确认放弃？')) return
    if (scriptShell.canJumpBack) {
      await jumpBack()
      return
    }
    scriptShell.reset()
    scope.scriptMode.value = 'run'
    showYaml.value = false
  }

  /** 新建脚本：空 ScriptModel（保存时落盘到当前应用分区）——脚本面板专属 */
  function startNewScript() {
    if (!activePkg.value) return toast('请先在右侧选择包名', 'warn')
    scriptScope.scriptMode.value = 'edit'
    showYaml.value = false
    scriptShell.newScript({ name: '新脚本.yml', pkg: activePkg.value })
  }

  /** 编辑当前选择（按面板资源类型分发）：脚本 = 脚本编辑上下文；函数 = 函数库编辑上下文。
   *  fnName 指定进入时聚焦的函数（摘要区逐函数「编辑」按钮直达），编辑态函数名为静态展示 */
  async function editCurrentTarget(scope, fnName = '') {
    if (scope.kind !== 'func') return editCurrentScript()
    const f = fnLib.list.find(x => x.id === selFnFile.value)
    if (!f) return toast('请先选择函数库文件', 'error')
    editFocusFn.value = fnName || ''
    scope.scriptMode.value = 'edit'
    showYaml.value = false
    try {
      await scriptShell.loadFunctionFile(f.id)
    } catch (e) {
      scriptShell.reset()
      scope.scriptMode.value = 'run'
      toast('函数库加载失败：' + e.message, 'error')
    }
  }

  /** 进入原文编辑态：直接读取资源原文，不经过前端 YAML codec，保存仍由服务端校验。 */
  async function editRawCurrentTarget(scope) {
    const id = scope.kind === 'func' ? selFnFile.value : selScript.value
    if (!id) return toast(scope.kind === 'func' ? '请先选择函数库文件' : '请先选择脚本', 'error')
    scope.scriptMode.value = 'raw'
    try {
      await rawEditor.load(scope.kind === 'func' ? 'function' : 'script', id)
    } catch (e) {
      rawEditor.reset()
      scope.scriptMode.value = 'run'
      toast('原文加载失败：' + e.message, 'error')
    }
  }

  /** 原文保存成功后刷新对应资源列表，避免摘要、函数候选和参数缓存继续使用旧内容。 */
  async function saveRawScript(scope) {
    if (rawEditor.loading.value || rawEditor.saving.value) return
    const r = await rawEditor.save()
    if (r.ok) {
      clearCallParamsCache()
      fnParamsMemo.clear()
      if (rawEditor.kind.value === 'function') await fnLib.refresh(activePkg.value)
      else await refreshScripts()
      rawEditor.reset()
      scope.scriptMode.value = 'run'
      toast('原文已保存', 'success')
    } else if (r.reason === 'invalid') {
      toast('校验未通过：' + r.diagnostics.slice(0, 3).map(d => d.message).join('；'), 'error')
    } else if (r.reason === 'conflict') {
      toast('原文保存遇到版本冲突，请重新进入原文编辑后再试', 'warn')
    } else if (r.reason !== 'empty') {
      toast('原文保存失败：' + (r.error?.message || r.error), 'error')
    }
  }

  /** 取消原文编辑：有修改时确认丢弃，回到资源运行视图。 */
  function cancelRawScript(scope) {
    rawEditor.reset()
    scope.scriptMode.value = 'run'
  }

  /** 新建当前面板类型：脚本 = 新建脚本；函数 = 直接进入新函数库文件编辑态，文件名可在编辑器顶部修改 */
  function startNewTarget(scope) {
    if (scope.kind !== 'func') return startNewScript()
    if (!activePkg.value) return toast('请先在右侧选择包名', 'warn')
    editFocusFn.value = ''
    scope.scriptMode.value = 'edit'
    showYaml.value = false
    scriptShell.newFunctionFile({ file: '新函数库', pkg: activePkg.value })
  }

  /** 删除当前选择：脚本面板删脚本 / 函数面板删函数库文件 */
  async function deleteCurrentTarget(scope) {
    if (scope.kind !== 'func') return deleteCurrentScript()
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

  /** 函数列表操作共用：读取当前文件、修改模型并按版本更新，完成后刷新函数库快照。 */
  async function updateCurrentFunctionFile(mutator, successMessage) {
    const f = fnLib.list.find(x => x.id === selFnFile.value)
    if (!f) {
      toast('请先选择函数库文件', 'warn')
      return false
    }
    let parsed
    try {
      parsed = fnLib.parseFunctionFile(f.content ?? '', f.file || '')
    } catch (e) {
      toast('函数库解析失败：' + e.message, 'error')
      return false
    }
    if (!parsed?.model || parsed.diagnostics?.length) {
      toast('函数库当前内容无法修改，请先进入编辑态修复诊断', 'error')
      return false
    }
    const changed = mutator(parsed.model)
    if (!changed) return false
    try {
      await api.updateFunction(f.id, {
        content: serialize(parsed.model),
        expected_version: f.version,
      })
      await fnLib.refresh(activePkg.value)
      fnParamsMemo.clear()
      clearCallParamsCache()
      toast(successMessage, 'success')
      return true
    } catch (e) {
      toast('函数库更新失败：' + e.message, 'error')
      return false
    }
  }

  /** 函数面板顶部「添加函数」：载入当前文件编辑态，在末尾插入空函数并选中它。 */
  async function addFunctionToCurrentFile() {
    const f = fnLib.list.find(x => x.id === selFnFile.value)
    if (!f) return toast('请先选择函数库文件', 'warn')
    const names = Array.isArray(f.functions) ? f.functions : []
    let i = 1
    while (names.includes(`func${i}`)) i++
    const name = `func${i}`

    editFocusFn.value = ''
    funcScope.scriptMode.value = 'edit'
    showYaml.value = false
    try {
      await scriptShell.loadFunctionFile(f.id)
      const added = scriptShell.stack?.apply({ type: 'insert_function', name }, `新增函数 ${name}`)
      if (!added) throw new Error(`函数 ${name} 已存在或当前内容无法修改`)
      editFocusFn.value = name
    } catch (e) {
      scriptShell.reset()
      funcScope.scriptMode.value = 'run'
      toast('添加函数失败：' + (e.message || e), 'error')
    }
  }

  /** 函数编辑态名称输入框的唯一改名入口：写入命令栈，失焦后由编辑外壳自动保存。 */
  function renameEditingFunction(fromName, toName) {
    const current = String(fromName || '').trim()
    const next = String(toName || '').trim()
    const functions = scriptShell.model?.functions
    if (!current || !next || next === current || !Array.isArray(functions)) return false
    if (functions.some(fn => fn.name === next)) {
      toast(`已存在同名函数：${next}`, 'warn')
      return false
    }
    const changed = scriptShell.stack?.apply(
      { type: 'rename_function', from: current, to: next },
      `重命名函数 ${current} → ${next}`,
    )
    if (changed) editFocusFn.value = next
    return !!changed
  }

  /** 函数摘要「删除」：删除当前文件中的一个函数，至少保留一个。 */
  async function deleteFunction(fnName) {
    const f = fnLib.list.find(x => x.id === selFnFile.value)
    if (!f) return toast('请先选择函数库文件', 'warn')
    await updateCurrentFunctionFile(model => {
      if (!Array.isArray(model.functions) || model.functions.length <= 1) {
        toast('函数库至少保留一个函数', 'warn')
        return false
      }
      const i = model.functions.findIndex(fn => fn.name === fnName)
      if (i < 0) {
        toast(`函数不存在：${fnName}`, 'warn')
        return false
      }
      model.functions.splice(i, 1)
      return true
    }, `函数 ${fnName} 已删除`)
  }

  /** 运行模式：编辑当前选中的脚本（getScript 读取最新内容与版本短码）——脚本面板专属 */
  async function editCurrentScript() {
    const s = scripts.value.find(x => x.id === selScript.value)
    if (!s) return toast('请先选择脚本', 'error')
    scriptScope.scriptMode.value = 'edit'
    showYaml.value = false
    try {
      await scriptShell.loadScript(s.id)
    } catch (e) {
      scriptShell.reset()
      scriptScope.scriptMode.value = 'run'
      toast('脚本加载失败：' + e.message, 'error')
    }
  }

  /** 运行模式：删除当前选中的脚本——脚本面板专属 */
  async function deleteCurrentScript() {
    const s = scripts.value.find(x => x.id === selScript.value)
    if (!s) return toast('请先选择脚本', 'error')
    if (scriptDeleteConfirmId.value !== s.id) {
      scriptDeleteConfirmId.value = s.id
      return
    }
    try {
      await api.deleteScript(s.id)
      await refreshScripts()
      clearCallParamsCache()
      if (selScript.value === s.id) selScript.value = ''
      scriptDeleteConfirmId.value = ''
      toast('脚本已删除', 'success')
    } catch (e) {
      scriptDeleteConfirmId.value = ''
      toast('删除失败：' + e.message, 'error')
    }
  }

  /** 脚本校验（结构化字段级）由 useScriptEditorShell.diagnostics 提供（validateScript + 解析期诊断） */

  /** 保存编辑中的脚本：shell.save() 序列化模型并携带 expected_version；
   *  校验失败 → 提示前 3 条诊断；409 version_conflict → shell.conflict 置位，SaveConflictModal 弹出。 */
  async function saveEditScript(scope) {
    if (!scriptShell.hasModel) return
    if (!String(scriptShell.name || '').trim()) return toast('请填写脚本名称', 'error')
    if (!scriptShell.pkg && !activePkg.value) return toast('请先选择应用分区', 'warn')
    const r = await scriptShell.save()
    if (r.ok) {
      clearCallParamsCache()
      await afterScriptSaved(scope, r.result)
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
  function autoSaveDebounced(scope) {
    if (scope.scriptMode.value !== 'edit' || !scriptShell.hasModel) return
    if (autoSaveTimer) clearTimeout(autoSaveTimer)
    autoSaveTimer = setTimeout(() => autoSave(scope), 600)
  }
  async function autoSave(scope) {
    autoSaveTimer = null
    if (scope.scriptMode.value !== 'edit' || !scriptShell.hasModel || !scriptShell.dirty || scriptShell.saving) return
    const wasNew = !scriptShell.resourceId
    const r = await scriptShell.save({ suppressConflict: true })
    if (r.ok) {
      clearCallParamsCache()
      // 函数库落盘后刷新文件清单（func 下拉与运行区函数下拉共用）；
      // 新建脚本落盘后刷新脚本列表（call 目标下拉候选）
      if (scriptShell.kind === 'function_library') await fnLib.refresh(activePkg.value)
      else if (wasNew) await refreshScripts()
      if (wasNew) selScript.value = scriptShell.resourceId // 首次落盘：运行区选择跟随
    } else if (r.reason === 'invalid') {
      toast('自动保存未通过：' + (r.diagnostics?.[0]?.message || '存在校验问题'), 'warn')
    } else if (r.reason === 'conflict') {
      toast('自动保存遇到版本冲突，请点「💾 保存」手动处理', 'warn')
    } else if (r.reason !== 'empty') {
      toast('自动保存失败：' + (r.error?.message || '未知错误'), 'warn')
    }
  }

  /** 保存成功后置：刷新列表、选中保存后的资源（按外壳实际类型归位到对应面板的选择）、退出编辑回到运行视图 */
  async function afterScriptSaved(scope, rep) {
    await refreshScripts()
    if (rep?.id) {
      if (scriptShell.kind === 'function_library') {
        selFnFile.value = rep.id
        await fnLib.refresh(activePkg.value)
      } else {
        selScript.value = rep.id
      }
    }
    scriptShell.reset()
    scope.scriptMode.value = 'run'
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
    if (r.ok) {
      clearCallParamsCache()
      await afterScriptSaved(scriptShell.kind === 'function_library' ? funcScope : scriptScope, r.result)
    }
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
      // v2 脚本（缺 version: 3）带版本诊断 → 不给摘要（编辑器只读写 v3，提示升级）
      const parsed = parseScript(s.content ?? '')
      return parsed.diagnostics.length === 0 ? parsed.model : null
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
  function runFromStep(scope, uuid) {
    return runScript(scope, { fromUuid: uuid })
  }

  // ---------- 结构化跳转（plan §10「调用文本链接预览」行：正则扫描源码 → 结构化引用） ----------

  /** call 步骤目标（v3 命名空间串）→ 同分区资源 id。script: 缺扩展名自动补全；function: 走 fnLib。 */
  function resolveCallTargetId(target) {
    const raw = String(target || '').trim()
    if (raw.startsWith('function:')) return fnLib.resolveTargetId(raw)
    if (!raw.startsWith('script:')) return null
    const path = raw.slice('script:'.length)
    const names = [`${path}.yaml`, `${path}.yml`, path]
    for (const n of names) {
      const hit = scripts.value.find(x => x.package === activePkg.value && x.name === n)
      if (hit) return hit.id
    }
    return null
  }

  function closeResourcePreview() {
    resourcePreview.open = false
    resourcePreview.kind = 'script'
    resourcePreview.title = ''
    resourcePreview.resource = ''
    resourcePreview.model = null
    resourcePreview.error = ''
  }

  /** 摘要 call 卡片「↗ 子脚本/函数」：只读弹窗展示目标的函数/步骤列表，
   *  不切换当前资源，也不进入编辑器。目标 namespace 决定资源类型。 */
  async function openScriptTarget({ target }) {
    const isFn = String(target || '').startsWith('function:')
    const id = resolveCallTargetId(target)
    if (!id) return toast(`跳转目标不存在：${target}`, 'warn')

    const entry = isFn
      ? fnLib.list.find(f => f.id === id)
      : scripts.value.find(s => s.id === id)
    if (!entry) return toast(`跳转目标不存在：${target}`, 'warn')

    resourcePreview.open = true
    resourcePreview.kind = isFn ? 'function_library' : 'script'
    resourcePreview.title = isFn ? `函数：${target}` : `子脚本：${entry.name || target}`
    resourcePreview.resource = entry.id || target
    resourcePreview.model = null
    resourcePreview.error = ''
    try {
      const parsed = isFn
        ? fnLib.parseFunctionFile(entry.content ?? '', entry.file || '')
        : parseScript(entry.content ?? '')
      if (!parsed?.model) throw new Error('资源内容为空或无法解析')
      resourcePreview.model = parsed.model
      if (parsed.diagnostics?.length) {
        resourcePreview.error = parsed.diagnostics[0].message || '资源解析失败'
        resourcePreview.model = null
      }
    } catch (e) {
      resourcePreview.error = '目标内容无法预览：' + (e.message || e)
    }
  }

  /** 编辑态跳转返回（call/func 打开目标后）：载回上一资源；栈空时按钮不显示。 */
  async function jumpBack() {
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

  /** 设备占用冲突（409 device_busy）：入队弹窗展示对方目标/来源/本地化开始时间，
   *  提供「仍要查看日志」跳控制台对应设备；不打断本页其他功能 */
  function openRunConflict(d) {
    console.warn('[run] device busy (409)', d)
    pushRunConflict({ ...(d || {}), device_id: store.deviceId })
  }

  // ---------- 运行参数流程（阶段 5）：目标声明 params 时先弹参数表单，稀疏 args 提交 ----------
  // exec 完成 API 调用与 run_id 登记；flow 负责表单开关/400 诊断回填字段/覆盖建议缓存/摘要
  const runArgsFlow = useRunArgsFlow({
    exec: async ({ id, name, kind, fnName, startIndex, args }) => {
      startPending.value = true
      // 每次运行清空日志区域，只显示本次运行产生的日志
      runStartTime = Date.now()
      rawLogs = []
      liveLogs.value = []
      try {
        // 函数面板（运行目标=函数库文件）：走函数测试入口运行单个函数
        const rep = kind === 'function_library'
          ? await runYamlFunction(id, store.deviceId, { function: fnName || undefined, start_index: startIndex, args })
          : await runYamlScript(id, store.deviceId, startIndex, args)
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

  /** 运行/从此步骤运行入口：经服务端 entrypoint schema API 取参数声明（P12.3，
   *  前端不解析 YAML）→ 无参数直接运行，有参数弹参数表单；
   *  函数面板：按函数 schema 弹表单，经函数测试入口运行所选函数（缺省 = 文件第一个函数）。
   *  opts.fromUuid（从此运行）→ 脚本取顶层 steps 序号 / 函数定位目标函数与步序；
   *  顶部「运行」按钮不传 → 从头跑。守卫失败一律 toast 说明原因，不做静默 no-op；
   *  schema 加载失败（404 不存在 / 400 无法解析）由 flow 抛结构化错误经 handleRunStartError 提示 */
  async function runScript(scope, opts = {}) {
    if (startPending.value || store.running) return
    if (!store.deviceId) return toast('请先在上方选择设备再运行', 'warn')
    if (scope.kind === 'func') {
      const f = fnLib.list.find(x => x.id === selFnFile.value)
      if (!f) return toast('请先选择函数库文件', 'warn')
      // 运行目标：从此运行落在某函数的某步 → 该函数从该步；顶部运行 → 第一个函数从头
      let fnName = opts.fnName || (f.functions || [])[0] || ''
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
          runnerId: GAMER_YAML_RUNNER_ID,
          entrypoint: `${f.id}#${fnName}`, // 与 runYamlFunction 的 entrypoint 拼装同形态
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
        runnerId: GAMER_YAML_RUNNER_ID,
        entrypoint: s.id,
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

  /** 编辑器模板预览的阈值：脚本沿用当前脚本 config，函数/无 config 时让服务端使用全局值。 */
  function editorMatchThreshold() {
    const configured = scriptShell.kind === 'script' ? Number(scriptShell.model?.config?.threshold) : NaN
    return Number.isFinite(configured) && configured > 0 && configured <= 1 ? configured : undefined
  }

  function onLogBoxMounted(el) { logBox.value = el }

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
    // 运行目标展示名：entrypoint 为主（runner 语义），script_id 为服务端保留的兼容展示字段
    const target = rec.entrypoint || rec.script_id || ''
    const srcTag = sourceLabel(rec.source)
    applyRunRecord({ ...rec, device_id: store.deviceId, display: srcTag ? `${target}（${srcTag}）` : target })
    selScript.value = target
    scriptScope.scriptMode.value = 'run'
    runStartTime = 0   // 不按开始时间过滤，恢复最近日志
    startLogPolling()
    startRunStatusPoll()
    toast(`检测到 ${target}${srcTag ? `（${srcTag}）` : ''} 正在运行，已恢复状态`, 'info')
  }

  /** 关页保护：有未保存修改时浏览器弹出确认（任一面板的编辑/原文态都算） */
  function onBeforeUnload(e) {
    const editing = (scope) => (scope.scriptMode.value === 'edit' && scriptShell.hasModel && scriptShell.dirty)
      || (scope.scriptMode.value === 'raw' && rawEditor.dirty.value)
    if (editing(scriptScope) || editing(funcScope)) {
      e.preventDefault()
      e.returnValue = ''
    }
  }

  onUnmounted(() => {
    if (autoSaveTimer) { clearTimeout(autoSaveTimer); autoSaveTimer = null }
    stopRunStatusPoll()
  })

  /** 面板作用域上下文：同一套共享机制 + 面板锁定的资源类型/编辑模式/选择。
   *  经 workspace context 注入（core.scriptRunner.scripts / .functions），两个
   *  扩展面板各自绑定一份，互不串台。 */
  function buildPanelContext(scope) {
    const isFunc = scope.kind === 'func'
    return {
      kind: scope.kind,
      kindLocked: true,
      runKind: scope.runKind,
      scriptMode: scope.scriptMode,
      activePkg, store, startPending, runStopping, stopScript,
      scriptDeleteConfirmId,
      // 运行区选择与可用性（按面板类型绑定）
      selScript, selFnFile,
      canRunTarget: isFunc ? canRunTargetFunc : canRunTargetScript,
      selTargetId: isFunc ? selTargetIdFunc : selTargetIdScript,
      fnLib, autoSaveDebounced: () => autoSaveDebounced(scope),
      // 函数模式摘要：逐函数分组视图（每组一个 ScriptSummary）+ 解析失败文案
      funcFnViews, funcSummaryError,
      runScript: opts => runScript(scope, opts),
      editCurrentTarget: (fnName = '') => editCurrentTarget(scope, fnName),
      editRawCurrentTarget: () => editRawCurrentTarget(scope),
      startNewTarget: () => startNewTarget(scope),
      deleteCurrentTarget: () => deleteCurrentTarget(scope),
      addFunctionToCurrentFile, renameEditingFunction, deleteFunction,
      editCurrentScript, startNewScript, deleteCurrentScript, liveLogs, onLogBoxMounted,
      // 运行视图：只读摘要 + 运行起点 + call/func 结构化跳转（替代旧源码行点击/文本预览）
      summaryModel, summaryError,
      runFromStep: uuid => runFromStep(scope, uuid),
      openScriptTarget, resourcePreview, closeResourcePreview,
      // 运行参数表单（阶段 5）：目标声明 params 时点运行/从此运行弹出
      runArgsFlow, onRunArgsSubmit,
      // 编辑视图：共享编辑器外壳 + 保存/取消/409 冲突回调
      shell: scriptShell, raw: rawEditor,
      saveEditScript: () => saveEditScript(scope),
      cancelEditScript: () => cancelEditScript(scope),
      saveRawScript: () => saveRawScript(scope),
      cancelRawScript: () => cancelRawScript(scope),
      showYaml, templateNames, jumpBack,
      // 函数编辑态聚焦的函数名（逐函数「编辑」直达；画布锁函数下拉为静态展示）
      editFocusFn,
      onConflictReload, onConflictOverwrite, onConflictDismiss,
      // call/func 目标实参类型回显（同步缓存命中形态），ScriptRunner 经 ctx 传给画布
      resolveTargetSync,
    }
  }
  const scriptPanel = buildPanelContext(scriptScope)
  const functionsPanel = buildPanelContext(funcScope)

  return {
    // 共享机制（Console 壳接线：弹窗/轮询/钩子）
    scriptShell, rawEditor, fnLib,
    liveLogs, startPending, runStopping, runArgsFlow, onRunArgsSubmit,
    startLogPolling, stopLogPolling, pushLog,
    clearCallParamsCache, editorMatchThreshold,
    startRunStatusPoll, stopRunStatusPoll, restoreRunState, onBeforeUnload,
    refreshScripts,
    // 面板作用域上下文（扩展面板经 workspace context 消费）
    scriptPanel, functionsPanel,
  }
}
