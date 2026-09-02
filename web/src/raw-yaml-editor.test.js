import { describe, expect, it, vi } from 'vitest'
import { useRawYamlEditor } from './composables/useRawYamlEditor'

const SCRIPT = 'steps:\n  - log: hello\n'
const FUNCTIONS = 'login:\n  steps: []\n'

describe('useRawYamlEditor', () => {
  it('脚本原文保持原样并携带版本保存', async () => {
    const api = {
      getScript: vi.fn(async () => ({ id: 'pkg/main.yaml', content: SCRIPT, version: 'v1' })),
      updateScript: vi.fn(async (id, payload) => ({ id, version: 'v2', payload })),
    }
    const editor = useRawYamlEditor({ api })

    await editor.load('script', 'pkg/main.yaml')
    expect(editor.content.value).toBe(SCRIPT)
    expect(editor.dirty.value).toBe(false)

    editor.content.value = 'steps:\n  - log: 修复后的原文\n'
    const result = await editor.save()

    expect(result.ok).toBe(true)
    expect(api.updateScript).toHaveBeenCalledWith('pkg/main.yaml', {
      content: 'steps:\n  - log: 修复后的原文\n',
      expected_version: 'v1',
    })
    expect(editor.version.value).toBe('v2')
    expect(editor.dirty.value).toBe(false)
  })

  it('函数库原文保存走函数更新接口，语法错误由服务端诊断返回', async () => {
    const error = new Error('invalid yaml')
    error.status = 400
    error.data = { diagnostics: [{ message: 'YAML 解析失败' }] }
    const api = {
      getFunction: vi.fn(async () => ({ id: 'pkg/common.yaml', content: FUNCTIONS, version: 'f1' })),
      updateFunction: vi.fn(async () => { throw error }),
    }
    const editor = useRawYamlEditor({ api })

    await editor.load('function', 'pkg/common.yaml')
    editor.content.value = 'login:\n  steps: [\n'
    const result = await editor.save()

    expect(api.updateFunction).toHaveBeenCalledWith('pkg/common.yaml', {
      content: 'login:\n  steps: [\n',
      expected_version: 'f1',
    })
    expect(result).toMatchObject({ ok: false, reason: 'invalid', diagnostics: [{ message: 'YAML 解析失败' }] })
    expect(editor.dirty.value).toBe(true)
  })
})
