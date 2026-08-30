<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">系统状态</div>
        <div class="page-sub">只读运行信息与依赖健康状态，数据来自当前服务端</div>
      </div>
      <button class="btn btn-primary" :disabled="loading" @click="load">
        {{ loading ? '读取中…' : '↻ 刷新' }}
      </button>
    </div>

    <div v-if="loading" class="card state-card" role="status">
      <div class="state-icon">⏳</div>
      <div>
        <div class="state-title">正在读取系统状态</div>
        <div class="state-desc">正在向服务端请求真实版本、部署和依赖信息。</div>
      </div>
    </div>

    <div v-else-if="error" class="card state-card error-card" role="alert">
      <div class="state-icon">⚠️</div>
      <div class="state-copy">
        <div class="state-title">系统状态暂不可用</div>
        <div class="state-desc">{{ error }}</div>
        <div class="state-desc muted">接口未接入或服务端不可达时不会显示伪造的默认状态。</div>
      </div>
      <button class="btn" @click="load">重试</button>
    </div>

    <template v-else-if="info">
      <div class="readonly-note">🔒 只读视图：当前页面不会修改服务端配置或更新策略。</div>

      <div class="settings-grid">
        <div class="card set-card">
          <div class="sc-title">🎮 应用与构建</div>
          <div class="info-list">
            <div v-for="row in buildRows" :key="row.label" class="info-row">
              <span class="info-label">{{ row.label }}</span>
              <span :class="row.mono ? 'mono' : ''">{{ row.value }}</span>
            </div>
          </div>
        </div>

        <div class="card set-card">
          <div class="sc-title">🚀 部署与更新</div>
          <div class="info-list">
            <div v-for="row in deploymentRows" :key="row.label" class="info-row">
              <span class="info-label">{{ row.label }}</span>
              <span :class="row.state ? `state-${row.state}` : ''">{{ row.value }}</span>
            </div>
          </div>
          <div class="card-footnote">当前版本只展示能力，不提供未接通的更新操作。</div>
        </div>

        <div class="card set-card dependency-card">
          <div class="sc-title">🩺 依赖与健康</div>
          <div class="dependency-list">
            <div v-for="row in dependencyRows" :key="row.label" class="dep-row">
              <div class="dep-main">
                <span class="dep-dot" :class="statusClass(row.item)"></span>
                <span>{{ row.label }}</span>
                <span class="dep-status">{{ statusText(row.item) }}</span>
              </div>
              <div class="dep-meta">
                <span v-if="row.item && row.item.version" class="mono">{{ row.item.version }}</span>
                <span v-if="sourceText(row.item)">{{ sourceText(row.item) }}</span>
              </div>
            </div>
          </div>
        </div>

        <div class="card set-card">
          <div class="sc-title">🗃️ 数据、时区与启动</div>
          <div class="info-list">
            <div v-for="row in runtimeRows" :key="row.label" class="info-row">
              <span class="info-label">{{ row.label }}</span>
              <span :class="row.mono ? 'mono' : ''">{{ row.value }}</span>
            </div>
          </div>
        </div>
      </div>

      <div class="readiness" :class="readinessClass">
        <span class="readiness-dot"></span>
        <span>服务就绪：{{ readinessText }}</span>
        <span class="readiness-detail">黑屏依赖与数据库状态均来自本次服务端探测</span>
      </div>
    </template>
  </div>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { handleUnauthorized } from '../auth'

const info = ref(null)
const loading = ref(true)
const error = ref('')

async function load() {
  loading.value = true
  error.value = ''
  try {
    const response = await fetch('/api/system/info', {
      headers: { Accept: 'application/json' }
    })
    if (!response.ok) {
      if (response.status === 401) handleUnauthorized()
      const body = await response.json().catch(() => ({}))
      throw new Error(body?.error || `HTTP ${response.status}`)
    }
    const body = await response.json()
    if (!body || typeof body !== 'object') throw new Error('服务端响应格式异常')
    info.value = body
  } catch (cause) {
    info.value = null
    error.value = cause?.message || '网络请求失败'
  } finally {
    loading.value = false
  }
}

function display(value) {
  return value === undefined || value === null || value === '' ? '未知' : String(value)
}

function capabilityText(name) {
  return info.value?.capabilities?.[name] === true ? '可用' : '不可用'
}

function capabilityState(name) {
  return info.value?.capabilities?.[name] === true ? 'ok' : 'off'
}

function statusText(item) {
  const labels = { ready: '就绪', missing: '缺失', invalid: '无效', timeout: '超时', error: '异常' }
  return labels[item?.status] || '未知'
}

function statusClass(item) {
  return item?.status === 'ready' ? 'ok' : 'off'
}

function sourceText(item) {
  const labels = { bundled: '随部署提供', system: '系统工具', custom: '自定义路径' }
  return labels[item?.source] || ''
}

function schemaValue(name) {
  const value = info.value?.schema?.[name]
  if (value && typeof value === 'object') return `${display(value.version)} · ${statusText(value)}`
  return display(value)
}

const timezoneValue = computed(() => {
  const timezone = info.value?.timezone
  if (!timezone) return '未知'
  const offset = timezone.offset ? ` (${timezone.offset})` : ''
  return `${display(timezone.name)}${offset}`
})

const buildRows = computed(() => [
  { label: '版本', value: display(info.value?.app?.version), mono: true },
  { label: 'Commit', value: display(info.value?.app?.git_commit), mono: true },
  { label: '构建时间', value: display(info.value?.app?.built_at), mono: true },
  { label: '通道', value: display(info.value?.app?.channel) },
  { label: '目标', value: display(info.value?.app?.target), mono: true },
])

const deploymentRows = computed(() => [
  { label: '部署模式', value: display(info.value?.deployment?.mode) },
  { label: '更新策略', value: display(info.value?.deployment?.update_strategy) },
  { label: '检查更新', value: capabilityText('check'), state: capabilityState('check') },
  { label: '下载安装', value: capabilityText('download'), state: capabilityState('download') },
  { label: '安装 / 回滚', value: `${capabilityText('install')} / ${capabilityText('rollback')}`, state: updateState.value },
])

const dependencyRows = computed(() => [
  { label: 'ADB', item: info.value?.dependencies?.adb },
  { label: 'ffmpeg', item: info.value?.dependencies?.ffmpeg },
  { label: 'scrcpy-server', item: info.value?.dependencies?.scrcpy },
  { label: '数据目录', item: info.value?.dependencies?.data },
  { label: 'SQLite', item: info.value?.dependencies?.database },
])

const runtimeRows = computed(() => [
  { label: '数据库 schema', value: schemaValue('database'), mono: true },
  { label: '文件 schema', value: schemaValue('files'), mono: true },
  { label: '回滚下限', value: schemaValue('rollback_floor'), mono: true },
  { label: '服务端时区', value: timezoneValue.value, mono: true },
  { label: '启动阶段', value: display(info.value?.startup?.stage) },
  { label: 'Boot ID', value: display(info.value?.startup?.boot_id), mono: true },
])

const readinessText = computed(() => info.value?.readiness?.ready === true ? '是' : '否')
const readinessClass = computed(() => info.value?.readiness?.ready === true ? 'ready' : 'not-ready')
const updateState = computed(() => (
  info.value?.capabilities?.install === true && info.value?.capabilities?.rollback === true ? 'ok' : 'off'
))

onMounted(load)
</script>

<style scoped>
.settings-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 14px; }
.set-card { display: flex; flex-direction: column; gap: 14px; }
.sc-title { font-size: 14px; font-weight: 700; }
.info-list { display: flex; flex-direction: column; gap: 10px; }
.info-row { display: flex; align-items: baseline; gap: 12px; min-width: 0; }
.info-label { flex: 0 0 92px; color: var(--text-2); font-size: 12px; }
.info-row > span:last-child { min-width: 0; overflow-wrap: anywhere; text-align: right; margin-left: auto; }
.state-ok { color: var(--green, #57d38c); }
.state-off { color: var(--text-2); }
.readonly-note { margin-bottom: 14px; color: var(--text-2); font-size: 12px; }
.card-footnote { color: var(--text-2); font-size: 11px; line-height: 1.5; }

.state-card { display: flex; align-items: center; gap: 14px; min-height: 92px; }
.state-copy { flex: 1; min-width: 0; }
.state-icon { font-size: 24px; }
.state-title { font-size: 14px; font-weight: 700; }
.state-desc { margin-top: 5px; color: var(--text-1); font-size: 12px; line-height: 1.5; }
.muted { color: var(--text-2); }
.error-card { border-color: color-mix(in srgb, var(--danger, #ef6b73) 45%, var(--border)); }

.dependency-list { display: flex; flex-direction: column; gap: 12px; }
.dep-row { display: flex; flex-direction: column; gap: 4px; }
.dep-main, .dep-meta { display: flex; align-items: center; gap: 7px; font-size: 12px; }
.dep-status { margin-left: auto; color: var(--text-2); }
.dep-meta { padding-left: 16px; color: var(--text-2); font-size: 11px; }
.dep-meta span + span { margin-left: auto; }
.dep-dot, .readiness-dot { width: 7px; height: 7px; flex: 0 0 auto; border-radius: 50%; background: var(--text-2); }
.dep-dot.ok, .readiness.ready .readiness-dot { background: var(--green, #57d38c); }
.dep-dot.off, .readiness.not-ready .readiness-dot { background: var(--danger, #ef6b73); }
.readiness { display: flex; align-items: center; gap: 8px; margin-top: 14px; padding: 11px 14px; border: 1px solid var(--border); border-radius: 8px; font-size: 12px; }
.readiness.ready { color: var(--green, #57d38c); }
.readiness.not-ready { color: var(--danger, #ef6b73); }
.readiness-detail { margin-left: auto; color: var(--text-2); font-size: 11px; }

@media (max-width: 640px) {
  .state-card { align-items: flex-start; flex-wrap: wrap; }
  .state-card .btn { margin-left: 38px; }
  .readiness { align-items: flex-start; flex-wrap: wrap; }
  .readiness-detail { flex-basis: 100%; margin-left: 15px; }
}
</style>
