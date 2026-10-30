<template>
  <Teleport to="body">
    <div v-if="state.mode" class="modal-mask" v-backdrop-dismiss="() => $emit('close')" @keydown.esc.stop="$emit('close')">
      <form class="modal clipboard-dialog" role="dialog" aria-modal="true" aria-labelledby="clipboard-title" @submit.prevent="$emit('submit', text)">
        <div class="modal-head"><h2 id="clipboard-title" class="title">{{ copying ? '复制游戏文本' : '粘贴到游戏' }}</h2><button type="button" class="btn btn-icon" aria-label="关闭剪贴板" @click="$emit('close')">×</button></div>
        <div class="modal-body">
          <p id="clipboard-help">{{ copying ? '已取回选中的文字。点击复制，或选中文本按 Ctrl+C。' : '在下方按 Ctrl+V 粘贴文本，再发送到游戏当前输入框。' }}</p>
          <label for="clipboard-text">{{ copying ? '选中的文字' : '要粘贴的文字' }}</label>
          <textarea id="clipboard-text" ref="field" v-model="text" class="input" rows="6" :readonly="copying" aria-describedby="clipboard-help" />
          <p v-if="state.error" role="alert">{{ state.error }}</p>
        </div>
        <div class="modal-foot"><button type="button" class="btn" @click="$emit('close')">取消</button><button type="submit" class="btn btn-primary" :disabled="!text">{{ copying ? '复制文本' : '发送到游戏' }}</button></div>
      </form>
    </div>
  </Teleport>
</template>

<script setup>
import { computed, nextTick, ref, watch } from 'vue'
import { vBackdropDismiss } from '../../../../plugins/ui-shared/backdrop-dismiss'
const props = defineProps({ state: { type: Object, required: true } })
defineEmits(['close', 'submit'])
const text = ref(''), field = ref(null)
const copying = computed(() => props.state.mode === 'copy')
watch(() => props.state.mode, async mode => {
  if (!mode) return
  text.value = props.state.text
  await nextTick()
  field.value?.focus()
  if (copying.value) field.value?.select()
})
</script>

<style scoped>
.clipboard-dialog { width: min(480px, calc(100vw - 32px)); }
.modal-body { display: grid; gap: 12px; }
.modal-body p { margin: 0; color: var(--text-1); }
textarea { width: 100%; min-height: 140px; resize: vertical; font-size: 16px; }
[role="alert"] { color: var(--danger); }
</style>
