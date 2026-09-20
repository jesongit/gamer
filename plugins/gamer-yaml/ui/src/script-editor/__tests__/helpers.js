/**
 * 测试共享工具：剥离步骤 uuid（uuid 是浏览器内临时 ID，不参与 golden/序列化对比）。
 */
export function stripUuids(value) {
  if (Array.isArray(value)) return value.map(stripUuids)
  if (value && typeof value === 'object') {
    const out = {}
    for (const [k, v] of Object.entries(value)) {
      if (k === 'uuid') continue
      out[k] = stripUuids(v)
    }
    return out
  }
  return value
}
