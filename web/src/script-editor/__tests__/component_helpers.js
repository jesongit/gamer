/**
 * 组件测试共享工具：reactive 模型 + CommandStack 装配（组件层标准接线形态）。
 */
import { reactive } from 'vue'
import { CommandStack } from '../commands'
import { parseFunctionLibrary, parseScript } from '../codec'

export function setupScript(yaml) {
  const { model } = parseScript(yaml)
  const reactiveModel = reactive(model)
  return { model: reactiveModel, stack: new CommandStack(reactiveModel), plain: model }
}

export function setupFunctions(yaml, file = 'common') {
  const { model } = parseFunctionLibrary(yaml, { file })
  const reactiveModel = reactive(model)
  return { model: reactiveModel, stack: new CommandStack(reactiveModel), plain: model }
}

/** 展开卡片（点击卡头展开按钮）。 */
export async function expandCard(wrapper, uuid) {
  const card = wrapper.find(`[data-step-uuid="${uuid}"]`)
  await card.find('button[title="展开编辑"]').trigger('click')
  return card
}

export function plainClone(value) {
  return JSON.parse(JSON.stringify(value))
}
