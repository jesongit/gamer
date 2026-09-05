// gamer-keymap-extension.js（gamer.keymap 扩展 id 唯一前端配置点）的行为契约：
// 远端映射运行态判定是输入路由开关（running → 输入交 keymap 控制器，否则直通
// scrcpy）；扩展禁用/卸载/未安装/轮询异常时恒 false，输入自动回落，无需壳清理。
import { describe, expect, it } from 'vitest'
import { GAMER_KEYMAP_EXTENSION_ID, isRemoteKeymapRunning } from './gamer-keymap-extension'

describe('gamer.keymap 扩展配置点（远端映射运行态判定）', () => {
  it('注册 id 与服务端扩展 id 一致', () => {
    expect(GAMER_KEYMAP_EXTENSION_ID).toBe('gamer.keymap')
  })

  it('running → true；disabled/stopped 等其余状态 → false', () => {
    expect(isRemoteKeymapRunning([{ id: 'gamer.keymap', state: 'running' }])).toBe(true)
    expect(isRemoteKeymapRunning([{ id: 'gamer.keymap', state: 'stopped' }])).toBe(false)
    expect(isRemoteKeymapRunning([{ id: 'gamer.keymap', state: 'disabled' }])).toBe(false)
  })

  it('扩展缺失（未安装/已卸载）或快照异常 → false（输入直通，不抛错）', () => {
    expect(isRemoteKeymapRunning([])).toBe(false)
    expect(isRemoteKeymapRunning([{ id: 'gamer.yaml', state: 'running' }])).toBe(false)
    expect(isRemoteKeymapRunning(undefined)).toBe(false)
    expect(isRemoteKeymapRunning('nope')).toBe(false)
  })
})
