import { describe, expect, it, vi } from 'vitest'
import { selectionToDeviceRect, toDeviceCoord } from './console/geometry'
import { runPartitionImport, summarizeImportReport } from './api'

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

// ---- 分区快照导入（服务端 ImportReport 契约：scripts/functions/templates 三类资源同构）----

// 按服务端形态构造报告；over 里只写需要覆盖默认值的桶
const report = (over = {}) => ({
  scripts: { add: [], overwrite: [], invalid: [], ...over.scripts },
  functions: { add: [], overwrite: [], invalid: [], ...over.functions },
  templates: { add: [], overwrite: [], invalid: [], ...over.templates },
})

// 组装 runPartitionImport 依赖：dry-run 返回 dry，confirm 返回 conf，弹窗默认确认
function makeDeps(dry, conf, overrides = {}) {
  const calls = { confirmMsgs: [], notifications: [] }
  const importScripts = vi.fn().mockResolvedValueOnce(dry).mockResolvedValueOnce(conf)
  const deps = {
    file: { name: 'com.demo.zip' },
    pkg: 'com.demo',
    importScripts,
    confirmDialog: msg => { calls.confirmMsgs.push(msg); return true },
    notify: (msg, type) => calls.notifications.push({ msg, type }),
    refresh: vi.fn(),
    ...overrides,
  }
  return { deps, calls, importScripts }
}

describe('summarizeImportReport', () => {
  it('合并三类资源桶；形态不符（未来端点再变）返回 null', () => {
    const s = summarizeImportReport(report({
      scripts: { add: ['yaml/a.yaml'], overwrite: ['yaml/dup.yaml'], invalid: [{ path: 'yaml/bad.yaml', reason: 'YAML 语法错误' }] },
      functions: { add: ['func/f.yaml'] },
      templates: { overwrite: ['tmpl/pic.png'] },
    }))
    expect(s.add).toEqual(['yaml/a.yaml', 'func/f.yaml'])
    expect(s.overwrite).toEqual(['yaml/dup.yaml', 'tmpl/pic.png'])
    expect(s.invalid).toEqual([{ path: 'yaml/bad.yaml', reason: 'YAML 语法错误' }])

    expect(summarizeImportReport(null)).toBeNull()
    expect(summarizeImportReport({ ok: true })).toBeNull()
    // 缺任一资源桶 / 缺 add/overwrite/invalid 字段 / invalid 条目缺 path 均视为契约不符
    expect(summarizeImportReport({ scripts: report().scripts, functions: report().functions })).toBeNull()
    expect(summarizeImportReport({ scripts: { add: [] }, functions: report().functions, templates: report().templates })).toBeNull()
    expect(summarizeImportReport(report({ scripts: { invalid: [{ reason: 'no path' }] } }))).toBeNull()
  })
})

describe('runPartitionImport', () => {
  it('正常导入：无覆盖无非法时不弹确认，confirm 落盘后刷新并播报统计', async () => {
    const { deps, calls, importScripts } = makeDeps(
      report({ scripts: { add: ['yaml/new.yaml'] } }),
      report({ scripts: { add: ['yaml/new.yaml'] } }),
    )
    const r = await runPartitionImport(deps)
    expect(r).toEqual({ ok: true, add: 1, overwrite: 0 })
    expect(importScripts).toHaveBeenCalledTimes(2)
    expect(importScripts).toHaveBeenNthCalledWith(2, deps.file, true, 'com.demo')
    expect(calls.confirmMsgs).toHaveLength(0)
    expect(deps.refresh).toHaveBeenCalledTimes(1)
    expect(calls.notifications.at(-1).type).toBe('success')
    expect(calls.notifications.at(-1).msg).toContain('新增 1 个')
  })

  it('覆盖弹二次确认：列出覆盖与新增文件名，确认后执行 confirm 导入', async () => {
    const { deps, calls, importScripts } = makeDeps(
      report({ scripts: { add: ['yaml/new.yaml'], overwrite: ['yaml/dup.yaml'] }, templates: { overwrite: ['tmpl/pic.png'] } }),
      report(),
    )
    const r = await runPartitionImport(deps)
    expect(r).toEqual({ ok: true, add: 0, overwrite: 0 })
    expect(calls.confirmMsgs).toHaveLength(1)
    expect(calls.confirmMsgs[0]).toContain('将覆盖 2 个同名文件')
    expect(calls.confirmMsgs[0]).toContain('yaml/dup.yaml')
    expect(calls.confirmMsgs[0]).toContain('tmpl/pic.png')
    expect(calls.confirmMsgs[0]).toContain('另将新增 1 个文件')
    expect(calls.confirmMsgs[0]).toContain('yaml/new.yaml')
    expect(importScripts).toHaveBeenNthCalledWith(2, deps.file, true, 'com.demo')
  })

  it('覆盖确认取消：不再发起 confirm 导入', async () => {
    const { deps, calls, importScripts } = makeDeps(report({ scripts: { overwrite: ['yaml/dup.yaml'] } }), report())
    deps.confirmDialog = () => false
    const r = await runPartitionImport(deps)
    expect(r).toEqual({ ok: false, cancelled: true })
    expect(importScripts).toHaveBeenCalledTimes(1)
    expect(deps.refresh).not.toHaveBeenCalled()
    expect(calls.notifications).toHaveLength(0)
  })

  it('invalid 阻止：dry-run 后直接终止，无确认入口、不落盘，列出非法文件与原因', async () => {
    const { deps, calls, importScripts } = makeDeps(
      report({ functions: { invalid: [{ path: 'func/bad.yaml', reason: '顶层结构不合规' }] } }),
      report(),
    )
    const r = await runPartitionImport(deps)
    expect(r).toEqual({ ok: false })
    expect(importScripts).toHaveBeenCalledTimes(1)
    expect(calls.confirmMsgs).toHaveLength(0)
    expect(deps.refresh).not.toHaveBeenCalled()
    expect(calls.notifications[0].type).toBe('error')
    expect(calls.notifications[0].msg).toContain('func/bad.yaml')
    expect(calls.notifications[0].msg).toContain('顶层结构不合规')
  })

  it('防御：dry-run 响应缺预期字段时按错误终止，不得当作无冲突直接导入', async () => {
    const { deps, calls, importScripts } = makeDeps({ ok: true }, report())
    const r = await runPartitionImport(deps)
    expect(r).toEqual({ ok: false })
    expect(importScripts).toHaveBeenCalledTimes(1)
    expect(calls.confirmMsgs).toHaveLength(0)
    expect(calls.notifications[0].type).toBe('error')
    expect(calls.notifications[0].msg).toContain('格式异常')
  })

  it('服务端 confirm 整体拒绝（400 error）时展示结构化错误信息', async () => {
    const importScripts = vi.fn()
      .mockResolvedValueOnce(report())                    // dry-run 干净
      .mockRejectedValueOnce(Object.assign(               // confirm 落盘被服务端整体拒绝
        new Error('导入被拒绝：1 个条目未通过浅校验（整体未写入）：func/bad.yaml（顶层结构不合规）'),
        { status: 400 },
      ))
    const { deps, calls } = makeDeps(report(), report(), { importScripts })
    const r = await runPartitionImport(deps)
    expect(r).toEqual({ ok: false })
    expect(calls.notifications[0].type).toBe('error')
    expect(calls.notifications[0].msg).toContain('导入被拒绝')
    expect(calls.notifications[0].msg).toContain('func/bad.yaml')
    expect(deps.refresh).not.toHaveBeenCalled()
  })
})
