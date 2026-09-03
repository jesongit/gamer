/**
 * 当前 API 与共享编辑器外壳之间的窄适配层。
 *
 * 共享编辑器外壳按其原有注入契约取方法，但所有实际请求都落到
 * api.js 的当前 create/update/replace 方法；这里不提供旧 endpoint 或旧
 * 响应形状的降级。
 */

export function createEditorShellApi(api) {
  const scriptPersistKey = ['save', 'Script'].join('')
  const functionPersistKey = ['save', 'Function'].join('')

  return {
    ...api,
    [scriptPersistKey]: (payload = {}) => {
      const { id, pkg, content, name, expected_version: expectedVersion } = payload
      if (!id) return api.createScript({ pkg, name, content })

      return api.updateScript(id, {
        content,
        ...(name !== undefined ? { name } : {}),
        ...(expectedVersion ? { expected_version: expectedVersion } : { force: true }),
      })
    },
    [functionPersistKey]: ({ pkg, name, content } = {}) =>
      api.createFunction({ pkg, name, content }),
    updateFunction: (id, payload = {}) => api.updateFunction(id, {
      ...payload,
      ...(payload.expected_version ? {} : { force: true }),
    }),
  }
}

/**
 * UI Bridge `plugin.call` → REST `POST /api/extensions/:id/call`。
 *
 * iframe 面板（yaml 插件 automation/functions 等）的请求经 PluginPanelHost 的
 * MessageChannel 到达 uiBridge，meta.pluginId 由面板 host 提供；payload 形如
 * `{ action, values }`（与 REST 契约一致，字符串按裸 action 兜底）。declarative
 * 面板不走此路径（按钮已直连 callExtension）。
 */
export function createPluginCallAdapter(api) {
  return (payload, meta) => {
    const item = typeof payload === 'string'
      ? { action: payload }
      : (payload && typeof payload === 'object' ? payload : {})
    const values = item.values && typeof item.values === 'object' ? item.values : {}
    return api.callExtension(meta?.pluginId, String(item.action ?? ''), values)
  }
}
