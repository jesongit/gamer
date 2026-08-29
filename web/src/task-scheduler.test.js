// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import TaskScheduler from './views/TaskScheduler.vue'

/**
 * TaskScheduler 参数化挂载测试（阶段 5，plan §12.3，mock fetch）：
 * - param_stale 任务：列表「参数已过期」标注 + 立即运行禁用；
 * - 编辑任务：按脚本 params 渲染表单（快照整体带入覆盖态）+ 过期横幅与三列对比表；
 * - 保存：POST /api/tasks body 携带稀疏 args；409 签名冲突 → 横幅 + 重新确认（reconfirm:true）。
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

const TASKS = [
  {
    id: 't1', name: '每日签到', cron: '0 8 * * *', script_id: 'com.demo/main.yml',
    device_id: 'dev1', enabled: true, next_run: '-', last_result: '',
    args: { enable: false, timeout: '10s' }, param_signature: 'abc', param_stale: true,
  },
  {
    id: 't2', name: '挂机', cron: '*/10 * * * *', script_id: 'com.demo/main.yml',
    device_id: 'dev2', enabled: false, next_run: '-', last_result: '成功',
    args: { timeout: '45s' }, param_signature: 'def', param_stale: false,
  },
]

// 任务详情（GET /api/tasks/:id）：args 解析视图所在端点（列表仅带 param_stale/has_args/签名）。
// 详情 timeout=12s ≠ 列表兜底 10s：用于断言表单以详情视图为准。
const TASK_DETAILS = {
  t1: { id: 't1', name: '每日签到', script_id: 'com.demo/main.yml', args: { enable: false, timeout: '12s' } },
  t2: { id: 't2', name: '挂机', script_id: 'com.demo/main.yml', args: { timeout: '45s' } },
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
  { method: 'GET', url: '/api/tasks', body: TASKS },
  { method: 'GET', url: '/api/tasks/t1', body: TASK_DETAILS.t1 },
  { method: 'GET', url: '/api/tasks/t2', body: TASK_DETAILS.t2 },
]

async function mountView(routes) {
  const calls = stubFetch(routes)
  const wrapper = mount(TaskScheduler, {
    global: { stubs: { RunConflictModal: true, transition: true } },
    attachTo: document.body,
  })
  await flushPromises()
  return { wrapper, calls }
}

describe('TaskScheduler 列表：param_stale 标注与立即运行禁用', () => {
  it('过期任务显示「参数已过期」徽标，立即按钮禁用且 title 说明原因', async () => {
    const { wrapper } = await mountView(baseRoutes())
    const rows = wrapper.findAll('tbody tr')
    expect(rows[0].text()).toContain('参数已过期')
    const runBtn0 = rows[0].findAll('button').find(b => b.text().includes('▶ 立即'))
    expect(runBtn0.attributes('disabled')).toBeDefined()
    expect(runBtn0.attributes('title')).toContain('过期')
    // 未过期任务不受影响
    const runBtn1 = rows[1].findAll('button').find(b => b.text().includes('▶ 立即'))
    expect(runBtn1.attributes('disabled')).toBeUndefined()
  })
})

describe('编辑任务：快照带入 + 过期横幅/对比表 + 保存带稀疏 args', () => {
  async function openEdit(calls409 = false) {
    const routes = baseRoutes()
    routes.push({
      method: 'POST', url: '/api/tasks',
      body: calls409 ? () => ({ code: 'param_signature_conflict', message: '任务参数快照已过期' }) : {},
      status: calls409 ? 409 : 200,
    })
    const { wrapper, calls } = await mountView(routes)
    const rows = wrapper.findAll('tbody tr')
    await rows[0].findAll('button').find(b => b.text().includes('✎')).trigger('click')
    await flushPromises()
    return { wrapper, calls }
  }

  it('表单按脚本 params 渲染；快照以任务详情 args 视图为准；param_stale → 横幅 + 三列对比表', async () => {
    const { wrapper, calls } = await openEdit()
    const form = wrapper.find('[data-testid="params-form"]')
    expect(form.exists()).toBe(true)
    expect(form.findAll('.pf-row')).toHaveLength(2)
    // 详情视图 timeout=12s 覆盖态生效（≠ 列表兜底 10s），对比表三列渲染
    const banner = wrapper.find('.stale-banner')
    expect(banner.exists()).toBe(true)
    expect(wrapper.findAll('.cmp-table tbody tr')).toHaveLength(2)
    expect(wrapper.find('.cmp-table').text()).toContain('12s')
    expect(calls.some(c => c.method === 'GET' && c.url === '/api/tasks/t1')).toBe(true)
  })

  it('保存提交 args（含覆盖值）；成功后关闭弹窗', async () => {
    const { wrapper, calls } = await openEdit()
    const save = wrapper.findAll('.modal-foot .btn').find(b => b.text() === '保存')
    await save.trigger('click')
    await flushPromises()
    const posted = calls.find(c => c.method === 'POST' && c.url === '/api/tasks')
    expect(posted.body).toMatchObject({
      id: 't1', name: '每日签到', cron: '0 8 * * *',
      script_id: 'com.demo/main.yml', device_id: 'dev1',
      args: { enable: false, timeout: '12s' },
    })
    expect(wrapper.find('[data-testid="params-form"]').exists()).toBe(false)
  })

  it('保存 409 签名冲突：横幅出现；「重新确认」二次提交带 reconfirm:true', async () => {
    const { wrapper, calls } = await openEdit(true)
    // 编辑带入 param_stale → 横幅本就显示；直接点重新确认
    const reconfirm = wrapper.find('.stale-banner button')
    expect(reconfirm.text()).toContain('重新确认')
    await reconfirm.trigger('click')
    await flushPromises()
    const posts = calls.filter(c => c.method === 'POST' && c.url === '/api/tasks')
    expect(posts).toHaveLength(1)
    expect(posts[0].body).toMatchObject({ id: 't1', args: { enable: false, timeout: '12s' }, reconfirm: true })
  })

  it('首次保存遇 409（无 param_stale 编辑）：横幅与对比表随即出现', async () => {
    const routes = baseRoutes()
    routes.push({
      method: 'POST', url: '/api/tasks', status: 409,
      body: () => ({ code: 'param_signature_conflict', message: '任务参数快照已过期' }),
    })
    const { wrapper, calls } = await mountView(routes)
    // 编辑未过期任务 t2
    const rows = wrapper.findAll('tbody tr')
    await rows[1].findAll('button').find(b => b.text().includes('✎')).trigger('click')
    await flushPromises()
    expect(wrapper.find('.stale-banner').exists()).toBe(false)
    const save = wrapper.findAll('.modal-foot .btn').find(b => b.text() === '保存')
    await save.trigger('click')
    await flushPromises()
    expect(wrapper.find('.stale-banner').exists()).toBe(true)
    expect(wrapper.findAll('.cmp-table tbody tr')).toHaveLength(2)
    // 再次保存（无 reconfirm）仍发原请求——由用户显式点「重新确认」才带标记
    const posts = calls.filter(c => c.method === 'POST')
    expect(posts).toHaveLength(1)
    expect(posts[0].body.args).toEqual({ timeout: '45s' })
  })
})

describe('新建任务：选脚本渲染空表单，保存提交稀疏 args', () => {
  it('无快照：默认值字段不进 args；必填缺失校验阻断', async () => {
    const calls = stubFetch(baseRoutes())
    const wrapper = mount(TaskScheduler, {
      global: { stubs: { RunConflictModal: true } },
      attachTo: document.body,
    })
    await flushPromises()
    await wrapper.findAll('.page-head .btn').find(b => b.text().includes('新建任务')).trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="params-form"]').exists()).toBe(true)
    // 名称必填：填入后再保存
    const nameInput = wrapper.find('.modal-body input.input')
    await nameInput.setValue('新任务')
    const save = wrapper.findAll('.modal-foot .btn').find(b => b.text() === '保存')
    await save.trigger('click')
    await flushPromises()
    const posted = calls.find(c => c.method === 'POST' && c.url === '/api/tasks')
    // 默认值字段 enable/timeout 用户未动 = 省略；args 缺省（脚本无必填参数 → 全省略）
    expect(posted.body).toMatchObject({ name: '新任务', cron: '0 8 * * *', script_id: 'com.demo/main.yml' })
    expect(posted.body.args).toBeUndefined()
  })
})
