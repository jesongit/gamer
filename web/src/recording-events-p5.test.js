import { describe, expect, it } from 'vitest'
import {
  filterRecordingEvents,
  formatRecordingEventTime,
  normalizeRecordingEvent,
} from './components/video/recordingEvents'

describe('P5 recordingEvents UI model', () => {
  const events = [
    { event_id: 'evt-2', source: 'runner', kind: 'key', timeline_us: 1250000, payload: { code: 66 }, status: 'accepted' },
    { event_id: 'evt-1', source: 'manual', kind: 'tap', timeline_us: 100000, payload: { x: 1, y: 2 }, status: 'rejected' },
  ]

  it('归一化事件不推断缺失字段，并格式化会话微秒时间轴', () => {
    const item = normalizeRecordingEvent(events[0])
    expect(item).toMatchObject({ eventId: 'evt-2', source: 'runner', kind: 'key', status: 'accepted' })
    expect(item.timelineLabel).toBe('0:01.250')
    expect(formatRecordingEventTime('bad')).toBe('时间未知')
  })

  it('按来源/状态/搜索筛选真实事件，保留拒绝事件的诊断可见性', () => {
    expect(filterRecordingEvents(events, { source: 'runner' })).toHaveLength(1)
    expect(filterRecordingEvents(events, { status: 'rejected' })[0].eventId).toBe('evt-1')
    expect(filterRecordingEvents(events, { search: 'tap' })[0].eventId).toBe('evt-1')
  })
})

