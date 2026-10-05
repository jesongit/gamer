import { describe, expect, it, vi } from 'vitest'
import { useScriptEditorShell } from '../../plugins/gamer-yaml/ui/src/composables/useScriptEditorShell'
import { serialize } from '../../plugins/gamer-yaml/ui/src/script-editor/codec'

const oldName = '确认-差分宇宙#777_873_900_942.png'
const newName = '确定-差分宇宙#777_873_900_942.png'
const rename = { pkg: 'game', oldName, newName }
const source = `functions:
  当前函数:
    run:
      - log: 确认-差分宇宙.png
  进入差分宇宙:
    vars:
      other: other.png
    run:
      - match_templates:
          cases:
            - template: 确认-差分宇宙.png
              do:
                - wait_find:
                    template: ${oldName}
                    obstacles: [确认-差分宇宙.png, '$other']
          else:
            - find_any:
                templates: [确认-差分宇宙.png]
`
// 独立期望值：模拟宿主改写后的文件，不用被测改写函数生成夹具。
const renamedSource = source.replace('template: 确认-', 'template: 确定-')
  .replace(`template: ${oldName}`, 'template: 确定-差分宇宙.png')
  .replace('obstacles: [确认-', 'obstacles: [确定-')
  .replace('templates: [确认-', 'templates: [确定-')

async function fixture(content = source) {
  let disk = { id: 'game/_function.yaml', pkg: 'game', file: '_function.yaml', content, version: 'before' }
  let renamed = false
  const api = {
    getFunction: vi.fn(async () => ({ ...disk })),
    updateFunction: vi.fn(async (id, payload) => {
      if (payload.expected_version !== disk.version) {
        throw Object.assign(new Error('conflict'), { status: 409, data: { code: 'version_conflict', resource: id } })
      }
      disk = { ...disk, content: payload.content, version: 'saved' }
      return { ...disk }
    }),
  }
  const shell = useScriptEditorShell({ api, getContext: () => ({ resolveTemplate: name =>
    renamed ? name === '确定-差分宇宙.png' : name === oldName || name === '确认-差分宇宙.png',
  }) })
  await shell.loadFunctionFile(disk.id)
  const renameDisk = (content = renamedSource) => {
    disk = { ...disk, content, version: 'renamed' }
    renamed = true
  }
  return { api, shell, renameDisk, getDisk: () => disk }
}

describe('模板重命名同步正在编辑的函数库', () => {
  it('当前函数无引用时也同步隐藏函数，保留未保存内容、选择、模型及撤销栈并使用新版本保存', async () => {
    const { shell, api, renameDisk, getDisk } = await fixture()
    const model = shell.model, stack = shell.stack, fn = model.functions[0]
    shell.select(fn.run[0].uuid)
    stack.apply({ type: 'update_step', path: ['functions', '当前函数', 'run', 0],
      fields: { args: { kind: 'value', cell: { lit: '尚未保存' } } } })
    renameDisk()
    expect(shell.diagnostics.some(d => d.code === 'yaml.resource.tmpl_not_found')).toBe(true)
    await shell.onTemplateRenamed(rename)
    expect(shell.model).toBe(model)
    expect(shell.stack).toBe(stack)
    expect(shell.model.functions[0]).toBe(fn)
    expect(shell.selectedUuid).toBe(fn.run[0].uuid)
    expect(shell.diagnostics).toEqual([])
    expect(shell.dirty).toBe(true)
    expect(shell.version).toBe('renamed')
    expect(shell.undo()).toBe(true)
    expect(shell.dirty).toBe(false)
    expect(serialize(model)).toContain('log: 确认-差分宇宙.png')
    expect(shell.redo()).toBe(true)
    expect((await shell.save()).ok).toBe(true)
    expect(api.updateFunction.mock.calls[0][1].expected_version).toBe('renamed')
    expect(getDisk().content).toContain('log: 尚未保存')
    expect(getDisk().content).toContain('template: 确定-差分宇宙.png')
    expect(getDisk().content).not.toContain('template: 确认-')
    expect(shell.model).toBe(model)
    expect(shell.stack).toBe(stack)
  })

  it('撤销删除和重做参数修改都不能复活旧模板，连续重命名仍指向最新名字', async () => {
    const { shell, renameDisk } = await fixture()
    shell.stack.apply({ type: 'update_template_case', path: ['functions', '进入差分宇宙', 'run', 0],
      index: 0, fields: { template: { lit: oldName } } })
    shell.stack.apply({ type: 'remove_step', path: ['functions', '进入差分宇宙', 'run'], index: 0 })
    renameDisk()
    await shell.onTemplateRenamed(rename)
    shell.undo()
    shell.undo()
    expect(serialize(shell.model)).toContain('template: 确定-差分宇宙.png')
    shell.redo()
    expect(serialize(shell.model)).not.toContain('template: 确认-')
    renameDisk(renamedSource.replaceAll('确定-', '新确定-'))
    await shell.onTemplateRenamed({ ...rename, oldName: newName, newName: '新确定-差分宇宙.png' })
    shell.undo()
    expect(serialize(shell.model)).toContain('template: 新确定-差分宇宙.png')
    expect(serialize(shell.model)).not.toContain('template: 确定-')
  })

  it('不自动接纳其他页面的额外修改，保留未保存内容并继续用版本冲突保护磁盘', async () => {
    const { shell, renameDisk, getDisk } = await fixture()
    shell.stack.apply({ type: 'update_step', path: ['functions', '当前函数', 'run', 0],
      fields: { args: { kind: 'value', cell: { lit: '本页修改' } } } })
    renameDisk(renamedSource.replace('log: 确认-差分宇宙.png', 'log: 别的页面修改'))
    await shell.onTemplateRenamed(rename)
    expect(shell.version).toBe('before')
    expect((await shell.save()).reason).toBe('conflict')
    expect(getDisk().content).toContain('log: 别的页面修改')
    expect(serialize(shell.model)).toContain('log: 本页修改')
  })

  it('保存时自动恢复另一页面改名导致的旧引用，保留当前修改并使用已核对的新版本', async () => {
    const { shell, api, renameDisk, getDisk } = await fixture()
    const model = shell.model, stack = shell.stack
    stack.apply({ type: 'update_step', path: ['functions', '当前函数', 'run', 0],
      fields: { args: { kind: 'value', cell: { lit: '改名前的未保存修改' } } } })
    renameDisk()
    expect(shell.diagnostics.some(d => d.code === 'yaml.resource.tmpl_not_found')).toBe(true)
    expect((await shell.save()).ok).toBe(true)
    expect(api.updateFunction.mock.calls[0][1].expected_version).toBe('renamed')
    expect(getDisk().content).toContain('改名前的未保存修改')
    expect(getDisk().content).toContain('template: 确定-差分宇宙.png')
    expect(shell.model).toBe(model)
    expect(shell.stack).toBe(stack)
    shell.undo()
    expect(serialize(model)).not.toContain('template: 确认-')
  })

  it('保存时不猜测缺失模板，也不把普通模板换选或其他页面额外编辑当成重命名', async () => {
    const { shell, api, renameDisk } = await fixture()
    renameDisk(renamedSource.replace('log: 确认-差分宇宙.png', 'log: 其他页面编辑'))
    expect((await shell.save()).reason).toBe('invalid')
    expect(api.updateFunction).not.toHaveBeenCalled()
    expect(shell.version).toBe('before')
    expect(serialize(shell.model)).toContain('template: 确认-')
  })

  it('迟到的版本响应不污染后来打开的新资源', async () => {
    const { shell, api, renameDisk } = await fixture()
    renameDisk()
    let finish
    api.getFunction.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const pending = shell.onTemplateRenamed(rename)
    await Promise.resolve()
    await Promise.resolve()
    shell.newScript({ pkg: 'other', name: 'new.yaml' })
    finish({ content: renamedSource, version: 'renamed' })
    await pending
    expect(shell.pkg).toBe('other')
    expect(shell.version).toBeNull()
    expect(shell.model.run).toEqual([])
  })

  it('重命名后网络失败仍改写模型并保留原版本，不强制覆盖；其他包重命名不触碰会话', async () => {
    const { shell, api, renameDisk } = await fixture()
    await shell.onTemplateRenamed({ ...rename, pkg: 'other' })
    expect(api.getFunction).toHaveBeenCalledTimes(1)
    renameDisk()
    api.getFunction.mockRejectedValueOnce(new Error('offline'))
    await expect(shell.onTemplateRenamed(rename)).rejects.toThrow('offline')
    expect(shell.diagnostics).toEqual([])
    expect(shell.version).toBe('before')
    expect((await shell.save()).reason).toBe('conflict')
  })

  it('脚本也同步短名、完整路径及分支内引用，变量和普通文本保持原意', async () => {
    const before = `run:\n  - repeat: 2\n    do:\n      - find: templates/${oldName}\n      - if: true\n        then:\n          - tap_template: $hit\n        else:\n          - wait_disappear: 确认-差分宇宙.png\n  - log: 确认-差分宇宙.png\n`
    const after = before.replace(`templates/${oldName}`, '确定-差分宇宙.png').replace('wait_disappear: 确认-', 'wait_disappear: 确定-')
    const api = { getScript: vi.fn().mockResolvedValueOnce({ id: 'game/a.yaml', name: 'a.yaml', package: 'game', content: before, version: 's1' })
      .mockResolvedValue({ content: after, version: 's2' }), saveScript: vi.fn(async () => ({ version: 's3' })) }
    const shell = useScriptEditorShell({ api })
    await shell.loadScript('game/a.yaml')
    expect(shell.parseDiags).toEqual([])
    await shell.onTemplateRenamed(rename)
    expect(shell.version).toBe('s2')
    expect(shell.dirty).toBe(false)
    expect(serialize(shell.model)).toContain('tap_template: $hit')
    expect(serialize(shell.model)).toContain('log: 确认-差分宇宙.png')
    expect(serialize(shell.model)).toContain('find: 确定-差分宇宙.png')
  })
})
