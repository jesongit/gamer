import { describe, expect, it } from 'vitest'
import { buildSearchSuffix, defaultShortName, isValidShortName } from '../naming'

/**
 * 录制模板命名（plan §11.7）：默认短名格式与零填充、短名校验（禁 #）、
 * 搜索区域展示串。完整文件名由服务端拼接，前端只生成建议值。
 */

describe('recording/naming：defaultShortName', () => {
  const D = new Date(2026, 7, 29, 10, 30, 0) // 本地 2026-08-29（构造即本地，无时区歧义）

  it('格式：record_<kind>_YYYYMMDD_NNN.png', () => {
    expect(defaultShortName('click', D, 1)).toBe('record_click_20260829_001.png')
    expect(defaultShortName('swipe', D, 2)).toBe('record_swipe_20260829_002.png')
  })

  it('NNN 三位零填充，seq 从 1 起；≥1000 自然扩位', () => {
    expect(defaultShortName('click', D, 42)).toBe('record_click_20260829_042.png')
    expect(defaultShortName('click', D, 999)).toBe('record_click_20260829_999.png')
    expect(defaultShortName('click', D, 1000)).toBe('record_click_20260829_1000.png')
  })

  it('跨日期用传入日期的本地年月日', () => {
    expect(defaultShortName('click', new Date(2027, 0, 3), 7)).toBe('record_click_20270103_007.png')
    expect(defaultShortName('swipe', new Date(2026, 11, 31), 12)).toBe('record_swipe_20261231_012.png')
  })

  it('非法 kind / 非法 date 报错', () => {
    expect(() => defaultShortName('alt', D, 1)).toThrow()
    expect(() => defaultShortName('click', '2026-08-29', 1)).toThrow()
  })
})

describe('recording/naming：isValidShortName', () => {
  it('合法：字母数字下划线连字符 + .png', () => {
    expect(isValidShortName('record_click_20260829_001.png')).toBe(true)
    expect(isValidShortName('A-Z_a-z_0-9.png')).toBe(true)
    expect(isValidShortName('login.png')).toBe(true)
  })

  it('非法：空名/缺扩展名/其他扩展名/路径/#/空格/点/中文', () => {
    expect(isValidShortName('')).toBe(false)
    expect(isValidShortName('record')).toBe(false)
    expect(isValidShortName('record.jpg')).toBe(false)
    expect(isValidShortName('record.PNG')).toBe(false) // 大写扩展名不允许，短名小写约定
    expect(isValidShortName('re#50_50_100_100.png')).toBe(false) // # 是服务端元数据分隔符
    expect(isValidShortName('re cord.png')).toBe(false)
    expect(isValidShortName('re.cord.png')).toBe(false)
    expect(isValidShortName('账号.png')).toBe(false)
    expect(isValidShortName('.png')).toBe(false)
    expect(isValidShortName(null)).toBe(false)
    expect(isValidShortName(123)).toBe(false)
  })
})

describe('recording/naming：buildSearchSuffix', () => {
  it('#x_y_w_h 原始像素', () => {
    expect(buildSearchSuffix({ x: 490, y: 910, w: 100, h: 100 })).toBe('#490_910_100_100')
    expect(buildSearchSuffix({ x: 0, y: 0, w: 63, h: 63 })).toBe('#0_0_63_63')
  })

  it('非整数坐标四舍五入到整数', () => {
    expect(buildSearchSuffix({ x: 12.5, y: 0.4, w: 62.5, h: 99.6 })).toBe('#13_0_63_100')
  })
})
