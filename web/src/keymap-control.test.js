import { describe, expect, it, vi } from 'vitest'
import {
  KEYMAP_ACTION_TYPES,
  INPUT_PROTOCOL_VERSION,
  buildTouchPhase,
  createKeymapController,
  indexKeymap,
  isInputSelector,
  normalizeInputEvent,
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
  it('shares a closed input selector vocabulary with the server gateway', () => {
    expect(INPUT_PROTOCOL_VERSION).toBe('gamer-input@1')
    expect(isInputSelector('MouseLeft')).toBe(true)
    expect(isInputSelector('GamepadButton31')).toBe(true)
    expect(isInputSelector('GamepadAxis7')).toBe(true)
    expect(isInputSelector('GamepadButton32')).toBe(false)
    expect(isInputSelector('MouseSide')).toBe(false)
    expect(normalizeInputEvent({ code: 'KeyW', repeat: true })).toEqual({
      type: 'key_down', code: 'KeyW', repeat: true, meta: 0,
    })
    expect(normalizeInputEvent({ code: 'KeyW' }, 'up')).toEqual({
      type: 'key_up', code: 'KeyW', meta: 0,
    })
    expect(normalizeInputEvent({ type: 'keyup', code: 'KeyW' })).toEqual({
      type: 'key_up', code: 'KeyW', meta: 0,
    })
    expect(normalizeInputEvent({ button: 0, x: 10.4, y: 20.6 })).toEqual({
      type: 'mouse_down', button: 0, x: 10, y: 21,
    })
    expect(normalizeInputEvent({ type: 'mousemove', x: 12.4, y: 8.6, movementX: 2, movementY: -1 })).toEqual({
      type: 'mouse_move', x: 12, y: 9, delta_x: 2, delta_y: -1,
    })
    expect(normalizeInputEvent({ type: 'wheel', x: 12, y: 9, deltaX: 1.2, deltaY: -3.4 })).toEqual({
      type: 'wheel', x: 12, y: 9, delta_x: 1, delta_y: -3,
    })
    expect(normalizeInputEvent({ kind: 'gamepad_axis', index: 2, value: -0.5 })).toEqual({
      type: 'gamepad_axis', index: 2, value: -0.5,
    })
  })

  it('builds the shared touch phase message used by mouse and keyboard input', () => {
    expect(buildTouchPhase('down', 3, 120, 240)).toEqual({
      type: 'touch',
      action: 'down',
      pointer_id: 3,
      x: 120,
      y: 240,
    })
  })

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

  it('keeps hold schema to a persisted single point without runtime pointer fields', () => {
    const result = validateKeymap({
      version: 1,
      name: 'hold',
      bindings: [{
        key: 'KeyW',
        action: {
          type: 'hold',
          at: [0.5, 0.5],
          from: [0.1, 0.1],
          to: [0.9, 0.9],
          pointer_id: 1,
        },
      }],
    })
    expect(result.valid).toBe(false)
    expect(result.issues.map(item => item.code)).toEqual(expect.arrayContaining([
      'keymap.unknown_field',
    ]))
  })
})

describe('keymap-control routing', () => {
  it('forwards keyboard, mouse, wheel, and gamepad events to the running server keymap', () => {
    const sendInputEvent = vi.fn()
    const remote = { value: true }
    const controller = createKeymapController({ remote, sendInputEvent })
    const down = keyEvent('KeyW')
    const mouseDown = { type: 'mousedown', button: 0, x: 10, y: 20, preventDefault: vi.fn() }

    expect(controller.handleKeydown(down)).toMatchObject({ handled: true, remote: true, sent: true })
    expect(controller.handleInputEvent(mouseDown, 'down', mouseDown))
      .toMatchObject({ handled: true, remote: true })
    controller.handleInputEvent({ type: 'mousemove', x: 12, y: 21, movementX: 2, movementY: 1 }, 'move')
    controller.handleInputEvent({ type: 'wheel', x: 12, y: 21, deltaX: 1, deltaY: -2 }, 'wheel')
    controller.handleInputEvent({ kind: 'gamepad_button', index: 2, pressed: true, value: 1 })
    controller.handleInputEvent({ kind: 'gamepad_axis', index: 1, value: -0.75 })
    controller.handleKeyup(keyEvent('KeyW'))

    expect(sendInputEvent.mock.calls.map(([message]) => message)).toEqual([
      { type: 'input_event', event: { type: 'key_down', code: 'KeyW', repeat: false, meta: 0 } },
      { type: 'input_event', event: { type: 'mouse_down', button: 0, x: 10, y: 20 } },
      { type: 'input_event', event: { type: 'mouse_move', x: 12, y: 21, delta_x: 2, delta_y: 1 } },
      { type: 'input_event', event: { type: 'wheel', x: 12, y: 21, delta_x: 1, delta_y: -2 } },
      { type: 'input_event', event: { type: 'gamepad_button', index: 2, pressed: true, value: 1 } },
      { type: 'input_event', event: { type: 'gamepad_axis', index: 1, value: -0.75 } },
      { type: 'input_event', event: { type: 'key_up', code: 'KeyW', meta: 0 } },
    ])
    expect(down.preventDefault).toHaveBeenCalledTimes(1)
    expect(mouseDown.preventDefault).toHaveBeenCalledTimes(1)
  })

  it('releases remote key state on blur and never falls back when the channel is closed', () => {
    const sendInputEvent = vi.fn(() => false)
    const fallback = { handleKeydown: vi.fn(), handleKeyup: vi.fn() }
    const controller = createKeymapController({ remote: true, sendInputEvent, fallback })
    controller.handleKeydown(keyEvent('KeyA'))
    controller.handleWindowBlur()
    expect(sendInputEvent).toHaveBeenCalledTimes(2)
    expect(fallback.handleKeydown).not.toHaveBeenCalled()
  })

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

  it('reuses the physical keyboard meta state for mapped raw keys', () => {
    const sendControl = vi.fn()
    const keyboard = { getMetaState: vi.fn(() => 0x1000) }
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
      getKeyMetaState: keyboard.getMetaState,
    })

    controller.handleKeydown(keyEvent('KeyR', { shiftKey: true }))
    controller.handleKeyup(keyEvent('KeyR', { shiftKey: true }))

    expect(keyboard.getMetaState).toHaveBeenCalled()
    expect(sendControl.mock.calls.map(([message]) => message)).toEqual([
      { type: 'key', action: 0, keycode: 29, repeat: 0, meta: 0x1001 },
      { type: 'key', action: 1, keycode: 29, repeat: 0, meta: 0x1001 },
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
      pointer_id: 1,
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
      { type: 'touch', action: 'up', pointer_id: 1, x: 0.4, y: 0.6 },
    ])
    expect(fallback.releaseAll).toHaveBeenCalledTimes(1)
  })

  it('sends one hold down, ignores repeat, and releases the saved point and pointer ID', () => {
    const sendControl = vi.fn()
    let size = { width: 1000, height: 500 }
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
      getVideoSize: () => size,
    })

    controller.handleKeydown(keyEvent('KeyW'))
    controller.handleKeydown(keyEvent('KeyW', { repeat: true }))
    size = { width: 2000, height: 1000 }
    controller.handleKeyup(keyEvent('KeyW'))

    expect(sendControl.mock.calls.map(([message]) => message)).toEqual([
      { type: 'touch', action: 'down', pointer_id: 1, x: 400, y: 300 },
      { type: 'touch', action: 'up', pointer_id: 1, x: 400, y: 300 },
    ])
    expect(controller.isPressed('KeyW')).toBe(false)
  })

  it('produces a quick hold click as down then up with no timer or extra phases', () => {
    const sendControl = vi.fn()
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
    })

    controller.handleKeydown(keyEvent('KeyW'))
    controller.handleKeyup(keyEvent('KeyW'))

    expect(sendControl.mock.calls.map(([message]) => message)).toEqual([
      { type: 'touch', action: 'down', pointer_id: 1, x: 0.4, y: 0.6 },
      { type: 'touch', action: 'up', pointer_id: 1, x: 0.4, y: 0.6 },
    ])
  })

  it('assigns distinct 1..31 pointer IDs to simultaneous holds and releases independently', () => {
    const sendControl = vi.fn()
    const keymap = {
      ...KEYMAP,
      bindings: [
        ...KEYMAP.bindings,
        { key: 'KeyA', action: { type: 'hold', at: [0.1, 0.2] } },
      ],
    }
    const controller = createKeymapController({
      getKeymap: () => keymap,
      sendControl,
    })

    controller.handleKeydown(keyEvent('KeyW'))
    controller.handleKeydown(keyEvent('KeyA'))
    controller.handleKeyup(keyEvent('KeyW'))
    controller.handleKeyup(keyEvent('KeyA'))

    expect(sendControl.mock.calls.map(([message]) => message)).toEqual([
      { type: 'touch', action: 'down', pointer_id: 1, x: 0.4, y: 0.6 },
      { type: 'touch', action: 'down', pointer_id: 2, x: 0.1, y: 0.2 },
      { type: 'touch', action: 'up', pointer_id: 1, x: 0.4, y: 0.6 },
      { type: 'touch', action: 'up', pointer_id: 2, x: 0.1, y: 0.2 },
    ])
  })

  it('releases every active hold on scheme switch, mode switch, blur, hidden, and destroy', () => {
    const sendControl = vi.fn()
    const mode = { value: 'game' }
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
      mode,
    })

    controller.handleKeydown(keyEvent('KeyW'))
    controller.setKeymap(KEYMAP)
    controller.handleKeydown(keyEvent('KeyW'))
    controller.setMode('text')
    controller.handleKeydown(keyEvent('KeyW'))
    controller.setMode('game')
    controller.handleKeydown(keyEvent('KeyW'))
    controller.handleWindowBlur()
    controller.handleKeydown(keyEvent('KeyW'))
    controller.handleVisibilityChange(true)
    controller.handleKeydown(keyEvent('KeyW'))
    controller.destroy()

    const phases = sendControl.mock.calls.map(([message]) => message)
    expect(phases.filter(message => message.type === 'touch' && message.action === 'down')).toHaveLength(5)
    expect(phases.filter(message => message.type === 'touch' && message.action === 'up')).toHaveLength(5)
    expect(controller.isPressed('KeyW')).toBe(false)
    expect(mode.value).toBe('game')
  })

  it('does not fallback stateful hold/raw_key actions when DataChannel sending fails', () => {
    const sendControl = vi.fn(() => false)
    const fallback = {
      handleKeydown: vi.fn(),
      handleKeyup: vi.fn(),
    }
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
      fallback,
    })

    const holdResult = controller.handleKeydown(keyEvent('KeyW'))
    const rawResult = controller.handleKeydown(keyEvent('KeyR'))
    controller.handleKeyup(keyEvent('KeyW'))
    controller.handleKeyup(keyEvent('KeyR'))

    expect(holdResult).toMatchObject({ handled: true, mapped: true, sent: false })
    expect(rawResult).toMatchObject({ handled: true, mapped: true, sent: false })
    expect(fallback.handleKeydown).not.toHaveBeenCalled()
    expect(fallback.handleKeyup).not.toHaveBeenCalled()
    expect(controller.getPressedCodes()).toEqual([])
  })

  it('keeps one-shot tap fallback behavior when the general sender reports not sent', () => {
    const sendControl = vi.fn(() => false)
    const fallback = { handleKeydown: vi.fn(() => ({ handled: true })) }
    const controller = createKeymapController({
      getKeymap: () => KEYMAP,
      sendControl,
      fallback,
    })

    const result = controller.handleKeydown(keyEvent('Space'))

    expect(result).toEqual({ handled: true })
    expect(fallback.handleKeydown).toHaveBeenCalledTimes(1)
  })
})
