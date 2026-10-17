// Common schedules are a view over the existing cron expression. Unrecognized
// expressions stay in the custom editor and are never rewritten on load/save.
const DAYS = { daily: '*', weekdays: '1-5', weekends: '0,6' }
export function readCommonSchedule(expression) {
  const parts = String(expression || '').trim().split(/\s+/)
  if (parts.length === 6 && parts[0] === '0') parts.shift()
  if (parts.length !== 5 || parts[2] !== '*' || parts[3] !== '*') return { kind: 'custom' }
  if (!/^\d{1,2}$/.test(parts[0]) || !/^\d{1,2}$/.test(parts[1])) return { kind: 'custom' }
  const minute = Number(parts[0]), hour = Number(parts[1])
  const kind = Object.keys(DAYS).find(key => DAYS[key] === parts[4])
  if (!kind || minute > 59 || hour > 23) return { kind: 'custom' }
  return { kind, time: `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}` }
}
export function writeCommonSchedule(kind, time) {
  const match = /^(\d{2}):(\d{2})$/.exec(String(time || ''))
  if (!DAYS[kind] || !match || Number(match[1]) > 23 || Number(match[2]) > 59) return null
  return `${Number(match[2])} ${Number(match[1])} * * ${DAYS[kind]}`
}
