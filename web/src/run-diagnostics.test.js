import { expect, it } from 'vitest'
import { runDiagnosticsFilename, runDiagnosticsUrl } from './run-diagnostics'

it('keeps support downloads same-origin and safely encodes run identity', () => {
  expect(runDiagnosticsUrl('run/one?x=2')).toBe('/api/runs/run%2Fone%3Fx%3D2/diagnostics')
  expect(runDiagnosticsFilename('run/one?x=2')).toBe('gamer-diagnostics-run_one_x_2.zip')
  expect(runDiagnosticsUrl('')).toBe('')
})
