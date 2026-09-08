import { describe, expect, it } from 'vitest'
import { parseFunctionLibrary, parseScript } from '../codec'
import { allocateUuids } from '../model'
import { breadcrumb, defaultAnchor, findStepLocation, rootContainerPath, startIndexMap, startIndexOf, containerLabel, cellDisplay } from '../selection'

describe('V1 selection / paths', () => {
  it('rootContainerPath: script run vs function body', () => {
    const script = parseScript('run: []\n').model
    expect(rootContainerPath(script)).toEqual(['run'])
    const lib = parseFunctionLibrary('functions:\n  a:\n    run: []\n').model
    expect(rootContainerPath(lib)).toEqual(['functions', 'a', 'run'])
  })

  it('findStepLocation returns run-based step_path', () => {
    const model = parseScript('run:\n  - if: $x\n    then:\n      - log: yes\n').model
    allocateUuids(model.run)
    const ifLoc = findStepLocation(model, model.run[0].uuid)
    expect(ifLoc.stepPath).toBe('run[0]')
    const logLoc = findStepLocation(model, model.run[0].then[0].uuid)
    expect(logLoc.stepPath).toBe('run[0].then[0]')
  })

  it('breadcrumb labels use V1 container names', () => {
    const model = parseScript('run:\n  - if: $x\n    then:\n      - repeat: 2\n        do:\n          - log: t\n').model
    allocateUuids(model.run)
    const logUuid = model.run[0].then[0].body[0].uuid
    const nodes = breadcrumb(model, logUuid)
    expect(nodes.map((n) => n.label)).toEqual(['主流程', '如果为真', '循环体'])
  })

  it('containerLabel V1 set', () => {
    const model = parseScript('run:\n  - repeat: 1\n    do: []\n').model
    allocateUuids(model.run)
    expect(containerLabel(model.run[0], 'body')).toBe('循环体')
    expect(containerLabel(null, '', '主流程')).toBe('主流程')
  })

  it('defaultAnchor uses selection then container end', () => {
    const model = parseScript('run:\n  - log: a\n  - log: b\n').model
    allocateUuids(model.run)
    const anchor = defaultAnchor(model, model.run[0].uuid)
    expect(anchor).toEqual({ containerPath: ['run'], index: 1 })
    const end = defaultAnchor(model, null)
    expect(end.index).toBe(2)
  })

  it('startIndexMap covers script top-level and function bodies', () => {
    const script = parseScript('run:\n  - log: a\n  - log: b\n').model
    allocateUuids(script.run)
    expect(startIndexOf(script, script.run[1].uuid)).toBe(1)
    const lib = parseFunctionLibrary('functions:\n  f:\n    run:\n      - log: x\n').model
    allocateUuids(lib.functions[0].run)
    expect(startIndexOf(lib, lib.functions[0].run[0].uuid)).toBe(0)
    expect(startIndexMap(lib)).toHaveLength(1)
  })

  it('cellDisplay shows $refs and arrays', () => {
    expect(cellDisplay({ ref: 'home.center' })).toBe('$home.center')
    expect(cellDisplay({ lit: [0.5, 0.5] })).toBe('[0.5, 0.5]')
  })
})
