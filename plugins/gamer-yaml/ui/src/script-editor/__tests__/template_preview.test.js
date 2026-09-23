// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import StepCard from '../components/StepCard.vue'
import { SE_TARGET_OPTIONS } from '../targets'
import { setupScript, expandCard } from './component_helpers'
import catalog from '../../../../../../tools/yaml-tests/native-functions.json'

let wrapper
afterEach(() => wrapper?.unmount())

async function setup(source) {
  const created = setupScript(source)
  const matchTemplate = vi.fn().mockResolvedValue({hit: false})
  const params = name => catalog.find(f => f.name === name)?.params || []
  wrapper = mount(StepCard, {
    props: {...created, step: created.model.run[0], containerPath: ['run'], basePath: 'run', index: 0},
    global: {provide: {seCellTools: {matchTemplate}, [SE_TARGET_OPTIONS]: {
      targets: catalog.map(f => ({target: f.name, group: 'plugin'})),
      resolveParams: async name => params(name), resolveParamsSync: params,
    }}},
  })
  await expandCard(wrapper, created.model.run[0].uuid)
  return matchTemplate
}

it('步骤预览使用函数默认阈值，不使用 Core 测试阈值', async () => {
  const match = await setup('run:\n  - wait_find: {template: main.png}\n')
  await wrapper.get('button[title*="按步骤实际匹配规则"]').trigger('click')
  expect(match).toHaveBeenCalledWith('main.png', {threshold: .8, region: undefined})
})

it('目标继承步骤阈值和区域，障碍列表只继承阈值', async () => {
  const match = await setup('run:\n  - wait_find: {template: main.png, threshold: 0.93, region: [0.1, 0.2, 0.5, 0.5], obstacles: [close.png]}\n')
  for (const button of wrapper.findAll('button[title*="按步骤实际匹配规则"]')) await button.trigger('click')
  expect(match).toHaveBeenCalledWith('main.png', {threshold: .93, region: [.1, .2, .5, .5]})
  expect(match).toHaveBeenCalledWith('close.png', {threshold: .93, region: undefined})
})

it('模板分支使用本步骤阈值', async () => {
  const match = await setup('run:\n  - match_templates:\n      threshold: 0.94\n      cases:\n        - template: reward.png\n          do: []\n')
  await wrapper.get('button[title*="按步骤实际匹配规则"]').trigger('click')
  expect(match).toHaveBeenCalledWith('reward.png', {threshold: .94, region: undefined})
})

it('引用阈值无法静态求值时传递错误，不静默替换默认值', async () => {
  const match = await setup('run:\n  - find: {template: main.png, threshold: $limit}\n')
  await wrapper.get('button[title*="按步骤实际匹配规则"]').trigger('click')
  expect(match).toHaveBeenCalledWith('main.png', {error: expect.stringContaining('运行时变量')})
})
