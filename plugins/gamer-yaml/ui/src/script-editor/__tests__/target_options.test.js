import { describe, expect, it } from 'vitest'

describe('V1 target options contract', () => {
  it('function candidates are plain names with plugin/package groups', () => {
    // V1 无 script:/function: 前缀——候选即函数名，group 标来源
    const targets = [
      { target: 'tap', group: 'plugin', hint: '点击相对坐标（0..1；可传 match 的 center）' },
      { target: 'wait_find', group: 'plugin', hint: '等待模板出现；超时返回 null' },
      { target: 'login', group: 'package' },
    ]
    const groups = [
      { id: 'plugin', options: targets.filter((t) => t.group === 'plugin') },
      { id: 'package', options: targets.filter((t) => t.group !== 'plugin') },
    ]
    expect(groups[0].options.map((o) => o.target)).toEqual(['tap', 'wait_find'])
    expect(groups[1].options.map((o) => o.target)).toEqual(['login'])
    // 同名冲突：Package 函数与原生同名时不重复出现（组合期服务端已拒绝，前端去重）
    const seen = new Set(groups[0].options.map((o) => o.target))
    const deduped = targets.filter((t) => t.group !== 'plugin' && !seen.has(t.target))
    expect(deduped.map((t) => t.target)).toEqual(['login'])
  })
})
