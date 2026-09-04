<template>
  <div class="task-board">
    <div class="board-head">
      <button class="btn btn-primary" @click="openAdd">＋ 新建任务</button>
      <!-- 服务端时区标识：契约禁止 /api/system/info 携带 timezone。P11.1 后任务
           时间戳均为 RFC3339 UTC 串（不携带服务端本地偏移），无法可靠推导 →
           常显兜底文案 -->
      <div class="tz-hint" data-testid="server-tz-hint">
        任务按服务端本地时区执行（Docker 部署可用 TZ 配置）
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
              <span v-if="t.state === 'dependency_missing'" class="tag stale-tag" title="运行依赖缺失（执行器或触发器未注册），任务已保留并休眠">依赖缺失</span>
            </td>
            <td>
              <label class="switch">
                <input type="checkbox" :checked="t.enabled" @change="toggle(t, $event)" />
                <span class="track"></span>
              </label>
            </td>
            <td>
              <div class="row-actions">
                <button class="btn btn-sm btn-ghost" :disabled="triggeringId === t.id" title="马上运行一次（使用任务保存的 runner payload）" @click="runNow(t)">{{ triggeringId === t.id ? '触发中…' : '▶ 测试' }}</button>
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
              <ScriptPicker
                v-model="form.script_id"
                :package="props.activePkg || ''"
                :lock-package="props.activePkg !== null"
                @update:model-value="onScriptPicked"
              />
            </div>
            <div class="form-item">
              <label>设备</label>
              <select v-model="form.device_id" class="select">
                <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }}</option>
              </select>
            </div>
          </div>
          <!-- 运行参数：选脚本后按其 params 渲染表单；保存提交稀疏 args 进
               runner.payload.args（gamer.yaml runner 运行时按脚本当前声明重绑） -->
          <div v-if="taskParams.length" class="form-item">
            <label>运行参数</label>
            <ParamsForm
              ref="taskFormEl"
              :params="taskParams"
              :initial-args="taskInitialArgs"
              :templates="taskTemplateNames"
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
 * Console 右侧任务页签内容（P11.1 机械适配 ADR-12 统一 Task 模型）：
 * - 首行「＋ 新建任务」，下方任务列表每行只保留 任务名 / 启用开关 / 测试 / 编辑 / 删除；
 * - 「▶ 测试」= POST /api/tasks/:id/run 马上跑一次；
 * - 新建与编辑复用同一弹窗：名称/cron（含预设）/脚本/设备/运行参数（ParamsForm 稀疏 args）；
 * - 保存按 ADR-12 形状提交：runner 嵌套（gamer.yaml + script_id + payload.args），
 *   schedule = {provider_id:'cron', config:{expression}}；启停走 enable/disable 端点。
 * 产品级 TaskBoard 重构（Runner/Provider 动态表单）为下一波任务，本版仅做字段映射。
 */
import { ref, reactive, computed, onMounted } from 'vue'
import { tasksData, scriptsData, devicesData, templatesData, useToast, pushRunConflict } from '../store'
import { api } from '../api'
import ScriptPicker from './ScriptPicker.vue'
import ParamsForm from '../script-editor/components/ParamsForm.vue'
import { extractParams } from '../script-editor/params'
import RunConflictModal from './RunConflictModal.vue'
import { shortRunId, isDeviceBusyConflict } from '../runs'

const props = defineProps({
  // Console 传入当前包名后，任务脚本选择器锁定该分区；独立挂载时保留原有自选分区行为。
  activePkg: { type: String, default: null },
})

const YAML_RUNNER_ID = 'gamer.yaml'
const CRON_PROVIDER_ID = 'cron'

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

// ---- 运行参数：表单态（args = 稀疏覆盖） ----
const taskFormEl = ref(null)
// 任务 runner.payload.args（编辑时整体带入覆盖态）
const taskInitialArgs = ref({})
// 打开弹窗时的脚本 id：切换脚本后任务原 payload 不再适用，清空带入
const openedScriptId = ref('')
const savingTask = ref(false)

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

function onScriptPicked(v) {
  if (v !== openedScriptId.value) {
    taskInitialArgs.value = {}
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

/** 服务端 ADR-12 任务 JSON → 视图行（script_id/cron/args 的最小机械映射）。 */
function adaptTaskRow(t) {
  return {
    id: t.id,
    name: t.name,
    state: t.state,
    enabled: t.enabled,
    cron: t.schedule?.config?.expression ?? '',
    script_id: t.runner?.entrypoint ?? '',
    device_id: t.app?.device_id ?? '',
    android_package: t.app?.android_package ?? '',
    content_package: t.app?.content_package ?? '',
    args: t.runner?.payload?.args ?? {},
  }
}

/** 视图表单 → ADR-12 任务保存 body（android/content 包名取自脚本分区前缀）。 */
function buildTaskSaveBody(args) {
  const android = String(form.script_id || '').split('/')[0] || 'legacy'
  return {
    ...(form.id ? { id: form.id } : {}),
    name: form.name,
    app: {
      device_id: form.device_id,
      android_package: android,
      content_package: android,
    },
    runner: {
      runner_id: YAML_RUNNER_ID,
      entrypoint: form.script_id,
      payload: { args },
    },
    schedule: {
      provider_id: CRON_PROVIDER_ID,
      config: { expression: form.cron },
    },
    enabled: true,
  }
}

function openAdd() {
  editing.value = false
  Object.assign(form, { id: null, name: '', cron: '0 8 * * *', script_id: scripts.value[0]?.id || '', device_id: devices.value[0]?.id || '' })
  taskInitialArgs.value = {}
  openedScriptId.value = form.script_id
  showAdd.value = true
}

function editTask(t) {
  editing.value = true
  Object.assign(form, { id: t.id, name: t.name, cron: t.cron, script_id: t.script_id, device_id: t.device_id })
  openedScriptId.value = t.script_id
  // 先按列表行兜底，再拉任务详情整体带入 payload.args（resolve 语义：本次采用=payload 值）
  taskInitialArgs.value = t.args && typeof t.args === 'object' ? JSON.parse(JSON.stringify(t.args)) : {}
  showAdd.value = true
  api.getTask(t.id).then((detail) => {
    if (form.id !== t.id || !showAdd.value) return // 弹窗已切换/关闭：丢弃迟到响应
    const detailArgs = detail?.runner?.payload?.args
    if (detailArgs && typeof detailArgs === 'object') {
      taskInitialArgs.value = JSON.parse(JSON.stringify(detailArgs))
    }
  }).catch(() => { /* 详情拉取失败：按列表行兜底渲染（默认值字段省略） */ })
}

async function toggle(t, e) {
  try {
    // 启停走显式状态迁移端点：任务参数（runner payload）保持不变
    if (e.target.checked) await api.enableTask(t.id)
    else await api.disableTask(t.id)
    toast(`${t.name} 已${e.target.checked ? '启用' : '停用'}`, 'info')
    loadTasks()
  } catch (err) {
    toast('操作失败：' + err.message, 'error')
    loadTasks()
  }
}

/** ▶ 测试（立即运行）：202 {run_id} 触发即返回并恢复按钮可用。 */
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

/** 保存任务：表单客户端校验（必填缺失/类型不合规阻断）→ ADR-12 body 提交。 */
async function saveTask() {
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
    const existed = !!form.id
    const body = buildTaskSaveBody(args)
    if (existed) {
      body.id = form.id
      await api.updateTask(form.id, body)
    } else {
      await api.saveTask(body)
    }
    showAdd.value = false
    toast(existed ? '任务已保存' : '任务已创建', 'success')
    loadTasks()
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  } finally {
    savingTask.value = false
  }
}

async function loadTasks() {
  try {
    const list = await api.listTasks()
    tasks.value = (Array.isArray(list) ? list : []).map(adaptTaskRow)
  } catch (e) {}
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
/* 服务端时区标识（契约禁止 system/info 带 timezone；P11.1 后时间戳均为 UTC 串） */
.tz-hint {
  display: flex; align-items: center; gap: 6px; flex-wrap: wrap;
  font-size: 12px; color: var(--text-2);
}
.task-name { font-weight: 600; }
/* 依赖缺失标注（dependency_missing：任务保留休眠） */
.stale-tag { margin-left: 6px; color: var(--warn); border-color: var(--warn); font-weight: 400; }
.mono { font-family: var(--mono); }
.row-actions { display: flex; gap: 4px; align-items: center; }
.row-actions .danger:hover { color: var(--danger); border-color: var(--danger); }
tr.disabled { opacity: .45; }
.empty-row { text-align: center; color: var(--text-2); font-size: 12px; padding: 22px 0; }

/* cron 预设（收进弹窗：点一下填入表达式） */
.cron-presets { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 8px; }
.cp-item { font-size: 11px; padding: 3px 10px; }
</style>
