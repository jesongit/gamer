// 函数库（data/<pkg>/functions/）外壳辅助：文件列表、目标解析、FunctionLibraryModel 解析
// 与函数级 params 扩展命令（阶段 4：commands set_params/insert_param/update_param/
// remove_param 支持 ['functions', 函数名, 'params'] 容器路径）。
import { reactive, ref } from 'vue'
import { paths } from '../script-editor/commands'
import { parseFunctionLibrary } from '../script-editor/codec'

export function useFunctionLibrary({ api } = {}) {
  const list = ref([]) // FunctionFile 列表：{id, pkg, file, content, version, functions[], updated_at}
  const loading = ref(false)
  const activeFileId = ref(null) // 当前打开/选中的函数库文件 id

  /** 拉取分区函数库文件列表（pkg 必填；失败置空不抛出，页面按无函数库处理）。 */
  async function refresh(pkg) {
    if (!pkg) {
      list.value = []
      return
    }
    loading.value = true
    try {
      list.value = (await api.listFunctions(pkg)) || []
    } catch {
      list.value = []
    } finally {
      loading.value = false
    }
  }

  function clear() {
    list.value = []
    activeFileId.value = null
  }

  function findByFile(file) {
    return list.value.find((f) => f.file === file) || null
  }

  /**
   * func 步骤目标（`<文件短路径>/<函数名>`）→ 函数库文件 id。
   * 文件或函数名不存在返回 null（页面据此提示，不让用户跳进悬空目标）。
   */
  function resolveTargetId(target) {
    const s = String(target || '')
    const idx = s.indexOf('/')
    if (idx <= 0 || idx === s.length - 1) return null
    const file = s.slice(0, idx)
    const fn = s.slice(idx + 1)
    if (fn.includes('/')) return null
    const entry = findByFile(file)
    if (!entry || !Array.isArray(entry.functions) || !entry.functions.includes(fn)) return null
    return entry.id
  }

  function selectFile(id) {
    activeFileId.value = id
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
    list, loading, activeFileId,
    refresh, clear, findByFile, resolveTargetId, selectFile, parseFunctionFile,
    setFunctionParams, insertFunctionParam, updateFunctionParam, removeFunctionParam,
  })
}
