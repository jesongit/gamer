<template>
  <div class="card sys-card">
    <div class="card-head">
      <span class="card-title">系统与依赖</span>
      <span v-if="info" class="tag" :class="startup.cls">{{ startup.label }}</span>
    </div>

    <template v-if="info">
      <div class="app-row">
        <span class="ver">GameBot {{ info.app.version }}</span>
        <span class="tag info">{{ channelLabel }}</span>
        <span v-if="devBuild" class="tag warn">开发构建</span>
        <span class="spacer"></span>
      </div>
      <div class="meta-line">
        <span>commit {{ commitLabel }}</span>
        <span>构建于 {{ builtAtLabel }}</span>
        <span>{{ targetLabel }}</span>
      </div>
      <div class="meta-line">
        <span>部署：{{ modeLabel }}</span>
        <span>升级策略：{{ strategyLabel }}</span>
        <span :title="bootId">boot {{ shortBoot }}</span>
      </div>
      <div class="meta-line">
        <span>数据库 schema v{{ info.schema.db }}</span>
        <span>文件布局 schema v{{ info.schema.file }}</span>
        <span>自动回滚下限 v{{ info.schema.rollback_floor }}</span>
      </div>

      <table class="table dep-table">
        <thead>
          <tr><th>依赖</th><th>状态</th><th>版本</th><th>来源</th><th>绑定</th></tr>
        </thead>
        <tbody>
          <tr v-for="d in depRows" :key="d.id" :class="{ degraded: d.degraded }">
            <td>{{ d.id }}</td>
            <td><span class="tag" :class="d.statusCls">{{ d.statusLabel }}</span></td>
            <td>{{ d.versionLabel }}</td>
            <td>{{ d.sourceLabel }}</td>
            <td>{{ d.bindingLabel }}</td>
          </tr>
        </tbody>
      </table>

      <div class="caps-row">
        <span class="caps-title">更新能力</span>
        <span v-for="c in capChips" :key="c.name" class="tag" :class="{ ok: c.on }">{{ c.label }}</span>
        <span class="spacer"></span>
        <button class="btn btn-sm" :disabled="!canCheck" @click="emit('check')">检查更新</button>
        <button class="btn btn-sm" :disabled="!canInstall" @click="emit('install')">安装更新</button>
      </div>
      <p v-if="capsOff" class="caps-note">
        当前部署不受升级器托管（update_not_managed）：Docker 模式请在宿主机更换镜像，直跑模式请手动替换程序；更新操作按钮已禁用。
      </p>
    </template>

    <div v-else class="empty sys-empty">
      <span class="icon">ℹ</span>
      <p>{{ error ? `系统信息加载失败：${error}` : '暂无系统信息' }}</p>
    </div>
  </div>
</template>

<script setup>
/**
 * 系统与依赖状态卡片（WEB-002，fixture 驱动）：
 * 展示 /api/system/info 契约字段——app 版本/commit/built_at/channel/target、部署模式与
 * 升级策略、DB/file schema 与回滚下限、启动阶段/boot id，以及 adb/ffmpeg/scrcpy 依赖三行
 *（状态/版本/来源/绑定）。依赖缺失/损坏、Docker 降级（能力全 false）均有明确视觉态；
 * dev/unknown 构建信息以「开发构建」标记如实显示，不伪装正式版（契约 §2.1）。
 * 纯展示组件：info 为 null 时显示空态/错误；「检查更新/安装更新」按钮按 capabilities
 * 禁用并向宿主 emit('check'|'install')。
 */
import { computed } from 'vue'
import { formatLocalTime } from '../runs'
import {
  CHANNEL_LABELS, DEPLOYMENT_LABELS, STRATEGY_LABELS, STARTUP_STAGE_META,
  DEP_STATUS_META, DEP_SOURCE_LABELS, DEP_BINDING_LABELS,
  isDevBuild, shortId,
} from '../system/states'

const props = defineProps({
  info: { type: Object, default: null },  // GET /api/system/info 契约响应
  error: { type: String, default: '' },   // 加载失败的展示文案（可选）
})

const emit = defineEmits(['check', 'install'])

const devBuild = computed(() => isDevBuild(props.info && props.info.app))
const channelLabel = computed(() => {
  const c = props.info && props.info.app
  return c ? (CHANNEL_LABELS[c.channel] || c.channel) : '—'
})
const commitLabel = computed(() => {
  const c = props.info && props.info.app
  if (!c) return '—'
  return c.commit === 'unknown' ? 'unknown（未注入构建信息）' : shortId(c.commit, 8)
})
const builtAtLabel = computed(() => {
  const c = props.info && props.info.app
  if (!c) return '—'
  if (c.built_at === 'unknown') return '未知（开发构建）'
  return formatLocalTime(c.built_at)
})
const targetLabel = computed(() => (props.info && props.info.app && props.info.app.target) || '—')
const modeLabel = computed(() => {
  const d = props.info && props.info.deployment
  return d ? (DEPLOYMENT_LABELS[d.mode] || d.mode) : '—'
})
const strategyLabel = computed(() => {
  const d = props.info && props.info.deployment
  return d ? (STRATEGY_LABELS[d.update_strategy] || d.update_strategy) : '—'
})
const startup = computed(() => {
  const s = props.info && props.info.startup
  return (s && STARTUP_STAGE_META[s.stage]) || { label: '未知', cls: '' }
})
const bootId = computed(() => (props.info && props.info.startup && props.info.startup.boot_id) || '')
const shortBoot = computed(() => shortId(bootId.value, 8) || '—')

const DEP_IDS = ['adb', 'ffmpeg', 'scrcpy']
const depRows = computed(() => {
  const deps = (props.info && props.info.dependencies) || {}
  return DEP_IDS.map((id) => {
    const d = deps[id] || {}
    const st = DEP_STATUS_META[d.status] || { label: d.status || '未知', cls: 'warn' }
    return {
      id,
      statusLabel: st.label,
      statusCls: st.cls,
      degraded: !!d.status && d.status !== 'ready',
      versionLabel: d.version ?? '—',
      sourceLabel: DEP_SOURCE_LABELS[d.source] || d.source || '—',
      bindingLabel: DEP_BINDING_LABELS[d.binding] || d.binding || '—',
    }
  })
})

const CAP_LABELS = { check: '检查', download: '下载', install: '安装', rollback: '回滚' }
const capChips = computed(() => {
  const c = (props.info && props.info.capabilities) || {}
  return ['check', 'download', 'install', 'rollback'].map((k) => ({ name: k, label: CAP_LABELS[k], on: !!c[k] }))
})
const caps = computed(() => (props.info && props.info.capabilities) || null)
const capsOff = computed(() => !!caps.value && !caps.value.install)
const canCheck = computed(() => !caps.value || !!caps.value.check)
const canInstall = computed(() => !caps.value || !!caps.value.install)
</script>

<style scoped>
.sys-card { display: flex; flex-direction: column; gap: 10px; }
.card-head { display: flex; align-items: center; gap: 10px; }
.card-title { font-size: 15px; font-weight: 600; }
.spacer { flex: 1; }
.app-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.app-row .ver { font-size: 16px; font-weight: 700; font-family: var(--mono); }
.meta-line { display: flex; gap: 16px; flex-wrap: wrap; font-size: 12px; color: var(--text-1); }
.dep-table { font-size: 12px; }
.dep-table th, .dep-table td { padding: 7px 10px; }
.dep-table tr.degraded td { background: rgba(248, 113, 113, .06); }
.dep-table tr.degraded td:first-child { color: var(--danger); }
.caps-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.caps-title { font-size: 12px; color: var(--text-2); }
.caps-note {
  font-size: 12px; color: var(--warn); line-height: 1.6;
  border: 1px solid rgba(251, 191, 36, .35); border-radius: var(--radius-sm);
  padding: 6px 10px; background: rgba(251, 191, 36, .06);
}
.sys-empty { padding: 28px 0; font-size: 12px; }
</style>
