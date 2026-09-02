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
