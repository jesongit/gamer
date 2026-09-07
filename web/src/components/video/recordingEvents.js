/**
 * 录制事件 ↔ 媒体时间轴对齐（Phase 6，计划 §9.2）。
 *
 * 事件 `timeline_us` 是**会话单调钟**域；视频帧 PTS 是**媒体时钟**域。两者
 * 的整数映射由录制服务端显式化：`SegmentMeta.base_pts_us`（段首帧原始 PTS），
 * `media_pts = base_pts_us + timeline_us − start_us`（对应 Rust 侧
 * `media_pts_for_timeline`，全程整数微秒，不混用浏览器时间）。
 *
 * 会话可能分段（断连/编码参数变化/磁盘压力）：先按 `[start_us, start_us +
 * duration_us)` 找段，再段内换算；落不进任何段的事件标注 `unmapped`（可诊断，
 * 不伪造位置）。外部视频无 recording 引用 → 正常制作、不伪造操作日志。
 * 全部纯函数，无 Vue 依赖。
 */

const SOURCE_LABELS = {
  manual: '手动',
  keymap: '键映射',
  runner: '脚本',
  plugin: '插件',
}

/** timeline_us（会话单调钟）→ 所在段；不存在（间隙/越界）返回 null。 */
export function segmentForTimeline(segments, timelineUs) {
  if (!Array.isArray(segments) || !segments.length) return null
  const t = Number(timelineUs)
  if (!Number.isFinite(t)) return null
  return segments.find(segment => {
    const start = Number(segment?.start_us) || 0
    const duration = Math.max(0, Number(segment?.duration_us) || 0)
    return t >= start && (t < start + duration || (duration === 0 && t === start))
  }) || null
}

/** 单个事件 → 媒体帧定位 `{media_id, pts_us, segment}`；映射不到返回 null。 */
export function eventMediaPosition(event, segments) {
  const segment = segmentForTimeline(segments, event?.timeline_us)
  if (!segment) return null
  const start = Number(segment.start_us) || 0
  const base = Number(segment.base_pts_us) || 0
  const timelineUs = Math.max(0, Math.round(Number(event.timeline_us) || 0))
  return {
    media_id: segment.media_id,
    // 与 Rust `SegmentMeta::media_pts_for_timeline` 同一整数映射
    pts_us: base + Math.max(0, timelineUs - start),
    segment,
  }
}

/** 防御性时间轴升序（服务端合同升序，客户端不信任输入顺序）。 */
export function sortEvents(events) {
  return [...(Array.isArray(events) ? events : [])].sort(
    (a, b) => (Number(a?.timeline_us) || 0) - (Number(b?.timeline_us) || 0),
  )
}

/**
 * 事件集合 → 时间轴视图模型：附媒体帧定位与 `unmapped` 诊断。
 * 只保留服务端已接受的输入（防御性：理论上事件流只含 accepted）。
 */
export function alignEvents(events, segments) {
  return sortEvents(events)
    .filter(event => String(event?.status ?? 'accepted') === 'accepted')
    .map(event => {
      const position = eventMediaPosition(event, segments)
      return {
        event,
        mediaId: position?.media_id || null,
        ptsUs: position?.pts_us ?? null,
        unmapped: !position,
        sourceLabel: SOURCE_LABELS[event?.source] || String(event?.source || '未知'),
      }
    })
}

/** 事件来源展示名。 */
export function eventSourceLabel(source) {
  return SOURCE_LABELS[source] || String(source || '未知')
}

/** 事件负载的一行摘要（tap/swipe/key/text/wait）。 */
export function eventSummary(event) {
  const payload = event?.payload || {}
  switch (event?.kind) {
    case 'tap': return `tap (${Math.round(Number(payload.x) || 0)}, ${Math.round(Number(payload.y) || 0)})`
    case 'swipe': return `swipe (${Math.round(Number(payload.x) || 0)}, ${Math.round(Number(payload.y) || 0)}) → (${Math.round(Number(payload.x2) || 0)}, ${Math.round(Number(payload.y2) || 0)})`
    case 'key': return `key ${payload.code ?? ''}`
    case 'text': return `text（长度 ${payload.length ?? '?'}，已脱敏）`
    case 'wait': return `wait ${(Number(payload.duration_us) || 0) / 1e6}s`
    default: return String(event?.kind || '未知动作')
  }
}
