import { describe, expect, it } from 'vitest'
import { renderOpTpl } from './console/op-template'

describe('renderOpTpl', () => {
  it('replaces values while keeping unknown placeholders intact', () => {
    expect(renderOpTpl('- tap: [{x}, {y}] {future}', { x: '0.2500', y: '0.5000' }))
      .toBe('- tap: [0.2500, 0.5000] {future}')
  })

  it('removes empty placeholders on blank lines and normalizes extra spacing', () => {
    const tpl = 'steps:\n  {optional}\n\n\n  - tap: [{x}, {y}]'
    expect(renderOpTpl(tpl, { optional: '', x: '0.1', y: '0.2' }))
      .toBe('steps:\n\n  - tap: [0.1, 0.2]')
  })

  it('indents continuation lines relative to the placeholder', () => {
    const tpl = '- swipe:\n    fm: {path}'
    expect(renderOpTpl(tpl, { path: '[0.1, 0.2]\n  to: [0.3, 0.4]' }))
      .toBe('- swipe:\n    fm: [0.1, 0.2]\n      to: [0.3, 0.4]')
  })
})
