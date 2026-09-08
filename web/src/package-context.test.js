import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createHash } from 'node:crypto'
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
  it('下拉选择即切换 Core Store 的 currentPackageId', () => {
    const { api, toast } = setup()
    const ctx = usePackageContext({ api, toast })
    ctx.onPackageChange({ target: { value: 'user.other' } })
    expect(packageStore.currentPackageId).toBe('user.other')
    ctx.onPackageChange({ target: { value: '' } })
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

  it('导出（无媒体引用）：直接下载 blob + 文件名（无响应头文件名时回退 <id>.gamerpkg）', async () => {
    const { api, toast, download } = setup()
    api.getPackage.mockResolvedValue({ media_refs: [], media_total_bytes: 0 })
    api.exportPackageArchive.mockResolvedValue({
      blob: new Blob(['archive']), filename: '', sha256: 'b'.repeat(64),
    })
    const ctx = usePackageContext({ api, toast, download })
    await ctx.exportPackage()
    expect(api.exportPackageArchive).toHaveBeenCalledWith('com.demo', { includeMedia: false })
    expect(download).toHaveBeenCalledWith(expect.any(Blob), 'com.demo.gamerpkg')
    expect(ctx.exportModal.open).toBe(false)
  })

  it('导出（有媒体引用）：弹确认框；确认后按勾选带 includeMedia 导出', async () => {
    const { api, toast, download } = setup()
    api.getPackage.mockResolvedValue({
      media_refs: [
        { id: 'clip01', name: 'clip.mp4', size: 2048, plugin_id: 'gamer.video', kind: 'project', state: 'ready' },
      ],
      media_total_bytes: 2048,
    })
    api.exportPackageArchive.mockResolvedValue({
      blob: new Blob(['archive']), filename: 'com.demo-1.0.0.gamerpkg', sha256: 'b'.repeat(64),
    })
    const ctx = usePackageContext({ api, toast, download })
    await ctx.exportPackage()
    // 有引用素材：不直接导出，弹确认框并列出素材与大小
    expect(api.exportPackageArchive).not.toHaveBeenCalled()
    expect(ctx.exportModal.open).toBe(true)
    expect(ctx.exportModal.entries).toHaveLength(1)
    expect(ctx.exportModal.totalBytes).toBe(2048)
    expect(ctx.exportModal.includeMedia).toBe(false)

    // 默认（仅引用）导出
    await ctx.confirmExport()
    expect(api.exportPackageArchive).toHaveBeenCalledWith('com.demo', { includeMedia: false })
    expect(download).toHaveBeenCalledWith(expect.any(Blob), 'com.demo-1.0.0.gamerpkg')
    expect(ctx.exportModal.open).toBe(false)

    // 勾选「包含媒体素材」再导出 → includeMedia=true
    await ctx.exportPackage()
    expect(ctx.exportModal.open).toBe(true)
    ctx.exportModal.includeMedia = true
    await ctx.confirmExport()
    expect(api.exportPackageArchive).toHaveBeenLastCalledWith('com.demo', { includeMedia: true })
    // 素材查询失败不阻塞导出（退化为直接导出）
    api.getPackage.mockRejectedValue(new Error('query failed'))
    await ctx.exportPackage()
    expect(api.exportPackageArchive).toHaveBeenCalledTimes(3)
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
