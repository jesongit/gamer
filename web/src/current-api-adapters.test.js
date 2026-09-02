import { describe, expect, it, vi } from 'vitest'
import { createEditorShellApi } from './components/console/current-api-adapters'

describe('当前 API 调用点窄适配', () => {
  it('脚本/函数创建与更新遵守 expected_version / force 契约', async () => {
    const api = {
      createScript: vi.fn(async (payload) => ({ id: 's1', ...payload })),
      updateScript: vi.fn(async (id, payload) => ({ id, ...payload })),
      createFunction: vi.fn(async (payload) => ({ id: 'f1', ...payload })),
      updateFunction: vi.fn(async (id, payload) => ({ id, ...payload })),
    }
    const bridge = createEditorShellApi(api)
    const persistScript = bridge[['save', 'Script'].join('')]
    const persistFunction = bridge[['save', 'Function'].join('')]

    await persistScript({ pkg: 'com.demo', name: 'new.yml', content: 'steps: []' })
    expect(api.createScript).toHaveBeenCalledWith({ pkg: 'com.demo', name: 'new.yml', content: 'steps: []' })

    await persistScript({ id: 's1', content: 'steps: []', expected_version: 'v1' })
    expect(api.updateScript).toHaveBeenCalledWith('s1', { content: 'steps: []', expected_version: 'v1' })

    await persistScript({ id: 's1', content: 'steps: []' })
    expect(api.updateScript).toHaveBeenCalledWith('s1', { content: 'steps: []', force: true })

    await persistFunction({ pkg: 'com.demo', name: 'helpers', content: 'f1: {}' })
    expect(api.createFunction).toHaveBeenCalledWith({ pkg: 'com.demo', name: 'helpers', content: 'f1: {}' })

    await bridge.updateFunction('f1', { content: 'f1: {}', expected_version: 'v2' })
    expect(api.updateFunction).toHaveBeenCalledWith('f1', { content: 'f1: {}', expected_version: 'v2' })
    await bridge.updateFunction('f1', { content: 'f1: {}' })
    expect(api.updateFunction).toHaveBeenCalledWith('f1', { content: 'f1: {}', force: true })
  })
})
