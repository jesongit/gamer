import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { load as loadYaml } from 'js-yaml'
import { normalizeKeymap, validateKeymap } from './keymap-control'

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const fixtureRoot = resolve(repoRoot, 'tests/fixtures')

function fixtureText(relative) {
  return readFileSync(resolve(fixtureRoot, relative), 'utf8')
}

describe('Phase 0 shared compatibility fixtures', () => {
  it('keeps the manifest categories and files available to the web test runtime', () => {
    const manifest = JSON.parse(fixtureText('manifest.json'))
    expect(manifest.schema_version).toBe(1)
    expect(Object.keys(manifest.files)).toEqual([
      'scripts',
      'keymaps',
      'templates',
      'tasks',
      'screenshots',
    ])

    for (const entries of Object.values(manifest.files)) {
      for (const entry of entries) {
        expect(entry.path).not.toContain('..')
        expect(readFileSync(resolve(fixtureRoot, entry.path)).length).toBeGreaterThan(0)
      }
    }
  })

  it('accepts the shared keymap fixture with the same action vocabulary as the UI', () => {
    const keymap = loadYaml(fixtureText('keymaps/phase0_combat.yaml'))
    expect(validateKeymap(keymap)).toMatchObject({ valid: true })
    expect(normalizeKeymap(keymap)).toEqual(keymap)
    expect(keymap.bindings.map(binding => binding.action.type)).toEqual([
      'tap',
      'swipe',
      'raw_key',
      'hold',
    ])
  })

  it('keeps the script and task fixture shapes within their frozen top-level contracts', () => {
    const script = loadYaml(fixtureText('scripts/phase0_smoke.yaml'))
    const task = JSON.parse(fixtureText('tasks/phase0_daily.json'))
    expect(Object.keys(script).sort()).toEqual(['steps'])
    expect(script.steps).toHaveLength(9)
    expect(Object.keys(task).sort()).toEqual([
      'app',
      'enabled',
      'id',
      'name',
      'runner',
      'schedule',
    ])
    expect(task.app).toMatchObject({ device_id: 'fixture-device' })
    expect(task.runner).toMatchObject({ runner_id: 'gamer.yaml', entrypoint: 'phase0_smoke.yaml' })
    expect(task.runner.payload).toEqual({ args: {} })
    expect(task.schedule).toEqual({ provider_id: 'cron', config: { expression: '0 */15 * * * * *' } })
    expect(task.enabled).toBe(true)
  })
})
