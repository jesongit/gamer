// @vitest-environment happy-dom
/**
 * 独立脚本页（ScriptEditor.vue）三页签外壳行为测试。视图含路由/后端依赖，
 * 按仓库惯例不整体接真实路由：vi.mock vue-router（捕获 push）与 ./api（契约化桩），
 * store 用真实模块注入数据。
 *
 * 锁定的行为：
 * - 页签切换驱动左栏列表数据源 / 新建入口 / 空态文案：脚本=脚本列表+新建脚本、
 *   函数库=func 文件（file 短路径 + 函数名清单）+新建函数库、模板=管理形态
 *   （缩略图短名列表 + 上传入口，条目点击开详情：预览/重命名/删除/跳控制台）；
 * - 跨页签模型不外泄（回归）：脚本打开时切到函数库页签，中央画布不显示脚本、
 *   测试函数面板/保存入口不出现，切回原页签模型仍在（不丢未保存内容）；
 * - 函数编辑上下文：打开函数文件进 FunctionLibraryModel（编辑函数/测试函数下拉）、
 *   新建函数库保存走 createFunction 并刷新列表、测试函数经 functions run 接口发起。
 */
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

const routerPush = vi.hoisted(() => vi.fn())
vi.mock('vue-router', async (importOriginal) => ({
  ...(await importOriginal()),
  useRouter: () => ({ push: routerPush }),
}))

vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal()),
  api: {
    listScripts: vi.fn(async () => []),
    listDevices: vi.fn(async () => []),
    listTemplates: vi.fn(async () => []),
    tplImageUrl: (name, pkg) => `/api/templates/${encodeURIComponent(name)}/image?pkg=${encodeURIComponent(pkg)}`,
    listFunctions: vi.fn(async () => []),
    getScript: vi.fn(),
    getFunction: vi.fn(),
    createScript: vi.fn(),
    updateScript: vi.fn(),
    createFunction: vi.fn(),
    updateFunction: vi.fn(),
    deleteScript: vi.fn(),
    deleteFunction: vi.fn(),
    createTemplate: vi.fn(),
    replaceTemplateImage: vi.fn(),
    renameTemplate: vi.fn(),
    deleteTemplate: vi.fn(),
    runScript: vi.fn(),
    runFunction: vi.fn(),
    getRun: vi.fn(),
    deviceRun: vi.fn(async () => ({ active: false })),
  },
}))

import ScriptEditor from './views/ScriptEditor.vue'
import { api } from './api'
import { store, scriptsData, devicesData } from './store'

const PKG = 'com.test.app'
const SCRIPT = { id: `${PKG}/verify_a.yml`, name: 'verify_a.yml', package: PKG, updated_at: '2026-08-30T02:04:00' }
const FUNC_ID = `${PKG}/common.yaml`
// 函数文件内容：顶层键 = 函数名，函数记录只有 params/steps
const FUNC_CONTENT = 'f1:\n  steps:\n    - log: hello\n'
const FUNC_FILE = { id: FUNC_ID, pkg: PKG, file: 'common', content: FUNC_CONTENT, version: 'a1', functions: ['f1'], updated_at: '2026-08-30T01:00:00' }
const TMPLS = [{ name: '开始挑战#757_909_857_971.png' }, { name: 'plain.png' }]

let funcFiles = [] // listFunctions 数据源（保存后追加，模拟服务端 upsert + 列表刷新）
let wrapper = null

async function mountView() {
  wrapper = mount(ScriptEditor)
  await flushPromises()
  return wrapper
}

const tabs = (w) => w.findAll('.res-tab')
const items = (w) => w.findAll('.res-items .res-item')
const centerEmpty = (w) => w.find('.editor-main .ed-empty')
const findBtn = (w, text) => w.findAll('button').find((b) => b.text().includes(text))

beforeEach(() => {
  vi.clearAllMocks()
  funcFiles = [{ ...FUNC_FILE }]
  scriptsData.value = [{ ...SCRIPT }]
  devicesData.value = []
  store.deviceId = null
  Object.assign(store, { running: false, runId: null })
  localStorage.clear()

  api.listScripts.mockResolvedValue([{ ...SCRIPT }])
  api.listDevices.mockResolvedValue([])
  api.listFunctions.mockImplementation(async () => funcFiles.map((f) => ({ ...f })))
  api.listTemplates.mockResolvedValue(TMPLS.map((t) => ({ ...t })))
  // upsert + 列表可见（与真实服务端同构，保存后 refresh 能看到新文件）
  api.createFunction.mockImplementation(async (p) => {
    const entry = { id: `${p.pkg}/${p.name}.yaml`, pkg: p.pkg, file: p.name, content: p.content, version: 'v2', functions: ['func1'], updated_at: '2026-08-30T03:00:00' }
    funcFiles.push(entry)
    return { ...entry }
  })
})

afterEach(() => {
  if (wrapper) {
    wrapper.unmount()
    wrapper = null
  }
})

describe('ScriptEditor 三页签外壳', () => {
  it('页签切换驱动列表数据源/新建入口：脚本↔脚本列表、函数库↔func 文件、模板↔模板短名', async () => {
    await mountView()

    // 初始脚本页签
    expect(tabs(wrapper).map((b) => b.text())).toEqual(['脚本', '函数库', '模板'])
    expect(tabs(wrapper)[0].classes()).toContain('active')
    expect(items(wrapper).map((i) => i.find('.ri-name').text())).toContain('verify_a.yml')
    expect(findBtn(wrapper, '＋ 新建脚本')).toBeTruthy()
    expect(findBtn(wrapper, '＋ 新建函数库')).toBeUndefined()
    expect(centerEmpty(wrapper).text()).toContain('从左侧选择脚本')

    // 函数库页签：列 func 文件（file 短路径 + 函数名清单），新建入口与空态区分
    await tabs(wrapper)[1].trigger('click')
    await flushPromises()
    expect(api.listFunctions).toHaveBeenCalledWith(PKG)
    expect(tabs(wrapper)[1].classes()).toContain('active')
    const fnItems = items(wrapper)
    expect(fnItems).toHaveLength(1)
    expect(fnItems[0].find('.ri-name').text()).toBe('common')
    expect(fnItems[0].find('.ri-meta').text()).toBe('f1')
    expect(findBtn(wrapper, '＋ 新建函数库')).toBeTruthy()
    expect(findBtn(wrapper, '＋ 新建脚本')).toBeUndefined()
    expect(centerEmpty(wrapper).text()).toContain('从左侧选择函数库文件')

    // 模板页签：管理形态（缩略图 + 短名列表，#区域元数据收进 title）；点击打开详情弹窗
    await tabs(wrapper)[2].trigger('click')
    await flushPromises()
    expect(api.listTemplates).toHaveBeenCalledWith(PKG)
    const tItems = items(wrapper)
    expect(tItems.map((i) => i.find('.ri-name').text())).toEqual(['开始挑战.png', 'plain.png'])
    expect(tItems[0].attributes('title')).toBe('开始挑战#757_909_857_971.png')
    expect(findBtn(wrapper, '新建模板')).toBeTruthy()
    await tItems[0].trigger('click')
    await flushPromises()
    const modal = wrapper.find('.tpl-modal')
    expect(modal.exists()).toBe(true)
    expect(modal.text()).toContain('开始挑战#757_909_857_971.png')
    await findBtn(wrapper, '前往投屏控制台').trigger('click')
    expect(routerPush).toHaveBeenCalledWith({ name: 'Console' })
    expect(centerEmpty(wrapper).text()).toContain('投屏控制台完成')
  })

  it('模板管理：重命名走 renameTemplate 并刷新列表，删除走 deleteTemplate 后列表为空', async () => {
    let tpls = [{ name: 'plain.png', pkg: PKG, size: 1024 }]
    api.listTemplates.mockImplementation(async () => tpls.map((t) => ({ ...t })))
    api.renameTemplate.mockImplementation(async (oldName, newName, pkg) => {
      tpls = tpls.map((t) => (t.name === oldName && t.pkg === pkg) ? { ...t, name: newName } : t)
      return { ok: true }
    })
    api.deleteTemplate.mockImplementation(async (name, pkg) => {
      tpls = tpls.filter((t) => !(t.name === name && t.pkg === pkg))
      return {}
    })
    await mountView()
    await tabs(wrapper)[2].trigger('click')
    await flushPromises()

    await items(wrapper)[0].trigger('click') // 打开详情
    await flushPromises()
    await wrapper.find('.tpl-modal input').setValue('renamed.png')
    await findBtn(wrapper, '重命名').trigger('click')
    await flushPromises()

    expect(api.renameTemplate).toHaveBeenCalledWith('plain.png', 'renamed.png', PKG)
    expect(items(wrapper)[0].find('.ri-name').text()).toBe('renamed.png')

    vi.spyOn(window, 'confirm').mockReturnValue(true)
    await items(wrapper)[0].find('.ri-del').trigger('click')
    await flushPromises()

    expect(api.deleteTemplate).toHaveBeenCalledWith('renamed.png', PKG)
    expect(wrapper.text()).toContain('该分区暂无模板')
  })

  it('模板新建：选文件转 base64 走 createTemplate 上传到当前分区', async () => {
    await mountView()
    await tabs(wrapper)[2].trigger('click')
    await flushPromises()

    api.createTemplate.mockResolvedValue({})
    const file = new File(['png-bytes'], 'shot.png', { type: 'image/png' })
    const input = wrapper.find('input[type="file"]')
    Object.defineProperty(input.element, 'files', { value: [file] })
    await input.trigger('change')
    // FileReader 回调挂在真实宏任务上，flushPromises（微任务）覆盖不到 → waitFor 轮询
    await vi.waitFor(() => expect(api.createTemplate).toHaveBeenCalledTimes(1))
    await flushPromises()

    expect(api.createTemplate).toHaveBeenCalledTimes(1)
    const [name, b64, pkg] = api.createTemplate.mock.calls[0]
    expect(name).toBe('shot.png')
    expect(b64).toBe(btoa('png-bytes'))
    expect(pkg).toBe(PKG)
  })

  it('模板图片替换：条目替换走 replaceTemplateImage，不复用新建接口', async () => {
    api.listTemplates.mockResolvedValue([{ name: 'plain.png', pkg: PKG, size: 10 }])
    api.replaceTemplateImage.mockResolvedValue({})
    await mountView()
    await tabs(wrapper)[2].trigger('click')
    await flushPromises()

    await items(wrapper)[0].find('.ri-action').trigger('click')
    const file = new File(['replacement'], 'replacement.png', { type: 'image/png' })
    const replaceInput = wrapper.findAll('input[type="file"]')[1]
    Object.defineProperty(replaceInput.element, 'files', { value: [file] })
    await replaceInput.trigger('change')
    await vi.waitFor(() => expect(api.replaceTemplateImage).toHaveBeenCalledTimes(1))

    expect(api.replaceTemplateImage).toHaveBeenCalledWith('plain.png', btoa('replacement'), PKG)
    expect(api.createTemplate).not.toHaveBeenCalled()
  })

  it('空态文案区分：三个页签各自的左栏空态互不串用', async () => {
    scriptsData.value = []
    api.listScripts.mockResolvedValue([])
    await mountView() // 无设备无脚本 → 无分区，函数库/模板列表为空

    expect(wrapper.text()).toContain('该分区暂无脚本')
    await tabs(wrapper)[1].trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('该分区暂无函数库文件')
    await tabs(wrapper)[2].trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('该分区暂无模板')
  })

  it('跨页签模型不外泄：脚本打开时切函数库页签，画布/保存/测试面板收起，切回仍在', async () => {
    api.getScript.mockResolvedValue({ ...SCRIPT, content: 'steps:\n  - log: 验证开始\n', version: 's1' })
    await mountView()

    await items(wrapper)[0].trigger('click') // 打开脚本
    await flushPromises()
    expect(wrapper.find('input.ed-name').element.value).toBe('verify_a.yml')
    expect(wrapper.find('.test-fn').exists()).toBe(false) // 脚本页签没有测试函数面板

    await tabs(wrapper)[1].trigger('click') // 切到函数库页签
    await flushPromises()
    expect(wrapper.find('input.ed-name').exists()).toBe(false) // 画布隐藏
    expect(wrapper.find('.test-fn').exists()).toBe(false)
    expect(findBtn(wrapper, '💾 保存').attributes('disabled')).toBeDefined()
    expect(wrapper.text()).not.toContain('未保存')
    expect(centerEmpty(wrapper).text()).toContain('从左侧选择函数库文件')

    // 切回脚本页签：模型仍在（不丢失、可继续编辑）
    await tabs(wrapper)[0].trigger('click')
    await flushPromises()
    expect(wrapper.find('input.ed-name').element.value).toBe('verify_a.yml')

    // 函数库页签打开函数文件 → FunctionLibraryModel 编辑上下文（函数下拉 + 测试面板）
    await tabs(wrapper)[1].trigger('click')
    api.getFunction.mockResolvedValue({ ...FUNC_FILE })
    await items(wrapper)[0].trigger('click')
    await flushPromises()
    expect(api.getFunction).toHaveBeenCalledWith(FUNC_ID)
    expect(wrapper.find('input.ed-name').element.value).toBe('common')
    expect(wrapper.find('.test-fn').exists()).toBe(true)
    const opts = wrapper.findAll('.test-fn select option')
    expect(opts.map((o) => o.text())).toEqual(['（画布当前函数）', 'f1'])
  })

  it('新建函数库：初始函数模型，保存走 createFunction 并刷新列表选中', async () => {
    await mountView()
    await tabs(wrapper)[1].trigger('click')
    vi.spyOn(window, 'prompt').mockReturnValue('helpers')
    await findBtn(wrapper, '＋ 新建函数库').trigger('click')
    await flushPromises()

    expect(wrapper.find('input.ed-name').element.value).toBe('helpers')
    const saveBtn = findBtn(wrapper, '💾 保存')
    expect(saveBtn.attributes('disabled')).toBeUndefined()
    const listCallsBeforeSave = api.listFunctions.mock.calls.length
    await saveBtn.trigger('click')
    await flushPromises()

    expect(api.createFunction).toHaveBeenCalledTimes(1)
    const payload = api.createFunction.mock.calls[0][0]
    expect(payload.pkg).toBe(PKG)
    expect(payload.name).toBe('helpers')
    expect(payload.content).toContain('func1')
    // 保存后刷新函数库列表（挂载期 watch(pkg)+applyPkg 会先各刷一次，此处只关心保存触发增量）
    expect(api.listFunctions.mock.calls.length).toBe(listCallsBeforeSave + 1)
    const sel = items(wrapper).find((i) => i.classes().includes('sel'))
    expect(sel?.find('.ri-name').text()).toBe('helpers')
  })

  it('测试函数：函数编辑上下文启用，经 functions run 接口发起单函数运行', async () => {
    api.getFunction.mockResolvedValue({ ...FUNC_FILE })
    await mountView()
    await tabs(wrapper)[1].trigger('click')
    await items(wrapper)[0].trigger('click')
    await flushPromises()

    const testBtn = findBtn(wrapper, '▶ 测试函数')
    expect(testBtn.attributes('disabled')).toBeDefined() // 未选设备禁用
    store.deviceId = 'dev1'
    await flushPromises()
    expect(testBtn.attributes('disabled')).toBeUndefined()

    api.runFunction.mockResolvedValue({ run_id: 'r1', state: 'running' })
    api.getRun.mockResolvedValue({ run_id: 'r1', device_id: 'dev1', script_id: FUNC_ID, state: 'success' })
    await testBtn.trigger('click')
    await flushPromises()
    await flushPromises()

    expect(api.runFunction).toHaveBeenCalledTimes(1)
    expect(api.runFunction.mock.calls[0][0]).toBe(FUNC_ID)
    expect(api.runFunction.mock.calls[0][1]).toBe('dev1')
    expect(api.runFunction.mock.calls[0][2]).toMatchObject({ function: 'f1', start_index: 0 })
    expect(api.getRun).toHaveBeenCalledWith('r1') // RunManager 统一实例轮询
    expect(store.running).toBe(false) // 终态复位
  })
})
