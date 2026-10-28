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
  localStorage.clear()
  mocks.api.packageSources.mockResolvedValue([source])
  mocks.api.packageSourceCatalog.mockResolvedValue({ packages: [entry] })
  mocks.api.downloadSourcePackage.mockResolvedValue(new Uint8Array([1, 2]))
  mocks.api.importPackageArchive.mockResolvedValue({ id: entry.id })
})
describe('公开配置仓库', () => {
  it('镜像与独立目录按原地址展示和保存，不拼接第二次 GitHub 前缀', async () => {
    for (const repository of ['https://gh-proxy.com/https://github.com/jesongit/gamer-packages', 'https://another-mirror.example/cache/https://github.com/owner/repo', 'https://mirror.example/configs/packages.json']) {
      mocks.api.packageSources.mockResolvedValue([{ ...source, repository }])
      const w = mount(Market); await flushPromises()
      expect(w.get('.source-address').attributes('href')).toBe(repository)
      mocks.api.savePackageSource.mockResolvedValue([{ ...source, repository, name: '我的镜像' }])
      await w.findAll('button').find(b => b.text() === '管理预设').trigger('click')
      await w.get('input[aria-label="预设名称"]').setValue('我的镜像')
      await w.get('input[aria-label="来源 URL"]').setValue(repository)
      await w.get('form').trigger('submit'); await flushPromises()
      expect(mocks.api.savePackageSource).toHaveBeenLastCalledWith(repository, true, { name: '我的镜像' })
      w.unmount()
    }
  })
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
  it('下拉切换只读取所选源，同 ID 不混合，源失败不显示上一来源的版本', async () => {
    mocks.api.packageSources.mockResolvedValue([source, { ...source, id: 'b', repository: 'owner/b' }, { ...source, id: 'c', repository: 'owner/c' }])
    mocks.api.packageSourceCatalog.mockImplementation(id => id === 'c' ? Promise.reject(new Error('源不可用')) : Promise.resolve({ packages: [entry] }))
    const w = mount(Market); await flushPromises()
    expect(w.findAll('.market-remote-card')).toHaveLength(1)
    expect(mocks.api.packageSourceCatalog).toHaveBeenCalledTimes(1)
    await w.get('select[aria-label="配置来源"]').setValue('b'); await flushPromises()
    expect(w.get('.market-remote-card').text()).toContain('owner/b')
    expect(w.get('.market-remote-card').text()).not.toContain('owner/a')
    await w.get('select[aria-label="配置来源"]').setValue('c'); await flushPromises()
    expect(w.findAll('.market-remote-card')).toHaveLength(0)
    expect(w.text()).toContain('源不可用'); w.unmount()
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
    const custom = { ...source, id: 'custom', name: '自建仓库', repository: 'owner/new' }
    mocks.api.savePackageSource.mockResolvedValue([source, custom])
    const w = mount(Market); await flushPromises()
    await w.findAll('button').find(b => b.text() === '管理预设').trigger('click')
    await w.get('input[aria-label="预设名称"]').setValue('自建仓库')
    await w.get('input[aria-label="来源 URL"]').setValue('git@github.com:owner/new.git'); await w.get('form').trigger('submit'); await flushPromises()
    expect(mocks.api.savePackageSource).toHaveBeenCalledWith('git@github.com:owner/new.git', true, { name: '自建仓库' })
    await w.findAll('button').find(b => b.text() === '删除当前预设').trigger('click'); await flushPromises()
    expect(mocks.api.removePackageSource).toHaveBeenCalledWith(custom.id)
    expect(mocks.api.importPackageArchive).not.toHaveBeenCalled(); w.unmount()
  })
  it('切换时忽略旧来源迟到的响应，并按当前来源下载', async () => {
    let resolveOld
    mocks.api.packageSources.mockResolvedValue([source, { ...source, id: 'lan', repository: 'http://192.168.31.10:13000/posase/gamer-packages' }])
    mocks.api.packageSourceCatalog.mockImplementation(id => id === source.id ? new Promise(resolve => { resolveOld = resolve }) : Promise.resolve({ packages: [{ ...entry, version: '2.0.0' }] }))
    const w = mount(Market); await flushPromises()
    await w.get('select[aria-label="配置来源"]').setValue('lan'); await flushPromises()
    resolveOld({ packages: [entry] }); await flushPromises()
    expect(w.get('.market-remote-card').text()).toContain('2.0.0')
    await w.get('.market-remote-actions button').trigger('click'); await flushPromises()
    expect(mocks.api.downloadSourcePackage).toHaveBeenCalledWith('lan', expect.objectContaining({ version: '2.0.0' }))
    w.unmount()
  })
})
