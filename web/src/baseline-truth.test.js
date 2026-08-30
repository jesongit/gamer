import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'

const root = new URL('./', import.meta.url)
const read = (path) => readFileSync(new URL(path, root), 'utf8')

describe('清洁基线的凭据与版本展示', () => {
  it('Docker 配置只声明 Argon2id PHC 或开发环境变量入口', () => {
    const config = read('../../docker-config.toml')

    expect(config).toContain('[auth]')
    expect(config).toContain('password_hash = ""')
    expect(config).toContain('Argon2id PHC')
    expect(config).toContain('GAMER_ADMIN_PASSWORD')
    expect(config).not.toMatch(/^\s*password\s*=/m)
  })

  it('登录页不提供默认凭据暗示，并说明服务端会话与部署凭据', () => {
    const login = read('./views/Login.vue')
    const versionPattern = /v\d+\.\d+\.\d+/

    expect(login).toContain("const user = ref('')")
    expect(login).toContain('当前部署配置的管理员凭据')
    expect(login).toContain('服务端会话')
    expect(login).not.toContain('placeholder="admin"')
    expect(login).not.toContain('默认账号')
    expect(login).not.toMatch(versionPattern)
  })

  it('主布局消费服务端系统信息，版本缺失时降级且不显示固定日志徽标', () => {
    const layout = read('./layouts/MainLayout.vue')
    const legacyBadgeClass = ['nav', 'badge'].join('-')

    expect(layout).toContain("fetch('/api/system/info'")
    expect(layout).toContain("'dev/unknown'")
    expect(layout).toContain('systemInfo.value?.app?.version')
    expect(layout).not.toMatch(/v\d+\.\d+\.\d+/)
    expect(layout).not.toContain(legacyBadgeClass)
    expect(layout).not.toContain('>3<')
  })
})
