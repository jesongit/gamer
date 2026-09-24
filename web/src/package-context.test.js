import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createHash } from 'node:crypto'
import { zipSync, strToU8 } from 'fflate'
import { ApiError } from './api'
import {
  isValidPackageId, normalizePackageId, usePackageContext,
} from './composables/usePackageContext'
import { packageStore, selectPackage } from './package-store'

// 与 registry-client.ts 的 WebCrypto 实现交叉验证（node:crypto 独立计算）
const sha256 = (buf) => createHash('sha256').update(Buffer.from(buf)).digest('hex')

function fileLike(bytes) {
  const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)
  return { arrayBuffer: async () => buf }
}

function apiError(status, body) {
  return new ApiError({ status, code: body?.error || `http_${status}`, message: body?.message || '', data: body })
}

function setup() {
  const api = {
    listPackages: vi.fn(),
    getPackage: vi.fn(),
    createPackage: vi.fn(),
    duplicatePackage: vi.fn(),
    deletePackage: vi.fn(),
    importPackageArchive: vi.fn(),
    exportPackageArchive: vi.fn(),
  }
  const toast = vi.fn()
  const confirmDialog = vi.fn(() => true)
  const refreshAll = vi.fn(async () => {})
  const download = vi.fn()
  return { api, toast, confirmDialog, refreshAll, download }
}

beforeEach(() => {
  packageStore.packages = [
    { id: 'com.demo', name: 'Demo', version: '1.0.0', targets: { android: { packages: [] } } },
    { id: 'user.other', name: '', version: '0.1.0', targets: { android: { packages: [] } } },
  ]
  packageStore.currentPackageId = 'com.demo'
  packageStore.loaded = true
})

afterEach(() => {
  vi.restoreAllMocks()
  selectPackage(null)
  packageStore.packages = []
})

describe('Package id 规整（服务端 validate_scope_id 前置口径）', () => {
  it('小写化 + 语法门禁', () => {
    expect(normalizePackageId(' User.HSR.Daily ')).toBe('user.hsr.daily')
    expect(isValidPackageId('user.hsr.daily')).toBe(true)
    expect(isValidPackageId('com.mihoyo.hkrpg')).toBe(true)
    expect(isValidPackageId('')).toBe(false)
    // 规整先小写化：大写输入是合法 id 的小写变体
    expect(isValidPackageId('User.demo')).toBe(true)
    expect(isValidPackageId('..demo')).toBe(false)
    expect(isValidPackageId('../escape')).toBe(false)
    expect(isValidPackageId('has space')).toBe(false)
  })
})

describe('usePackageContext（plan §28：导入/导出/新建/复制/删除）', () => {
  it('等待异步草稿确认时不切换配置，取消还原选择且阻止重复切换', async () => {
    let answer
    const beforePackageChange = vi.fn(() => new Promise(resolve => { answer = resolve }))
    const ctx = usePackageContext({ ...setup(), beforePackageChange })
    const target = { value: 'user.other' }
    const pending = ctx.onPackageChange({ target })
    expect(packageStore.currentPackageId).toBe('com.demo')
    expect(target.value).toBe('com.demo')
    await ctx.onPackageChange({ target: { value: '' } })
    expect(beforePackageChange).toHaveBeenCalledTimes(1)
    answer(false); await pending
    expect(packageStore.currentPackageId).toBe('com.demo')
    const accepted = ctx.onPackageChange({ target: { value: 'user.other' } })
    answer(true); await accepted
    expect(packageStore.currentPackageId).toBe('user.other')
  })
  it('下拉选择即切换 Core Store 的 currentPackageId', async () => {
    const { api, toast } = setup()
    const ctx = usePackageContext({ api, toast })
    await ctx.onPackageChange({ target: { value: 'user.other' } })
    expect(packageStore.currentPackageId).toBe('user.other')
    await ctx.onPackageChange({ target: { value: '' } })
    expect(packageStore.currentPackageId).toBeNull()
  })

  it('导入成功：SHA-256 头 + 刷新包列表 + 选中导入的包 + refreshAll', async () => {
    const { api, toast, refreshAll } = setup()
    api.listPackages.mockResolvedValue({ packages: [{ id: 'pkg.new', name: '', version: '1.0.0' }] })
    api.importPackageArchive.mockResolvedValue({ id: 'pkg.new', version: '1.0.0' })
    const ctx = usePackageContext({ api, toast, refreshAll })
    const file = fileLike(new TextEncoder().encode('archive'))
    await ctx.importPackage(file)
    expect(api.importPackageArchive).toHaveBeenCalledTimes(1)
    const [importBytes, importOpts] = api.importPackageArchive.mock.calls[0]
    expect(new TextDecoder().decode(importBytes)).toBe('archive')
    expect(importOpts).toEqual({
      expectedSha256: sha256(new TextEncoder().encode('archive')),
    })
    expect(packageStore.currentPackageId).toBe('pkg.new')
    expect(refreshAll).toHaveBeenCalledTimes(1)
    expect(toast).toHaveBeenCalledWith(expect.stringContaining('pkg.new'), 'success')
  })

  it('同 id 导入 409：打开覆盖确认弹窗；确认后带 overwrite 重发并刷新', async () => {
    const { api, toast, refreshAll } = setup()
    const bytes = new TextEncoder().encode('archive')
    api.importPackageArchive
      .mockRejectedValueOnce(apiError(409, {
        error: 'package_exists',
        message: '已存在',
        existing: { id: 'pkg.demo', name: 'Demo', version: '0.9.0', targets: { android: { packages: ['com.x'] } }, plugins: [] },
      }))
      .mockResolvedValueOnce({ id: 'pkg.demo', version: '1.0.0' })
    api.listPackages.mockResolvedValue({ packages: [{ id: 'pkg.demo', name: '', version: '1.0.0' }] })
    const ctx = usePackageContext({ api, toast, refreshAll })
    await ctx.importPackage(fileLike(bytes))
    expect(ctx.overwriteModal.open).toBe(true)
    expect(ctx.overwriteModal.summary.id).toBe('pkg.demo')
    expect(ctx.overwriteModal.summary.androidTargets).toEqual(['com.x'])
    await ctx.confirmOverwrite()
    expect(api.importPackageArchive).toHaveBeenCalledTimes(2)
    expect(api.importPackageArchive.mock.calls[1][1]).toEqual({
      expectedSha256: sha256(bytes), overwrite: true,
    })
    expect(ctx.overwriteModal.open).toBe(false)
    expect(refreshAll).toHaveBeenCalledTimes(1)
  })

  function archive(entries = [], extra = {}) {
    return { blob: new Blob([zipSync({
      'package.toml': strToU8('id = "com.demo"'),
      'plugins/gamer-yaml/automations/main.yaml': strToU8('run: []'),
      'plugins/gamer-yaml/automations/_function.yaml': strToU8('functions: {}'),
      'plugins/gamer-yaml/templates/首页.png': new Uint8Array([1, 2]),
      'shared/readme.txt': strToU8('notes'),
      'plugins/uninstalled/custom.dat': new Uint8Array([3]),
      'media/index.json': strToU8(JSON.stringify({ schema_version: 1, entries })),
      ...extra,
    })]), filename: '', sha256: 'b'.repeat(64) }
  }

  it('无媒体也预览实际清单，确认下载同一份文件并绑定打开时的包', async () => {
    const { api, toast, download } = setup()
    const exported = archive()
    api.exportPackageArchive.mockResolvedValue(exported)
    const ctx = usePackageContext({ api, toast, download })
    await ctx.exportPackage()
    expect(download).not.toHaveBeenCalled()
    expect(ctx.exportModal.ready).toBe(true)
    expect(ctx.exportModal.files).toHaveLength(7)
    expect(ctx.exportModal.groups).toEqual(expect.arrayContaining([
      { label: '自动化脚本', count: 1 }, { label: '函数库文件', count: 1 },
      { label: '模板图片', count: 1 }, { label: '共享文件', count: 1 },
      { label: '插件资源 · uninstalled', count: 1 },
    ]))
    expect(ctx.exportModal.archiveBytes).toBe(exported.blob.size)
    selectPackage('user.other')
    await ctx.confirmExport()
    expect(api.exportPackageArchive).toHaveBeenCalledTimes(1)
    expect(api.exportPackageArchive).toHaveBeenCalledWith('com.demo', { includeMedia: false })
    expect(download).toHaveBeenCalledWith(exported.blob, 'com.demo.gamerpkg')
    expect(ctx.exportModal.open).toBe(false)
    await ctx.confirmExport()
    expect(download).toHaveBeenCalledTimes(1)
  })

  it('切换媒体选项重新准备清单，按内容去重统计且保留缺失标注', async () => {
    const { api, download } = setup()
    const media = { id: 'clip01', name: 'clip.mp4', size: 2048, sha256: 'a'.repeat(64), plugin_id: 'gamer-video', kind: 'project', included: false }
    api.exportPackageArchive.mockResolvedValueOnce(archive([media, { ...media, kind: 'other' }]))
    const ctx = usePackageContext({ api, download })
    await ctx.exportPackage()
    expect(ctx.exportModal.totalBytes).toBe(2048)
    ctx.exportModal.includeMedia = true
    await ctx.confirmExport()
    expect(download).not.toHaveBeenCalled()
    const included = archive([{ ...media, included: true }, { ...media, id: 'missing', sha256: 'b'.repeat(64) }], {
      ['media/files/' + media.sha256]: new Uint8Array(2048),
    })
    api.exportPackageArchive.mockResolvedValueOnce(included)
    await ctx.refreshExport()
    expect(api.exportPackageArchive).toHaveBeenLastCalledWith('com.demo', { includeMedia: true })
    expect(ctx.exportModal.entries.map(e => e.included)).toEqual([true, false])
    expect(ctx.exportModal.files.some(file => file.category === '媒体原文件')).toBe(true)
    await ctx.confirmExport()
    expect(download).toHaveBeenCalledWith(included.blob, 'com.demo.gamerpkg')
  })

  it('关闭加载中的预览后，旧请求不覆盖新包、不触发下载', async () => {
    const { api, download } = setup()
    let resolveOld
    api.exportPackageArchive.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve }))
    const ctx = usePackageContext({ api, download })
    const pending = ctx.exportPackage()
    expect(ctx.exportModal.loading).toBe(true)
    await ctx.confirmExport()
    ctx.closeExport()
    selectPackage('user.other')
    api.exportPackageArchive.mockResolvedValueOnce({ ...archive(), filename: 'other.gamerpkg' })
    await ctx.exportPackage()
    resolveOld({ ...archive(), filename: 'old.gamerpkg' })
    await pending
    expect(ctx.exportModal.packageId).toBe('user.other')
    expect(ctx.exportModal.filename).toBe('other.gamerpkg')
    expect(download).not.toHaveBeenCalled()
    ctx.closeExport()
    expect(ctx.exportModal.files).toEqual([])
  })

  it('准备失败或归档非法时禁止下载并允许重试，下载失败保留清单', async () => {
    const { api, download } = setup()
    api.exportPackageArchive.mockRejectedValueOnce(new Error('network failed'))
    const ctx = usePackageContext({ api, download })
    await ctx.exportPackage()
    expect(ctx.exportModal.error).toBe('network failed')
    await ctx.confirmExport()
    expect(download).not.toHaveBeenCalled()
    api.exportPackageArchive.mockResolvedValueOnce({ blob: new Blob(['invalid']) })
    await ctx.refreshExport()
    expect(ctx.exportModal.ready).toBe(false)
    api.exportPackageArchive.mockResolvedValueOnce(archive())
    await ctx.refreshExport()
    expect(ctx.exportModal.ready).toBe(true)
    download.mockRejectedValueOnce(new Error('download failed'))
    await ctx.confirmExport()
    expect(ctx.exportModal.open).toBe(true)
    expect(ctx.exportModal.error).toBe('download failed')
    await ctx.confirmExport()
    expect(ctx.exportModal.open).toBe(false)
  })

  it('新建：非法 id 客户端拒绝；合法 id 走 createPackage 并选中', async () => {
    const { api, toast, refreshAll } = setup()
    api.createPackage.mockResolvedValue({ id: 'user.new' })
    api.listPackages.mockResolvedValue({ packages: [{ id: 'user.new', name: '', version: '0.1.0' }] })
    const ctx = usePackageContext({ api, toast, refreshAll })
    ctx.openCreate()
    ctx.formModal.form = { id: 'Bad Id', name: '', version: '1.0.0', androidPackagesText: '' }
    await ctx.submitForm()
    expect(api.createPackage).not.toHaveBeenCalled()
    expect(ctx.formModal.error).toContain('非法')

    ctx.formModal.form = { id: 'User.New', name: 'New', version: '1.0.0', androidPackagesText: 'com.A, com.B' }
    await ctx.submitForm()
    expect(api.createPackage).toHaveBeenCalledWith({
      id: 'user.new', version: '1.0.0', name: 'New',
      targets: { android: { packages: ['com.A', 'com.B'] } },
    })
    expect(packageStore.currentPackageId).toBe('user.new')
    expect(refreshAll).toHaveBeenCalledTimes(1)
  })

  it('新建配置可填入当前应用：ID 自动小写，兼容目标保留包名，名称使用应用名', async () => {
    const { api, toast } = setup()
    const ctx = usePackageContext({
      api, toast,
      currentApp: { value: { pkg: 'com.example.Game', label: '示例游戏' } },
    })
    ctx.openCreate()
    await ctx.fillCurrentApp()
    expect(ctx.formModal.form).toMatchObject({
      id: 'com.example.game',
      name: '示例游戏',
      androidPackagesText: 'com.example.Game',
    })
  })

  it('新建表单默认 Android Targets = *（通用配置、零插件依赖）', async () => {
    const { api, toast } = setup()
    api.createPackage.mockResolvedValue({ id: 'user.new' })
    api.listPackages.mockResolvedValue({ packages: [{ id: 'user.new', name: '', version: '0.1.0' }] })
    const ctx = usePackageContext({ api, toast })
    ctx.openCreate()
    expect(ctx.formModal.form.androidPackagesText).toBe('*')
    ctx.formModal.form.id = 'user.new'
    ctx.formModal.form.name = ''
    await ctx.submitForm()
    expect(api.createPackage).toHaveBeenCalledWith({
      id: 'user.new', version: '1.0.0',
      targets: { android: { packages: ['*'] } },
    })
  })

  it('复制：duplicatePackage(new_id) 后选中新包', async () => {
    const { api, toast, refreshAll } = setup()
    api.duplicatePackage.mockResolvedValue({ id: 'user.demo.copy' })
    api.listPackages.mockResolvedValue({ packages: [{ id: 'user.demo.copy', name: '', version: '1.0.0' }] })
    const ctx = usePackageContext({ api, toast, refreshAll })
    ctx.openDuplicate()
    expect(ctx.formModal.mode).toBe('duplicate')
    expect(ctx.formModal.form.id).toBe('user.com.demo.copy')
    ctx.formModal.form.id = 'user.demo.copy'
    await ctx.submitForm()
    expect(api.duplicatePackage).toHaveBeenCalledWith('com.demo', 'user.demo.copy')
    expect(packageStore.currentPackageId).toBe('user.demo.copy')
  })

  it('删除：确认后调 deletePackage 并刷新（当前包回落由 store 负责）', async () => {
    const { api, toast } = setup()
    api.deletePackage.mockResolvedValue(undefined)
    api.listPackages.mockResolvedValue({ packages: [{ id: 'user.other', name: '', version: '0.1.0' }] })
    const ctx = usePackageContext({ api, toast })
    ctx.openDelete()
    expect(ctx.deleteModal.target.id).toBe('com.demo')
    await ctx.confirmDelete()
    expect(api.deletePackage).toHaveBeenCalledWith('com.demo')
    expect(ctx.deleteModal.open).toBe(false)
  })
})
