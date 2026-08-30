// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import Settings from './views/Settings.vue'

const SYSTEM_INFO = {
  schema_version: 1,
  app: {
    version: '0.2.0-dev',
    git_commit: '0123456789abcdef',
    built_at: '2026-08-31T00:00:00Z',
    channel: 'dev',
    target: 'x86_64-windows',
  },
  deployment: { mode: 'direct', update_strategy: 'unsupported' },
  readiness: { ready: false, status: 'not_ready' },
  dependencies: {
    adb: { status: 'ready', version: '1.0.41', source: 'system' },
    ffmpeg: { status: 'missing', version: null, source: 'system' },
    scrcpy: { status: 'ready', version: '3.3.3', source: 'bundled' },
    data: { status: 'ready' },
    database: { status: 'ready' },
  },
  schema: {
    database: { version: 1, status: 'ready' },
    files: { version: 1, status: 'ready' },
    rollback_floor: 1,
  },
  capabilities: { check: false, download: false, install: false, rollback: false },
  timezone: { name: 'Asia/Shanghai', offset: '+08:00', source: 'TZ' },
  startup: { stage: 'ready', boot_id: 'boot-1' },
}

function response(status, body) {
  return {
    ok: status < 400,
    status,
    json: async () => body,
  }
}

afterEach(() => vi.unstubAllGlobals())

describe('Settings 系统状态页', () => {
  it('请求期间保持 loading，不显示任何原型设置', async () => {
    let resolve
    vi.stubGlobal('fetch', vi.fn(() => new Promise(r => { resolve = r })))
    const wrapper = mount(Settings)

    expect(wrapper.get('[role="status"]').text()).toContain('正在读取系统状态')
    expect(wrapper.text()).not.toContain('设置已保存')
    expect(wrapper.text()).not.toContain('管理员密码')

    resolve(response(200, SYSTEM_INFO))
    await flushPromises()
    wrapper.unmount()
  })

  it('成功时只显示服务端系统信息、健康状态和不可用更新能力', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => response(200, SYSTEM_INFO)))
    const wrapper = mount(Settings)
    await flushPromises()

    expect(wrapper.text()).toContain('0.2.0-dev')
    expect(wrapper.text()).toContain('Asia/Shanghai (+08:00)')
    expect(wrapper.text()).toContain('ffmpeg')
    expect(wrapper.text()).toContain('缺失')
    expect(wrapper.text()).toContain('不可用')
    expect(wrapper.text()).not.toContain('设置已保存')
    expect(fetch).toHaveBeenCalledWith('/api/system/info', {
      headers: { Accept: 'application/json' },
    })
    wrapper.unmount()
  })

  it('接口未接入或服务不可达时显示 error 与重试入口', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => response(404, { error: 'not_found' })))
    const wrapper = mount(Settings)
    await flushPromises()

    expect(wrapper.get('[role="alert"]').text()).toContain('系统状态暂不可用')
    expect(wrapper.text()).toContain('not_found')
    expect(wrapper.text()).toContain('接口未接入')
    expect(wrapper.findAll('button').some(button => button.text() === '重试')).toBe(true)
    wrapper.unmount()
  })
})
