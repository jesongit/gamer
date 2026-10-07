// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, ref } from 'vue'
import { mount } from '@vue/test-utils'
import { useConsoleWorkspacePanels } from './components/console/useConsoleWorkspacePanels'
vi.mock('./workspace/official-plugin-ui', () => ({ isRemoteKeymapRunning: () => true }))
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals() })
describe('gamepad focus handoff', () => {
  it('neutralizes old buttons/axes and does not replay held inputs in a new tile', async () => {
    vi.useFakeTimers()
    let buttons = [{ pressed: false, value: 0 }], axes = [0]
    const original = navigator.getGamepads
    Object.defineProperty(navigator, 'getGamepads', { configurable: true, value: () => [{ index: 0, buttons, axes }] })
    const keymap = { handleInputEvent: vi.fn() }
    let controller
    const wrapper = mount(defineComponent({ setup() { controller = useConsoleWorkspacePanels({
      route: { path: '/console', query: { panel: 'workbench' } },
      router: { push: vi.fn(async () => {}), replace: vi.fn(async () => {}) },
      panelRegistry: { resolve: () => null }, serverUiAdapter: { refresh: vi.fn(async () => ({})), dispose: vi.fn() },
      remoteKeymapRunning: ref(true), keymap, connected: ref(true), activePanelKey: ref('workbench'),
    }); return () => null } }))
    controller.startExtensionPolling()
    await vi.advanceTimersByTimeAsync(16)
    buttons = [{ pressed: true, value: 1 }]; axes = [0.7]
    await vi.advanceTimersByTimeAsync(16)
    expect(keymap.handleInputEvent).toHaveBeenCalledWith({ kind: 'gamepad_button', index: 0, pressed: true, value: 1 })
    controller.releaseRemoteGamepads()
    expect(keymap.handleInputEvent).toHaveBeenCalledWith({ kind: 'gamepad_button', index: 0, pressed: false, value: 0 })
    expect(keymap.handleInputEvent).toHaveBeenCalledWith({ kind: 'gamepad_axis', index: 0, value: 0 })
    keymap.handleInputEvent.mockClear()
    await vi.advanceTimersByTimeAsync(16)
    expect(keymap.handleInputEvent).not.toHaveBeenCalled()
    wrapper.unmount()
    vi.clearAllTimers()
    Object.defineProperty(navigator, 'getGamepads', { configurable: true, value: original })
  })
})
