import { describe, expect, it } from 'vitest'
import { parseScript, serialize } from './codec'
import { initializeArgsFromSchema } from './factories'
import { schemaToParamDecls } from './entrypointParams'
import { validateArgsAgainstParams } from './params'
import {
  checkLiteral,
  defaultLiteralForType,
  hasParamDefault,
  normalizeParamDecl,
  normalizeParamType,
  paramControlType,
} from './schema'

const nativeSchema = [
  ['tap', 'point', true],
  ['swipe', 'point', true],
  ['key', 'key', true],
  ['input_text', 'string', true],
  ['launch', 'string', false],
  ['stop_app', 'string', false],
  ['sleep', 'duration', true],
  ['log', 'any', true],
  ['find', 'template', true],
  ['wait_find', 'template', true],
  ['tap_template', 'template', true],
  ['wait_disappear', 'template', true],
  ['eq', 'any', true],
  ['ne', 'any', true],
  ['gt', 'number', true],
  ['ge', 'number', true],
  ['lt', 'number', true],
  ['le', 'number', true],
].map(([name, type, required]) => ({ name, type, required, default: null, desc: '' }))

describe('P3-SCHEMA：正式类型、默认值与初始化契约', () => {
  it('覆盖全部 18 个正式函数入口，并把类型映射到唯一控件模型', () => {
    const decls = schemaToParamDecls(nativeSchema)
    expect(decls).toHaveLength(18)
    expect(decls.map((decl) => decl.type)).toEqual(nativeSchema.map((decl) => decl.type))
    expect(decls.filter((decl) => decl.required)).toHaveLength(16)
    expect(paramControlType('bool')).toBe('bool')
    expect(paramControlType('int')).toBe('number')
    expect(paramControlType('float')).toBe('number')
    expect(paramControlType('text')).toBe('text')
    expect(paramControlType('list')).toBe('json')
    expect(paramControlType('object')).toBe('json')
    expect(paramControlType('duration')).toBe('time')
    expect(paramControlType('point')).toBe('coord')
    expect(paramControlType('template')).toBe('tmpl')
  })

  it('required/default 以存在性判断，false、0、空字符串和显式 null 不丢失', () => {
    const noDefault = normalizeParamDecl({ name: 'required_value', type: 'string', required: true })
    const falseDefault = normalizeParamDecl({ name: 'flag', type: 'boolean', required: true, default: false })
    const zeroDefault = normalizeParamDecl({ name: 'count', type: 'integer', required: true, default: 0 })
    const emptyDefault = normalizeParamDecl({ name: 'text', type: 'string', required: true, default: '' })
    const nullDefault = normalizeParamDecl({ name: 'value', type: 'any', required: true, default: null, hasDefault: true })

    expect(noDefault).toMatchObject({ required: true, default: null })
    expect(hasParamDefault(noDefault)).toBe(false)
    expect(falseDefault).toMatchObject({ required: false, default: false })
    expect(zeroDefault).toMatchObject({ required: false, default: 0 })
    expect(emptyDefault).toMatchObject({ required: false, default: '' })
    expect(nullDefault).toMatchObject({ required: false, default: null, hasDefault: true })
    expect([falseDefault, zeroDefault, emptyDefault, nullDefault].every(hasParamDefault)).toBe(true)
  })

  it('YAML 参数默认值 round-trip 保留 null、false、0、list/object 的真实类型', () => {
    const source = [
      'params:',
      '  empty:',
      '    type: string',
      '    default: ""',
      '  disabled:',
      '    type: boolean',
      '    default: false',
      '  retries:',
      '    type: integer',
      '    default: 0',
      '  maybe:',
      '    type: any',
      '    default: null',
      '  tags:',
      '    type: list',
      '    default: [a, b]',
      '  options:',
      '    type: object',
      '    default: {enabled: true}',
      'run: []',
      '',
    ].join('\n')
    const parsed = parseScript(source)
    expect(parsed.diagnostics).toEqual([])
    expect(parsed.model.params.map((decl) => decl.default)).toEqual([
      '', false, 0, null, ['a', 'b'], { enabled: true },
    ])
    expect(parsed.model.params[3]).toMatchObject({ hasDefault: true, required: false })
    const reparsed = parseScript(serialize(parsed.model))
    expect(reparsed.diagnostics).toEqual([])
    expect(reparsed.model.params).toEqual(parsed.model.params)
  })

  it('新增/切换初始化显示必填字段，默认值按真实类型复制且不共享引用', () => {
    const params = [
      { name: 'template', type: 'template', required: true, default: null },
      { name: 'flag', type: 'boolean', required: false, default: false },
      { name: 'count', type: 'integer', required: false, default: 0 },
      { name: 'tags', type: 'list', required: false, default: ['one'] },
      { name: 'options', type: 'object', required: false, default: { enabled: true } },
    ]
    const args = initializeArgsFromSchema(params)
    expect(args).toEqual({
      kind: 'map',
      entries: {
        template: { lit: null, missing: true },
        flag: { lit: false },
        count: { lit: 0 },
        tags: { lit: ['one'] },
        options: { lit: { enabled: true } },
      },
    })
    if (args.kind !== 'map') throw new Error('expected named args')
    args.entries.tags.lit.push('two')
    args.entries.options.lit.enabled = false
    expect(params[3].default).toEqual(['one'])
    expect(params[4].default).toEqual({ enabled: true })
  })

  it('类型别名大小写归一，合法 null/false/0/空字符串不靠 truthy 判断', () => {
    expect(normalizeParamType(' BOOL ')).toBe('boolean')
    expect(normalizeParamType('INT')).toBe('integer')
    expect(normalizeParamType('Float')).toBe('number')
    expect(checkLiteral('any', null)).toBeNull()
    expect(checkLiteral('boolean', false)).toBeNull()
    expect(checkLiteral('integer', 0)).toBeNull()
    expect(checkLiteral('string', '')).not.toBeNull()
    expect(checkLiteral('list', [])).toBeNull()
    expect(checkLiteral('object', {})).toBeNull()
    expect(checkLiteral('template', '')).not.toBeNull()
    expect(defaultLiteralForType('list')).toEqual([])
    expect(defaultLiteralForType('object')).toEqual({})
  })

  it('缺省的可选参数不被错误报告为必填，显式参数值保持 false/0/null', () => {
    const params = schemaToParamDecls([
      { name: 'optional', type: 'string', required: false, default: null },
      { name: 'required', type: 'any', required: true, default: null },
    ])
    expect(validateArgsAgainstParams(params, { required: false })).toEqual([])
    expect(validateArgsAgainstParams(params, { required: 0 })).toEqual([])
    expect(validateArgsAgainstParams(params, { required: null })).toEqual([])
    expect(validateArgsAgainstParams(params, {})).toEqual([
      { name: 'required', message: '必填参数 $required 缺失' },
    ])
  })
})
