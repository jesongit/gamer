import { computed, ref } from 'vue'
import { videoApi } from './videoApi'
import { GAMER_YAML_PLUGIN_ID } from '../../gamer-plugin-ids'
/**
 * gamer.yaml 依赖能力探测（简化计划 Phase 4/5）：视频工作台的模板创建/模板
 * 离线测试/草稿生成与保存依赖 gamer.yaml 的公开动作（template.create_from_frame
 * 等，清单由 gamer.yaml actions.rs 集中声明，经 POST /api/extensions/gamer.yaml/call
 * 分发）。依赖判定走**运行时能力发现**（GET /api/extensions/gamer.yaml/capabilities：
 * 目标 Running + 动作在公开清单内），manifest `[[dependencies]]` 只表达插件
 * 关系、不代替运行时检查。gamer.yaml 缺失/停用时：视频导入、录制、播放、
 * 标记、项目、校准**不受影响**，仅 YAML 相关制作入口禁用并给出依赖提示。
 */

/** 扩展快照列表 → gamer.yaml 是否 Running（快照形态的兜底判定，保留给无
 * capabilities 端点的旧服务端场景）。 */
export function isGamerYamlRunning(extensions) {
  const list = Array.isArray(extensions) ? extensions : []
  const snapshot = list.find(item => item?.id === GAMER_YAML_PLUGIN_ID)
  return snapshot?.state === 'running'
}

const LIFECYCLE_EVENTS = [
  // The first event is the browser-side companion to the existing
  // `extensions-changed` Vue event. The aliases make this small bridge useful
  // to hosts which already publish a DOM lifecycle notification.
  'gamer:extensions-changed',
  'gamer:extension-lifecycle',
  'extensions-changed',
]

function unwrap(value) {
  if (typeof value === 'function') return unwrap(value())
  if (value && typeof value === 'object' && 'value' in value) return unwrap(value.value)
  return value
}

function hasValue(value) {
  return value !== undefined && value !== null && !(typeof value === 'string' && value.trim() === '')
}

function normalizedVersion(value) {
  const text = String(value ?? '').trim().replace(/^v/i, '')
  if (!text) return null
  const match = text.match(/^(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:-([0-9A-Za-z.-]+))?$/)
  if (!match) return null
  return {
    major: Number(match[1]),
    minor: Number(match[2] || 0),
    patch: Number(match[3] || 0),
    prerelease: match[4] || '',
  }
}

function compareVersions(left, right) {
  const a = normalizedVersion(left)
  const b = normalizedVersion(right)
  if (!a || !b) return null
  for (const key of ['major', 'minor', 'patch']) {
    if (a[key] !== b[key]) return a[key] < b[key] ? -1 : 1
  }
  if (!a.prerelease && b.prerelease) return 1
  if (a.prerelease && !b.prerelease) return -1
  if (a.prerelease !== b.prerelease) return a.prerelease < b.prerelease ? -1 : 1
  return 0
}

function exactOrPrefixVersion(actual, expected) {
  const value = normalizedVersion(actual)
  if (!value) return false
  const requested = String(expected ?? '').trim().replace(/^v/i, '')
  if (!requested) return false
  if (/^\d+$/.test(requested)) return value.major === Number(requested)
  if (/^\d+\.\d+$/.test(requested)) {
    const [major, minor] = requested.split('.').map(Number)
    return value.major === major && value.minor === minor
  }
  return compareVersions(actual, requested) === 0
}

/**
 * Small semver matcher for capability metadata. The server remains the
 * authority; this only prevents an obviously incompatible cached action from
 * being presented as callable while a newer capability response is settling.
 */
function versionSatisfies(actual, requirement) {
  actual = unwrap(actual)
  requirement = unwrap(requirement)
  if (!hasValue(actual) || !hasValue(requirement)) return false
  if (Array.isArray(requirement)) return requirement.some(item => versionSatisfies(actual, item))
  if (typeof requirement === 'number') return exactOrPrefixVersion(actual, requirement)

  const requested = String(requirement).trim()
  if (!requested) return false
  if (requested.includes('||')) {
    return requested.split('||').some(item => versionSatisfies(actual, item.trim()))
  }
  if (/^[~^]/.test(requested)) {
    const operator = requested[0]
    const baseline = normalizedVersion(requested.slice(1))
    const current = normalizedVersion(actual)
    if (!baseline || !current) return false
    const lower = compareVersions(actual, requested.slice(1))
    if (lower === null || lower < 0) return false
    if (operator === '~') {
      return current.major === baseline.major && current.minor === baseline.minor
    }
    if (baseline.major > 0) return current.major === baseline.major
    if (baseline.minor > 0) {
      return current.major === 0 && current.minor === baseline.minor
    }
    return current.major === 0 && current.minor === 0 && current.patch === baseline.patch
  }
  if (/^(?:>=|<=|>|<)/.test(requested)) {
    const requirements = requested.match(/(?:>=|<=|>|<)\s*[^\s]+/g) || []
    return requirements.length > 0 && requirements.every(requirement => {
      const match = requirement.match(/^(>=|<=|>|<)\s*(.+)$/)
      const result = match && compareVersions(actual, match[2])
      if (result === null) return false
      return { '>': result > 0, '>=': result >= 0, '<': result < 0, '<=': result <= 0 }[match[1]]
    })
  }
  return exactOrPrefixVersion(actual, requested)
}

function firstDefined(...values) {
  return values.find(value => value !== undefined && value !== null)
}

function explicitFalse(...values) {
  return values.some(value => value === false)
}

function actionName(action) {
  return String(action || '').trim()
}

function normalizeActions(value) {
  if (!Array.isArray(value)) return []
  return value
    .map(item => typeof item === 'string' ? { action: item } : item)
    .filter(item => item && typeof item === 'object' && actionName(item.action))
}

function requiredContextKeys(snapshot, descriptor, callOptions) {
  const from = value => {
    if (Array.isArray(value)) return value
      .map(item => String(item || '').trim())
      .filter(Boolean)
    if (typeof value === 'string' && value.trim()) return [value.trim()]
    if (value && typeof value === 'object') {
      const declared = firstDefined(value.required, value.fields, value.keys)
      if (declared !== undefined && declared !== null) return from(declared)
      return Object.entries(value)
        .filter(([, requirement]) => requirement !== false && requirement?.available !== false)
        .map(([key]) => key)
    }
    return []
  }
  const declared = [
    snapshot?.required_context,
    snapshot?.requires_context,
    descriptor?.required_context,
    descriptor?.requires_context,
    descriptor?.context_required,
    // `context` is used by some capability producers for the required field
    // list. The current Gamer server omits it, so this is backward-compatible.
    descriptor?.context,
    callOptions?.requiredContext,
  ].flatMap(from)
  return [...new Set(declared)]
}

function contextValue(context, key) {
  const source = unwrap(context)
  if (!source || typeof source !== 'object') return undefined
  const aliases = {
    package_id: ['package_id', 'packageId', 'content_package', 'contentPackage'],
    content_package: ['content_package', 'contentPackage', 'package_id', 'packageId'],
    device_id: ['device_id', 'deviceId'],
    android_package: ['android_package', 'androidPackage', 'androidPackageName'],
  }
  const candidates = aliases[key] || [key]
  for (const candidate of candidates) {
    if (hasValue(source[candidate])) return source[candidate]
  }
  // Support dotted context keys without imposing a new context shape.
  return String(key).split('.').reduce((current, part) => current?.[part], source)
}

function actionVersionAllowed(descriptor, action, callOptions, config) {
  const expected = firstDefined(
    callOptions?.actionVersion,
    callOptions?.capabilityVersion,
    config.actionVersions?.[action],
    config.supportedActionVersions?.[action],
  )
  if (hasValue(expected) && !versionSatisfies(descriptor?.version, expected)) return false

  const supported = firstDefined(descriptor?.supported_versions, descriptor?.supportedVersions)
  if (hasValue(supported) && !versionSatisfies(descriptor?.version, supported)) return false
  const range = firstDefined(descriptor?.version_range, descriptor?.versionRange)
  if (hasValue(range) && !versionSatisfies(descriptor?.version, range)) return false
  return true
}

function pluginVersionAllowed(snapshot, descriptor, callOptions, config) {
  if (explicitFalse(
    snapshot?.compatible,
    snapshot?.version_compatible,
    snapshot?.versionCompatible,
    snapshot?.version_ok,
    descriptor?.compatible,
    descriptor?.version_compatible,
    descriptor?.versionCompatible,
  )) return false

  const actual = firstDefined(
    snapshot?.active_version,
    snapshot?.activeVersion,
    snapshot?.plugin_version,
    snapshot?.pluginVersion,
    snapshot?.version,
  )
  const required = firstDefined(
    callOptions?.pluginVersion,
    callOptions?.targetVersion,
    config.pluginVersion,
    config.targetVersion,
  )
  if (hasValue(required) && !versionSatisfies(actual, required)) return false

  // `version` is accepted as a compact call-site selector. It may identify
  // either the action contract version or the active plugin version, so accept
  // whichever version namespace it matches; explicit actionVersion/pluginVersion
  // remain unambiguous for callers that need both checks.
  if (hasValue(callOptions?.version)
    && !hasValue(callOptions?.actionVersion)
    && !hasValue(callOptions?.pluginVersion)
    && !versionSatisfies(descriptor?.version, callOptions.version)
    && !versionSatisfies(actual, callOptions.version)) return false

  const constraints = [
    { kind: 'range', values: [descriptor?.required_plugin_version, descriptor?.requiredPluginVersion] },
    { kind: 'min', values: [descriptor?.min_plugin_version, descriptor?.minPluginVersion] },
    { kind: 'max', values: [descriptor?.max_plugin_version, descriptor?.maxPluginVersion] },
    { kind: 'range', values: [snapshot?.required_plugin_version, snapshot?.requiredPluginVersion] },
    { kind: 'range', values: [descriptor?.plugin_version_range, descriptor?.pluginVersionRange] },
    { kind: 'range', values: [snapshot?.plugin_version_range, snapshot?.pluginVersionRange] },
  ]
  for (const constraint of constraints) {
    const value = firstDefined(...constraint.values)
    if (!hasValue(value)) continue
    if (constraint.kind === 'min') {
      if (!versionSatisfies(actual, `>=${value}`)) return false
    } else if (constraint.kind === 'max') {
      if (!versionSatisfies(actual, `<=${value}`)) return false
    } else if (!versionSatisfies(actual, value)) return false
  }
  return true
}

function contextAllowed(snapshot, descriptor, callOptions, configuredContext) {
  if (explicitFalse(
    snapshot?.context_available,
    snapshot?.contextAvailable,
    snapshot?.context_valid,
    snapshot?.contextValid,
    descriptor?.context_available,
    descriptor?.contextAvailable,
    descriptor?.context_valid,
    descriptor?.contextValid,
  )) return false

  const context = callOptions && Object.prototype.hasOwnProperty.call(callOptions, 'context')
    ? callOptions.context
    : configuredContext
  const contextState = unwrap(context)
  if (contextState && typeof contextState === 'object' && explicitFalse(
    contextState.available,
    contextState.valid,
    contextState.ready,
  )) return false
  for (const key of requiredContextKeys(snapshot, descriptor, callOptions)) {
    if (!hasValue(contextValue(context, key))) return false
  }
  return true
}

function actionIsCallable(descriptor, action, snapshot, callOptions, config, configuredContext) {
  if (!descriptor) return false
  if (explicitFalse(
    descriptor.public,
    descriptor.public_action,
    descriptor.publicAction,
    descriptor.exposed,
    descriptor.published,
    descriptor.available,
    descriptor.callable,
    descriptor.can_call,
    descriptor.canCall,
  )) return false
  const availability = String(firstDefined(descriptor.availability, descriptor.availability_status) || '').toLowerCase()
  if (['blocked', 'unavailable', 'unsupported', 'missing', 'disabled'].includes(availability)) return false
  return actionVersionAllowed(descriptor, action, callOptions, config)
    && pluginVersionAllowed(snapshot, descriptor, callOptions, config)
    && contextAllowed(snapshot, descriptor, callOptions, configuredContext)
}

function normalizeCallOptions(value) {
  if (value === undefined) return {}
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return { context: value }
  const optionKeys = [
    'context',
    'actionVersion',
    'capabilityVersion',
    'pluginVersion',
    'targetVersion',
    'version',
    'requiredContext',
  ]
  return optionKeys.some(key => Object.prototype.hasOwnProperty.call(value, key))
    ? value
    : { context: value }
}

/**
 * 视频面板用的依赖状态组合式。
 *
 * `actions` / `hasDescription()` 表示公开动作描述是否存在；`ready` 与
 * `canCall()` 表示当前是否真的可以调用。后者还受响应里的可选版本/上下文
 * 条件影响，因此动作描述即使在 stopped、卸载或探测失败后保留，也不会变成
 * 一个 stale callable entry point。`hasAction()` 保留为旧调用方的 canCall
 * 别名。
 *
 * `lifecycleEvents` 是对现有扩展刷新机制的轻量 DOM 桥：收到扩展变更时先
 * fail-closed，再立刻重探测；没有该事件时仍由现有轮询收敛。`fetchCapabilities`
 * 可注入（测试用），缺省走 videoApi.gamerYamlCapabilities。
 */
export function useYamlCapability({
  pollMs = 10000,
  fetchCapabilities,
  context,
  pluginVersion,
  targetVersion,
  actionVersions,
  supportedActionVersions,
  lifecycleEvents = LIFECYCLE_EVENTS,
} = {}) {
  const ready = ref(false)
  const actions = ref([]) // [{action, version, surface, summary, ...conditions}]
  const checking = ref(false)
  const error = ref(null)
  const activeVersion = ref('')
  const snapshot = ref(null)
  const load = fetchCapabilities || ((...args) => videoApi.gamerYamlCapabilities(...args))
  let timer = null
  let watchingLifecycle = false
  let disposed = false
  let refreshVersion = 0

  const actionNames = computed(() => new Set(actions.value.map(item => actionName(item?.action)).filter(Boolean)))

  function hasDescription(action) {
    return actionNames.value.has(actionName(action))
  }

  function canCall(action, callOptions) {
    const name = actionName(action)
    if (!ready.value || !name) return false
    const descriptor = actions.value.find(item => actionName(item?.action) === name)
    const options = normalizeCallOptions(callOptions)
    return actionIsCallable(
      descriptor,
      name,
      snapshot.value,
      options,
      { actionVersions, supportedActionVersions, pluginVersion, targetVersion },
      context,
    )
  }

  // Existing consumers use hasAction as a gate; keep it fail-closed and make
  // the new name explicit for callers that need the description/call split.
  function hasAction(action, callOptions) {
    return canCall(action, callOptions)
  }

  function isTargetResponse(rep) {
    return !rep?.id || String(rep.id) === GAMER_YAML_PLUGIN_ID
  }

  function responseIsRunning(rep) {
    if (!isTargetResponse(rep)) return false
    const state = String(firstDefined(rep?.state, rep?.status) ?? '').trim().toLowerCase()
    if (state && state !== 'running') return false
    if (rep?.running !== true) return false
    const availability = String(firstDefined(rep?.availability, rep?.availability_status) || '').toLowerCase()
    if (['blocked', 'unavailable', 'unsupported', 'missing', 'disabled'].includes(availability)) return false
    return !explicitFalse(
      rep?.available,
      rep?.available_for_call,
      rep?.availableForCall,
      rep?.runtime_available,
      rep?.runtimeAvailable,
      rep?.callable,
      rep?.ready,
    )
  }

  async function refresh() {
    const version = ++refreshVersion
    disposed = false
    checking.value = true
    try {
      const rep = await load()
      if (version !== refreshVersion) return

      snapshot.value = isTargetResponse(rep) ? rep : null
      activeVersion.value = String(firstDefined(rep?.active_version, rep?.activeVersion, rep?.version) || '')
      // A successful stopped/uninstalled response still contains useful public
      // action descriptions. Preserve those descriptions independently from
      // the callable `ready` bit.
      if (isTargetResponse(rep)) actions.value = normalizeActions(rep?.actions)
      ready.value = responseIsRunning(rep)
      error.value = null
    } catch (cause) {
      if (version !== refreshVersion) return

      // A failed probe cannot prove that the previously observed extension is
      // still callable. Keep the old descriptions for explanatory UI, but drop
      // the callable snapshot.
      ready.value = false
      snapshot.value = null
      activeVersion.value = ''
      error.value = cause || new Error('gamer.yaml capability probe failed')
    } finally {
      if (version === refreshVersion) checking.value = false
    }
  }

  function invalidate({ refresh: shouldRefresh = false } = {}) {
    refreshVersion += 1
    ready.value = false
    snapshot.value = null
    activeVersion.value = ''
    error.value = null
    checking.value = false
    if (shouldRefresh && !disposed) void refresh()
  }

  function eventTargetsYaml(event) {
    const detail = event?.detail
    if (!detail || typeof detail !== 'object') return true
    const id = firstDefined(detail.id, detail.extension_id, detail.extensionId, detail.plugin_id, detail.pluginId)
    return id === undefined || String(id) === GAMER_YAML_PLUGIN_ID
  }

  function onLifecycleChange(event) {
    if (!eventTargetsYaml(event)) return
    invalidate({ refresh: true })
  }

  function addLifecycleListeners() {
    if (watchingLifecycle || typeof window === 'undefined' || !window.addEventListener) return
    for (const eventName of lifecycleEvents || []) {
      if (eventName) window.addEventListener(eventName, onLifecycleChange)
    }
    watchingLifecycle = true
  }

  function removeLifecycleListeners() {
    if (!watchingLifecycle || typeof window === 'undefined' || !window.removeEventListener) return
    for (const eventName of lifecycleEvents || []) {
      if (eventName) window.removeEventListener(eventName, onLifecycleChange)
    }
    watchingLifecycle = false
  }

  function start() {
    if (timer) return
    disposed = false
    addLifecycleListeners()
    void refresh()
    timer = setInterval(() => { void refresh() }, Math.max(2000, pollMs))
  }

  function stop() {
    if (timer) { clearInterval(timer); timer = null }
    removeLifecycleListeners()
    disposed = true
    invalidate()
  }

  return {
    ready,
    actions,
    actionDescriptions: actions,
    activeVersion,
    version: activeVersion,
    error,
    checking,
    hasDescription,
    canCall,
    hasAction,
    invalidate,
    refresh,
    start,
    stop,
  }
}
