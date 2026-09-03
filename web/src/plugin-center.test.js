import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from './api'
import {
  downloadFixedVersion,
  isProductionRemoteUi,
  normalizeRegistry,
} from './workspace/plugin-center/registry-client'
import {
  dependencyStatus,
  installPolicy,
  lifecyclePrompt,
  permissionDiff,
  uninstallPrompt,
  registryProofFor,
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

describe('Phase 10 plugin center contracts', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('crypto', { subtle: { digest: async () => new Uint8Array(32).buffer } })
  })

  afterEach(() => vi.unstubAllGlobals())

  it('normalizes a registry and rejects duplicate or floating versions', () => {
    const registry = normalizeRegistry({
      schema_version: 1,
      plugins: [{
        id: 'official.vision', version: '1.2.0', name: 'Vision',
        download_url: 'https://example.test/official.vision-1.2.0.gplugin',
        signature: { status: 'valid', key_id: 'official-1' },
      }],
    })
    expect(registry.plugins[0]).toMatchObject({ id: 'official.vision', version: '1.2.0', source: 'official' })
    expect(() => normalizeRegistry({ schema_version: 1, plugins: [
      { id: 'a', version: '1.0.0', name: 'A', download_url: 'https://x/a' },
      { id: 'a', version: '1.0.0', name: 'A', download_url: 'https://x/a2' },
    ] })).toThrow(/重复版本/)
    expect(() => normalizeRegistry({ schema_version: 1, plugins: [
      { id: 'a', version: 'latest', name: 'A', download_url: 'https://x/a' },
    ] })).toThrow(/SemVer/)
  })

  it('downloads the registry fixed URL as an archive and never treats it as UI', async () => {
    const bytes = new Uint8Array([80, 75, 3, 4])
    fetch.mockResolvedValueOnce(archiveResponse(bytes))
    const result = await downloadFixedVersion({
      id: 'official.vision', version: '1.0.0', name: 'Vision',
      download_url: 'https://example.test/vision/1.0.0.gplugin',
      signature: { status: 'valid' },
    })
    expect(fetch).toHaveBeenCalledWith('https://example.test/vision/1.0.0.gplugin', expect.any(Object))
    expect(result.bytes).toEqual(bytes)
    expect(result.file.type).toBe('application/zip')
    expect(isProductionRemoteUi('https://example.test/ui/index.html')).toBe(true)
  })

  it('requires official signatures but allows local imports only with a warning', () => {
    expect(installPolicy({ kind: 'official', signature: { status: 'unsigned' } }).allowed).toBe(false)
    expect(installPolicy({ kind: 'official', signature: { status: 'valid' } }).requiresWarning).toBe(false)
    expect(installPolicy({ kind: 'local', signature: { status: 'unsigned' } })).toMatchObject({ allowed: true, requiresWarning: true })
    expect(lifecyclePrompt('disable', { id: 'official.vision', version: '1.0.0', state: 'enabled' })).toMatch(/停用 official\.vision@1\.0\.0/)
  })

  it('sends the official Registry proof and permission confirmation to the server', async () => {
    fetch.mockResolvedValueOnce(jsonResponse(200, { id: 'official.vision', version: '1.0.0' }))
    const entry = {
      id: 'official.vision', version: '1.0.0', name: 'Vision',
      download_url: 'https://registry.example/vision.gplugin', sha256: 'a'.repeat(64),
      signature: { status: 'valid', key_id: 'registry-1', value: 'signature-value' },
    }
    const proof = registryProofFor(entry)
    expect(proof).toMatchObject({ id: entry.id, version: entry.version, key_id: 'registry-1' })
    await api.inspectExtension(new Blob([new Uint8Array([1])]), {
      source: 'official', registryProof: proof, permissionConfirmed: true,
    })
    const headers = fetch.mock.calls[0][1].headers
    expect(headers['X-Gamer-Extension-Source']).toBe('official')
    expect(headers['X-Gamer-Permission-Confirm']).toBe('1')
    expect(headers['X-Gamer-Registry-Proof']).toBeTruthy()
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
      .mockResolvedValueOnce(jsonResponse(200, { id: 'official.vision', version: '1.0.0' }))
      .mockResolvedValueOnce(jsonResponse(200, { ok: true }))
      .mockResolvedValueOnce(jsonResponse(200, { extensions: [] }))
      .mockResolvedValueOnce({ ok: true, status: 204, headers: { get: () => null } })
    const archive = new Blob([new Uint8Array([1, 2, 3])], { type: 'application/zip' })
    await api.inspectExtension(archive)
    await api.installExtension(archive)
    await api.getExtensionManagement()
    await api.uninstallExtension('official.vision', '1.0.0', { deleteData: true })
    expect(fetch.mock.calls[0][0]).toBe('/api/extensions/inspect')
    expect(fetch.mock.calls[0][1].headers['Content-Type']).toBe('application/zip')
    expect(fetch.mock.calls[1][0]).toBe('/api/extensions')
    expect(fetch.mock.calls[2][0]).toBe('/api/extensions/management')
    expect(fetch.mock.calls[3][0]).toBe('/api/extensions/official.vision/1.0.0?delete_data=1')
    expect(uninstallPrompt({ id: 'official.vision', version: '1.0.0', state: 'enabled' }, true)).toMatch(/删除该插件的用户数据/)
  })

  it('activate posts the target version to the activate endpoint（回滚走同一契约）', async () => {
    fetch.mockResolvedValueOnce(jsonResponse(200, { id: 'official.vision', active_version: '2.9.0', state: 'enabled' }))
    const result = await api.activateExtension('official.vision', '2.9.0')
    expect(fetch.mock.calls[0][0]).toBe('/api/extensions/official.vision/activate')
    expect(fetch.mock.calls[0][1]).toMatchObject({ method: 'POST', body: JSON.stringify({ version: '2.9.0' }) })
    expect(result).toMatchObject({ active_version: '2.9.0', state: 'enabled' })
    // requireId 在进入 fetch 前同步拒绝空版本
    expect(() => api.activateExtension('official.vision', '')).toThrow('extension_version 不能为空')
  })
})
