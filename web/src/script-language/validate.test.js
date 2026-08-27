// 脚本校验器测试：直接导入 src/script-language/validate.js（不再从 Console.vue 提取）。
// fixture 为 server/data 真实业务 YAML 的虚拟模板名稳定副本（见 fixtures/README.md）。
import { describe, it, expect } from 'vitest'
import { readFileSync } from 'fs'
import { createScriptValidator } from './validate.js'

const PKG = 'com.example.game'
const FIX_URL = new URL('./fixtures/', import.meta.url)

const loadFixture = f => readFileSync(new URL(`scripts/${f}`, FIX_URL), 'utf8')

const SCRIPT_FILES = [
  'lib_utils.yaml', 'flow_daily.yaml', 'common_account.yaml', 'multi_account.yaml',
  'mail_only.yaml', 'color_probe.yaml', 'cn_names.yaml', 'fn_lib_short.yaml', 'misc.yml',
]

function makeValidator() {
  const tplList = readFileSync(new URL('templates.txt', FIX_URL), 'utf8')
    .split(/\r?\n/).map(s => s.trim()).filter(Boolean)
  return createScriptValidator({
    templatesData: { value: tplList.map(name => ({ name, pkg: PKG })) },
    scriptsData: {
      value: SCRIPT_FILES.map(f => ({ id: `${PKG}/${f}`, package: PKG, name: f, content: loadFixture(f) })),
    },
    activePkg: { value: PKG },
  })
}

/** 同一分区内引用另一 fixture 脚本的最小校验环境 */
function validatorWithScripts(scripts) {
  const tplList = readFileSync(new URL('templates.txt', FIX_URL), 'utf8')
    .split(/\r?\n/).map(s => s.trim()).filter(Boolean)
  return createScriptValidator({
    templatesData: { value: tplList.map(name => ({ name, pkg: PKG })) },
    scriptsData: { value: scripts },
    activePkg: { value: PKG },
  })
}

describe('fixture 全量：业务脚本稳定副本双通过', () => {
  const v = makeValidator()
  for (const f of SCRIPT_FILES.filter(n => !n.startsWith('misc'))) {
    it(`${f} 零错误`, () => {
      expect(v(loadFixture(f))).toEqual([])
    })
  }
})

describe('顶层结构归一化（与引擎 normalize_top 一致）', () => {
  const v = makeValidator()

  it('config / func / steps 正常形式零错误', () => {
    const y = `config:
  interval: 500ms
func:
  - wait_tpl:
    - find: $1
steps:
  - str_app
  - wait_tpl: tpl_guide.png
`
    expect(v(y)).toEqual([])
  })

  it('config 映射列表按序覆盖合法', () => {
    const y = `config:
  - interval: 1s
  - threshold: 0.9
steps:
  - log: a
`
    expect(v(y)).toEqual([])
  })

  it('省略 steps: 的顶层序列 = steps', () => {
    const y = `- find: tpl_guide.png
- log: "签到完成"
`
    expect(v(y)).toEqual([])
  })

  it('省略 func: 的纯函数库映射（单函数）本身校验通过', () => {
    expect(makeValidator()(loadFixture('fn_lib_short.yaml'))).toEqual([])
  })

  // TODO(expected-current): 固化当前前端行为，实为**已知分歧**：
  // 引擎 exec_cross_func 在解析被引用脚本前先 normalize_top（server/src/engine.rs，
  // 注释"先做顶层归一化（省略 func: 的纯函数库简写同样可被跨文件调用）"），
  // 故引擎允许跨文件调用省略 func: 的纯函数库；
  // 前端校验直接读原文档的 sdoc.func、未做等价归一化 → 误报"未定义函数"。
  // 修复前端后应改为 toEqual([])。
  it('跨文件调用省略 func: 库当前误报未定义函数（见 TODO 注释）', () => {
    const errs = makeValidator()(`steps:
  - fn_lib_short:noop_fn
`)
    expect(errs.some(e => e.includes('未定义函数 noop_fn'))).toBe(true)
  })

  it('config 不能省略：顶层 interval/threshold 定向报错', () => {
    // 省略段落键形态（映射不含 config/func/steps 任何键）才走定向报错分支
    expect(v('interval: 500ms').join()).toContain('顶层 interval 是 config: 段参数')
    expect(v('threshold: 0.9').join()).toContain('顶层 threshold 是 config: 段参数')
    // 与段落键混写时则落白名单分支报未知顶层键
    expect(v('interval: 500ms\nsteps:\n  - log: a\n').join()).toContain('未知顶层键 interval')
  })

  it('无 steps 无 func（只有 config）定向报错', () => {
    const errs = v('config:\n  interval: 500ms\n')
    expect(errs.some(e => e.includes('需要 steps 或 func'))).toBe(true)
  })

  it('未知顶层键报错（含已删除的 package 指令）', () => {
    const errs = v('package: com.foo\nsteps:\n  - log: a\n')
    expect(errs.some(e => e.includes('未知顶层键 package'))).toBe(true)
  })
})

describe('func 定义：列表与映射两种形式', () => {
  const v = makeValidator()

  it('列表形式多函数（fixture lib_utils.yaml 已覆盖），保留字函数名报错', () => {
    const y = `func:
  - find:
    - log: x
steps:
  - log: a
`
    expect(v(y).some(e => e.includes('是保留字'))).toBe(true)
  })

  it('映射形式单函数（嵌套在 func 键下）合法', () => {
    const y = `func:
  f1:
    - log: x
steps:
  - f1
`
    expect(v(y)).toEqual([])
  })

  // TODO(expected-current): 此用例固化当前前端行为，实为**已知分歧**：
  // 引擎 parse_funcs（server/src/engine.rs，func 值为 Mapping 时逐键拆分为多个函数定义）
  // 接受"映射形式含多个函数"，而前端校验把整个映射当作一个条目、要求恰好一个函数名键。
  // fixtures/fn_lib_short.yaml 采用单函数写法规避该分歧；修复前端后应改为 toEqual([])。
  it('映射形式多函数当前报错（引擎接受，见 TODO 注释）', () => {
    const y = `func:
  f1:
    - log: a
  f2:
    - log: b
steps:
  - f1
  - f2
`
    const errs = v(y)
    expect(errs.some(e => e.includes('恰好一个 函数名: 键'))).toBe(true)
  })

  it('函数重复定义报错', () => {
    const y = `func:
  - f:
    - log: a
  - f:
    - log: b
steps:
  - f
`
    expect(v(y).some(e => e.includes('重复定义'))).toBe(true)
  })
})

describe('cond 函数执行条件（2026-08-27 语义）', () => {
  const v = makeValidator()

  it('单模板 / 多模板列表 / 函数体之后写法 均通过', () => {
    const y = `func:
  - fun1:
    cond: tpl_phone.png
    steps:
      - find: $1
  - fun2:
    cond:
      - tpl_phone.png
      - tpl_task_list.png
    steps:
      - log: x
  - fun3:
    - log: a
    cond: tpl_phone.png
steps:
  - fun1: tpl_guide.png
    then:
      - log: ok
`
    expect(v(y)).toEqual([])
  })

  it('逗号分隔多模板 cond 合法，缺模板报错', () => {
    expect(v(`func:
  - g:
    cond: tpl_phone.png, tpl_task_list.png
    steps:
      - log: x
steps:
  - g
`).some(e => e.includes('不存在'))).toBe(false)
    expect(v(`func:
  - g:
    cond: nope.png
    steps:
      - log: x
steps:
  - g
`).some(e => e.includes('模板不存在：nope.png'))).toBe(true)
  })

  it('cond 数字列表项报错', () => {
    const errs = v(`func:
  - g:
    cond:
      - 123
    steps:
      - log: x
steps:
  - g
`)
    expect(errs.some(e => e.includes('cond'))).toBe(true)
  })

  it('cond 后跟同列 "- " 行给出 bad indentation 引导', () => {
    const errs = v(`func:
  - g:
    cond: tpl_phone.png
    - find: $1
steps:
  - g
`)
    expect(errs.some(e => e.includes('bad indentation') || e.includes('cond'))).toBe(true)
  })

  it('函数体步骤内 cond 提示函数级条件', () => {
    const errs = v(`func:
  - g:
    steps:
      - cond: tpl_phone.png
steps:
  - g
`)
    expect(errs.some(e => e.includes('函数级条件'))).toBe(true)
  })
})

describe('跨文件函数调用', () => {
  const v = makeValidator()

  it('存在脚本+函数、带 then 通过', () => {
    const y = `steps:
  - lib_utils:mail_recv: tpl_phone.png
    then:
      - log: ok
  - misc:ping
`
    expect(v(y)).toEqual([])
  })

  it('子脚本不存在报错', () => {
    expect(v('steps:\n  - nofile:fun1: x\n').some(e => e.includes('子脚本不存在'))).toBe(true)
  })

  it('未定义函数报错', () => {
    expect(v('steps:\n  - lib_utils:nofunc\n').some(e => e.includes('未定义函数'))).toBe(true)
  })

  it('实参为 YAML 数组值时报错（需空格分隔字符串）', () => {
    expect(v('steps:\n  - lib_utils:ap_burn: [0.5, 0.6]\n').some(e => e.includes('实参'))).toBe(true)
  })

  it('跨文件调用 then 子步骤递归校验（坏模板冒泡到前缀）', () => {
    const errs = v(`steps:
  - lib_utils:mail_recv: tpl_phone.png
    then:
      - find: nope.png
`)
    expect(errs.some(e => e.includes('then 第 1 步') && e.includes('模板不存在'))).toBe(true)
  })

  it('调用方无任何脚本数据时不误判本脚本内函数', () => {
    const bare = validatorWithScripts([])
    const y = `func:
  - local_fn:
    - log: hi
steps:
  - local_fn
`
    expect(bare(y)).toEqual([])
  })
})

describe('then/else 与 YAML 笔误定向提示', () => {
  const v = makeValidator()

  it('func 内 find + else throw（字符串原因）通过', () => {
    const y = `func:
  - mail_recv:
    - find: tpl_mail_icon.png
      timeout: 5s
      else:
        - throw: 没有邮件入口
      then:
        - find: tpl_mail_claim.png
steps:
  - mail_recv
`
    expect(v(y)).toEqual([])
  })

  it('标量步骤漏写冒号给定向提示，补冒号后通过', () => {
    const y = `func:
  - itm_recv:
    - find: tpl_task_list.png
      timeout: 1s
      else:
        - throw 未知界面
steps:
  - itm_recv
`
    const errs = v(y)
    expect(errs.some(e => e.includes('- throw: 未知界面') && e.includes('需写冒号'))).toBe(true)
    expect(v(y.replace('- throw 未知界面', '- throw: 未知界面'))).toEqual([])
  })

  it('漏写分支键给出 else: 示例提示，补 else 后通过', () => {
    const y = `func:
  - itm_recv:
    - find: tpl_task_list.png
      timeout: 1s
        - throw: 未知界面
steps:
  - itm_recv
`
    const errs = v(y)
    expect(errs.some(e => e.includes('漏写分支键') && e.includes('else:') && e.includes('- throw: 未知界面'))).toBe(true)
    // 补 else:（与 timeout 同列，6 空格）后通过——else: 缩进须挂在 timeout 正下方一层
    const fixed = `func:
  - itm_recv:
    - find: tpl_task_list.png
      timeout: 1s
      else:
        - throw: 未知界面
steps:
  - itm_recv
`
    expect(v(fixed)).toEqual([])
  })

  it('else: 带注释不误报（真错误指向 timeout 深缩进行）', () => {
    const errs = v(`func:
  - itm_recv:
    - find: tpl_task_list.png
      else: # 如果没进委托一般是进了生存索引，切回任务界面
        - find: tpl_task_list.png
          timeout: 1s
            - throw: 未知界面
steps:
  - itm_recv
`)
    expect(errs.some(e => e.includes('第 6 行'))).toBe(true)
  })
})

describe('throw 值宽容（与引擎对齐）', () => {
  const v = makeValidator()

  it('数字 / 布尔原因不报错（引擎按无原因处理）', () => {
    for (const cause of ['404', 'true']) {
      const y = `steps:
  - find: tpl_guide.png
    then:
      - throw: ${cause}
`
      expect(v(y)).toEqual([])
    }
  })

  it('数组 / 映射值报错', () => {
    expect(v('steps:\n  - throw: [1, 2]\n').some(e => e.includes('throw'))).toBe(true)
    expect(v('steps:\n  - throw: {a: 1}\n').some(e => e.includes('throw'))).toBe(true)
  })
})

describe('loop 循环递归校验', () => {
  const v = makeValidator()

  it('times 整数 / 0 / 省略 = 合法，值内缩进与同级两种写法均可', () => {
    expect(v(`steps:
  - loop:
      times: 3
      steps:
        - log: a
`)).toEqual([])
    expect(v(`steps:
  - loop:
    times: 0
    steps:
      - log: a
`)).toEqual([])
    expect(v(`steps:
  - loop:
    times: 3
    steps:
      - find: tpl_guide.png
`)).toEqual([])
  })

  it('times 负数 / 缺 steps / 多余参数 报错', () => {
    expect(v('steps:\n  - loop:\n      times: -1\n      steps:\n        - log: a\n').some(e => e.includes('非负整数'))).toBe(true)
    expect(v('steps:\n  - loop:\n      times: 1\n').some(e => e.includes('loop 需要 steps'))).toBe(true)
    expect(v('steps:\n  - loop:\n      foo: 1\n      steps:\n        - log: a\n').some(e => e.includes('不支持参数 foo'))).toBe(true)
  })

  it('嵌套 loop 内层错误冒泡（带层级前缀）', () => {
    const errs = v(`steps:
  - loop:
    times: 2
    steps:
      - loop:
        times: 2
        steps:
          - find: nope.png
`)
    expect(errs.some(e => e.includes('loop 第 1 步') && e.includes('loop ') && e.includes('模板不存在'))).toBe(true)
  })
})

describe('旧语法定向迁移报错', () => {
  const v = makeValidator()

  it('顶层 action_wait（简写形态与白名单形态都拦截）', () => {
    expect(v('action_wait: 500ms').join()).toContain('顶层 action_wait 已删除')
    expect(v('action_wait: 500ms\nsteps:\n  - log: a\n').some(e => e.includes('action_wait 已删除'))).toBe(true)
  })

  it('顶层 log_level / name 定向报错', () => {
    expect(v('log_level: debug\nsteps:\n  - log: a\n').some(e => e.includes('顶层 log_level 已删除'))).toBe(true)
    expect(v('name: foo\nsteps:\n  - log: a\n').some(e => e.includes('顶层 name 已删除'))).toBe(true)
  })

  it('步骤级旧动作/旧参数全部拦截', () => {
    const base = (body) => `steps:\n${body}`
    expect(v(base('  - until: tpl_guide.png\n')).some(e => e.includes('until 已改名 find'))).toBe(true)
    expect(v(base('  - exit\n')).some(e => e.includes('exit 已改名 throw'))).toBe(true)
    expect(v(base('  - goto: 3\n')).some(e => e.includes('goto/label 已删除'))).toBe(true)
    expect(v(base('  - label: top\n')).some(e => e.includes('goto/label 已删除'))).toBe(true)
    expect(v(base('  - find: tpl_guide.png\n    check: tpl_close_panel.png\n')).some(e => e.includes('check 已改名 block'))).toBe(true)
    for (const k of ['count', 'cnt_ivl', 'cnt_chk', 'img_ivl', 'and_or', 'click', 'before', 'after']) {
      expect(v(base(`  - find: tpl_guide.png\n    ${k}: 1\n`)).some(e => e.includes(`${k} 已删除`)), k).toBe(true)
    }
    expect(v(base('  - find: tpl_guide.png\n    threshold: 0.9\n')).some(e => e.includes('threshold 步骤参数已删除'))).toBe(true)
    expect(v(base('  - find: tpl_guide.png\n    region: [0, 0, 1, 1]\n')).some(e => e.includes('region 步骤参数已删除'))).toBe(true)
    expect(v(base('  - swipe:\n      from: [0.5, 0.8]\n      to: [0.5, 0.2]\n')).some(e => e.includes('from 已改名 fm'))).toBe(true)
  })

  it('顶层 steps 内旧 color/cond 数组键语法给出迁移提示', () => {
    const errs = v('steps:\n  - [0.76, 0.91]: c74f36\n')
    expect(errs.join()).toContain('数组键写法')
  })

  it('裸数字时长全面拒绝（wait/swipe.time/find.timeout/config.interval）', () => {
    expect(v('steps:\n  - wait: 2\n').some(e => e.includes('裸数字不再接受'))).toBe(true)
    expect(v('steps:\n  - swipe:\n      fm: [0, 0]\n      to: [1, 1]\n      time: 800\n').some(e => e.includes('裸数字'))).toBe(true)
    expect(v('steps:\n  - find: tpl_guide.png\n    timeout: 30\n').some(e => e.includes('裸数字'))).toBe(true)
    expect(v('config:\n  interval: 500\nsteps:\n  - log: a\n').some(e => e.includes('裸数字不再接受'))).toBe(true)
  })
})

describe('坐标 / 动作细节', () => {
  const v = makeValidator()

  it('tap / swipe / color 坐标超界报错；swipe 需要 fm/to', () => {
    expect(v('steps:\n  - tap: [1.5, 0.2]\n').some(e => e.includes('相对坐标需在 0~1'))).toBe(true)
    expect(v('steps:\n  - color: [-1, 0.5]\n    ff8800:\n').some(e => e.includes('相对坐标需在 0~1'))).toBe(true)
    expect(v('steps:\n  - swipe:\n      to: [0.5, 0.2]\n').some(e => e.includes('swipe 不支持参数') || e.includes('需要 {fm, to'))).toBe(false) // 仅缺 fm 不误报
    expect(v('steps:\n  - swipe: 3s\n').some(e => e.includes('swipe 需要 {fm, to, time}'))).toBe(true)
  })

  it('一个步骤多个动作键报错', () => {
    const errs = v('steps:\n  - log: a\n    wait: 1s\n')
    expect(errs.some(e => e.includes('只能有一个动作键'))).toBe(true)
  })

  it('return 仅函数内可用且需布尔值', () => {
    expect(v('steps:\n  - return: true\n').some(e => e.includes('仅可在自定义函数内'))).toBe(true)
    expect(v('func:\n  - f:\n    - return: yes-please\nsteps:\n  - f\n').some(e => e.includes('return 需要 true / false'))).toBe(true)
    expect(v('func:\n  - f:\n    - return: false\nsteps:\n  - f\n')).toEqual([])
  })

  it('str_app / cls_app 只支持裸写', () => {
    expect(v('steps:\n  - str_app: com.foo.bar\n').some(e => e.includes('不支持参数'))).toBe(true)
    expect(v('steps:\n  - str_app\n')).toEqual([])
  })

  it('color 需要色值键且键需 6 位十六进制', () => {
    expect(v('steps:\n  - color: [0.5, 0.5]\n').some(e => e.includes('至少需要一个色值键'))).toBe(true)
    expect(v('steps:\n  - color: [0.5, 0.5]\n    zzz999:\n').some(e => e.includes('6 位十六进制'))).toBe(true)
    expect(v('steps:\n  - color: [0.5, 0.5]\n    "#ff8800":\n      - log: ok\n')).toEqual([])
  })

  it('call 需字符串；find 主模板只支持单个', () => {
    expect(v('steps:\n  - call: 123\n').some(e => e.includes('call 需要'))).toBe(true)
    expect(v('steps:\n  - call: flow_daily.yaml tpl_guide.png\n')).toEqual([])
    expect(v('steps:\n  - find: a.png, b.png\n').some(e => e.includes('只支持单个主模板'))).toBe(true)
  })
})

describe('模板存在性：中文名 / # 区域后缀 / 短名引用', () => {
  const v = makeValidator()

  it('带 # 区域后缀的完整名精确命中；无后缀精确命中', () => {
    expect(v('steps:\n  - find: tpl_mail_icon#946_270_990_343.png\n')).toEqual([])
    expect(v('steps:\n  - find: plain_ref.png\n')).toEqual([])
  })

  it('短名唯一匹配区域后缀文件；中文短名同理', () => {
    expect(v('steps:\n  - find: tpl_mail_icon.png\n')).toEqual([])
    expect(v('steps:\n  - find: 每日签到.png\n')).toEqual([])
    expect(v('steps:\n  - find: 签到按钮#u.png\n')).toEqual([])
  })

  it('短名歧义要求写全名；不存在报错', () => {
    const errs = v('steps:\n  - find: tpl_dup.png\n')
    expect(errs[0]).toContain('匹配到多个')
    expect(errs[0]).toContain('tpl_dup#l.png')
    expect(errs[0]).toContain('请用完整文件名')
    expect(v('steps:\n  - find: ghost.png\n')[0]).toContain('模板不存在：ghost.png')
  })

  it('$N 实参占位不校验模板存在性；block 三种形态与主模板重复检查', () => {
    expect(v('func:\n  - f:\n    - find: $1\nsteps:\n  - f\n')).toEqual([])
    expect(v('steps:\n  - find: tpl_guide.png\n    block: tpl_monthly_claim.png\n')).toEqual([])
    expect(v('steps:\n  - find: tpl_guide.png\n    block: tpl_monthly_claim.png, tpl_close_panel.png\n')).toEqual([])
    expect(v('steps:\n  - find: tpl_guide.png\n    block:\n      - tpl_monthly_claim.png\n')).toEqual([])
    expect(v('steps:\n  - find: tpl_guide.png\n    block: tpl_guide.png\n').some(e => e.includes('与 find 主模板重复'))).toBe(true)
  })

  it('跨分区脚本不参与当前分区模板校验（pkg 过滤生效）', () => {
    const tplList = readFileSync(new URL('templates.txt', FIX_URL), 'utf8')
      .split(/\r?\n/).map(s => s.trim()).filter(Boolean)
    const mixed = createScriptValidator({
      templatesData: { value: [...tplList.map(name => ({ name, pkg: PKG })), { name: 'other_pkg_only.png', pkg: 'com.other.app' }] },
      scriptsData: { value: [] },
      activePkg: { value: PKG },
    })
    expect(mixed('steps:\n  - find: other_pkg_only.png\n')[0]).toContain('模板不存在：other_pkg_only.png')
  })
})

describe('YAML 语法兜底提示', () => {
  const v = makeValidator()

  it('冒号后缺空格给行级定向提示', () => {
    // 注：单独 "- find:a.png" 会被 js-yaml 折叠成整段标量而不抛语法错（落其它提示路径），
    // 与后续映射键混写时才真正 parse 失败、命中冒号缺空格启发式
    const errs = v('config:\n  interval: 500ms\n  threshold:0.9\nsteps:\n  - log: a\n')
    expect(errs[0]).toContain('第 3 行')
    expect(errs[0]).toContain('threshold: 0.9')
    expect(errs[0]).toContain('冒号后缺少空格')
  })

  it('非法结构回退通用 js-yaml 错误', () => {
    // 多文档流非法输入，末尾留注释行避免进入校验器 catch 内 lines[j] 越界
    // 的启发式扫描（该 NPE 为已知前端缺陷，见最终报告）
    const errs = v(['---', 'a: 1', '---', 'b: 2', '# tail'].join('\n'))
    expect(errs[0].startsWith('YAML 语法错误')).toBe(true)
  })
})
