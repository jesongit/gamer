import { describe, expect, it } from 'vitest'
import { schemaToParamDecls } from '../entrypointParams'

describe('V1 entrypoint params adapter', () => {
  it('maps descriptor schema array to ParamDecl[]', () => {
    const decls = schemaToParamDecls([
      { name: 'msg', type: 'string', required: false, default: '默认', desc: '消息' },
      { name: 'count', type: 'int', required: true, default: null, desc: '' },
      { name: 'wait', type: 'duration', required: false, default: '2s', desc: '' },
      { name: 'at', type: 'point', required: false, default: [0.5, 0.5], desc: '' },
      { name: 'key', type: 'key', required: false, default: 'BACK', desc: '' },
      { name: 'tpl', type: 'template', required: false, default: '', desc: '' },
      { name: 'flag', type: 'bool', required: false, default: false, desc: '' },
    ])
    expect(decls).toHaveLength(7)
    expect(decls[0]).toEqual({ name: 'msg', type: 'string', required: false, default: '默认', desc: '消息' })
    // 别名归一：int→integer、bool→boolean
    expect(decls[1].type).toBe('integer')
    expect(decls[1].required).toBe(true)
    expect(decls[6].type).toBe('boolean')
    expect(decls[3].default).toEqual([0.5, 0.5])
  })

  it('required without default stays required; default clears required', () => {
    const decls = schemaToParamDecls([
      { name: 'a', type: 'string', required: true },
      { name: 'b', type: 'string', required: true, default: 'x' },
    ])
    expect(decls[0].required).toBe(true)
    expect(decls[1].required).toBe(false)
  })

  it('unknown/missing shapes degrade to empty or string', () => {
    expect(schemaToParamDecls(null)).toEqual([])
    expect(schemaToParamDecls(undefined)).toEqual([])
    expect(schemaToParamDecls([{ name: 'x', type: 'wat' }])[0].type).toBe('string')
    expect(schemaToParamDecls([{ type: 'string' }])).toEqual([])
  })
})
