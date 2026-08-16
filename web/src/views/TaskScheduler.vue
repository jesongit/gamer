<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">定时任务</div>
        <div class="page-sub">服务端 cron 调度 · Docker 内 7×24 运行 · 浏览器关闭不影响执行</div>
      </div>
      <button class="btn btn-primary" @click="openAdd">＋ 新建任务</button>
    </div>

    <div class="card" style="padding: 0; overflow: auto;">
      <table class="table">
        <thead>
          <tr>
            <th>任务</th>
            <th>Cron 表达式</th>
            <th>脚本</th>
            <th>设备</th>
            <th>下次执行</th>
            <th>上次结果</th>
            <th>启用</th>
            <th style="width: 110px">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in tasks" :key="t.id" :class="{ disabled: !t.enabled }">
            <td class="task-name">{{ t.name }}</td>
            <td><span class="cron mono">{{ t.cron }}</span></td>
            <td class="mono">{{ scriptName(t.script_id) }}</td>
            <td>{{ deviceName(t.device_id) }}</td>
            <td class="mono">{{ t.next_run }}</td>
            <td><span class="tag" :class="lastTag(t.last_result)">{{ t.last_result || '未运行' }}</span></td>
            <td>
              <label class="switch">
                <input type="checkbox" :checked="t.enabled" @change="toggle(t, $event)" />
                <span class="track"></span>
              </label>
            </td>
            <td>
              <div class="row-actions">
                <button class="btn btn-sm btn-ghost" @click="runNow(t)">▶ 立即</button>
                <button class="btn btn-sm btn-ghost" @click="editTask(t)">✎</button>
                <button class="btn btn-sm btn-ghost danger" @click="removeTask(t)">🗑</button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <div class="cron-presets card">
      <div class="cp-title">常用 cron 预设</div>
      <div class="cp-items">
        <div v-for="p in presets" :key="p.cron" class="cp-item" @click="applyPreset(p)">
          <div class="cp-name">{{ p.name }}</div>
          <div class="cp-cron mono">{{ p.cron }}</div>
        </div>
      </div>
      <div class="cp-hint">点击预设可快速填入 · cron 格式：分 时 日 月 周</div>
    </div>

    <!-- 新建/编辑弹窗 -->
    <div v-if="showAdd" class="modal-mask" @click.self="showAdd = false">
      <div class="modal">
        <div class="modal-head">
          <span class="title">{{ editing ? '编辑任务' : '新建任务' }}</span>
          <button class="btn btn-ghost btn-sm" @click="showAdd = false">✕</button>
        </div>
        <div class="modal-body">
          <div class="form-item">
            <label>任务名称</label>
            <input v-model="form.name" class="input" placeholder="例如：每日签到" />
          </div>
          <div class="form-item">
            <label>Cron 表达式</label>
            <input v-model="form.cron" class="input mono" placeholder="0 8 * * *" />
          </div>
          <div class="form-row">
            <div class="form-item">
              <label>脚本</label>
              <select v-model="form.script_id" class="select mono">
                <option v-for="s in scripts" :key="s.id" :value="s.id">{{ s.name }}</option>
              </select>
            </div>
            <div class="form-item">
              <label>设备</label>
              <select v-model="form.device_id" class="select">
                <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }}</option>
              </select>
            </div>
          </div>
        </div>
        <div class="modal-foot">
          <button class="btn" @click="showAdd = false">取消</button>
          <button class="btn btn-primary" @click="saveTask">保存</button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, reactive, onMounted } from 'vue'
import { tasksData, scriptsData, devicesData, useToast } from '../store'
import { api } from '../api'

const toast = useToast()
const tasks = tasksData
const scripts = scriptsData
const devices = devicesData
const showAdd = ref(false)
const editing = ref(false)
const form = reactive({ id: null, name: '', cron: '', script_id: '', device_id: '' })

const presets = [
  { name: '每分钟', cron: '* * * * *' },
  { name: '每 10 分钟', cron: '*/10 * * * *' },
  { name: '每小时', cron: '0 * * * *' },
  { name: '每天 8:00', cron: '0 8 * * *' },
  { name: '每天 12:30', cron: '30 12 * * *' },
  { name: '每天 21:00', cron: '0 21 * * *' },
  { name: '每周一 9:00', cron: '0 9 * * 1' }
]

function scriptName(id) { return scripts.value.find(s => s.id === id)?.name || id }
function deviceName(id) { return devices.value.find(d => d.id === id)?.name || id }
function lastTag(last) {
  if (last === '成功') return 'ok'
  if (last === '失败') return 'err'
  return ''
}

function openAdd() {
  editing.value = false
  Object.assign(form, { id: null, name: '', cron: '0 8 * * *', script_id: scripts.value[0]?.id || '', device_id: devices.value[0]?.id || '' })
  showAdd.value = true
}

function editTask(t) {
  editing.value = true
  Object.assign(form, { id: t.id, name: t.name, cron: t.cron, script_id: t.script_id, device_id: t.device_id })
  showAdd.value = true
}

async function toggle(t, e) {
  try {
    await api.saveTask({ id: t.id, name: t.name, cron: t.cron, script_id: t.script_id, device_id: t.device_id, enabled: e.target.checked })
    toast(`${t.name} 已${e.target.checked ? '启用' : '停用'}`, 'info')
    loadTasks()
  } catch (err) {
    toast('操作失败：' + err.message, 'error')
  }
}

async function runNow(t) {
  try {
    await api.runTaskNow(t.id)
    toast(`已触发 ${t.name}`, 'success')
    setTimeout(loadTasks, 3000)
  } catch (e) {
    toast('触发失败：' + e.message, 'error')
  }
}

async function removeTask(t) {
  if (!confirm(`删除任务 ${t.name}？`)) return
  try {
    await api.deleteTask(t.id)
    tasks.value = tasks.value.filter(x => x.id !== t.id)
    toast('任务已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

function applyPreset(p) { form.cron = p.cron }

async function saveTask() {
  if (!form.name || !form.cron) return toast('请填写名称和 cron 表达式', 'error')
  try {
    await api.saveTask({
      id: form.id, name: form.name, cron: form.cron,
      script_id: form.script_id, device_id: form.device_id
    })
    showAdd.value = false
    toast('任务已保存', 'success')
    loadTasks()
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  }
}

async function loadTasks() {
  try { tasks.value = await api.listTasks() } catch (e) {}
}
async function loadScripts() {
  try { scripts.value = await api.listScripts() } catch (e) {}
}
async function loadDevices() {
  try { devices.value = await api.listDevices() } catch (e) {}
}

onMounted(() => { loadTasks(); loadScripts(); loadDevices() })
</script>

<style scoped>
.task-name { font-weight: 600; }
.cron { color: var(--accent-2); font-size: 12px; }
tr.disabled { opacity: .45; }
.row-actions { display: flex; gap: 2px; }
.row-actions .danger:hover { color: var(--danger); border-color: var(--danger); }

.cron-presets { display: flex; flex-direction: column; gap: 10px; }
.cp-title { font-size: 13px; font-weight: 600; }
.cp-items { display: flex; gap: 8px; flex-wrap: wrap; }
.cp-item {
  display: flex; flex-direction: column; gap: 2px; padding: 8px 14px;
  background: var(--bg-3); border: 1px solid var(--border); border-radius: var(--radius-sm);
  cursor: pointer; transition: all .15s;
}
.cp-item:hover { border-color: var(--accent); }
.cp-name { font-size: 12px; }
.cp-cron { font-size: 11px; color: var(--accent-2); }
.cp-hint { font-size: 11px; color: var(--text-2); }
</style>
