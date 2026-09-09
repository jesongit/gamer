/**
 * VideoDraft 专属的录制事件工作流模型。
 *
 * 这里只处理前端展示/筛选/选择，不复制服务端的 YAML 映射器，也不猜测
 * 未公开的事件语义。真正的草稿转换仍通过 automation.create_draft 完成。
 */

export const DRAFT_EVENT_KINDS = Object.freeze(['tap', 'swipe', 'key', 'wait'])

const UNSUPPORTED_REASONS = Object.freeze({
  text: 'text 事件只有脱敏长度，无法还原原文，不能安全生成 YAML 步骤',
})

function stableEventId(event) {
  const value = event?.event_id ?? event?.id
  return value === undefined || value === null ? '' : String(value).trim()
}

function safeJson(value) {
  if (value === undefined) return ''
  try {
    return JSON.stringify(value)
  } catch {
    return '[负载不可序列化]'
  }
}

export function draftEventKey(event, index = 0) {
  const id = stableEventId(event)
  return id || `row-${index}`
}

/**
 * 归一化仅用于 UI。`event_id` 缺失时不生成可提交的伪 ID，调用方应禁用该行。
 */
export function normalizeDraftEvent(event, index = 0) {
  const eventId = stableEventId(event)
  const kind = String(event?.kind || 'unknown')
  const source = String(event?.source || 'unknown')
  const status = String(event?.status || 'accepted')
  const timelineUs = Number(event?.timeline_us)
  const supported = DRAFT_EVENT_KINDS.includes(kind)
  const reason = eventId
    ? (UNSUPPORTED_REASONS[kind] || (!supported ? `事件类型「${kind}」没有 V1 草稿映射` : ''))
    : '事件缺少 event_id，不能安全提交到草稿生成接口'

  return {
    raw: event,
    event_id: eventId,
    event_key: draftEventKey(event, index),
    kind,
    source,
    status,
    timeline_us: timelineUs,
    payload: event?.payload,
    payload_text: safeJson(event?.payload),
    supported: supported && !!eventId,
    support_reason: reason,
  }
}

export function normalizeDraftEvents(events) {
  return (Array.isArray(events) ? events : []).map(normalizeDraftEvent)
}

/** 仅生成展示诊断；服务端返回的诊断仍由 VideoDraft 合并展示。 */
export function draftEventDiagnostics(events) {
  return normalizeDraftEvents(events)
    .filter(event => event.support_reason)
    .map(event => ({
      event_id: event.event_id || event.event_key,
      reason: event.support_reason,
      code: event.event_id ? 'event_kind_unsupported' : 'event_id_missing',
      source: 'client_preflight',
    }))
}

function asLower(value) {
  return String(value || '').trim().toLowerCase()
}

/**
 * 事件筛选只作用于视图，不改变已选顺序。from/to 使用 timeline_us 的秒数，
 * 以便与界面展示一致；空值代表不限制。
 */
export function filterDraftEvents(events, filters = {}) {
  const search = asLower(filters.search)
  const source = String(filters.source || '')
  const kind = String(filters.kind || '')
  const status = String(filters.status || '')
  const from = Number(filters.from)
  const to = Number(filters.to)
  const hasFrom = filters.from !== '' && Number.isFinite(from) && from >= 0
  const hasTo = filters.to !== '' && Number.isFinite(to) && to >= 0

  return normalizeDraftEvents(events).filter(event => {
    if (source && event.source !== source) return false
    if (kind && event.kind !== kind) return false
    if (status && event.status !== status) return false
    if (hasFrom && (!Number.isFinite(event.timeline_us) || event.timeline_us < from * 1e6)) return false
    if (hasTo && (!Number.isFinite(event.timeline_us) || event.timeline_us > to * 1e6)) return false
    if (!search) return true
    return [
      event.event_id,
      event.kind,
      event.source,
      event.status,
      event.payload_text,
    ].some(value => asLower(value).includes(search))
  })
}

export function uniqueDraftValues(events, field) {
  return [...new Set(normalizeDraftEvents(events).map(event => event[field]).filter(Boolean))]
}

export function validDraftEventId(event) {
  return !!stableEventId(event)
}
