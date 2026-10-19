<template>
  <section class="card runtime-dependencies" aria-label="运行依赖">
    <div class="runtime-head"><b>运行依赖</b><button class="btn btn-sm" :disabled="busy" @click="load">{{ loading ? '检测中…' : '重新检测' }}</button></div>
    <p class="runtime-hint">优先使用已有工具。缺失时自动准备到 Gamer 专用目录，保留你已安装的软件。文字识别已内置，无需安装 OCR 插件。</p>
    <p v-if="error" class="runtime-error" role="alert">{{ error }}</p>
    <p v-if="note" class="runtime-note" role="status">{{ note }}</p>
    <p v-if="loading && !view" role="status">正在检测本机依赖…</p>
    <ul v-if="view" class="runtime-list">
      <li v-for="dep in view.dependencies" :key="dep.id">
        <div class="runtime-summary"><strong>{{ names[dep.id] || dep.id }}</strong><span :class="dep.status === 'ready' ? 'runtime-ready' : 'runtime-error'">{{ dep.status === 'ready' ? '可用' : dep.status === 'broken' ? '配置需修正' : '待准备' }}</span><span class="runtime-source">{{ sources[dep.source] || dep.source }}<template v-if="dep.version"> · {{ dep.version }}</template></span></div>
        <p v-if="dep.status !== 'ready'">{{ dep.detail }}</p>
        <p v-if="dep.restart_required" class="runtime-hint">新路径将在重启服务后用于新连接；已有运行继续使用原路径。</p>
        <div v-if="dep.status !== 'ready' && dep.auto_install && !dep.configured_path" class="runtime-actions"><button class="btn btn-primary btn-sm" :disabled="busy" @click="prepare(dep)">{{ preparing === dep.id ? '正在下载并验证…' : '自动准备' }}</button></div>
        <details v-if="dep.id !== 'scrcpy-server'">
          <summary>选择已有工具</summary>
          <form @submit.prevent="save(dep)">
            <label :for="`dependency-${dep.id}`">{{ dep.id === 'tessdata' ? '语言包目录（eng、chi_sim）' : '程序完整路径' }}</label>
            <div class="runtime-path"><input :id="`dependency-${dep.id}`" v-model="paths[dep.id]" class="input" :disabled="busy" placeholder="留空使用自动检测" /><button class="btn btn-sm" :disabled="busy || paths[dep.id] === dep.configured_path">保存选择</button></div>
            <p class="runtime-hint">{{ dep.path ? `当前：${dep.path}` : '尚未选择可用依赖' }}。指定路径失效时会提示修正。</p>
          </form>
        </details>
      </li>
    </ul>
  </section>
</template>
<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
const view = ref(null), paths = ref({}), loading = ref(false), preparing = ref(''), saving = ref(false), error = ref(''), note = ref('')
const busy = computed(() => loading.value || !!preparing.value || saving.value)
const names = { adb:'Android 连接（ADB）', ffmpeg:'画面与媒体（FFmpeg）', browser:'云游戏浏览器', tesseract:'文字识别（Tesseract）', tessdata:'识别语言包', 'scrcpy-server':'Android 画面采集' }
const sources = { auto:'自动检测', system:'已有工具', custom:'指定路径', managed:'Gamer 管理', application:'随应用分发' }
function apply(data, savedId = null) {
  const drafts = new Map((view.value?.dependencies || []).filter(d => d.id !== savedId && paths.value[d.id] !== d.configured_path).map(d => [d.id, paths.value[d.id]]))
  view.value = data; paths.value = Object.fromEntries(data.dependencies.map(d => [d.id, drafts.has(d.id) ? drafts.get(d.id) : d.configured_path]))
}
async function load() {
  if (busy.value) return
  loading.value = true; error.value = ''; note.value = ''
  try { apply(await api.getRuntimeDependencies()) } catch (e) { error.value = e.message } finally { loading.value = false }
}
async function prepare(dep) {
  if (busy.value) return
  preparing.value = dep.id; error.value = ''; note.value = '正在准备依赖。首次下载可能需要几分钟，请保持此页面打开。'
  try { apply(await api.prepareRuntimeDependency(dep.id)); note.value = '依赖已准备并通过验证。' } catch (e) { error.value = e.message; note.value = '' } finally { preparing.value = '' }
}
async function save(dep) {
  if (busy.value) return
  saving.value = true; error.value = ''; note.value = ''
  try { apply(await api.saveRuntimeDependency(dep.id, paths.value[dep.id], view.value.revision), dep.id); note.value = '已保存选择。需要重启的项目会显示提示。' } catch (e) { error.value = e.message } finally { saving.value = false }
}
onMounted(load)
</script>
<style scoped>
.runtime-dependencies{padding:16px;margin-bottom:16px}.runtime-head,.runtime-summary,.runtime-path{display:flex;align-items:center;gap:12px;flex-wrap:wrap}.runtime-head{justify-content:space-between}.runtime-hint,.runtime-source{color:var(--text-2);font-size:12px;line-height:1.6;overflow-wrap:anywhere}.runtime-list{list-style:none;padding:0;margin:0}.runtime-list>li{padding:14px 0;border-bottom:1px solid var(--border)}.runtime-list>li:last-child{border-bottom:0}.runtime-summary{font-size:13px}.runtime-source{flex:1}.runtime-list p{font-size:13px;line-height:1.6}.runtime-error{color:var(--danger)}.runtime-ready,.runtime-note{color:var(--ok)}.runtime-actions{margin:10px 0}details{margin-top:10px;font-size:13px}summary{cursor:pointer;color:var(--text-2);padding:4px 0}form{margin-top:10px}label{display:block;margin-bottom:6px}.runtime-path input{flex:1;min-width:160px}.runtime-path .btn{flex-shrink:0}.runtime-note{font-size:13px;line-height:1.6}
@media(max-width:600px){.runtime-summary{gap:8px}.runtime-source{flex-basis:100%}.runtime-path{align-items:stretch;flex-direction:column}.runtime-path input{min-width:0;width:100%;box-sizing:border-box}.runtime-path .btn{align-self:flex-start}}
</style>
