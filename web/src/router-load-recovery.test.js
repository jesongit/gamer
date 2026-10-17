// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'

const handlers = vi.hoisted(() => ({}))
vi.mock('vue-router', () => ({
  createWebHashHistory: () => ({}),
  createRouter: () => ({
    beforeEach() {},
    onError(handler) { handlers.error = handler },
    afterEach(handler) { handlers.after = handler },
  }),
}))
vi.mock('./auth', () => ({ probeSession() {}, resolveGuardTarget() {} }))

afterEach(() => { sessionStorage.clear(); vi.restoreAllMocks() })

it('persistent module failure reloads once and resets only after successful navigation', async () => {
  const reload = vi.spyOn(window.location, 'reload').mockImplementation(() => {})
  vi.spyOn(console, 'error').mockImplementation(() => {})
  await import('./router')
  const failure = new Error('Failed to fetch dynamically imported module')
  handlers.error(failure)
  handlers.after({}, {}, new Error('navigation failed'))
  handlers.error(failure)
  expect(reload).toHaveBeenCalledTimes(1)
  handlers.after({}, {}, undefined)
  handlers.error(failure)
  expect(reload).toHaveBeenCalledTimes(2)
})

it('storage failure does not enter an unbounded refresh loop', async () => {
  const reload = vi.spyOn(window.location, 'reload').mockImplementation(() => {})
  vi.spyOn(console, 'error').mockImplementation(() => {})
  vi.spyOn(sessionStorage, 'getItem').mockImplementation(() => { throw new Error('storage unavailable') })
  await import('./router')
  handlers.error(new Error('Importing a module script failed'))
  expect(reload).not.toHaveBeenCalled()
})
