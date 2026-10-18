<template>
  <section class="task-run-history" aria-label="任务运行记录">
    <div class="history-controls">
      <button class="btn" @click="$emit('edit')">返回任务</button>
      <select v-model="selected" class="select" aria-label="选择任务运行记录">
        <option value="" disabled>{{ loading ? '正在读取…' : '暂无运行记录' }}</option>
        <option v-for="run in records" :key="run.run_id" :value="run.run_id">{{ new Date(run.started_at).toLocaleString() }} · {{ runState(run.state) }}</option>
      </select>
      <button class="btn" :disabled="loading" @click="load()">刷新</button>
      <a v-if="current?.finished_at" class="btn" :href="runDiagnosticsUrl(current.run_id)" :download="runDiagnosticsFilename(current.run_id)">下载诊断包</a>
      <button v-else-if="current" class="btn" disabled>运行结束后可下载诊断包</button>
    </div>
    <p class="history-hint">仅显示当前任务的运行；详细动作由对应执行器提供。</p>
    <p v-if="current?.finished_at" class="history-hint">诊断包包含本次运行记录和判断画面，注明采样与证据缺口；参数值与私密配置不导出。</p>
    <p v-if="current?.diagnostics?.details_status === 'expired'" class="history-hint">详细步骤已到期，运行结果仍保留。</p>
    <p v-if="error" class="history-error" role="alert">{{ error }}</p>
    <div v-if="current" class="history-result">
      <strong>{{ runState(current.state) }}</strong>
      <span>{{ current.entrypoint }}</span>
      <span>{{ deviceName(current.device_id) }}</span>
      <p v-if="current.error" class="history-error">{{ current.error }}</p>
    </div>
    <p v-else class="history-hint">{{ loading ? '正在读取运行记录…' : '本任务尚无运行记录，运行后可在这里查看结果。' }}</p>
    <button v-if="hasOlder" class="btn" :disabled="loading" @click="load(true)">加载更早运行记录</button>
  </section>
</template>
<script setup>
import { computed, onScopeDispose, ref, watch } from 'vue'
import { api } from '../../api'
import { devicesData } from '../../store'
import { runDiagnosticsFilename, runDiagnosticsUrl } from '../../run-diagnostics'

const props = defineProps({ taskId: { type: String, required: true } })
defineEmits(['edit'])
const records = ref([]), selected = ref(''), loading = ref(false), error = ref(''), hasOlder = ref(false)
const current = computed(() => records.value.find(run => run.run_id === selected.value))
let generation = 0, olderCursor = ''
const runState = state => ({ starting: '准备中', running: '运行中', paused: '已暂停', stopping: '正在停止', success: '成功', failed: '失败', cancelled: '已停止' }[state] || state)
const deviceName = id => devicesData.value.find(device => device.id === id)?.name || id
async function load(older = false) {
  if (loading.value) return
  const request = generation, task = props.taskId
  loading.value = true
  try {
    const list = await api.listRunHistory(undefined, undefined, older ? olderCursor : undefined, task)
    if (request !== generation) return
    const filtered = list.filter(run => run.task_id === task)
    records.value = [...new Map([...records.value, ...filtered].map(run => [run.run_id, run])).values()]
      .sort((a, b) => b.started_at.localeCompare(a.started_at) || b.run_id.localeCompare(a.run_id))
    if (older || !olderCursor) { olderCursor = filtered.at(-1)?.run_id || ''; hasOlder.value = list.length === 30 }
    if (!selected.value) selected.value = records.value[0]?.run_id || ''
    error.value = ''
  } catch (cause) { if (request === generation) error.value = `读取运行记录失败：${cause.message}` }
  finally { if (request === generation) loading.value = false }
}
watch(() => props.taskId, () => { generation++; records.value = []; selected.value = ''; olderCursor = ''; hasOlder.value = false; loading.value = false; void load() }, { immediate: true })
const timer = setInterval(() => { void load() }, 5000)
onScopeDispose(() => { generation++; clearInterval(timer) })
</script>
<style scoped>
.task-run-history{display:flex;flex-direction:column;gap:12px;min-height:0;overflow:auto}.history-controls{display:flex;gap:8px;flex-wrap:wrap}.history-controls .select{flex:1;min-width:160px}.history-hint{color:var(--text-2);font-size:13px;line-height:1.6;margin:0}.history-error{color:var(--danger);font-size:13px;line-height:1.6;white-space:pre-wrap;overflow-wrap:anywhere;margin:0}.history-result{display:flex;gap:12px;flex-wrap:wrap;padding:12px;background:var(--bg-0);font-size:13px;line-height:1.6}.history-result .history-error{flex-basis:100%}.task-run-history>.btn{align-self:flex-start}
</style>
