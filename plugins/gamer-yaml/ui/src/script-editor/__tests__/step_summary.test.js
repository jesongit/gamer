import { expect, it } from 'vitest'
import { parseScript, serialize } from '../codec'
import { stepSummary } from '../components/kinds'

it.each([
  ['wait_find: 登录按钮.png', '等待模板出现 · 登录按钮.png'],
  ['wait_find: {template: $button, timeout: 0}', '等待模板出现 · $button'],
  ['tap_template: {template: 确认.png, name: 点击确认}', '点击确认 · 确认.png'],
  ['tap: {position: $hit.center}', '点击 · $hit.center'],
  ['tap: {position: {x: 0.2, y: 0.8}}', '点击 · (0.2, 0.8)'],
  ['swipe: {from: [0.1, 0.2], to: [0.8, 0.2]}', '滑动 · (0.1, 0.2) → (0.8, 0.2)'],
  ['sleep: 0', '等待 · 0ms'],
  ['sleep: 2s', '等待 · 2s'],
  ['key: {key: BACK, action: down}', '按键 · BACK · down'],
  ['input_text: hello', '输入文本 · "hello"'],
  ['log: false', '日志 · false'],
  ['launch: {}', '启动应用 · 设备配置的应用'],
  ['stop_app: com.example.game', '停止应用 · com.example.game'],
  ['ge: {a: $score, b: 0}', '大于等于 · $score 与 0'],
  ['claim_reward: {count: 0, retry: false}', 'claim_reward · count=0 · retry=false'],
  ['wait_find: {}', '等待模板出现 · 未选择模板'],
  ['return: null', '返回 · null'],
  ['return: false', '返回 · false'],
])('%s 收起后保留关键语义', (yaml, expected) => {
  const {model} = parseScript(`run:\n  - ${yaml}\n`)
  const original = serialize(model)
  expect(stepSummary(model.run[0])).toBe(expected)
  expect(serialize(model)).toBe(original)
})
it('接收返回值显示在参数后，不混入动作名称', () => {
  const {model} = parseScript('run:\n  - find: {template: 开始.png}\n    as: hit\n')
  expect(stepSummary(model.run[0])).toBe('查找模板 · 开始.png → hit')
})
