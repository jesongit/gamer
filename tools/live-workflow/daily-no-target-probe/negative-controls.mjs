import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const run = promisify(execFile)
const args = process.argv.slice(2)
const option = (name, fallback) => args.includes(name) ? args[args.indexOf(name) + 1] : fallback
const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../..')
const candidate = path.resolve(option('--candidate', path.join(repo, 'server/data/live-workflow-20261016/no-target-enter-roi/云游戏日常.candidate.yaml')))
const output = path.resolve(option('--out', path.join(repo, 'server/data/live-workflow-20261016/no-target-enter-roi/negative-controls')))
const executable = path.resolve(option('--probe', path.join(repo, 'server/target/live-daily-no-target-20261015/probe-target/debug/gamer-daily-no-target-probe.exe')))
const bytes = await readFile(candidate)
const hash = value => createHash('sha256').update(value).digest('hex')
const source = bytes.toString('utf8')
const sourceHash = hash(bytes)
function replaceExactly(input, before, after, count = 1) {
  const actual = input.split(before).length - 1
  if (actual !== count) throw new Error(`Mutation anchor count ${actual}; expected ${count}: ${before}`)
  return input.replaceAll(before, after)
}
function replaceEnterRegion(input, after) {
  const pattern = /(template: daily_no_target_enter_20261015\.png[\s\S]*?\n\s*region: )\[[^\]\n]+\]/g
  const matches = [...input.matchAll(pattern)]
  if (matches.length !== 1) throw new Error(`Enter region anchor count ${matches.length}; expected 1`)
  return input.replace(pattern, (_, prefix) => prefix + after)
}
const mutations = [
  ['first-miss-enter-obstacle-removed', value => replaceExactly(value, 'obstacles: [daily_click_enter_obstacle_20261015#350_880_650_980.png]', 'obstacles: []', 2)],
  ['monthly-phone-priority-reversed', value => replaceExactly(value, 'templates: [daily_supply_bottom_tight_20261011#440_840_560_890.png, 手机#009_036_043_094.png]', 'templates: [手机#009_036_043_094.png, daily_supply_bottom_tight_20261011#440_840_560_890.png]', 2)],
  ['phone-stabilization-too-short', value => replaceExactly(value, '          - sleep: {duration: 3s}', '          - sleep: {duration: 1s}', 2)],
  ['enter-search-can-hit-other-row', value => replaceEnterRegion(value, '[0.75, 0.35, 0.1171875, 0.5]')],
  ['enter-search-old-narrow-band', value => replaceExactly(
    replaceEnterRegion(value, '[0.75, $default_enter_top, 0.1171875, 0.0629629630]'),
    'a: $storage_hit.center.y, b: 0.0787037037', 'a: $storage_hit.center.y, b: 0.0481481481')],
  ['storage-anchor-refresh-removed', value => replaceExactly(value,
    '      - sleep: {duration: 600ms}\n      - wait_find: {template: daily_no_target_storage_20261015.png, threshold: 0.9, timeout: 2s, interval: 250ms}\n        as: storage_hit',
    '      - sleep: {duration: 600ms}')],
  ['storage-anchor-refresh-result-ignored', value => replaceExactly(value,
    'timeout: 2s, interval: 250ms}\n        as: storage_hit',
    'timeout: 2s, interval: 250ms}\n        as: discarded_storage_hit')],
  ['left-scroll-limit-exceeded', value => replaceExactly(value, '      - repeat: 8\n        do:\n          - swipe:', '      - repeat: 9\n        do:\n          - swipe:')],
  ['right-scroll-limit-exceeded', value => replaceExactly(value, '      - repeat: 12\n        do:\n          - swipe:', '      - repeat: 13\n        do:\n          - swipe:')],
  ['cloud-loading-old-150-limit', value => replaceExactly(value, '      - repeat: 3600', '      - repeat: 150')],
  ['left-swipe-outside-recorded-list', value => replaceExactly(value, 'from: [0.2833333333, 0.7527777778]', 'from: [0.98, 0.7527777778]')],
]
await mkdir(output, { recursive: true })
const results = []
for (const [name, mutate] of mutations) {
  const mutated = mutate(source.replaceAll('\r\n', '\n'))
  const input = path.join(output, `${name}.yaml`)
  const report = path.join(output, `${name}.json`)
  await writeFile(input, mutated)
  let exitCode = 0
  try {
    await run(executable, ['--candidate', input, '--report', report], { timeout: 60000, maxBuffer: 1024 * 1024, windowsHide: true })
  } catch (error) {
    if (error.killed || typeof error.code !== 'number') throw error
    exitCode = error.code
  }
  const evidence = JSON.parse(await readFile(report, 'utf8'))
  const failures = evidence.scenarios.filter(scenario => !scenario.passed).map(scenario => ({ name: scenario.name, outcome: scenario.outcome, failed_checks: Object.entries(scenario.checks ?? {}).filter(([, value]) => value !== true).map(([key]) => key) }))
  const result = { name, passed: exitCode !== 0 && evidence.passed === false && failures.length > 0, exit_code: exitCode, candidate_sha256: evidence.candidate_sha256, failures }
  results.push(result)
  await writeFile(report, JSON.stringify(result, null, 2))
  console.log(JSON.stringify({ name, regression_detected: result.passed, failing_scenarios: failures.length }))
}
if (hash(await readFile(candidate)) !== sourceHash) throw new Error('Original candidate changed during negative controls')
const summary = { passed: results.every(result => result.passed), original_candidate: candidate, candidate_sha256: sourceHash, original_candidate_unchanged: true, real_game: false, device_input: false, listeners_started: 0, count: results.length, results }
await writeFile(path.join(output, 'summary.json'), JSON.stringify(summary, null, 2))
if (!summary.passed) process.exitCode = 1
