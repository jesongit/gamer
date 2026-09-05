// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import ParamsForm from '../components/ParamsForm.vue'
import {
  describeResolvedArgs, fmtLiteral, loadRunArgsSuggestion,
  mapArgDiagnostics, runArgsCacheKey, saveRunArgsSuggestion, validateArgsAgainstParams,
} from '../params'

/**
 * 参数表单（v3 / P12.3）：类型化控件渲染、三态（使用默认值不进 args / 显式覆盖
 * 进 args / 必填恒覆盖）、校验（schema.checkCellLiteral 同规则）、覆盖建议缓存
 * 只作覆盖态预填、400 诊断字段映射、resolved_args 摘要。
 * 参数声明自 P12.3 起来自服务端 entrypoint schema API（entrypointParams.ts 适配），
 * 此处直接以 ParamDecl[] 夹具驱动（不再从 YAML 解析）。
 */

const decl = (name, type, def = null, remark = '') => ({ type, name, remark, default: def, rawForm: false })

// 账号必填 + 五类默认值字段（类型覆盖 canonical 与 v2 别名两种形态）
const SCRIPT_DECLS = [
  decl('account', 'string', null, '账号模板'),
  decl('count', 'int', 3),
  decl('ratio', 'number', 0.5),
  decl('flag', 'bool', true),
  decl('enable', 'boolean', true),
  decl('mode', 'string', 'auto'),
]

function mountForm(props = {}) {
  return mount(ParamsForm, {
    props: { params: SCRIPT_DECLS, ...props },
  })
}

const row = (wrapper, name) =>
  wrapper.findAll('.pf-row').find(r => r.text().includes(`$${name}`))

describe('ParamsForm 三态', () => {
  it('canonical 类型控件渲染（number/boolean/text，复用 CellEditor）', async () => {
    const w = mountForm({ initialArgs: { ratio: 0.1, flag: false } })
    // ratio 覆盖态 → number 输入；flag 覆盖态 → 布尔下拉；account 必填 → 文本输入
    const ratioRow = row(w, 'ratio')
    expect(ratioRow.find('input[type="number"]').exists()).toBe(true)
    expect(row(w, 'flag').find('select.cell-select').exists()).toBe(true)
    expect(row(w, 'account').find('input.cell-input').exists()).toBe(true)
    await ratioRow.findAll('input[type="number"]')[0].setValue('0.9')
    expect(w.findComponent(ParamsForm).vm.getArgs().ratio).toBe(0.9)
  })

  it('有默认值字段初始为「使用默认值」：显示当前声明默认值；args 只含必填字段', () => {
    const w = mountForm()
    expect(row(w, 'count').text()).toContain('默认: 3')
    expect(row(w, 'flag').text()).toContain('默认: true')
    expect(row(w, 'mode').text()).toContain('默认: auto')
    const form = w.findComponent(ParamsForm)
    expect(form.vm.getArgs()).toEqual({ account: '' }) // 必填恒在；默认值字段省略
  })

  it('必填字段恒为覆盖态（初始类型空值，服务端校验必填）', () => {
    const w = mountForm()
    const form = w.findComponent(ParamsForm)
    expect(form.vm.getArgs().account).toBe('')
    expect(form.vm.validate()).toEqual([]) // 空字符串对 string 型合法（必填性由服务端判定）
  })

  it('勾选「覆盖」进 args（预填当前声明默认值），取消勾选移出 args', async () => {
    const w = mountForm()
    const form = w.findComponent(ParamsForm)
    await row(w, 'enable').find('input[type="checkbox"]').setValue(true)
    expect(form.vm.getArgs()).toEqual({ enable: true, account: '' })
    await row(w, 'enable').find('input[type="checkbox"]').setValue(false)
    expect(form.vm.getArgs().enable).toBeUndefined()
  })

  it('initialArgs（任务快照/重编辑）直接激活覆盖态', () => {
    const w = mountForm({ initialArgs: { ratio: 0.25, count: '7' } })
    const args = w.findComponent(ParamsForm).vm.getArgs()
    expect(args).toEqual({ account: '', ratio: 0.25, count: '7' })
    expect(row(w, 'ratio').text()).not.toContain('默认:')
  })

  it('客户端类型校验：integer 收到小数报字段错误', async () => {
    const w = mountForm({ initialArgs: { count: 2.5 } })
    const form = w.findComponent(ParamsForm)
    const errs = form.vm.validate()
    expect(errs.map(e => e.name)).toEqual(['count'])
    await w.vm.$nextTick()
    expect(row(w, 'count').classes()).toContain('pf-row-error')
  })

  it('change 事件回传稀疏 args + 完整采用值视图', async () => {
    const w = mountForm()
    await row(w, 'enable').find('input[type="checkbox"]').setValue(true)
    const evt = w.emitted('change').at(-1)[0]
    expect(evt.args).toEqual({ account: '', enable: true })
    expect(evt.effective).toEqual({
      account: '', count: 3, ratio: 0.5, flag: true, enable: true, mode: 'auto',
    })
  })

  it('serverErrors（400 诊断字段映射）与客户端错误共同标红', () => {
    const w = mountForm({ serverErrors: { ratio: ['参数 ratio 越界'] } })
    expect(row(w, 'ratio').classes()).toContain('pf-row-error')
    expect(row(w, 'ratio').text()).toContain('参数 ratio 越界')
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
    saveRunArgsSuggestion('com.a/a.yaml', { count: 9 }, s)
    expect(loadRunArgsSuggestion('com.a/a.yaml', s)).toEqual({ count: 9 })
    expect(loadRunArgsSuggestion('com.a/b.yaml', s)).toEqual({})
    s.setItem(runArgsCacheKey('bad'), '{oops')
    expect(loadRunArgsSuggestion('bad', s)).toEqual({})
    s.setItem(runArgsCacheKey('arr'), '[1,2]')
    expect(loadRunArgsSuggestion('arr', s)).toEqual({})
    expect(loadRunArgsSuggestion('', s)).toEqual({})
  })

  it('默认值字段始终显示当前声明默认值（建议/旧缓存不遮蔽）；仅覆盖态预填建议值', async () => {
    const w = mountForm({
      suggestions: { ratio: 0.2, count: 99, ghost: 'x' },
    })
    const form = w.findComponent(ParamsForm)
    // 未覆盖：展示声明默认值（不是建议值）
    expect(row(w, 'ratio').text()).toContain('默认: 0.5')
    expect(row(w, 'count').text()).toContain('默认: 3')
    // 切覆盖：预填建议值；建议里的未知参数（ghost）不进 args
    await row(w, 'ratio').find('input[type="checkbox"]').setValue(true)
    await row(w, 'count').find('input[type="checkbox"]').setValue(true)
    const args = form.vm.getArgs()
    expect(args.ratio).toBe(0.2)
    expect(args.count).toBe(99)
    expect(args.ghost).toBeUndefined()
  })
})

describe('mapArgDiagnostics / describeResolvedArgs / validateArgsAgainstParams', () => {
  const names = SCRIPT_DECLS.map(p => p.name)

  it('field 命中参数名；step_path args.x 兜底；对不上进 other', () => {
    const m = mapArgDiagnostics([
      { code: 'yaml.v3.call.args_type_mismatch', message: 'count 应为整数', field: 'count' },
      { code: 'yaml.v3.call.args_missing_required', message: '缺少必填参数 account', step_path: 'args.account' },
      { code: 'yaml.v3.call.args_unknown', message: '未知参数 ghost', field: 'ghost', step_path: 'args.ghost' },
    ], names)
    expect(m.byName).toEqual({
      count: ['count 应为整数'],
      account: ['缺少必填参数 account'],
    })
    expect(m.other).toEqual(['未知参数 ghost'])
  })

  it('resolved_args 摘要带「覆盖/默认/必填」来源标注；无参数返回空；超长截断', () => {
    const text = describeResolvedArgs(SCRIPT_DECLS, { ratio: 0.1 }, {
      account: 'a.png', count: 3, ratio: 0.1, flag: true, enable: true, mode: 'auto',
    })
    expect(text).toContain('ratio=0.1（覆盖）')
    expect(text).toContain('count=3（默认）')
    expect(text).toContain('account=a.png（必填）')
    expect(text.startsWith('运行参数：')).toBe(true)
    expect(describeResolvedArgs([], {}, {})).toBe('')
    expect(describeResolvedArgs(SCRIPT_DECLS, {}, null).length).toBeLessThanOrEqual(241)
  })

  it('validateArgsAgainstParams：类型不符报字段；未知参数不查', () => {
    expect(validateArgsAgainstParams(SCRIPT_DECLS, { account: 'a', count: 2 })).toEqual([])
    expect(validateArgsAgainstParams(SCRIPT_DECLS, { account: 'a', count: 2.5 }).map(e => e.name)).toEqual(['count'])
  })

  it('fmtLiteral 展示形态', () => {
    expect(fmtLiteral([0.5, 0.8])).toBe('[0.5, 0.8]')
    expect(fmtLiteral(true)).toBe('true')
    expect(fmtLiteral(3)).toBe('3')
    expect(fmtLiteral(null)).toBe('—')
    expect(fmtLiteral('abc')).toBe('abc')
  })
})
