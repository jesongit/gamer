// @vitest-environment happy-dom
import { beforeEach, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import Login from './views/Login.vue'
import { login, getSetupStatus, setupInitialPassword } from './auth'

vi.mock('vue-router', () => ({useRouter: () => ({push: vi.fn(), replace: vi.fn()}), useRoute: () => ({query: {}})}))
vi.mock('./auth', async importOriginal => ({
  ...await importOriginal(), login: vi.fn(), getSetupStatus: vi.fn(), setupInitialPassword: vi.fn(),
}))
beforeEach(() => {
  vi.clearAllMocks()
  getSetupStatus.mockResolvedValue({ok:true, setupRequired:false})
  login.mockResolvedValue({ok:true})
  setupInitialPassword.mockResolvedValue({ok:true})
})

it('保持登录默认勾选，取消后将 false 传给登录 API', async () => {
  const wrapper = mount(Login)
  await flushPromises()
  const remember = wrapper.get('input[type="checkbox"]')
  expect(remember.element.checked).toBe(true)
  expect(wrapper.text()).toContain('保持登录（30 天）')
  await wrapper.get('input[autocomplete="username"]').setValue('admin')
  await wrapper.get('input[type="password"]').setValue('test-password')
  await wrapper.get('form').trigger('submit')
  await flushPromises()
  expect(login).toHaveBeenLastCalledWith('admin', 'test-password', true)
  await remember.setValue(false)
  await wrapper.get('form').trigger('submit')
  await flushPromises()
  expect(login).toHaveBeenLastCalledWith('admin', 'test-password', false)
  wrapper.unmount()
})

it('首次设置密码也传递保持登录选择', async () => {
  getSetupStatus.mockResolvedValue({ok:true, setupRequired:true})
  const wrapper = mount(Login)
  await flushPromises()
  for (const field of wrapper.findAll('input[type="password"]')) await field.setValue('test-password')
  await wrapper.get('input[type="checkbox"]').setValue(false)
  await wrapper.get('form').trigger('submit')
  await flushPromises()
  expect(setupInitialPassword).toHaveBeenCalledWith('test-password', 'test-password', false)
  wrapper.unmount()
})
