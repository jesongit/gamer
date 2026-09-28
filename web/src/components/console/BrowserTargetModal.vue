<template>
  <div class="modal-mask" @click.self="$emit('close')">
    <form class="browser-form" role="dialog" aria-modal="true" aria-label="浏览器目标" @submit.prevent="save">
      <h3>{{ target ? '浏览器目标设置' : '新增浏览器目标' }}</h3>
      <label>名称<input v-model="form.name" required maxlength="255" /></label>
      <label>云游戏网址<input v-model="form.url" type="url" required placeholder="https://…" /></label>
      <label>账号资料 ID<input v-model="form.profile_id" :disabled="!!target" required pattern="[a-z0-9][a-z0-9._-]*" maxlength="80" /></label>
      <p>不同账号使用不同资料 ID。登录信息保存在服务器专用浏览器目录；删除目标会保留登录资料。修改设置前请先关闭浏览器。</p>
      <label>画面宽度<input v-model.number="form.width" type="number" min="320" max="3840" required /></label>
      <label>画面高度<input v-model.number="form.height" type="number" min="240" max="2160" required /></label>
      <p v-if="error" role="alert">{{ error }}</p>
      <div class="buttons"><button type="button" class="btn" @click="$emit('close')">取消</button><button class="btn btn-primary" :disabled="busy">保存</button></div>
    </form>
  </div>
</template>
<script setup>
import { reactive, ref } from 'vue'
import { api } from '../../api'
const props = defineProps({ target: { type: Object, default: null } })
const emit = defineEmits(['close', 'saved'])
const suffix = crypto.randomUUID().slice(0, 8)
const form = reactive(Object.fromEntries(['id', 'name', 'url', 'profile_id', 'width', 'height'].map(key => [key, props.target?.[key] ?? ({ id: `browser-${suffix}`, name: '', url: '', profile_id: `account-${suffix}`, width: 1280, height: 720 })[key]])))
const error = ref(''), busy = ref(false)
async function save() {
  busy.value = true
  try { await api.saveBrowser(form); emit('saved', form.id) } catch (e) { error.value = e.message } finally { busy.value = false }
}
</script>
<style scoped>
.browser-form { width: min(460px, 90vw); padding: 24px; background: var(--bg, #202126); color: var(--text, #ddd); border-radius: 8px; }
label { display: flex; justify-content: space-between; gap: 16px; margin: 14px 0; align-items: center; }
input { min-width: 0; width: 65%; padding: 6px; } p { font-size: 12px; line-height: 1.7; } .buttons { display: flex; justify-content: flex-end; gap: 10px; }
</style>
