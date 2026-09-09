// @vitest-environment node
/**
 * gamer.yaml 依赖能力判定（简化计划 Phase 4/5）：视频制作入口的门禁态。
 * `isGamerYamlRunning` = 快照形态兜底；`useYamlCapability` = 能力发现端点
 * （Running + 公开动作清单，fetchCapabilities 注入便于单测）。
 */
import { describe, expect, it, vi } from 'vitest'
import { isGamerYamlRunning, useYamlCapability } from './components/video/yamlCapability'

describe('isGamerYamlRunning', () => {
  it('gamer.yaml running → true；其他状态/缺失 → false', () => {
    expect(isGamerYamlRunning([
      { id: 'gamer.video', state: 'running' },
      { id: 'gamer.yaml', state: 'running' },
    ])).toBe(true)
    for (const state of ['installed', 'enabled', 'stopped', 'disabled', 'failed']) {
      expect(isGamerYamlRunning([{ id: 'gamer.yaml', state }])).toBe(false)
    }
    expect(isGamerYamlRunning([{ id: 'gamer.keymap', state: 'running' }])).toBe(false)
    expect(isGamerYamlRunning([])).toBe(false)
    expect(isGamerYamlRunning(undefined)).toBe(false)
  })
})

describe('useYamlCapability：能力发现端点（Running + 公开动作清单）', () => {
  it('running 且动作在清单内 → ready；hasAction 按清单判定', async () => {
    const fetchCapabilities = vi.fn().mockResolvedValue({
      id: 'gamer.yaml',
      state: 'running',
      running: true,
      actions: [{ action: 'template.create_from_frame', version: 1, surface: 'native' }],
    })
    const capability = useYamlCapability({ fetchCapabilities })
    await capability.refresh()
    expect(fetchCapabilities).toHaveBeenCalledTimes(1)
    expect(capability.ready.value).toBe(true)
    expect(capability.hasAction('template.create_from_frame')).toBe(true)
    expect(capability.hasAction('automation.create_draft')).toBe(false)
  })

  it('未安装（404 兜底形态）→ ready=false；探测失败保持上一次状态', async () => {
    const fetchCapabilities = vi.fn()
      .mockResolvedValueOnce({ id: 'gamer.yaml', running: false, actions: [] })
      .mockRejectedValueOnce(new Error('network'))
    const capability = useYamlCapability({ fetchCapabilities })
    await capability.refresh()
    expect(capability.ready.value).toBe(false)
    expect(capability.hasAction('template.create_from_frame')).toBe(false)

    // 网络抖动：不闪断，保持上一次的判定
    await capability.refresh()
    expect(capability.ready.value).toBe(false)
  })
})
