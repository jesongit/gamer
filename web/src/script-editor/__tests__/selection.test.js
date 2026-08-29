import { describe, expect, it } from 'vitest'
import { parseScript } from '../codec'
import {
  findStepLocation,
  findStep,
  stepPathOf,
  defaultAnchor,
  breadcrumb,
  startIndexMap,
  startIndexOf,
  rootContainerPath,
  pathToString,
} from '../selection'

/**
 * 选择与插入：uuid ↔ 路径互查、插入锚点、面包屑、start_index 映射。
 */

const NESTED = `steps:
  - match:
    - test1.png:
      - if: true
        then:
          - log: 深
    else:
      - log: 兜底
    timeout: 30s
`

describe('selection：uuid ↔ 路径互查', () => {
  it('顶层与深层嵌套都能定位，step_path 与 validation 字符串形态一致', () => {
    const { model } = parseScript(NESTED)
    const matchStep = model.steps[0]
    const ifStep = matchStep.candidates[0].steps[0]
    const deepLog = ifStep.then[0]

    const loc1 = findStepLocation(model, matchStep.uuid)
    expect(loc1.path).toEqual(['steps', 0])
    expect(loc1.stepPath).toBe('steps[0]')
    expect(loc1.containerPath).toEqual(['steps'])

    const loc2 = findStepLocation(model, deepLog.uuid)
    // 路径语法：候选分支容器 = [..., 'candidates', n]；候选内步骤再带自身下标段。
    expect(loc2.path).toEqual(['steps', 0, 'candidates', 0, 0, 'then', 0])
    // step_path 字符串形态按契约 §5.2：candidates 段带 .steps（steps[2].candidates[1].steps[0]）。
    expect(loc2.stepPath).toBe('steps[0].candidates[0].steps[0].then[0]')
    expect(loc2.containerPath).toEqual(['steps', 0, 'candidates', 0, 0, 'then'])
    expect(loc2.list).toBe(ifStep.then)

    expect(findStep(model, deepLog.uuid)).toBe(deepLog)
    expect(stepPathOf(model, deepLog.uuid)).toBe('steps[0].candidates[0].steps[0].then[0]')
    expect(stepPathOf(model, '不存在')).toBeNull()
  })
})

describe('selection：插入锚点', () => {
  it('选中步骤 → 同容器其后；未选中 → 当前容器末尾', () => {
    const { model } = parseScript(NESTED)
    const matchStep = model.steps[0]
    const anchor1 = defaultAnchor(model, matchStep.uuid)
    expect(anchor1).toEqual({ containerPath: ['steps'], index: 1 })

    const anchor2 = defaultAnchor(model, null)
    expect(anchor2).toEqual({ containerPath: ['steps'], index: 1 })

    const ifStep = matchStep.candidates[0].steps[0]
    const anchor3 = defaultAnchor(model, ifStep.uuid)
    expect(anchor3).toEqual({ containerPath: ['steps', 0, 'candidates', 0], index: 1 })

    // 显式当前容器（面包屑切到分支内）
    const anchor4 = defaultAnchor(model, null, ['steps', 0, 'candidates', 0])
    expect(anchor4.index).toBe(1)
  })

  it('函数库根容器', () => {
    // 简单函数库模型由 codec 解析
    const lib = parseScript('steps: []\n').model
    expect(rootContainerPath(lib)).toEqual(['steps'])
  })
})

describe('selection：面包屑', () => {
  it('主流程 / 命中 test1 / 如果为真', () => {
    const { model } = parseScript(NESTED)
    const ifStep = model.steps[0].candidates[0].steps[0]
    const deepLog = ifStep.then[0]
    const crumbs = breadcrumb(model, deepLog.uuid)
    expect(crumbs.map((c) => c.label)).toEqual(['主流程', '命中 test1.png', '如果为真'])
    expect(crumbs[0].stepUuid).toBeNull()
    expect(crumbs[1].containerPath).toEqual(['steps', 0, 'candidates', 0])
    expect(crumbs[2].containerPath).toEqual(['steps', 0, 'candidates', 0, 0, 'then'])
  })

  it('color/loop/else 容器命名', () => {
    const { model } = parseScript(`steps:
  - color:
      at: [0.5, 0.5]
      expect:
        - ff8800:
          - log: 红
      else:
        - log: 兜
  - loop:
      steps:
        - log: 体
`)
    const colorStep = model.steps[0]
    const elseStep = colorStep.else[0]
    expect(breadcrumb(model, elseStep.uuid).map((c) => c.label)).toEqual(['主流程', '颜色未命中'])
    const loopStep = model.steps[1]
    expect(breadcrumb(model, loopStep.steps[0].uuid).map((c) => c.label)).toEqual(['主流程', '循环体'])
  })
})

describe('selection：start_index 映射', () => {
  it('顶层步骤 → 0 基序号；嵌套步骤不在映射内', () => {
    const { model } = parseScript(NESTED)
    const map = startIndexMap(model)
    expect(map).toEqual([{ uuid: model.steps[0].uuid, index: 0 }])
    expect(startIndexOf(model, model.steps[0].uuid)).toBe(0)
    const ifStep = model.steps[0].candidates[0].steps[0]
    expect(startIndexOf(model, ifStep.uuid)).toBeNull()
  })
})

describe('selection：路径展示', () => {
  it('pathToString', () => {
    expect(pathToString(['steps'])).toBe('steps')
    expect(pathToString(['steps', 0, 'then', 1])).toBe('steps[0].then[1]')
    expect(pathToString(['functions', 'login', 'steps', 2])).toBe('functions.login.steps[2]')
  })
})
