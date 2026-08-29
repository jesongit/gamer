// 定时任务参数化（阶段 5，plan §12.3）语义工具：纯函数，vitest 直测。
// 服务端契约：创建/更新接受稀疏 args（显式覆盖映射）→ 服务端解析为完整快照存储并计算
// param_signature；列表响应含 args 视图 / param_signature / param_stale；签名不匹配且无
// reconfirm:true → 409 {code:"param_signature_conflict", ...}，带 reconfirm 按当前声明重算。

/**
 * 「参数已过期」对比表行（任务原快照 / 当前声明默认值 / 本次采用值）。
 * resolve_entry_args 语义：本次采用 = 快照值（覆盖态），除非用户改（含关掉覆盖 → 当前默认值）。
 * effectiveArgs 为 ParamsForm 的完整采用值视图；snapshotArgs 为任务存储快照（可能缺新参数）。
 */
export function staleCompareRows(params, snapshotArgs = {}, effectiveArgs = {}) {
  return params.map((p) => ({
    name: p.name,
    type: p.type,
    snapshot: Object.prototype.hasOwnProperty.call(snapshotArgs, p.name)
      ? snapshotArgs[p.name]
      : null,
    currentDefault: p.default,
    adopted: Object.prototype.hasOwnProperty.call(effectiveArgs, p.name)
      ? effectiveArgs[p.name]
      : p.default,
  }))
}

/** 是否任务参数签名冲突（契约：HTTP 409 + body.code === 'param_signature_conflict'）。 */
export function isParamSignatureConflict(e) {
  return !!e && e.status === 409 && e.data?.code === 'param_signature_conflict'
}

/** param_stale 任务的禁用原因（立即运行按钮 title / 列表提示）。 */
export function staleReason(t) {
  return `${t?.name || '该任务'} 的参数快照已过期：脚本参数声明已变化，请编辑任务确认参数后再运行`
}

/**
 * 任务保存 payload：基础字段 + 稀疏 args（仅显式覆盖；服务端解析补默认值成快照）。
 * reconfirm 为 true 时携带标记（服务端按当前声明重算签名，绕过 409）。
 */
export function buildTaskSavePayload(t, { reconfirm = false } = {}) {
  const args = t.args && typeof t.args === 'object' ? t.args : {}
  return {
    id: t.id ?? null,
    name: t.name,
    cron: t.cron,
    script_id: t.script_id,
    device_id: t.device_id,
    enabled: t.enabled,
    ...(Object.keys(args).length ? { args } : {}),
    ...(reconfirm ? { reconfirm: true } : {}),
  }
}
