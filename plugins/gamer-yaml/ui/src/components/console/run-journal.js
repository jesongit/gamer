// Build a step tree from persisted events; frame + path distinguishes nested calls and repeats.
export function buildRunTree(events, record) {
  const roots = [], stack = []
  const append = node => (stack.at(-1)?.children || roots).push(node)
  for (const event of events) {
    const frame = event.trace?.frame_id ?? 0
    if (event.ev === 'step_start') {
      const node = { ...event, kind: 'step', children: [], details: [], matches: [], logs: [], state: 'running' }
      // Capture the parent's iteration now; its final iteration is different after a run finishes.
      node.iteration = stack.at(-1)?.details.findLast(e => e.name === 'iteration')?.data.iteration
      append(node); stack.push(node)
    } else if (event.ev === 'step_end') {
      const index = stack.findLastIndex(n => n.path === event.path && (n.trace?.frame_id ?? 0) === frame)
      if (index >= 0) {
        const node = stack[index]
        node.state = event.ok ? 'success' : 'failed'
        node.error = event.error
        node.failure = event
        node.duration = Math.max(0, Date.parse(event.time) - Date.parse(node.time))
        stack.splice(index)
      }
    } else if (event.ev === 'vision') {
      if (stack.length) stack.at(-1).matches.push(event)
      else append({ ...event, kind: 'event' })
    } else if (event.ev === 'detail') {
      const node = stack.at(-1)
      if (node) (event.name === 'log' ? node.logs : node.details).push(event)
      else append({ ...event, kind: 'event' })
    } else if (event.ev === 'tap' || event.ev === 'swipe') {
      if (stack.length) stack.at(-1).details.push({ ...event, name: event.ev, data: event })
    } else if (event.ev !== 'call_start') append({ ...event, kind: 'event' })
  }
  if (record?.finished_at) {
    for (const node of stack) {
      node.state = record.state === 'cancelled' ? 'cancelled' : 'failed'
      node.error = record.error || '步骤执行中断'
      node.duration = Math.max(0, Date.parse(record.finished_at) - Date.parse(node.time))
    }
  }
  return roots
}

export const stateText = state => ({ starting: '准备中', running: '运行中', stopping: '停止中', success: '成功', failed: '失败', cancelled: '已停止' }[state] || state || '')
export function eventText(e) {
  if (e.ev === 'run_start') return '开始执行'
  if (e.ev === 'run_end') return e.ok ? '执行完成' : `执行失败：${e.error || '未知错误'}`
  if (e.ev === 'budget') return `运行终止：${e.kind}`
  if (e.name === 'log') return e.data?.message || ''
  if (e.ev === 'vision') return `${e.template} ${e.found ? '命中' : '未命中'}`
  return `${e.name || e.ev} ${JSON.stringify(e.data || {})}`
}
export const detailText = name => ({ click_delay: '点击延迟', arguments: '调用参数', effective_args: '实际参数（含默认值）', result: '返回值', branch: '分支选择', iteration: '循环进度', tap: '点击坐标', swipe: '滑动坐标' }[name] || name)
export const timeText = time => new Date(time).toLocaleTimeString('zh-CN', { hour12: false })

// The execution tree is data only: render siblings in one list, never nested cards.
export function flattenRunTree(tree) {
  const rows = [], pending = tree.map(node => ({ node, ancestors: [] })).reverse()
  while (pending.length) {
    const row = pending.pop(), { node, ancestors } = row
    const labels = ancestors.map(n => `${n.desc || n.path}${n.iteration ? ` · 第 ${n.iteration} 轮` : ''}`)
    const source = node.trace?.source?.function
    if (source && !labels.includes(source)) labels.push(source)
    if (node.iteration) labels.push(`第 ${node.iteration} 轮`)
    rows.push({ ...row, depth: ancestors.length, context: labels.join(' › ') })
    for (let i = (node.children?.length || 0) - 1; i >= 0; i--) {
      pending.push({ node: node.children[i], ancestors: [...ancestors, node] })
    }
  }
  return rows
}

export function isRunIssue(node) {
  return node.state === 'failed' || node.state === 'cancelled' || node.ok === false || node.ev === 'budget'
    || node.logs?.some(log => ['warn', 'error'].includes(log.data?.level))
    || (node.name === 'log' && ['warn', 'error'].includes(node.data?.level))
}

// The default list shows work performed, not every enclosing function/loop frame.
// Keep a container's own observations and failures without a failing descendant.
export function actionRunRows(rows) {
  const failuresBelow = new Set()
  for (const { node, ancestors } of rows) {
    if (node.state === 'failed' || node.state === 'cancelled' || node.ok === false || node.ev === 'budget') {
      for (const parent of ancestors) failuresBelow.add(parent.id)
    }
  }
  return rows.filter(({ node }) => node.kind !== 'step' || !node.children.some(child => child.kind === 'step')
    || node.logs.length || node.matches.length || node.details.some(detail => detail.name === 'branch')
    || (isRunIssue(node) && !failuresBelow.has(node.id)))
}

export function filterRunRows(rows, { query = '', issuesOnly = false, includeFlow = false } = {}) {
  const needle = query.trim().toLocaleLowerCase()
  // A search can find any record, including flow frames omitted from the default list.
  return (includeFlow || needle ? rows : actionRunRows(rows)).filter(({ node, context }) => {
    if (issuesOnly && !isRunIssue(node)) return false
    if (!needle) return true
    return [node.desc, node.path, node.error, context, node.kind === 'event' ? eventText(node) : '',
      ...(node.logs || []).map(log => log.data?.message), ...(node.matches || []).map(match => match.template),
      ...(node.details || []).map(detail => JSON.stringify(detail.data)),
    ].filter(Boolean).join(' ').toLocaleLowerCase().includes(needle)
  })
}

export function stepSummary(node) {
  const parts = [], decision = node.details.findLast(e => e.name === 'branch' || e.name === 'iteration')
  if (decision?.name === 'iteration') parts.push(`第 ${decision.data.iteration} / ${decision.data.total} 轮`)
  if (decision?.name === 'branch') {
    const selected = decision.data.selected
    parts.push(typeof selected === 'number' ? `模板分支 ${selected + 1}` : ({ then: '条件成立', else: '否则分支' }[selected] || selected))
  }
  if (node.matches.length) {
    const hits = node.matches.filter(match => match.found)
    parts.push(`匹配 ${node.matches.length} 次 · ${hits.length ? `命中 ${[...new Set(hits.map(match => match.template))].join('、')}` : '未命中'}`)
  }
  const log = node.logs.at(-1)
  if (log) parts.push(log.data?.message)
  const tap = node.details.findLast(e => e.name === 'tap')
  if (tap) parts.push(`点击 (${tap.data.x}, ${tap.data.y})`)
  if (!parts.length) {
    const result = node.details.findLast(e => e.name === 'result')
    if (result?.data && Object.hasOwn(result.data, 'value') && (result.data.value !== null || result.data.as)) {
      parts.push(`${result.data.as ? `${result.data.as} = ` : '返回 '}${formatRunValue(result.data.value)}`)
    }
  }
  return parts.filter(Boolean).join(' · ')
}

export function formatRunValue(value) {
  if (value === undefined) return '—'
  return typeof value === 'string' ? value : JSON.stringify(value, null, 2)
}

export function runRowState(node) {
  if (node.state && node.state !== 'success') return node.state
  if (node.ok === false || node.ev === 'budget') return 'failed'
  const levels = (node.logs || []).map(log => log.data?.level)
  if (node.name === 'log') levels.push(node.data?.level)
  if (levels.includes('error')) return 'error'
  if (levels.includes('warn')) return 'warning'
  if (node.state === 'success' || node.ev === 'run_end' && node.ok) return 'success'
  return 'info'
}

export const rowStateText = state => ({ info: '记录', warning: '警告', error: '错误日志' }[state] || stateText(state))
export function runDuration(node, now) {
  if (node.kind !== 'step') return '—'
  const ms = node.duration ?? Math.max(0, now - Date.parse(node.time))
  return Number.isFinite(ms) ? `${(ms / 1000).toFixed(1)}s` : '—'
}
