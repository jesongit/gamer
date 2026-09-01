<template>
  <div v-if="preview.open" class="modal-mask" @click.self="emit('close')">
    <div class="modal resource-preview-modal" role="dialog" aria-modal="true" :aria-label="preview.title">
      <div class="modal-head">
        <div>
          <div class="title">{{ preview.title }}</div>
          <div class="preview-resource mono">{{ preview.resource }}</div>
        </div>
        <button class="btn btn-ghost btn-sm" title="关闭" @click="emit('close')">✕</button>
      </div>
      <div class="modal-body preview-body">
        <div v-if="preview.error" class="preview-error">{{ preview.error }}</div>
        <template v-else-if="preview.kind === 'function_library' && preview.model">
          <div class="preview-count">函数列表 · {{ preview.model.functions.length }} 个</div>
          <section v-for="fn in preview.model.functions" :key="fn.name" class="preview-function">
            <div class="preview-function-head">
              <span class="preview-function-name mono">{{ fnSignature(fn) }}</span>
              <span class="preview-step-count">步骤 {{ fn.steps?.length || 0 }} 个</span>
            </div>
            <ScriptSummary :model="functionModel(fn)" readonly />
          </section>
        </template>
        <template v-else-if="preview.model">
          <div class="preview-count">步骤列表 · {{ preview.model.steps?.length || 0 }} 个</div>
          <ScriptSummary :model="preview.model" readonly />
        </template>
        <div v-else class="preview-empty">暂无可显示内容</div>
      </div>
      <div class="modal-foot">
        <button class="btn" @click="emit('close')">关闭</button>
      </div>
    </div>
  </div>
</template>

<script setup>
import ScriptSummary from './ScriptSummary.vue'

defineProps({
  preview: { type: Object, required: true },
})

const emit = defineEmits(['close'])

function fnSignature(fn) {
  const params = Array.isArray(fn?.params) ? fn.params : []
  const signature = params.map(p => (p.remark ? `${p.name}:${p.remark}` : p.name)).join(', ')
  return `${fn?.name || '函数'}(${signature})`
}

function functionModel(fn) {
  return { params: fn?.params || [], steps: fn?.steps || [] }
}
</script>

<style scoped>
.resource-preview-modal {
  width: min(760px, calc(100vw - 32px));
  max-width: none;
}
.resource-preview-modal .modal-head {
  align-items: flex-start;
}
.preview-resource {
  margin-top: 4px;
  color: var(--text-2);
}
.preview-body {
  max-height: calc(85vh - 150px);
  overflow: auto;
}
.preview-count {
  color: var(--text-2);
  font-size: 12px;
}
.preview-function {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding-top: 2px;
}
.preview-function + .preview-function {
  border-top: 1px solid var(--border);
  padding-top: 12px;
}
.preview-function-head {
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.preview-function-name {
  color: var(--accent);
  font-size: 14px;
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.preview-step-count {
  flex: none;
  color: var(--text-2);
  font-size: 12px;
}
.preview-body :deep(.script-summary) {
  min-height: 0;
}
.preview-body :deep(.sum-list) {
  overflow: visible;
}
.preview-error,
.preview-empty {
  color: var(--text-2);
  font-size: 13px;
  line-height: 1.7;
}
.preview-error {
  color: var(--danger);
}
.mono {
  font-family: var(--mono);
  font-size: 11px;
}
</style>
