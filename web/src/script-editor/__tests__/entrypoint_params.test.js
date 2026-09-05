import { describe, expect, it } from 'vitest'
import { schemaParamType, schemaToParamDecls } from '../entrypointParams'

/**
 * P12.3 服务端参数 schema descriptor → ParamDecl[] 适配（契约 §7）：
 * param_type 透传与规范化（boolean→bool、template→tmpl）、param_type 缺失时
 * JSON Schema type 兜底、required（default 键缺省/schema.required）、
 * description→remark、enum（key 型）、默认值形态清洗、键序保持。
 */

const prop = (extra = {}) => ({ type: 'string', ...extra })

describe('schemaParamType：param_type 透传与规范化', () => {
  it('param_type 缺失时按 JSON Schema type 兜底；未知一律 string', () => {
    expect(schemaParamType(prop({ param_type: 'tmpl' }))).toBe('tmpl')
    expect(schemaParamType(prop({ param_type: ' time ' }))).toBe('time')
    expect(schemaParamType(prop({ param_type: 'boolean' }))).toBe('bool') // 规范化：校验/控件键是 bool
    expect(schemaParamType(prop({ param_type: 'template' }))).toBe('tmpl')
    expect(schemaParamType(prop())).toBe('string')
    expect(schemaParamType(prop({ type: 'boolean' }))).toBe('bool')
    expect(schemaParamType(prop({ type: 'integer' }))).toBe('integer')
    expect(schemaParamType(prop({ type: 'number' }))).toBe('number')
    expect(schemaParamType(prop({ type: 'array', items: { type: 'number', minItems: 2, maxItems: 2 } }))).toBe('coord')
    expect(schemaParamType(prop({ type: 'any' }))).toBe('string')
  })
})

describe('schemaToParamDecls：契约 §7 descriptor → ParamsForm 声明', () => {
  it('全类型样本：param_type/enum/description/default 按规则映射且保持键序', () => {
    const decls = schemaToParamDecls({
      type: 'object',
      properties: {
        count: { type: 'integer', default: 3, param_type: 'int' },
        msg: { type: 'string', default: '默认', description: '消息', param_type: 'text' },
        wait: { type: 'string', default: '2s', param_type: 'time' },
        key: { type: 'string', enum: ['HOME', 'BACK'], param_type: 'key' },
        pos: { type: 'array', items: { type: 'number', minItems: 2, maxItems: 2 }, default: [0.5, 0.5], param_type: 'coord' },
        account: { type: 'string', param_type: 'tmpl' },
      },
      required: ['account'],
    })
    expect(decls.map((d) => d.name)).toEqual(['count', 'msg', 'wait', 'key', 'pos', 'account'])
    expect(decls.map((d) => d.type)).toEqual(['int', 'text', 'time', 'key', 'coord', 'tmpl'])
    expect(decls.map((d) => d.default)).toEqual([3, '默认', '2s', null, [0.5, 0.5], null])
    expect(decls.find((d) => d.name === 'msg').remark).toBe('消息')
    expect(decls.find((d) => d.name === 'wait').remark).toBe('')
    // enum 仅 key 型产出（服务端 KEY_NAMES ⊆ 前端 KEY_ENUM），由既有 key 下拉承载，
    // 声明形态不携带额外字段
  })

  it('required：default 键缺省或列入 schema.required 均为必填（default=null）', () => {
    const decls = schemaToParamDecls({
      properties: {
        a: { type: 'string' }, // default 键缺省 → 必填
        b: { type: 'string', default: 'x' }, // 有默认
        c: { type: 'string', default: 'y', param_type: 'text' }, // 同时列 required → 必填优先
      },
      required: ['c'],
    })
    expect(decls.map((d) => d.default)).toEqual([null, 'x', null])
  })

  it('默认值形态清洗：非标量/非 [x,y]（对象、错形态数组、null default）→ 无默认', () => {
    const decls = schemaToParamDecls({
      properties: {
        bad_obj: { type: 'string', default: { nested: 1 }, param_type: 'text' },
        bad_arr: { type: 'array', default: [1, 2, 3], param_type: 'coord' },
        null_default: { type: 'string', default: null },
        ok_time_num: { type: 'number', default: 500, param_type: 'time' },
      },
    })
    expect(decls.map((d) => d.default)).toEqual([null, null, null, 500])
  })

  it('schema 缺失/形态不符 → []；非对象 property 跳过', () => {
    expect(schemaToParamDecls(null)).toEqual([])
    expect(schemaToParamDecls(undefined)).toEqual([])
    expect(schemaToParamDecls({})).toEqual([])
    expect(schemaToParamDecls({ properties: 'oops' })).toEqual([])
    expect(schemaToParamDecls({ properties: { ghost: null, ok: { type: 'string' } } }).map((d) => d.name))
      .toEqual(['ok'])
  })
})
