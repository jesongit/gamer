// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import ErrorSummary from '../components/ErrorSummary.vue'
import YamlPreview from '../components/YamlPreview.vue'
import { setupScript } from './component_helpers'

/**
 * ErrorSummary：诊断列表渲染与点击定位事件（服务端错误回填同构 props）；
 * YamlPreview：codec.serialize 只读输出 + 复制/下载 + 不可编辑。
 */

const DIAGS = [
  { code: 'step.coord.range', step_path: 'steps[1]', field: 'at', message: '坐标超出 0~1' },
  { code: 'resource.tmpl.not_found', step_path: 'steps[0]', field: 'template', message: '模板 a.png 不存在' },
]

describe('ErrorSummary', () => {
  it('渲染 code + step_path + message', () => {
    const wrapper = mount(ErrorSummary, { props: { diagnostics: DIAGS } })
    expect(wrapper.text()).toContain('step.coord.range')
    expect(wrapper.text()).toContain('steps[1]')
    expect(wrapper.text()).toContain('坐标超出 0~1')
    expect(wrapper.text()).toContain('校验结果（2）')
  })

  it('点击行 → emit locate(原诊断对象)', async () => {
    const wrapper = mount(ErrorSummary, { props: { diagnostics: DIAGS } })
    await wrapper.findAll('.err-row')[1].trigger('click')
    expect(wrapper.emitted('locate')).toHaveLength(1)
    expect(wrapper.emitted('locate')[0]).toEqual([DIAGS[1]])
  })

  it('空列表显示无错误', () => {
    const wrapper = mount(ErrorSummary, { props: { diagnostics: [] } })
    expect(wrapper.text()).toContain('无错误')
  })

  it('接受服务端结构化错误（resource 字段透传）', () => {
    const serverDiag = { code: 'param.args.missing_required', message: '必填参数 x 未出现在 args 中', resource: 'sub.yaml', step_path: 'steps[2]', field: 'args' }
    const wrapper = mount(ErrorSummary, { props: { diagnostics: [serverDiag] } })
    expect(wrapper.text()).toContain('param.args.missing_required')
    expect(wrapper.text()).toContain('steps[2]')
  })
})

describe('YamlPreview', () => {
  it('展示 codec.serialize 输出（只读 pre）', () => {
    const created = setupScript("params:\n  - 'bool:enable:开关:true'\nconfig:\n  interval: 500ms\n  threshold: 0.85\n  log_level: info\nsteps:\n  - if: $enable\n    then:\n      - tap: [0.5, 0.5]\n")
    const wrapper = mount(YamlPreview, { props: { model: created.model } })
    const pre = wrapper.find('.yaml-pre')
    expect(pre.exists()).toBe(true)
    expect(pre.text()).toContain('steps:')
    expect(pre.text()).toContain("if: $enable")
    expect(pre.text()).toContain('tap: [0.5, 0.5]')
    expect(pre.text()).toContain('threshold: 0.85')
    // 无任何输入控件（不可编辑）
    expect(wrapper.find('textarea').exists()).toBe(false)
    expect(wrapper.find('input').exists()).toBe(false)
  })

  it('模型变化时预览跟随（serialize 响应式）', async () => {
    const created = setupScript('steps:\n  - log: a\n')
    const wrapper = mount(YamlPreview, { props: { model: created.model } })
    expect(wrapper.text()).not.toContain('tap')
    created.stack.apply({
      type: 'insert_step',
      path: ['steps'],
      index: 1,
      step: { uuid: 'yaml-preview-test-uuid', kind: 'tap', at: { lit: [0.1, 0.2] } },
    })
    await wrapper.vm.$nextTick()
    expect(wrapper.text()).toContain('tap: [0.1, 0.2]')
  })

  it('复制按钮可点击（无剪贴板环境走降级不抛错）', async () => {
    const created = setupScript('steps:\n  - log: a\n')
    const wrapper = mount(YamlPreview, { props: { model: created.model } })
    await wrapper.findAll('button')[0].trigger('click')
    expect(wrapper.text()).toContain('已复制')
  })

  it('下载按钮（stub URL.createObjectURL）', async () => {
    const created = setupScript('steps:\n  - log: a\n')
    const objUrl = 'blob:mock'
    const createObjectURL = vi.fn(() => objUrl)
    const revokeObjectURL = vi.fn()
    vi.stubGlobal('URL', { ...URL, createObjectURL, revokeObjectURL })
    const wrapper = mount(YamlPreview, { props: { model: created.model, filename: 'out.yaml' } })
    await wrapper.findAll('button')[1].trigger('click')
    expect(createObjectURL).toHaveBeenCalledTimes(1)
    expect(revokeObjectURL).toHaveBeenCalledWith(objUrl)
    vi.unstubAllGlobals()
  })

  it('关闭按钮 emit close', async () => {
    const created = setupScript('steps:\n  - log: a\n')
    const wrapper = mount(YamlPreview, { props: { model: created.model } })
    await wrapper.findAll('button')[2].trigger('click')
    expect(wrapper.emitted('close')).toHaveLength(1)
  })
})
