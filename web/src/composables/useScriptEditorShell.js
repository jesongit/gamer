// 脚本编辑器外壳（阶段 4）：Console 紧凑外壳与独立全屏外壳共用的编辑会话状态机。
//
// 职责（plan §8.1「一个编辑核心、两个页面外壳」）：
// - 加载资源（GET /api/scripts/:id 或 /api/functions/:id）→ codec 严格解析 → reactive 模型
//   + CommandStack 接线（组件层所有写操作经命令栈，撤销/重做与 uuid 稳定性由栈保证）；
// - dirty：serialize(model) 与最近一次保存/加载快照比对（computed 随命令栈写入自动重算）；
// - 保存：serialize → saveScript/saveFunction/updateFunction，携带 expected_version；
//   409 {code:"version_conflict"} → conflict 状态，页面弹「重载 / 覆盖」选择；
// - 校验：parse 期诊断（加载时冻结）+ validateScript/validateFunctionLibrary 即时结果合并，
//   解析失败（旧语法残留等）阻塞保存，防止把部分模型序列化覆写磁盘；
// - 选中与插入锚点：受控 selectedUuid；Alt 生成步骤经 setAnchorProvider 注入的画布锚点插入
//   （无画布时回退 defaultAnchor：选中卡之后 / 当前流程末尾）；
// - 结构化跳转：call/func 卡片打开子脚本/函数定义，jumpStack 记录返回位置（资源 + 选中）。
import { computed, reactive, ref } from 'vue'
import { CommandStack } from '../script-editor/commands'
import { parseFunctionLibrary, parseScript, serialize } from '../script-editor/codec'
import { createStep } from '../script-editor/factories'
import { lit } from '../script-editor/model'
import { defaultAnchor, findStepLocation, startIndexOf } from '../script-editor/selection'
import { validateFunctionLibrary, validateScript } from '../script-editor/validation'

const YAML_EXT_RE = /\.(ya?ml)$/i

function ensureYamlExt(name) {
  const t = String(name || '').trim()
  if (!t) return t
  return YAML_EXT_RE.test(t) ? t : `${t}.yml`
}

/** 相对坐标保留 4 位小数（与旧文本记录同精度），且夹取到 0~1。 */
function round4(n) {
  const v = Number(n)
  if (!Number.isFinite(v)) return 0
  return Number(Math.min(1, Math.max(0, v)).toFixed(4))
}

export function useScriptEditorShell({ api, getContext = null } = {}) {
  // ---- 会话状态 ----
  const kind = ref('script') // 'script' | 'function_library'
  const resourceId = ref(null) // <pkg>/<file>.yaml（脚本或函数库文件；新建未保存 = null）
  const pkg = ref('')
  const name = ref('') // 脚本文件名（含扩展名，页面可改名）；函数库 = 文件短路径
  const model = ref(null) // reactive EditorModel
  const stack = ref(null) // CommandStack
  const version = ref(null) // 内容版本短码（expected_version 冲突检测依据）
  const loading = ref(false)
  const saving = ref(false)
  const selectedUuid = ref(null)
  const conflict = ref(null) // {resource, message}：保存 409 version_conflict
  const jumpStack = ref([])
  const parseDiags = ref([]) // 加载时冻结的解析期诊断
  const savedYaml = ref('') // 最近加载/保存的规范 YAML 快照
  const historyTick = ref(0) // 命令栈变更计数（驱动 undo/redo 可用性重算）

  let anchorProvider = null
  let offChange = null

  // ---- 派生 ----
  const hasModel = computed(() => !!model.value && !!stack.value)

  const dirty = computed(() => {
    if (!hasModel.value) return false
    try {
      return serialize(model.value) !== savedYaml.value
    } catch {
      return true // 序列化异常一律按有未保存修改处理（防静默丢失）
    }
  })

  const editorContext = computed(() => (kind.value === 'function_library' ? 'function' : 'script'))

  const diagnostics = computed(() => {
    const base = parseDiags.value
    const m = model.value
    if (!m) return base
    let extra = []
    try {
      const ctx = { ...(getContext ? getContext() : {}), context: editorContext.value }
      if (kind.value === 'script') {
        if (name.value) ctx.selfFile = name.value
        extra = validateScript(m, ctx)
      } else {
        extra = validateFunctionLibrary(m, ctx)
      }
    } catch {
      // 校验器异常不阻塞编辑（保存仍会被 parse 诊断兜底拦截）
    }
    return [...base, ...extra]
  })

  const canUndo = computed(() => {
    void historyTick.value
    const s = stack.value
    return !!s && s.canUndo
  })
  const canRedo = computed(() => {
    void historyTick.value
    const s = stack.value
    return !!s && s.canRedo
  })

  const canJumpBack = computed(() => jumpStack.value.length > 0)
  const jumpBackLabel = computed(() => jumpStack.value[jumpStack.value.length - 1]?.resourceId || '')

  // ---- 模型挂载 ----

  function bindStackNotifications() {
    if (offChange) {
      offChange()
      offChange = null
    }
    if (stack.value) offChange = stack.value.onChange(() => { historyTick.value++ })
  }

  function mountModel(parsedKind, parsed, meta = {}) {
    if (offChange) {
      offChange()
      offChange = null
    }
    kind.value = parsedKind
    model.value = reactive(parsed.model)
    stack.value = new CommandStack(model.value)
    bindStackNotifications()
    resourceId.value = meta.resourceId ?? null
    pkg.value = meta.pkg ?? ''
    name.value = meta.name ?? ''
    version.value = meta.version ?? null
    parseDiags.value = parsed.diagnostics || []
    selectedUuid.value = null
    conflict.value = null
    savedYaml.value = serialize(model.value)
    historyTick.value++
  }

  // ---- 加载 / 新建 ----

  async function loadScript(id) {
    loading.value = true
    try {
      const s = await api.getScript(id)
      const parsed = parseScript(s.content ?? '')
      mountModel('script', parsed, {
        resourceId: s.id,
        pkg: s.package || String(id).split('/')[0] || '',
        name: s.name || '',
        version: s.version ?? null,
      })
      return parsed
    } finally {
      loading.value = false
    }
  }

  async function loadFunctionFile(id) {
    loading.value = true
    try {
      const f = await api.getFunction(id)
      const short = f.file || String(id).split('/').slice(1).join('/').replace(/\.yaml$/i, '')
      const parsed = parseFunctionLibrary(f.content ?? '', { file: short })
      mountModel('function_library', parsed, {
        resourceId: f.id,
        pkg: f.pkg || String(id).split('/')[0] || '',
        name: short,
        version: f.version ?? null,
      })
      return parsed
    } finally {
      loading.value = false
    }
  }

  /** 新建脚本：空模型（config 缺省不启用，步骤为空列表），保存时落盘。 */
  function newScript({ name: n = '新脚本.yml', pkg: p = '' } = {}) {
    mountModel('script', { model: { params: [], config: null, steps: [] }, diagnostics: [] }, {
      pkg: p,
      name: ensureYamlExt(n),
    })
  }

  /** 新建函数库文件：预置一个空函数（顶层键 = 函数名），画布切换/编辑后保存。 */
  function newFunctionFile({ file, pkg: p = '' } = {}) {
    const short = String(file || '').replace(/\.yaml$/i, '')
    mountModel('function_library', {
      model: { file: short, functions: [{ name: 'func1', params: [], steps: [] }] },
      diagnostics: [],
    }, { pkg: p, name: short })
  }

  // ---- 保存 / 冲突 / 重载 ----

  async function save(opts = {}) {
    const m = model.value
    if (!m || !stack.value) return { ok: false, reason: 'empty' }
    const diags = diagnostics.value
    if (diags.length) return { ok: false, reason: 'invalid', diagnostics: diags }
    const yaml = serialize(m)
    saving.value = true
    try {
      const expected = opts.force || !version.value ? undefined : version.value
      let rep
      if (kind.value === 'script') {
        const payload = {
          content: yaml,
          pkg: pkg.value,
          name: ensureYamlExt(name.value || '新脚本.yml'),
        }
        if (resourceId.value) payload.id = resourceId.value
        if (expected) payload.expected_version = expected
        rep = await api.saveScript(payload)
        resourceId.value = rep.id ?? resourceId.value
        pkg.value = rep.package ?? pkg.value
        name.value = rep.name ?? name.value
        version.value = rep.version ?? null
      } else {
        rep = resourceId.value
          ? await api.updateFunction(resourceId.value, { content: yaml, ...(expected ? { expected_version: expected } : {}) })
          : await api.saveFunction({ pkg: pkg.value, name: name.value, content: yaml })
        resourceId.value = rep.id ?? resourceId.value
        if (rep.file) name.value = rep.file
        version.value = rep.version ?? null
      }
      savedYaml.value = yaml
      conflict.value = null
      return { ok: true, result: rep }
    } catch (e) {
      if (e && e.status === 409 && e.data && e.data.code === 'version_conflict') {
        // suppressConflict（自动保存）：不置 conflict 态（不弹重载/覆盖窗），由调用方提示
        if (!opts.suppressConflict) {
          conflict.value = {
            resource: e.data.resource || resourceId.value || '',
            message: e.data.message || '资源已被其他页面修改，请重新加载后再保存',
          }
        }
        return { ok: false, reason: 'conflict', error: e }
      }
      return { ok: false, reason: 'error', error: e }
    } finally {
      saving.value = false
    }
  }

  /** 409 后重载磁盘版本（放弃本地未保存修改）。 */
  async function reload() {
    if (!resourceId.value) {
      conflict.value = null
      return { ok: false, reason: 'empty' }
    }
    const r = kind.value === 'script' ? await loadScript(resourceId.value) : await loadFunctionFile(resourceId.value)
    conflict.value = null
    return { ok: true, result: r }
  }

  function dismissConflict() {
    conflict.value = null
  }

  /** 撤销/重做透传（命令栈为唯一写入口）。 */
  function undo() {
    return stack.value ? stack.value.undo() : false
  }

  function redo() {
    return stack.value ? stack.value.redo() : false
  }

  /** 409 后强制覆盖：不带 expected_version 重存（磁盘版本被无条件替换）。 */
  async function overwrite() {
    return save({ force: true })
  }

  // ---- 会话复位 ----

  function reset() {
    if (offChange) {
      offChange()
      offChange = null
    }
    model.value = null
    stack.value = null
    resourceId.value = null
    pkg.value = ''
    name.value = ''
    version.value = null
    parseDiags.value = []
    selectedUuid.value = null
    conflict.value = null
    jumpStack.value = []
    savedYaml.value = ''
    historyTick.value++
  }

  // ---- 选中 / 插入（Alt 与面板同源锚点） ----

  function select(uuid) {
    selectedUuid.value = uuid
  }

  /** 页面挂载画布后注入锚点提供者（StepCanvas defineExpose 的 anchor）。 */
  function setAnchorProvider(fn) {
    anchorProvider = typeof fn === 'function' ? fn : null
  }

  function insertStep(step, label = '插入步骤') {
    if (!hasModel.value) return false
    const anchor = (anchorProvider && anchorProvider()) || defaultAnchor(model.value, selectedUuid.value)
    return stack.value.apply({ type: 'insert_step', path: anchor.containerPath, index: anchor.index, step }, label)
  }

  /** 录制接线（阶段 6）：在显式锚点插入步骤（不经 anchorProvider/选中态）——
   *  录制开始时锁定的插入目标在多次插入间保持稳定。返回是否成功。 */
  function insertStepWithAnchor(step, label = '插入步骤', anchor = null) {
    if (!hasModel.value || !anchor) return false
    return stack.value.apply({ type: 'insert_step', path: anchor.containerPath, index: anchor.index, step }, label)
  }

  /**
   * 录制接线（阶段 6）：按 uuid 把占位步骤替换为最终步骤（一次事务 = 一次撤销）。
   * 同 kind → update_step 就地改字段（uuid 稳定）；跨 kind（坐标降级 find→tap）→
   * 同事务 remove+insert 整体替换。成功返回 { path, step }，失败（无模型/找不到）返回 null。
   */
  function replaceStepFields(uuid, fields, label = '替换录制占位') {
    if (!hasModel.value) return null
    const loc = findStepLocation(model.value, uuid)
    if (!loc) return null
    const sameKind = fields && fields.kind ? fields.kind === loc.step.kind : true
    let ok = false
    if (sameKind) {
      const patch = { ...fields }
      delete patch.uuid
      delete patch.kind
      stack.value.transaction(() => {
        ok = stack.value.apply({ type: 'update_step', path: loc.path, fields: patch }, label)
      }, label)
    } else {
      const next = { ...fields }
      delete next.uuid
      stack.value.transaction(() => {
        const removed = stack.value.apply({ type: 'remove_step', path: loc.containerPath, index: loc.index }, label)
        const inserted = stack.value.apply({ type: 'insert_step', path: loc.containerPath, index: loc.index, step: createStep(next.kind, next) }, label)
        ok = removed && inserted
      }, label)
    }
    return ok ? { path: loc.path, step: loc.step } : null
  }

  // ---- Alt 便捷工厂（plan §10 迁移矩阵：Alt 投屏/模板/取色 → 类型化步骤） ----

  function insertTapAt(x, y) {
    return insertStep(createStep('tap', { at: lit([round4(x), round4(y)]) }), `Alt 点击 → tap (${round4(x)}, ${round4(y)})`)
  }

  function insertSwipeBetween(from, to, durationMs) {
    const dur = Math.max(1, Math.round(durationMs || 1000))
    return insertStep(createStep('swipe', {
      from: lit([round4(from[0]), round4(from[1])]),
      to: lit([round4(to[0]), round4(to[1])]),
      time: lit(`${dur}ms`),
    }), `Alt 滑动 → swipe ${dur}ms`)
  }

  function insertFindTemplate(template) {
    return insertStep(createStep('find', {
      template: lit(template),
      block: [],
      verify: false,
      timeout: null,
      then: [],
      else: [],
    }), `Alt 模板 → find ${template}`)
  }

  function insertColorCheck(at, hex) {
    return insertStep(createStep('color', {
      at: lit([round4(at[0]), round4(at[1])]),
      expect: [{ color: lit(hex), click: false, steps: [] }],
      else: [],
    }), `Alt 取色 → color ${hex}`)
  }

  // ---- 运行起点映射（uuid → 引擎 start_index；嵌套步骤返回 null） ----

  function runStartIndexOf(uuid) {
    if (!hasModel.value) return null
    return startIndexOf(model.value, uuid)
  }

  // ---- 结构化跳转（call/func 卡片 → 目标模型，带返回位置） ----

  async function jumpToScript(id) {
    if (hasModel.value && resourceId.value) {
      pushJump()
    }
    return loadScript(id)
  }

  async function jumpToFunctionFile(id) {
    if (hasModel.value && resourceId.value) {
      pushJump()
    }
    return loadFunctionFile(id)
  }

  function pushJump() {
    jumpStack.value.push({ kind: kind.value, resourceId: resourceId.value, selectedUuid: selectedUuid.value })
    if (jumpStack.value.length > 8) jumpStack.value.shift()
  }

  async function jumpBack() {
    const prev = jumpStack.value.pop()
    if (!prev) return false
    if (!prev.resourceId) {
      reset()
      return true
    }
    if (prev.kind === 'function_library') await loadFunctionFile(prev.resourceId)
    else await loadScript(prev.resourceId)
    selectedUuid.value = prev.selectedUuid ?? null
    return true
  }

  return reactive({
    kind, resourceId, pkg, name, model, stack, version, loading, saving,
    selectedUuid, conflict, jumpStack, parseDiags, savedYaml,
    hasModel, dirty, editorContext, diagnostics, canUndo, canRedo,
    canJumpBack, jumpBackLabel,
    loadScript, loadFunctionFile, newScript, newFunctionFile,
    save, reload, overwrite, dismissConflict, reset, undo, redo,
    select, setAnchorProvider, insertStep, insertStepWithAnchor, replaceStepFields,
    insertTapAt, insertSwipeBetween, insertFindTemplate, insertColorCheck,
    runStartIndexOf, jumpToScript, jumpToFunctionFile, jumpBack,
  })
}
