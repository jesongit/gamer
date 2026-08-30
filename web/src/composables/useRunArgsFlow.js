// 运行参数流程（阶段 5，plan §12.1/§12.2）：Console 手动运行与独立页函数测试共用的
// 「有参数先弹表单 → 稀疏 args 提交 → 400 诊断回填字段 → 202 摘要+建议缓存」状态机。
//
// 宿主注入：
// - exec({ id, name, kind, fnName, startIndex, args }) → 202 回复（宿主完成 API 调用与
//   run_id 登记/轮询启动；抛错时 400 invalid_args 由本流程消化，其余原样抛回宿主处理）；
// - notify({ rep, args, summary })：202 成功后的页面反馈（toast/日志区摘要）。
// 纯流程不持有路由/toast/api 单例，vitest 直测。
import { reactive } from 'vue'
import {
  describeResolvedArgs, extractParams, loadRunArgsSuggestion,
  mapArgDiagnostics, saveRunArgsSuggestion,
} from '../script-editor/params'

export function useRunArgsFlow({ exec, notify = () => {}, storage = undefined } = {}) {
  // 弹窗态（RunParamsModal props 直接绑定 modal.*）
  const modal = reactive({
    open: false,
    title: '运行参数',
    desc: '',
    submitLabel: '▶ 运行',
    targetId: '',       // 脚本 id 或函数库文件 id（<pkg>/<file>.yaml）
    targetName: '',
    kind: 'script',     // 'script' | 'function_library'
    fnName: null,       // 函数测试目标函数名
    startIndex: 0,
    params: [],
    initialArgs: {},
    suggestions: {},
    templates: [],
    fieldErrors: {},    // 服务端 400 诊断按参数名映射
    generalErrors: [],
    submitting: false,
  })

  function reset() {
    modal.open = false
    modal.desc = ''
    modal.fieldErrors = {}
    modal.generalErrors = []
    modal.initialArgs = {}
  }

  function close() {
    reset()
  }

  /**
   * 发起一次运行/测试。有参数声明 → 打开表单（返回 {form:true} 等待用户提交）；
   * 无参数声明 → 跳过表单直接 exec（args 省略）。submitting 期间与表单打开时忽略重复发起。
   */
  async function begin(opts = {}) {
    if (modal.open || modal.submitting) return { form: false, busy: true }
    const kind = opts.kind || 'script'
    const params = extractParams(opts.yaml ?? '', kind, opts.fnName ?? null)
    Object.assign(modal, {
      title: opts.title || (kind === 'function_library' ? '测试函数参数' : '运行参数'),
      desc: opts.desc || '',
      submitLabel: opts.submitLabel || (kind === 'function_library' ? '▶ 测试' : '▶ 运行'),
      targetId: opts.id || '',
      targetName: opts.name || opts.id || '',
      kind,
      fnName: opts.fnName ?? null,
      startIndex: opts.startIndex || 0,
      params,
      initialArgs: opts.initialArgs || {},
      suggestions: loadRunArgsSuggestion(opts.id || '', storage),
      templates: opts.templates || [],
    })
    if (!params.length) {
      await run(undefined)
      return { form: false }
    }
    modal.open = true
    return { form: true }
  }

  /** RunParamsModal 提交（已过客户端校验）：稀疏 args → 服务端。 */
  async function confirm(args) {
    return run(args)
  }

  async function run(args) {
    modal.submitting = true
    modal.fieldErrors = {}
    modal.generalErrors = []
    const opts = {
      id: modal.targetId,
      name: modal.targetName,
      kind: modal.kind,
      fnName: modal.fnName,
      startIndex: modal.startIndex,
      args,
    }
    try {
      const rep = await exec(opts)
      modal.open = false
      // 仅显式覆盖值进建议缓存；「使用默认值」不写入（默认值变化不被旧缓存遮蔽）
      if (args && Object.keys(args).length) saveRunArgsSuggestion(modal.targetId, args, storage)
      notify({
        rep,
        args,
        summary: describeResolvedArgs(modal.params, args, rep?.resolved_args),
      })
      return { ok: true, rep }
    } catch (e) {
      if (e && e.status === 400 && e.data && e.data.error === 'invalid_args' && modal.open) {
        const mapped = mapArgDiagnostics(
          e.data.diagnostics,
          modal.params.map((p) => p.name),
        )
        modal.fieldErrors = mapped.byName
        const general = [...mapped.other]
        if (e.data.message) general.unshift(String(e.data.message))
        modal.generalErrors = general
        return { ok: false, reason: 'invalid_args', diagnostics: e.data.diagnostics || [] }
      }
      // 设备占用 409 等其他错误：关闭表单，交宿主统一处理（冲突弹窗/报错提示）；
      // invalid_args 但表单未打开（无参数直跑路径）同样上抛——否则诊断无处展示、点运行看似无反应
      reset()
      throw e
    } finally {
      modal.submitting = false
    }
  }

  return { modal, begin, confirm, close }
}
