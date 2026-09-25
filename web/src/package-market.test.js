// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
const mocks = vi.hoisted(() => ({ api: {
  packageSources: vi.fn(), packageSourceCatalog: vi.fn(), savePackageSource: vi.fn(), removePackageSource: vi.fn(),
  downloadSourcePackage: vi.fn(), importPackageArchive: vi.fn(),
}, confirm: vi.fn(), refresh: vi.fn() }))
vi.mock('./api', () => ({ api: mocks.api }))
vi.mock('./package-store', () => ({ loadPackages: vi.fn(), refreshPackages: mocks.refresh, packageStore: { packages: [] } }))
vi.mock('./store', () => ({ useToast: () => vi.fn() }))
vi.mock('./components/ui/useConfirmDialog', () => ({ useConfirmDialog: () => mocks.confirm }))
vi.mock('./workspace/operation-feedback', () => ({ OPERATION_FEEDBACK_KEY: Symbol(), operationReporter: () => () => vi.fn() }))
import Market from './workspace/MarketView.vue'
const source = { id: 'source-a', repository: 'owner/a', enabled: true }
const entry = { id: 'demo', name: '示例', version: '1.0.0', sha256: 'a'.repeat(64), required_plugins: [], android_targets: ['*'] }
beforeEach(() => {
  vi.resetAllMocks()
  mocks.api.packageSources.mockResolvedValue([source])
  mocks.api.packageSourceCatalog.mockResolvedValue({ packages: [entry] })
  mocks.api.downloadSourcePackage.mockResolvedValue(new Uint8Array([1, 2]))
  mocks.api.importPackageArchive.mockResolvedValue({ id: entry.id })
})
describe('公开配置仓库', () => {
  it('刷新强制检查源；安装只用同源下载，并传递强制哈希', async () => {
    const w = mount(Market); await flushPromises()
    expect(mocks.api.packageSourceCatalog).toHaveBeenCalledWith(source.id, false)
    await w.findAll('button').find(b => b.text() === '刷新').trigger('click'); await flushPromises()
    expect(mocks.api.packageSourceCatalog).toHaveBeenLastCalledWith(source.id, true)
    await w.get('.market-remote-actions button').trigger('click'); await flushPromises()
    expect(mocks.api.downloadSourcePackage).toHaveBeenCalledWith(source.id, expect.objectContaining(entry))
    expect(mocks.api.importPackageArchive).toHaveBeenCalledWith(new Uint8Array([1, 2]), { expectedSha256: entry.sha256 })
    expect(mocks.refresh).toHaveBeenCalled(); w.unmount()
  })
  it('同 ID 跨仓库分别展示，单源失败不隐藏其他仓库', async () => {
    mocks.api.packageSources.mockResolvedValue([source, { ...source, id: 'b', repository: 'owner/b' }, { ...source, id: 'c', repository: 'owner/c' }])
    mocks.api.packageSourceCatalog.mockImplementation(id => id === 'c' ? Promise.reject(new Error('源不可用')) : Promise.resolve({ packages: [entry] }))
    const w = mount(Market); await flushPromises()
    expect(w.findAll('.market-remote-card')).toHaveLength(2)
    expect(w.text()).toContain('owner/a'); expect(w.text()).toContain('owner/b'); expect(w.text()).toContain('源不可用'); w.unmount()
  })
  it('用户取消同 ID 覆盖时不再次导入，提示来源和本地修改', async () => {
    mocks.api.importPackageArchive.mockRejectedValue({ status: 409, data: { existing: { version: '0.9.0' } } })
    mocks.confirm.mockResolvedValue(false)
    const w = mount(Market); await flushPromises(); await w.get('.market-remote-actions button').trigger('click'); await flushPromises()
    expect(mocks.confirm.mock.calls[0][0]).toContain('owner/a'); expect(mocks.confirm.mock.calls[0][0]).toContain('本地修改')
    expect(mocks.api.importPackageArchive).toHaveBeenCalledTimes(1); expect(mocks.refresh).not.toHaveBeenCalled(); w.unmount()
  })
  it('归档校验失败不导入', async () => {
    mocks.api.downloadSourcePackage.mockRejectedValue(new Error('SHA256 不符'))
    const w = mount(Market); await flushPromises(); await w.get('.market-remote-actions button').trigger('click'); await flushPromises()
    expect(mocks.api.importPackageArchive).not.toHaveBeenCalled(); w.unmount()
  })
  it('添加仓库与移除只修改源，不删除配置包', async () => {
    const w = mount(Market); await flushPromises()
    await w.get('input[aria-label]').setValue('git@github.com:owner/new.git'); await w.get('form').trigger('submit'); await flushPromises()
    expect(mocks.api.savePackageSource).toHaveBeenCalledWith('git@github.com:owner/new.git')
    await w.findAll('button').find(b => b.text() === '移除').trigger('click'); await flushPromises()
    expect(mocks.api.removePackageSource).toHaveBeenCalledWith(source.id)
    expect(mocks.api.importPackageArchive).not.toHaveBeenCalled(); w.unmount()
  })
})
