// @vitest-environment happy-dom
/**
 * useConsoleKeymap 适配 P11.6 通用资源 API 后的 keymap 面板运行时契约：
 * - 列表返回摘要条目（name/binding_count/valid + content 原文），面板原样透传；
 * - 选中方案经 GET 资源条目（不再携带解析模型）→ 前端按需把 content YAML
 *   解析为输入控制器/可视化消费的 {name, bindings} 模型；
 * - 注记 valid=false（服务端 schema 校验失败）→ 报错并清空选择，不把坏方案
 *   装进输入链路。
 */
import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { useConsoleKeymap } from './components/console/useConsoleKeymap'

const KEYMAP_YAML = [
  'version: 1',
  'name: 战斗方案',
  'bindings:',
  '  - key: KeyW',
  '    action:',
  '      type: hold',
  '      at: [0.5, 0.5]',
  '  - key: Space',
  '    action:',
  '      type: raw_key',
  '      code: Home',
].join('\n')

const SUMMARY_ENTRY = {
  id: 'com.demo/battle.yaml',
  package: 'com.demo',
  pkg: 'com.demo',
  name: '战斗方案',
  file: 'battle',
  content: KEYMAP_YAML,
  version: 'a1b2c3d4e5f6',
  binding_count: 2,
  valid: true,
}

function makeDeps(apiOverrides = {}) {
  return {
    api: {
      listKeymaps: vi.fn(async () => [SUMMARY_ENTRY]),
      getKeymap: vi.fn(async () => SUMMARY_ENTRY),
      ...apiOverrides,
    },
    toast: vi.fn(),
    activePkg: ref('com.demo'),
    keyboardMode: ref('game'),
    keymap: { releaseAll: vi.fn(), setEnabled: vi.fn(), handleInputEvent: vi.fn(), handleKeyDown: vi.fn(), handleKeyUp: vi.fn(), getPressedCodes: () => [] },
    keymapPressed: new Set(),
    videoElement: ref(null),
    videoWrap: ref(null),
    deviceRectStyle: () => ({}),
    pickCoord: async () => ({ x: 0.5, y: 0.5 }),
  }
}

describe('useConsoleKeymap（通用资源 API 摘要形态适配）', () => {
  it('列表透传摘要条目（binding_count/valid），不要求解析模型', async () => {
    const deps = makeDeps()
    const km = useConsoleKeymap(deps)
    await km.loadKeymaps('com.demo')
    expect(deps.api.listKeymaps).toHaveBeenCalledWith('com.demo')
    expect(km.keymaps.value).toHaveLength(1)
    expect(km.keymaps.value[0].binding_count).toBe(2)
    expect(km.keymaps.value[0].valid).toBe(true)
  })

  it('选中方案：GET 资源条目按 content 解析出 bindings 模型供输入链路消费', async () => {
    const deps = makeDeps()
    const km = useConsoleKeymap(deps)
    await km.loadKeymaps('com.demo')
    await km.onKeymapChange(km.keymaps.value[0])
    expect(deps.api.getKeymap).toHaveBeenCalledWith('com.demo/battle.yaml', 'com.demo')
    expect(km.activeKeymapModel.value).toMatchObject({
      name: '战斗方案',
      bindings: [
        { key: 'KeyW', action: { type: 'hold', at: [0.5, 0.5] } },
        { key: 'Space', action: { type: 'raw_key', code: 'Home' } },
      ],
    })
    expect(km.keymapStatus.value).toMatchObject({ name: '战斗方案', inactive: false })
  })

  it('无效方案（valid=false）：带诊断报错并清空选择，不产生活跃模型', async () => {
    const deps = makeDeps({
      getKeymap: vi.fn(async () => ({
        ...SUMMARY_ENTRY,
        valid: false,
        diagnostics: ['bindings 必须是数组'],
      })),
    })
    const km = useConsoleKeymap(deps)
    await km.loadKeymaps('com.demo')
    await km.onKeymapChange(km.keymaps.value[0])
    expect(km.activeKeymapName.value).toBe('')
    expect(km.activeKeymapModel.value).toBeNull()
    expect(km.keymapError.value).toContain('映射方案无效')
    expect(km.keymapError.value).toContain('bindings 必须是数组')
    expect(deps.toast).toHaveBeenCalled()
  })
})
