import { describe, expect, it } from 'vitest'
import { createCall, createControl, argsFromSchema, makeCall, CONTROL_ENTRIES } from '../factories'

describe('V1 factories', () => {
  it('createCall builds a call step with args and as', () => {
    const step = createCall('tap', { kind: 'value', cell: { lit: [0.5, 0.5] } }, 'hit')
    expect(step).toMatchObject({
      kind: 'call', fn: 'tap', as: 'hit',
      args: { kind: 'value', cell: { lit: [0.5, 0.5] } },
    })
    expect(typeof step.uuid).toBe('string')
  })

  it('createControl covers if/repeat/return', () => {
    const ifStep = createControl('if')
    expect(ifStep).toMatchObject({ kind: 'if', cond: { lit: true }, then: [], else: [] })
    const repeatStep = createControl('repeat')
    expect(repeatStep).toMatchObject({ kind: 'repeat', times: { lit: 3 }, body: [] })
    const returnStep = createControl('return')
    expect(returnStep).toMatchObject({ kind: 'return', value: { lit: null } })
  })

  it('argsFromSchema prefills defaults and skips empty', () => {
    const args = argsFromSchema([
      { name: 'template', type: 'template', required: true, default: null },
      { name: 'threshold', type: 'number', required: false, default: 0.8 },
    ])
    expect(args).toEqual({ kind: 'map', entries: { threshold: { lit: 0.8 } } })
    expect(argsFromSchema([])).toEqual({ kind: 'none' })
  })

  it('makeCall is the no-schema fallback', () => {
    expect(makeCall('sleep')).toMatchObject({ kind: 'call', fn: 'sleep', args: { kind: 'none' }, as: null })
  })

  it('CONTROL_ENTRIES covers the three control kinds', () => {
    expect(CONTROL_ENTRIES.map((e) => e.kind)).toEqual(['if', 'repeat', 'return'])
  })
})
