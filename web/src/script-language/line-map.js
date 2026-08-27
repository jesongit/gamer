/** 行 → 运行目标映射（与传入的脚本行数组平行）：可选逻辑行 → { func: 函数名|null,
 * index: 步骤序号 }，其余行 → null。按根段落（config/func/steps，缩进 0）扫描：
 * steps 段首个 "- " 行确立顶层缩进；func 段首个条目行（"- 名:" 列表形式 /
 * "名:" 映射形式）确立条目缩进，函数体内首个 "- " 行确立函数体缩进。
 * 省略段落键的简写（与引擎 normalize_top 一致）：无 config:/func:/steps: 根键
 * 时顶层序列按 steps、顶层映射按 func 扫描（条目缩进 0）。
 * 函数名行也可选：点击 = 从头运行整个函数（引擎先判 cond 再跑函数体），
 * 与函数体内首行同目标（func + index 0）
 *
 * 从 Console.vue 原样搬移（2026-08-27 抽离为独立模块，零行为变化）。 */
export function computeRunLineMap(lines) {
  // 省略段落键判定：无任何段落根键时，首个内容行是 "- " → steps，否则 → func
  let implied = ''
  if (!lines.some(l => /^(config|func|steps):(?:\s|$)/.test(l))) {
    const first = lines.find(l => l.trim() && !/^\s*#/.test(l)) || ''
    implied = /^\s*-\s/.test(first) ? 'steps' : 'func'
  }
  const map = new Array(lines.length).fill(null)
  let section = implied         // 当前根段落：steps / func / ''（其他或未入段；省略写法全程不变）
  let stepsIndent = -1, stepCount = 0
  let entryIndent = -1, entryDash = null, bodyIndent = -1, funcName = null, bodyCount = 0
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]
    if (!line.trim() || /^\s*#/.test(line)) continue
    if (!implied) {
      const root = line.match(/^(\S+?):(?:\s|$)/)
      if (root) {
        section = root[1] === 'steps' || root[1] === 'func' ? root[1] : ''
        stepsIndent = -1; stepCount = 0
        entryIndent = -1; entryDash = null; bodyIndent = -1; funcName = null; bodyCount = 0
        continue
      }
    }
    const dash = line.match(/^(\s*)-\s/)
    const indent = dash ? dash[1].length : (line.match(/^(\s*)\S/) || ['', ''])[1].length
    if (indent < 0) continue
    if (section === 'steps') {
      if (dash) {
        if (stepsIndent < 0) stepsIndent = indent
        if (indent === stepsIndent) map[i] = { func: null, index: stepCount++ }
      }
    } else if (section === 'func') {
      if (entryDash === false && dash && indent === entryIndent) {
        // 映射形式条目（"名:" 无 -）的同列 "- " 行 = 函数体（YAML 序列值同列特例）
        if (bodyIndent < 0) bodyIndent = indent
        map[i] = { func: funcName, index: bodyCount++ }
      } else if (indent === entryIndent || entryIndent < 0) {
        // 函数条目行（首个条目确立条目缩进）："- 名:"（列表形式）或 "名:"（映射形式）
        const em = line.match(/^\s*(?:-\s+)?([^:\s]+)\s*:/)
        entryIndent = indent
        entryDash = !!dash
        funcName = em ? em[1] : null
        bodyIndent = -1; bodyCount = 0
        if (funcName) map[i] = { func: funcName, index: 0 }
      } else if (funcName && dash && indent > entryIndent) {
        // 函数体顶层步骤（首个确立函数体缩进；then/else 等更深子步骤不选）
        if (bodyIndent < 0) bodyIndent = indent
        if (indent === bodyIndent) map[i] = { func: funcName, index: bodyCount++ }
      }
    }
  }
  return map
}
