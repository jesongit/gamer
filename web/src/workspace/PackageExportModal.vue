<template>
  <div v-if="open" class="modal-mask" @click.self="emit('close')">
    <div class="modal pkg-export-modal">
      <div class="modal-head">
        <span class="title">导出游戏包</span>
        <button class="btn btn-ghost btn-sm" :disabled="exporting" @click="emit('close')">✕</button>
      </div>
      <div class="modal-body">
        <p class="pkg-export-desc">将当前编辑区打包为 .gamerpkg 归档（含元数据与全部资源）。</p>
        <div class="pkg-kv">
          <span class="pkg-kv-key">游戏包 ID</span>
          <span class="pkg-kv-val mono">{{ info.id || '—' }}</span>
          <span class="pkg-kv-key">名称</span>
          <span class="pkg-kv-val">{{ info.name || '—' }}</span>
          <span class="pkg-kv-key">版本</span>
          <span class="pkg-kv-val mono">{{ info.version || '—' }}</span>
          <span class="pkg-kv-key">Android Package</span>
          <span class="pkg-kv-val mono">{{ androidPackagesText }}</span>
        </div>
        <div class="pkg-stats">
          <span class="pkg-stats-title">资源统计</span>
          <div v-for="[key, label] in statRows" :key="key" class="pkg-stat-row">
            <span>{{ label }}</span>
            <span class="mono">{{ statOf(key) }}</span>
          </div>
        </div>
        <!-- 400 preflight_failed：逐行问题列表（等宽预格式化展示，不静默吞掉） -->
        <div v-if="errorLines.length" class="pkg-export-errors">
          <span class="pkg-export-errors-title">导出前校验未通过，请修复后重试：</span>
          <pre class="pkg-export-errors-list">{{ errorLines.join('\n') }}</pre>
        </div>
      </div>
      <div class="modal-foot">
        <button class="btn" :disabled="exporting" @click="emit('close')">取消</button>
        <button class="btn btn-primary" :disabled="exporting" @click="emit('confirm')">{{ exporting ? '导出中…' : '导出' }}</button>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * 导出确认弹窗：展示工作区元数据（游戏包 ID/名称/版本/Android Package）与六目录
 * 资源统计；确认后由宿主调 POST /api/app-packages/export 并下载归档。400
 * preflight_failed 的问题列表经 errorLines prop 原样多行展示。
 */
import { computed } from 'vue'
import { PACKAGE_STAT_ROWS } from '../composables/useWorkspacePackages'

const props = defineProps({
  open: { type: Boolean, default: false },
  exporting: { type: Boolean, default: false },
  errorLines: { type: Array, default: () => [] },
  info: {
    type: Object,
    default: () => ({ id: '', name: '', version: '', androidPackages: [], stats: {} }),
  },
})

const emit = defineEmits(['confirm', 'close'])

const statRows = PACKAGE_STAT_ROWS
const statOf = (key) => Number(props.info?.stats?.[key]) || 0
const androidPackagesText = computed(() => {
  const list = props.info?.androidPackages || []
  return list.length ? list.join(', ') : '—'
})
</script>

<style scoped>
.pkg-export-modal { width: 420px; max-width: calc(100vw - 32px); }
.pkg-export-desc { margin: 0; font-size: 12px; color: var(--text-2); line-height: 1.6; }
.pkg-kv {
  display: grid; grid-template-columns: auto 1fr; gap: 4px 14px;
  font-size: 12px; align-items: baseline;
}
.pkg-kv-key { color: var(--text-2); }
.pkg-kv-val { color: var(--text-0); word-break: break-all; }
.pkg-stats {
  display: flex; flex-direction: column; gap: 4px;
  border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 8px 10px;
}
.pkg-stats-title { font-size: 11px; color: var(--text-2); }
.pkg-stat-row {
  display: flex; justify-content: space-between; font-size: 12px; color: var(--text-1);
}
.pkg-export-errors {
  display: flex; flex-direction: column; gap: 4px;
  border: 1px solid var(--danger); border-radius: var(--radius-sm); padding: 8px 10px;
}
.pkg-export-errors-title { font-size: 12px; color: var(--danger); }
.pkg-export-errors-list {
  margin: 0; white-space: pre-wrap; word-break: break-all;
  font-family: var(--mono); font-size: 11px; line-height: 1.6; color: var(--text-1);
}
</style>
