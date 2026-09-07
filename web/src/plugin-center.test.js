import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from './api'
import {
  downloadFixedVersion,
  isProductionRemoteUi,
  normalizeRegistry,
  REGISTRY_SCHEMA_VERSION,
} from './workspace/plugin-center/registry-client'
import {
  dependencyStatus,
  executionChangeDetail,
  executionLabel,
  hostVersionLabel,
  installErrorText,
  installPolicy,
  lifecyclePrompt,
  normalizeExecution,
  permissionDiff,
  uninstallPrompt,
} from './workspace/plugin-center/plugin-service'

function jsonResponse(status, body) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: name => (/content-type/i.test(name) ? 'application/json' : null) },
    json: async () => body,
  }
}

function archiveResponse(bytes) {
  return {
    ok: true,
    status: 200,
    headers: { get: () => null },
    arrayBuffer: async () => bytes.buffer,
  }
}

// Phase 1 契约（计划 §4）：registry schema_version=2，条目带 execution{kind}、无 signature；
// 读端容忍 v1（无 execution 视为 wasm、有 signature 忽略）；官方市场安装不再做签名/proof 门禁，
// 完整性仍由固定版本 SHA-256 强制。
describe('Phase 1 plugin center contracts（免签名市场）', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('crypto', { subtle: { digest: async () => new Uint8Array(32).buffer } })
  })

  afterEach(() => vi.unstubAllGlobals())

  it('normalizes a schema_version=2 registry with execution kinds (three official plugins)', () => {
    expect(REGISTRY_SCHEMA_VERSION).toBe(2)
    const registry = normalizeRegistry({
      schema_version: 2,
      plugins: [
        {
          id: 'gamer.yaml', version: '3.1.1', name: '自动化',
          download_url: '/plugins/gamer.yaml-3.1.1.gplugin',
          sha256: 'a'.repeat(64), size: 100,
          execution: { kind: 'wasm' },
          permissions: ['resource.read'],
        },
        {
          id: 'gamer.keymap', version: '1.0.1', name: '按键映射',
          download_url: '/plugins/gamer.keymap-1.0.1.gplugin',
          sha256: 'b'.repeat(64),
          execution: { kind: 'wasm' },
        },
        {
          id: 'gamer.video', version: '1.0.0', name: '视频工作台',
          download_url: '/plugins/gamer.video-1.0.0.gplugin',
          sha256: 'c'.repeat(64),
          execution: { kind: 'builtin', host_version: '>=1.3.0' },
          permissions: ['media.read'],
        },
      ],
    })
    expect(registry.plugins).toHaveLength(3)
    const [yaml, keymap, video] = registry.plugins
    expect(yaml).toMatchObject({ id: 'gamer.yaml', version: '3.1.1', source: 'official', execution: { kind: 'wasm' } })
    expect(keymap.execution).toEqual({ kind: 'wasm' })
    expect(video.execution).toEqual({ kind: 'builtin', host_version: '>=1.3.0' })
    expect(executionLabel(video.execution)).toContain('宿主预置')
    expect(executionLabel(yaml.execution)).toBe('WASM 插件')
    expect(hostVersionLabel(video.execution)).toBe('>=1.3.0')
    expect(hostVersionLabel(yaml.execution)).toBe('')
    // 重复版本 / 浮动版本仍然拒绝
    expect(() => normalizeRegistry({ schema_version: 2, plugins: [
      { id: 'a', version: '1.0.0', name: 'A', download_url: 'https://x/a' },
      { id: 'a', version: '1.0.0', name: 'A', download_url: 'https://x/a2' },
    ] })).toThrow(/重复版本/)
    expect(() => normalizeRegistry({ schema_version: 2, plugins: [
      { id: 'a', version: 'latest', name: 'A', download_url: 'https://x/a' },
    ] })).toThrow(/SemVer/)
    expect(() => normalizeRegistry({ schema_version: 3, plugins: [] })).toThrow(/不受支持/)
  })

  it('tolerates legacy v1 registries: missing execution means wasm and a signature field is ignored', () => {
    const registry = normalizeRegistry({
      schema_version: 1,
      plugins: [{
        id: 'legacy.plugin', version: '1.0.0', name: 'Legacy',
        download_url: '/plugins/legacy-1.0.0.gplugin',
        signature: { status: 'valid', key_id: 'gamer-dev-1', value: 'proof-blob' },
      }],
    })
    expect(registry.schema_version).toBe(1)
    expect(registry.plugins[0].execution).toEqual({ kind: 'wasm' })
    expect(registry.plugins[0]).not.toHaveProperty('signature')
  })

  it('downloads the registry fixed URL as an archive and verifies a matching sha256', async () => {
    const bytes = new Uint8Array([80, 75, 3, 4])
    fetch.mockResolvedValueOnce(archiveResponse(bytes))
    // digest stub 返回全零 → 与全零哈希匹配
    const result = await downloadFixedVersion({
      id: 'gamer.yaml', version: '3.1.1', name: '自动化',
      download_url: 'https://example.test/gamer.yaml-3.1.1.gplugin',
      sha256: '0'.repeat(64),
      execution: { kind: 'wasm' },
    })
    expect(fetch).toHaveBeenCalledWith('https://example.test/gamer.yaml-3.1.1.gplugin', expect.any(Object))
    expect(result.bytes).toEqual(bytes)
    expect(result.file.type).toBe('application/zip')
    expect(isProductionRemoteUi('https://example.test/ui/index.html')).toBe(true)
  })

  it('rejects downloads whose bytes do not match the registry sha256 (no silent install)', async () => {
    const bytes = new Uint8Array([80, 75, 3, 4])
    fetch.mockResolvedValueOnce(archiveResponse(bytes))
    const failure = downloadFixedVersion({
      id: 'gamer.yaml', version: '3.1.1', name: '自动化',
      download_url: 'https://example.test/gamer.yaml-3.1.1.gplugin',
      sha256: 'a'.repeat(64),
    })
    await expect(failure).rejects.toMatchObject({ code: 'hash_mismatch' })
    await expect(failure.catch(error => { throw new Error(error.message) })).rejects.toThrow(/不污染已装版本/)
  })

  it('official market entries without a fixed sha256 are refused before download', async () => {
    await expect(downloadFixedVersion({
      id: 'gamer.yaml', version: '3.1.1', name: '自动化',
      download_url: 'https://example.test/gamer.yaml-3.1.1.gplugin',
      source: 'official',
    })).rejects.toMatchObject({ code: 'missing_hash' })
  })

  it('allows every source without signature gating: official unsigned installs, local/url warn only', () => {
    // official 无签名信息 = 正常态（v2 registry/inspect 不再有签名字段）
    expect(installPolicy({ kind: 'official' })).toMatchObject({ allowed: true, requiresWarning: false })
    expect(installPolicy({ kind: 'local', label: '本地文件' })).toMatchObject({ allowed: true, requiresWarning: true })
    expect(installPolicy({ kind: 'url', label: 'https://x' }).warning).toContain('来源非官方')
    expect(lifecyclePrompt('disable', { id: 'gamer.yaml', version: '3.1.1', state: 'enabled' })).toMatch(/停用 gamer\.yaml@3\.1\.1/)
  })

  it('maps typed install/download errors to human-readable hints', () => {
    expect(installErrorText(Object.assign(new Error('x'), { code: 'hash_mismatch' }))).toContain('不污染已装版本')
    expect(installErrorText(Object.assign(new Error('x'), { code: 'host_feature_unavailable' }))).toContain('升级 Gamer')
    expect(installErrorText(Object.assign(new Error('插件 gamer.video@1.0.0 的安装包在发布源不存在（404）'), {
      code: 'download_not_found', name: 'RegistryError',
    }))).toContain('404')
    expect(installErrorText(Object.assign(new Error('x'), { code: 'download_network_error' }))).toContain('网络')
    expect(installErrorText(Object.assign(new Error('x'), { code: 'unsupported_registry' }))).toContain('更新 Gamer')
    // 未识别错误原样透传
    expect(installErrorText(new Error('manifest 无效'))).toBe('manifest 无效')
  })

  it('sends the official source header and permission confirmation without a registry proof header', async () => {
    fetch.mockResolvedValueOnce(jsonResponse(200, { id: 'gamer.yaml', version: '3.1.1' }))
    await api.inspectExtension(new Blob([new Uint8Array([1])]), {
      source: 'official', permissionConfirmed: true,
    })
    const headers = fetch.mock.calls[0][1].headers
    expect(headers['X-Gamer-Extension-Source']).toBe('official')
    expect(headers['X-Gamer-Permission-Confirm']).toBe('1')
    // proof 头已随签名门禁移除，不再出现
    expect(headers).not.toHaveProperty('X-Gamer-Registry-Proof')
  })

  it('normalizes execution conservatively: only explicit builtin is host-provided', () => {
    expect(normalizeExecution(undefined)).toEqual({ kind: 'wasm' })
    expect(normalizeExecution({ kind: 'builtin' })).toEqual({ kind: 'builtin' })
    expect(normalizeExecution({ kind: 'BUILTIN', host_version: '>=2.0' })).toEqual({ kind: 'builtin', host_version: '>=2.0' })
    expect(normalizeExecution({ kind: 'nonsense' })).toEqual({ kind: 'wasm' })
  })

  it('computes permission additions and dependency failures deterministically', () => {
    expect(permissionDiff(['device.read', 'vision.match'], ['vision.match', 'input.tap'])).toEqual({
      added: ['input.tap'], removed: ['device.read'], unchanged: ['vision.match'],
    })
    expect(dependencyStatus([
      { id: 'dep.ready' }, { id: 'dep.missing' }, { id: 'dep.disabled' },
    ], [
      { id: 'dep.ready', state: 'enabled' }, { id: 'dep.disabled', state: 'disabled' },
    ])).toMatchObject({ ok: false, missing: [{ id: 'dep.missing' }], disabled: [{ id: 'dep.disabled' }] })
    expect(dependencyStatus([{ id: 'dep.ready', version: '2' }], [
      { id: 'dep.ready', active_version: '1.4.0', state: 'enabled' },
    ])).toMatchObject({ ok: false, missing: [{ id: 'dep.ready', state: 'version 1.4.0' }] })
  })

  it('exposes inspect, management, and uninstall data policy through the API', async () => {
    fetch
      .mockResolvedValueOnce(jsonResponse(200, { id: 'gamer.yaml', version: '3.1.1' }))
      .mockResolvedValueOnce(jsonResponse(200, { ok: true }))
      .mockResolvedValueOnce(jsonResponse(200, { extensions: [] }))
      .mockResolvedValueOnce({ ok: true, status: 204, headers: { get: () => null } })
    const archive = new Blob([new Uint8Array([1, 2, 3])], { type: 'application/zip' })
    await api.inspectExtension(archive)
    await api.installExtension(archive)
    await api.getExtensionManagement()
    await api.uninstallExtension('gamer.yaml', '3.1.1', { deleteData: true })
    expect(fetch.mock.calls[0][0]).toBe('/api/extensions/inspect')
    expect(fetch.mock.calls[0][1].headers['Content-Type']).toBe('application/zip')
    expect(fetch.mock.calls[1][0]).toBe('/api/extensions')
    expect(fetch.mock.calls[2][0]).toBe('/api/extensions/management')
    expect(fetch.mock.calls[3][0]).toBe('/api/extensions/gamer.yaml/3.1.1?delete_data=1')
    expect(uninstallPrompt({ id: 'gamer.yaml', version: '3.1.1', state: 'enabled' }, true)).toMatch(/删除该插件的用户数据/)
  })

  it('activate posts the target version to the activate endpoint（回滚走同一契约）', async () => {
    fetch.mockResolvedValueOnce(jsonResponse(200, { id: 'gamer.yaml', active_version: '2.9.0', state: 'enabled' }))
    const result = await api.activateExtension('gamer.yaml', '2.9.0')
    expect(fetch.mock.calls[0][0]).toBe('/api/extensions/gamer.yaml/activate')
    expect(fetch.mock.calls[0][1]).toMatchObject({ method: 'POST', body: JSON.stringify({ version: '2.9.0' }) })
    expect(result).toMatchObject({ active_version: '2.9.0', state: 'enabled' })
    // requireId 在进入 fetch 前同步拒绝空版本
    expect(() => api.activateExtension('gamer.yaml', '')).toThrow('extension_version 不能为空')
  })
})

// Phase 8 遗留（D2）：inspect 的 execution_change:{from,to} 必须在确认弹窗明确提示。
describe('executionChangeDetail（wasm↔builtin 形态变化提示）', () => {
  it('wasm → builtin 迁移给出警示行', () => {
    expect(executionChangeDetail({ from: 'wasm', to: 'builtin' })).toContain('执行形态变化：wasm → builtin')
    expect(executionChangeDetail({ from: 'builtin', to: 'wasm' })).toContain('builtin → wasm')
  })

  it('无变化/缺失/空字段不加行', () => {
    expect(executionChangeDetail({ from: 'wasm', to: 'wasm' })).toBe('')
    expect(executionChangeDetail(null)).toBe('')
    expect(executionChangeDetail(undefined)).toBe('')
    expect(executionChangeDetail({})).toBe('')
  })
})
