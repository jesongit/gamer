// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import PluginCenter from './workspace/plugin-center/PluginCenter.vue'
import ConfirmDialogHost from './components/ui/ConfirmDialogHost.vue'
import { finishConfirmation } from './components/ui/useConfirmDialog'

const { entry } = vi.hoisted(() => ({ entry: { id: 'demo.plugin', name: '测试插件', version: '1.1.0', sha256: 'a'.repeat(64), download_url: '/demo.gplugin', execution: { kind: 'wasm' } } }))
vi.mock('./workspace/plugin-center/registry-client', async importOriginal => ({
  ...await importOriginal(),
  fetchRegistry: vi.fn(async () => ({ schema_version: 2, plugins: [entry] })),
  downloadFixedVersion: vi.fn(async () => ({ file: new File(['demo'], 'demo.gplugin') })),
}))
let center, host
afterEach(() => { center?.unmount(); host?.unmount(); finishConfirmation(false); document.body.innerHTML = ''; vi.restoreAllMocks() })
async function setup() {
  const client = {
    getExtensionManagement: vi.fn(async () => ({ extensions: [{ ...entry, active_version: '1.0.0', version: '1.0.0', state: 'disabled', permissions: ['resource.read'] }] })),
    inspectExtension: vi.fn(async () => ({ ...entry, permissions: ['resource.read', 'device.control'] })),
    updateExtension: vi.fn(async () => ({ ...entry, state: 'disabled' })),
  }
  host = mount(ConfirmDialogHost, { attachTo: document.body })
  center = mount(PluginCenter, { props: { apiClient: client } })
  await flushPromises()
  return client
}
async function openUpdate() {
  await center.findAll('button').find(button => button.text() === '更新到 1.1.0').trigger('click')
  await flushPromises()
}
it('更新显示前后版本及新增权限，取消不上传，确认后只上传一次', async () => {
  const client = await setup()
  const native = vi.spyOn(window, 'confirm').mockImplementation(() => { throw new Error('不应调用原生确认') })
  await openUpdate()
  const text = document.querySelector('.confirmation-dialog').textContent
  expect(text).toContain('1.0.0 → 1.1.0')
  expect(text).toContain('新增权限：device.control')
  expect(client.updateExtension).not.toHaveBeenCalled()
  document.querySelector('.confirmation-dialog footer button').click()
  await flushPromises()
  expect(client.updateExtension).not.toHaveBeenCalled()
  await openUpdate()
  document.querySelector('.confirmation-dialog .btn-primary').click()
  await flushPromises()
  expect(client.updateExtension).toHaveBeenCalledTimes(1)
  expect(client.updateExtension.mock.calls[0][2]).toMatchObject({ permissionConfirmed: true, source: 'official' })
  expect(native).not.toHaveBeenCalled()
})
it('等待确认时卸载插件页取消请求，不在后台继续更新', async () => {
  const client = await setup()
  await openUpdate()
  center.unmount()
  await flushPromises()
  expect(client.updateExtension).not.toHaveBeenCalled()
  expect(document.querySelector('.confirmation-dialog')).toBeNull()
})
