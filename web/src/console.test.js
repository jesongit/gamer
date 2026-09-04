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
})
