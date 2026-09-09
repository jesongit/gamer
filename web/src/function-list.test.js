// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest'
import { buildFunctionViews, filterFunctionViews, createPinyinInitials } from './console/function-list'
import { isFunctionLibraryFile } from './gamer-plugin-ids'

const files = [
  { id: 'pkg/_function.yaml', file: '_function', content: '', functions: ['登录', 'logout'] },
  { id: 'pkg/_function_battle.yaml', file: '_function_battle', content: '', functions: ['领取奖励'] },
]

function fakeParse(content, file) {
  const entry = files.find(f => f.file === file)
  return {
    model: {
      functions: (entry?.functions || []).map(name => ({ name, params: [], run: [{ uuid: `${file}-${name}` }] })),
    },
  }
}

describe('buildFunctionViews：跨函数库文件平铺全部函数', () => {
  it('每个函数一个视图，自带来源文件与文件 id', () => {
    const views = buildFunctionViews(files, fakeParse)
    expect(views.map(v => `${v.category}/${v.name}`)).toEqual(['_function/登录', '_function/logout', '_function_battle/领取奖励'])
    expect(views[0].fileId).toBe('pkg/_function.yaml')
    expect(views[0].model.run[0].uuid).toBe('_function-登录')
  })

  it('解析失败的文件跳过，不炸整个列表', () => {
    const views = buildFunctionViews(files, (content, file) => {
      if (file === '_function') throw new Error('bad yaml')
      return fakeParse(content, file)
    })
    expect(views.map(v => v.name)).toEqual(['领取奖励'])
  })
})

describe('filterFunctionViews：模糊匹配（名称/来源文件/拼音首字母）', () => {
  const views = buildFunctionViews(files, fakeParse)
  const py = createPinyinInitials()

  it('空查询原样返回', () => {
    expect(filterFunctionViews(views, '  ', py)).toBe(views)
  })

  it('中文名/来源文件/「文件/名」子串命中', () => {
    expect(filterFunctionViews(views, '登录', py)).toHaveLength(1)
    expect(filterFunctionViews(views, '_function_battle', py)).toHaveLength(1)
    expect(filterFunctionViews(views, '_function_battle/领取', py)).toHaveLength(1)
  })

  it('拼音首字母命中（登录→dl、领取奖励→lqjl）', () => {
    expect(filterFunctionViews(views, 'dl', py).map(v => v.name)).toEqual(['登录'])
    expect(filterFunctionViews(views, 'lqjl', py).map(v => v.name)).toEqual(['领取奖励'])
  })

  it('无命中返回空', () => {
    expect(filterFunctionViews(views, 'zzz', py)).toHaveLength(0)
  })
})

describe('isFunctionLibraryFile：与后端 resources::is_function_library_path 同规则', () => {
  it('_function 前缀 + .yaml 后缀识别为函数库', () => {
    expect(isFunctionLibraryFile('_function.yaml')).toBe(true)
    expect(isFunctionLibraryFile('_function_common.yaml')).toBe(true)
    expect(isFunctionLibraryFile('automations/_function2.yaml')).toBe(true)
    expect(isFunctionLibraryFile('daily.yaml')).toBe(false)
    expect(isFunctionLibraryFile('_function.yml')).toBe(false)
    expect(isFunctionLibraryFile('functions.yaml')).toBe(false)
    expect(isFunctionLibraryFile('_FUNCTION.yaml')).toBe(false)
  })
})
