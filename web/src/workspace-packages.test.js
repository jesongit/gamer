import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createHash } from 'node:crypto'
import { ref } from 'vue'
import { ApiError } from './api'
import { parseAndroidPackages, useWorkspacePackages } from './composables/useWorkspacePackages'

// 与 registry-client.ts 的 WebCrypto 实现交叉验证（node:crypto 独立计算）
const sha256 = (buf) => createHash('sha256').update(Buffer.from(buf)).digest('hex')

const STATS = { scripts: 2, functions: 1, templates: 3, keymaps: 0, presets: 0, resources: 0 }

function fileLike(bytes) {
  const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)
  return { arrayBuffer: async () => buf }
}

function setup() {
  const api = {
    listAppPackages: vi.fn(),
    installAppPackage: vi.fn(),
    exportAppPackage: vi.fn(),
    editAppPackage: vi.fn(),
    getWorkspace: vi.fn(),
    saveWorkspace: vi.fn(),
  }
  const toast = vi.fn()
  const activePkg = ref('com.demo')
  const loadData = vi.fn(async () => {})
  const refreshFnLib = vi.fn(async () => {})
  const refreshKeymaps = vi.fn(async () => {})
  const download = vi.fn()
  const tools = useWorkspacePackages({
    api, toast, activePkg, loadData, refreshFnLib, refreshKeymaps, download,
  })
  return { api, toast, activePkg, loadData, refreshFnLib, refreshKeymaps, download, tools }
}

const toastOf = (toast, type) => toast.mock.calls.find(([, t]) => t === type)?.[0] || ''

describe('游戏包导入', () => {
  let bytes
  beforeEach(() => { bytes = new TextEncoder().encode('gamerpkg-archive-bytes') })

  it('读文件字节算 SHA-256 后安装；成功 toast 含 id@version 并全量刷新；input 复位', async () => {
    const { api, toast, tools, loadData, refreshFnLib, refreshKeymaps } = setup()
    api.installAppPackage.mockResolvedValue({ id: 'pkg.demo', active_version: '1.2.0' })
    const target = { files: [fileLike(bytes)], value: 'C:\\x.pkg' }
    await tools.onImportPicked({ target })

    expect(api.installAppPackage).toHaveBeenCalledTimes(1)
    const [sentBytes, sentSha] = api.installAppPackage.mock.calls[0]
    expect(Buffer.from(sentBytes).toString()).toBe('gamerpkg-archive-bytes')
    expect(sentSha).toBe(sha256(bytes))
    expect(toastOf(toast, 'success')).toContain('pkg.demo@1.2.0')
    expect(loadData).toHaveBeenCalledTimes(1)
    expect(refreshFnLib).toHaveBeenCalledWith('com.demo')
    expect(refreshKeymaps).toHaveBeenCalledWith('com.demo')
    expect(target.value).toBe('')
  })

  it('409 同版本已安装 → toast 追加「该版本已存在」人话提示', async () => {
    const { api, toast, tools } = setup()
    api.installAppPackage.mockRejectedValue(new ApiError({
      status: 409, code: 'App Package 已安装: pkg.demo@1.0.0', message: 'App Package 已安装: pkg.demo@1.0.0',
    }))
    await tools.importPackage(fileLike(bytes))
    const msg = toastOf(toast, 'error')
    expect(msg).toContain('App Package 已安装: pkg.demo@1.0.0')
    expect(msg).toContain('该版本已存在，请修改版本号后重新导出')
  })

  it('409 primary 冲突 → toast 提示「同一安卓应用已有激活的游戏包」', async () => {
    const { api, toast, tools } = setup()
    api.installAppPackage.mockRejectedValue(new ApiError({
      status: 409,
      message: 'Android package com.demo 已由其他 App Package 激活: other@2.0；需先卸载或切换激活',
    }))
    await tools.importPackage(fileLike(bytes))
    const msg = toastOf(toast, 'error')
    expect(msg).toContain('同一安卓应用已有激活的游戏包，请先卸载或切换')
  })

  it('导入进行中重复触发被 busy 守卫拒绝', async () => {
    const { api, tools } = setup()
    let release
    api.installAppPackage.mockReturnValue(new Promise(resolve => { release = resolve }))
    const first = tools.importPackage(fileLike(bytes))
    const second = tools.importPackage(fileLike(bytes))
    release({ id: 'p', active_version: '1' })
    await Promise.all([first, second])
    expect(api.installAppPackage).toHaveBeenCalledTimes(1)
  })
})

describe('游戏包导出', () => {
  it('编辑区未初始化 → 先弹元数据弹窗并按当前分区预填默认值', async () => {
    const { api, tools } = setup()
    api.getWorkspace.mockResolvedValue({ metadata: null, stats: STATS })
    await tools.openExport()
    expect(api.getWorkspace).toHaveBeenCalledWith('com.demo')
    expect(tools.metaModal.open).toBe(true)
    expect(tools.metaModal.form).toEqual({
      id: 'com.demo', name: '', version: '1.0.0', androidPackagesText: 'com.demo',
    })
    expect(tools.exportModal.open).toBe(false)
  })

  it('初始化保存成功 → PUT workspace（空名称省略）→ 带着统计进入导出确认弹窗', async () => {
    const { api, tools } = setup()
    api.getWorkspace.mockResolvedValue({ metadata: null, stats: STATS })
    api.saveWorkspace.mockResolvedValue({
      metadata: { id: 'pkg.demo', version: '1.0.0', android_packages: ['com.demo'] },
    })
    await tools.openExport()
    tools.metaModal.form.id = 'pkg.demo'
    await tools.submitMeta()
    expect(api.saveWorkspace).toHaveBeenCalledWith('com.demo', {
      id: 'pkg.demo', version: '1.0.0', android_packages: ['com.demo'],
    })
    expect(tools.metaModal.open).toBe(false)
    expect(tools.exportModal.open).toBe(true)
    expect(tools.exportModal.info).toMatchObject({ id: 'pkg.demo', version: '1.0.0', stats: STATS })
  })

  it('保存 400 校验失败 → 错误留在初始化弹窗内不关窗', async () => {
    const { api, tools } = setup()
    api.getWorkspace.mockResolvedValue({ metadata: null, stats: {} })
    api.saveWorkspace.mockRejectedValue(new ApiError({ status: 400, message: '版本号不合法: abc' }))
    await tools.openExport()
    await tools.submitMeta()
    expect(tools.metaModal.open).toBe(true)
    expect(tools.metaModal.error).toContain('版本号不合法')
    expect(tools.exportModal.open).toBe(false)
  })

  it('元数据已存在 → 直接进入导出确认弹窗', async () => {
    const { api, tools } = setup()
    api.getWorkspace.mockResolvedValue({
      metadata: { id: 'pkg.demo', version: '3.1.0', android_packages: ['com.demo'] },
      stats: STATS,
    })
    await tools.openExport()
    expect(tools.metaModal.open).toBe(false)
    expect(tools.exportModal.open).toBe(true)
    expect(tools.exportModal.info).toMatchObject({ id: 'pkg.demo', version: '3.1.0' })
  })

  it('确认导出 → 按 Content-Disposition 文件名下载，toast 含 SHA-256 前 12 位', async () => {
    const { api, toast, download, tools } = setup()
    api.getWorkspace.mockResolvedValue({
      metadata: { id: 'pkg.demo', version: '1.0.0', android_packages: ['com.demo'] },
      stats: STATS,
    })
    api.exportAppPackage.mockResolvedValue({
      blob: new Blob(['archive']),
      filename: 'pkg.demo-1.0.0.gamerpkg',
      sha256: 'abcdef0123456789' + '0'.repeat(48),
    })
    await tools.openExport()
    await tools.confirmExport()
    expect(api.exportAppPackage).toHaveBeenCalledWith('com.demo')
    expect(download).toHaveBeenCalledWith(expect.anything(), 'pkg.demo-1.0.0.gamerpkg')
    expect(toastOf(toast, 'success')).toContain('abcdef012345')
    expect(tools.exportModal.open).toBe(false)
  })

  it('响应头解析不出文件名 → 回退 <id>-<version>.gamerpkg', async () => {
    const { api, download, tools } = setup()
    api.getWorkspace.mockResolvedValue({
      metadata: { id: 'pkg.demo', version: '2.0.0', android_packages: ['com.demo'] },
      stats: {},
    })
    api.exportAppPackage.mockResolvedValue({ blob: new Blob(['x']), filename: '', sha256: '' })
    await tools.openExport()
    await tools.confirmExport()
    expect(download).toHaveBeenCalledWith(expect.anything(), 'pkg.demo-2.0.0.gamerpkg')
  })

  it('400 preflight_failed → 弹窗保持打开并逐行展示问题（去掉标题行）', async () => {
    const { api, tools } = setup()
    api.getWorkspace.mockResolvedValue({
      metadata: { id: 'pkg.demo', version: '1.0.0', android_packages: ['com.demo'] },
      stats: {},
    })
    api.exportAppPackage.mockRejectedValue(new ApiError({
      status: 400,
      code: 'preflight_failed',
      message: '导出 preflight 失败:\n- 模板 hit.png 缺失\n- 脚本 main.yaml 引用了不存在的模板',
      data: {
        code: 'preflight_failed',
        error: '导出 preflight 失败:\n- 模板 hit.png 缺失\n- 脚本 main.yaml 引用了不存在的模板',
      },
    }))
    await tools.openExport()
    await tools.confirmExport()
    expect(tools.exportModal.open).toBe(true)
    expect(tools.exportModal.errorLines).toEqual([
      '- 模板 hit.png 缺失',
      '- 脚本 main.yaml 引用了不存在的模板',
    ])
  })
})

describe('游戏包编辑', () => {
  it('没有激活包 → toast 提示先导入，不弹确认窗', async () => {
    const { api, toast, tools } = setup()
    api.listAppPackages.mockResolvedValue({
      packages: [{ id: 'pkg.demo', active_version: null, android_packages: ['com.demo'] }],
    })
    await tools.openEdit()
    expect(toastOf(toast, 'info')).toBe('当前应用没有已激活的 Gamer 游戏包，请先导入')
    expect(tools.editModal.open).toBe(false)
    expect(api.editAppPackage).not.toHaveBeenCalled()
  })

  it('找到激活包 → 确认弹窗 target=当前分区；确认后调用 edit 并全量刷新（toast 含替换总数）', async () => {
    const { api, toast, tools, refreshFnLib, refreshKeymaps } = setup()
    api.listAppPackages.mockResolvedValue({
      packages: [
        { id: 'pkg.demo', active_version: '2.0.0', android_packages: ['com.demo', 'com.other'] },
      ],
    })
    await tools.openEdit()
    expect(tools.editModal.open).toBe(true)
    expect(tools.editModal.showTargetPicker).toBe(false)
    expect(tools.editModal.target).toBe('com.demo')

    api.editAppPackage.mockResolvedValue({
      android_package: 'com.demo',
      replaced: { scripts: 2, functions: 1, templates: 3, keymaps: 0, presets: 0, resources: 0 },
    })
    await tools.confirmEdit()
    expect(api.editAppPackage).toHaveBeenCalledWith('pkg.demo', '2.0.0', 'com.demo')
    expect(toastOf(toast, 'success')).toContain('替换 6 项资源')
    expect(tools.editModal.open).toBe(false)
    expect(refreshFnLib).toHaveBeenCalledWith('com.demo')
    expect(refreshKeymaps).toHaveBeenCalledWith('com.demo')
  })

  it('当前分区不在包 android_packages → 弹窗提供 target 单选并回退第一个；可改选后提交', async () => {
    const { api, tools } = setup()
    api.listAppPackages.mockResolvedValue({
      packages: [
        { id: 'pkg.demo', active_version: '2.0.0', android_packages: ['com.other', 'com.third'] },
      ],
    })
    await tools.openEdit()
    expect(tools.editModal.showTargetPicker).toBe(true)
    expect(tools.editModal.target).toBe('com.other')
    tools.editModal.target = 'com.third'
    api.editAppPackage.mockResolvedValue({ replaced: { scripts: 0 } })
    await tools.confirmEdit()
    expect(api.editAppPackage).toHaveBeenCalledWith('pkg.demo', '2.0.0', 'com.third')
  })

  it('编辑失败 → toast 服务端消息且弹窗保持打开', async () => {
    const { api, toast, tools } = setup()
    api.listAppPackages.mockResolvedValue({
      packages: [{ id: 'pkg.demo', active_version: '2.0.0', android_packages: ['com.demo'] }],
    })
    await tools.openEdit()
    api.editAppPackage.mockRejectedValue(new ApiError({ status: 400, message: 'target 不在该包 android.packages: com.demo' }))
    await tools.confirmEdit()
    expect(toastOf(toast, 'error')).toContain('target 不在该包 android.packages')
    expect(tools.editModal.open).toBe(true)
  })
})

describe('parseAndroidPackages', () => {
  it('逗号/中文逗号/换行混用均可解析，trim 并去重', () => {
    expect(parseAndroidPackages('a.b, c.d\n\ne.f，a.b ')).toEqual(['a.b', 'c.d', 'e.f'])
    expect(parseAndroidPackages('')).toEqual([])
    expect(parseAndroidPackages(null)).toEqual([])
  })
})
