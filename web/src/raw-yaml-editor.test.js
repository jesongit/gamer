import { describe, expect, it, vi } from 'vitest'
import { useRawYamlEditor } from '../../plugins/gamer-yaml/ui/src/composables/useRawYamlEditor'
import { load } from 'js-yaml'

const SCRIPT = 'steps:\n  - log: hello\n'
const FUNCTIONS = 'login:\n  steps: []\n'

describe('useRawYamlEditor', () => {
  it.each([false, true])('退出后的原文请求不会覆盖新会话（旧请求失败：%s）', async failed => {
    let finish, fail
    const api = { getScript: vi.fn().mockImplementationOnce(() => new Promise((resolve, reject) => { finish = resolve; fail = reject }))
      .mockResolvedValue({ id: 'pkg/new.yaml', content: 'run: []', version: 'new' }) }
    const editor = useRawYamlEditor({ api })
    const old = editor.load('script', 'pkg/old.yaml')
    editor.reset()
    await editor.load('script', 'pkg/new.yaml')
    if (failed) fail(new Error('old failed'))
    else finish({ id: 'pkg/old.yaml', content: 'run: []', version: 'old' })
    expect(await old).toBeNull()
    expect(editor.resourceId.value).toBe('pkg/new.yaml')
    expect(editor.version.value).toBe('new')
  })
  const library = 'functions:\n  first:\n    description: 保留的函数\n    run:\n      - log: sibling\n  chosen:\n    run:\n      - log: selected\n'
  function scopedEditor() {
    const api = {
      getFunction: vi.fn(async () => ({ id: 'pkg/_function.yaml', content: library, version: 'f1' })),
      updateFunction: vi.fn(async () => ({ id: 'pkg/_function.yaml', version: 'f2' })),
    }
    return { api, editor: useRawYamlEditor({ api }) }
  }
  it('选中第二个函数时只编辑该函数，保存/重命名合并回原库且保留其他函数', async () => {
    const { api, editor } = scopedEditor()
    await editor.load('function', 'pkg/_function.yaml', { functionName: 'chosen' })
    expect(Object.keys(load(editor.content.value).functions)).toEqual(['chosen'])
    editor.content.value = 'functions:\n  renamed:\n    run:\n      - log: edited\n'
    expect(await editor.save()).toMatchObject({ ok: true, functionName: 'renamed' })
    const payload = api.updateFunction.mock.calls[0][1]
    expect(payload.expected_version).toBe('f1')
    expect(load(payload.content).functions).toEqual({ first: load(library).functions.first, renamed: { run: [{ log: 'edited' }] } })
  })
  it.each(['functions: {}', 'functions:\n  first:\n    run: []\n', 'functions:\n  chosen:\n    run: []\n  added:\n    run: []\n', 'functions: ['])('单函数原文拒绝删除、覆盖其他函数、多函数和坏语法：%s', async content => {
    const { api, editor } = scopedEditor()
    await editor.load('function', 'pkg/_function.yaml', { functionName: 'chosen' })
    editor.content.value = content
    expect((await editor.save()).ok).toBe(false)
    expect(api.updateFunction).not.toHaveBeenCalled()
    expect(editor.dirty.value).toBe(true)
  })
  it('函数原文遇到并发版本冲突时保留草稿，不强制覆盖整库', async () => {
    const { api, editor } = scopedEditor()
    await editor.load('function', 'pkg/_function.yaml', { functionName: 'chosen' })
    editor.content.value = editor.content.value.replace('selected', 'edited')
    api.updateFunction.mockRejectedValue(Object.assign(new Error('conflict'), { status: 409, data: { code: 'version_conflict' } }))
    expect((await editor.save()).reason).toBe('conflict')
    expect(editor.dirty.value).toBe(true)
    expect(editor.version.value).toBe('f1')
    expect(api.updateFunction.mock.calls[0][1]).not.toHaveProperty('force')
  })
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
