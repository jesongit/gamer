<template>
  <div class="task-board">
    <div class="board-head">
      <button class="btn btn-primary" @click="openAdd">＋ 新建任务</button>
      <!-- 服务端时区标识：契约禁止 /api/system/info 携带 timezone，只能从任务时间戳
           的 RFC3339 偏移推导（task-tz.js）；推导不出时明确说明按服务端本地时区执行 -->
      <div class="tz-hint" data-testid="server-tz-hint">
        <template v-if="serverTzLabel">
          <span>服务端时区</span>
          <span class="tz-badge mono">{{ serverTzLabel }}</span>
          <span>「下次执行」按服务端时区显示</span>
        </template>
        <template v-else>任务按服务端本地时区执行（Docker 部署可用 TZ 配置）</template>
      </div>
    </div>

    <div class="card" style="padding: 0; overflow: auto;">
      <table class="table">
        <thead>
          <tr>
            <th>任务名</th>
            <th style="width: 90px">启用</th>
            <th style="width: 170px">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in tasks" :key="t.id" :class="{ disabled: !t.enabled }">
            <td class="task-name">
              {{ t.name }}
              <span v-if="t.param_stale" class="tag stale-tag" title="任务参数快照与脚本当前参数声明不一致，测试/调度前请编辑任务重新确认">参数已过期</span>
            </td>
            <td>
              <label class="switch">
                <input type="checkbox" :checked="t.enabled" @change="toggle(t, $event)" />
                <span class="track"></span>
              </label>
            </td>
            <td>
              <div class="row-actions">
                <button class="btn btn-sm btn-ghost" :disabled="triggeringId === t.id || t.param_stale" :title="t.param_stale ? staleReason(t) : '马上运行一次（使用任务保存的参数快照）'" @click="runNow(t)">{{ triggeringId === t.id ? '触发中…' : '▶ 测试' }}</button>
                <button class="btn btn-sm btn-ghost" title="编辑任务" @click="editTask(t)">✎ 编辑</button>
                <button class="btn btn-sm btn-ghost danger" title="删除任务" @click="removeTask(t)">🗑</button>
              </div>
            </td>
          </tr>
          <tr v-if="!tasks.length">
            <td colspan="3" class="empty-row">暂无定时任务，点上方「＋ 新建任务」创建。</td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 新建/编辑弹窗（新建与编辑复用） -->
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
            <label>Cron 表达式（分 时 日 月 周）</label>
            <input v-model="form.cron" class="input mono" placeholder="0 8 * * *" />
            <div class="cron-presets">
              <button v-for="p in presets" :key="p.cron" class="btn btn-sm cp-item" type="button" @click="form.cron = p.cron">{{ p.name }}</button>
            </div>
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
              :templates="taskTemplateNames"
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

    <!-- 设备占用冲突 409 提示（测试运行命中活动 run 时） -->
    <RunConflictModal />
  </div>
</template>

<script setup>
/**
 * Console 右侧任务页签内容：
 * - 首行「＋ 新建任务」，下方任务列表每行只保留 任务名 / 启用开关 / 测试 / 编辑 / 删除；
 * - 「▶ 测试」= POST /api/tasks/:id/run 马上跑一次（用任务已存参数快照，param_stale 时禁用）；
 * - 新建与编辑复用同一弹窗：名称/cron（含预设）/脚本/设备/运行参数（ParamsForm 稀疏 args）；
 * - 保存/启停带参数快照与 psig1 签名门禁：409 param_signature_conflict → 横幅 + 三列对比表，
 *   「重新确认」带 reconfirm:true 按当前声明重算快照。
 */
import { ref, reactive, computed, onMounted } from 'vue'
import { tasksData, scriptsData, devicesData, templatesData, useToast, pushRunConflict } from '../store'
import { api } from '../api'
import ScriptPicker from './ScriptPicker.vue'
import ParamsForm from '../script-editor/components/ParamsForm.vue'
import { extractParams, fmtLiteral } from '../script-editor/params'
import { buildTaskSavePayload, isParamSignatureConflict, staleCompareRows, staleReason } from '../task-args'
import { serverTzLabelFromTasks } from '../task-tz'
import RunConflictModal from './RunConflictModal.vue'
import { shortRunId, isDeviceBusyConflict } from '../runs'

const toast = useToast()
const tasks = tasksData
const scripts = scriptsData
const devices = devicesData
const templates = templatesData
const showAdd = ref(false)
const editing = ref(false)
const form = reactive({ id: null, name: '', cron: '', script_id: '', device_id: '' })
// 测试（立即执行）触发中（行级防重复点击）；202 一返回即复位——不等任务完成
const triggeringId = ref('')

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
/** 参数里的 tmpl 控件与步骤画布共用当前脚本分区的模板短名候选。 */
const taskTemplateNames = computed(() => {
  const script = scripts.value.find(x => x.id === form.script_id)
  if (!script?.package) return []
  return templates.value
    .filter(t => t.pkg === script.package)
    .map(t => templateShortName(t.name))
})
// 服务端时区标签：从任务 next_run/last_run_at 的 RFC3339 偏移推导；null → 兜底文案
const serverTzLabel = computed(() => serverTzLabelFromTasks(tasks.value))
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

/** 去掉模板文件名上的区域/保色后缀，保持与步骤画布的短名输入口径一致。 */
function templateShortName(name) {
  return String(name || '')
    .replace(/#1(\.(png|jpe?g))$/i, '$1')
    .replace(/#[^#./\\]+(\.(png|jpe?g))$/i, '$1')
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

/** ▶ 测试（立即运行）：当前契约固定返回 202 {run_id}，触发即返回并恢复按钮可用。 */
async function runNow(t) {
  if (triggeringId.value) return
  triggeringId.value = t.id
  try {
    const rep = await api.runTaskNow(t.id)
    toast(`已触发（run ${shortRunId(rep.run_id)}）`, 'success')
    setTimeout(() => loadTasks(), 3000)
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
async function loadTemplates() {
  try { templates.value = await api.listTemplates() } catch (e) {}
}

onMounted(async () => {
  loadScripts()
  loadDevices()
  loadTemplates()
  await loadTasks()
})
</script>

<style scoped>
.board-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; flex-wrap: wrap; }
/* 服务端时区标识（契约禁止 system/info 带 timezone，从任务时间戳偏移推导） */
.tz-hint {
  display: flex; align-items: center; gap: 6px; flex-wrap: wrap;
  font-size: 12px; color: var(--text-2);
}
.tz-badge {
  color: var(--accent-2); border: 1px solid var(--border);
  border-radius: var(--radius-sm); padding: 1px 8px; background: var(--bg-3);
}
.task-name { font-weight: 600; }
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
.row-actions { display: flex; gap: 4px; align-items: center; }
.row-actions .danger:hover { color: var(--danger); border-color: var(--danger); }
tr.disabled { opacity: .45; }
.empty-row { text-align: center; color: var(--text-2); font-size: 12px; padding: 22px 0; }

/* cron 预设（收进弹窗：点一下填入表达式） */
.cron-presets { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 8px; }
.cp-item { font-size: 11px; padding: 3px 10px; }
</style>
