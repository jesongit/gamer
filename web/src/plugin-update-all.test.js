// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import PluginCenter from './workspace/plugin-center/PluginCenter.vue'
import ConfirmDialogHost from './components/ui/ConfirmDialogHost.vue'
import { finishConfirmation } from './components/ui/useConfirmDialog'
import { downloadFixedVersion, fetchRegistry } from './workspace/plugin-center/registry-client'

vi.mock('./workspace/plugin-center/registry-client', async importOriginal => ({
  ...await importOriginal(),
  fetchRegistry: vi.fn(),
  downloadFixedVersion: vi.fn(),
}))

let center, host
const entry = (id, version = '2.0.0', extra = {}) => ({ id, name: id, version, sha256: 'a'.repeat(64), download_url: `/${id}.gplugin`, execution: { kind: 'wasm' }, ...extra })
const installed = (id, version = '1.0.0', extra = {}) => ({ ...entry(id, version), active_version: version, state: 'disabled', permissions: ['resource.read'], ...extra })

beforeEach(() => {
  vi.clearAllMocks()
  downloadFixedVersion.mockImplementation(async plugin => ({ file: new File([plugin.id], plugin.id) }))
})
afterEach(() => {
  center?.unmount(); host?.unmount(); finishConfirmation(false)
  document.body.innerHTML = ''
  vi.restoreAllMocks()
})

async function setup(market, extensions) {
  let current = extensions
  fetchRegistry.mockResolvedValue({ schema_version: 2, plugins: market })
  const client = {
    getExtensionManagement: vi.fn(async () => ({ extensions: current })),
    inspectExtension: vi.fn(async file => ({ ...market.find(plugin => plugin.id === file.name), permissions: ['resource.read', 'device.control'] })),
    updateExtension: vi.fn(async id => {
      const target = market.find(plugin => plugin.id === id)
      const snapshot = { ...current.find(plugin => plugin.id === id), version: target.version, active_version: target.version }
      current = current.map(plugin => plugin.id === id ? snapshot : plugin)
      return snapshot
    }),
    installExtension: vi.fn(),
    enableExtension: vi.fn(),
  }
  host = mount(ConfirmDialogHost, { attachTo: document.body })
  center = mount(PluginCenter, { props: { apiClient: client }, attachTo: document.body })
  await flushPromises()
  return client
}

function updateButton() { return center.findAll('.plugin-toolbar button').find(button => button.text() === '更新全部') }
async function startUpdates() { await updateButton().trigger('click'); await flushPromises() }
async function confirmUpdates() { document.querySelector('.confirmation-dialog .btn-primary').click(); await flushPromises() }

it('刷新后显示更新全部，搜索不限制候选，一次确认真实权限后只更新已安装的较低版本', async () => {
  const client = await setup([
    entry('alpha'), entry('beta'), entry('same', '1.0.0'), entry('newer'), entry('absent'), entry('incompatible'),
  ], [
    installed('alpha', '1.0.0', { state: 'running' }), installed('beta'), installed('same'), installed('newer', '3.0.0'),
    installed('incompatible', '1.0.0', { execution: { kind: 'builtin' } }),
  ])
  expect(center.findAll('.plugin-toolbar button').map(button => button.text()).slice(0, 3)).toEqual(['刷新', '更新全部', '本地导入'])
  await center.get('[aria-label="搜索插件"]').setValue('alpha')
  await startUpdates()
  expect(fetchRegistry).toHaveBeenLastCalledWith(expect.any(Function), expect.any(String), { refresh: true })
  expect(downloadFixedVersion.mock.calls.map(([plugin]) => plugin.id)).toEqual(['alpha', 'beta'])
  expect(client.updateExtension).not.toHaveBeenCalled()
  const dialog = document.querySelector('.confirmation-dialog')
  expect(dialog.textContent).toContain('更新以下 2 个插件')
  expect(dialog.textContent).toContain('beta')
  expect(dialog.textContent).toContain('1.0.0 → 2.0.0')
  expect(dialog.textContent).toContain('新增权限：device.control')
  expect(updateButton().attributes('disabled')).toBeDefined()
  await confirmUpdates()
  expect(client.updateExtension.mock.calls.map(([id]) => id)).toEqual(['alpha', 'beta'])
  expect(client.updateExtension.mock.calls.every(([, , options]) => options.permissionConfirmed && options.source === 'official')).toBe(true)
  expect(client.installExtension).not.toHaveBeenCalled()
  expect(client.enableExtension).not.toHaveBeenCalled()
  expect(center.text()).toContain('成功 2 个，失败 0 个')
  expect(document.querySelector('.confirmation-dialog')).toBeNull()
  expect(center.emitted('changed')).toHaveLength(1)
  await center.get('[aria-label="搜索插件"]').setValue('')
  expect(center.get('[data-plugin-id="beta"]').text()).toContain('已停用')
  expect(center.get('[data-plugin-id="alpha"]').text()).toContain('运行中')
})

it('准备期间按钮不可重入，取消汇总确认不上传任何插件', async () => {
  const client = await setup([entry('alpha')], [installed('alpha')])
  let resolveDownload
  downloadFixedVersion.mockImplementationOnce(() => new Promise(resolve => { resolveDownload = resolve }))
  await startUpdates()
  expect(updateButton().attributes('disabled')).toBeDefined()
  await updateButton().trigger('click')
  expect(downloadFixedVersion).toHaveBeenCalledTimes(1)
  resolveDownload({ file: new File(['alpha'], 'alpha') })
  await flushPromises()
  document.querySelector('.confirmation-dialog footer button').click()
  await flushPromises()
  expect(client.updateExtension).not.toHaveBeenCalled()
  expect(center.text()).toContain('已取消全部更新')
  expect(updateButton().attributes('disabled')).toBeUndefined()
})

it('单个更新失败继续其余插件，最终状态刷新失败仍保留各插件结果', async () => {
  const client = await setup([entry('alpha'), entry('beta')], [installed('alpha'), installed('beta')])
  client.updateExtension.mockRejectedValueOnce(new Error('插件正在使用，更新失败'))
  await startUpdates()
  client.getExtensionManagement.mockRejectedValueOnce(new Error('状态读取暂时失败'))
  await confirmUpdates()
  expect(client.updateExtension.mock.calls.map(([id]) => id)).toEqual(['alpha', 'beta'])
  expect(center.text()).toContain('成功 1 个，失败 1 个')
  expect(center.text()).toContain('alpha：插件正在使用，更新失败')
  expect(center.text()).toContain('beta 已更新到 v2.0.0')
  expect(center.text()).toContain('刷新插件状态失败：状态读取暂时失败')
  expect(center.emitted('changed')).toHaveLength(1)
})

it('缺少哈希、校验失败或归档身份不匹配均不上传，其余候选仍可确认更新', async () => {
  const client = await setup([
    entry('nohash', '2.0.0', { sha256: undefined }), entry('badhash'), entry('wrong'), entry('good'),
  ], [installed('nohash'), installed('badhash'), installed('wrong'), installed('good')])
  downloadFixedVersion.mockRejectedValueOnce(Object.assign(new Error('文件与索引不一致'), { code: 'hash_mismatch' }))
  client.inspectExtension.mockResolvedValueOnce(entry('other'))
  await startUpdates()
  expect(downloadFixedVersion.mock.calls.map(([plugin]) => plugin.id)).toEqual(['badhash', 'wrong', 'good'])
  const text = document.querySelector('.confirmation-dialog').textContent
  expect(text).toContain('更新以下 1 个插件')
  expect(text).toContain('无法更新的插件')
  expect(text).toContain('固定版本校验失败')
  expect(text).toContain('SHA-256 校验失败')
  await confirmUpdates()
  expect(client.updateExtension.mock.calls.map(([id]) => id)).toEqual(['good'])
  expect(center.text()).toContain('成功 1 个，失败 3 个')
})

it('刷新读取失败时不使用旧候选；没有更新时直接提示', async () => {
  const client = await setup([entry('alpha')], [installed('alpha')])
  fetchRegistry.mockRejectedValueOnce(new Error('市场无法访问'))
  await startUpdates()
  expect(center.text()).toContain('市场无法访问')
  expect(downloadFixedVersion).not.toHaveBeenCalled()
  expect(client.updateExtension).not.toHaveBeenCalled()
  fetchRegistry.mockResolvedValue({ schema_version: 2, plugins: [entry('alpha', '1.0.0')] })
  await startUpdates()
  expect(center.text()).toContain('已安装插件均无可用更新')
  expect(document.querySelector('.confirmation-dialog')).toBeNull()
})

it('全部候选无法准备时汇总失败；等待确认离开插件页不会继续更新', async () => {
  const client = await setup([entry('alpha')], [installed('alpha')])
  downloadFixedVersion.mockRejectedValueOnce(new Error('下载超时'))
  await startUpdates()
  expect(center.text()).toContain('成功 0 个，失败 1 个')
  expect(center.text()).toContain('alpha：下载超时')
  expect(document.querySelector('.confirmation-dialog')).toBeNull()
  await startUpdates()
  center.unmount()
  await flushPromises()
  expect(client.updateExtension).not.toHaveBeenCalled()
  expect(document.querySelector('.confirmation-dialog')).toBeNull()
})
