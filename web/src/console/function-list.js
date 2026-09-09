// 函数列表视图纯逻辑（函数面板「函数即函数」：用户看到的是一个个函数，不按
// 文件分组浏览）。无 Vue/网络依赖，UI 层在 useConsoleScriptRunner：
// - buildFunctionViews：跨函数库文件平铺全部函数（每个视图自带所属文件短名
//   category 与文件 id，运行按 `<pkg>#<名>` 寻址，编辑仅默认库 `_function`）；
//   解析失败的文件内容跳过（编辑态诊断可见）；
// - filterFunctionViews：模糊过滤——「来源文件/函数名」子串（覆盖中文名/英文
//   名）或中文拼音首字母（「登录」→ dl）任一命中。
import { pinyin } from 'pinyin-pro'

/** 拼音首字母匹配器工厂：cached(text) → 小写首字母串（非汉字字符原样保留）。 */
export function createPinyinInitials() {
  const cache = new Map()
  return text => {
    const key = String(text || '')
    let value = cache.get(key)
    if (value === undefined) {
      value = pinyin(key, { pattern: 'first', toneType: 'none', type: 'array' })
        .join('').replace(/\s+/g, '').toLowerCase()
      cache.set(key, value)
    }
    return value
  }
}

/**
 * 函数库文件列表 → 全部函数视图列表 [{fileId, category, name, model}]。
 * category = 文件短名（去 .yaml 后缀；默认库 = `_function`，手动拆分文件如
 * `_function_battle`）。model = 该函数的伪脚本模型（params + run），供
 * ScriptSummary 渲染与运行起点定位。
 */
export function buildFunctionViews(files, parseFunctionFile) {
  const views = []
  for (const file of files) {
    let parsed
    try {
      parsed = parseFunctionFile(file.content ?? '', file.file || '')
    } catch {
      continue
    }
    const functions = parsed?.model?.functions
    if (!Array.isArray(functions)) continue
    const category = String(file.file || '').replace(/\.yaml$/i, '')
    for (const fn of functions) {
      views.push({
        fileId: file.id,
        category,
        name: fn.name,
        model: { params: fn.params || [], run: fn.run || [] },
      })
    }
  }
  return views
}

/** 模糊过滤：「分类/函数名」子串或拼音首字母串命中即保留；空查询原样返回。 */
export function filterFunctionViews(views, query, pinyinInitials) {
  const q = String(query || '').trim().toLowerCase()
  if (!q) return views
  return views.filter(view =>
    `${view.category}/${view.name}`.toLowerCase().includes(q)
    || pinyinInitials(`${view.category}${view.name}`).includes(q))
}
