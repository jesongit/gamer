// @vitest-environment node
/**
 * TemplateStudio 纯函数（Phase 7 §10.2）：相对区域换算、oriented→reference
 * 变换（calibration.js 接入）、contain 叠加/取点映射、帧身份标签。
 */
import { describe, expect, it } from 'vitest'
import {
  describeFrameIdentity,
  eventToImagePoint,
  orientedToReference,
  pixelRectToStyle,
  regionFromRect,
  regionToPixelRect,
} from './components/video/templateStudio'

describe('regionFromRect / regionToPixelRect（模板区域 ↔ 帧像素，双向一致）', () => {
  it('像素矩形 → 相对区域，clamp 到画面内；太小选框拒绝', () => {
    // 1000×500 帧上的 (100,50,300,150) → [0.1, 0.1, 0.4, 0.4]
    expect(regionFromRect({ x: 100, y: 50, w: 300, h: 150 }, 1000, 500)).toEqual([0.1, 0.1, 0.4, 0.4])
    // 越界钳制
    expect(regionFromRect({ x: -50, y: -50, w: 1000, h: 600 }, 1000, 500)).toEqual([0, 0, 0.95, 1])
    // 过小（<0.8% 边长）拒绝
    expect(regionFromRect({ x: 0, y: 0, w: 2, h: 2 }, 1000, 500)).toBeNull()
    expect(regionFromRect({ x: 0, y: 0, w: 10, h: 10 }, 0, 0)).toBeNull()
  })

  it('相对区域 → 帧像素矩形（离线测试 region 参数；不同参考尺寸一致换算）', () => {
    const region = [0.1, 0.1, 0.4, 0.4]
    expect(regionToPixelRect(region, 1000, 500)).toEqual({ x: 100, y: 50, w: 300, h: 150 })
    // 同一相对区域在 2000×1000 帧上等比放大——搜索区不因帧尺寸被遗忘
    expect(regionToPixelRect(region, 2000, 1000)).toEqual({ x: 200, y: 100, w: 600, h: 300 })
    expect(regionToPixelRect([0.5, 0.5, 0.5, 0.5], 100, 100)).toBeNull()
  })
})

describe('orientedToReference（帧 PNG 像素 → 制作参考坐标，经 calibration.js）', () => {
  const encoded = { width: 1000, height: 500 }
  const identity = {
    version: 1, rotation: 0, pixel_aspect: { num: 1, den: 1 },
    content_rect: null, reference_size: { width: 1000, height: 500 },
  }

  it('恒等校准：oriented == reference', () => {
    expect(orientedToReference({ x: 250, y: 125 }, identity, encoded)).toEqual({ x: 250, y: 125 })
  })

  it('content 裁剪 + 等比缩放 + letterbox：不静默非等比拉伸', () => {
    // 裁掉左侧 200px（800×500 content）→ 参考分辨率 400×200（比例不同 → min 缩放 0.4 + 居中补边）
    const cal = {
      version: 2, rotation: 0, pixel_aspect: { num: 1, den: 1 },
      content_rect: { x: 200, y: 0, w: 800, h: 500 },
      reference_size: { width: 400, height: 200 },
    }
    const point = orientedToReference({ x: 600, y: 250 }, cal, encoded)
    // content 内坐标 (400, 250) → ×0.4 = (160, 100) → 补边 padY=(200-200)/2=0, padX=(400-320)/2=40
    expect(point.x).toBeCloseTo(160 + 40, 6)
    expect(point.y).toBeCloseTo(100, 6)
  })
})

describe('pixelRectToStyle / eventToImagePoint（contain letterbox 映射互逆）', () => {
  const imgRect = { left: 10, top: 20, width: 500, height: 300 }
  const natural = { width: 1000, height: 500 } // ratio = min(0.5, 0.6) = 0.5

  it('帧像素矩形 → 显示样式', () => {
    // ratio = 0.5；offsetX = (500-1000*0.5)/2 = 0，offsetY = (300-500*0.5)/2 = 25
    const style = pixelRectToStyle({ x: 100, y: 50, w: 300, h: 150 }, imgRect, 1000, 500)
    expect(style).toEqual({ left: '50px', top: '50px', width: '150px', height: '75px' })
  })

  it('鼠标事件 → 帧像素（clamp 越界）；与样式映射互逆', () => {
    // 逆变换：帧像素 (250,125) → clientX = 10+0+250*0.5，clientY = 20+25+125*0.5
    const point = eventToImagePoint(
      { clientX: 10 + 0 + 125, clientY: 20 + 25 + 62.5 },
      imgRect, 1000, 500,
    )
    expect(point.x).toBeCloseTo(250, 3)
    expect(point.y).toBeCloseTo(125, 3)
    const outside = eventToImagePoint({ clientX: -999, clientY: -999 }, imgRect, 1000, 500)
    expect(outside).toEqual({ x: 0, y: 0 })
  })
})

describe('describeFrameIdentity（回查标签）', () => {
  it('帧身份优先展示序索引；缺 mediaId 为空', () => {
    expect(describeFrameIdentity({ mediaId: 'm1', frameIndex: 7, ptsUs: 333000 })).toBe('m1 · 帧 #7')
    expect(describeFrameIdentity({ mediaId: 'm1', ptsUs: 333000 })).toBe('m1 · pts_us=333000')
    expect(describeFrameIdentity({ frameIndex: 7 })).toBe('')
  })
})
