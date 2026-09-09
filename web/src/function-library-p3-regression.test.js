// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { useFunctionLibrary } from './composables/useFunctionLibrary'
import {
  buildFunctionViews,
  functionFileCategory,
  isDefaultFunctionFile,
} from './console/function-list'
import { parseFunctionLibrary, serialize } from './script-editor/codec'

const source = `functions:
  login:
    description: 登录流程
    params:
      account:
        type: string
        required: true
      dry:
        type: boolean
        default: false
      payload:
        type: object
        default: { mode: safe, count: 0 }
    vars:
      region: [0.1, 0.2, 0.8, 0.9]
    returns: { ok: true }
    run:
      - log:
          message: $$literal
      - if: $dry
        then:
          - return: null
        else:
          - repeat: 0
            do: []
`

describe('P3-LIB：Package 函数库列表、寻址与参数 Schema', () => {
  it('Package 切换的迟到响应不污染当前函数库，函数名缺注记时仍从原文补全', async () => {
    const deferred = new Map()
    const api = {
      listFunctions: vi.fn((pkg) => new Promise(resolve => deferred.set(pkg, resolve))),
    }
    const library = useFunctionLibrary({ api })

    const oldRequest = library.refresh('content.old')
    const currentRequest = library.refresh('content.current')
    deferred.get('content.current')([{
      id: 'content.current/_function.yaml',
      pkg: 'content.current',
      file: '_function.yaml',
      content: source,
      version: 'v2',
      functions: [],
    }])
    await currentRequest
    deferred.get('content.old')([{ pkg: 'content.old', file: '_function.yaml' }])
    await oldRequest

    expect(library.list[0].pkg).toBe('content.current')
    expect(library.namesFor(library.list[0])).toEqual(['login'])
    expect(library.findByName('login').id).toBe('content.current/_function.yaml')
  })

  it('列表只有注记时按需读取一次完整文件，参数 Schema 可供正式入口消费', async () => {
    const api = {
      listFunctions: vi.fn(async () => [{
        id: 'pkg/_function_extra.yaml', pkg: 'pkg', file: '_function_extra.yaml',
        functions: ['login'], version: 'v1',
      }]),
      getFunction: vi.fn(async () => ({ id: 'pkg/_function_extra.yaml', file: '_function_extra.yaml', content: source, version: 'v2' })),
    }
    const library = useFunctionLibrary({ api })
    await library.refresh('pkg')

    const [first, second] = await Promise.all([
      library.loadFile('pkg/_function_extra.yaml'),
      library.loadFile('pkg/_function_extra.yaml'),
    ])
    expect(api.getFunction).toHaveBeenCalledTimes(1)
    expect(first.content).toBe(source)
    expect(second.content).toBe(source)
    expect(library.list[0].content).toBe(source)
  })
})

describe('P3-LIB：默认/额外函数库与 Raw/Visual 语义往返', () => {
  it('只把 _function.yaml 作为默认可编辑文件，额外 _function*.yaml 仍可展示', () => {
    expect(functionFileCategory('automations/_function.yaml')).toBe('_function')
    expect(functionFileCategory('_function_battle.yaml')).toBe('_function_battle')
    expect(isDefaultFunctionFile('_function.yaml')).toBe(true)
    expect(isDefaultFunctionFile('_function_battle.yaml')).toBe(false)

    const files = [{
      id: 'pkg/_function_battle.yaml',
      file: '_function_battle.yaml',
      content: source,
      functions: ['login'],
    }]
    const views = buildFunctionViews(files, (content, file) => parseFunctionLibrary(content, { file }))
    expect(views).toHaveLength(1)
    expect(views[0]).toMatchObject({ fileId: 'pkg/_function_battle.yaml', category: '_function_battle', name: 'login' })
  })

  it('合法函数结构经过 Visual 解析/序列化仍保留参数类型、vars、returns、分支与 $$ 语义', () => {
    const parsed = parseFunctionLibrary(source, { file: '_function' })
    expect(parsed.diagnostics).toEqual([])
    const once = serialize(parsed.model)
    const again = parseFunctionLibrary(once, { file: '_function' })
    expect(again.diagnostics).toEqual([])
    const fn = again.model.functions[0]
    expect(fn.params.map(param => [param.name, param.type, param.default])).toEqual([
      ['account', 'string', null],
      ['dry', 'boolean', false],
      ['payload', 'object', { mode: 'safe', count: 0 }],
    ])
    expect(fn.vars.region).toEqual([0.1, 0.2, 0.8, 0.9])
    expect(fn.returns).toEqual({ ok: true })
    expect(fn.run[0].args.entries.message.lit).toBe('$literal')
    expect(fn.run[1].then[0].kind).toBe('return')
    expect(fn.run[1].else[0].kind).toBe('repeat')
    expect(fn.run[1].else[0].body).toEqual([])
  })

  it('函数库 Visual 序列化对 $ 字面量只编码一次，重载后保持原值', () => {
    const parsed = parseFunctionLibrary(`functions:
  quote:
    run:
      - log:
          one: $$literal
          two: $$$literal
          three: $$
`, { file: '_function' })
    expect(parsed.diagnostics).toEqual([])

    const encoded = serialize(parsed.model)
    expect(encoded).toContain('one: $$literal')
    expect(encoded).toContain('two: $$$literal')
    expect(encoded).toContain('three: $$')

    const reparsed = parseFunctionLibrary(encoded, { file: '_function' })
    expect(reparsed.diagnostics).toEqual([])
    expect(reparsed.model.functions[0].run[0].args.entries).toEqual({
      one: { lit: '$literal' },
      two: { lit: '$$literal' },
      three: { lit: '$' },
    })
  })
})
