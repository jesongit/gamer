// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ParamsForm from '../components/ParamsForm.vue'
import {
  describeResolvedArgs, extractParams, fmtLiteral, loadRunArgsSuggestion,
  mapArgDiagnostics, runArgsCacheKey, saveRunArgsSuggestion, validateArgsAgainstParams,
} from '../params'

/**
 * 阶段 5 参数表单：extractParams（脚本/函数库）、七类控件渲染、三态
 * （使用默认值不进 args / 显式覆盖进 args / 必填缺失阻断）、校验（与服务端同规则）、
 * 覆盖建议缓存只作覆盖态预填（不遮蔽当前声明默认值）、400 诊断字段映射、resolved_args 摘要。
 */

const SCRIPT_YAML = [
  'params:',
  "  - 'tmpl:account:账号模板'",
  "  - 'coord:pos:位置:[0.5, 0.8]'",
  "  - 'color:target:目标颜色:ff8800'",
  "  - 'time:timeout:最长等待:30s'",
  "  - 'key:quit:退出按键:ESC'",
  "  - 'text:message:提示文本:\"开始\"'",
  "  - 'bool:enable:开关:true'",
  'steps:',
  '  - tap: $pos',
].join('\n')

const FN_YAML = [
  'login:',
  '  params:',
  "    - 'tmpl:account:账号模板:account.png'",
  "    - 'time:timeout:等待时间:30s'",
  '  steps:',
  '    - find: $account',
  '      timeout: $timeout',
  '    - return: true',
  '',
  'is_enabled:',
  '  params:',
  "    - 'bool:enable:开关'",
  '  steps:',
  '    - return: $enable',
].join('\n')

function mountForm(props = {}) {
  return mount(ParamsForm, {
    props: { params: extractParams(SCRIPT_YAML), ...props },
  })
}

const row = (wrapper, name) =>
  wrapper.findAll('.pf-row').find(r => r.text().includes(`$${name}`))

describe('extractParams', () => {
  it('脚本取文件级 params；函数库按函数名取（缺省第一个）；缺失/解析失败为空', () => {
    expect(extractParams(SCRIPT_YAML).map(p => p.name))
      .toEqual(['account', 'pos', 'target', 'timeout', 'quit', 'message', 'enable'])
    expect(extractParams(FN_YAML, 'function_library').map(p => p.name)).toEqual(['account', 'timeout'])
    expect(extractParams(FN_YAML, 'function_library', 'login').map(p => p.name)).toEqual(['account', 'timeout'])
    expect(extractParams(FN_YAML, 'function_library', 'is_enabled').map(p => p.name)).toEqual(['enable'])
    expect(extractParams(FN_YAML, 'function_library', 'nope')).toEqual([])
    expect(extractParams('- a: 1\n- b: 2', 'function_library')).toEqual([]) // 旧语法/解析失败 → 空
  })
})

describe('ParamsForm 三态', () => {
  it('七类类型化控件渲染（复用 CellEditor）', () => {
    const w = mountForm({ initialArgs: { account: 'a.png', pos: [0.1, 0.2], target: '112233', timeout: '5s', quit: 'BACK', message: 'hi', enable: false } })
    expect(w.find('.tmpl-wrap input.cell-input').exists()).toBe(true) // tmpl（自定义下拉 + ▾）
    expect(w.find('.tmpl-wrap .tpl-toggle').exists()).toBe(true)       // 悬停缩略图下拉开关
    expect(w.findAll('input[type="number"]')).toHaveLength(3)           // coord X/Y + time 数值
    expect(w.find('input[type="color"]').exists()).toBe(true)           // color
    expect(w.find('select.cell-select.unit').exists()).toBe(true)       // time 单位
    expect(w.findAll('select').some(s => s.findAll('option').some(o => o.text() === 'ESC'))).toBe(true) // key 枚举
    expect(w.find('input[type="checkbox"]').exists()).toBe(true)        // bool
    expect(w.find('input.cell-input:not(.num):not(.hex)').exists()).toBe(true) // text/tmpl 文本
  })

  it('有默认值字段初始为「使用默认值」：显示当前声明默认值；args 只含必填字段', () => {
    const w = mountForm()
    expect(row(w, 'pos').text()).toContain('默认: [0.5, 0.8]')
    expect(row(w, 'enable').text()).toContain('默认: true')
    const form = w.findComponent(ParamsForm)
    expect(form.vm.getArgs()).toEqual({ account: '' }) // 必填恒在；默认值字段省略
  })

  it('无默认值字段（必填）恒为覆盖态并参与校验；account 为空模板名阻断提交', async () => {
    const w = mountForm()
    const form = w.findComponent(ParamsForm)
    const errs = form.vm.validate()
    expect(errs.map(e => e.name)).toEqual(['account']) // 其余都有默认值或已合法
    expect(errs[0].message).toContain('模板短名')
    await w.vm.$nextTick()
    expect(row(w, 'account').classes()).toContain('pf-row-error')
  })

  it('勾选「覆盖」进 args（预填当前声明默认值），取消勾选移出 args', async () => {
    const w = mountForm()
    const form = w.findComponent(ParamsForm)
    const enableBox = row(w, 'enable').find('input[type="checkbox"]')
    await enableBox.setValue(true)
    expect(form.vm.getArgs()).toEqual({ enable: true, account: '' })
    await row(w, 'enable').find('input[type="checkbox"]').setValue(false)
    expect(form.vm.getArgs().enable).toBeUndefined()
  })
  it('initialArgs（任务快照/重编辑）直接激活覆盖态', () => {
    const w = mountForm({ initialArgs: { pos: [0.25, 0.75], timeout: '10s' } })
    const args = w.findComponent(ParamsForm).vm.getArgs()
    expect(args).toEqual({ account: '', pos: [0.25, 0.75], timeout: '10s' })
    expect(row(w, 'pos').text()).not.toContain('默认:')
  })

  it('客户端校验与服务端同规则：颜色格式/坐标越界/时间缺单位/未知按键', async () => {
    const w = mountForm({
      initialArgs: { target: 'xyz', quit: 'BACK' },
    })
    const form = w.findComponent(ParamsForm)
    expect(form.vm.validate().map(e => e.name)).toEqual(['account', 'target'])
    // 坐标越界：编辑 X 后越界
    await row(w, 'pos').find('input[type="checkbox"]').setValue(true)
    const xs = row(w, 'pos').findAll('input[type="number"]')
    await xs[0].setValue('1.5')
    const errs = form.vm.validate()
    expect(errs.find(e => e.name === 'pos').message).toContain('0~1')
  })

  it('change 事件回传稀疏 args + 完整采用值视图', async () => {
    const w = mountForm()
    await row(w, 'enable').find('input[type="checkbox"]').setValue(true)
    const evt = w.emitted('change').at(-1)[0]
    expect(evt.args).toEqual({ account: '', enable: true })
    expect(evt.effective).toEqual({
      account: '', pos: [0.5, 0.8], target: 'ff8800', timeout: '30s',
      quit: 'ESC', message: '开始', enable: true,
    })
  })

  it('serverErrors（400 诊断字段映射）与客户端错误共同标红', () => {
    const w = mountForm({ serverErrors: { pos: ['参数 pos 越界'] } })
    expect(row(w, 'pos').classes()).toContain('pf-row-error')
    expect(row(w, 'pos').text()).toContain('参数 pos 越界')
  })
})

describe('覆盖建议缓存：只作覆盖态预填，不遮蔽当前声明默认值', () => {
  const store = () => {
    const m = new Map()
    return {
      getItem: k => (m.has(k) ? m.get(k) : null),
      setItem: (k, v) => m.set(k, v),
      removeItem: k => m.delete(k),
    }
  }

  it('缓存读写往返 + 按 id 隔离 + 损坏数据兜底', () => {
    const s = store()
    saveRunArgsSuggestion('com.a/a.yaml', { timeout: '10s' }, s)
    expect(loadRunArgsSuggestion('com.a/a.yaml', s)).toEqual({ timeout: '10s' })
    expect(loadRunArgsSuggestion('com.a/b.yaml', s)).toEqual({})
    s.setItem(runArgsCacheKey('bad'), '{oops')
    expect(loadRunArgsSuggestion('bad', s)).toEqual({})
    s.setItem(runArgsCacheKey('arr'), '[1,2]')
    expect(loadRunArgsSuggestion('arr', s)).toEqual({})
    expect(loadRunArgsSuggestion('', s)).toEqual({})
  })

  it('默认值字段始终显示当前声明默认值（建议/旧缓存不遮蔽）；仅覆盖态预填建议值', async () => {
    const w = mountForm({
      suggestions: { pos: [0.2, 0.3], timeout: '10s', ghost: 'x' },
    })
    const form = w.findComponent(ParamsForm)
    // 未覆盖：展示声明默认值（不是建议值）
    expect(row(w, 'pos').text()).toContain('默认: [0.5, 0.8]')
    expect(row(w, 'timeout').text()).toContain('默认: 30s')
    // 切覆盖：预填建议值；建议里的未知参数（ghost）不进 args
    await row(w, 'pos').find('input[type="checkbox"]').setValue(true)
    await row(w, 'timeout').find('input[type="checkbox"]').setValue(true)
    const args = form.vm.getArgs()
    expect(args.pos).toEqual([0.2, 0.3])
    expect(args.timeout).toBe('10s')
    expect(args.ghost).toBeUndefined()
  })
})

describe('mapArgDiagnostics / describeResolvedArgs / validateArgsAgainstParams', () => {
  const decls = extractParams(SCRIPT_YAML)
  const names = decls.map(p => p.name)

  it('field 命中参数名；step_path args.x 兜底；对不上进 other', () => {
    const m = mapArgDiagnostics([
      { code: 'param.args.type_mismatch', message: 'timeout 应为带单位时间', field: 'timeout' },
      { code: 'param.args.missing_required', message: '缺少必填参数 account', step_path: 'args.account' },
      { code: 'param.args.unknown', message: '未知参数 ghost', field: 'ghost', step_path: 'args.ghost' },
    ], names)
    expect(m.byName).toEqual({
      timeout: ['timeout 应为带单位时间'],
      account: ['缺少必填参数 account'],
    })
    expect(m.other).toEqual(['未知参数 ghost'])
  })

  it('resolved_args 摘要带「覆盖/默认/必填」来源标注；无参数返回空；超长截断', () => {
    const text = describeResolvedArgs(decls, { pos: [0.1, 0.1] }, {
      account: 'a.png', pos: [0.1, 0.1], target: 'ff8800', timeout: '30s',
      quit: 'ESC', message: '开始', enable: true,
    })
    expect(text).toContain('pos=[0.1, 0.1]（覆盖）')
    expect(text).toContain('timeout=30s（默认）')
    expect(text).toContain('account=a.png（必填）')
    expect(text.startsWith('运行参数：')).toBe(true)
    expect(describeResolvedArgs([], {}, {})).toBe('')
    expect(describeResolvedArgs(decls, {}, null).length).toBeLessThanOrEqual(241)
  })

  it('validateArgsAgainstParams：缺必填 missing、类型不符报字段', () => {
    expect(validateArgsAgainstParams(decls, {}).map(e => e.name)).toEqual(['account'])
    expect(validateArgsAgainstParams(decls, { account: 'a.png', timeout: '30' }).map(e => e.name)).toEqual(['timeout'])
    expect(validateArgsAgainstParams(decls, { account: 'a.png', timeout: '30s', extra: 1 })).toEqual([])
  })

  it('fmtLiteral 展示形态', () => {
    expect(fmtLiteral([0.5, 0.8])).toBe('[0.5, 0.8]')
    expect(fmtLiteral(true)).toBe('true')
    expect(fmtLiteral(null)).toBe('—')
    expect(fmtLiteral('abc')).toBe('abc')
  })
})
