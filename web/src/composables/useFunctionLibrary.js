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

  /** 拉取函数库文件列表（pkg 必填；失败置空不抛出，页面按无函数库处理）。 */
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
  }

  /** 函数名 → 定义它的函数库文件（统一命名空间；找不到返回 null）。 */
  function findByName(name) {
    const key = String(name || '')
    return list.value.find((f) => Array.isArray(f.functions) && f.functions.includes(key)) || null
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
    refresh, clear, findByName, parseFunctionFile,
    setFunctionParams, insertFunctionParam, updateFunctionParam, removeFunctionParam,
  })
}
