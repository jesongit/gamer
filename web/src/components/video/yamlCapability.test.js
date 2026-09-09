// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { isGamerYamlRunning, useYamlCapability } from './yamlCapability'

const ACTION = 'template.create_from_frame'
const description = (extra = {}) => ({ action: ACTION, version: 1, ...extra })
const running = (extra = {}) => ({
  id: 'gamer.yaml',
  state: 'running',
  running: true,
  active_version: '3.1.1',
  actions: [description()],
  ...extra,
})

afterEach(() => {
  vi.restoreAllMocks()
})

describe('useYamlCapability', () => {
  it('Running → Stop → Start：动作描述保留，但 canCall 随运行态关闭/恢复', async () => {
    const fetchCapabilities = vi.fn()
      .mockResolvedValueOnce(running())
      .mockResolvedValueOnce({ ...running(), state: 'enabled', running: false })
      .mockResolvedValueOnce(running())
    const capability = useYamlCapability({ fetchCapabilities })

    await capability.refresh()
    expect(capability.ready.value).toBe(true)
    expect(capability.hasDescription(ACTION)).toBe(true)
    expect(capability.canCall(ACTION)).toBe(true)
    expect(capability.hasAction(ACTION)).toBe(true)

    await capability.refresh()
    expect(capability.ready.value).toBe(false)
    expect(capability.hasDescription(ACTION)).toBe(true)
    expect(capability.canCall(ACTION)).toBe(false)
    expect(capability.hasAction(ACTION)).toBe(false)

    await capability.refresh()
    expect(capability.ready.value).toBe(true)
    expect(capability.canCall(ACTION)).toBe(true)
  })

  it('卸载/显式失效后立即关闭 YAML 动作，且下一次探测可恢复', async () => {
    const fetchCapabilities = vi.fn()
      .mockResolvedValueOnce(running())
      .mockResolvedValueOnce({ id: 'gamer.yaml', state: null, running: false, actions: [] })
      .mockResolvedValueOnce(running())
    const capability = useYamlCapability({ fetchCapabilities })

    await capability.refresh()
    capability.invalidate()
    expect(capability.ready.value).toBe(false)
    expect(capability.hasDescription(ACTION)).toBe(true)
    expect(capability.canCall(ACTION)).toBe(false)

    await capability.refresh()
    expect(capability.ready.value).toBe(false)
    expect(capability.hasDescription(ACTION)).toBe(false)
    expect(capability.canCall(ACTION)).toBe(false)

    await capability.refresh()
    expect(capability.ready.value).toBe(true)
    expect(capability.canCall(ACTION)).toBe(true)
  })

  it('探测失败时 ready=false，旧动作描述可保留但不能继续 canCall', async () => {
    const fetchCapabilities = vi.fn()
      .mockResolvedValueOnce(running())
      .mockRejectedValueOnce(new Error('network'))
    const capability = useYamlCapability({ fetchCapabilities })

    await capability.refresh()
    await capability.refresh()

    expect(capability.ready.value).toBe(false)
    expect(capability.hasDescription(ACTION)).toBe(true)
    expect(capability.canCall(ACTION)).toBe(false)
    expect(capability.hasAction(ACTION)).toBe(false)
    expect(capability.error.value).toBeInstanceOf(Error)
  })

  it('乱序响应：新状态优先，旧 Running 响应不能覆盖 Stop', async () => {
    let resolveOld
    let resolveNew
    const oldProbe = new Promise(resolve => { resolveOld = resolve })
    const newProbe = new Promise(resolve => { resolveNew = resolve })
    const fetchCapabilities = vi.fn()
      .mockReturnValueOnce(oldProbe)
      .mockReturnValueOnce(newProbe)
    const capability = useYamlCapability({ fetchCapabilities })

    const oldRefresh = capability.refresh()
    const newRefresh = capability.refresh()
    resolveNew({
      id: 'gamer.yaml',
      state: 'disabled',
      running: false,
      actions: [description()],
    })
    await newRefresh
    expect(capability.ready.value).toBe(false)
    expect(capability.hasDescription(ACTION)).toBe(true)
    expect(capability.canCall(ACTION)).toBe(false)

    resolveOld(running())
    await oldRefresh
    expect(capability.ready.value).toBe(false)
    expect(capability.canCall(ACTION)).toBe(false)
    expect(capability.checking.value).toBe(false)
  })

  it('生命周期通知先失效再刷新，不依赖整页刷新', async () => {
    const fetchCapabilities = vi.fn()
      .mockResolvedValueOnce(running())
      .mockResolvedValueOnce({ ...running(), state: 'disabled', running: false })
    const capability = useYamlCapability({ fetchCapabilities, pollMs: 60000 })

    capability.start()
    await vi.waitFor(() => expect(capability.ready.value).toBe(true))
    window.dispatchEvent(new CustomEvent('gamer:extension-lifecycle', {
      detail: { id: 'gamer.yaml', operation: 'disable' },
    }))
    expect(capability.ready.value).toBe(false)
    await vi.waitFor(() => expect(fetchCapabilities).toHaveBeenCalledTimes(2))
    await vi.waitFor(() => expect(capability.canCall(ACTION)).toBe(false))
    capability.stop()
  })

  it('版本/上下文不满足时只保留描述，不允许调用', async () => {
    const fetchCapabilities = vi.fn().mockResolvedValue({
      ...running(),
      actions: [description({ required_context: ['package_id'], min_plugin_version: '3.0.0' })],
    })
    const capability = useYamlCapability({
      fetchCapabilities,
      context: { packageId: 'pkg-1' },
      targetVersion: '>=3.0.0 <4.0.0',
      actionVersions: { [ACTION]: 1 },
    })

    await capability.refresh()
    expect(capability.hasDescription(ACTION)).toBe(true)
    expect(capability.canCall(ACTION)).toBe(true)
    expect(capability.canCall(ACTION, { package_id: 'pkg-2' })).toBe(true)
    expect(capability.canCall(ACTION, { context: {} })).toBe(false)
    expect(capability.canCall(ACTION, { actionVersion: 2 })).toBe(false)
    expect(capability.canCall(ACTION, { version: '2.0.0' })).toBe(false)
  })
})

describe('无 gamer.yaml 时的视频基础能力', () => {
  it('不把 gamer.video 的存在误认为 gamer.yaml 可用，基础状态判定仍可独立工作', async () => {
    expect(isGamerYamlRunning([{ id: 'gamer.video', state: 'running' }])).toBe(false)

    const capability = useYamlCapability({
      fetchCapabilities: vi.fn().mockResolvedValue({
        id: 'gamer.yaml', state: null, running: false, actions: [],
      }),
    })
    await capability.refresh()
    expect(capability.ready.value).toBe(false)
    expect(capability.canCall(ACTION)).toBe(false)
    // The composable only gates YAML actions; it has no dependency on a media,
    // recording, project, frame, marker, or calibration API.
    expect(isGamerYamlRunning([{ id: 'gamer.video', state: 'running' }])).toBe(false)
  })
})
