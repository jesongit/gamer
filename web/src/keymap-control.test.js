import { describe, expect, it, vi } from 'vitest'
import {
  KEYMAP_ACTION_TYPES,
  createKeymapController,
  indexKeymap,
  normalizeKeymap,
  validateKeymap,
} from './keymap-control'

function keyEvent(code, options = {}) {
  return {
    code,
    repeat: false,
    target: { tagName: 'DIV' },
    preventDefault: vi.fn(),
    ...options,
  }
}

const KEYMAP = {
  version: 1,
  name: '战斗方案',
  bindings: [
    { key: 'Space', action: { type: 'tap', at: [0.25, 0.5] } },
    {
      key: 'KeyE',
      action: {
        type: 'swipe',
        from: [0.1, 0.2],
        to: [0.8, 0.2],
        duration_ms: 300,
      },
    },
    { key: 'KeyR', action: { type: 'raw_key', code: 'KeyA' } },
    { key: 'KeyW', action: { type: 'hold', at: [0.4, 0.6] } },
  ],
}

describe('keymap-control schema', () => {
  it('accepts the planned action types and indexes bindings by code', () => {
    expect(KEYMAP_ACTION_TYPES).toEqual(['tap', 'swipe', 'raw_key', 'hold'])
    expect(validateKeymap(KEYMAP).valid).toBe(true)
    expect(indexKeymap(KEYMAP).get('KeyE').action.type).toBe('swipe')
    expect(normalizeKeymap(KEYMAP)).toEqual(KEYMAP)
  })

  it('rejects duplicate keys, unknown fields, invalid coordinates and duration', () => {
    const result = validateKeymap({
      version: 1,
      name: '非法',
      bindings: [
        { key: 'KeyA', action: { type: 'tap', at: [1.1, 0] } },
        {
          key: 'KeyA',
          action: {
            type: 'swipe',
            from: [0, 0],
            to: [1, 1],
            duration_ms: 0,
            extra: true,
          },
        },
      ],
      extra: true,
    })
    expect(result.valid).toBe(false)
    expect(result.issues.map(item => item.code)).toEqual(expect.arrayContaining([
      'keymap.unknown_field',
      'keymap.coordinate',
      'keymap.duplicate_key',
      'keymap.duration',
    ]))
  })
})

describe('keymap-control routing', () => {
  it('uses the stable getter/sendControl/getVideoSize contract for a tap hit', () => {
    const sendControl = vi.fn()
    const getKeymap = vi.fn(() => KEYMAP)
    const fallback = { handleKeydown: vi.fn(), handleKeyup: vi.fn() }
    const controller = createKeymapController({
      getKeymap,
      sendControl,
      getVideoSize: () => ({ width: 1000, height: 500 }),
      fallback,
    })
    const down = keyEvent('Space')
    const up = keyEvent('Space')

    expect(controller.handleKeydown(down).mapped).toBe(true)
    expect(controller.handleKeyup(up).mapped).toBe(true)
    expect(sendControl).toHaveBeenCalledWith({ type: 'tap', x: 250, y: 250 })
    expect(fallback.handleKeydown).not.toHaveBeenCalled()
    expect(fallback.handleKeyup).not.toHaveBeenCalled()
    expect(down.preventDefault).toHaveBeenCalledTimes(1)
    expect(getKeymap).toHaveBeenCalled()
  })

  it('sends the existing swipe DataChannel shape with pixel coordinates', () => {
    const sendControl = vi.fn()
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
      getVideoSize: () => ({ width: 1000, height: 500 }),
    })
    controller.handleKeydown(keyEvent('KeyE'))

    expect(sendControl).toHaveBeenCalledWith({
      type: 'swipe',
      x1: 100,
      y1: 100,
      x2: 800,
      y2: 100,
      duration: 300,
    })
  })

  it('forwards raw key down/up and repeat, while tap ignores repeat', () => {
    const sendControl = vi.fn()
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
    })
    controller.handleKeydown(keyEvent('KeyR'))
    controller.handleKeydown(keyEvent('KeyR', { repeat: true }))
    controller.handleKeydown(keyEvent('Space'))
    controller.handleKeydown(keyEvent('Space', { repeat: true }))
    controller.handleKeyup(keyEvent('KeyR'))
    controller.handleKeyup(keyEvent('Space'))

    expect(sendControl.mock.calls.map(([message]) => message)).toEqual([
      { type: 'key', action: 0, keycode: 29, repeat: 0, meta: 0 },
      { type: 'key', action: 0, keycode: 29, repeat: 1, meta: 0 },
      { type: 'tap', x: 0.25, y: 0.5 },
      { type: 'key', action: 1, keycode: 29, repeat: 0, meta: 0 },
    ])
  })

  it('holds touch state until keyup and releases it on keymap/mode switch', () => {
    const sendControl = vi.fn()
    const mode = { value: 'game' }
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
      mode,
    })
    controller.handleKeydown(keyEvent('KeyW'))
    expect(controller.isPressed('KeyW')).toBe(true)
    controller.setMode('text')
    expect(mode.value).toBe('text')
    expect(sendControl).toHaveBeenLastCalledWith({
      type: 'touch',
      action: 'up',
      x: 0.4,
      y: 0.6,
    })
    expect(controller.isPressed('KeyW')).toBe(false)
  })

  it('delegates mapping misses and all text-mode input to the fallback controller', () => {
    const fallback = {
      handleKeydown: vi.fn(() => ({ handled: true })),
      handleKeyup: vi.fn(() => ({ handled: true })),
    }
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      mode: 'text',
      fallback,
      sendControl: vi.fn(),
    })
    const down = keyEvent('Space', { key: ' ' })
    controller.handleKeydown(down)
    controller.handleKeyup(keyEvent('Space'))
    expect(fallback.handleKeydown).toHaveBeenCalledWith(down)
    expect(fallback.handleKeyup).toHaveBeenCalled()

    const gameController = createKeymapController({
      getKeymap: () => KEYMAP,
      fallback,
    })
    gameController.handleKeydown(keyEvent('KeyZ'))
    expect(fallback.handleKeydown).toHaveBeenLastCalledWith(
      expect.objectContaining({ code: 'KeyZ' }),
    )
  })

  it('releases mapped raw and hold states plus fallback without REST knowledge', () => {
    const sendControl = vi.fn()
    const fallback = { releaseAll: vi.fn(() => ({ handled: true })) }
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
      fallback,
    })
    controller.handleKeydown(keyEvent('KeyR'))
    controller.handleKeydown(keyEvent('KeyW'))
    expect(controller.releaseAll().handled).toBe(true)
    expect(sendControl.mock.calls.slice(-2).map(([message]) => message)).toEqual([
      { type: 'key', action: 1, keycode: 29, repeat: 0, meta: 0 },
      { type: 'touch', action: 'up', x: 0.4, y: 0.6 },
    ])
    expect(fallback.releaseAll).toHaveBeenCalledTimes(1)
  })
})
