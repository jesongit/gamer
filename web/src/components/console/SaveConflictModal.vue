<template>
  <div v-if="open" class="modal-mask" @click.self="emit('close')">
    <div class="modal conflict-modal">
      <div class="modal-head">
        <span class="title">保存冲突</span>
        <button class="btn btn-ghost btn-sm" @click="emit('close')">✕</button>
      </div>
      <div class="modal-body">
        <p class="conflict-msg">{{ message || '资源已被其他页面修改，请重新加载后再保存' }}</p>
        <p class="conflict-resource mono" v-if="resource">{{ resource }}</p>
        <p class="conflict-hint">「重新加载」放弃本页未保存修改，恢复磁盘版本；「强制覆盖」以本页内容替换磁盘版本（对方的修改将丢失）。</p>
        <div class="conflict-actions">
          <button class="btn" @click="emit('reload')">↻ 重新加载</button>
          <button class="btn btn-danger" @click="emit('overwrite')">⚠ 强制覆盖</button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * 保存版本冲突弹窗（阶段 4：expected_version → 409 {code:"version_conflict"}）：
 * Console 紧凑外壳与独立全屏外壳共用。重载 = shell.reload()（放弃本地）；覆盖 = shell.overwrite()。
 */
defineProps({
  open: { type: Boolean, default: false },
  resource: { type: String, default: '' },
  message: { type: String, default: '' },
})

const emit = defineEmits(['reload', 'overwrite', 'close'])
</script>

<style scoped>
.conflict-modal { min-width: 420px; }
.conflict-msg { margin: 0 0 6px; font-size: 13px; color: var(--text-0); }
.conflict-resource { margin: 0 0 10px; font-size: 11px; color: var(--accent-2); }
.conflict-hint { margin: 0 0 12px; font-size: 12px; color: var(--text-2); }
.conflict-actions { display: flex; gap: 10px; justify-content: flex-end; }
</style>
