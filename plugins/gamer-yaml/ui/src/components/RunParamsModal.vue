<template>
  <div v-if="open" class="modal-mask" @click.self="emit('close')">
    <div class="modal run-params-modal">
      <div class="modal-head">
        <span class="title">{{ title }}</span>
        <button class="btn btn-ghost btn-sm" :disabled="submitting" @click="emit('close')">✕</button>
      </div>
      <div class="modal-body">
        <p v-if="desc" class="rp-desc">{{ desc }}</p>
        <!-- 无法定位到字段的服务端诊断（未知参数/整体拒绝等） -->
        <div v-for="(m, i) in generalErrors" :key="i" class="rp-general-err">{{ m }}</div>
        <ParamsForm
          ref="formEl"
          :params="params"
          :initial-args="initialArgs"
          :suggestions="suggestions"
          :templates="templates"
          :server-errors="fieldErrors"
        />
      </div>
      <div class="modal-foot">
        <button class="btn" :disabled="submitting" @click="emit('close')">取消</button>
        <button class="btn btn-primary" :disabled="submitting" @click="submit">{{ submitting ? '提交中…' : submitLabel }}</button>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * 运行参数弹窗（阶段 5）：ParamsForm 的模态外壳——Console 手动运行 /
 * 独立页函数测试共用。提交前先跑客户端校验（必填缺失/类型不合规阻断并标红），
 * 通过后 emit('submit', { args })（稀疏映射，「使用默认值」字段省略）；
 * 服务端 400 invalid_args 由宿主经 fieldErrors/generalErrors props 回填标红。
 */
import { ref } from 'vue'
import ParamsForm from '../script-editor/components/ParamsForm.vue'

defineProps({
  open: { type: Boolean, default: false },
  title: { type: String, default: '运行参数' },
  desc: { type: String, default: '' },
  submitLabel: { type: String, default: '▶ 运行' },
  params: { type: Array, required: true },
  initialArgs: { type: Object, default: () => ({}) },
  suggestions: { type: Object, default: () => ({}) },
  templates: { type: Array, default: () => [] },
  fieldErrors: { type: Object, default: () => ({}) },
  generalErrors: { type: Array, default: () => [] },
  submitting: { type: Boolean, default: false },
})

const emit = defineEmits(['submit', 'close'])
const formEl = ref(null)
function submit() {
  const form = formEl.value
  if (!form) return
  const errs = form.validate()
  if (errs.length) return // 必填缺失/类型不合规：已标红，阻断提交
  emit('submit', { args: form.getArgs() })
}
</script>

<style scoped>
.run-params-modal { width: 480px; max-width: calc(100vw - 32px); }
.rp-desc { margin: 0 0 8px; font-size: 12px; color: var(--text-2); line-height: 1.6; }
.rp-general-err {
  font-size: 12px; color: var(--danger);
  border: 1px solid var(--danger); border-radius: var(--radius-sm);
  padding: 5px 8px; margin-bottom: 8px; word-break: break-all;
}
</style>
