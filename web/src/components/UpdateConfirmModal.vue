<template>
  <div v-if="open" class="modal-mask" @click.self="emit('close')">
    <div class="modal upd-confirm">
      <div class="modal-head">
        <span class="title">{{ isRollback ? '回滚确认' : '安装更新确认' }}</span>
        <button class="btn btn-ghost btn-sm" :disabled="submitting" @click="emit('close')">✕</button>
      </div>
      <div class="modal-body">
        <div class="kv">
          <div class="kv-row"><span class="k">当前版本</span><span class="v">{{ curVersion }}</span></div>
          <template v-if="!isRollback">
            <div class="kv-row">
              <span class="k">目标版本</span>
              <span class="v strong">{{ candidate ? candidate.version : '（无候选信息）' }}</span>
            </div>
            <div v-if="candidate" class="kv-row">
              <span class="k">渠道 / 大小</span>
              <span class="v">{{ candidate.channel }} · {{ formatBytes(candidate.size_bytes) }}</span>
            </div>
          </template>
          <template v-else>
            <div class="kv-row">
              <span class="k">回滚目标</span>
              <span class="v strong">上一个稳定版本（previous）</span>
            </div>
          </template>
          <div v-if="schema" class="kv-row">
            <span class="k">数据 schema</span>
            <span class="v">数据库 v{{ schema.db }} · 文件布局 v{{ schema.file }} · 自动回滚下限 v{{ schema.rollback_floor }}</span>
          </div>
          <div v-if="policy && !isRollback" class="kv-row">
            <span class="k">维护窗口</span>
            <span class="v">{{ policyText }}</span>
          </div>
        </div>

        <div class="warn-box">
          <p v-if="!isRollback">
            安装过程中服务将重启：投屏连接会断开、运行中的脚本会中断；安装失败会自动回滚到升级前快照。请确认当前没有正在执行的任务。
          </p>
          <p v-else>
            回滚过程中服务将重启：数据库与文件将恢复到升级前快照，升级后产生的数据变更将丢失。请确认当前没有正在执行的任务。
          </p>
          <p v-if="policy && !isRollback && policy.strategy === 'auto'">
            当前为 auto 策略：手动安装将优先于维护窗口等待，立即执行。
          </p>
        </div>

        <!-- 同步拒绝/异步失败展示：错误码 + blocking 门禁列表（update_not_ready 等） -->
        <div v-if="error" class="err-box">
          <p class="err-msg">{{ error.message || '操作失败' }}</p>
          <p v-if="error.code" class="err-code">错误码：{{ error.code }}</p>
          <ul v-if="blocking.length" class="blocking">
            <li v-for="b in blocking" :key="b">{{ b }}</li>
          </ul>
        </div>
      </div>
      <div class="modal-foot">
        <button class="btn" :disabled="submitting" @click="emit('close')">取消</button>
        <button
          :class="isRollback ? 'btn btn-danger' : 'btn btn-primary'"
          :disabled="submitting"
          data-testid="confirm-btn"
          @click="emit('confirm')"
        >{{ confirmLabel }}</button>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * 安装/回滚确认弹窗（WEB-004）：
 * 确认项：目标版本、channel、包大小、data_schema/rollback_floor、维护窗口提示，
 * 以及「服务将重启/投屏与脚本会中断」的固定警示（契约 §4.1：断连是正常路径）。
 * 同步拒绝（409 update_not_ready / rollback_unavailable 等）经 error prop 回填展示：
 * 错误码 + message + details.blocking 中文门禁列表。提交本身由宿主经 useUpdateFlow 执行。
 */
import { computed } from 'vue'
import { formatBytes, blockingLabels, STRATEGY_LABELS } from '../system/states'

const props = defineProps({
  open: { type: Boolean, default: false },
  mode: { type: String, default: 'install' },  // 'install' | 'rollback'
  info: { type: Object, default: null },       // GET /api/system/info（当前版本/schema 源）
  status: { type: Object, default: null },     // GET /api/system/update（candidate/policy 源）
  submitting: { type: Boolean, default: false },
  error: { type: Object, default: null },      // 归一化错误 {code, message, details}
})

const emit = defineEmits(['confirm', 'close'])

const isRollback = computed(() => props.mode === 'rollback')
const candidate = computed(() => (props.status && props.status.candidate) || null)
const policy = computed(() => (props.status && props.status.policy) || null)
const schema = computed(() => (props.info && props.info.schema) || null)
const curVersion = computed(() => (props.info && props.info.app && props.info.app.version) || '—')
const blocking = computed(() =>
  blockingLabels(props.error && props.error.details && props.error.details.blocking))
const confirmLabel = computed(() => {
  if (props.submitting) return '提交中…'
  return isRollback.value ? '确认回滚' : '确认安装'
})
const policyText = computed(() => {
  const p = policy.value
  if (!p) return ''
  const win = p.maintenance_window && p.maintenance_window.start && p.maintenance_window.end
    ? `${p.maintenance_window.start}–${p.maintenance_window.end}`
    : '未设置'
  return `${STRATEGY_LABELS[p.strategy] || p.strategy} · 窗口 ${win} · 定时任务冻结窗口 ${p.freeze_window_minutes} 分钟`
})
</script>

<style scoped>
.upd-confirm { width: 500px; max-width: calc(100vw - 32px); }
.kv { display: flex; flex-direction: column; gap: 8px; }
.kv-row { display: flex; gap: 12px; font-size: 13px; }
.kv-row .k { flex-shrink: 0; width: 84px; color: var(--text-2); font-size: 12px; padding-top: 1px; }
.kv-row .v { color: var(--text-0); word-break: break-all; }
.kv-row .v.strong { font-weight: 700; font-family: var(--mono); }
.warn-box {
  border: 1px solid rgba(251, 191, 36, .35); border-radius: var(--radius-sm);
  background: rgba(251, 191, 36, .06); padding: 8px 12px;
}
.warn-box p { font-size: 12px; color: var(--warn); line-height: 1.7; }
.warn-box p + p { margin-top: 4px; }
.err-box {
  border: 1px solid rgba(248, 113, 113, .35); border-radius: var(--radius-sm);
  background: rgba(248, 113, 113, .06); padding: 8px 12px;
  display: flex; flex-direction: column; gap: 4px;
}
.err-box .err-msg { font-size: 12px; color: var(--danger); line-height: 1.6; }
.err-box .err-code { font-size: 12px; color: var(--text-2); font-family: var(--mono); }
.blocking { margin: 2px 0 0 16px; }
.blocking li { font-size: 12px; color: var(--text-1); line-height: 1.7; }
</style>
