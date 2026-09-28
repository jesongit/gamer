// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { ref, nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { usePackageContext } from './composables/usePackageContext'
import PackageDetailModal from './workspace/PackageDetailModal.vue'

function setup() {
  const targetId = ref('browser-cloud')
  const pkg = { id: 'cloud', revision: 4, version: '1.0.0', targets: { android: { packages: [] }, web: { url_prefixes: ['https://sr.mihoyo.com/cloud'] } }, plugins: [] }
  const api = {
    getPackage: vi.fn(async () => ({ package: pkg })),
    listPackages: vi.fn(async () => ({ packages: [pkg] })),
    updatePackage: vi.fn(async (id, body) => ({ ...pkg, ...body })),
    packageCompatibility: vi.fn(),
    targetIdentity: vi.fn(async () => ({ kind: 'web', value: 'https://sr.mihoyo.com/cloud?account=1#game', source: 'bound', note: '绑定的标签页' })),
    createPackage: vi.fn(),
  }
  const ctx = usePackageContext({ api, toast: vi.fn(), currentTargetId: targetId })
  return { ctx, api, targetId }
}

describe('配置包网页目标', () => {
  it('网页配置展示、编辑和保存不会丢失网址规则，空目标不能保存', async () => {
    const { ctx, api } = setup()
    await ctx.openDetail('cloud')
    const wrapper = mount(PackageDetailModal, { props: { context: ctx } })
    expect(wrapper.text()).toContain('网页: https://sr.mihoyo.com/cloud')
    expect(wrapper.text()).not.toContain('通用')
    ctx.startEdit()
    await nextTick()
    expect(wrapper.find('textarea').element.value).toBe('https://sr.mihoyo.com/cloud')
    await ctx.saveEdit()
    expect(api.updatePackage).toHaveBeenCalledWith('cloud', expect.objectContaining({ targets: { android: { packages: [] }, web: { url_prefixes: ['https://sr.mihoyo.com/cloud'] } }, expected_revision: 4 }))
    ctx.startEdit()
    ctx.detailModal.form.webUrlPrefixesText = ''
    api.updatePackage.mockClear()
    await ctx.saveEdit()
    expect(api.updatePackage).not.toHaveBeenCalled()
    expect(ctx.detailModal.editError).toContain('至少')
    wrapper.unmount()
  })

  it('从当前绑定目标添加站点和路径，新建网页包不隐式声明 Android', async () => {
    const { ctx, api } = setup()
    ctx.openCreate()
    ctx.formModal.form.id = 'cloud'
    await ctx.addCurrentFormTarget()
    expect(api.targetIdentity).toHaveBeenCalledWith('browser-cloud')
    expect(ctx.formModal.form.webUrlPrefixesText).toBe('https://sr.mihoyo.com/cloud')
    await ctx.submitForm()
    expect(api.createPackage).toHaveBeenCalledWith(expect.objectContaining({ targets: { android: { packages: [] }, web: { url_prefixes: ['https://sr.mihoyo.com/cloud'] } } }))
  })

  it('检查当前目标走 ID；目标切换后忽略迟到的检查与添加结果', async () => {
    const { ctx, api, targetId } = setup()
    await ctx.openDetail('cloud')
    let resolve
    api.packageCompatibility.mockReturnValue(new Promise(r => { resolve = r }))
    const pending = ctx.checkCompatibility(undefined, true)
    expect(api.packageCompatibility).toHaveBeenCalledWith('cloud', { target_id: 'browser-cloud' })
    targetId.value = 'browser-other'
    await nextTick()
    resolve({ status: 'match', compatible: true })
    await pending
    expect(ctx.detailModal.compat.result).toBeNull()
    ctx.openCreate()
    api.targetIdentity.mockReturnValue(new Promise(r => { resolve = r }))
    const adding = ctx.addCurrentFormTarget()
    targetId.value = 'browser-third'
    resolve({ kind: 'web', value: 'https://example.com' })
    await adding
    expect(ctx.formModal.form.webUrlPrefixesText).toBe('')
    expect(ctx.formModal.error).toContain('变化')
  })

  it('未知结果显示未知，离线估计带来源说明，手动网页检查保留完整网址', async () => {
    const { ctx, api } = setup()
    await ctx.openDetail('cloud')
    const wrapper = mount(PackageDetailModal, { props: { context: ctx } })
    ctx.detailModal.compat.kind = 'web'
    ctx.detailModal.compat.input = 'https://sr.mihoyo.com/cloud?a=1'
    api.packageCompatibility.mockResolvedValue({ status: 'unknown', compatible: null, reason: '无法确定目标身份', target: { note: '目标已变化' } })
    await ctx.checkCompatibility()
    expect(api.packageCompatibility).toHaveBeenCalledWith('cloud', { url: 'https://sr.mihoyo.com/cloud?a=1' })
    await flushPromises()
    expect(wrapper.text()).toContain('无法确定目标身份')
    expect(wrapper.find('.compat-ok').exists()).toBe(false)
    api.packageCompatibility.mockResolvedValue({ status: 'match', compatible: true, target: { source: 'configured', note: '未连接：按配置网址估计' } })
    await ctx.checkCompatibility(undefined, true)
    await flushPromises()
    expect(wrapper.text()).toContain('未连接：按配置网址估计')
    wrapper.unmount()
  })
})
