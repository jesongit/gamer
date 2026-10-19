// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import Component from './components/AdminAccountSettings.vue'
vi.mock('./api', () => ({ api: { getAdminAccount: vi.fn(), updateAdminAccount: vi.fn() } }))
vi.mock('./auth', () => ({ accountChanged: vi.fn() }))
import { api } from './api'
import { accountChanged } from './auth'
const view = { username: 'admin', revision: 'r1', password_editable: true }
afterEach(() => vi.resetAllMocks())
async function form(data = view) {
  api.getAdminAccount.mockResolvedValue(data)
  const wrapper = mount(Component)
  await flushPromises()
  return wrapper
}
describe('管理员账号设置', () => {
  it('支持只修改用户名；当前密码必填，保存成功清除密码并要求重新登录', async () => {
    const wrapper = await form()
    const inputs = wrapper.findAll('input')
    await inputs[0].setValue('new-owner')
    expect(wrapper.get('[type="submit"]').attributes('disabled')).toBeDefined()
    await inputs[1].setValue('current-password')
    api.updateAdminAccount.mockResolvedValue({ ok: true })
    await wrapper.get('form').trigger('submit'); await flushPromises()
    expect(api.updateAdminAccount).toHaveBeenCalledWith({ username: 'new-owner', current_password: 'current-password', new_password: '', confirm_password: '', expected_revision: 'r1' })
    expect(accountChanged).toHaveBeenCalledWith('new-owner')
    expect(inputs[1].element.value).toBe('')
    wrapper.unmount()
  })
  it('密码不一致不提交；错误密码留在当前表单并保留其他编辑', async () => {
    const wrapper = await form()
    const inputs = wrapper.findAll('input')
    await inputs[1].setValue('wrong-password'); await inputs[2].setValue('new-password')
    await wrapper.get('form').trigger('submit'); await flushPromises()
    expect(api.updateAdminAccount).not.toHaveBeenCalled()
    await inputs[3].setValue('new-password')
    api.updateAdminAccount.mockRejectedValue({ code: 'invalid_current_password' })
    await wrapper.get('form').trigger('submit'); await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('当前密码不正确')
    expect(inputs[1].element.value).toBe('')
    expect(inputs[2].element.value).toBe('new-password')
    expect(accountChanged).not.toHaveBeenCalled()
    wrapper.unmount()
  })
  it('环境变量管理密码时禁用密码修改，仍可修改用户名', async () => {
    const wrapper = await form({ ...view, password_editable: false })
    expect(wrapper.text()).toContain('GAMER_ADMIN_PASSWORD')
    const inputs = wrapper.findAll('input')
    expect(inputs[2].attributes('disabled')).toBeDefined()
    expect(inputs[3].attributes('disabled')).toBeDefined()
    expect(inputs[0].attributes('disabled')).toBeUndefined()
    wrapper.unmount()
  })
  it('读取失败可重试；冲突不覆盖新用户名', async () => {
    api.getAdminAccount.mockRejectedValueOnce(new Error('读取失败'))
    const wrapper = mount(Component); await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('读取失败')
    api.getAdminAccount.mockResolvedValue(view)
    await wrapper.get('button').trigger('click'); await flushPromises()
    const inputs = wrapper.findAll('input')
    await inputs[0].setValue('new-owner'); await inputs[1].setValue('current-password')
    api.updateAdminAccount.mockRejectedValue({ code: 'account_revision_conflict' })
    await wrapper.get('form').trigger('submit'); await flushPromises()
    expect(wrapper.get('[role="alert"]').text()).toContain('重新读取')
    expect(inputs[0].element.value).toBe('new-owner')
    wrapper.unmount()
  })
})
