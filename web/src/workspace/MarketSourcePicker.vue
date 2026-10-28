<template>
  <div class="market-source-picker">
    <div class="source-choice-row">
      <label class="source-choice-label" :for="selectId">{{ label }}</label>
      <select :id="selectId" class="input source-select" :aria-label="label" :value="selectedId" :disabled="disabled || saving || initialLoading" @change="choose($event.target.value)">
        <option v-if="!sources.length" value="">{{ initialLoading ? '正在读取来源…' : '来源暂不可用' }}</option>
        <option v-for="source in sources" :key="source.id" :value="source.id">{{ source.name || source.repository }}</option>
      </select>
      <button class="btn btn-sm" :aria-expanded="manageOpen" :disabled="disabled || saving || initialLoading || !sources.length" @click="manageOpen = !manageOpen">管理预设</button>
      <a v-if="selectedSource" class="source-address" :href="sourceHref(selectedSource.repository)" target="_blank" rel="noopener noreferrer">{{ selectedSource.repository }}</a>
    </div>
    <div v-if="error" class="source-error" role="alert">{{ error }} <button v-if="!sources.length" class="btn btn-sm" :disabled="initialLoading" @click="load">重试</button></div>
    <div v-if="manageOpen" class="source-editor">
      <div class="source-editor-heading">
        <strong>{{ editingId ? '编辑预设' : '新增预设' }}</strong>
        <div v-if="selectedSource && !selectedSource.builtin" class="source-editor-actions">
          <button class="btn btn-sm" :disabled="disabled || saving" @click="editSelected">编辑当前预设</button>
          <button class="btn btn-sm" :disabled="disabled || saving" @click="removeSelected">删除当前预设</button>
        </div>
      </div>
      <form class="source-preset-form" @submit.prevent="save">
        <label>预设名称<input v-model="name" class="input" aria-label="预设名称" maxlength="80" required :disabled="disabled || saving" placeholder="例如：家中服务器" /></label>
        <label class="source-url-field">来源 URL<input v-model="url" class="input" aria-label="来源 URL" maxlength="2048" required :disabled="disabled || saving" spellcheck="false" placeholder="仓库地址或目录 JSON 地址" /></label>
        <div class="source-form-actions"><button class="btn btn-sm btn-primary" :disabled="disabled || saving || !name.trim() || !url.trim()">{{ saving ? '正在保存…' : '保存预设' }}</button><button v-if="editingId" type="button" class="btn btn-sm" :disabled="saving" @click="resetForm">取消编辑</button></div>
      </form>
      <p class="source-help">支持公开 GitHub、Gitea 仓库和 {{ kind === 'plugin' ? 'registry.json' : 'packages.json' }} 目录。内网 HTTP 请使用私有 IP；仓库需要包含发行附件。预设在服务端共享，当前选择仅保存在此浏览器。</p>
      <p v-if="notice" class="source-notice" role="status">{{ notice }}</p>
    </div>
  </div>
</template>

<script setup>
import { computed, onMounted, ref, useId } from 'vue'
import { api } from '../api'

const props = defineProps({ kind: { type: String, required: true }, apiClient: { type: Object, default: () => api }, disabled: Boolean })
const emit = defineEmits(['change', 'error'])
const selectId = `market-source-${useId()}`
const label = computed(() => props.kind === 'plugin' ? '插件来源' : '配置来源')
const sources = ref([]), selectedId = ref(''), initialLoading = ref(false), saving = ref(false)
const manageOpen = ref(false), editingId = ref(''), name = ref(''), url = ref(''), error = ref(''), notice = ref('')
const selectedSource = computed(() => sources.value.find(s => s.id === selectedId.value))
const storageKey = `gamer.market.${props.kind}.source`
const sourceHref = value => /^https?:\/\//i.test(value) ? value : `https://github.com/${value}`
const methods = () => props.kind === 'plugin'
  ? { list: props.apiClient.pluginSources, save: props.apiClient.savePluginSource, remove: props.apiClient.removePluginSource }
  : { list: props.apiClient.packageSources, save: props.apiClient.savePackageSource, remove: props.apiClient.removePackageSource }

function remember(id) { try { localStorage.setItem(storageKey, id) } catch { /* Session selection still works when storage is unavailable. */ } }
function restoredId() { try { return localStorage.getItem(storageKey) || '' } catch { return '' } }
function select(id) {
  selectedId.value = id
  remember(id)
  emit('change', selectedSource.value)
}
async function choose(id) {
  error.value = ''; notice.value = ''
  const source = sources.value.find(s => s.id === id)
  if (!source) return
  if (!source.enabled) {
    saving.value = true
    try { sources.value = await methods().save(source.repository, true) }
    catch (e) { error.value = e.message; return }
    finally { saving.value = false }
  }
  select(id)
}
async function load() {
  initialLoading.value = true; error.value = ''
  try {
    const result = await methods().list()
    if (!Array.isArray(result) || !result.length) throw new Error('来源预设列表不可用，请刷新重试')
    sources.value = result
    const id = [selectedId.value, restoredId()].find(id => result.some(s => s.id === id && s.enabled)) || result.find(s => s.enabled)?.id || result[0].id
    await choose(id)
  } catch (e) { error.value = `来源预设读取失败：${e.message}`; emit('error', error.value) }
  finally { initialLoading.value = false }
}
function resetForm() { editingId.value = ''; name.value = ''; url.value = '' }
function editSelected() {
  editingId.value = selectedSource.value.id; name.value = selectedSource.value.name || ''; url.value = selectedSource.value.repository; error.value = ''; notice.value = ''
}
async function save() {
  if (saving.value || props.disabled || !name.value.trim() || !url.value.trim()) return
  saving.value = true; error.value = ''; notice.value = ''
  try {
    const result = await methods().save(url.value.trim(), true, { name: name.value.trim(), ...(editingId.value ? { id: editingId.value } : {}) })
    const saved = result.find(s => s.id === editingId.value) || result.find(s => s.name === name.value.trim())
    if (!saved) throw new Error('预设保存结果未确认，请刷新检查')
    sources.value = result; select(saved.id); resetForm(); notice.value = '预设已保存，其他浏览器重新读取后也可使用。'
  } catch (e) { error.value = e.message }
  finally { saving.value = false }
}
async function removeSelected() {
  if (!selectedSource.value || selectedSource.value.builtin || saving.value || props.disabled) return
  saving.value = true; error.value = ''; notice.value = ''
  try {
    await methods().remove(selectedId.value)
    sources.value = await methods().list()
    select(sources.value.find(s => s.enabled)?.id || sources.value[0]?.id || '')
    resetForm(); notice.value = '预设已删除，已安装内容保留。'
  } catch (e) { error.value = e.message }
  finally { saving.value = false }
}
onMounted(load)
</script>

<style scoped>
.market-source-picker { min-width:0; margin-bottom:14px; font-size:13px; flex-shrink:0; }
.source-choice-row,.source-editor-heading,.source-editor-actions,.source-form-actions { display:flex; align-items:center; gap:8px; flex-wrap:wrap; }
.source-choice-label { color:var(--text-1); flex-shrink:0; }.source-select { width:200px; max-width:100%; min-width:0; }
.source-address { flex:1; min-width:140px; overflow-wrap:anywhere; color:var(--text-2); text-underline-offset:3px; font-size:13px; }
.source-editor { margin-top:10px; padding:12px; background:var(--bg-1); border:1px solid var(--border); border-radius:3px; }
.source-editor-heading { justify-content:space-between; margin-bottom:12px; }.source-editor-heading strong { font-size:13px; font-weight:600; }
.source-preset-form { display:flex; align-items:flex-end; gap:10px; flex-wrap:wrap; }.source-preset-form label { display:grid; gap:6px; flex:1; min-width:150px; color:var(--text-1); }.source-preset-form .source-url-field { flex:3; min-width:min(100%,260px); }.source-preset-form .input { width:100%; min-width:0; box-sizing:border-box; }
.source-help { color:var(--text-2); margin:10px 0 0; line-height:1.6; }.source-error { color:var(--danger); margin-top:8px; line-height:1.6; }.source-notice { color:var(--ok); margin:10px 0 0; line-height:1.6; }
@media(max-width:600px) { .source-address { flex-basis:100%; }.source-preset-form label { flex-basis:100%; }.source-form-actions { width:100%; }.source-select { flex:1; width:auto; }.source-editor-heading { align-items:flex-start; } }
</style>
