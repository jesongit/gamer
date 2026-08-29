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
            <td class="task-name">
              {{ t.name }}
              <span v-if="t.param_stale" class="tag stale-tag" title="任务参数快照与脚本当前参数声明不一致，调度前请编辑任务重新确认">参数已过期</span>
              <span v-if="activeRuns[t.device_id]" class="tag run run-now" title="该设备当前有活动中的自动化运行（含本任务或手动触发）">运行中<template v-if="sourceLabel(activeRuns[t.device_id].source)"> · {{ sourceLabel(activeRuns[t.device_id].source) }}</template></span>
            </td>
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
                <button class="btn btn-sm btn-ghost" :disabled="triggeringId === t.id || t.param_stale" :title="t.param_stale ? staleReason(t) : '立即运行（使用任务保存的参数快照）'" @click="runNow(t)">{{ triggeringId === t.id ? '触发中…' : '▶ 立即' }}</button>
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
              <ScriptPicker v-model="form.script_id" @update:model-value="onScriptPicked" />
            </div>
            <div class="form-item">
              <label>设备</label>
              <select v-model="form.device_id" class="select">
                <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }}</option>
              </select>
            </div>
          </div>
          <!-- 运行参数（阶段 5）：选脚本后按其 params 渲染表单；保存提交稀疏 args（显式覆盖），
               服务端解析为完整快照存储。默认值字段不动 = 省略（服务端取声明默认值）。 -->
          <div v-if="taskParams.length" class="form-item">
            <label>运行参数</label>
            <div v-if="staleNotice" class="stale-banner">
              <span>⚠️ 参数已过期：脚本参数声明已变化，任务原快照可能与当前脚本不一致。</span>
              <button class="btn btn-sm" :disabled="savingTask" title="按当前参数声明重算快照并保存（reconfirm）" @click="saveTask(true)">重新确认</button>
            </div>
            <table v-if="staleNotice" class="cmp-table">
              <thead>
                <tr><th>参数</th><th>任务原快照</th><th>当前默认值</th><th>本次采用</th></tr>
              </thead>
              <tbody>
                <tr v-for="r in staleRows" :key="r.name">
                  <td class="mono">{{ r.name }}</td>
                  <td class="mono">{{ fmtLiteral(r.snapshot) }}</td>
                  <td class="mono">{{ fmtLiteral(r.currentDefault) }}</td>
                  <td class="mono adopted">{{ fmtLiteral(r.adopted) }}</td>
                </tr>
              </tbody>
            </table>
            <ParamsForm
              ref="taskFormEl"
              :params="taskParams"
              :initial-args="taskInitialArgs"
              @change="onFormChange"
            />
          </div>
        </div>
        <div class="modal-foot">
          <button class="btn" @click="showAdd = false">取消</button>
          <button class="btn btn-primary" :disabled="savingTask" @click="saveTask()">{{ savingTask ? '保存中…' : '保存' }}</button>
        </div>
      </div>
    </div>

    <!-- 设备占用冲突 409 提示（立即运行命中活动 run 时；仍要查看日志 → 跳控制台对应设备） -->
    <RunConflictModal />
  </div>
</template>

<script setup>
import { ref, reactive, computed, onMounted } from 'vue'
import { tasksData, scriptsData, devicesData, useToast, pushRunConflict } from '../store'
import { api } from '../api'
import ScriptPicker from '../components/ScriptPicker.vue'
import ParamsForm from '../script-editor/components/ParamsForm.vue'
import { extractParams, fmtLiteral } from '../script-editor/params'
import { buildTaskSavePayload, isParamSignatureConflict, staleCompareRows, staleReason } from '../task-args'
import RunConflictModal from '../components/RunConflictModal.vue'
import { sourceLabel, shortRunId, normalizeActiveRunResponse, isDeviceBusyConflict } from '../runs'

const toast = useToast()
const tasks = tasksData
const scripts = scriptsData
const devices = devicesData
const showAdd = ref(false)
const editing = ref(false)
const form = reactive({ id: null, name: '', cron: '', script_id: '', device_id: '' })
// 立即执行触发中（行级防重复点击）；202 一返回即复位——不等任务完成
const triggeringId = ref('')
// 各设备当前活动 run 摘要（deviceId → 归一化记录）：列表标注「运行中 · 来源」
const activeRuns = ref({})

// ---- 运行参数（阶段 5，plan §12.3）：表单态 + 过期横幅 + 快照对比 ----
const taskFormEl = ref(null)
// 任务存储 args 快照（编辑时整体带入 → 全部为覆盖态，本次采用=快照值，除非用户改）
const taskInitialArgs = ref({})
// 打开弹窗时的脚本 id：切换脚本后任务原快照不再适用，清空带入并隐藏横幅
const openedScriptId = ref('')
// param_stale 编辑带入 或 保存 409 param_signature_conflict → 横幅 + 三列对比表
const staleNotice = ref(false)
const savingTask = ref(false)
// ParamsForm 变化回传（args=稀疏覆盖；effective=完整采用值视图，对比表「本次采用」列）
const formChange = ref({ args: {}, effective: {} })

const taskParams = computed(() => {
  const s = scripts.value.find(x => x.id === form.script_id)
  if (!s) return []
  return extractParams(s.content ?? '')
})
const staleRows = computed(() =>
  staleCompareRows(taskParams.value, taskInitialArgs.value, formChange.value.effective))

function onFormChange(payload) {
  formChange.value = payload
}

function onScriptPicked(v) {
  if (v !== openedScriptId.value) {
    taskInitialArgs.value = {}
    staleNotice.value = false
  }
}

const presets = [
  { name: '每分钟', cron: '* * * * *' },
  { name: '每 10 分钟', cron: '*/10 * * * *' },
  { name: '每小时', cron: '0 * * * *' },
  { name: '每天 8:00', cron: '0 8 * * *' },
  { name: '每天 12:30', cron: '30 12 * * *' },
  { name: '每天 21:00', cron: '0 21 * * *' },
  { name: '每周一 9:00', cron: '0 9 * * 1' }
]

function scriptName(id) {
  const s = scripts.value.find(s => s.id === id)
  if (!s) return id
  return s.package === 'default' ? s.name : `${s.package}/${s.name}`
}
function deviceName(id) { return devices.value.find(d => d.id === id)?.name || id }
function lastTag(last) {
  if (last === '成功') return 'ok'
  if (last === '失败') return 'err'
  return ''
}

function openAdd() {
  editing.value = false
  Object.assign(form, { id: null, name: '', cron: '0 8 * * *', script_id: scripts.value[0]?.id || '', device_id: devices.value[0]?.id || '' })
  taskInitialArgs.value = {}
  openedScriptId.value = form.script_id
  staleNotice.value = false
  showAdd.value = true
}

function editTask(t) {
  editing.value = true
  Object.assign(form, { id: t.id, name: t.name, cron: t.cron, script_id: t.script_id, device_id: t.device_id })
  openedScriptId.value = t.script_id
  staleNotice.value = !!t.param_stale
  // 先按列表兜底（旧快照字段若有），再拉任务详情的 args 解析视图整体带入覆盖态
  //（resolve_entry_args 语义：本次采用=快照值，除非用户改/关覆盖）
  taskInitialArgs.value = t.args && typeof t.args === 'object' ? JSON.parse(JSON.stringify(t.args)) : {}
  showAdd.value = true
  api.getTask(t.id).then((detail) => {
    if (form.id !== t.id || !showAdd.value) return // 弹窗已切换/关闭：丢弃迟到响应
    if (detail && detail.args && typeof detail.args === 'object') {
      taskInitialArgs.value = JSON.parse(JSON.stringify(detail.args))
    }
  }).catch(() => { /* 详情拉取失败：按现有兜底渲染（默认值字段省略） */ })
}

async function toggle(t, e) {
  try {
    // 启停带原快照：避免服务端把缺省 args 当作「清空快照」（保持任务参数不变）
    await api.saveTask(buildTaskSavePayload({
      id: t.id, name: t.name, cron: t.cron, script_id: t.script_id,
      device_id: t.device_id, enabled: e.target.checked, args: t.args,
    }))
    toast(`${t.name} 已${e.target.checked ? '启用' : '停用'}`, 'info')
    loadTasks()
  } catch (err) {
    if (isParamSignatureConflict(err)) toast('参数快照已过期：请先编辑任务确认参数，再启用调度', 'error')
    else toast('操作失败：' + err.message, 'error')
    loadTasks()
  }
}

/** 立即运行：新契约 202 {run_id} —— 触发即返回并立刻恢复按钮可用（不等任务完成），
 *  提示「已触发（run xxxxxxxx）」；409 设备占用入队冲突弹窗；
 *  旧后端响应无 run_id → 静默降级为旧文案「已触发 <名>」 */
async function runNow(t) {
  if (triggeringId.value) return
  triggeringId.value = t.id
  try {
    const rep = await api.runTaskNow(t.id)
    toast(rep && rep.run_id ? `已触发（run ${shortRunId(rep.run_id)}）` : `已触发 ${t.name}`, 'success')
    // 稍后刷新列表与活跃标注（服务端开始执行后设备侧才登记）
    setTimeout(() => { loadTasks(); refreshActiveRuns() }, 3000)
  } catch (e) {
    if (isDeviceBusyConflict(e)) {
      pushRunConflict({ ...(e.data || {}), device_id: t.device_id })
    } else if (isParamSignatureConflict(e)) {
      toast('任务参数快照已过期：请编辑任务重新确认参数后再运行', 'error')
    } else {
      toast('触发失败：' + e.message, 'error')
    }
  } finally {
    triggeringId.value = ''
  }
}

/** 拉取各任务设备的当前活动 run（有则标注「运行中 · 来源」）。
 *  端点缺失（旧后端仅兼容形状也能归一化）或网络错 → 静默留空，不影响列表 */
async function refreshActiveRuns() {
  const ids = [...new Set(tasks.value.map(t => t.device_id).filter(Boolean))]
  if (!ids.length) { activeRuns.value = {}; return }
  const reps = await Promise.allSettled(ids.map(id => api.deviceRun(id)))
  const map = {}
  reps.forEach((r, i) => {
    if (r.status !== 'fulfilled') return
    const rec = normalizeActiveRunResponse(r.value)
    if (rec && rec.run_id) map[ids[i]] = rec
  })
  activeRuns.value = map
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

/**
 * 保存任务（阶段 5 参数化）：表单客户端校验（必填缺失/类型不合规阻断）→ 稀疏 args 提交，
 * 服务端解析为完整快照存储并计算 param_signature。签名不匹配 409 → 横幅 + 对比表，
 * 用户「重新确认」带 reconfirm:true 按当前声明重算。
 */
async function saveTask(reconfirm = false) {
  if (!form.name || !form.cron) return toast('请填写名称和 cron 表达式', 'error')
  if (!form.script_id) return toast('请选择脚本', 'error')
  let args = {}
  if (taskParams.value.length && taskFormEl.value) {
    const errs = taskFormEl.value.validate()
    if (errs.length) {
      return toast('参数校验未通过：' + errs.map(e => `$${e.name} ${e.message}`).join('；'), 'error')
    }
    args = taskFormEl.value.getArgs()
  }
  savingTask.value = true
  try {
    await api.saveTask(buildTaskSavePayload({
      id: form.id, name: form.name, cron: form.cron,
      script_id: form.script_id, device_id: form.device_id, args,
    }, { reconfirm }))
    showAdd.value = false
    toast(reconfirm ? '参数已重新确认，任务已保存' : '任务已保存', 'success')
    loadTasks()
  } catch (e) {
    if (isParamSignatureConflict(e)) {
      // 脚本参数声明已变化：展示原快照/当前默认值/本次采用对比，由用户显式重新确认
      staleNotice.value = true
      toast('任务参数快照已过期（脚本参数声明已变化），请核对对比表后点「重新确认」', 'error')
    } else {
      toast('保存失败：' + e.message, 'error')
    }
  } finally {
    savingTask.value = false
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

onMounted(async () => {
  loadScripts()
  loadDevices()
  await loadTasks()
  refreshActiveRuns()
})
</script>

<style scoped>
.task-name { font-weight: 600; }
/* 设备当前活动 run 标注（normalizeActiveRunResponse 命中时展示；配色复用全局 tag.run） */
.run-now { margin-left: 6px; font-weight: 400; vertical-align: 1px; }
.cron { color: var(--accent-2); font-size: 12px; }
tr.disabled { opacity: .45; }
/* 参数已过期标注（param_stale）与弹窗内横幅/对比表 */
.stale-tag { margin-left: 6px; color: var(--warn); border-color: var(--warn); font-weight: 400; }
.stale-banner {
  display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
  border: 1px solid var(--warn); border-radius: var(--radius-sm);
  background: rgba(250, 179, 135, .08); color: var(--warn);
  font-size: 12px; padding: 6px 10px; margin-bottom: 8px;
}
.cmp-table { width: 100%; border-collapse: collapse; font-size: 11px; margin-bottom: 8px; }
.cmp-table th, .cmp-table td { border: 1px solid var(--border); padding: 3px 6px; text-align: left; word-break: break-all; }
.cmp-table th { color: var(--text-2); font-weight: 500; background: var(--bg-3); }
.cmp-table td.adopted { color: var(--accent); }
.mono { font-family: var(--mono); }
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
