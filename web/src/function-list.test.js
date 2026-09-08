// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest'
import { buildFunctionViews, filterFunctionViews, createPinyinInitials } from './console/function-list'

const files = [
  { id: 'pkg/functions/login.yaml', file: 'login', content: '', functions: ['登录', 'logout'] },
  { id: 'pkg/functions/daily.yaml', file: '日常', content: '', functions: ['领取奖励'] },
]

function fakeParse(content, file) {
  const entry = files.find(f => f.file === file)
  return {
    model: {
      functions: (entry?.functions || []).map(name => ({ name, params: [], steps: [{ uuid: `${file}-${name}` }] })),
    },
  }
}

describe('buildFunctionViews：跨分类平铺全部函数', () => {
  it('每个函数一个视图，自带分类与文件 id', () => {
    const views = buildFunctionViews(files, fakeParse)
    expect(views.map(v => `${v.category}/${v.name}`)).toEqual(['login/登录', 'login/logout', '日常/领取奖励'])
    expect(views[0].fileId).toBe('pkg/functions/login.yaml')
    expect(views[0].model.steps[0].uuid).toBe('login-登录')
  })

  it('解析失败的文件跳过，不炸整个列表', () => {
    const views = buildFunctionViews(files, (content, file) => {
      if (file === 'login') throw new Error('bad yaml')
      return fakeParse(content, file)
    })
    expect(views.map(v => v.name)).toEqual(['领取奖励'])
  })
})

describe('filterFunctionViews：模糊匹配（名称/分类/拼音首字母）', () => {
  const views = buildFunctionViews(files, fakeParse)
  const py = createPinyinInitials()

  it('空查询原样返回', () => {
    expect(filterFunctionViews(views, '  ', py)).toBe(views)
  })

  it('中文名/分类名/「分类/名」子串命中', () => {
    expect(filterFunctionViews(views, '登录', py)).toHaveLength(1)
    expect(filterFunctionViews(views, 'login', py)).toHaveLength(2)
    expect(filterFunctionViews(views, '日常/领取', py)).toHaveLength(1)
  })

  it('拼音首字母命中（登录→dl、领取奖励→lqjl）', () => {
    expect(filterFunctionViews(views, 'dl', py).map(v => v.name)).toEqual(['登录'])
    expect(filterFunctionViews(views, 'lqjl', py).map(v => v.name)).toEqual(['领取奖励'])
  })

  it('无命中返回空', () => {
    expect(filterFunctionViews(views, 'zzz', py)).toHaveLength(0)
  })
})
