// @vitest-environment node
/**
 * gamer.yaml 依赖能力判定（Phase 7 §10.1）：视频制作入口的门禁态。
 */
import { describe, expect, it } from 'vitest'
import { isGamerYamlRunning } from './components/video/yamlCapability'

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
