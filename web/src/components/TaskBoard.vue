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
            <th style="width: 118px">状态</th>
            <th style="width: 68px">启用</th>
            <th style="width: 230px">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="t in tasks" :key="t.id" :class="{ disabled: !t.enabled }">
            <td class="task-name">{{ t.name }}</td>
            <td>
              <span class="tag" :class="stateTagClass(t.state)" data-testid="state-badge">{{ stateLabel(t.state) }}</span>
              <div v-if="missingDependency(t)" class="dep-hint" data-testid="dep-hint" :title="t.suspend_reason">
                缺少依赖：{{ missingDependency(t) }}
              </div>
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
                <button class="btn btn-sm btn-ghost" title="编辑任务" @click="editTask(t)">✎</button>
                <button v-if="t.state === 'active'" class="btn btn-sm btn-ghost" title="挂起调度（任务保留，可恢复）" @click="suspendRow(t)">⏸</button>
                <button v-if="t.state === 'suspended' || t.state === 'dependency_missing'" class="btn btn-sm btn-ghost" title="恢复调度（重算唤醒时间）" @click="resumeRow(t)">↻</button>
                <button v-if="t.state !== 'cancelled'" class="btn btn-sm btn-ghost" title="取消调度（终态，不再排程；任务记录保留）" @click="cancelRow(t)">✕</button>
                <button class="btn btn-sm btn-ghost danger" title="删除任务" @click="removeTask(t)">🗑</button>
              </div>
            </td>
          </tr>
          <tr v-if="!tasks.length">
            <td colspan="4" class="empty-row">暂无定时任务，点上方「＋ 新建任务」创建。</td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 新建/编辑弹窗（新建与编辑复用）：名称/设备/触发方式/执行器/执行目标/参数/启用 -->
    <div v-if="showAdd" class="modal-mask" @click.self="showAdd = false">
      <div class="modal">
        <div class="modal-head">
          <span class="title">{{ form.id ? '编辑任务' : '新建任务' }}</span>
          <button class="btn btn-ghost btn-sm" @click="showAdd = false">✕</button>
        </div>
        <div class="modal-body">
          <div class="form-item">
            <label>任务名称</label>
            <input v-model="form.name" class="input" placeholder="例如：每日签到" />
          </div>

          <div class="form-item">
            <label>设备</label>
            <select v-model="form.device_id" class="select" data-testid="device-select">
              <option value="" disabled>选择设备…</option>
              <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }}</option>
            </select>
          </div>

          <div class="form-item">
            <label>触发方式（Schedule Provider）</label>
            <select v-model="providerSelect" class="select" data-testid="provider-select">
              <option v-for="p in providers" :key="p.provider_id" :value="p.provider_id">{{ p.provider_id }}</option>
              <option value="__manual__">手动输入 provider…</option>
            </select>
            <!-- 降级输入：provider 未注册/列表为空，或显式选择手填 -->
            <input
              v-if="providerManual"
              v-model="form.providerId"
              class="input mono"
              style="margin-top: 6px"
              placeholder="provider_id，例如 thirdparty.calendar"
              data-testid="manual-provider-input"
            />
            <template v-if="isCron">
              <input v-model="form.cronExpr" class="input mono" style="margin-top: 6px" placeholder="0 8 * * *" data-testid="cron-input" />
              <div class="cron-presets">
                <button v-for="p in cronPresets" :key="p.cron" class="btn btn-sm cp-item" type="button" @click="form.cronExpr = p.cron">{{ p.name }}</button>
              </div>
            </template>
            <template v-else>
              <label class="sub-label">provider config（JSON）</label>
              <textarea
                v-model="form.configJson"
                class="input mono"
                rows="3"
                placeholder="{}"
                data-testid="config-json"
              ></textarea>
            </template>
          </div>

          <div class="form-item">
            <label>执行器（Runner）</label>
            <select v-model="form.runnerId" class="select" data-testid="runner-select" @change="onRunnerChange">
              <option v-for="opt in runnerOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
            </select>
          </div>

          <!-- 执行目标 / 参数：由 runner 对应的编辑器贡献渲染（TaskBoard 不感知具体 runner 语义） -->
          <template v-if="currentContrib">
            <div class="form-item">
              <label>执行目标</label>
              <component
                :is="currentContrib.entrypointEditor"
                v-if="currentContrib.entrypointEditor"
                v-model="form.entrypoint"
                v-bind="entrypointEditorProps"
              />
              <select v-else v-model="form.entrypoint" class="select" data-testid="entrypoint-select">
                <option value="" disabled>选择执行目标…</option>
                <option v-for="opt in entrypointOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
              </select>
            </div>
            <div class="form-item">
              <label>参数（由执行器声明）</label>
              <component
                :is="currentContrib.payloadEditor"
                ref="payloadEditorEl"
                :entrypoint="form.entrypoint"
                :payload="form.payload"
                :ctx="editorCtx"
                @update:payload="form.payload = $event"
              />
            </div>
          </template>
          <template v-else>
            <div class="form-item">
              <label>执行目标 / 参数</label>
              <div class="runner-missing" data-testid="runner-missing-placeholder">
                {{ runnerKnownToServer
                  ? '该执行器未提供编辑器（未安装对应扩展）。'
                  : '未知执行器：未安装对应扩展。entrypoint/payload 将原样保留，其他字段可修改，任务仍可保存。' }}
              </div>
              <pre class="runner-json" data-testid="runner-json">{{ runnerJsonText }}</pre>
            </div>
          </template>

          <div class="form-item form-switch">
            <label>启用</label>
            <label class="switch">
              <input v-model="form.enabled" type="checkbox" data-testid="enabled-switch" />
              <span class="track"></span>
            </label>
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
 * Console 右侧任务页签：ADR-12 通用任务表单（P11.1 §6.6/§6.7 产品级重写）。
 *
 * Task = 任意 ScheduleProvider + 任意 Runner，TaskBoard 只认抽象字段：
 * - 触发方式：provider 下拉来自 GET /api/schedule-providers；内置 cron provider
 *   渲染表达式输入 + 快捷预设 chips；provider 未注册/列表为空降级为手填
 *   provider_id + config JSON；
 * - 执行器：runner 下拉来自 GET /api/runners（有编辑器贡献的显示 title）；
 *   执行目标/参数由 RunnerEditorContribution 注册表渲染（见 ./task/runner-editors.ts）。
 *   本组件不直接依赖任何业务资源组件、不读 store 的脚本/模板数据——资源获取与
 *   参数编辑全是贡献的内部事务；
 * - 未注册贡献的 runner：占位提示 + runner JSON 只读展示，其他字段可改、任务仍可保存；
 * - 状态：active/suspended/dependency_missing/cancelled 徽标；dependency_missing
 *   显示 missing_dependency 提示与「恢复」动作；run/suspend/resume/cancel/
 *   enable/disable/delete 动作齐全。
 */
import { ref, reactive, computed, onMounted } from 'vue'
import { tasksData, devicesData, useToast, pushRunConflict } from '../store'
import { api } from '../api'
import RunConflictModal from './RunConflictModal.vue'
import { shortRunId, isDeviceBusyConflict } from '../runs'
import {
  getRunnerEditor, listRunnerEditors,
} from './task/runner-editors'
import { registerBuiltinRunnerEditors } from './task/builtin-runner-editors'

const props = defineProps({
  // Console 传入当前包名后，任务执行目标选择器锁定该分区；独立挂载时保留自选分区行为。
  activePkg: { type: String, default: null },
})

const CRON_PROVIDER_ID = 'cron'
const MANUAL_PROVIDER = '__manual__'

const toast = useToast()
const tasks = tasksData
const devices = devicesData
const providers = ref([])
const runners = ref([])
const showAdd = ref(false)
const savingTask = ref(false)
// 测试（立即执行）触发中（行级防重复点击）；202 一返回即复位——不等任务完成
const triggeringId = ref('')

// ---- 表单态（触发方式与执行器拆为通用字段 + 结构化编辑字段） ----
const form = reactive({
  id: null,
  name: '',
  enabled: true,
  device_id: '',
  providerId: CRON_PROVIDER_ID,
  cronExpr: '0 8 * * *',
  configJson: '{}',
  runnerId: '',
  entrypoint: '',
  payload: {},
})
// 编辑既有任务时保留原 app 包名（runner 无法推导包名时回退，不因表单重写丢数据）
const originalApp = ref(null)
// 触发方式降级手填模式（provider 未注册/列表为空/显式选择手填）
const manualProvider = ref(false)

// ---- RunnerEditorContribution 注册表（内置贡献随组件装配注册） ----
const unregisterEditors = registerBuiltinRunnerEditors()

const currentContrib = computed(() => getRunnerEditor(form.runnerId))
const runnerKnownToServer = computed(() => runners.value.some((r) => r.runner_id === form.runnerId))
const editorCtx = computed(() => ({ androidPackage: props.activePkg ?? null, deviceId: form.device_id }))
const entrypointEditorProps = computed(() => currentContrib.value?.entrypointEditorProps?.(editorCtx.value) ?? {})

const payloadEditorEl = ref(null)

/** runner 下拉选项：服务端已注册 + 前端有贡献 + 当前任务值（未知 runner 保持可见）。 */
const runnerOptions = computed(() => {
  const ids = new Set(runners.value.map((r) => r.runner_id))
  for (const c of listRunnerEditors()) ids.add(c.runnerId)
  if (form.runnerId) ids.add(form.runnerId)
  return [...ids].sort((a, b) => a.localeCompare(b)).map((id) => ({
    value: id,
    label: getRunnerEditor(id)?.title || id,
  }))
})

// ---- 通用执行目标候选（贡献未提供 entrypointEditor 组件时走原生下拉） ----
const entrypointOptions = ref([])
async function loadEntrypointOptions() {
  entrypointOptions.value = []
  const contrib = currentContrib.value
  if (!contrib?.entrypoints) return
  try {
    const opts = await contrib.entrypoints(editorCtx.value)
    if (currentContrib.value !== contrib) return // 迟到结果：执行器已切换
    entrypointOptions.value = Array.isArray(opts) ? opts : []
  } catch {
    entrypointOptions.value = []
  }
}

/** 换执行器：原 entrypoint/payload 语义不再适用，清空（贡献会带出新目标的默认态）。 */
function onRunnerChange() {
  void loadEntrypointOptions()
  if (!currentContrib.value) return
  form.entrypoint = ''
  form.payload = {}
}

// ---- 触发方式（Schedule Provider） ----
const isCron = computed(() => form.providerId === CRON_PROVIDER_ID)
const providerKnown = computed(() => providers.value.some((p) => p.provider_id === form.providerId))
const providerManual = computed(() => manualProvider.value || !providerKnown.value)

const providerSelect = computed({
  get: () => (providerManual.value ? MANUAL_PROVIDER : form.providerId),
  set: (v) => {
    if (v === MANUAL_PROVIDER) {
      manualProvider.value = true
      return
    }
    manualProvider.value = false
    form.providerId = v
  },
})

/** 非 cron 触发方式的 config 解析（降级/通用形态；空串视为空对象）。 */
function parseScheduleConfig() {
  const text = String(form.configJson ?? '').trim() || '{}'
  try {
    const parsed = JSON.parse(text)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { error: '必须是 JSON 对象' }
    }
    return { config: parsed }
  } catch (e) {
    return { error: e.message }
  }
}

/** cron 预设（写入 schedule.config.expression；点一下填入表达式）。 */
const cronPresets = [
  { name: '每分钟', cron: '* * * * *' },
  { name: '每 10 分钟', cron: '*/10 * * * *' },
  { name: '每小时', cron: '0 * * * *' },
  { name: '每天 8:00', cron: '0 8 * * *' },
  { name: '每天 12:30', cron: '30 12 * * *' },
  { name: '每天 21:00', cron: '0 21 * * *' },
  { name: '每周一 9:00', cron: '0 9 * * 1' },
]

// ---- 状态呈现 ----
const STATE_LABELS = { active: '调度中', suspended: '已挂起', dependency_missing: '依赖缺失', cancelled: '已取消' }
function stateLabel(state) {
  return STATE_LABELS[state] || state || '—'
}
function stateTagClass(state) {
  if (state === 'active') return 'ok'
  if (state === 'dependency_missing') return 'warn'
  if (state === 'cancelled') return 'err'
  return 'info'
}
/** suspend_reason = "missing_dependency=<runner_id 或 provider_id>" → 提示 id。 */
function missingDependency(t) {
  const reason = String(t?.suspend_reason ?? '')
  return reason.startsWith('missing_dependency=') ? reason.slice('missing_dependency='.length) : ''
}

// ---- 列表行（保留服务端原始形状，编辑直接回填） ----
function adaptTaskRow(t) {
  return {
    id: t.id,
    name: t.name,
    enabled: !!t.enabled,
    state: t.state ?? '',
    suspend_reason: t.suspend_reason ?? '',
    app: t.app && typeof t.app === 'object' ? t.app : {},
    runner: t.runner && typeof t.runner === 'object'
      ? t.runner
      : { runner_id: '', entrypoint: '', payload: {} },
    schedule: t.schedule && typeof t.schedule === 'object'
      ? t.schedule
      : { provider_id: '', config: {} },
  }
}

/** 未注册贡献的 runner：runner JSON 只读展示（entrypoint/payload 原样保留可保存）。 */
const runnerJsonText = computed(() => JSON.stringify({
  runner_id: form.runnerId,
  entrypoint: form.entrypoint,
  payload: form.payload,
}, null, 2))

// ---- 表单填充 / 保存 ----
function defaultRunnerId() {
  const listed = runners.value.map((r) => r.runner_id)
  const contributed = listRunnerEditors().map((c) => c.runnerId)
  return listed.find((id) => contributed.includes(id)) || listed[0] || contributed[0] || ''
}

function resetForm() {
  Object.assign(form, {
    id: null,
    name: '',
    enabled: true,
    device_id: devices.value[0]?.id ?? '',
    providerId: CRON_PROVIDER_ID,
    cronExpr: '0 8 * * *',
    configJson: '{}',
    runnerId: defaultRunnerId(),
    entrypoint: '',
    payload: {},
  })
  originalApp.value = null
  manualProvider.value = false
}

function openAdd() {
  resetForm()
  showAdd.value = true
  void loadEntrypointOptions()
}

/** 列表行（服务端 ADR-12 全量 JSON）→ 表单。 */
function fillForm(t) {
  const schedule = t.schedule ?? {}
  const config = schedule.config ?? {}
  Object.assign(form, {
    id: t.id,
    name: t.name,
    enabled: !!t.enabled,
    device_id: t.app?.device_id ?? '',
    providerId: schedule.provider_id || '',
    cronExpr: schedule.provider_id === CRON_PROVIDER_ID ? String(config.expression ?? '') : '',
    configJson: JSON.stringify(config && typeof config === 'object' ? config : {}, null, 2),
    runnerId: t.runner?.runner_id ?? '',
    entrypoint: t.runner?.entrypoint ?? '',
    payload: t.runner?.payload && typeof t.runner.payload === 'object' ? t.runner.payload : {},
  })
  originalApp.value = {
    android_package: t.app?.android_package ?? '',
    content_package: t.app?.content_package ?? null,
  }
  manualProvider.value = false
}

function editTask(t) {
  fillForm(t)
  showAdd.value = true
  void loadEntrypointOptions()
}

/** 视图表单 → ADR-12 保存 body。app 包名优先由贡献推导，回退既有任务原值。 */
function buildTaskSaveBody() {
  const contrib = currentContrib.value
  const app = contrib?.resolveAppPackages?.(form.entrypoint, form.payload, editorCtx.value) ?? originalApp.value
  const config = isCron.value
    ? { expression: form.cronExpr.trim() }
    : parseScheduleConfig().config ?? {}
  return {
    ...(form.id ? { id: form.id } : {}),
    name: form.name.trim(),
    enabled: form.enabled,
    app: {
      device_id: form.device_id,
      android_package: app?.android_package ?? '',
      content_package: app?.content_package ?? null,
    },
    runner: {
      runner_id: form.runnerId,
      entrypoint: form.entrypoint,
      payload: form.payload && typeof form.payload === 'object' ? form.payload : {},
    },
    schedule: {
      provider_id: form.providerId,
      config,
    },
  }
}

/** 保存任务：客户端校验（必填/JSON/贡献 validate）→ ADR-12 body 提交。 */
async function saveTask() {
  if (!form.name.trim()) return toast('请填写任务名称', 'error')
  if (!form.device_id) return toast('请选择设备', 'error')
  if (!form.providerId.trim()) return toast('请填写 provider_id', 'error')
  if (isCron.value && !form.cronExpr.trim()) return toast('请填写 cron 表达式', 'error')
  if (!isCron.value) {
    const parsed = parseScheduleConfig()
    if (parsed.error) return toast('provider config JSON 解析失败：' + parsed.error, 'error')
  }
  if (!form.entrypoint.trim()) return toast('请选择执行目标', 'error')
  if (currentContrib.value && payloadEditorEl.value) {
    const issues = payloadEditorEl.value.validate?.() ?? []
    if (issues.length) {
      return toast('执行器参数校验未通过：' + issues.map((i) => (i.name ? `$${i.name} ${i.message}` : i.message)).join('；'), 'error')
    }
  }
  savingTask.value = true
  try {
    const existed = !!form.id
    const body = buildTaskSaveBody()
    if (existed) await api.updateTask(form.id, body)
    else await api.saveTask(body)
    showAdd.value = false
    toast(existed ? '任务已保存' : '任务已创建', 'success')
    loadTasks()
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  } finally {
    savingTask.value = false
  }
}

// ---- 行内动作 ----
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
      pushRunConflict({ ...(e.data || {}), device_id: t.app?.device_id ?? '' })
    } else {
      toast('触发失败：' + e.message, 'error')
    }
  } finally {
    triggeringId.value = ''
  }
}

async function suspendRow(t) {
  try {
    await api.suspendTask(t.id, 'suspended')
    toast(`${t.name} 已挂起`, 'info')
    loadTasks()
  } catch (e) {
    toast('挂起失败：' + e.message, 'error')
  }
}

async function resumeRow(t) {
  try {
    await api.resumeTask(t.id)
    toast(`${t.name} 已恢复调度`, 'success')
    loadTasks()
  } catch (e) {
    toast('恢复失败：' + e.message, 'error')
  }
}

async function cancelRow(t) {
  if (!confirm(`取消任务 ${t.name} 的调度？（终态，不再排程；任务记录保留）`)) return
  try {
    await api.cancelTask(t.id)
    toast(`${t.name} 已取消调度`, 'info')
    loadTasks()
  } catch (e) {
    toast('取消失败：' + e.message, 'error')
  }
}

async function removeTask(t) {
  if (!confirm(`删除任务 ${t.name}？`)) return
  try {
    await api.deleteTask(t.id)
    tasks.value = tasks.value.filter((x) => x.id !== t.id)
    toast('任务已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

// ---- 数据加载（只拉任务/设备/provider/runner；脚本与模板归编辑器贡献管） ----
async function loadTasks() {
  try {
    const list = await api.listTasks()
    tasks.value = (Array.isArray(list) ? list : []).map(adaptTaskRow)
  } catch (e) {}
}
async function loadDevices() {
  try { devices.value = await api.listDevices() } catch (e) {}
}
async function loadScheduleProviders() {
  try { providers.value = await api.listScheduleProviders() } catch (e) { providers.value = [] }
}
async function loadRunners() {
  try { runners.value = await api.listRunners() } catch (e) { runners.value = [] }
}

onMounted(() => {
  loadDevices()
  loadScheduleProviders()
  loadRunners()
  loadTasks()
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
.dep-hint { font-size: 11px; color: var(--warn); margin-top: 2px; font-weight: 400; }
.mono { font-family: var(--mono); }
.sub-label { font-size: 11px; color: var(--text-2); margin-top: 6px; }
.row-actions { display: flex; gap: 4px; align-items: center; }
.row-actions .danger:hover { color: var(--danger); border-color: var(--danger); }
tr.disabled { opacity: .45; }
.empty-row { text-align: center; color: var(--text-2); font-size: 12px; padding: 22px 0; }

/* 未注册贡献的 runner：占位提示 + 只读 JSON */
.runner-missing {
  font-size: 12px; color: var(--warn);
  border: 1px dashed var(--border); border-radius: var(--radius-sm);
  padding: 6px 8px;
}
.runner-json {
  margin: 6px 0 0; padding: 6px 8px;
  font-family: var(--mono); font-size: 11px; color: var(--text-2);
  background: var(--bg-0); border: 1px solid var(--border); border-radius: var(--radius-sm);
  max-height: 140px; overflow: auto; white-space: pre-wrap; word-break: break-all;
}

/* cron 预设（点一下填入表达式） */
.cron-presets { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 8px; }
.cp-item { font-size: 11px; padding: 3px 10px; }
.form-switch { flex-direction: row; align-items: center; gap: 10px; }
</style>
