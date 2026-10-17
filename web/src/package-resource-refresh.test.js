// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope } from 'vue'
import { flushPromises } from '@vue/test-utils'
import { usePackageContext } from './composables/usePackageContext'
import { packageStore } from './package-store'
import { pluginMessageChannel } from './workspace/plugin-messages'
const scopes = [], changed = pluginMessageChannel('core:package-resources-changed')
const detail = (id = 'pkg', files = 1, revision = 1) => ({ package: { id, name: id, revision }, stats: { files, bytes: files * 10 }, plugin_states: [] })
function context() {
  const api = { listPackages: vi.fn(async () => ({ packages: [{ id: 'pkg', revision: 2 }] })), getPackage: vi.fn(async id => detail(id)) }
  const scope = effectScope(); scopes.push(scope)
  const ctx = scope.run(() => usePackageContext({ api })); return { api, ctx }
}
beforeEach(() => { packageStore.packages = [{ id: 'pkg' }]; packageStore.currentPackageId = 'pkg'; packageStore.loaded = true; packageStore.loading = false; changed.packageId = ''; changed.seq = 0 })
afterEach(() => scopes.splice(0).forEach(s => s.stop()))
describe('package resource notification', () => {
  it('refreshes package summary and visible resource counts after a publish', async () => {
    const { api, ctx } = context(); await ctx.openDetail('pkg')
    api.getPackage.mockResolvedValue(detail('pkg', 12, 2))
    changed.packageId = 'pkg'; changed.seq++; await flushPromises()
    expect(api.listPackages).toHaveBeenCalledTimes(1)
    expect(ctx.detailModal.detail.stats.files).toBe(12)
    expect(ctx.detailModal.detail.package.revision).toBe(2)
  })
  it('preserves metadata edits and their base revision while refreshing counts', async () => {
    const { api, ctx } = context(); await ctx.openDetail('pkg'); ctx.startEdit()
    ctx.detailModal.form.name = 'unsaved name'; api.getPackage.mockResolvedValue(detail('pkg', 9, 5))
    changed.packageId = 'pkg'; changed.seq++; await flushPromises()
    expect(ctx.detailModal.detail.stats.files).toBe(9)
    expect(ctx.detailModal.form.name).toBe('unsaved name')
    expect(ctx.detailModal.detail.package.revision).toBe(1)
  })
  it('ignores unrelated packages and late detail responses after closing', async () => {
    const { api, ctx } = context(); await ctx.openDetail('pkg')
    changed.packageId = 'other'; changed.seq++; await flushPromises()
    expect(api.listPackages).not.toHaveBeenCalled()
    let resolve; api.getPackage.mockReturnValue(new Promise(done => { resolve = done }))
    const reload = ctx.reloadDetail(); ctx.closeDetail(); resolve(detail('pkg', 99)); await reload
    expect(ctx.detailModal.detail.stats.files).toBe(1)
  })
})
