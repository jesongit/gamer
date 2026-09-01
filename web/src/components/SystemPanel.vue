<template>
  <div class="system-panel">
    <div class="sp-head">
      <div>
        <div class="sp-title">系统与更新</div>
        <div class="sp-sub">系统信息、软件更新与更新策略；数据与操作均对接服务端真实 API（/api/system/*，更新策略持久化在服务端数据目录）</div>
      </div>
      <button class="btn btn-primary" :disabled="st.loading" @click="ctl.refresh()">
        {{ st.loading ? '读取中…' : '↻ 刷新' }}
      </button>
    </div>

    <div v-if="st.loading && !st.info && !st.update" class="card state-card" role="status">
      <div class="state-icon">⏳</div>
      <div>
        <div class="state-title">正在读取系统状态</div>
        <div class="state-desc">正在向服务端请求系统信息与更新状态。</div>
      </div>
    </div>

    <template v-else>
      <SystemInfoCard class="stack-gap" :info="st.info" :error="infoErrorText"
        @check="onCheck" @install="requestInstall" />

      <div class="two-col">
        <UpdateStatusCard :status="st.update" :info="st.info" :busy="flowBusy"
          @action="onAction" @refresh="ctl.refresh()" />

        <!-- 更新策略（PUT /api/system/update/policy，整对象替换；能力全 false 时仍可保存，契约 §6） -->
        <section class="card policy-card" data-testid="policy-card">
          <div class="card-head">
            <span class="card-title">更新策略</span>
          </div>

          <template v-if="policyAvailable">
            <fieldset class="policy-form" :disabled="savingPolicy" @change="onPolicyEdit">
              <label v-for="opt in STRATEGY_OPTIONS" :key="opt.value" class="strategy-row">
                <input type="radio" name="update-strategy" :value="opt.value"
                  v-model="policyForm.strategy" data-testid="policy-strategy" />
                <span>
                  <b>{{ opt.label }}</b>
                  <small>{{ opt.desc }}</small>
                </span>
              </label>

              <div class="window-fields" :class="{ dim: policyForm.strategy !== 'auto' }">
                <span>维护窗口（仅 auto 使用）</span>
                <input type="time" v-model="policyForm.start" data-testid="window-start" />
                <span>–</span>
                <input type="time" v-model="policyForm.end" data-testid="window-end" />
                <label class="freeze">
                  冻结窗口
                  <input type="number" min="0" max="1440" v-model.number="policyForm.freeze"
                    data-testid="freeze-window" />
                  分钟
                </label>
              </div>

              <p v-if="policyForm.strategy === 'auto'" class="gate-note" data-testid="gate-note">
                auto 门禁：仅维护窗口内、无运行中脚本、无其他升级/维护事务、且距下一次定时任务触发大于冻结窗口时才自动安装；
                不满足则一直等待，不会中断运行中的脚本。窗口允许跨午夜（如 23:00–05:00）。
              </p>

              <p v-if="caps && !caps.install" class="caps-note">
                当前部署不受升级器托管（update_not_managed）：策略可保存，但不会产生任何自动更新行为。
              </p>

              <div class="policy-foot">
                <button class="btn btn-primary btn-sm" data-testid="policy-save"
                  :disabled="savingPolicy" @click="savePolicy">
                  {{ savingPolicy ? '保存中…' : '保存策略' }}
                </button>
                <span v-if="policyNote" class="save-note" data-testid="policy-note">{{ policyNote }}</span>
                <span v-if="policyError" class="save-error" data-testid="policy-error">{{ policyError }}</span>
              </div>
            </fieldset>
          </template>

          <div v-else class="policy-unavailable">
            <p>更新状态接口暂不可用，无法编辑更新策略。</p>
            <p v-if="updateErrorText" class="muted">{{ updateErrorText }}</p>
            <button class="btn btn-sm" @click="ctl.refresh()">重试</button>
          </div>
        </section>
      </div>
    </template>

    <!-- 安装/回滚确认 + 202 断连容忍流程（useUpdateFlow） -->
    <UpdateConfirmModal :open="confirmOpen" :mode="confirmMode" :info="st.info" :status="st.update"
      :submitting="flowBusy" :error="confirmError" @confirm="onConfirm" @close="closeConfirm" />
  </div>
</template>

<script setup>
/**
 * Console 右侧设置页签内容（系统与更新功能全量保留）：
 * - SystemInfoCard：/api/system/info 展示（useSystemStatus 提供数据，页签卸载自动停止轮询）；
 * - UpdateStatusCard：11 状态展示 + 动作按钮；check/download 直接提交，install/rollback 先过
 *   §4.2 状态×动作受理矩阵（canAct）再弹 UpdateConfirmModal，确认后走 useUpdateFlow 完整流：
 *   202 受理 → 断连容忍轮询 → 重连按 app.version / boot_id 判定成功/失败/人工恢复/超时；
 * - 策略编辑：off/notify/auto + 维护窗口 + 冻结窗口，PUT /api/system/update/policy 整对象替换；
 *   轮询不会回填覆盖编辑中的表单（仅首次与保存回显时同步服务端值）。
 */
import { computed, reactive, ref, watch } from 'vue'
import SystemInfoCard from './SystemInfoCard.vue'
import UpdateStatusCard from './UpdateStatusCard.vue'
import UpdateConfirmModal from './UpdateConfirmModal.vue'
import { useToast } from '../store'
import { systemApi, SYSTEM_ERRORS } from '../system/api'
import { allowedActions } from '../system/states'
import { useSystemStatus } from '../system/useSystemStatus'
import { createUpdateFlow } from '../system/useUpdateFlow'

const toast = useToast()

// ---- 系统信息 + 更新状态：页签级轮询（活跃更新高频 / 驻留低频，卸载自动停止） ----
const ctl = useSystemStatus()
const st = ctl.st
const infoErrorText = computed(() => (st.infoError && (st.infoError.message || '系统信息加载失败')) || '')
const updateErrorText = computed(() => (st.updateError && (st.updateError.message || '更新状态加载失败')) || '')
const caps = computed(() => (st.info && st.info.capabilities) || null)

// ---- 动作分发（check/download 直提交；install/rollback 先过矩阵再进确认弹窗） ----
function errHint(e) {
  const known = e && SYSTEM_ERRORS[e.code]
  return known ? `${known.hint}（${e.code}）` : (e && e.message) || '操作失败'
}

async function onCheck() {
  try {
    await systemApi.checkUpdate()
    toast('已受理检查更新', 'info')
  } catch (e) {
    toast(errHint(e), 'error')
  }
  ctl.refresh()
}

async function onDownload() {
  try {
    await systemApi.downloadUpdate()
    toast('已受理后台下载', 'info')
  } catch (e) {
    toast(errHint(e), 'error')
  }
  ctl.refresh()
}

/** §4.2 状态×动作受理矩阵 ∧ capabilities：install/rollback 是否允许进确认弹窗 */
const canAct = computed(() => {
  const none = { install: false, rollback: false }
  if (!st.update) return none
  const a = allowedActions(st.update.state)
  const c = caps.value || {}
  return {
    install: !!a.install && c.install === true,
    rollback: !!a.rollback && c.rollback === true,
  }
})

function onAction(name) {
  if (name === 'check') return onCheck()
  if (name === 'download') return onDownload()
  if (name === 'install') return requestInstall()
  if (name === 'rollback') return requestRollback()
}

// ---- 安装/回滚确认流：确认弹窗 → 202 → 断连容忍 → 重连按版本/boot_id 判定 ----
const flowCtl = createUpdateFlow()
const flow = flowCtl.flow
const flowBusy = computed(() => flow.phase === 'submitting' || flow.phase === 'waiting')

const confirmOpen = ref(false)
const confirmMode = ref('install')

function requestInstall() {
  if (!canAct.value.install) { toast('当前没有可安装的更新候选，请先检查更新', 'error'); return }
  confirmMode.value = 'install'
  flowCtl.reset()
  confirmOpen.value = true
}

function requestRollback() {
  if (!canAct.value.rollback) { toast('当前没有可用的自动回滚点', 'error'); return }
  confirmMode.value = 'rollback'
  flowCtl.reset()
  confirmOpen.value = true
}

function closeConfirm() {
  confirmOpen.value = false
  flowCtl.reset()
}

async function onConfirm() {
  const info = st.info
  const r = confirmMode.value === 'rollback'
    ? await flowCtl.submitRollback(info)
    : await flowCtl.submitInstall(info)
  // submit 的 ok:true 只表示「已受理且等待循环结束」；终态结论以 flow.verdict 为准——
  // failed / manual_recovery / timeout 时弹窗保持打开，错误经 confirmError 回显
  if (!r.ok || flow.verdict !== 'success') return
  confirmOpen.value = false
  flowCtl.reset()
  toast(confirmMode.value === 'rollback' ? '回滚完成：已恢复上一版本' : '安装完成：服务已切换到新版本', 'success')
  ctl.refresh()
}

const confirmError = computed(() => {
  if (flow.verdict === 'timeout') {
    return {
      code: 'update_wait_timeout',
      message: '等待超时：有界重连时间内服务未恢复，请确认服务进程状态后重试或刷新页面。',
      details: null,
    }
  }
  return flow.error
})

// ---- 更新策略（PUT /api/system/update/policy；契约 §6：400 invalid_argument → details.field） ----
const TIME_RE = /^([01]\d|2[0-3]):[0-5]\d$/
const policyForm = reactive({ strategy: 'notify', start: '02:00', end: '06:00', freeze: 30 })
const STRATEGY_OPTIONS = [
  { value: 'off', label: 'off · 关闭', desc: '不检查更新' },
  { value: 'notify', label: 'notify · 通知确认', desc: '自动检查并提示，由你确认后再安装（产品默认）' },
  { value: 'auto', label: 'auto · 自动安装', desc: '自动下载，并在维护窗口与空闲门禁满足后自动安装' },
]
const policyAvailable = computed(() => !!st.update)

let policyHydrated = false
watch(() => st.update && st.update.policy, (p) => {
  // 仅首次到达时回填；此后轮询刷新不覆盖用户编辑中的表单
  if (p && !policyHydrated) applyPolicy(p)
})

function applyPolicy(p) {
  policyHydrated = true
  policyForm.strategy = typeof p.strategy === 'string' ? p.strategy : 'notify'
  const w = p.maintenance_window && typeof p.maintenance_window === 'object' ? p.maintenance_window : {}
  policyForm.start = TIME_RE.test(String(w.start ?? '')) ? w.start : '02:00'
  policyForm.end = TIME_RE.test(String(w.end ?? '')) ? w.end : '06:00'
  const f = Number(p.freeze_window_minutes)
  policyForm.freeze = Number.isInteger(f) && f >= 0 && f <= 1440 ? f : 30
}

const savingPolicy = ref(false)
const policyNote = ref('')
const policyError = ref('')

function onPolicyEdit() {
  policyNote.value = ''
  policyError.value = ''
}

async function savePolicy() {
  policyNote.value = ''
  policyError.value = ''
  const { strategy, start, end } = policyForm
  const freeze = Number(policyForm.freeze)
  if (!TIME_RE.test(String(start)) || !TIME_RE.test(String(end))) {
    policyError.value = '维护窗口时间格式应为 HH:MM（24 小时制）'
    return
  }
  if (start === end) {
    policyError.value = '维护窗口开始与结束时间不能相同'
    return
  }
  if (!Number.isInteger(freeze) || freeze < 0 || freeze > 1440) {
    policyError.value = '冻结窗口须为 0~1440 的整数分钟'
    return
  }
  savingPolicy.value = true
  try {
    const echo = await systemApi.setUpdatePolicy({
      strategy,
      maintenance_window: { start, end },
      freeze_window_minutes: freeze,
    })
    if (echo && typeof echo === 'object') applyPolicy(echo) // 保存回显（200 body = 保存后的策略）
    policyNote.value = '已保存'
    ctl.refresh()
  } catch (e) {
    policyError.value = errHint(e)
  } finally {
    savingPolicy.value = false
  }
}
</script>

<style scoped>
.system-panel { display: flex; flex-direction: column; gap: 14px; flex: 1; min-height: 0; overflow: auto; padding-right: 2px; }
.sp-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; flex-wrap: wrap; }
.sp-title { font-size: 16px; font-weight: 700; }
.sp-sub { font-size: 12px; color: var(--text-2); margin-top: 3px; }
.stack-gap { margin-bottom: 0; }
.two-col { display: grid; grid-template-columns: repeat(auto-fit, minmax(360px, 1fr)); gap: 14px; align-items: start; }

.state-card { display: flex; align-items: center; gap: 14px; min-height: 92px; }
.state-icon { font-size: 24px; }
.state-title { font-size: 14px; font-weight: 700; }
.state-desc { margin-top: 5px; color: var(--text-1); font-size: 12px; line-height: 1.5; }

.policy-card { display: flex; flex-direction: column; gap: 10px; }
.card-head { display: flex; align-items: center; gap: 10px; }
.card-title { font-size: 15px; font-weight: 600; }
.policy-form { border: 0; padding: 0; margin: 0; display: flex; flex-direction: column; gap: 10px; }
.strategy-row {
  display: flex; gap: 9px; align-items: flex-start; cursor: pointer;
  padding: 7px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm);
}
.strategy-row b { font-size: 13px; }
.strategy-row small { display: block; color: var(--text-2); font-size: 11px; margin-top: 2px; line-height: 1.5; }
.window-fields { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-size: 12px; color: var(--text-1); }
.window-fields.dim { opacity: .55; }
.window-fields input[type='time'],
.window-fields input[type='number'] {
  background: var(--bg-3); border: 1px solid var(--border); border-radius: 6px;
  color: var(--text-0); padding: 4px 6px; font-size: 12px;
}
.window-fields input[type='number'] { width: 72px; }
.freeze { display: flex; align-items: center; gap: 6px; }
.gate-note { margin: 0; font-size: 12px; color: var(--text-2); line-height: 1.7; }
.caps-note {
  margin: 0; font-size: 12px; color: var(--warn); line-height: 1.6;
  border: 1px solid rgba(251, 191, 36, .35); border-radius: var(--radius-sm);
  padding: 6px 10px; background: rgba(251, 191, 36, .06);
}
.policy-foot { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.save-note { color: var(--green, #57d38c); font-size: 12px; }
.save-error { color: var(--danger, #ef6b73); font-size: 12px; }
.policy-unavailable { display: flex; flex-direction: column; gap: 8px; font-size: 12px; color: var(--text-1); }
.policy-unavailable p { margin: 0; }
.muted { color: var(--text-2); }

@media (max-width: 640px) {
  .state-card { align-items: flex-start; flex-wrap: wrap; }
  .state-card .btn { margin-left: 38px; }
}
</style>
