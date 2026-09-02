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

import { mapKeyboardCode } from './keyboard-control'

export const KEYMAP_VERSION = 1
export const KEYMAP_ACTION_TYPES = Object.freeze(['tap', 'swipe', 'raw_key', 'hold'])

const ROOT_KEYS = new Set(['version', 'name', 'bindings'])
const ACTION_KEYS = {
  tap: new Set(['type', 'at']),
  swipe: new Set(['type', 'from', 'to', 'duration_ms']),
  raw_key: new Set(['type', 'code', 'keycode']),
  // pointer_id is reserved for a future multi-pointer wire protocol.
  hold: new Set(['type', 'at', 'from', 'to', 'pointer_id']),
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
  if (type === 'hold' && action.pointer_id !== undefined
    && (!Number.isInteger(action.pointer_id)
      || action.pointer_id < 0 || action.pointer_id > 31)) {
    errors.push(makeIssue(
      path + '.pointer_id',
      'pointer_id 必须是 0~31 的整数',
      'keymap.pointer_id',
    ))
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
      if (typeof binding.key === 'string' && binding.key.trim() && mapKeyboardCode(binding.key) == null) {
        errors.push(makeIssue(
          path + '.key',
          '无法映射 KeyboardEvent.code：' + binding.key,
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
  const suppressedKeyups = new Set()
  const fallbackController = fallback || keyboard
  const emitControl = typeof sendControl === 'function'
    ? sendControl
    : (typeof send === 'function' ? send : () => {})

  function getCurrentKeymap() {
    if (typeof getKeymap === 'function') return getKeymap()
    return readValue(keymapSource, null)
  }

  function routerEnabled() {
    return readValue(isEnabled === null ? active : isEnabled, true) !== false
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
      emitControl({ type: 'tap', x: point.x, y: point.y })
      return true
    }
    if (action.type === 'swipe') {
      const from = pointFor(action.from, action, event)
      const to = pointFor(action.to, action, event)
      if (!from || !to) return false
      emitControl({
        type: 'swipe',
        x1: from.x,
        y1: from.y,
        x2: to.x,
        y2: to.y,
        duration: action.duration_ms,
      })
      return true
    }
    return false
  }

  function sendRawKey(action, event, keyAction) {
    const keycode = rawKeycode(action)
    if (keycode == null) return false
    emitControl({
      type: 'key',
      action: keyAction,
      keycode,
      repeat: keyAction === 0 && event && event.repeat ? 1 : 0,
      meta: 0,
    })
    return true
  }

  function sendHold(action, event, keyAction) {
    const point = pointFor(action.at, action, event)
    if (!point) return false
    const message = {
      type: 'touch',
      action: keyAction === 0 ? 'down' : 'up',
      x: point.x,
      y: point.y,
    }
    if (Number.isInteger(action.pointer_id)) message.pointer_id = action.pointer_id
    emitControl(message)
    return true
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
    const code = eventCode(event)
    if (!code || !routerEnabled() || modeIsText(mode)) return fallbackDown(event)

    const binding = indexKeymap(getCurrentKeymap()).get(code)
    const action = actionForBinding(binding)
    if (!action) return fallbackDown(event)

    const current = held.get(code)
    if (current) {
      if (event && event.repeat && action.type === 'raw_key') {
        sendRawKey(current.action, event, 0)
      }
      return mappedResult(current.action, code, false)
    }

    let sent = false
    if (action.type === 'tap' || action.type === 'swipe') {
      sent = !(event && event.repeat) && sendPointAction(action, event)
    } else if (action.type === 'raw_key') {
      sent = sendRawKey(action, event, 0)
    } else if (action.type === 'hold') {
      sent = !(event && event.repeat) && sendHold(action, event, 0)
    }
    if (!sent) return fallbackDown(event)

    held.set(code, { action: clone(action) })
    preventDefault(event)
    return mappedResult(action, code, true)
  }

  function handleKeyup(event) {
    const code = eventCode(event)
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
    else if (current.action.type === 'hold') sent = sendHold(current.action, event, 1)
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
        if (sendHold(current.action, null, 1)) released += 1
      } else {
        released += 1
      }
      if (suppressKeyups) suppressedKeyups.add(code)
    })
    return released
  }

  function releaseAll() {
    const mappedReleased = releaseMapped(false)
    const fallback = fallbackReleaseAll()
    return {
      handled: mappedReleased > 0 || fallback.handled,
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
    releaseAll,
    releaseMapped,
    handleWindowBlur,
    handleVisibilityChange,
    setKeymap,
    setMapping: setKeymap,
    setMode,
    setEnabled(value) { active = value },
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
