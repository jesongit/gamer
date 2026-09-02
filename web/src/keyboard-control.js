/**
 * Browser KeyboardEvent.code -> Android KeyEvent keycode mapping and state.
 *
 * This module deliberately does not install DOM listeners or know about Vue.
 * The caller owns focus/connection wiring and passes a send(message) callback
 * to createKeyboardController().
 */

const KEYCODES = {
  // Navigation and editing.
  ArrowUp: 19,
  ArrowDown: 20,
  ArrowLeft: 21,
  ArrowRight: 22,
  Home: 122,
  End: 123,
  PageUp: 92,
  PageDown: 93,
  Insert: 124,
  Delete: 112,

  // Whitespace and control keys.
  Space: 62,
  Enter: 66,
  NumpadEnter: 160,
  Tab: 61,
  Escape: 111,
  Backspace: 67,

  // Modifiers.
  AltLeft: 57,
  AltRight: 58,
  ShiftLeft: 59,
  ShiftRight: 60,
  ControlLeft: 113,
  ControlRight: 114,
  MetaLeft: 117,
  MetaRight: 118,

  // Common lock/system keys.
  CapsLock: 115,
  NumLock: 143,
  ScrollLock: 116,
  PrintScreen: 120,
  Pause: 121,
  ContextMenu: 82,

  // Common punctuation. These values are the Android physical keyboard
  // keycodes, not the character produced by the current keyboard layout.
  Backquote: 68,
  Minus: 69,
  Equal: 70,
  BracketLeft: 71,
  BracketRight: 72,
  Backslash: 73,
  IntlBackslash: 73,
  Semicolon: 74,
  Quote: 75,
  Comma: 55,
  Period: 56,
  Slash: 76,

  // Number row and function keys.
  F1: 131,
  F2: 132,
  F3: 133,
  F4: 134,
  F5: 135,
  F6: 136,
  F7: 137,
  F8: 138,
  F9: 139,
  F10: 140,
  F11: 141,
  F12: 142,

  // Numeric keypad.
  Numpad0: 144,
  Numpad1: 145,
  Numpad2: 146,
  Numpad3: 147,
  Numpad4: 148,
  Numpad5: 149,
  Numpad6: 150,
  Numpad7: 151,
  Numpad8: 152,
  Numpad9: 153,
  NumpadDivide: 154,
  NumpadMultiply: 155,
  NumpadSubtract: 156,
  NumpadAdd: 157,
  NumpadDecimal: 158,
  NumpadComma: 159,
  NumpadEqual: 161,
  NumpadParenLeft: 162,
  NumpadParenRight: 163,
}

for (let i = 0; i < 26; i += 1) {
  KEYCODES[`Key${String.fromCharCode(65 + i)}`] = 29 + i
}

for (let i = 0; i < 10; i += 1) {
  KEYCODES[`Digit${i}`] = 7 + i
}

export const ANDROID_KEYCODES = Object.freeze(KEYCODES)
// The shorter name is useful to callers that only need the lookup table.
export const KEYBOARD_KEYCODES = ANDROID_KEYCODES
// Stable parent-component contract names.
export const KEYCODE_BY_CODE = ANDROID_KEYCODES

/** Android KeyEvent meta-state masks. */
export const ANDROID_META = Object.freeze({
  NONE: 0,
  SHIFT_ON: 0x0001,
  ALT_ON: 0x0002,
  SHIFT_LEFT_ON: 0x0040,
  SHIFT_RIGHT_ON: 0x0080,
  CTRL_ON: 0x1000,
  CTRL_LEFT_ON: 0x2000,
  CTRL_RIGHT_ON: 0x4000,
  META_ON: 0x10000,
  META_LEFT_ON: 0x20000,
  META_RIGHT_ON: 0x40000,
  ALT_LEFT_ON: 0x0010,
  ALT_RIGHT_ON: 0x0020,
})
export const KEY_META_MASKS = ANDROID_META

const MODIFIER_CODES = new Set([
  'ShiftLeft', 'ShiftRight',
  'ControlLeft', 'ControlRight',
  'AltLeft', 'AltRight',
  'MetaLeft', 'MetaRight',
])

const IGNORE_TARGET_SELECTOR = [
  'input',
  'textarea',
  'select',
  'button',
  'dialog',
  '[role="dialog"]',
  '[aria-modal="true"]',
  '[contenteditable="true"]',
  '[contenteditable=""]',
  '[contenteditable="plaintext-only"]',
  '[role="textbox"]',
  '[role="combobox"]',
  '[role="listbox"]',
  '[data-keyboard-ignore]',
  '[data-editor]',
  '.editor',
  '.script-editor',
  '.code-editor',
  '.yaml-editor',
  '.raw-editor',
  '.cm-editor',
  '.monaco-editor',
  '.ace_editor',
  '.modal',
  '.dialog',
  '.popup',
].join(',')

function elementForTarget(target) {
  if (!target) return null
  if (target.nodeType === 3) return target.parentElement || target.parentNode || null
  return target
}

function hasAttribute(element, name) {
  if (typeof element?.hasAttribute === 'function') return element.hasAttribute(name)
  if (typeof element?.getAttribute === 'function') return element.getAttribute(name) !== null
  return Object.prototype.hasOwnProperty.call(element || {}, name)
}

function hasIgnoredShape(element) {
  if (!element) return false

  const tag = String(element.tagName || element.nodeName || '').toLowerCase()
  if (tag === 'input' || tag === 'textarea' || tag === 'select' || tag === 'button'
    || tag === 'dialog') {
    return true
  }

  if (element.isContentEditable === true) return true

  const contentEditable = typeof element.getAttribute === 'function'
    ? element.getAttribute('contenteditable')
    : element.contentEditable
  if (contentEditable != null && String(contentEditable).toLowerCase() !== 'false') return true

  const role = typeof element.getAttribute === 'function'
    ? element.getAttribute('role')
    : element.role
  if (['dialog', 'textbox', 'combobox', 'listbox'].includes(String(role || '').toLowerCase())) {
    return true
  }

  const ariaModal = typeof element.getAttribute === 'function'
    ? element.getAttribute('aria-modal')
    : element['aria-modal']
  if (String(ariaModal || '').toLowerCase() === 'true') return true

  if (hasAttribute(element, 'data-keyboard-ignore') || hasAttribute(element, 'data-editor')) {
    return true
  }

  const className = typeof element.className === 'string' ? element.className : ''
  if (/(^|\s)(editor|script-editor|code-editor|yaml-editor|raw-editor|cm-editor|monaco-editor|ace_editor|modal|dialog|popup)(\s|$)/i.test(className)) {
    return true
  }

  return false
}

/**
 * Whether a KeyboardEvent target belongs to an editor or UI control.
 * Ancestors are checked so a key pressed on a child span inside a modal is
 * also kept out of the device control channel.
 */
export function shouldIgnoreKeyboardTarget(target) {
  let element = elementForTarget(target)
  if (!element) return false

  if (typeof element.closest === 'function') {
    try {
      if (element.closest(IGNORE_TARGET_SELECTOR)) return true
    } catch (error) {
      // A lightweight test double may implement closest but not CSS selectors.
    }
  }

  while (element) {
    if (hasIgnoredShape(element)) return true
    element = element.parentElement || element.parentNode || null
  }
  return false
}

// Keep this alias discoverable for callers that phrase the predicate as a
// positive "is ignored" check.
export const isIgnoredKeyboardTarget = shouldIgnoreKeyboardTarget
export const shouldIgnoreTarget = shouldIgnoreKeyboardTarget
export const isKeyboardTargetIgnored = shouldIgnoreKeyboardTarget

/** Return the Android keycode for an exact KeyboardEvent.code, or null. */
export function mapKeyboardCode(code) {
  if (typeof code !== 'string' || !code) return null
  const value = ANDROID_KEYCODES[code]
  return Number.isInteger(value) && value > 0 ? value : null
}

export const keyCodeForEventCode = mapKeyboardCode
export const androidKeycodeForCode = mapKeyboardCode
export const keycodeForCode = mapKeyboardCode

function hasPressedCode(pressedCodes, code) {
  if (!pressedCodes) return false
  if (typeof pressedCodes.has === 'function') return pressedCodes.has(code)
  if (typeof pressedCodes === 'string') return pressedCodes === code
  try {
    return Array.from(pressedCodes).includes(code)
  } catch (error) {
    return false
  }
}

function addModifierMeta(meta, pressedCodes, group, genericMask, leftMask, rightMask) {
  const left = hasPressedCode(pressedCodes, `${group}Left`)
  const right = hasPressedCode(pressedCodes, `${group}Right`)
  if (left || right) meta |= genericMask
  if (left) meta |= leftMask
  if (right) meta |= rightMask
  return meta
}

/**
 * Calculate Android's meta-state from the currently held physical modifier
 * codes. Both the aggregate bit and left/right bits are sent.
 */
export function getMetaState(pressedCodes) {
  let meta = ANDROID_META.NONE
  meta = addModifierMeta(
    meta, pressedCodes, 'Shift', ANDROID_META.SHIFT_ON,
    ANDROID_META.SHIFT_LEFT_ON, ANDROID_META.SHIFT_RIGHT_ON,
  )
  meta = addModifierMeta(
    meta, pressedCodes, 'Control', ANDROID_META.CTRL_ON,
    ANDROID_META.CTRL_LEFT_ON, ANDROID_META.CTRL_RIGHT_ON,
  )
  meta = addModifierMeta(
    meta, pressedCodes, 'Alt', ANDROID_META.ALT_ON,
    ANDROID_META.ALT_LEFT_ON, ANDROID_META.ALT_RIGHT_ON,
  )
  meta = addModifierMeta(
    meta, pressedCodes, 'Meta', ANDROID_META.META_ON,
    ANDROID_META.META_LEFT_ON, ANDROID_META.META_RIGHT_ON,
  )
  return meta
}

export const computeMetaState = getMetaState
export const calculateMetaState = getMetaState

function metaStateForEvent(event, pressedCodes) {
  let meta = getMetaState(pressedCodes)
  // The event flags make the controller resilient when it starts while a
  // modifier is already held, even if that modifier's keydown was missed.
  if (event?.shiftKey) meta |= ANDROID_META.SHIFT_ON
  if (event?.ctrlKey) meta |= ANDROID_META.CTRL_ON
  if (event?.altKey) meta |= ANDROID_META.ALT_ON
  if (event?.metaKey) meta |= ANDROID_META.META_ON
  return meta
}

function isEnabledValue(value) {
  if (typeof value === 'function') return value() !== false
  if (value && typeof value === 'object' && 'value' in value) return value.value !== false
  return value !== false
}

function eventCode(event) {
  return typeof event?.code === 'string' && event.code ? event.code : null
}

function eventPreventDefault(event) {
  if (typeof event?.preventDefault === 'function') event.preventDefault()
}

/**
 * Create a stateful keyboard event adapter.
 *
 * send receives plain objects ready for the WebRTC control DataChannel:
 * { type: 'key', action: 0|1, keycode, repeat: 0|1, meta }
 */
export function createKeyboardController({
  onKey = null,
  // send is retained as a small backwards-compatible adapter for callers
  // written before the parent-facing onKey contract was finalized.
  send = null,
  enabled = true,
  isEnabled = null,
  shouldIgnoreTarget: ignoreTarget = shouldIgnoreKeyboardTarget,
} = {}) {
  const pressedCodes = new Set()
  let active = enabled
  const emitKey = typeof onKey === 'function'
    ? onKey
    : (typeof send === 'function' ? send : () => {})

  const result = handled => ({ handled })

  function canHandle(event) {
    if (!isEnabledValue(isEnabled == null ? active : isEnabled)) return false
    return typeof ignoreTarget !== 'function' || !ignoreTarget(event?.target)
  }

  function dispatch(code, action, repeat, event) {
    const message = {
      type: 'key',
      action,
      keycode: mapKeyboardCode(code),
      repeat,
      meta: metaStateForEvent(event, pressedCodes),
    }
    emitKey(message)
    return message
  }

  function handleKeydown(event) {
    const code = eventCode(event)
    const keycode = mapKeyboardCode(code)
    if (keycode == null || !canHandle(event)) return result(false)

    // A non-repeat keydown for a held physical key is a browser/DOM duplicate.
    // Genuine browser auto-repeat remains observable through repeat: 1.
    if (pressedCodes.has(code) && !event?.repeat) return result(false)
    pressedCodes.add(code)
    dispatch(code, 0, event?.repeat ? 1 : 0, event)
    eventPreventDefault(event)
    return result(true)
  }

  function handleKeyup(event) {
    const code = eventCode(event)
    const keycode = mapKeyboardCode(code)
    if (keycode == null || !pressedCodes.has(code)) return result(false)

    // Release a key that this controller pressed even if focus moved into an
    // input/editor before keyup; otherwise the remote device could get stuck.
    pressedCodes.delete(code)
    dispatch(code, 1, 0, event)
    eventPreventDefault(event)
    return result(true)
  }

  function releaseAll() {
    // Release ordinary keys before modifiers so their keyup still carries the
    // modifier state. Modifier keyups then naturally carry meta: 0.
    const codes = Array.from(pressedCodes).sort((a, b) => {
      const aModifier = MODIFIER_CODES.has(a) ? 1 : 0
      const bModifier = MODIFIER_CODES.has(b) ? 1 : 0
      return aModifier - bModifier
    })
    const released = []
    for (const code of codes) {
      if (!pressedCodes.has(code)) continue
      pressedCodes.delete(code)
      released.push(dispatch(code, 1, 0))
    }
    return result(released.length > 0)
  }

  function setEnabled(value) {
    active = value
  }

  function isControllerEnabled() {
    return isEnabledValue(isEnabled == null ? active : isEnabled)
  }

  function handleWindowBlur() {
    return releaseAll()
  }

  function handleVisibilityChange(eventOrHidden) {
    const hidden = typeof eventOrHidden === 'boolean'
      ? eventOrHidden
      : eventOrHidden?.target?.hidden ?? eventOrHidden?.hidden
    return hidden ? releaseAll() : result(false)
  }

  return {
    handleKeydown,
    handleKeyup,
    handleKeyDown: handleKeydown,
    handleKeyUp: handleKeyup,
    onKeydown: handleKeydown,
    onKeyup: handleKeyup,
    keydown: handleKeydown,
    keyup: handleKeyup,
    releaseAll,
    handleWindowBlur,
    onWindowBlur: handleWindowBlur,
    handleVisibilityChange,
    onVisibilityChange: handleVisibilityChange,
    setEnabled,
    isEnabled: isControllerEnabled,
    isPressed: code => pressedCodes.has(code),
    getPressedCodes: () => Array.from(pressedCodes),
    getPressedKeyCodes: () => Array.from(pressedCodes),
    getMetaState: () => getMetaState(pressedCodes),
  }
}

export const createKeyController = createKeyboardController
