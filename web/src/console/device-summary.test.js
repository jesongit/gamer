import { describe, expect, it } from 'vitest'
import { formatScreenSummary } from './device-summary'

describe('formatScreenSummary', () => {
  it('formats virtual and mirrored devices', () => {
    expect(formatScreenSummary(null)).toBe('—')
    expect(formatScreenSummary({ screen_mode: 'virtual', vd_res: '1080x1920', vd_dpi: 0 })).toBe('🖥️ 虚拟屏 · 1080x1920 · DPI 自动')
    expect(formatScreenSummary({ screen_mode: 'virtual', vd_res: '1920x1080', vd_dpi: 420 })).toBe('🖥️ 虚拟屏 · 1920x1080 @420dpi')
    expect(formatScreenSummary({ screen_mode: 'mirror' })).toBe('🖥️ 镜像主屏')
  })
})
