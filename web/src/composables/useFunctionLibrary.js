// 函数库外壳辅助（简化计划 Phase 1：Package 函数库 = automations/ 内
// `_function*.yaml`，默认只有 `_function.yaml`；统一命名空间，函数名即调用名，
// 目录与文件名不影响调用）：文件列表、FunctionLibraryModel 解析与函数级
// params 扩展命令（commands set_params/insert_param/update_param/remove_param
// 支持 ['functions', 函数名, 'params'] 容器路径）。
import { reactive, ref } from 'vue'
import { paths } from '../script-editor/commands'
import { parseFunctionLibrary } from '../script-editor/codec'

export function useFunctionLibrary({ api } = {}) {
  const list = ref([]) // FunctionFile 列表：{id, pkg, file, content, version, functions[], updated_at}
  const loading = ref(false)
  let refreshSeq = 0
  let activePackage = ''
  const fileLoads = new Map()

  /** 拉取函数库文件列表（pkg 必填；失败置空不抛出，页面按无函数库处理）。 */
  async function refresh(pkg) {
    const requestSeq = ++refreshSeq
    const requestedPackage = String(pkg || '').trim()
    if (requestedPackage !== activePackage) {
      // 新请求返回前先隐藏旧 Package，避免补全/运行暂时串用旧函数。
      activePackage = requestedPackage
      list.value = []
    }
    if (!requestedPackage) {
      list.value = []
      loading.value = false
      return
    }
    loading.value = true
    try {
      const next = (await api.listFunctions(requestedPackage)) || []
      // Package 切换后，旧函数库响应不得覆盖当前候选。
      if (requestSeq === refreshSeq) list.value = next
    } catch {
      if (requestSeq === refreshSeq) list.value = []
    } finally {
      if (requestSeq === refreshSeq) loading.value = false
    }
  }

  function clear() {
    refreshSeq += 1
    activePackage = ''
    fileLoads.clear()
    list.value = []
  }

  /** 从列表注记或原文取得函数名；列表注记缺失时不让补全静默失效。 */
  function namesFor(entry) {
    if (!entry) return []
    const annotated = Array.isArray(entry.functions)
      ? entry.functions.filter((name) => typeof name === 'string' && name)
      : []
    if (annotated.length || typeof entry.content !== 'string' || !entry.content) {
      return [...new Set(annotated)]
    }
    try {
      const parsed = parseFunctionFile(entry.content, entry.file || '')
      return (parsed?.model?.functions || []).map((fn) => fn.name).filter(Boolean)
    } catch {
      return []
    }
  }

  /** 函数名 → 定义它的函数库文件（统一命名空间；找不到返回 null）。 */
  function findByName(name) {
    const key = String(name || '')
    return list.value.find((f) => namesFor(f).includes(key)) || null
  }

  /** 列表只带函数名注记而没有内容时，按需读取完整文件供参数 Schema 使用。 */
  async function loadFile(id) {
    const key = String(id || '')
    if (!key || typeof api?.getFunction !== 'function') return null
    const entry = list.value.find((item) => item.id === key)
    if (entry && typeof entry.content === 'string' && entry.content !== '') return entry
    if (fileLoads.has(key)) return fileLoads.get(key)
    const request = Promise.resolve()
      .then(() => api.getFunction(key))
      .then((data) => {
        const current = list.value.find((item) => item.id === key)
        if (current && data && typeof data === 'object') Object.assign(current, data)
        return current || data
      })
      .finally(() => fileLoads.delete(key))
    fileLoads.set(key, request)
    return request
  }

  /** 函数库文件内容 → FunctionLibraryModel（shell.loadFunctionFile 的同步解析形态）。 */
  function parseFunctionFile(content, file) {
    return parseFunctionLibrary(content ?? '', { file: file ?? '' })
  }

  // ---- 函数级 params 命令（写入仍走 CommandStack，可撤销） ----

  function setFunctionParams(stack, fnName, params) {
    return stack.apply({ type: 'set_params', path: paths.functionParams(fnName), params }, '编辑函数参数')
  }

  function insertFunctionParam(stack, fnName, index, decl) {
    return stack.apply({ type: 'insert_param', path: paths.functionParams(fnName), index, decl }, '添加函数参数')
  }

  function updateFunctionParam(stack, fnName, index, decl) {
    return stack.apply({ type: 'update_param', path: paths.functionParams(fnName), index, decl }, '编辑函数参数')
  }

  function removeFunctionParam(stack, fnName, index) {
    return stack.apply({ type: 'remove_param', path: paths.functionParams(fnName), index }, '删除函数参数')
  }

  return reactive({
    list, loading,
    refresh, clear, findByName, namesFor, loadFile, parseFunctionFile,
    setFunctionParams, insertFunctionParam, updateFunctionParam, removeFunctionParam,
  })
}
