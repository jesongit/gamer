// @vitest-environment happy-dom
// Package 详情弹窗（plan §18/§21/§37 + §17）与导入覆盖弹窗 Required Plugin
// 缺失提示（§36）：ctx 逻辑（usePackageContext）+ PackageDetailModal 挂载渲染。
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { ApiError } from './api'
import PackageDetailModal from './workspace/PackageDetailModal.vue'
import PackageContextBar from './workspace/PackageContextBar.vue'
import { formatBytes, usePackageContext } from './composables/usePackageContext'
import { packageStore, selectPackage } from './package-store'

function apiError(status, body) {
  return new ApiError({ status, code: body?.error || `http_${status}`, message: body?.message || '', data: body })
}

function fileLike(bytes) {
  const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)
  return { arrayBuffer: async () => buf }
}

function detailRep(overrides = {}) {
  return {
    package: {
      id: 'com.demo', name: 'Demo', version: '1.2.3', author: 'alice', revision: 7,
      targets: { android: { packages: ['com.miHoYo.hkrpg'] } },
      plugins: [
        { id: 'gamer.yaml', required: true },
        { id: 'gamer.keymap', required: false },
      ],
    },
    stats: {
      files: 12, bytes: 2048,
      plugins: [{ plugin: 'gamer.yaml', files: 10, bytes: 1024 }],
    },
    plugin_states: [
      { plugin: 'gamer.yaml', required: true, state: 'available' },
      { plugin: 'gamer.keymap', required: false, state: 'missing_optional' },
    ],
    ...overrides,
  }
}

function fiveStateRep() {
  return detailRep({
    plugins: [
      { id: 'p.a', required: true },
      { id: 'p.b', required: true },
      { id: 'p.c', required: true },
      { id: 'p.d', required: false },
      { id: 'p.e', required: false },
    ],
    plugin_states: [
      { plugin: 'p.a', required: true, state: 'available' },
      { plugin: 'p.b', required: true, state: 'disabled' },
      { plugin: 'p.c', required: true, state: 'missing_required' },
      { plugin: 'p.d', required: false, state: 'missing_optional' },
      { plugin: 'p.e', required: null, state: 'unknown' },
    ],
  })
}

function setup() {
  const api = {
    listPackages: vi.fn(async () => ({ packages: [{ id: 'com.demo', name: '', version: '1.0.0' }] })),
    getPackage: vi.fn(),
    updatePackage: vi.fn(),
    packageCompatibility: vi.fn(),
    listExtensions: vi.fn(async () => ({ extensions: [] })),
    importPackageArchive: vi.fn(),
  }
  const toast = vi.fn()
  const refreshAll = vi.fn(async () => {})
  return { api, toast, refreshAll }
}

/** 建 ctx 并打开详情弹窗（openDetail 用 store 的 currentPackageId）。 */
async function makeCtx(api, toast) {
  const ctx = usePackageContext({ api, toast })
  await ctx.openDetail()
  await flushPromises()
  return ctx
}

async function mountModal(ctx) {
  const wrapper = mount(PackageDetailModal, { props: { context: ctx, androidCandidates: ['com.miHoYo.hkrpg'] } })
  await flushPromises()
  return wrapper
}

beforeEach(() => {
  packageStore.packages = [
    { id: 'com.demo', name: 'Demo', version: '1.0.0', targets: { android: { packages: [] } } },
  ]
  packageStore.currentPackageId = 'com.demo'
  packageStore.loaded = true
})

afterEach(() => {
  vi.restoreAllMocks()
  selectPackage(null)
  packageStore.packages = []
})

describe('formatBytes（stats 展示）', () => {
  it('字节人性化', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(2048)).toBe('2.0 KB')
    expect(formatBytes(1024)).toBe('1.0 KB')
    expect(formatBytes(1536 * 1024)).toBe('1.5 MB')
    expect(formatBytes('bad')).toBe('0 B')
  })
})

describe('PackageDetailModal（plan §18/§21/§37 + §17）', () => {
  it('详情渲染 manifest 全字段 + 五态徽章 + missing_required 提示 + stats', async () => {
    const { api } = setup()
    api.getPackage.mockResolvedValue(fiveStateRep())
    const ctx = await makeCtx(api)
    const wrapper = await mountModal(ctx)

    const text = wrapper.text()
    expect(text).toContain('com.demo')
    expect(text).toContain('Demo')
    expect(text).toContain('1.2.3')
    expect(text).toContain('alice')
    expect(text).toContain('7') // revision
    expect(text).toContain('com.miHoYo.hkrpg')

    const badges = wrapper.findAll('.state-badge')
    expect(badges).toHaveLength(5)
    expect(badges[0].classes()).toContain('st-available')
    expect(badges[1].classes()).toContain('st-disabled')
    expect(badges[2].classes()).toContain('st-missing-required')
    expect(badges[3].classes()).toContain('st-missing-optional')
    expect(badges[4].classes()).toContain('st-unknown')
    expect(wrapper.find('.miss-hint').text()).toContain('部分功能不可用，可安装后恢复')

    expect(text).toContain('12') // stats.files
    expect(text).toContain('2.0 KB') // formatBytes(2048)
    expect(text).toContain('10 个文件 / 1.0 KB') // 插件级统计
  })

  it('0 targets 显示通用配置提示；兼容性检查不兼容 → 黄条 warning（不禁止）', async () => {
    const { api } = setup()
    api.getPackage.mockResolvedValue(detailRep({ package: { ...detailRep().package, targets: { android: { packages: [] } } } }))
    const ctx = await makeCtx(api)
    const wrapper = await mountModal(ctx)

    expect(wrapper.text()).toContain('通用配置，兼容所有应用')

    api.packageCompatibility.mockResolvedValueOnce({
      android_package: 'com.other.app', compatible: false, android_targets: [],
    })
    await wrapper.find('input[list="pkg-compat-candidates"]').setValue('com.other.app')
    await wrapper.findAll('button').find(b => b.text() === '检查').trigger('click')
    await flushPromises()

    expect(api.packageCompatibility).toHaveBeenCalledWith('com.demo', 'com.other.app')
    expect(wrapper.find('.compat-warning').text()).toContain('该应用不在配置声明的兼容列表中（仍可继续运行）')

    api.packageCompatibility.mockResolvedValueOnce({
      android_package: 'com.miHoYo.hkrpg', compatible: true, android_targets: ['com.miHoYo.hkrpg'],
    })
    await wrapper.find('input[list="pkg-compat-candidates"]').setValue('com.miHoYo.hkrpg')
    await wrapper.findAll('button').find(b => b.text() === '检查').trigger('click')
    await flushPromises()
    expect(wrapper.find('.compat-ok').text()).toContain('在配置声明的兼容列表中')
  })

  it('编辑保存：PUT 带 expected_revision + targets/plugins，成功后退出编辑态并 refreshPackages', async () => {
    const { api, toast } = setup()
    api.getPackage.mockResolvedValue(detailRep())
    const ctx = await makeCtx(api, toast)
    const wrapper = await mountModal(ctx)

    await wrapper.findAll('button').find(b => b.text().includes('编辑')).trigger('click')
    expect(ctx.detailModal.editing).toBe(true)
    // 表单预填
    expect(ctx.detailModal.form.name).toBe('Demo')
    expect(ctx.detailModal.form.androidPackagesText).toBe('com.miHoYo.hkrpg')
    expect(ctx.detailModal.form.plugins).toEqual([
      { id: 'gamer.yaml', required: true },
      { id: 'gamer.keymap', required: false },
    ])

    const inputs = wrapper.findAll('.detail-edit input.input')
    await inputs[1].setValue('2.0.0')          // version
    await inputs[3].setValue('com.A, com.B')   // android targets
    // 插件依赖行：勾掉第二行 gamer.keymap 的 required
    const checks = wrapper.findAll('.plugin-dep-row input[type="checkbox"]')
    await checks[1].setValue(false)

    api.updatePackage.mockResolvedValueOnce({ ...detailRep().package, revision: 8 })
    api.listPackages.mockResolvedValueOnce({ packages: [{ id: 'com.demo', name: '', version: '2.0.0' }] })
    await wrapper.findAll('button').find(b => b.text() === '保存').trigger('click')
    await flushPromises()

    expect(api.updatePackage).toHaveBeenCalledWith('com.demo', {
      name: 'Demo',
      version: '2.0.0',
      author: 'alice',
      targets: { android: { packages: ['com.A', 'com.B'] } },
      plugins: { 'gamer.yaml': true, 'gamer.keymap': false },
      expected_revision: 7,
    })
    expect(ctx.detailModal.editing).toBe(false)
    expect(api.listPackages).toHaveBeenCalled() // 保存后 refreshPackages
    expect(toast).toHaveBeenCalledWith(expect.stringContaining('元数据已保存'), 'success')
  })

  it('409 冲突：提示重试或强制保存；重试保留编辑按最新 revision 重放，强制保存不带 expected_revision', async () => {
    const { api, toast } = setup()
    api.getPackage.mockResolvedValue(detailRep())
    const ctx = await makeCtx(api, toast)
    const wrapper = await mountModal(ctx)

    await wrapper.findAll('button').find(b => b.text().includes('编辑')).trigger('click')
    await wrapper.findAll('.detail-edit input.input')[1].setValue('2.0.0')

    api.updatePackage.mockRejectedValueOnce(apiError(409, { message: 'expected_revision 冲突' }))
    await wrapper.findAll('button').find(b => b.text() === '保存').trigger('click')
    await flushPromises()

    expect(ctx.detailModal.editConflict).toBe(true)
    expect(wrapper.find('.conflict-warning').text()).toContain('强制覆盖')
    const buttonTexts = () => wrapper.findAll('button').map(b => b.text())
    expect(buttonTexts()).toContain('重试')
    expect(buttonTexts()).toContain('强制保存')

    // 重试：重拉最新详情（revision 更新）并保留用户编辑
    api.getPackage.mockResolvedValueOnce(detailRep())
    await wrapper.findAll('button').find(b => b.text() === '重试').trigger('click')
    await flushPromises()
    expect(api.getPackage).toHaveBeenCalledTimes(2)
    expect(ctx.detailModal.editConflict).toBe(false)
    expect(ctx.detailModal.form.version).toBe('2.0.0')
    expect(buttonTexts()).not.toContain('强制保存') // 冲突解除后回到普通保存

    // 再次冲突后强制保存：force:true 且不再带 expected_revision
    api.updatePackage
      .mockRejectedValueOnce(apiError(409, { message: 'expected_revision 冲突' }))
      .mockResolvedValueOnce({ ...detailRep().package, revision: 9 })
    await wrapper.findAll('button').find(b => b.text() === '保存').trigger('click')
    await flushPromises()
    expect(ctx.detailModal.editConflict).toBe(true)
    await wrapper.findAll('button').find(b => b.text() === '强制保存').trigger('click')
    await flushPromises()
    expect(api.updatePackage).toHaveBeenLastCalledWith('com.demo', {
      name: 'Demo',
      version: '2.0.0',
      author: 'alice',
      targets: { android: { packages: ['com.miHoYo.hkrpg'] } },
      plugins: { 'gamer.yaml': true, 'gamer.keymap': false },
      force: true,
    })
    expect(ctx.detailModal.editing).toBe(false)
  })

  it('取消编辑不触发 PUT；加载失败可重试关闭', async () => {
    const { api } = setup()
    api.getPackage.mockResolvedValue(detailRep())
    const ctx = await makeCtx(api)
    const wrapper = await mountModal(ctx)

    await wrapper.findAll('button').find(b => b.text().includes('编辑')).trigger('click')
    await wrapper.findAll('button').find(b => b.text() === '取消').trigger('click')
    expect(ctx.detailModal.editing).toBe(false)
    expect(api.updatePackage).not.toHaveBeenCalled()

    api.getPackage.mockRejectedValueOnce(apiError(404, { message: '配置不存在' }))
    await ctx.openDetail()
    await flushPromises()
    expect(ctx.detailModal.error).toContain('配置不存在')
    api.getPackage.mockResolvedValueOnce(detailRep())
    await ctx.reloadDetail()
    await flushPromises()
    expect(ctx.detailModal.error).toBe('')
  })
})

describe('导入覆盖弹窗 Required Plugin 缺失提示（plan §36）', () => {
  it('existing.plugins 对照已安装扩展：缺 required 列出、optional 不提示', async () => {
    const { api, toast } = setup()
    const bytes = new TextEncoder().encode('archive')
    api.importPackageArchive.mockRejectedValueOnce(apiError(409, {
      error: 'package_exists',
      message: '已存在',
      existing: {
        id: 'pkg.demo', name: 'Demo', version: '1.0.0',
        plugins: [
          { id: 'gamer.yaml', required: true },
          { id: 'other.plugin', required: true },
          { id: 'opt.plugin', required: false },
        ],
      },
    }))
    api.listExtensions.mockResolvedValue({ extensions: [{ id: 'gamer.yaml', state: 'Running' }] })
    const ctx = usePackageContext({ api, toast })
    await ctx.importPackage(fileLike(bytes))

    expect(ctx.overwriteModal.open).toBe(true)
    expect(ctx.overwriteModal.missingRequired).toEqual(['other.plugin'])
  })

  it('扩展列表不可得时不阻塞：弹窗照常打开、可继续覆盖导入', async () => {
    const { api, toast, refreshAll } = setup()
    const bytes = new TextEncoder().encode('archive')
    api.importPackageArchive
      .mockRejectedValueOnce(apiError(409, {
        error: 'package_exists',
        existing: { id: 'pkg.demo', plugins: [{ id: 'other.plugin', required: true }] },
      }))
      .mockResolvedValueOnce({ id: 'pkg.demo', version: '1.0.0' })
    api.listExtensions.mockRejectedValueOnce(new Error('boom'))
    api.listPackages.mockResolvedValue({ packages: [{ id: 'pkg.demo', name: '', version: '1.0.0' }] })
    const ctx = usePackageContext({ api, toast, refreshAll })
    await ctx.importPackage(fileLike(bytes))

    expect(ctx.overwriteModal.open).toBe(true)
    expect(ctx.overwriteModal.missingRequired).toEqual([])
    await ctx.confirmOverwrite()
    expect(api.importPackageArchive).toHaveBeenCalledTimes(2)
    expect(api.importPackageArchive.mock.calls[1][1]).toMatchObject({ overwrite: true })
    expect(ctx.overwriteModal.open).toBe(false)
  })

  it('PackageContextBar 覆盖弹窗渲染缺失插件黄条（§36 文案）', async () => {
    const { api, toast } = setup()
    const bytes = new TextEncoder().encode('archive')
    api.importPackageArchive.mockRejectedValueOnce(apiError(409, {
      error: 'package_exists',
      existing: {
        id: 'pkg.demo', name: 'Demo', version: '1.0.0',
        plugins: [{ id: 'other.plugin', required: true }],
      },
    }))
    // setup() 默认 listExtensions 返回空扩展表 → other.plugin 必然缺失
    const ctx = usePackageContext({ api, toast })
    const wrapper = mount(PackageContextBar, { props: { context: ctx } })
    await ctx.importPackage(fileLike(bytes))
    await flushPromises()

    expect(ctx.overwriteModal.open).toBe(true)
    expect(ctx.overwriteModal.missingRequired).toEqual(['other.plugin'])
    const warning = wrapper.find('.missing-required-warning')
    expect(warning.exists()).toBe(true)
    expect(warning.text()).toContain('缺少插件 other.plugin')
    expect(warning.text()).toContain('导入后部分功能不可用，安装插件后自动恢复')
  })

  it('顶栏「详情」按钮打开 PackageDetailModal（当前包为空时禁用）', async () => {
    const { api } = setup()
    const ctx = usePackageContext({ api, toast: vi.fn() })
    const wrapper = mount(PackageContextBar, { props: { context: ctx } })

    const detailBtn = () => wrapper.findAll('button').find(b => b.text().includes('详情'))
    expect(detailBtn().attributes('disabled')).toBeUndefined() // 有当前包 → 可点

    await detailBtn().trigger('click')
    await flushPromises()
    expect(api.getPackage).toHaveBeenCalledWith('com.demo')
    expect(wrapper.findComponent(PackageDetailModal).exists()).toBe(true)
    expect(wrapper.text()).toContain('配置详情')

    // 无当前包 → 按钮禁用
    selectPackage(null)
    await flushPromises()
    expect(detailBtn().attributes('disabled')).toBeDefined()
    expect(api.getPackage).toHaveBeenCalledTimes(1)
  })
})
