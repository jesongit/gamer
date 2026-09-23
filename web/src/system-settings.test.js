// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import SystemSettings from './components/SystemSettings.vue'
import ExtensionSettings from './components/ExtensionSettings.vue'

const base = { idle_power_secs:300, log_retain_days:14, compute_max_concurrency:0,
  auth:{ session_abs_secs:43200, session_idle_secs:7200, login_max_fails:10, login_window_secs:300 } }
const view = (saved = base, restart_required = false) => ({ saved, active:base, revision:'r1', restart_required })
const response = (body, status = 200) => ({ ok:status < 400, status, text:async () => JSON.stringify(body), json:async () => body, headers:{ get:() => 'application/json' } })
afterEach(() => vi.unstubAllGlobals())

describe('系统设置表单', () => {
  it('保存稀疏修改后的完整白名单设置，显示待重启，并保留冲突时的输入', async () => {
    const fetch = vi.fn(async (_url, options = {}) => {
      if (options.method !== 'PUT') return response(view())
      const request = JSON.parse(options.body)
      expect(request.expected_revision).toBe('r1')
      expect(request.settings.auth).toEqual(base.auth)
      expect(request.settings).not.toHaveProperty('password_hash')
      if (request.settings.idle_power_secs === 100) return response({ error:'配置已发生变化，请重新读取后再保存' },409)
      return response(view(request.settings, true))
    })
    vi.stubGlobal('fetch',fetch)
    const wrapper = mount(SystemSettings)
    await flushPromises()
    await wrapper.get('[aria-label="空闲省电时间"]').setValue(90)
    await wrapper.get('[aria-label="计算并发上限"]').setValue(2)
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(wrapper.text()).toContain('重启服务后生效')
    expect(wrapper.text()).toContain('当前设置：自动')
    await wrapper.get('[aria-label="空闲省电时间"]').setValue(100)
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('重新读取')
    expect(wrapper.get('[aria-label="空闲省电时间"]').element.value).toBe('100')
    wrapper.unmount()
  })

  it('阻止非法整数，读取失败时可以重试', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(response({ error:'读取失败' },500)).mockResolvedValue(response(view()))
    vi.stubGlobal('fetch',fetch)
    const wrapper = mount(SystemSettings)
    await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('读取失败')
    await wrapper.get('button').trigger('click')
    await flushPromises()
    await wrapper.get('[aria-label="普通登录有效期"]').setValue(0)
    await wrapper.get('form').trigger('submit')
    expect(wrapper.get('[role="alert"]').text()).toContain('60～2592000')
    expect(fetch).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })
})

describe('插件设置按能力发现', () => {
  it('只展示已运行且提供设置动作的插件，保存默认值并显示生效时机', async () => {
    const metadata = { title:'自动化', values:{default_timeout_secs:10,before_click_ms:300,after_click_ms:300}, fields:[{key:'default_timeout_secs',label:'默认模板等待超时',unit:'秒',min:1,max:3600,effect:'下次运行生效',help:'步骤指定的 timeout 优先'},{key:'before_click_ms',label:'点击前延迟',unit:'毫秒',min:0,max:60000,effect:'下次运行生效'},{key:'after_click_ms',label:'点击后延迟',unit:'毫秒',min:0,max:60000,effect:'下次运行生效'}] }
    const fetch = vi.fn(async (url, options = {}) => {
      if (url === '/api/extensions') return response({ extensions:[{id:'example-plugin',state:'running'},{id:'disabled-plugin',state:'enabled'}] })
      if (url.endsWith('/capabilities')) return response({running:true,actions:[{action:'settings.get'},{action:'settings.save'}]})
      const request = JSON.parse(options.body)
      if (request.action === 'settings.get') return response(metadata)
      expect(request).toEqual({action:'settings.save',values:{settings:{default_timeout_secs:25,before_click_ms:0,after_click_ms:500},expected:{default_timeout_secs:10,before_click_ms:300,after_click_ms:300}}})
      return response({...metadata,values:{default_timeout_secs:25,before_click_ms:0,after_click_ms:500}})
    })
    vi.stubGlobal('fetch',fetch)
    const wrapper = mount(ExtensionSettings)
    await flushPromises()
    expect(wrapper.text()).toContain('下次运行生效')
    expect(wrapper.get('[aria-label="点击前延迟"]').element.value).toBe('300')
    expect(wrapper.get('[aria-label="点击后延迟"]').element.value).toBe('300')
    await wrapper.get('[aria-label="点击前延迟"]').setValue(0)
    await wrapper.get('[aria-label="点击后延迟"]').setValue(500)
    await wrapper.get('[aria-label="默认模板等待超时"]').setValue(25)
    await wrapper.get('form').trigger('submit')
    await flushPromises()
    expect(wrapper.text()).toContain('正在运行的任务保持原值')
    expect(fetch.mock.calls.some(([url]) => url.includes('disabled-plugin'))).toBe(false)
    wrapper.unmount()
  })
})
