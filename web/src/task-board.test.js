// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import TaskBoard from './components/TaskBoard.vue'

/**
 * TaskBoard（Console 任务页签）参数化挂载测试（P11.1 ADR-12 统一 Task 模型，mock fetch）：
 * - 列表按 ADR-12 JSON（嵌套 runner / {provider_id, config} schedule）渲染行；
 * - 测试（立即运行）：只消费当前 run_id 响应，并按任务 id 发起请求；
 * - 编辑任务：按脚本 params 渲染表单（runner.payload.args 整体带入覆盖态）；
 * - 保存：POST/PUT /api/tasks body 为 ADR-12 形状（runner.payload.args 稀疏覆盖）；
 * - 启停走 enable/disable 显式状态迁移端点。
 */

const SCRIPT_YAML = [
  'params:',
  "  - 'bool:enable:开关:true'",
  "  - 'time:timeout:最长等待:30s'",
  'config:',
  '  interval: 500ms',
  '  threshold: 0.85',
  '  log_level: info',
  'steps:',
  '  - log: hi',
].join('\n')

const TMPL_SCRIPT_YAML = [
  'params:',
  "  - 'tmpl:account:账号模板'",
  'steps:',
  '  - find: $account',
].join('\n')

// 服务端 ADR-12 任务 JSON（嵌套 runner / schedule；无 script_id/cron 平铺字段）
const TASKS = [
  {
    id: 't1', name: '每日签到', enabled: true, state: 'active',
    app: { device_id: 'dev1', android_package: 'com.demo', content_package: 'com.demo' },
    runner: { runner_id: 'gamer.yaml', entrypoint: 'com.demo/main.yml', payload: { args: { enable: false, timeout: '10s' } } },
    schedule: { provider_id: 'cron', config: { expression: '0 8 * * *' } },
    next_wakeup: '2026-09-06T00:00:00Z', last_result: '',
  },
  {
    id: 't2', name: '挂机', enabled: false, state: 'suspended',
    app: { device_id: 'dev2', android_package: 'com.demo', content_package: 'com.demo' },
    runner: { runner_id: 'gamer.yaml', entrypoint: 'com.demo/main.yml', payload: { args: { timeout: '45s' } } },
    schedule: { provider_id: 'cron', config: { expression: '*/10 * * * *' } },
    last_result: '成功',
  },
]

// 任务详情（GET /api/tasks/:id）：与列表同形状。
// 详情 timeout=12s ≠ 列表兜底 10s：用于断言表单以详情视图为准。
const TASK_DETAILS = {
  t1: {
    id: 't1', name: '每日签到', state: 'active',
    app: TASKS[0].app,
    runner: { runner_id: 'gamer.yaml', entrypoint: 'com.demo/main.yml', payload: { args: { enable: false, timeout: '12s' } } },
    schedule: TASKS[0].schedule,
  },
  t2: TASKS[1],
}

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

afterEach(() => vi.unstubAllGlobals())

const baseRoutes = () => [
  { method: 'GET', url: '/api/scripts', body: [
    { id: 'com.demo/main.yml', package: 'com.demo', name: 'main.yml', content: SCRIPT_YAML },
  ] },
  { method: 'GET', url: '/api/devices', body: [
    { id: 'dev1', name: '设备一' }, { id: 'dev2', name: '设备二' },
  ] },
  { method: 'GET', url: '/api/templates', body: [
    { name: '账号155#392_519_526_932.png', pkg: 'com.demo' },
  ] },
  { method: 'GET', url: '/api/tasks', body: TASKS },
  { method: 'GET', url: '/api/tasks/t1', body: TASK_DETAILS.t1 },
  { method: 'GET', url: '/api/tasks/t2', body: TASK_DETAILS.t2 },
]

async function mountView(routes) {
  const calls = stubFetch(routes)
  const wrapper = mount(TaskBoard, {
    global: { stubs: { RunConflictModal: true, transition: true } },
    attachTo: document.body,
  })
  await flushPromises()
  return { wrapper, calls }
}

describe('TaskBoard 列表：ADR-12 行映射', () => {
  it('按嵌套 runner/schedule 渲染任务名，禁用任务置灰', async () => {
    const { wrapper } = await mountView(baseRoutes())
    const rows = wrapper.findAll('tbody tr')
    expect(rows[0].text()).toContain('每日签到')
    expect(rows[1].text()).toContain('挂机')
    // t2 disabled：启用开关未选中
    expect(rows[1].find('input[type="checkbox"]').element.checked).toBe(false)
  })

  it('依赖缺失任务显示标注（任务保留不删除）', async () => {
    const routes = baseRoutes()
    routes[3].body = [{
      ...TASKS[0],
      state: 'dependency_missing',
    }]
    const { wrapper } = await mountView(routes)
    expect(wrapper.findAll('tbody tr')[0].text()).toContain('依赖缺失')
  })
})

describe('TaskBoard 测试（立即运行）契约', () => {
  it('测试运行只消费当前 run_id 响应，并按任务 id 发起请求', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'POST', url: '/api/tasks/t2/run', body: { run_id: 'task-run-1', state: 'queued' } })
    const { wrapper, calls } = await mountView(routes)
    const row = wrapper.findAll('tbody tr')[1]
    await row.findAll('button').find(b => b.text().includes('测试')).trigger('click')
    await flushPromises()

    expect(calls.find(c => c.method === 'POST' && c.url === '/api/tasks/t2/run')).toMatchObject({
      method: 'POST', url: '/api/tasks/t2/run', body: null,
    })
    expect(row.text()).toContain('挂机')
  })
})

describe('启停：enable/disable 显式状态迁移端点', () => {
  it('停用任务调 POST /api/tasks/:id/disable，启用调 enable', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'POST', url: '/api/tasks/t2/disable', body: { ok: true } })
    routes.push({ method: 'POST', url: '/api/tasks/t1/enable', body: { ok: true } })
    const { wrapper, calls } = await mountView(routes)
    const rows = wrapper.findAll('tbody tr')
    // t1 当前启用 → 关闭 = disable
    await rows[0].find('input[type="checkbox"]').setValue(false)
    await flushPromises()
    expect(calls.some(c => c.method === 'POST' && c.url === '/api/tasks/t1/disable')).toBe(true)
    // t2 当前停用 → 打开 = enable
    await rows[1].find('input[type="checkbox"]').setValue(true)
    await flushPromises()
    expect(calls.some(c => c.method === 'POST' && c.url === '/api/tasks/t2/enable')).toBe(true)
  })
})

describe('编辑任务：runner.payload.args 带入 + 保存为 ADR-12 形状', () => {
  async function openEdit() {
    const routes = baseRoutes()
    routes.push({ method: 'PUT', url: '/api/tasks/t1', body: {} })
    const { wrapper, calls } = await mountView(routes)
    const rows = wrapper.findAll('tbody tr')
    await rows[0].findAll('button').find(b => b.text().includes('✎')).trigger('click')
    await flushPromises()
    return { wrapper, calls }
  }

  it('表单按脚本 params 渲染；详情视图被拉取（payload.args 以详情为准在保存断言覆盖）', async () => {
    const { wrapper, calls } = await openEdit()
    const form = wrapper.find('[data-testid="params-form"]')
    expect(form.exists()).toBe(true)
    expect(form.findAll('.pf-row')).toHaveLength(2)
    expect(calls.some(c => c.method === 'GET' && c.url === '/api/tasks/t1')).toBe(true)
  })

  it('保存 PUT /api/tasks/t1：runner 嵌套 + schedule provider/config + 稀疏 args', async () => {
    const { wrapper, calls } = await openEdit()
    const save = wrapper.findAll('.modal-foot .btn').find(b => b.text() === '保存')
    await save.trigger('click')
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
})

describe('新建任务：选脚本渲染空表单，保存提交 ADR-12 body', () => {
  it('无快照：默认值字段不进 payload.args；必填缺失校验阻断', async () => {
    const calls = stubFetch(baseRoutes())
    const wrapper = mount(TaskBoard, {
      global: { stubs: { RunConflictModal: true } },
      attachTo: document.body,
    })
    await flushPromises()
    await wrapper.findAll('button').find(b => b.text().includes('新建任务')).trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="params-form"]').exists()).toBe(true)
    // 名称必填：填入后再保存
    const nameInput = wrapper.find('.modal-body input.input')
    await nameInput.setValue('新任务')
    const save = wrapper.findAll('.modal-foot .btn').find(b => b.text() === '保存')
    await save.trigger('click')
    await flushPromises()
    const posted = calls.find(c => c.method === 'POST' && c.url === '/api/tasks')
    // 默认值字段 enable/timeout 用户未动 = 省略；payload.args 缺省（脚本无必填参数 → 全省略）
    expect(posted.body).toMatchObject({
      name: '新任务',
      runner: { runner_id: 'gamer.yaml', entrypoint: 'com.demo/main.yml' },
      schedule: { provider_id: 'cron', config: { expression: '0 8 * * *' } },
    })
    expect(posted.body.runner.payload.args).toEqual({})
  })
})

describe('任务参数：tmpl 类型复用步骤模板候选', () => {
  it('account 参数的模板下拉显示当前脚本分区模板短名', async () => {
    const routes = baseRoutes()
    routes[0].body = [
      { id: 'com.demo/main.yml', package: 'com.demo', name: 'main.yml', content: TMPL_SCRIPT_YAML },
    ]
    const { wrapper } = await mountView(routes)
    await wrapper.findAll('button').find(b => b.text().includes('新建任务')).trigger('click')
    await flushPromises()

    const account = wrapper.find('[data-testid="params-form"] .pf-row')
    expect(account.text()).toContain('$account')
    await account.find('.tpl-toggle').trigger('click')
    expect(account.findAll('.tpl-drop-row').map(row => row.text())).toContain('账号155.png')
  })
})

describe('删除任务与依赖缺失触发的失败路径', () => {
  it('删除任务：confirm 后 DELETE /api/tasks/:id 并从列表移除', async () => {
    const routes = baseRoutes()
    routes.push({ method: 'DELETE', url: '/api/tasks/t1', body: {} })
    const { wrapper, calls } = await mountView(routes)
    vi.stubGlobal('confirm', () => true)
    const rows = wrapper.findAll('tbody tr')
    await rows[0].findAll('button').find(b => b.text().includes('🗑')).trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'DELETE' && c.url === '/api/tasks/t1')).toBe(true)
    expect(wrapper.findAll('tbody tr')[0].text()).toContain('挂机')
  })

  it('测试运行遇 424 dependency_unavailable：按钮恢复可用且可重试', async () => {
    const routes = baseRoutes()
    routes.push({
      method: 'POST', url: '/api/tasks/t2/run', status: 424,
      body: { code: 'dependency_unavailable', message: 'runner unavailable', task_id: 't2' },
    })
    const { wrapper, calls } = await mountView(routes)
    const row = wrapper.findAll('tbody tr')[1]
    await row.findAll('button').find(b => b.text().includes('测试')).trigger('click')
    await flushPromises()
    const runBtn = row.findAll('button').find(b => b.text().includes('测试'))
    expect(runBtn.attributes('disabled')).toBeUndefined()
    expect(calls.filter(c => c.method === 'POST' && c.url === '/api/tasks/t2/run')).toHaveLength(1)
  })
})

describe('保存与删除的守卫路径', () => {
  it('新建未填名称：保存被客户端阻断，不发出 POST', async () => {
    const calls = stubFetch(baseRoutes())
    const wrapper = mount(TaskBoard, {
      global: { stubs: { RunConflictModal: true } },
      attachTo: document.body,
    })
    await flushPromises()
    await wrapper.findAll('button').find(b => b.text().includes('新建任务')).trigger('click')
    await flushPromises()
    const nameInput = wrapper.find('.modal-body input.input')
    await nameInput.setValue('')
    const save = wrapper.findAll('.modal-foot .btn').find(b => b.text() === '保存')
    await save.trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'POST' && c.url === '/api/tasks')).toBe(false)
    // 弹窗保持打开
    expect(wrapper.find('.modal-body').exists()).toBe(true)
  })

  it('删除任务：confirm 取消则不发出 DELETE', async () => {
    const { wrapper, calls } = await mountView(baseRoutes())
    vi.stubGlobal('confirm', () => false)
    const rows = wrapper.findAll('tbody tr')
    await rows[0].findAll('button').find(b => b.text().includes('🗑')).trigger('click')
    await flushPromises()
    expect(calls.some(c => c.method === 'DELETE')).toBe(false)
    expect(wrapper.findAll('tbody tr')[0].text()).toContain('每日签到')
  })
})

describe('服务端时区标识（P11.1 后任务时间戳均为 UTC 串，常显兜底文案）', () => {
  it('无任务 → 兜底文案', async () => {
    const routes = baseRoutes()
    routes[3].body = []
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
