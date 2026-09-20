<template>
  <Teleport to="body"><div class="modal-mask" @click.self="$emit('close')"><section class="modal error-location">
    <div class="modal-head"><span>运行报错位置</span><button class="btn btn-icon" aria-label="关闭报错位置" @click="$emit('close')"><UiIcon name="close" /></button></div>
    <div class="modal-body">
      <p class="source mono">{{ source?.package_id }} / {{ source?.path }}<template v-if="source?.function"> · {{ source.function }}</template></p>
      <p class="source">{{ event.path }} · 调用帧 {{ event.trace?.frame_id }} · {{ event.trace?.run_id }}</p>
      <p class="failure">{{ event.error || '运行失败' }}</p>
      <p v-if="note" role="status">{{ note }}</p>
      <div v-if="model" inert><StepCanvas ref="canvas" :model="model" :stack="stack" :diagnostics="[]" :initial-fn="source?.function || ''" lock-fn compact-toolbar /></div>
    </div>
    <div class="modal-foot"><span>只读定位，当前未保存修改已保留</span><button class="btn" @click="$emit('close')">关闭</button></div>
  </section></div></Teleport>
</template>
<script setup>
import { computed, nextTick, onMounted, ref, shallowRef } from 'vue'
import UiIcon from '../../../../../../web/src/components/ui/UiIcon.vue'
import StepCanvas from '../../script-editor/components/StepCanvas.vue'
import { api } from '../../../../../../web/src/api'
import { parseFunctionLibrary, parseScript } from '../../script-editor/codec'
import { CommandStack } from '../../script-editor/commands'
const props = defineProps({ event: { type: Object, required: true } })
defineEmits(['close'])
const source = computed(() => props.event.trace?.source)
const note = ref('正在读取定义…'), model = shallowRef(null), stack = shallowRef(null), canvas = ref(null)
onMounted(async () => {
  const s = source.value
  if (!s?.package_id || !s.path?.startsWith('automations/')) { note.value = '该事件没有可验证的资源位置。'; return }
  try {
    const id = `${s.package_id}/${s.path.slice('automations/'.length)}`
    const file = await (s.function ? api.getFunction(id) : api.getScript(id))
    if (!s.version || file.version !== s.version) { note.value = '源文件已变化，无法把执行时步骤对应到当前内容。请根据上方文件、函数和路径检查；当前编辑未改变。'; return }
    const parsed = s.function ? parseFunctionLibrary(file.content, { file: s.path }) : parseScript(file.content)
    if (!parsed.model || parsed.diagnostics?.length) { note.value = '定义无法解析，请查看源文件诊断。'; return }
    model.value = parsed.model
    stack.value = new CommandStack(model.value)
    note.value = `执行版本 ${s.version}`
    await nextTick()
    let path = props.event.path
    if (s.function && !path.startsWith('functions.')) path = path.startsWith(`${s.function}.`) ? `functions.${path}` : `functions.${s.function}.${path}`
    canvas.value?.locate({ step_path: path, field: '', message: props.event.error || '运行报错' })
  } catch (error) { note.value = `无法读取执行源文件：${error.message}` }
})
</script>
<style scoped>
.error-location{width:min(740px,calc(100vw - 32px))}.modal-body{max-height:70vh;overflow:auto}.source{font-size:12px;overflow-wrap:anywhere;margin:4px 0}.failure{color:var(--danger);white-space:pre-wrap}.modal-foot span{font-size:12px;color:var(--text-2);margin-right:auto}
</style>
