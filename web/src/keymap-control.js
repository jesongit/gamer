/**
 * Console keyboard mapping router.
 *
 * This module owns no DOM listeners and has no Vue dependency. The Console
 * supplies:
 *   createKeymapController({
 *     getKeymap: () => activeKeymap,
 *     sendControl: existingDataChannelSendControl,
 *     getVideoSize: () => ({ width, height }),
 *     fallback: existingKeyboardController,
 *   })
 *
 * Coordinates in a keymap are normalized [0, 1] values. They are converted
 * to current video pixels before the existing sendControl callback receives
 * tap/swipe/touch messages.
 */

import { ANDROID_META, mapKeyboardCode } from './keyboard-control'

export const KEYMAP_VERSION = 1
export const KEYMAP_ACTION_TYPES = Object.freeze(['tap', 'swipe', 'raw_key', 'hold'])
export const INPUT_PROTOCOL_VERSION = 'gamer-input@1'

const MOUSE_SELECTORS = new Set([
  'MouseLeft', 'MouseMiddle', 'MouseRight', 'MouseBack', 'MouseForward', 'MouseMove',
])

/** Validate the closed selector vocabulary shared with server InputEvent. */
export function isInputSelector(value) {
  if (typeof value !== 'string' || !value) return false
  if (mapKeyboardCode(value) != null || MOUSE_SELECTORS.has(value)) return true
  const button = value.match(/^GamepadButton(\d+)$/)
  if (button) return Number(button[1]) <= 31
  const axis = value.match(/^GamepadAxis(\d+)$/)
  return !!axis && Number(axis[1]) <= 7
}

/** Convert browser input values into the small event envelope used by Core. */
export function normalizeInputEvent(event, phase = undefined) {
  if (!event || typeof event !== 'object') return null
  if (typeof event.code === 'string' && event.code) {
    const type = phase === 'up' || event.type === 'keyup' ? 'key_up' : 'key_down'
    return {
      type,
      code: event.code,
      ...(type === 'key_down' ? { repeat: !!event.repeat } : {}),
      meta: Number.isInteger(event.meta) && event.meta >= 0 ? event.meta : 0,
    }
  }
  const mouseType = event.type
  if ((phase === 'move' || mouseType === 'mousemove')
    && Number.isFinite(Number(event.x)) && Number.isFinite(Number(event.y))) {
    return {
      type: 'mouse_move',
      x: Math.max(0, Math.round(event.x)),
      y: Math.max(0, Math.round(event.y)),
      delta_x: Number.isFinite(Number(event.movementX)) ? Math.round(event.movementX) : 0,
      delta_y: Number.isFinite(Number(event.movementY)) ? Math.round(event.movementY) : 0,
    }
  }
  if ((phase === 'wheel' || mouseType === 'wheel')
    && Number.isFinite(Number(event.x)) && Number.isFinite(Number(event.y))) {
    return {
      type: 'wheel',
      x: Math.max(0, Math.round(event.x)),
      y: Math.max(0, Math.round(event.y)),
      delta_x: Number.isFinite(Number(event.deltaX)) ? Math.round(event.deltaX) : 0,
      delta_y: Number.isFinite(Number(event.deltaY)) ? Math.round(event.deltaY) : 0,
    }
  }
  if (Number.isInteger(event.button) && Number.isFinite(Number(event.x)) && Number.isFinite(Number(event.y))) {
    const type = phase === 'up' || mouseType === 'mouseup' ? 'mouse_up' : 'mouse_down'
    return { type, button: event.button, x: Math.max(0, Math.round(event.x)), y: Math.max(0, Math.round(event.y)) }
  }
  if (event.kind === 'gamepad_button' && Number.isInteger(event.index)) {
    return { type: 'gamepad_button', index: event.index, pressed: !!event.pressed, value: Number(event.value) || 0 }
  }
  if (event.kind === 'gamepad_axis' && Number.isInteger(event.index)) {
    return { type: 'gamepad_axis', index: event.index, value: Number(event.value) || 0 }
  }
  return null
}

/** Build the shared DataChannel touch-phase wire shape for mouse and keymap input. */
export function buildTouchPhase(action, pointerId, x, y) {
  return {
    type: 'touch',
    action,
    pointer_id: pointerId,
    x,
    y,
  }
}

const ROOT_KEYS = new Set(['version', 'name', 'bindings'])
const ACTION_KEYS = {
  tap: new Set(['type', 'at']),
  swipe: new Set(['type', 'from', 'to', 'duration_ms']),
  raw_key: new Set(['type', 'code', 'keycode']),
  // pointer_id is a runtime transport field, never persisted in keymap YAML.
  hold: new Set(['type', 'at']),
}

function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function readValue(value, fallback) {
  if (typeof value === 'function') return value()
  if (value && typeof value === 'object' && 'value' in value) return value.value
  return value === undefined ? fallback : value
}

function makeIssue(path, message, code) {
  return { path, message, code: code || 'keymap.invalid' }
}

function unknownKeys(value, allowed, path, errors) {
  Object.keys(value).forEach(key => {
    if (!allowed.has(key)) {
      errors.push(makeIssue(path + '.' + key, '不支持字段：' + key, 'keymap.unknown_field'))
    }
  })
}

function validPoint(value) {
  return Array.isArray(value)
    && value.length === 2
    && value.every(item => typeof item === 'number'
      && Number.isFinite(item) && item >= 0 && item <= 1)
}

function validatePoint(value, path, errors) {
  if (!validPoint(value)) {
    errors.push(makeIssue(
      path,
      '坐标必须是 [0, 1] 范围内的两个数字',
      'keymap.coordinate',
    ))
  }
}

function validateAction(action, path, errors) {
  if (!isRecord(action)) {
    errors.push(makeIssue(path, '动作必须是对象', 'keymap.action'))
    return
  }

  const type = action.type
  if (!KEYMAP_ACTION_TYPES.includes(type)) {
    errors.push(makeIssue(
      path + '.type',
      '未知动作：' + (type || '（空）'),
      'keymap.action_type',
    ))
    return
  }

  unknownKeys(action, ACTION_KEYS[type], path, errors)
  if (type === 'tap' || type === 'hold') {
    validatePoint(action.at, path + '.at', errors)
  }
  if (type === 'swipe') {
    validatePoint(action.from, path + '.from', errors)
    validatePoint(action.to, path + '.to', errors)
    if (!Number.isInteger(action.duration_ms)
      || action.duration_ms < 1 || action.duration_ms > 600000) {
      errors.push(makeIssue(
        path + '.duration_ms',
        'duration_ms 必须是 1~600000 的整数',
        'keymap.duration',
      ))
    }
  }
  if (type === 'raw_key') {
    const hasCode = typeof action.code === 'string' && action.code.length > 0
    const hasKeycode = Number.isInteger(action.keycode)
      && action.keycode >= 1 && action.keycode <= 1000
    if (!hasCode && !hasKeycode) {
      errors.push(makeIssue(
        path,
        'raw_key 必须提供有效的 code 或 keycode',
        'keymap.raw_key',
      ))
    }
    if (hasCode && mapKeyboardCode(action.code) == null) {
      errors.push(makeIssue(
        path + '.code',
        '无法映射 KeyboardEvent.code：' + action.code,
        'keymap.raw_key_code',
      ))
    }
    if (action.keycode !== undefined && !hasKeycode) {
      errors.push(makeIssue(
        path + '.keycode',
        'keycode 必须是 1~1000 的整数',
        'keymap.raw_keycode',
      ))
    }
  }
}

/**
 * Validate a keymap model without throwing.
 * Result: { valid, issues, errors }, where issues and errors are the same
 * array for convenient use by diagnostics and forms.
 */
export function validateKeymap(value) {
  const errors = []
  if (!isRecord(value)) {
    const error = makeIssue('$', '按键映射必须是对象', 'keymap.object')
    return { valid: false, issues: [error], errors: [error] }
  }

  unknownKeys(value, ROOT_KEYS, '$', errors)
  if (!Number.isInteger(value.version) || value.version !== KEYMAP_VERSION) {
    errors.push(makeIssue(
      '$.version',
      'version 必须为 ' + KEYMAP_VERSION,
      'keymap.version',
    ))
  }
  if (typeof value.name !== 'string' || !value.name.trim()) {
    errors.push(makeIssue('$.name', '方案名称不能为空', 'keymap.name'))
  }
  if (!Array.isArray(value.bindings)) {
    errors.push(makeIssue('$.bindings', 'bindings 必须是数组', 'keymap.bindings'))
  } else {
    const seen = new Set()
    value.bindings.forEach((binding, index) => {
      const path = '$.bindings[' + index + ']'
      if (!isRecord(binding)) {
        errors.push(makeIssue(path, '绑定必须是对象', 'keymap.binding'))
        return
      }
      unknownKeys(binding, new Set(['key', 'action']), path, errors)
      if (typeof binding.key !== 'string' || !binding.key.trim()) {
        errors.push(makeIssue(
          path + '.key',
          '按键必须使用 KeyboardEvent.code',
          'keymap.key',
        ))
      } else if (seen.has(binding.key)) {
        errors.push(makeIssue(
          path + '.key',
          '重复绑定按键：' + binding.key,
          'keymap.duplicate_key',
        ))
      } else {
        seen.add(binding.key)
      }
      if (typeof binding.key === 'string' && binding.key.trim() && !isInputSelector(binding.key)) {
        errors.push(makeIssue(
          path + '.key',
          '无法映射输入选择器：' + binding.key,
          'keymap.key_code',
        ))
      }
      validateAction(binding.action, path + '.action', errors)
    })
  }
  return { valid: errors.length === 0, issues: errors, errors }
}

export const checkKeymap = validateKeymap

function clone(value) {
  if (typeof structuredClone === 'function') {
    try { return structuredClone(value) } catch (error) { /* Vue proxy 等对象走 JSON 兜底 */ }
  }
  return JSON.parse(JSON.stringify(value))
}

export function normalizeKeymap(value) {
  const result = validateKeymap(value)
  if (!result.valid) {
    const error = new Error(result.issues
      .map(item => item.path + ': ' + item.message).join('；'))
    error.code = 'keymap.invalid'
    error.issues = result.issues
    throw error
  }
  return clone(value)
}

function actionForBinding(binding) {
  return binding && isRecord(binding.action) ? binding.action : null
}

export function indexKeymap(value) {
  const keymap = readValue(value, null)
  const index = new Map()
  if (!isRecord(keymap) || !Array.isArray(keymap.bindings)) return index
  keymap.bindings.forEach(binding => {
    if (!isRecord(binding) || typeof binding.key !== 'string' || index.has(binding.key)) return
    const action = actionForBinding(binding)
    if (action && KEYMAP_ACTION_TYPES.includes(action.type)) index.set(binding.key, binding)
  })
  return index
}

export const buildKeymapIndex = indexKeymap

function pointObject(point) {
  if (Array.isArray(point)) return { x: point[0], y: point[1] }
  if (isRecord(point)) return { x: point.x, y: point.y }
  return null
}

function mapPointResult(value) {
  const point = pointObject(value)
  if (!point || !Number.isFinite(Number(point.x)) || !Number.isFinite(Number(point.y))) {
    return null
  }
  return { x: Number(point.x), y: Number(point.y) }
}

function videoSize(value) {
  const size = readValue(value, null)
  if (Array.isArray(size) && size.length >= 2) {
    return { width: Number(size[0]), height: Number(size[1]) }
  }
  if (isRecord(size)) return { width: Number(size.width), height: Number(size.height) }
  return null
}

function rawKeycode(action) {
  if (Number.isInteger(action && action.keycode)
    && action.keycode >= 1 && action.keycode <= 1000) return action.keycode
  return mapKeyboardCode(action && action.code)
}

function eventCode(event) {
  return typeof (event && event.code) === 'string' && event.code ? event.code : null
}

function preventDefault(event) {
  if (typeof (event && event.preventDefault) === 'function') event.preventDefault()
}

function fallbackResult(value) {
  if (value && typeof value === 'object' && 'handled' in value) return value
  return { handled: value !== false }
}

/**
 * DataChannel senders historically returned nothing, while the Console
 * sender returns true/false to report whether the channel actually sent.
 * Keep the old callback contract working, but make an explicit false (or a
 * result object carrying sent=false) observable by stateful actions.
 */
function controlWasSent(value) {
  if (value && typeof value === 'object') {
    if ('sent' in value) return value.sent === true
    if ('dataChannelSent' in value) return value.dataChannelSent === true
  }
  return value !== false
}

function modeIsText(value) {
  return readValue(value, 'game') === 'text'
}

/**
 * Stable runtime contract:
 *   createKeymapController({ getKeymap, sendControl, getVideoSize })
 *   controller.handleKeydown(event)
 *   controller.handleKeyup(event)
 *   controller.releaseAll()
 *
 * The fallback option should be the existing keyboard controller. If a
 * mapping is absent, invalid at runtime, or text mode is active, this router
 * delegates to it exactly once.
 */
export function createKeymapController({
  getKeymap = null,
  keymap = null,
  mapping = undefined,
  mode = 'game',
  enabled = true,
  isEnabled = null,
  sendControl = null,
  send = null,
  remote = false,
  sendInputEvent = null,
  sendStateControl = null,
  getKeyMetaState = null,
  getVideoSize = null,
  deviceSize = null,
  coordinateMapper = null,
  fallback = null,
  keyboard = null,
  onFallbackKeydown = null,
  onFallbackKeyup = null,
  onFallbackReleaseAll = null,
} = {}) {
  let keymapSource = mapping !== undefined ? mapping : keymap
  let active = enabled
  const held = new Map()
  const remoteHeld = new Set()
  const suppressedKeyups = new Set()
  const fallbackController = fallback || keyboard
  const emitControl = typeof sendControl === 'function'
    ? sendControl
    : (typeof send === 'function' ? send : () => {})
  // A caller may provide a DataChannel-only sender separately from the
  // general sender (which is allowed to REST-fallback for one-shot actions).
  // The legacy `send` alias is also a useful DataChannel-only sender when it
  // differs from sendControl.
  const emitStateControl = typeof sendStateControl === 'function'
    ? sendStateControl
    : (typeof send === 'function' && send !== emitControl ? send : emitControl)
  const emitInput = typeof sendInputEvent === 'function' ? sendInputEvent : emitControl

  function getCurrentKeymap() {
    if (typeof getKeymap === 'function') return getKeymap()
    return readValue(keymapSource, null)
  }

  function routerEnabled() {
    return readValue(isEnabled === null ? active : isEnabled, true) !== false
  }

  function remoteEnabled() {
    return readValue(remote, false) === true
  }

  function forwardRemoteInput(event, phase, domEvent = null) {
    const normalized = normalizeInputEvent(event, phase)
    if (!normalized) return { handled: false, remote: true, sent: false }
    const sent = controlWasSent(emitInput({ type: 'input_event', event: normalized }))
    if (domEvent) preventDefault(domEvent)
    if (normalized.type === 'key_down') remoteHeld.add(normalized.code)
    if (normalized.type === 'key_up') remoteHeld.delete(normalized.code)
    return { handled: true, mapped: true, remote: true, sent }
  }

  function fallbackDown(event) {
    if (typeof onFallbackKeydown === 'function') return fallbackResult(onFallbackKeydown(event))
    if (typeof fallbackController?.handleKeydown === 'function') return fallbackResult(fallbackController.handleKeydown(event))
    if (typeof fallbackController?.handleKeyDown === 'function') return fallbackResult(fallbackController.handleKeyDown(event))
    return { handled: false }
  }

  function fallbackUp(event) {
    if (typeof onFallbackKeyup === 'function') return fallbackResult(onFallbackKeyup(event))
    if (typeof fallbackController?.handleKeyup === 'function') return fallbackResult(fallbackController.handleKeyup(event))
    if (typeof fallbackController?.handleKeyUp === 'function') return fallbackResult(fallbackController.handleKeyUp(event))
    return { handled: false }
  }

  function fallbackReleaseAll() {
    if (typeof onFallbackReleaseAll === 'function') return fallbackResult(onFallbackReleaseAll())
    if (typeof fallbackController?.releaseAll === 'function') return fallbackResult(fallbackController.releaseAll())
    return { handled: false }
  }

  function pointFor(point, action, event) {
    const normalized = pointObject(point)
    if (!normalized) return null
    if (typeof coordinateMapper === 'function') {
      const mapped = mapPointResult(coordinateMapper(
        [normalized.x, normalized.y], action, event,
      ))
      if (mapped) return mapped
    }
    const size = videoSize(getVideoSize || deviceSize)
    if (size && Number.isFinite(size.width) && Number.isFinite(size.height)
      && size.width > 0 && size.height > 0) {
      return {
        x: Math.round(normalized.x * size.width),
        y: Math.round(normalized.y * size.height),
      }
    }
    return normalized
  }

  function sendPointAction(action, event) {
    if (action.type === 'tap') {
      const point = pointFor(action.at, action, event)
      if (!point) return false
      return controlWasSent(emitControl({ type: 'tap', x: point.x, y: point.y }))
    }
    if (action.type === 'swipe') {
      const from = pointFor(action.from, action, event)
      const to = pointFor(action.to, action, event)
      if (!from || !to) return false
      return controlWasSent(emitControl({
        type: 'swipe',
        x1: from.x,
        y1: from.y,
        x2: to.x,
        y2: to.y,
        duration: action.duration_ms,
      }))
    }
    return false
  }

  function sendRawKey(action, event, keyAction) {
    const keycode = rawKeycode(action)
    if (keycode == null) return false
    return controlWasSent(emitStateControl({
      type: 'key',
      action: keyAction,
      keycode,
      repeat: keyAction === 0 && event && event.repeat ? 1 : 0,
      meta: keyMetaForEvent(event),
    }))
  }

  function sendTouchPhase(point, phase, pointerId) {
    if (!point) return false
    const message = buildTouchPhase(phase, pointerId, point.x, point.y)
    return controlWasSent(emitStateControl(message))
  }

  function allocatePointerId() {
    for (let pointerId = 1; pointerId <= 31; pointerId += 1) {
      if (!Array.from(held.values()).some(item => item.pointerId === pointerId)) {
        return pointerId
      }
    }
    return null
  }

  function keyMetaForEvent(event) {
    const provider = typeof getKeyMetaState === 'function'
      ? getKeyMetaState
      : fallbackController?.getMetaState
    let meta = 0
    if (typeof provider === 'function') {
      const value = Number(provider())
      if (Number.isInteger(value) && value >= 0) meta = value
    }
    // DOM flags cover a modifier keydown that the fallback controller has not
    // observed because the mapping layer consumed that event.
    if (event?.shiftKey) meta |= ANDROID_META.SHIFT_ON
    if (event?.ctrlKey) meta |= ANDROID_META.CTRL_ON
    if (event?.altKey) meta |= ANDROID_META.ALT_ON
    if (event?.metaKey) meta |= ANDROID_META.META_ON
    return meta
  }

  function sendHoldDown(action, event, pointerId) {
    const point = pointFor(action.at, action, event)
    if (!point || pointerId == null) return { sent: false, point: null, pointerId: null }
    return {
      sent: sendTouchPhase(point, 'down', pointerId),
      point,
      pointerId,
    }
  }

  function sendHoldUp(current) {
    return sendTouchPhase(current.point, 'up', current.pointerId)
  }

  function mappedResult(action, code, sent) {
    return {
      handled: true,
      mapped: true,
      action: action.type,
      code,
      sent: sent !== false,
    }
  }

  function handleKeydown(event) {
    if (remoteEnabled() && !modeIsText(mode)) return forwardRemoteInput(event, 'down', event)
    const code = eventCode(event)
    if (!code || !routerEnabled() || modeIsText(mode)) return fallbackDown(event)

    const binding = indexKeymap(getCurrentKeymap()).get(code)
    const action = actionForBinding(binding)
    if (!action) return fallbackDown(event)

    const current = held.get(code)
    if (current) {
      preventDefault(event)
      if (event && event.repeat && current.action.type === 'raw_key') {
        const sent = sendRawKey(current.action, event, 0)
        return mappedResult(current.action, code, sent)
      }
      return mappedResult(current.action, code, false)
    }

    let sent = false
    let state = null
    if (action.type === 'tap' || action.type === 'swipe') {
      sent = !(event && event.repeat) && sendPointAction(action, event)
    } else if (action.type === 'raw_key') {
      sent = sendRawKey(action, event, 0)
    } else if (action.type === 'hold') {
      if (!(event && event.repeat)) {
        const pointerId = allocatePointerId()
        state = sendHoldDown(action, event, pointerId)
        sent = state.sent
      }
    }
    if (!sent) {
      // A stateful mapping must never become an unrelated fallback key/REST
      // press when its DataChannel is unavailable.
      if (action.type === 'hold' || action.type === 'raw_key') {
        // Consume the matching keyup too.  There is no held state to release,
        // but forwarding that keyup to the ordinary keyboard controller would
        // turn a failed mapped action into an unrelated Android key event.
        suppressedKeyups.add(code)
        preventDefault(event)
        return mappedResult(action, code, false)
      }
      return fallbackDown(event)
    }

    held.set(code, {
      action: clone(action),
      ...(action.type === 'hold'
        ? { point: state.point, pointerId: state.pointerId }
        : {}),
    })
    preventDefault(event)
    return mappedResult(action, code, true)
  }

  function handleKeyup(event) {
    const code = eventCode(event)
    if (remoteEnabled() || (code && remoteHeld.has(code))) {
      return forwardRemoteInput(event, 'up', event)
    }
    if (!code) return fallbackUp(event)
    if (suppressedKeyups.delete(code)) {
      preventDefault(event)
      return { handled: true, mapped: true, released: false }
    }

    const current = held.get(code)
    if (!current) return fallbackUp(event)
    held.delete(code)
    let sent = true
    if (current.action.type === 'raw_key') sent = sendRawKey(current.action, event, 1)
    else if (current.action.type === 'hold') sent = sendHoldUp(current)
    preventDefault(event)
    return mappedResult(current.action, code, sent)
  }

  function releaseMapped(suppressKeyups) {
    const codes = Array.from(held.keys())
    let released = 0
    codes.forEach(code => {
      const current = held.get(code)
      if (!current) return
      held.delete(code)
      if (current.action.type === 'raw_key') {
        if (sendRawKey(current.action, null, 1)) released += 1
      } else if (current.action.type === 'hold') {
        if (sendHoldUp(current)) released += 1
      } else {
        released += 1
      }
      if (suppressKeyups) suppressedKeyups.add(code)
    })
    return released
  }

  function releaseAll() {
    let remoteReleased = 0
    for (const code of remoteHeld) {
      if (controlWasSent(emitInput({
        type: 'input_event',
        event: { type: 'key_up', code, meta: 0 },
      }))) remoteReleased += 1
    }
    remoteHeld.clear()
    const mappedReleased = releaseMapped(false)
    const fallback = fallbackReleaseAll()
    return {
      handled: remoteReleased > 0 || mappedReleased > 0 || fallback.handled,
      remoteReleased,
      mappedReleased,
      fallback,
    }
  }

  function switchMode() {
    const codes = Array.from(held.keys())
    releaseMapped(true)
    codes.forEach(code => suppressedKeyups.add(code))
  }

  function setKeymap(nextKeymap) {
    switchMode()
    keymapSource = nextKeymap
    return nextKeymap
  }

  function setMode(nextMode) {
    switchMode()
    if (mode && typeof mode === 'object' && 'value' in mode) mode.value = nextMode
    return nextMode
  }

  function handleWindowBlur() {
    return releaseAll()
  }

  function handleVisibilityChange(eventOrHidden) {
    const hidden = typeof eventOrHidden === 'boolean'
      ? eventOrHidden
      : eventOrHidden?.target?.hidden ?? eventOrHidden?.hidden
    return hidden ? releaseAll() : { handled: false }
  }

  function handleInputEvent(event, phase = undefined, domEvent = null) {
    if (!remoteEnabled() || modeIsText(mode)) return { handled: false, remote: false, sent: false }
    return forwardRemoteInput(event, phase, domEvent)
  }

  function destroy() {
    releaseAll()
    suppressedKeyups.clear()
  }

  return {
    handleKeydown,
    handleKeyup,
    handleKeyDown: handleKeydown,
    handleKeyUp: handleKeyup,
    keydown: handleKeydown,
    keyup: handleKeyup,
    handleInputEvent,
    forwardInputEvent: handleInputEvent,
    releaseAll,
    releaseMapped,
    handleWindowBlur,
    handleVisibilityChange,
    setKeymap,
    setMapping: setKeymap,
    setMode,
    setEnabled(value) {
      if (value === false) releaseAll()
      active = value
    },
    setRemote(value) {
      if (value === false) remoteHeld.clear()
      if (remote && typeof remote === 'object' && 'value' in remote) remote.value = value
      else remote = value
    },
    isEnabled: () => routerEnabled(),
    getKeymap: getCurrentKeymap,
    getBinding: code => indexKeymap(getCurrentKeymap()).get(code) || null,
    isPressed: code => held.has(code),
    getPressedCodes: () => Array.from(held.keys()),
    destroy,
    dispose: destroy,
  }
}

export const createKeymapRouter = createKeymapController
export const createKeyboardMappingController = createKeymapController
