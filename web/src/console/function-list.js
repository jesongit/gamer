// 函数列表视图纯逻辑（函数面板「函数即函数」：用户看到的是一个个函数，不按
// 分类分组浏览）。无 Vue/网络依赖，UI 层在 useConsoleScriptRunner：
// - buildFunctionViews：跨分类平铺全部函数（每个视图自带所属分类与文件 id，
//   运行/编辑/删除按视图寻址）；解析失败的分类内容跳过（编辑态诊断可见）；
// - filterFunctionViews：模糊过滤——「分类/函数名」子串（覆盖中文名/英文名）
//   或中文拼音首字母（「登录」→ dl）任一命中。
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
 * model = 该函数的伪脚本模型（params + run），供 ScriptSummary 渲染与运行起点定位。
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
    for (const fn of functions) {
      views.push({
        fileId: file.id,
        category: file.file || '',
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
