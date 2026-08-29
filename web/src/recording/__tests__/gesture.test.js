import { describe, expect, it } from 'vitest'
import { CLICK_MAX_MS, classifyGesture, moveThresholdPx } from '../gesture'

/**
 * 手势分类纯逻辑（plan §11.4/§11.5，§16.3 阈值/分类边界项）：
 * 阈值下限 8px、短边 0.5% 比例、600ms 边界（含等值）、阈值等值不算滑动、
 * longpress 不被静默转成点击、超阈值与时长无关。
 */

describe('recording/gesture：moveThresholdPx', () => {
  it('短边 0.5% 高于 8px 时按比例放宽', () => {
    // 3840x2160：min=2160 → 2160*0.005 = 10.8
    expect(moveThresholdPx(3840, 2160)).toBeCloseTo(10.8, 10)
    // 2000x2000：10
    expect(moveThresholdPx(2000, 2000)).toBe(10)
    // 3200x1600 横屏：min=1600 → 8（1600*0.005 = 8，等值仍取 8）
    expect(moveThresholdPx(3200, 1600)).toBe(8)
  })

  it('小分辨率保底 8px', () => {
    expect(moveThresholdPx(1080, 1920)).toBe(8) // 1080*0.005 = 5.4 → 8
    expect(moveThresholdPx(800, 600)).toBe(8) // 3 → 8
    expect(moveThresholdPx(100, 100)).toBe(8) // 0.5 → 8
    expect(moveThresholdPx(1200, 900)).toBe(8) // 4.5 → 8
  })

  it('横竖屏只看短边', () => {
    expect(moveThresholdPx(1920, 1080)).toBe(8)
    expect(moveThresholdPx(1080, 1920)).toBe(8)
    expect(moveThresholdPx(2560, 1440)).toBe(8) // 1440*0.005=7.2 → 保底 8
  })
})

describe('recording/gesture：classifyGesture', () => {
  const F = { frameW: 1080, frameH: 1920 } // 阈值 = 8

  it('零位移零时长 → click', () => {
    expect(classifyGesture({ dxPx: 0, dyPx: 0, durationMs: 0, ...F })).toBe('click')
  })

  it('位移恰等于阈值 → 不算滑动（阈值内含等值）', () => {
    expect(classifyGesture({ dxPx: 8, dyPx: 0, durationMs: 100, ...F })).toBe('click')
    expect(classifyGesture({ dxPx: 0, dyPx: 8, durationMs: 100, ...F })).toBe('click')
    // 对角向量 |(4,4)| = 8·√0.5·√2 = 8 → 等值
    expect(classifyGesture({ dxPx: 4 * Math.SQRT2, dyPx: 4 * Math.SQRT2, durationMs: 100, ...F })).toBe('click')
  })

  it('位移刚超过阈值 → swipe（缓慢长按拖动也算滑动）', () => {
    expect(classifyGesture({ dxPx: 8.0000001, dyPx: 0, durationMs: 100, ...F })).toBe('swipe')
    expect(classifyGesture({ dxPx: 9, dyPx: 0, durationMs: 5000, ...F })).toBe('swipe')
    expect(classifyGesture({ dxPx: 0, dyPx: -30, durationMs: 1200, ...F })).toBe('swipe')
  })

  it(`时长恰为 ${CLICK_MAX_MS}ms → click；超过 → longpress`, () => {
    expect(classifyGesture({ dxPx: 0, dyPx: 0, durationMs: CLICK_MAX_MS, ...F })).toBe('click')
    expect(classifyGesture({ dxPx: 3, dyPx: 4, durationMs: CLICK_MAX_MS, ...F })).toBe('click') // |5| < 8
    expect(classifyGesture({ dxPx: 0, dyPx: 0, durationMs: CLICK_MAX_MS + 1, ...F })).toBe('longpress')
    expect(classifyGesture({ dxPx: 0, dyPx: 0, durationMs: 3000, ...F })).toBe('longpress')
  })

  it('负方向/合成位移按模长判定', () => {
    expect(classifyGesture({ dxPx: -50, dyPx: 0, durationMs: 200, ...F })).toBe('swipe')
    expect(classifyGesture({ dxPx: 3, dyPx: -4, durationMs: 200, ...F })).toBe('click')
    expect(classifyGesture({ dxPx: 6, dyPx: -8, durationMs: 200, ...F })).toBe('swipe') // |10| > 8
  })

  it('比例阈值随帧尺寸变化：大帧等值位移可成滑动', () => {
    // 3840x2160 → 阈值 10.8
    const L = { frameW: 3840, frameH: 2160 }
    expect(classifyGesture({ dxPx: 10, dyPx: 0, durationMs: 100, ...L })).toBe('click')
    expect(classifyGesture({ dxPx: 11, dyPx: 0, durationMs: 100, ...L })).toBe('swipe')
  })
})
