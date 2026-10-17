// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import PackageContextBar from './workspace/PackageContextBar.vue'
import { usePackageContext } from './composables/usePackageContext'
import { packageStore, selectPackage } from './package-store'
let wrapper
afterEach(() => { wrapper?.unmount(); selectPackage(null); packageStore.packages = [] })
describe('常驻配置包选择', () => {
  it('新目标尚未选择配置包时显示明确入口，保留独立上下文', () => {
    packageStore.packages = [{ id:'default',name:'默认配置' }]
    selectPackage(null)
    wrapper = mount(PackageContextBar,{ props:{ context:usePackageContext({toast:vi.fn()}), compact:true, dialogs:false } })
    expect(wrapper.get('select').element.selectedOptions[0].textContent).toBe('选择配置包…')
    expect(packageStore.currentPackageId).toBeNull()
  })
  it('异步候选晚于已选标识到达时仍展示当前配置', async () => {
    packageStore.packages = []
    selectPackage(null)
    const ctx = usePackageContext({ toast:vi.fn() })
    wrapper = mount(PackageContextBar,{ props:{ context:ctx, compact:true, dialogs:false } })
    // A previously restored identifier can precede the asynchronous list.
    packageStore.currentPackageId = 'default'
    await nextTick()
    packageStore.packages = [{ id:'default',name:'默认配置' }]
    await nextTick()
    expect(wrapper.get('select').element.value).toBe('default')
    expect(wrapper.get('select').element.selectedOptions[0].textContent).toContain('默认配置')
  })
  it('草稿确认期间显示原配置，取消保留，接受后再切换', async () => {
    packageStore.packages = [{ id:'default',name:'默认配置' },{ id:'other',name:'另一配置' }]
    selectPackage('default')
    let resolve
    const ctx = usePackageContext({ toast:vi.fn(),beforePackageChange:vi.fn(() => new Promise(done => { resolve = done })) })
    wrapper = mount(PackageContextBar,{ props:{ context:ctx, compact:true, dialogs:false } })
    await wrapper.get('select').setValue('other')
    expect(packageStore.currentPackageId).toBe('default')
    expect(wrapper.get('select').element.value).toBe('default')
    resolve(false)
    await flushPromises()
    expect(wrapper.get('select').element.value).toBe('default')
    await wrapper.get('select').setValue('other')
    resolve(true)
    await flushPromises()
    expect(packageStore.currentPackageId).toBe('other')
    expect(wrapper.get('select').element.value).toBe('other')
  })
})
