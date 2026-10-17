import { describe, expect, it } from 'vitest'
import { readCommonSchedule, writeCommonSchedule } from './components/task/schedule-ui'

describe('player schedule controls preserve the cron contract', () => {
  it.each([
    ['0 8 * * *', 'daily', '08:00'],
    ['15 21 * * 1-5', 'weekdays', '21:15'],
    ['0 0 8 * * 0,6', 'weekends', '08:00'],
  ])('reads %s without changing its original value', (expression, kind, time) => {
    expect(readCommonSchedule(expression)).toEqual({ kind, time })
  })
  it.each(['*/30 * * * *', '0 9 * * 1', '5 0 8 * * *', '60 8 * * *', '0 24 * * *', '0 8 1 * *', '@daily'])('keeps %s in the custom editor', expression => {
    expect(readCommonSchedule(expression)).toEqual({ kind: 'custom' })
  })
  it.each(['daily', 'weekdays', 'weekends'])('roundtrips %s at midnight and at the end of the day', kind => {
    for (const time of ['00:00', '23:59']) expect(readCommonSchedule(writeCommonSchedule(kind, time))).toEqual({ kind, time })
  })
  it.each(['', '24:00', '08:60', '8:00', 'invalid'])('does not invent a time for %s', time => {
    expect(writeCommonSchedule('daily', time)).toBeNull()
  })
})
