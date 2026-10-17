<template>
  <div class="modal-mask" v-backdrop-dismiss="() => { if (!busy) $emit('close') }">
    <form class="browser-form" role="dialog" aria-modal="true" aria-label="浏览器目标" @submit.prevent="save">
      <header><h3>{{ target ? '浏览器目标设置' : '新增浏览器目标' }}</h3><button type="button" class="btn btn-icon btn-ghost" aria-label="关闭浏览器目标设置" :disabled="busy" @click="$emit('close')"><UiIcon name="close" /></button></header>
      <fieldset :disabled="busy">
      <label>目标名称<input v-model="form.name" class="input" aria-label="浏览器目标名称" required maxlength="255" placeholder="例如：云游戏账号一" /></label>
      <label>游戏网址<input v-model="form.url" class="input" aria-label="游戏网址" type="url" required placeholder="https://…" /></label>
      <p>保存后在顶部选择这个目标，点击“连接画面”进入浏览器。</p>
      <details class="browser-advanced"><summary>账号资料与画面尺寸</summary>
      <label>账号资料标识<input v-model="form.profile_id" class="input mono" aria-label="账号资料标识" :disabled="!!target" required pattern="[a-z0-9][a-z0-9._-]*" maxlength="80" /></label>
      <p>已自动分配独立资料。不同账号应使用不同标识；登录状态保存在服务器的专用浏览器目录。</p>
      <div class="dimension-fields"><label>画面宽度<input v-model.number="form.width" class="input" aria-label="浏览器画面宽度" type="number" min="320" max="3840" required /></label><label>画面高度<input v-model.number="form.height" class="input" aria-label="浏览器画面高度" type="number" min="240" max="2160" required /></label></div>
      </details>
      <p v-if="target">修改网址或画面尺寸前，请先关闭浏览器；仅修改名称不需要关闭。</p>
      </fieldset>
      <p v-if="error" role="alert">{{ error }}</p>
      <div class="buttons"><button type="button" class="btn" :disabled="busy" @click="$emit('close')">取消</button><button class="btn btn-primary" :disabled="busy">{{ busy ? '保存中…' : target ? '保存设置' : '添加目标' }}</button></div>
    </form>
  </div>
</template>
<script setup>
import { vBackdropDismiss } from '../../../../plugins/ui-shared/backdrop-dismiss.js'
import { reactive, ref } from 'vue'
import { api } from '../../api'
import UiIcon from '../ui/UiIcon.vue'
const props = defineProps({ target: { type: Object, default: null } })
const emit = defineEmits(['close', 'saved'])
const suffix = Array.from(crypto.getRandomValues(new Uint8Array(6)), byte => byte.toString(16).padStart(2, '0')).join('')
const form = reactive(Object.fromEntries(['id', 'name', 'url', 'profile_id', 'width', 'height'].map(key => [key, props.target?.[key] ?? ({ id: `browser-${suffix}`, name: '', url: '', profile_id: `account-${suffix}`, width: 1280, height: 720 })[key]])))
const error = ref(''), busy = ref(false)
async function save() {
  if (busy.value) return
  error.value = ''
  busy.value = true
  try { await api.saveBrowser(form); emit('saved', form.id) } catch (e) { error.value = e.message } finally { busy.value = false }
}
</script>
<style scoped>
.browser-form{width:min(500px,calc(100vw - 24px));max-height:calc(100dvh - 24px);overflow:auto;padding:16px;background:var(--bg-1);color:var(--text-0);border:1px solid var(--border);border-radius:var(--radius)}header{display:flex;justify-content:space-between;align-items:center;gap:10px}h3{font-size:17px}fieldset{border:0;margin:0;padding:0;min-width:0}label{display:flex;flex-direction:column;gap:6px;margin:12px 0;font-size:13px}input{min-width:0;width:100%}p{font-size:13px;line-height:1.6;color:var(--text-2)}.buttons{display:flex;justify-content:flex-end;gap:10px;margin-top:16px;padding-top:12px;border-top:1px solid var(--border)}.browser-advanced{margin-top:14px}.browser-advanced summary{cursor:pointer;font-size:13px;padding:4px 0}.dimension-fields{display:grid;grid-template-columns:1fr 1fr;gap:12px}[role=alert]{color:var(--danger)}
</style>
