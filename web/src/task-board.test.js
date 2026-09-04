// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { mount, flushPromises } from '@vue/test-utils'
import TaskBoard from './components/TaskBoard.vue'
import { runConflicts, scriptsData, templatesData, devicesData, tasksData } from './store'

/**
 * TaskBoard（Console 任务页签）ADR-12 通用任务表单测试（P11.1 §6.6/§6.7）：
 * - 表单 = 名称/设备/触发方式(provider)/执行器(runner)/执行目标/参数/启用；
 * - 执行目标与参数由 RunnerEditorContribution 渲染（gamer.yaml：ScriptPicker + ParamsForm），
 *   TaskBoard 不 import 业务组件、不读 scriptsData/templatesData（源码边界断言）；
 * - 未知 runner / 未注册 provider 降级与只读保留；dependency_missing 呈现与恢复；
 * - 保存 body 为 ADR-12 嵌套形状（runner.payload / schedule.config）。
 */

const read = (p) => readFileSync(join(process.cwd(), 'src', p), 'utf8')

const SCRIPT_YAML = [
  'params:',
  "  - 'bool:enable:开关:true'",
  "  - 'time:timeout:最长等待:30s'",
  'config:',
  '  interval: 500ms',
  'steps:',
  '  - log: hi',
].join('\n')

const TMPL_SCRIPT_YAML = [
  'params:',
  "  - 'tmpl:account:账号模板'",
  'steps:',
  '  - find: $account',
].join('\n')

const SCRIPTS = [
  { id: 'com.demo/main.yml', package: 'com.demo', name: 'main.yml', content: SCRIPT_YAML },
]

// 服务端 ADR-12 任务 JSON（列表与详情同形状）
const TASKS = [
  {
    id: 't1', name: '每日签到', enabled: true, state: 'active',
    app: { device_id: 'dev1', android_package: 'com.demo', content_package: 'com.demo' },
    runner: { runner_id: 'gamer.yaml', entrypoint: 'com.demo/main.yml', payload: { args: { enable: false, timeout: '12s' } } },
    schedule: { provider_id: 'cron', config: { expression: '0 8 * * *' } },
    next_wakeup: '2026-09-06T00:00:00Z', last_result: '',
  },
  {
    id: 't2', name: '挂机', enabled: false, state: 'suspended', suspend_reason: 'disabled',
    app: { device_id: 'dev2', android_package: 'com.demo', content_package: 'com.demo' },
    runner: { runner_id: 'gamer.yaml', entrypoint: 'com.demo/main.yml', payload: { args: { timeout: '45s' } } },
    schedule: { provider_id: 'cron', config: { expression: '*/10 * * * *' } },
    last_result: '成功',
  },
]

function stubFetch(routes) {
  const calls = []
  vi.stubGlobal('fetch', vi.fn(async (url, opt = {}) => {
    const method = opt.method || 'GET'
    const body = opt.body ? JSON.parse(opt.body) : null
    calls.push({ url: String(url), method, body })
    const hit = routes.find(r => method === r.method && String(url).split('?')[0] === r.url)
    if (!hit) throw new Error(`unexpected fetch: ${method} ${url}`)
    const status = hit.status || 200
    return {
      ok: status < 400,
      status,
      headers: { get: k => (String(k).toLowerCase() === 'content-type' ? 'application/json' : null) },
      json: async () => (typeof hit.body === 'function' ? hit.body() : hit.body),
      blob: async () => new Blob(),
    }
  }))
  return calls
}

afterEach(() => {
  vi.unstubAllGlobals()
  // 全局 store 是模块级单例：清空，防用例间经 scriptsData 等短路贡献的资源拉取
  runConflicts.value.length = 0
  scriptsData.value = []
  templatesData.value = []
  devicesData.value = []
  tasksData.value = []
})

function baseRoutes() {
  return [
    { method: 'GET', url: '/api/apps/-/resources/scripts', body: SCRIPTS },
    { method: 'GET', url: '/api/apps/-/resources/scripts/com.demo%2Fmain.yml', body: SCRIPTS[0] },
    { method: 'GET', url: '/api/apps/-/resources/templates', body: [
      { name: '账号155#392_519_526_932.png', pkg: 'com.demo' },
    ] },
    { method: 'GET', url: '/api/devices', body: [
      { id: 'dev1', name: '设备一' }, { id: 'dev2', name: '设备二' },
    ] },
    { method: 'GET', url: '/api/runners', body: [
      { runner_id: 'gamer.yaml' },
    ] },
    { method: 'GET', url: '/api/schedule-providers', body: [
      { provider_id: 'cron' },
    ] },
    { method: 'GET', url: '/api/tasks', body: TASKS },
  ]
}

async function mountView(routes) {
  const calls = stubFetch(routes)
  const wrapper = mount(TaskBoard, {
    global: { stubs: { RunConflictModal: true, transition: true } },
    attachTo: document.body,
  })
  await flushPromises()
  return { wrapper, calls }
}

async function openEdit(wrapper, rowIndex = 0) {
  const row = wrapper.findAll('tbody tr')[rowIndex]
  await row.findAll('button').find(b => b.text().includes('✎')).trigger('click')
  await flushPromises()
}

function saveButton(wrapper) {
  return wrapper.findAll('.modal-foot .btn').find(b => b.text() === '保存')
}

describe('TaskBoard 边界：不感知脚本/模板资源与业务编辑组件（§6.6 验收）', () => {
  const source = read('components/TaskBoard.vue')
  for (const banned of ['ScriptPicker', 'ParamsForm', 'scriptsData', 'templatesData', 'listScripts', 'listTemplates']) {
    it(`不出现 ${banned}`, () => {
      expect(source).not.toContain(banned)
    })
  }
  it('挂载即拉取 runner/provider（执行器与触发方式下拉数据源）', async () => {
    const { calls } = await mountView(baseRoutes())
    expect(calls.some(c => c.method === 'GET' && c.url === '/api/runners')).toBe(true)
    expect(calls.some(c => c.method === 'GET' && c.url === '/api/schedule-providers')).toBe(true)
  })
})

describe('列表：ADR-12 行映射与状态呈现', () => {
  it('任务名/启用开关渲染，禁用任务置灰；行内不含 script_id/cron 平铺心智', async () => {
    const { wrapper } = await mountView(baseRoutes())
    const rows = wrapper.findAll('tbody tr')
    expect(rows[0].text()).toContain('每日签到')
    expect(rows[1].text()).toContain('挂机')
    expect(rows[1].find('input[type="checkbox"]').element.checked).toBe(false)
  })

  it('状态徽标：active=调度中 / suspended=已挂起 / cancelled=已取消', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/tasks').body = [
      { ...TASKS[0], state: 'active' },
      { ...TASKS[1], state: 'cancelled', suspend_reason: '' },
    ]
    const { wrapper } = await mountView(routes)
    const badges = wrapper.findAll('[data-testid="state-badge"]')
    expect(badges[0].text()).toBe('调度中')
    expect(badges[1].text()).toBe('已取消')
  })

  it('dependency_missing：徽标 + missing_dependency 提示 + 恢复动作（POST resume）', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/tasks').body = [{
      ...TASKS[0],
      state: 'dependency_missing',
      suspend_reason: 'missing_dependency=gamer.yaml',
    }]
    routes.push({ method: 'POST', url: '/api/tasks/t1/resume', body: { ...TASKS[0], state: 'active', suspend_reason: '' } })
    const { wrapper, calls } = await mountView(routes)
    expect(wrapper.find('[data-testid="state-badge"]').text()).toContain('依赖缺失')
    expect(wrapper.find('[data-testid="dep-hint"]').text()).toContain('缺少依赖：gamer.yaml')
    const row = wrapper.findAll('tbody tr')[0]
    await row.findAll('button').find(b => b.attributes('title')?.includes('恢复调度')).trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'POST' && c.url === '/api/tasks/t1/resume')).toBe(true)
  })
})

describe('行内动作：挂起 / 取消调度 / 删除 / 启停 / 测试', () => {
  it('挂起（⏸）：POST suspend 携带 reason', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'POST', url: '/api/tasks/t1/suspend', body: TASKS[0] })
    const { wrapper, calls } = await mountView(routes)
    const row = wrapper.findAll('tbody tr')[0]
    await row.findAll('button').find(b => b.attributes('title')?.includes('挂起调度')).trigger('click')
    await flushPromises()
    expect(calls.find(c => c.method === 'POST' && c.url === '/api/tasks/t1/suspend'))
      .toMatchObject({ body: { reason: 'suspended' } })
  })

  it('取消调度（✕）：confirm 后 POST cancel；confirmed 取消不触发', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'POST', url: '/api/tasks/t1/cancel', body: TASKS[0] })
    const { wrapper, calls } = await mountView(routes)
    vi.stubGlobal('confirm', () => true)
    const row = wrapper.findAll('tbody tr')[0]
    await row.findAll('button').find(b => b.attributes('title')?.includes('取消调度')).trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'POST' && c.url === '/api/tasks/t1/cancel')).toBe(true)

    vi.stubGlobal('confirm', () => false)
    await row.findAll('button').find(b => b.attributes('title')?.includes('取消调度')).trigger('click')
    await flushPromises()
    expect(calls.filter(c => c.method === 'POST' && c.url === '/api/tasks/t1/cancel')).toHaveLength(1)
  })

  it('删除：confirm 后 DELETE 并从列表移除；confirm 取消则不发请求', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'DELETE', url: '/api/tasks/t1', body: {} })
    const { wrapper, calls } = await mountView(routes)
    vi.stubGlobal('confirm', () => true)
    const row = wrapper.findAll('tbody tr')[0]
    await row.findAll('button').find(b => b.attributes('title')?.includes('删除任务')).trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'DELETE' && c.url === '/api/tasks/t1')).toBe(true)
    expect(wrapper.findAll('tbody tr')[0].text()).toContain('挂机')

    vi.stubGlobal('confirm', () => false)
    await row.findAll('button').find(b => b.attributes('title')?.includes('删除任务')).trigger('click')
    await flushPromises()
    expect(calls.filter(c => c.method === 'DELETE')).toHaveLength(1)
  })

  it('启停：开关走 enable/disable 显式状态迁移端点', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'POST', url: '/api/tasks/t1/disable', body: { ok: true } })
    routes.push({ method: 'POST', url: '/api/tasks/t2/enable', body: { ok: true } })
    const { wrapper, calls } = await mountView(routes)
    const rows = wrapper.findAll('tbody tr')
    await rows[0].find('input[type="checkbox"]').setValue(false)
    await flushPromises()
    expect(calls.some(c => c.method === 'POST' && c.url === '/api/tasks/t1/disable')).toBe(true)
    await rows[1].find('input[type="checkbox"]').setValue(true)
    await flushPromises()
    expect(calls.some(c => c.method === 'POST' && c.url === '/api/tasks/t2/enable')).toBe(true)
  })

  it('测试运行：按任务 id 触发，只消费 run_id 响应；424 依赖缺失后按钮恢复可重试', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'POST', url: '/api/tasks/t2/run', status: 424, body: { code: 'dependency_unavailable', message: 'runner unavailable', task_id: 't2' } })
    const { wrapper, calls } = await mountView(routes)
    const row = wrapper.findAll('tbody tr')[1]
    await row.findAll('button').find(b => b.text().includes('测试')).trigger('click')
    await flushPromises()
    expect(calls.find(c => c.method === 'POST' && c.url === '/api/tasks/t2/run')).toMatchObject({ body: null })
    expect(row.findAll('button').find(b => b.text().includes('测试')).attributes('disabled')).toBeUndefined()

    routes.push({ method: 'POST', url: '/api/tasks/t2/run', body: { run_id: 'task-run-9', state: 'queued' } })
    await row.findAll('button').find(b => b.text().includes('测试')).trigger('click')
    await flushPromises()
    expect(calls.filter(c => c.method === 'POST' && c.url === '/api/tasks/t2/run')).toHaveLength(2)
  })

  it('测试运行遇 409 device_busy：推入冲突队列（RunConflictModal 数据源）', async () => {
    const routes = baseRoutes()
    routes.push({
      method: 'POST', url: '/api/tasks/t2/run', status: 409,
      body: { error: 'device_busy', run_id: 'r1', script_id: 'com.demo/main.yml', source: 'manual', started_at: '2026-09-05T00:00:00Z' },
    })
    const { wrapper } = await mountView(routes)
    const row = wrapper.findAll('tbody tr')[1]
    await row.findAll('button').find(b => b.text().includes('测试')).trigger('click')
    await flushPromises()
    expect(runConflicts.value).toHaveLength(1)
    expect(runConflicts.value[0]).toMatchObject({ device_id: 'dev2', error: 'device_busy' })
  })
})

describe('新建任务：贡献渲染 + cron 触发方式 + 保存 ADR-12 body', () => {
  async function openAdd(wrapper) {
    await wrapper.findAll('button').find(b => b.text().includes('新建任务')).trigger('click')
    await flushPromises()
  }

  it('表单骨架：provider=cron 渲染表达式与预设；执行器显示贡献 title；执行目标为 ScriptPicker', async () => {
    const { wrapper } = await mountView(baseRoutes())
    await openAdd(wrapper)
    expect(wrapper.find('[data-testid="provider-select"]').element.value).toBe('cron')
    expect(wrapper.find('[data-testid="cron-input"]').exists()).toBe(true)
    expect(wrapper.find('[data-testid="config-json"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="manual-provider-input"]').exists()).toBe(false)
    // 执行器下拉显示贡献 title；选中 gamer.yaml 后渲染执行目标与参数区
    const runnerSelect = wrapper.find('[data-testid="runner-select"]')
    expect(runnerSelect.findAll('option').map(o => o.text())).toContain('YAML 脚本')
    expect(runnerSelect.element.value).toBe('gamer.yaml')
    expect(wrapper.find('.spicker').exists()).toBe(true) // ScriptPicker（经贡献）
    // 未选执行目标前：参数区提示先选目标
    expect(wrapper.find('[data-testid="gy-pick-first"]').exists()).toBe(true)
  })

  it('cron 预设 chips 写入 expression；选脚本后按 params 声明渲染参数表单', async () => {
    const { wrapper } = await mountView(baseRoutes())
    await openAdd(wrapper)
    const chips = wrapper.findAll('.cron-presets .cp-item')
    await chips.find(b => b.text() === '每天 21:00').trigger('click')
    expect(wrapper.find('[data-testid="cron-input"]').element.value).toBe('0 21 * * *')
    // 选脚本（ScriptPicker 未锁分区：分区自动跟随唯一分区，脚本下拉直接可挑）
    await wrapper.find('.sp-name').setValue('com.demo/main.yml')
    await flushPromises()
    const form = wrapper.find('[data-testid="params-form"]')
    expect(form.exists()).toBe(true)
    expect(form.findAll('.pf-row')).toHaveLength(2)
  })

  it('tmpl 参数的模板候选 = 脚本分区模板短名（候选逻辑自 TaskBoard 迁入贡献）', async () => {
    const routes = baseRoutes()
    const tmpl = { id: 'com.demo/main.yml', package: 'com.demo', name: 'main.yml', content: TMPL_SCRIPT_YAML }
    routes.find(r => r.url === '/api/apps/-/resources/scripts').body = [tmpl]
    routes.find(r => r.url === '/api/apps/-/resources/scripts/com.demo%2Fmain.yml').body = tmpl
    const { wrapper } = await mountView(routes)
    await openAdd(wrapper)
    await wrapper.find('.sp-name').setValue('com.demo/main.yml')
    await flushPromises()
    const account = wrapper.find('[data-testid="params-form"] .pf-row')
    expect(account.text()).toContain('$account')
    await account.find('.tpl-toggle').trigger('click')
    expect(account.findAll('.tpl-drop-row').map(row => row.text())).toContain('账号155.png')
  })

  it('保存：默认值字段省略进 payload.args（稀疏），app 包名由贡献按 entrypoint 前缀推导', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'POST', url: '/api/tasks', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await openAdd(wrapper)
    await wrapper.find('.modal-body input.input').setValue('新任务')
    await wrapper.find('.sp-name').setValue('com.demo/main.yml')
    await flushPromises()
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    const posted = calls.find(c => c.method === 'POST' && c.url === '/api/tasks')
    expect(posted.body).toMatchObject({
      name: '新任务',
      enabled: true,
      app: { device_id: 'dev1', android_package: 'com.demo', content_package: 'com.demo' },
      runner: { runner_id: 'gamer.yaml', entrypoint: 'com.demo/main.yml' },
      schedule: { provider_id: 'cron', config: { expression: '0 8 * * *' } },
    })
    expect(posted.body.runner.payload.args).toEqual({})
    expect(posted.body.id).toBeUndefined()
  })

  it('弹窗内「启用」开关随保存提交；未选执行目标/未填名称被客户端阻断', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'POST', url: '/api/tasks', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await openAdd(wrapper)
    // 未填名称：阻断
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'POST')).toBe(false)
    // 只填名称、未选执行目标：仍阻断
    await wrapper.find('.modal-body input.input').setValue('新任务')
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'POST')).toBe(false)
    // 补齐：关掉启用开关 → body.enabled=false
    await wrapper.find('[data-testid="enabled-switch"]').setValue(false)
    await wrapper.find('.sp-name').setValue('com.demo/main.yml')
    await flushPromises()
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    const posted = calls.find(c => c.method === 'POST' && c.url === '/api/tasks')
    expect(posted.body.enabled).toBe(false)
  })
})

describe('编辑任务：payload.args 采用与保存形状', () => {
  it('按列表行回填（无详情请求）；参数表单带入快照覆盖态；保存 PUT 为 ADR-12 形状', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'PUT', url: '/api/tasks/t1', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await openEdit(wrapper, 0)
    // 表单按脚本 params 渲染（内容经 GET /api/scripts/:id 获取——贡献内部事务）
    expect(calls.some(c => c.method === 'GET' && c.url === '/api/apps/-/resources/scripts/com.demo%2Fmain.yml')).toBe(true)
    const form = wrapper.find('[data-testid="params-form"]')
    expect(form.exists()).toBe(true)
    expect(form.findAll('.pf-row')).toHaveLength(2)
    // 快照 args（enable=false/timeout=12s）带入覆盖态
    const overrides = form.findAll('input[type="checkbox"]').filter(c => c.attributes('aria-label')?.includes('覆盖默认值'))
    expect(overrides.map(c => c.element.checked)).toEqual([true, true])

    await saveButton(wrapper).trigger('click')
    await flushPromises()
    const put = calls.find(c => c.method === 'PUT' && c.url === '/api/tasks/t1')
    expect(put.body).toMatchObject({
      id: 't1', name: '每日签到',
      app: { device_id: 'dev1', android_package: 'com.demo', content_package: 'com.demo' },
      runner: {
        runner_id: 'gamer.yaml',
        entrypoint: 'com.demo/main.yml',
        payload: { args: { enable: false, timeout: '12s' } },
      },
      schedule: { provider_id: 'cron', config: { expression: '0 8 * * *' } },
    })
    expect(wrapper.find('[data-testid="params-form"]').exists()).toBe(false)
  })

  it('切换执行目标后原 payload 不再适用：保存时 args 清空', async () => {
    const routes = baseRoutes()
    const second = { id: 'com.demo/other.yml', package: 'com.demo', name: 'other.yml', content: SCRIPT_YAML }
    routes.find(r => r.url === '/api/apps/-/resources/scripts').body = [...SCRIPTS, second]
    routes.push({ method: 'GET', url: '/api/apps/-/resources/scripts/com.demo%2Fother.yml', body: second })
    routes.push({ method: 'PUT', url: '/api/tasks/t1', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await openEdit(wrapper, 0)
    await wrapper.find('.sp-name').setValue('com.demo/other.yml')
    await flushPromises()
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    const put = calls.find(c => c.method === 'PUT' && c.url === '/api/tasks/t1')
    expect(put.body.runner.entrypoint).toBe('com.demo/other.yml')
    expect(put.body.runner.payload.args).toEqual({})
  })
})

describe('未知/未注册 runner：占位 + 只读 JSON，其他字段可改、任务仍可保存', () => {
  const UNKNOWN_TASK = {
    id: 't3', name: '三方宏任务', enabled: true, state: 'dependency_missing',
    suspend_reason: 'missing_dependency=thirdparty.macro',
    app: { device_id: 'dev3', android_package: 'com.other', content_package: 'com.other' },
    runner: { runner_id: 'thirdparty.macro', entrypoint: 'macro://boot', payload: { steps: 3 } },
    schedule: { provider_id: 'cron', config: { expression: '0 9 * * *' } },
  }

  it('后端返回未注册 runner：占位提示 + runner JSON 只读展示，保存原样保留 runner', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/tasks').body = [UNKNOWN_TASK]
    routes.push({ method: 'PUT', url: '/api/tasks/t3', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await openEdit(wrapper, 0)
    expect(wrapper.find('[data-testid="runner-missing-placeholder"]').text())
      .toContain('未知执行器')
    expect(wrapper.find('[data-testid="runner-json"]').text()).toContain('macro://boot')
    expect(wrapper.find('[data-testid="runner-json"]').text()).toContain('thirdparty.macro')
    // runner 下拉保留当前值（原文名显示），执行目标/参数无编辑器
    expect(wrapper.find('[data-testid="runner-select"]').element.value).toBe('thirdparty.macro')
    // 其他字段可改
    await wrapper.find('.modal-body input.input').setValue('改名后的宏任务')
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    const put = calls.find(c => c.method === 'PUT' && c.url === '/api/tasks/t3')
    expect(put.body).toMatchObject({
      id: 't3', name: '改名后的宏任务',
      app: { device_id: 'dev3', android_package: 'com.other', content_package: 'com.other' },
      runner: { runner_id: 'thirdparty.macro', entrypoint: 'macro://boot', payload: { steps: 3 } },
      schedule: { provider_id: 'cron', config: { expression: '0 9 * * *' } },
    })
  })

  it('已注册但无贡献的 runner：占位「未提供编辑器」，保存保留 entrypoint/payload', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/runners').body = [{ runner_id: 'gamer.yaml' }, { runner_id: 'gamer.macro' }]
    routes.push({ method: 'PUT', url: '/api/tasks/t1', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await openEdit(wrapper, 0)
    // 切到无贡献的已注册 runner（下拉显示 runner_id 原文）
    const option = wrapper.find('[data-testid="runner-select"]').findAll('option')
      .find(o => o.element.value === 'gamer.macro')
    expect(option.text()).toBe('gamer.macro')
    await wrapper.find('[data-testid="runner-select"]').setValue('gamer.macro')
    await flushPromises()
    expect(wrapper.find('[data-testid="runner-missing-placeholder"]').text())
      .toContain('该执行器未提供编辑器（未安装对应扩展）')
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    const put = calls.find(c => c.method === 'PUT' && c.url === '/api/tasks/t1')
    expect(put.body.runner).toEqual({
      runner_id: 'gamer.macro',
      entrypoint: 'com.demo/main.yml',
      payload: { args: { enable: false, timeout: '12s' } },
    })
  })
})

describe('触发方式降级与通用 provider config', () => {
  it('provider 列表为空：降级手填 provider_id（默认 cron）+ 表达式，保存走 cron config', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/schedule-providers').body = []
    routes.push({ method: 'POST', url: '/api/tasks', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await wrapper.findAll('button').find(b => b.text().includes('新建任务')).trigger('click')
    await flushPromises()
    const manual = wrapper.find('[data-testid="manual-provider-input"]')
    expect(manual.exists()).toBe(true)
    expect(manual.element.value).toBe('cron')
    expect(wrapper.find('[data-testid="cron-input"]').exists()).toBe(true)
    await wrapper.find('.modal-body input.input').setValue('新任务')
    await wrapper.find('.sp-name').setValue('com.demo/main.yml')
    await flushPromises()
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    expect(calls.find(c => c.method === 'POST' && c.url === '/api/tasks').body.schedule)
      .toEqual({ provider_id: 'cron', config: { expression: '0 8 * * *' } })
  })

  it('已注册非 cron provider：渲染 config JSON 输入，保存按 JSON 解析', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/schedule-providers').body = [{ provider_id: 'cron' }, { provider_id: 'interval' }]
    routes.push({ method: 'POST', url: '/api/tasks', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await wrapper.findAll('button').find(b => b.text().includes('新建任务')).trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="provider-select"]').setValue('interval')
    expect(wrapper.find('[data-testid="cron-input"]').exists()).toBe(false)
    const config = wrapper.find('[data-testid="config-json"]')
    expect(config.exists()).toBe(true)
    await config.setValue('{"every": "10m"}')
    await wrapper.find('.modal-body input.input').setValue('每10分钟')
    await wrapper.find('.sp-name').setValue('com.demo/main.yml')
    await flushPromises()
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    expect(calls.find(c => c.method === 'POST' && c.url === '/api/tasks').body.schedule)
      .toEqual({ provider_id: 'interval', config: { every: '10m' } })
  })

  it('任务携带未注册 provider：编辑回填手填框与 config JSON，保存原样保留', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/tasks').body = [{
      ...TASKS[0],
      schedule: { provider_id: 'thirdparty.calendar', config: { tz: 'Asia/Shanghai' } },
    }]
    routes.push({ method: 'PUT', url: '/api/tasks/t1', body: {} })
    const { wrapper, calls } = await mountView(routes)
    await openEdit(wrapper, 0)
    expect(wrapper.find('[data-testid="manual-provider-input"]').element.value).toBe('thirdparty.calendar')
    expect(JSON.parse(wrapper.find('[data-testid="config-json"]').element.value))
      .toEqual({ tz: 'Asia/Shanghai' })
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    expect(calls.find(c => c.method === 'PUT' && c.url === '/api/tasks/t1').body.schedule)
      .toEqual({ provider_id: 'thirdparty.calendar', config: { tz: 'Asia/Shanghai' } })
  })

  it('config JSON 非法时保存被客户端阻断', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/schedule-providers').body = [{ provider_id: 'interval' }]
    const { wrapper, calls } = await mountView(routes)
    await wrapper.findAll('button').find(b => b.text().includes('新建任务')).trigger('click')
    await flushPromises()
    await wrapper.find('[data-testid="provider-select"]').setValue('interval')
    await wrapper.find('[data-testid="config-json"]').setValue('{oops')
    await wrapper.find('.modal-body input.input').setValue('坏配置')
    await wrapper.find('.sp-name').setValue('com.demo/main.yml')
    await flushPromises()
    await saveButton(wrapper).trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'POST')).toBe(false)
    expect(wrapper.find('.modal-body').exists()).toBe(true)
  })
})

describe('服务端时区标识（P11.1 后任务时间戳均为 UTC 串，常显兜底文案）', () => {
  it('无任务 → 兜底文案', async () => {
    const routes = baseRoutes()
    routes.find(r => r.url === '/api/tasks').body = []
    const { wrapper } = await mountView(routes)
    expect(wrapper.find('[data-testid="server-tz-hint"]').text())
      .toContain('任务按服务端本地时区执行（Docker 部署可用 TZ 配置）')
  })

  it('有任务同样显示兜底文案（next_wakeup 为 UTC 串不携带本地偏移）', async () => {
    const { wrapper } = await mountView(baseRoutes())
    expect(wrapper.find('[data-testid="server-tz-hint"]').text())
      .toContain('任务按服务端本地时区执行（Docker 部署可用 TZ 配置）')
  })
})
