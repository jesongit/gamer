<template>
  <div class="card upd-card">
    <div class="card-head">
      <span class="card-title">软件更新</span>
      <span v-if="view" class="tag" :class="meta.tone" data-testid="state-tag">{{ meta.label }}</span>
      <span class="spacer"></span>
      <button class="btn btn-ghost btn-sm" :disabled="busy" @click="emit('refresh')">刷新</button>
    </div>

    <template v-if="view">
      <p class="state-desc">{{ meta.desc }}</p>
      <p v-if="detailLabel" class="detail-line">当前阶段：{{ detailLabel }}</p>
      <p v-if="view.update_id" class="meta-line">
        <span>事务 {{ view.update_id }}</span>
        <span v-if="view.updated_at">更新于 {{ formatLocalTime(view.updated_at) }}</span>
      </p>

      <div v-if="view.candidate" class="cand-row">
        <span class="cand-ver">可更新到 {{ view.candidate.version }}</span>
        <span class="tag info">{{ view.candidate.channel }}</span>
        <span class="meta-text">
          {{ formatBytes(view.candidate.size_bytes) }} · 发布于 {{ formatLocalTime(view.candidate.published_at) }}
        </span>
        <a
          v-if="view.candidate.release_notes_url"
          :href="view.candidate.release_notes_url"
          target="_blank"
          rel="noopener"
        >发布说明</a>
      </div>

      <div v-if="showProgress" class="prog">
        <div class="prog-bar"><div class="prog-in" :style="{ width: pct + '%' }"></div></div>
        <span class="meta-text">
          {{ formatBytes(view.progress.bytes_done) }} / {{ formatBytes(view.progress.bytes_total) }}（{{ pct }}%）
        </span>
      </div>

      <div v-if="view.state === 'failed' && view.last_error" class="err-box">
        <span class="err-code">{{ view.last_error.code }}</span>
        <span>{{ view.last_error.message }}</span>
      </div>

      <!-- manual_recovery：唯一无自动迁出的终态（契约 §5.1），展示恢复指引 + journal 摘要 -->
      <div v-if="view.state === 'manual_recovery'" class="recovery">
        <p class="rec-title">需要人工恢复</p>
        <p class="rec-text">
          升级与自动回滚均已失败，系统已停止全部自动重试。请按维护手册执行人工恢复；
          journal、数据快照与新旧版本证据均已保留，恢复完成后状态将自动复位。
        </p>
        <dl class="journal">
          <div class="j-row"><dt>事务</dt><dd>{{ view.update_id || '—' }}</dd></div>
          <div class="j-row"><dt>阶段</dt><dd>{{ detailLabel || view.detail || '—' }}</dd></div>
          <div class="j-row"><dt>最后错误</dt><dd>{{ view.last_error ? `${view.last_error.code}：${view.last_error.message}` : '—' }}</dd></div>
          <div class="j-row"><dt>状态时间</dt><dd>{{ view.updated_at || '—' }}</dd></div>
        </dl>
      </div>

      <p v-if="caps && !caps.install" class="caps-note">
        当前部署不受升级器托管（update_not_managed）：安装与回滚操作不可用。
      </p>

      <div class="acts">
        <button class="btn btn-sm" data-action="check" :disabled="busy || !can.check" @click="act('check')">检查更新</button>
        <button class="btn btn-sm" data-action="download" :disabled="busy || !can.download" @click="act('download')">下载</button>
        <button class="btn btn-primary btn-sm" data-action="install" :disabled="busy || !can.install" @click="act('install')">立即安装</button>
        <button class="btn btn-danger btn-sm" data-action="rollback" :disabled="busy || !can.rollback" @click="act('rollback')">回滚</button>
      </div>
    </template>

    <div v-else class="empty upd-empty">
      <span class="icon">⟳</span>
      <p>暂无更新状态（尚未加载）</p>
    </div>
  </div>
</template>

<script setup>
/**
 * 更新状态卡片（WEB-003，fixture 驱动）：
 * 渲染契约 §5 的 11 个展示状态（状态文案 + 描述/进度 + 可用动作）；动作按钮可用性严格按
 * §4.2 状态×动作受理矩阵（states.allowedActions），叠加部署能力门禁（info.capabilities，
 * Docker/direct 全 false 时全部禁用并显示 update_not_managed 说明）。
 * - status prop 提供时纯展示（fixture/页面受控模式）；
 * - autoPoll 时自持轮询（useSystemStatus：活跃态高频 / idle 低频，卸载自动停止）；
 * - 动作点击只 emit('action', name)，由宿主经 API 提交（安装/回滚确认走 UpdateConfirmModal）。
 */
import { computed, onMounted, onUnmounted } from 'vue'
import { formatLocalTime } from '../runs'
import { STATE_META, DETAIL_LABELS, allowedActions, formatBytes } from '../system/states'
import { createSystemStatus } from '../system/useSystemStatus'

const props = defineProps({
  status: { type: Object, default: null }, // GET /api/system/update 契约响应；null 时可用 autoPoll 自取
  info: { type: Object, default: null },   // GET /api/system/info 响应（capabilities 门禁源，可选）
  busy: { type: Boolean, default: false }, // 动作请求在途：禁用全部动作按钮
  autoPoll: { type: Boolean, default: false }, // 自持轮询（无外部页面接管时使用）
})

const emit = defineEmits(['action', 'refresh'])

const ctl = createSystemStatus()
onMounted(() => {
  if (props.autoPoll) {
    ctl.refresh()
    ctl.startPolling()
  }
})
onUnmounted(() => ctl.stopPolling())

const view = computed(() => props.status || ctl.st.update)
const caps = computed(() => (props.info && props.info.capabilities) || null)
const meta = computed(() =>
  (view.value && STATE_META[view.value.state]) || { label: '未知状态', desc: '', tone: '' })
const detailLabel = computed(() => {
  const d = view.value && view.value.detail
  return d ? (DETAIL_LABELS[d] || d) : ''
})
const can = computed(() => {
  const none = { check: false, download: false, install: false, rollback: false }
  const a = view.value ? allowedActions(view.value.state) : none
  if (!caps.value) return a
  return {
    check: a.check && !!caps.value.check,
    download: a.download && !!caps.value.download,
    install: a.install && !!caps.value.install,
    rollback: a.rollback && !!caps.value.rollback,
  }
})
const showProgress = computed(() => !!view.value
  && view.value.state === 'downloading'
  && view.value.progress
  && Number(view.value.progress.bytes_total) > 0)
const pct = computed(() => {
  if (!showProgress.value) return 0
  const { bytes_done, bytes_total } = view.value.progress
  return Math.min(100, Math.max(0, Math.floor((bytes_done / bytes_total) * 100)))
})

function act(name) {
  if (can.value[name] && !props.busy) emit('action', name)
}
</script>

<style scoped>
.upd-card { display: flex; flex-direction: column; gap: 10px; }
.card-head { display: flex; align-items: center; gap: 10px; }
.card-title { font-size: 15px; font-weight: 600; }
.spacer { flex: 1; }
.state-desc { font-size: 13px; color: var(--text-1); }
.detail-line { font-size: 12px; color: var(--text-2); }
.meta-line { display: flex; gap: 16px; flex-wrap: wrap; font-size: 12px; color: var(--text-2); }
.cand-row { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.cand-ver { font-size: 14px; font-weight: 600; }
.meta-text { font-size: 12px; color: var(--text-1); }
.cand-row a { font-size: 12px; color: var(--accent-2); text-decoration: none; }
.cand-row a:hover { text-decoration: underline; }
.prog { display: flex; flex-direction: column; gap: 6px; }
.prog-bar { height: 6px; border-radius: 3px; background: var(--bg-3); overflow: hidden; }
.prog-in { height: 100%; border-radius: 3px; background: var(--accent); transition: width .3s; }
.err-box {
  display: flex; gap: 8px; align-items: baseline; flex-wrap: wrap;
  font-size: 12px; color: var(--danger); line-height: 1.6;
  border: 1px solid rgba(248, 113, 113, .35); border-radius: var(--radius-sm);
  padding: 6px 10px; background: rgba(248, 113, 113, .06);
}
.err-code { font-family: var(--mono); font-weight: 600; }
.recovery {
  border: 1px solid rgba(248, 113, 113, .35); border-radius: var(--radius-sm);
  background: rgba(248, 113, 113, .06); padding: 10px 12px;
  display: flex; flex-direction: column; gap: 8px;
}
.rec-title { font-size: 13px; font-weight: 600; color: var(--danger); }
.rec-text { font-size: 12px; color: var(--text-1); line-height: 1.7; }
.journal { display: flex; flex-direction: column; gap: 4px; margin: 0; }
.j-row { display: flex; gap: 10px; font-size: 12px; }
.j-row dt { color: var(--text-2); flex-shrink: 0; }
.j-row dd { margin: 0; color: var(--text-1); word-break: break-all; font-family: var(--mono); }
.caps-note {
  font-size: 12px; color: var(--warn); line-height: 1.6;
  border: 1px solid rgba(251, 191, 36, .35); border-radius: var(--radius-sm);
  padding: 6px 10px; background: rgba(251, 191, 36, .06);
}
.acts { display: flex; gap: 8px; flex-wrap: wrap; }
.upd-empty { padding: 24px 0; font-size: 12px; }
</style>
