import { describe, expect, it, vi } from 'vitest'
import {
  ANDROID_KEYCODES,
  ANDROID_META,
  calculateMetaState,
  createKeyboardController,
  getMetaState,
  isKeyboardTargetIgnored,
  KEYCODE_BY_CODE,
  KEY_META_MASKS,
  keycodeForCode,
  mapKeyboardCode,
  shouldIgnoreKeyboardTarget,
} from './keyboard-control'

function keyEvent(code, options = {}) {
  return {
    code,
    repeat: false,
    shiftKey: false,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    target: { tagName: 'DIV' },
    preventDefault: vi.fn(),
    ...options,
  }
}

describe('keyboard-control mapping', () => {
  it('maps letters, digits, navigation, editing, modifiers and function keys', () => {
    expect(mapKeyboardCode('KeyA')).toBe(29)
    expect(mapKeyboardCode('KeyZ')).toBe(54)
    expect(mapKeyboardCode('Digit0')).toBe(7)
    expect(mapKeyboardCode('Digit9')).toBe(16)
    expect(mapKeyboardCode('ArrowUp')).toBe(19)
    expect(mapKeyboardCode('ArrowRight')).toBe(22)
    expect(mapKeyboardCode('Home')).toBe(122)
    expect(mapKeyboardCode('End')).toBe(123)
    expect(mapKeyboardCode('PageUp')).toBe(92)
    expect(mapKeyboardCode('PageDown')).toBe(93)
    expect(mapKeyboardCode('Space')).toBe(62)
    expect(mapKeyboardCode('Enter')).toBe(66)
    expect(mapKeyboardCode('Tab')).toBe(61)
    expect(mapKeyboardCode('Escape')).toBe(111)
    expect(mapKeyboardCode('Backspace')).toBe(67)
    expect(mapKeyboardCode('Delete')).toBe(112)
    expect(mapKeyboardCode('ShiftLeft')).toBe(59)
    expect(mapKeyboardCode('ControlRight')).toBe(114)
    expect(mapKeyboardCode('AltLeft')).toBe(57)
    expect(mapKeyboardCode('MetaRight')).toBe(118)
    expect(mapKeyboardCode('F1')).toBe(131)
    expect(mapKeyboardCode('F12')).toBe(142)
  })

  it('maps common punctuation and numeric keypad keys', () => {
    expect({
      comma: mapKeyboardCode('Comma'),
      period: mapKeyboardCode('Period'),
      slash: mapKeyboardCode('Slash'),
      quote: mapKeyboardCode('Quote'),
      bracket: mapKeyboardCode('BracketLeft'),
      backquote: mapKeyboardCode('Backquote'),
    }).toEqual({ comma: 55, period: 56, slash: 76, quote: 75, bracket: 71, backquote: 68 })
    expect(mapKeyboardCode('Numpad0')).toBe(144)
    expect(mapKeyboardCode('Numpad9')).toBe(153)
    expect(mapKeyboardCode('NumpadAdd')).toBe(157)
    expect(mapKeyboardCode('NumpadDecimal')).toBe(158)
    expect(mapKeyboardCode('NumpadEnter')).toBe(160)
    expect(ANDROID_KEYCODES.KeyM).toBe(41)
  })

  it('returns null for unsupported or non-code input', () => {
    expect(mapKeyboardCode('KeyNotReal')).toBeNull()
    expect(mapKeyboardCode('a')).toBeNull()
    expect(mapKeyboardCode('')).toBeNull()
    expect(mapKeyboardCode(null)).toBeNull()
  })

  it('exposes stable parent-facing mapping exports', () => {
    expect(KEYCODE_BY_CODE).toBe(ANDROID_KEYCODES)
    expect(KEY_META_MASKS).toBe(ANDROID_META)
    expect(keycodeForCode('KeyA')).toBe(29)
    expect(isKeyboardTargetIgnored({ tagName: 'INPUT' })).toBe(true)
  })
})

describe('keyboard-control meta state', () => {
  it('sets aggregate and side-specific Android modifier bits', () => {
    const expected = ANDROID_META.SHIFT_ON
      | ANDROID_META.SHIFT_LEFT_ON
      | ANDROID_META.CTRL_ON
      | ANDROID_META.CTRL_RIGHT_ON
      | ANDROID_META.ALT_ON
      | ANDROID_META.ALT_LEFT_ON
      | ANDROID_META.META_ON
      | ANDROID_META.META_RIGHT_ON
    expect(getMetaState(new Set(['ShiftLeft', 'ControlRight', 'AltLeft', 'MetaRight']))).toBe(expected)
    expect(calculateMetaState(['ShiftRight'])).toBe(
      ANDROID_META.SHIFT_ON | ANDROID_META.SHIFT_RIGHT_ON,
    )
    expect(getMetaState([])).toBe(ANDROID_META.NONE)
  })
})

describe('keyboard-control target filtering', () => {
  it('ignores controls, editors, dialogs and their descendants', () => {
    expect(shouldIgnoreKeyboardTarget({ tagName: 'INPUT' })).toBe(true)
    expect(shouldIgnoreKeyboardTarget({ tagName: 'TEXTAREA' })).toBe(true)
    expect(shouldIgnoreKeyboardTarget({ contentEditable: 'true' })).toBe(true)
    expect(shouldIgnoreKeyboardTarget({ className: 'script-editor' })).toBe(true)
    expect(shouldIgnoreKeyboardTarget({ role: 'dialog' })).toBe(true)
    expect(shouldIgnoreKeyboardTarget({ tagName: 'SPAN', parentElement: { className: 'modal' } })).toBe(true)
    expect(shouldIgnoreKeyboardTarget({ tagName: 'CANVAS' })).toBe(false)
  })
})

describe('keyboard-control state controller', () => {
  it('sends keydown and keyup, deduplicates non-repeat keydown, and forwards repeat', () => {
    const onKey = vi.fn()
    const controller = createKeyboardController({ onKey })
    const first = keyEvent('KeyA')
    const duplicate = keyEvent('KeyA')
    const repeat = keyEvent('KeyA', { repeat: true })
    const up = keyEvent('KeyA')

    expect(controller.handleKeyDown(first)).toEqual({ handled: true })
    expect(controller.handleKeyDown(duplicate)).toEqual({ handled: false })
    expect(controller.handleKeyDown(repeat)).toEqual({ handled: true })
    expect(controller.handleKeyUp(up)).toEqual({ handled: true })
    expect(controller.handleKeyUp(keyEvent('KeyA'))).toEqual({ handled: false })
    expect(onKey.mock.calls.map(([message]) => message)).toEqual([
      { type: 'key', action: 0, keycode: 29, repeat: 0, meta: 0 },
      { type: 'key', action: 0, keycode: 29, repeat: 1, meta: 0 },
      { type: 'key', action: 1, keycode: 29, repeat: 0, meta: 0 },
    ])
    expect(first.preventDefault).toHaveBeenCalledTimes(1)
    expect(duplicate.preventDefault).not.toHaveBeenCalled()
    expect(repeat.preventDefault).toHaveBeenCalledTimes(1)
    expect(controller.getPressedCodes()).toEqual([])
  })

  it('tracks modifier combinations on both keydown and keyup', () => {
    const onKey = vi.fn()
    const controller = createKeyboardController({ onKey })
    controller.keydown(keyEvent('ShiftLeft'))
    controller.keydown(keyEvent('ControlLeft'))
    controller.keydown(keyEvent('KeyA', { shiftKey: true, ctrlKey: true }))
    controller.keyup(keyEvent('KeyA', { shiftKey: true, ctrlKey: true }))
    controller.keyup(keyEvent('ControlLeft'))
    controller.keyup(keyEvent('ShiftLeft'))

    const messages = onKey.mock.calls.map(([message]) => message)
    const shiftCtrl = ANDROID_META.SHIFT_ON
      | ANDROID_META.SHIFT_LEFT_ON
      | ANDROID_META.CTRL_ON
      | ANDROID_META.CTRL_LEFT_ON
    expect(messages[0].meta).toBe(ANDROID_META.SHIFT_ON | ANDROID_META.SHIFT_LEFT_ON)
    expect(messages[1].meta).toBe(shiftCtrl)
    expect(messages[2].meta).toBe(shiftCtrl)
    expect(messages[3].meta).toBe(shiftCtrl)
    expect(messages[4].meta).toBe(ANDROID_META.SHIFT_ON | ANDROID_META.SHIFT_LEFT_ON)
    expect(messages[5].meta).toBe(0)
    expect(controller.getMetaState()).toBe(0)
  })

  it('releases all keys on blur/hidden and avoids stuck modifier state', () => {
    const onKey = vi.fn()
    const controller = createKeyboardController({ onKey })
    controller.keydown(keyEvent('ShiftRight'))
    controller.keydown(keyEvent('ArrowUp', { shiftKey: true }))

    expect(controller.releaseAll()).toEqual({ handled: true })
    expect(onKey.mock.calls.slice(-2).map(([message]) => message)).toEqual([
      {
        type: 'key', action: 1, keycode: 19, repeat: 0,
        meta: ANDROID_META.SHIFT_ON | ANDROID_META.SHIFT_RIGHT_ON,
      },
      { type: 'key', action: 1, keycode: 60, repeat: 0, meta: 0 },
    ])
    expect(controller.handleVisibilityChange(true)).toEqual({ handled: false })
    expect(controller.getPressedCodes()).toEqual([])
  })

  it('does not send from ignored targets, but still releases a previously sent key', () => {
    const onKey = vi.fn()
    const controller = createKeyboardController({ onKey })
    const ignoredDown = keyEvent('Space', { target: { tagName: 'INPUT' } })
    expect(controller.handleKeyDown(ignoredDown)).toEqual({ handled: false })
    expect(onKey).not.toHaveBeenCalled()
    expect(ignoredDown.preventDefault).not.toHaveBeenCalled()

    const stageDown = keyEvent('Space')
    controller.handleKeyDown(stageDown)
    const inputUp = keyEvent('Space', { target: { tagName: 'TEXTAREA' } })
    expect(controller.handleKeyUp(inputUp)).toEqual({ handled: true })
    expect(onKey).toHaveBeenCalledTimes(2)
    expect(inputUp.preventDefault).toHaveBeenCalledTimes(1)
  })

  it('can be disabled without losing the explicit release path', () => {
    const onKey = vi.fn()
    const controller = createKeyboardController({ onKey })
    controller.handleKeyDown(keyEvent('KeyW'))
    controller.setEnabled(false)
    expect(controller.handleKeyDown(keyEvent('KeyD'))).toEqual({ handled: false })
    expect(controller.handleKeyUp(keyEvent('KeyW'))).toEqual({ handled: true })
    expect(onKey.mock.calls.map(([message]) => message.keycode)).toEqual([51, 51])
    expect(controller.isEnabled()).toBe(false)
  })
})
