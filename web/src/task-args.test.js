import { describe, expect, it } from 'vitest'
import { buildTaskSavePayload, isParamSignatureConflict, staleCompareRows, staleReason } from './task-args'
import { extractParams } from './script-editor/params'

/**
 * 定时任务参数化语义工具（plan §12.3）：对比表行（任务原快照/当前默认值/本次采用值）、
 * 409 param_signature_conflict 判定、保存 payload（稀疏 args + reconfirm）。
 */

const SCRIPT_YAML = [
  'params:',
  "  - 'bool:enable:开关:true'",
  "  - 'time:timeout:最长等待:30s'",
  "  - 'tmpl:account:账号模板'",
  'steps:',
  '  - log: hi',
].join('\n')
const decls = extractParams(SCRIPT_YAML)

describe('staleCompareRows', () => {
  it('三列对比：快照值 / 当前默认值 / 本次采用（覆盖态=表单值，默认态=当前默认）', () => {
    const rows = staleCompareRows(
      decls,
      { enable: false, timeout: '10s' },                 // 任务原快照（缺 account——脚本后加的必填）
      { enable: false, timeout: '30s', account: 'a.png' }, // 表单采用值视图
    )
    expect(rows).toEqual([
      { name: 'enable', type: 'bool', snapshot: false, currentDefault: true, adopted: false },
      { name: 'timeout', type: 'time', snapshot: '10s', currentDefault: '30s', adopted: '30s' },
      { name: 'account', type: 'tmpl', snapshot: null, currentDefault: null, adopted: 'a.png' },
    ])
  })

  it('快照/采用缺省时回退 null 与声明默认值', () => {
    const rows = staleCompareRows(decls)
    expect(rows[0]).toEqual({ name: 'enable', type: 'bool', snapshot: null, currentDefault: true, adopted: true })
    expect(rows[2].adopted).toBeNull()
  })
})

describe('isParamSignatureConflict / staleReason', () => {
  it('仅 409 + code=param_signature_conflict 命中', () => {
    expect(isParamSignatureConflict({ status: 409, data: { code: 'param_signature_conflict' } })).toBe(true)
    expect(isParamSignatureConflict({ status: 409, data: { error: 'device_busy' } })).toBe(false)
    expect(isParamSignatureConflict({ status: 400 })).toBe(false)
    expect(isParamSignatureConflict(null)).toBe(false)
  })

  it('staleReason 含任务名与处置指引', () => {
    expect(staleReason({ name: '每日签到' })).toContain('每日签到')
    expect(staleReason({ name: '每日签到' })).toContain('确认参数')
  })
})

describe('buildTaskSavePayload', () => {
  it('携带稀疏 args；空 args 省略；reconfirm 显式置位', () => {
    expect(buildTaskSavePayload({
      id: 't1', name: '签到', cron: '0 8 * * *', script_id: 'com.a/a.yaml',
      device_id: 'dev1', enabled: true, args: { timeout: '10s' },
    })).toEqual({
      id: 't1', name: '签到', cron: '0 8 * * *', script_id: 'com.a/a.yaml',
      device_id: 'dev1', enabled: true, args: { timeout: '10s' },
    })
    expect(buildTaskSavePayload({ id: 't1', name: 'x', cron: '', script_id: '', device_id: '', args: {} }))
      .toEqual({ id: 't1', name: 'x', cron: '', script_id: '', device_id: '', enabled: undefined })
    expect(buildTaskSavePayload({ id: 't1', name: 'x', cron: '', script_id: '', device_id: '' }, { reconfirm: true }))
      .toMatchObject({ reconfirm: true })
  })
})
