import { describe, expect, it } from 'vitest'
import { selectionToDeviceRect, toDeviceCoord } from './console/geometry'

describe('Console geometry helpers', () => {
  it('preserves contain mapping and clips selections to device bounds', () => {
    const rect = { left: 10, top: 20, width: 1000, height: 1000 }
    expect(toDeviceCoord(510, 520, rect, 1920, 1080)).toEqual({ x: 960, y: 540 })
    expect(selectionToDeviceRect({ x: -100, y: 200 }, { x: 1100, y: 800 }, rect, 1920, 1080)).toEqual({
      x: 0,
      y: 0,
      w: 1920,
      h: 1080,
    })
  })
  it('maps manual pointer edges to valid pixels while region edges stay exclusive', () => {
    const rect = { left: 10, top: 20, width: 960, height: 540 }
    expect(toDeviceCoord(970, 560, rect, 1920, 1080)).toEqual({ x: 1919, y: 1079 })
    // Rounding to width/height can happen even slightly inside the image.
    expect(toDeviceCoord(969.9, 559.9, rect, 1920, 1080)).toEqual({ x: 1919, y: 1079 })
    expect(toDeviceCoord(2000, 2000, rect, 1920, 1080)).toEqual({ x: 1919, y: 1079 })
    expect(toDeviceCoord(-100, -100, rect, 1920, 1080)).toEqual({ x: 0, y: 0 })
    expect(selectionToDeviceRect({ x: 0, y: 0 }, { x: 960, y: 540 }, rect, 1920, 1080))
      .toEqual({ x: 0, y: 0, w: 1920, h: 1080 })
  })
})
