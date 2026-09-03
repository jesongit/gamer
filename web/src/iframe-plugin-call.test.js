// iframe 面板 plugin.call → UI Bridge → host pluginCall → REST 的转发链路。
// declarative 面板直连 api.callExtension（见 declarative-panel-host.test.js），
// 这里覆盖 iframe 路径：Console.vue 经 createPluginCallAdapter(api) 注入
// workspace context 的 pluginCall 后，guest 的 bridge 请求才能真正到服务端。
import { describe, expect, it, vi } from 'vitest'
import { createPluginCallAdapter } from './components/console/current-api-adapters'
import { createWorkspaceContext } from './workspace/context'
import { BRIDGE_RESPONSE_TYPE, replyToBridgeRequest } from './workspace/bridge'

function fakeApi() {
  return { callExtension: vi.fn().mockResolvedValue({ ok: true, result: 'done' }) }
}

function fakePort() {
  return { messages: [], postMessage(message) { this.messages.push(message) } }
}

describe('iframe 面板 plugin.call 经 UI Bridge 转发到 REST', () => {
  it('createPluginCallAdapter 把 {action, values} 转发为 api.callExtension(pluginId, action, values)', async () => {
    const api = fakeApi()
    const pluginCall = createPluginCallAdapter(api)
    const result = await pluginCall(
      { action: 'list_scripts', values: { pkg: 'com.game' } },
      { pluginId: 'gamer.yaml', panelId: 'automation' },
    )
    expect(api.callExtension).toHaveBeenCalledTimes(1)
    expect(api.callExtension).toHaveBeenCalledWith('gamer.yaml', 'list_scripts', { pkg: 'com.game' })
    expect(result).toEqual({ ok: true, result: 'done' })
  })

  it('payload 形态兜底：字符串按裸 action，缺 values 补空对象', async () => {
    const api = fakeApi()
    const pluginCall = createPluginCallAdapter(api)
    await pluginCall('refresh', { pluginId: 'gamer.yaml', panelId: 'functions' })
    await pluginCall({ action: 'refresh' }, { pluginId: 'gamer.yaml', panelId: 'functions' })
    expect(api.callExtension).toHaveBeenNthCalledWith(1, 'gamer.yaml', 'refresh', {})
    expect(api.callExtension).toHaveBeenNthCalledWith(2, 'gamer.yaml', 'refresh', {})
  })

  it('注入 pluginCall 后，workspace context 的 uiBridge 按 iframe 请求封套转发 plugin.call', async () => {
    const api = fakeApi()
    const ctx = createWorkspaceContext({ pluginCall: createPluginCallAdapter(api) })
    const port = fakePort()
    await replyToBridgeRequest(
      { id: 'req-1', method: 'plugin.call', params: { action: 'reload', values: { hard: true } } },
      ctx.uiBridge,
      port,
      { pluginId: 'gamer.yaml', panelId: 'automation' },
    )
    expect(api.callExtension).toHaveBeenCalledWith('gamer.yaml', 'reload', { hard: true })
    expect(port.messages[0]).toMatchObject({
      type: BRIDGE_RESPONSE_TYPE, id: 'req-1', ok: true,
      result: { ok: true, result: 'done' },
    })
  })

  it('REST 失败经 bridge 以 ok:false 回传 iframe（保留服务端 message）', async () => {
    const api = {
      callExtension: vi.fn().mockRejectedValue(Object.assign(new Error('插件未运行'), { status: 409 })),
    }
    const ctx = createWorkspaceContext({ pluginCall: createPluginCallAdapter(api) })
    const port = fakePort()
    await replyToBridgeRequest(
      { id: 'req-2', method: 'plugin.call', params: { action: 'refresh' } },
      ctx.uiBridge,
      port,
      { pluginId: 'gamer.yaml', panelId: 'automation' },
    )
    expect(port.messages[0]).toMatchObject({ type: BRIDGE_RESPONSE_TYPE, id: 'req-2', ok: false })
    expect(port.messages[0].error).toEqual({ code: 'bridge_error', message: '插件未运行' })
  })

  it('未注入 pluginCall 时 plugin.call 报 plugin_call_unavailable（原始缺口的行为基线）', async () => {
    const ctx = createWorkspaceContext({})
    await expect(ctx.uiBridge.dispatch('plugin.call', { action: 'x' }, { pluginId: 'gamer.yaml' }))
      .rejects.toMatchObject({ code: 'plugin_call_unavailable' })
  })
})
