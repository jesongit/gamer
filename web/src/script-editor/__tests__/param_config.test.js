// @vitest-environment happy-dom
import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import { validateScript } from '../validation'
import ParamEditor from '../components/ParamEditor.vue'
import ConfigEditor from '../components/ConfigEditor.vue'
import { setupScript } from './component_helpers'

/**
 * ParamEditor：增删/排序/类型切换重置默认值/即时校验提示/既有引用的错误传播；
 * ConfigEditor：启用清除/interval 组合/threshold/log_level/未知键提示。
 */

const YAML = "params:\n  - 'bool:enable:是否启用:true'\n  - 'coord:pos:位置:[0.5, 0.5]'\nsteps:\n  - tap: $pos\n"

function mountParam(props = {}) {
  const created = setupScript(YAML)
  const wrapper = mount(ParamEditor, { props: { model: created.model, stack: created.stack, ...props } })
  return { ...created, wrapper }
}

describe('ParamEditor：增删与排序（经 CommandStack）', () => {
  it('添加参数 + undo', async () => {
    const { wrapper, model, stack } = mountParam()
    await wrapper.find('button.add').trigger('click')
    expect(model.params).toHaveLength(3)
    expect(model.params[2]).toEqual({ type: 'text', name: '', remark: '', default: null })
    stack.undo()
    expect(model.params).toHaveLength(2)
  })

  it('删除参数 + undo', async () => {
    const { wrapper, model, stack } = mountParam()
    await wrapper.findAll('button[title="删除参数"]')[0].trigger('click')
    expect(model.params.map((p) => p.name)).toEqual(['pos'])
    stack.undo()
    expect(model.params.map((p) => p.name)).toEqual(['enable', 'pos'])
  })

  it('上移/下移排序（set_params）+ undo', async () => {
    const { wrapper, model, stack } = mountParam()
    const downs = wrapper.findAll('button[title="下移"]')
    expect(downs[1].attributes('disabled')).toBeDefined() // 末行禁用
    await downs[0].trigger('click')
    expect(model.params.map((p) => p.name)).toEqual(['pos', 'enable'])
    stack.undo()
    expect(model.params.map((p) => p.name)).toEqual(['enable', 'pos'])
  })

  it('编辑变量名/备注（update_param）+ undo', async () => {
    const { wrapper, model, stack } = mountParam()
    const name = wrapper.find('input[aria-label="变量名"]')
    await name.setValue('flag')
    expect(model.params[0].name).toBe('flag')
    const remark = wrapper.find('input[aria-label="备注"]')
    await remark.setValue('开关')
    expect(model.params[0].remark).toBe('开关')
    stack.undo()
    stack.undo()
    expect(model.params[0].name).toBe('enable')
    expect(model.params[0].remark).toBe('是否启用')
  })
})

describe('ParamEditor：类型切换与默认值', () => {
  it('类型切换重置默认值为无', async () => {
    const { wrapper, model } = mountParam()
    const typeSel = wrapper.findAll('select[aria-label="参数类型"]')[1]
    await typeSel.setValue('text')
    expect(model.params[1].type).toBe('text')
    expect(model.params[1].default).toBeNull()
  })

  it('类型切换触发既有引用的校验错误传播', async () => {
    const { wrapper, model } = mountParam()
    await wrapper.findAll('select[aria-label="参数类型"]')[1].setValue('text')
    const diags = validateScript(model)
    expect(diags.some((d) => d.code === 'param.ref.type_mismatch' && d.step_path === 'steps[0]' && d.field === 'at')).toBe(true)
  })

  it('有默认值开关：开启生成类型化默认值，关闭回到 null', async () => {
    const { wrapper, model } = mountParam()
    const row1 = wrapper.findAll('.param-row')[1]
    // pos 已有默认值 → 关闭
    await row1.find('input[type="checkbox"]').setValue(false)
    expect(model.params[1].default).toBeNull()
    // 开启 → coord 默认 [0.5, 0.5]
    await row1.find('input[type="checkbox"]').setValue(true)
    expect(model.params[1].default).toEqual([0.5, 0.5])
  })

  it('默认值控件编辑（coord X）', async () => {
    const { wrapper, model } = mountParam()
    await wrapper.find('input[aria-label="pos 默认值X"]').setValue('0.9')
    expect(model.params[1].default).toEqual([0.9, 0.5])
  })
})

describe('ParamEditor：即时校验提示', () => {
  it('非法变量名提示', async () => {
    const { wrapper } = mountParam()
    const name = wrapper.find('input[aria-label="变量名"]')
    await name.setValue('9bad')
    expect(wrapper.text()).toContain('不符合 [A-Za-z_][A-Za-z0-9_]*')
  })

  it('重复变量名提示', async () => {
    const { wrapper } = mountParam()
    const name = wrapper.find('input[aria-label="变量名"]')
    await name.setValue('pos')
    expect(wrapper.text()).toContain('重复')
  })

  it('外部诊断（params[i] step_path）透传不报错', () => {
    const { wrapper } = mountParam({
      diagnostics: [{ code: 'param.decl.name_invalid', step_path: 'params[0]', field: 'name', message: 'x' }],
    })
    expect(wrapper.find('[data-testid="param-editor"]').exists()).toBe(true)
  })
})

describe('ConfigEditor', () => {
  it('未启用 → 启用配置（set_config）+ undo 回 null', async () => {
    const created = setupScript('steps:\n  - log: a\n')
    const w = mount(ConfigEditor, { props: { model: created.model, stack: created.stack } })
    expect(created.model.config).toBeNull()
    await w.find('button.add').trigger('click')
    expect(created.model.config).toEqual({ interval: '500ms', threshold: 0.85, log_level: 'info' })
    created.stack.undo()
    expect(created.model.config).toBeNull()
  })

  it('interval 数值+单位组合', async () => {
    const created = setupScript("config:\n  interval: 500ms\n  threshold: 0.85\n  log_level: info\nsteps:\n  - log: a\n")
    const w = mount(ConfigEditor, { props: { model: created.model, stack: created.stack } })
    await w.find('input[aria-label="轮询间隔数值"]').setValue('3')
    expect(created.model.config.interval).toBe('3ms')
    await w.find('select[aria-label="轮询间隔单位"]').setValue('s')
    expect(created.model.config.interval).toBe('3s')
  })

  it('threshold 滑块 + 数值', async () => {
    const created = setupScript("config:\n  interval: 500ms\n  threshold: 0.85\n  log_level: info\nsteps:\n  - log: a\n")
    const w = mount(ConfigEditor, { props: { model: created.model, stack: created.stack } })
    await w.find('input[aria-label="匹配阈值滑块"]').setValue('0.9')
    expect(created.model.config.threshold).toBe(0.9)
    const num = w.find('input[aria-label="匹配阈值数值"]')
    await num.setValue('1.4')
    await num.trigger('change')
    expect(created.model.config.threshold).toBe(1) // 收敛到 0~1
  })

  it('log_level 下拉', async () => {
    const created = setupScript("config:\n  interval: 500ms\n  threshold: 0.85\n  log_level: info\nsteps:\n  - log: a\n")
    const w = mount(ConfigEditor, { props: { model: created.model, stack: created.stack } })
    await w.find('select[aria-label="日志级别"]').setValue('warn')
    expect(created.model.config.log_level).toBe('warn')
  })

  it('未知配置键提示', () => {
    const created = setupScript("config:\n  interval: 500ms\n  threshold: 0.85\n  log_level: info\nsteps:\n  - log: a\n")
    created.model.config.extra = 'x'
    const w = mount(ConfigEditor, { props: { model: created.model, stack: created.stack } })
    expect(w.text()).toContain('未知配置键 extra')
  })

  it('清除配置', async () => {
    const created = setupScript("config:\n  interval: 500ms\n  threshold: 0.85\n  log_level: info\nsteps:\n  - log: a\n")
    const w = mount(ConfigEditor, { props: { model: created.model, stack: created.stack } })
    await w.find('button[title="清除配置（使用服务端默认值）"]').trigger('click')
    expect(created.model.config).toBeNull()
  })
})
