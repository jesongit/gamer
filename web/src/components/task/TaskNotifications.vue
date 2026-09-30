<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../../api'
import { GAMER_NOTIFY_PLUGIN_ID } from '../../gamer-plugin-ids'
import { notificationResults, taskNotificationPolicy, updateTaskNotificationPolicy } from './task-notifications'

const props = defineProps({ modelValue: { type: Object, default: () => ({}) } })
const emit = defineEmits(['update:modelValue'])
const policy = computed(() => taskNotificationPolicy(props.modelValue))
const channels = ref([]), available = ref(false), loading = ref(false)
const availability = ref('正在检查通知发送能力…')
function changePolicy(patch) {
  emit('update:modelValue', updateTaskNotificationPolicy(props.modelValue, { ...policy.value, ...patch }))
}
function changeRule(key, patch) {
  changePolicy({ results: { ...policy.value.results, [key]: { ...policy.value.results[key], ...patch } } })
}
function channelOptions(rule) {
  return [...channels.value, ...rule.channels.filter(id => !channels.value.some(c => c.id === id)).map(id => ({ id, name: id, enabled: false, missing: true }))]
}
async function refresh() {
  loading.value = true; available.value = false
  try {
    const list = await api.listExtensions()
    const plugin = (Array.isArray(list) ? list : list.extensions || []).find(p => p.id === GAMER_NOTIFY_PLUGIN_ID)
    if (plugin?.state !== 'running') {
      availability.value = '通知插件未安装或未启用，配置保留，运行时不会发送。'
      return
    }
    const result = await api.callExtension(GAMER_NOTIFY_PLUGIN_ID, 'channels.read')
    channels.value = result.channels || []; available.value = true
    availability.value = channels.value.length ? '' : '尚未配置通道，请到通知助手添加；也可以先填写通道 ID 保存。'
  } catch {
    availability.value = '通知发送能力暂不可用，配置可以继续编辑和保存。'
  } finally { loading.value = false }
}
onMounted(refresh)
</script>

<template>
  <section class="task-notifications" data-testid="task-notifications">
    <div class="heading">
      <label><input type="checkbox" :checked="policy.enabled" @change="changePolicy({ enabled: $event.target.checked })" /> 运行结果通知</label>
      <button type="button" class="btn btn-sm btn-ghost" :disabled="loading" @click="refresh">刷新通道</button>
    </div>
    <p v-if="availability" class="hint" role="status">{{ availability }}</p>
    <template v-if="policy.enabled">
      <p class="hint">定时执行与任务测试使用此配置。通知失败不影响任务结果。</p>
      <details v-for="result in notificationResults" :key="result.key" :open="policy.results[result.key].enabled">
        <summary>
          <label @click.stop><input type="checkbox" :checked="policy.results[result.key].enabled" @change="changeRule(result.key, { enabled: $event.target.checked })" /> {{ result.label }}时通知</label>
        </summary>
        <div class="rule">
          <label>通知通道（可多选）</label>
          <select v-if="available && channels.length" class="select" multiple :value="policy.results[result.key].channels" @change="changeRule(result.key, { channels: Array.from($event.target.selectedOptions, o => o.value) })">
            <option v-for="channel in channelOptions(policy.results[result.key])" :key="channel.id" :value="channel.id">{{ channel.name }}{{ channel.missing ? '（不存在）' : !channel.enabled ? '（已停用）' : '' }}</option>
          </select>
          <input v-else class="input mono" :value="policy.results[result.key].channels.join(', ')" placeholder="通道 ID，多个用英文逗号分隔" @input="changeRule(result.key, { channels: [...new Set($event.target.value.split(',').map(s => s.trim()).filter(Boolean))] })" />
          <label>标题</label>
          <input class="input" :value="policy.results[result.key].title" placeholder="留空使用任务名与结果" @input="changeRule(result.key, { title: $event.target.value })" />
          <label>正文</label>
          <textarea class="input" rows="3" :value="policy.results[result.key].content" placeholder="留空使用默认摘要：任务、设备、结果、时间、耗时、错误" @input="changeRule(result.key, { content: $event.target.value })" />
        </div>
      </details>
      <p class="hint variables">可用变量：<code v-pre>{{task.name}} {{task.id}} {{device.name}} {{device.id}} {{result}} {{time}} {{elapsed}} {{error}} {{entrypoint}} {{run.id}}</code></p>
    </template>
  </section>
</template>

<style scoped>
.task-notifications { border-top: 1px solid var(--border, #ddd); padding-top: 12px; margin-top: 12px; }
.heading { display: flex; align-items: center; justify-content: space-between; }
.heading label, summary label { display: flex; align-items: center; gap: 6px; }
.hint { font-size: 12px; color: var(--text-secondary, #777); line-height: 1.6; margin: 6px 0; }
details { border: 1px solid var(--border, #ddd); border-radius: 4px; margin: 8px 0; padding: 8px; }
summary { cursor: pointer; }
.rule { display: grid; gap: 6px; margin-top: 8px; }
.rule label { font-size: 12px; }
.rule select { min-height: 62px; }
.variables { overflow-wrap: anywhere; }
</style>
