// 临时验证脚本：从 Console.vue 提取 validateScriptCode / computeRunLineMap 运行
import { readFileSync, readdirSync } from 'fs'
import { createRequire } from 'module'
const require = createRequire(import.meta.url)
const yamlLoad = require('js-yaml').load

const src = readFileSync(new URL('./src/views/Console.vue', import.meta.url), 'utf8')

function extractFn(name) {
  const start = src.indexOf(`function ${name}(`)
  if (start < 0) throw new Error(`${name} not found`)
  let depth = 0, i = src.indexOf('{', start), end = -1
  for (; i < src.length; i++) {
    if (src[i] === '{') depth++
    else if (src[i] === '}') { depth--; if (depth === 0) { end = i + 1; break } }
  }
  return src.slice(start, end)
}

const tmplDir = new URL('../server/data/com.miHoYo.hkrpg/tmpl/', import.meta.url)
const names = readdirSync(tmplDir).map(f => f.replace(/\.png$/, '') + '.png')
const templatesData = { value: names.map(n => ({ name: n, pkg: 'com.miHoYo.hkrpg' })) }
const activePkg = { value: 'com.miHoYo.hkrpg' }
const scriptsData = { value: [
  { id: 'com.miHoYo.hkrpg/test1.yaml', package: 'com.miHoYo.hkrpg', name: 'test1.yaml', content: [
    'func:',
    '  - fun1:',
    '    cond: mail.png',
    '    steps:',
    '      - find: $1',
    '  - fun2:',
    '    - log: xxx',
    'steps:',
    '  - log: top',
  ].join('\n') },
  { id: 'com.miHoYo.hkrpg/test2.yml', package: 'com.miHoYo.hkrpg', name: 'test2.yml', content: 'func:\n  - nope:\n    - log: x\nsteps:\n  - log: y' },
] }
const make = new Function('templatesData', 'activePkg', 'scriptsData', 'yamlParse', 'yamlLoad', `
  ${extractFn('validateScriptCode')}
  ${extractFn('computeRunLineMap')}
  return { validateScriptCode, computeRunLineMap }
`)
const { validateScriptCode, computeRunLineMap } = make(templatesData, activePkg, scriptsData, (c) => yamlLoad(c), yamlLoad)

let fail = 0
function check(name, cond, detail) {
  console.log(`${cond ? 'PASS' : 'FAIL'} [${name}]${cond ? '' : ' ' + detail}`)
  if (!cond) fail++
}

// ---------- 1. throw 校验放宽 ----------
check('throw 数字原因不报错', validateScriptCode(`
func:
  - f:
    - find: tpl0.png
      then:
        - throw: 404
steps:
  - f
`).filter(e => !e.includes('模板不存在')).length === 0)
check('throw 布尔原因不报错', validateScriptCode(`
func:
  - f:
    - find: tpl0.png
      then:
        - throw: true
steps:
  - f
`).filter(e => !e.includes('模板不存在')).length === 0)
check('throw 数组值仍报错', validateScriptCode(`
func:
  - f:
    - find: tpl0.png
      then:
        - throw: [1, 2]
steps:
  - f
`).some(e => e.includes('throw')))
check('真实脚本（含 func）校验通过', validateScriptCode(
  readFileSync(new URL('../server/data/com.miHoYo.hkrpg/yaml/日常遗器.yml', import.meta.url), 'utf8')
).length === 0)
check('func 内 find + else throw（字符串原因）通过', validateScriptCode(`
func:
  - mail_recv:
    - find: mail.png
      timeout: 5s
      else:
        - throw: 没有邮件入口
      then:
        - find: mail_rev.png
steps:
  - mail_recv
`).length === 0)

// ---------- 2. 行映射 ----------
const real = readFileSync(new URL('../server/data/com.miHoYo.hkrpg/yaml/日常遗器.yml', import.meta.url), 'utf8')
const lines = real.split('\n')
const map = computeRunLineMap(lines)
const fmt = m => m ? `${m.func ? m.func + '()' : 'steps'}[${m.index}]` : '-'
console.log('\n真实脚本行映射（只列非空行）：')
lines.forEach((l, i) => { if (l.trim()) console.log(`${String(i).padStart(3)} ${fmt(map[i]).padEnd(14)} ${l}`) })

// 断言：func 定义行不可选；steps 行索引从 0 连续；func 体内索引正确
check('func 定义行不可选', map[2] === null) // "- mail_recv:" 行（index 2）
const stepsEntries = map.filter(m => m && m.func === null)
check('steps 索引连续', stepsEntries.every((m, i) => m.index === i), JSON.stringify(stepsEntries))
const mailBody = map.filter(m => m && m.func === 'mail_recv')
check('mail_recv 函数体仅顶层可选（then 子步骤不可选）', mailBody.length === 1 && mailBody[0].index === 0, JSON.stringify(mailBody))
const apBody = map.filter(m => m && m.func === 'ap_burn')
check('ap_burn 函数体 12 步索引连续', apBody.length === 12 && apBody.every((m, i) => m.index === i), JSON.stringify(apBody))
const mailCallLine = lines.findIndex(l => /^(\s*)- mail_recv\s*$/.test(l))
check('选中 steps[1]（mail_recv 调用行）映射正确', map[mailCallLine] && map[mailCallLine].func === null && map[mailCallLine].index === 1, JSON.stringify(map[mailCallLine]))

// 映射形式 func + 深缩进函数体
const mapForm = computeRunLineMap(`
func:
  f1:
    - find: a.png
    - log: x
  f2:
    - log: y
steps:
  - log: top
`.split('\n'))
check('映射形式 func 体可选', mapForm[3]?.func === 'f1' && mapForm[3]?.index === 0 && mapForm[4]?.func === 'f1' && mapForm[4]?.index === 1 && mapForm[6]?.func === 'f2', JSON.stringify(mapForm.filter(Boolean)))

// 映射形式 + 同列函数体（YAML 序列值同列特例）
const sameCol = computeRunLineMap(`
func:
  f1:
  - log: a
  - log: b
steps:
  - log: top
`.split('\n'))
check('映射同列函数体可选', sameCol[3]?.func === 'f1' && sameCol[3]?.index === 0 && sameCol[4]?.func === 'f1' && sameCol[4]?.index === 1 && sameCol[6]?.func === null && sameCol[6]?.index === 0, JSON.stringify(sameCol.filter(Boolean)))

// 列表形式 func（用户脚本同款）：then 子步骤不可选
const nested = computeRunLineMap(`
func:
  - f:
    - find: a.png
      then:
        - log: hit
steps:
  - f
`.split('\n'))
const nestedSel = nested.map((m, i) => m ? i : -1).filter(i => i >= 0)
check('列表形式只选函数体首层与 steps 顶层', JSON.stringify(nestedSel) === '[3,7]', JSON.stringify(nestedSel))

// 无 func 的脚本（行为与旧版一致）
const plain = computeRunLineMap(`
steps:
  - log: a
  - find: b.png
    then:
      - log: c
  - log: d
`.split('\n'))
const plainSel = plain.map((m, i) => m ? { i, ...m } : null).filter(Boolean)
check('纯 steps 脚本索引正确', JSON.stringify(plainSel) === '[{"i":2,"func":null,"index":0},{"i":3,"func":null,"index":1},{"i":6,"func":null,"index":2}]', JSON.stringify(plainSel))

// config 列表形式不干扰 steps 计数（旧版会把这些行也计入）
const cfgList = computeRunLineMap(`
config:
  - interval: 500ms
  - threshold: 0.9
steps:
  - log: a
  - log: b
`.split('\n'))
const cfgSel = cfgList.map((m, i) => m ? { i, ...m } : null).filter(Boolean)
check('config 列表项不可选', JSON.stringify(cfgSel) === '[{"i":5,"func":null,"index":0},{"i":6,"func":null,"index":1}]', JSON.stringify(cfgSel))

// ---------- 3. func cond 参数（2026-08-27） ----------
const condOk = `func:
  - fun1:
    cond: mail.png
    steps:
      - find: $1
  - fun2:
    cond:
      - mail.png
      - task_list.png
    steps:
      - log: x
  - fun3:
    - log: a
    cond: mail.png
steps:
  - fun1: a.png
    then:
      - log: ok
`
check('cond 单模板/多模板/函数体之后 均通过', validateScriptCode(condOk).length === 0, JSON.stringify(validateScriptCode(condOk)))
check('cond 模板不存在报错', validateScriptCode(`func:
  - f:
    cond: nope.png
    steps:
      - log: x
steps:
  - f
`).some(e => e.includes('模板不存在')))
check('cond 非法（数字列表项）报错', validateScriptCode(`func:
  - f:
    cond:
      - 123
    steps:
      - log: x
steps:
  - f
`).some(e => e.includes('cond')))
check('代码写法 cond+同列 dash 行给出引导提示', validateScriptCode(`func:
  - f:
    cond: mail.png
    - find: $1
steps:
  - f
`).some(e => e.includes('bad indentation') || e.includes('cond')))
check('函数体步骤内 cond 提示函数级', validateScriptCode(`func:
  - f:
    steps:
      - cond: mail.png
steps:
  - f
`).some(e => e.includes('函数级条件')))

// ---------- 4. 跨文件函数调用（2026-08-27） ----------
check('跨文件调用存在脚本+函数 通过', validateScriptCode(`steps:
  - test1:fun1: a.png mail.png
    then:
      - log: ok
  - test1:fun2
`).length === 0, JSON.stringify(validateScriptCode(`steps:
  - test1:fun1: a.png mail.png
    then:
      - log: ok
  - test1:fun2
`)))
check('跨文件调用缺扩展名自动补全（test2.yml 命中）', validateScriptCode('steps:\n  - test2:nope\n').length === 0, JSON.stringify(validateScriptCode('steps:\n  - test2:nope\n')))
check('跨文件调用子脚本不存在报错', validateScriptCode('steps:\n  - nofile:fun1: x\n').some(e => e.includes('子脚本不存在')))
check('跨文件调用函数不存在报错', validateScriptCode('steps:\n  - test1:nofunc\n').some(e => e.includes('未定义函数')))
check('跨文件调用参数非法报错（YAML 数组值）', validateScriptCode('steps:\n  - test1:fun1: [0.5, 0.6]\n').some(e => e.includes('实参')))
check('跨文件调用+then/else 子步骤递归校验', validateScriptCode(`steps:
  - test1:fun1: a.png
    then:
      - find: nope.png
`).some(e => e.includes('模板不存在')))

// ---------- 5. 带值动作漏写冒号（- throw 未知界面 标量步骤） ----------
const noColon = `func:
  - itm_recv:
    - find: task_list.png
      timeout: 1s
      else:
        - throw 未知界面
steps:
  - itm_recv
`
const noColonErrs = validateScriptCode(noColon)
check('漏写冒号给出定向提示', noColonErrs.some(e => e.includes('- throw: 未知界面') && e.includes('需写冒号')), JSON.stringify(noColonErrs))
check('补冒号后校验通过', validateScriptCode(noColon.replace('- throw 未知界面', '- throw: 未知界面')).length === 0)

// ---------- 6. 漏写分支键（timeout 后跟更深缩进 - 行） ----------
const noElse = `func:
  - itm_recv:
    - find: task_list.png
      timeout: 1s
        - throw: 未知界面
steps:
  - itm_recv
`
const noElseErrs = validateScriptCode(noElse)
check('漏写分支键给出 else: 示例提示', noElseErrs.some(e => e.includes('漏写分支键') && e.includes('else:') && e.includes('- throw: 未知界面')), JSON.stringify(noElseErrs))
check('补 else: 后校验通过', validateScriptCode(`func:
  - itm_recv:
    - find: task_list.png
      timeout: 1s
      else:
        - throw: 未知界面
steps:
  - itm_recv
`).length === 0)
check('else: 带注释不误报（真错误指向 timeout）', validateScriptCode(`func:
  - itm_recv:
    - find: task_list.png
      else: # 如果没进委托一般是进了生存索引，切回任务界面
        - find: task_list.png
          timeout: 1s
            - throw: 未知界面
steps:
  - itm_recv
`).some(e => e.includes('第 6 行')))

// ---------- 7. 纯函数库脚本（无 steps） ----------
check('纯函数库（只有 func）校验通过', validateScriptCode(`func:
  - hello:
    - log: hi
  - f2:
    cond: mail.png
    steps:
      - log: $1
`).length === 0, JSON.stringify(validateScriptCode(`func:
  - hello:
    - log: hi
  - f2:
    cond: mail.png
    steps:
      - log: $1
`)))
check('无 steps 无 func 报错', validateScriptCode(`config:
  interval: 500ms
`).some(e => e.includes('需要 steps 或 func')))

console.log(fail ? `\n${fail} 项失败` : '\n全部通过')
process.exit(fail ? 1 : 0)
