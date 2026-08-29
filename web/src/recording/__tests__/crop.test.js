import { describe, expect, it } from 'vitest'
import {
  autoTemplateRect,
  clampRect,
  rectCenterPx,
  searchRectAuto,
  searchRectManual,
  toRelative,
  unionRect,
} from '../crop'

/**
 * 录制裁切数学（plan §11.6，§16.3：50×50、四边/四角边缘裁剪、100×100、
 * union(A,M)+25px、横竖屏、相对坐标）。
 * 全部期望值为按实现规则（Math.round 居中、负起点归零收缩、右/下越界收缩）手工推演的精确数。
 */

describe('recording/crop：autoTemplateRect（A：50×50 居中 + 边界裁剪）', () => {
  it('画面中央：完整 50×50 居中', () => {
    // 中心 (540, 960) → x = 540-25 = 515
    expect(autoTemplateRect(1080, 1920, 540, 960)).toEqual({ x: 515, y: 935, w: 50, h: 50 })
  })

  it('size 参数可覆盖默认 50', () => {
    expect(autoTemplateRect(1080, 1920, 100, 100, 20)).toEqual({ x: 90, y: 90, w: 20, h: 20 })
  })

  it('四角：左上、右上、左下、右下各自收缩', () => {
    // 左上 (0,0)：x=-25 → 0，w=50-25=25
    expect(autoTemplateRect(1080, 1920, 0, 0)).toEqual({ x: 0, y: 0, w: 25, h: 25 })
    // 右上 (1080,0)：x=1055，x+w=1105>1080 → w=25；y=0 收缩 h=25
    expect(autoTemplateRect(1080, 1920, 1080, 0)).toEqual({ x: 1055, y: 0, w: 25, h: 25 })
    // 左下 (0,1920)
    expect(autoTemplateRect(1080, 1920, 0, 1920)).toEqual({ x: 0, y: 1895, w: 25, h: 25 })
    // 右下 (1080,1920)
    expect(autoTemplateRect(1080, 1920, 1080, 1920)).toEqual({ x: 1055, y: 1895, w: 25, h: 25 })
  })

  it('四边（非角）：单边收缩', () => {
    // 左边中点 (0, 960)：w 收缩
    expect(autoTemplateRect(1080, 1920, 0, 960)).toEqual({ x: 0, y: 935, w: 25, h: 50 })
    // 右边中点 (1080, 960)
    expect(autoTemplateRect(1080, 1920, 1080, 960)).toEqual({ x: 1055, y: 935, w: 25, h: 50 })
    // 顶边中点 (540, 0)
    expect(autoTemplateRect(1080, 1920, 540, 0)).toEqual({ x: 515, y: 0, w: 50, h: 25 })
    // 底边中点 (540, 1920)
    expect(autoTemplateRect(1080, 1920, 540, 1920)).toEqual({ x: 515, y: 1895, w: 50, h: 25 })
  })

  it('奇数坐标 Math.round 居中：cx=101 → x=76', () => {
    expect(autoTemplateRect(1080, 1920, 101, 333)).toEqual({ x: 76, y: 308, w: 50, h: 50 })
  })

  it('横屏 1920×1080 与竖屏对称一致', () => {
    expect(autoTemplateRect(1920, 1080, 0, 0)).toEqual({ x: 0, y: 0, w: 25, h: 25 })
    expect(autoTemplateRect(1920, 1080, 1920, 1080)).toEqual({ x: 1895, y: 1055, w: 25, h: 25 })
    expect(autoTemplateRect(1920, 1080, 960, 540)).toEqual({ x: 935, y: 515, w: 50, h: 50 })
  })

  it('中心越界输入钳进帧内（防御）', () => {
    expect(autoTemplateRect(1080, 1920, -100, -100)).toEqual({ x: 0, y: 0, w: 25, h: 25 })
    expect(autoTemplateRect(1080, 1920, 5000, 5000)).toEqual({ x: 1055, y: 1895, w: 25, h: 25 })
  })
})

describe('recording/crop：searchRectAuto（S：以 A 为中心的 100×100）', () => {
  it('A 在画面中央：完整 100×100', () => {
    const a = { x: 515, y: 935, w: 50, h: 50 } // 中心 (540, 960)
    expect(searchRectAuto(a, 1080, 1920)).toEqual({ x: 490, y: 910, w: 100, h: 100 })
  })

  it('A 贴四边：S 单边/双边收缩', () => {
    // A 贴左上角 {0,0,25,25}，中心 (12.5,12.5)：x = round(12.5-50) = round(-37.5) = -37 → 归零收缩 w=63
    const corner = { x: 0, y: 0, w: 25, h: 25 }
    expect(searchRectAuto(corner, 1080, 1920)).toEqual({ x: 0, y: 0, w: 63, h: 63 })
    // A 贴右边 {1055,935,25,50}，中心 (1067.5,960)：x = round(1017.5) = 1018，x+100 > 1080 → w = 62
    const right = { x: 1055, y: 935, w: 25, h: 50 }
    expect(searchRectAuto(right, 1080, 1920)).toEqual({ x: 1018, y: 910, w: 62, h: 100 })
  })

  it('小帧：A 占满画面时 S 被钳为整帧', () => {
    const a = { x: 0, y: 0, w: 40, h: 40 }
    expect(searchRectAuto(a, 40, 40)).toEqual({ x: 0, y: 0, w: 40, h: 40 })
  })

  it('横竖屏一致（同一 A 相对位置下镜像约束成立）', () => {
    const a = { x: 0, y: 0, w: 25, h: 25 }
    expect(searchRectAuto(a, 1920, 1080)).toEqual({ x: 0, y: 0, w: 63, h: 63 })
  })
})

describe('recording/crop：searchRectManual（union(A,M)+25px）', () => {
  it('M 与 A 部分重叠：并集外扩 25', () => {
    const a = { x: 75, y: 75, w: 50, h: 50 }
    const m = { x: 200, y: 150, w: 40, h: 30 }
    // union = {75,75,165,105}；外扩 → {50,50,215,155}
    expect(searchRectManual(a, m, 1080, 1920)).toEqual({ x: 50, y: 50, w: 215, h: 155 })
  })

  it('M 完全在 A 外（右下方）：并集覆盖两者再外扩', () => {
    const a = { x: 75, y: 75, w: 50, h: 50 }
    const m = { x: 600, y: 500, w: 80, h: 60 }
    // union = {75,75,605,485}；外扩 → {50,50,655,535}
    expect(searchRectManual(a, m, 1080, 1920)).toEqual({ x: 50, y: 50, w: 655, h: 535 })
  })

  it('M 在 A 内：并集 = A，等于自动 100×100 再各扩 25（若不越界）', () => {
    const a = { x: 200, y: 200, w: 50, h: 50 }
    const m = { x: 210, y: 210, w: 10, h: 10 }
    // union = A = {200,200,50,50}；外扩 → {175,175,100,100}
    expect(searchRectManual(a, m, 1080, 1920)).toEqual({ x: 175, y: 175, w: 100, h: 100 })
  })

  it('外扩越界：贴近帧边时收缩到边界', () => {
    const a = { x: 0, y: 0, w: 25, h: 25 }
    const m = { x: 10, y: 10, w: 20, h: 20 }
    // union = {0,0,30,30}；外扩 → {-25,-25,80,80} → {0,0,55,55}
    expect(searchRectManual(a, m, 1080, 1920)).toEqual({ x: 0, y: 0, w: 55, h: 55 })
    // 右下角：A={1055,1895,25,25}, M 同点 → union={1055,1895,25,25} → 外扩 {1030,1870,75,75} → x+w=1105>1080 → w=50
    expect(searchRectManual({ x: 1055, y: 1895, w: 25, h: 25 }, { x: 1055, y: 1895, w: 25, h: 25 }, 1080, 1920))
      .toEqual({ x: 1030, y: 1870, w: 50, h: 50 })
  })

  it('横屏 1920×1080 下同样成立', () => {
    // union({935,515,50,50},{960,540,30,30}) = {935,515,55,55}；外扩 → {910,490,105,105}
    expect(searchRectManual({ x: 935, y: 515, w: 50, h: 50 }, { x: 960, y: 540, w: 30, h: 30 }, 1920, 1080))
      .toEqual({ x: 910, y: 490, w: 105, h: 105 })
  })
})

describe('recording/crop：工具函数', () => {
  it('clampRect：负起点平移、右/下越界收缩、宽高非负', () => {
    expect(clampRect({ x: -10, y: -5, w: 50, h: 50 }, 1080, 1920)).toEqual({ x: 0, y: 0, w: 40, h: 45 })
    expect(clampRect({ x: 1070, y: 10, w: 100, h: 20 }, 1080, 1920)).toEqual({ x: 1070, y: 10, w: 10, h: 20 })
    expect(clampRect({ x: 2000, y: 3000, w: 10, h: 10 }, 1080, 1920)).toEqual({ x: 1080, y: 1920, w: 0, h: 0 })
    expect(clampRect({ x: 10, y: 10, w: -5, h: -1 }, 1080, 1920)).toEqual({ x: 10, y: 10, w: 0, h: 0 })
    expect(clampRect(null, 100, 100)).toBeNull()
  })

  it('unionRect', () => {
    expect(unionRect({ x: 0, y: 0, w: 10, h: 10 }, { x: 5, y: 5, w: 10, h: 10 })).toEqual({ x: 0, y: 0, w: 15, h: 15 })
    expect(unionRect({ x: 50, y: 50, w: 5, h: 5 }, { x: 0, y: 0, w: 3, h: 3 })).toEqual({ x: 0, y: 0, w: 55, h: 55 })
  })

  it('rectCenterPx（半像素中心）', () => {
    expect(rectCenterPx({ x: 515, y: 935, w: 50, h: 50 })).toEqual([540, 960])
    expect(rectCenterPx({ x: 0, y: 0, w: 25, h: 25 })).toEqual([12.5, 12.5])
  })

  it('toRelative：中央/四角/越界钳制/除零防护', () => {
    expect(toRelative(540, 960, 1080, 1920)).toEqual([0.5, 0.5])
    expect(toRelative(0, 0, 1080, 1920)).toEqual([0, 0])
    expect(toRelative(1080, 1920, 1080, 1920)).toEqual([1, 1])
    expect(toRelative(-10, 2000, 1080, 1920)).toEqual([0, 1])
    expect(toRelative(100, 100, 0, 0)).toEqual([0, 0])
  })
})
