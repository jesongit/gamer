import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { assertMatchingBuildVersion } from '../build-version.mjs'

const cargo = version => `[package]\nname = "gamer-server"\nversion = "${version}"\n\n[dependencies]\nother = "1"\n`

describe('前端构建版本门禁', () => {
  it('当前仓库前端元数据必须与服务端权威版本一致', () => {
    const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'))
    const manifest = readFileSync(new URL('../../server/Cargo.toml', import.meta.url), 'utf8')
    expect(assertMatchingBuildVersion(pkg.version, manifest)).toBe(pkg.version)
  })

  it('拒绝旧前端元数据与新服务端组合，并明确指出两个版本', () => {
    expect(() => assertMatchingBuildVersion('0.5.16', cargo('0.5.17')))
      .toThrow("web/package.json version '0.5.16' != server/Cargo.toml package.version '0.5.17'")
  })

  it('保留预发布版本完整值并处理 CRLF 与行末注释', () => {
    const manifest = '# version = "9.9.9"\r\n[package] # product\r\nversion = "0.6.0-beta.1" # release\r\n[dependencies.example]\r\nversion = "1.0.0"\r\n'
    expect(assertMatchingBuildVersion('0.6.0-beta.1', manifest)).toBe('0.6.0-beta.1')
    expect(() => assertMatchingBuildVersion('0.6.0', manifest)).toThrow('[version-check]')
  })

  it('缺少产品版本时不能误取依赖版本或继续生成产物', () => {
    expect(() => assertMatchingBuildVersion('1.0.0', '[package]\nname = "gamer-server"\n[dependencies.example]\nversion = "1.0.0"\n'))
      .toThrow('Cannot read [package].version')
  })
})
