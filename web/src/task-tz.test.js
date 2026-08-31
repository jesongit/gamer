import { describe, expect, it } from 'vitest'
import { parseOffsetMinutes, formatUtcOffset, serverTzLabelFromTasks } from './task-tz'

/**
 * 任务页服务端时区推导（缺陷修复回归）：
 * /api/system/info 按契约禁止携带 timezone，服务端时区只能从任务时间戳的
 * RFC3339 偏移推导；推导不出必须走兜底文案（不能拿固定 UTC 的 last_run_at 说谎）。
 */

describe('parseOffsetMinutes', () => {
  it('解析正偏移（+08:00）', () => {
    expect(parseOffsetMinutes('2026-09-01T08:00:00+08:00')).toBe(480)
    expect(parseOffsetMinutes('2026-09-01T08:00:00.123+08:00')).toBe(480)
  })

  it('解析负半时区偏移（-05:30）', () => {
    expect(parseOffsetMinutes('2026-09-01T08:00:00-05:30')).toBe(-330)
  })

  it('解析 Z（UTC，含小写与无冒号形态）', () => {
    expect(parseOffsetMinutes('2026-08-31T23:00:00Z')).toBe(0)
    expect(parseOffsetMinutes('2026-08-31T23:00:00.789z')).toBe(0)
    expect(parseOffsetMinutes('2026-08-31T23:00:00+0530')).toBe(330)
  })

  it('无偏移/无数据分支返回 null', () => {
    expect(parseOffsetMinutes('2026-09-01 08:00:00')).toBeNull() // 服务端现行 next_run 形态（无偏移）
    expect(parseOffsetMinutes('-')).toBeNull()
    expect(parseOffsetMinutes('')).toBeNull()
    expect(parseOffsetMinutes(null)).toBeNull()
    expect(parseOffsetMinutes(undefined)).toBeNull()
    expect(parseOffsetMinutes(123)).toBeNull()
    expect(parseOffsetMinutes('2026-09-01T08:00:00+08:60')).toBeNull() // 分钟位非法
  })
})

describe('formatUtcOffset', () => {
  it('格式化为 UTC±HH:MM 标签', () => {
    expect(formatUtcOffset(480)).toBe('UTC+08:00')
    expect(formatUtcOffset(-330)).toBe('UTC-05:30')
    expect(formatUtcOffset(0)).toBe('UTC+00:00')
    expect(formatUtcOffset(95)).toBe('UTC+01:35')
  })

  it('非法输入返回 null', () => {
    expect(formatUtcOffset(null)).toBeNull()
    expect(formatUtcOffset(undefined)).toBeNull()
    expect(formatUtcOffset(Number.NaN)).toBeNull()
    expect(formatUtcOffset('480')).toBeNull()
  })
})

describe('serverTzLabelFromTasks', () => {
  it('next_run 带偏移 → 推导服务端时区（优先于 last_run_at）', () => {
    expect(serverTzLabelFromTasks([
      { next_run: '-', last_run_at: '2026-08-31T15:00:00.000Z' },
      { next_run: '2026-09-01T08:00:00+08:00', last_run_at: '2026-08-31T23:00:00.000Z' },
    ])).toBe('UTC+08:00')
  })

  it('next_run 无偏移且 last_run_at 为固定 UTC Z 串 → 不误报，返回 null', () => {
    // 现行服务端序列化形态：Z 不随 TZ 变化，不能当作本地偏移展示
    expect(serverTzLabelFromTasks([
      { next_run: '2026-09-01 08:00:00', last_run_at: '2026-08-31T23:00:00.000Z' },
    ])).toBeNull()
  })

  it('last_run_at 带显式数字偏移时可作为兜底来源', () => {
    expect(serverTzLabelFromTasks([
      { next_run: '-', last_run_at: '2026-08-31T22:30:00.000-05:30' },
    ])).toBe('UTC-05:30')
  })

  it('空列表/非法输入返回 null', () => {
    expect(serverTzLabelFromTasks([])).toBeNull()
    expect(serverTzLabelFromTasks(null)).toBeNull()
    expect(serverTzLabelFromTasks('tasks')).toBeNull()
    expect(serverTzLabelFromTasks([null, {}])).toBeNull()
  })
})
