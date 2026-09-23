<template>
  <div>
    <p v-if="error" class="error" role="alert">{{ error }} <button class="btn btn-sm" @click="load">重试插件设置</button></p>
    <section v-for="card in cards" :key="card.id" class="card plugin-settings" :aria-label="`${card.title}设置`">
      <div class="settings-head"><b>{{ card.title }}</b><button class="btn btn-sm" :disabled="card.busy" @click="reload(card)">重新读取</button></div>
      <form @submit.prevent="save(card)"><fieldset :disabled="card.busy || !card.fields">
        <label v-for="field in card.fields" :key="field.key" class="setting-row">
          <span class="setting-label">{{ field.label }}<small>{{ field.help }}</small></span>
          <span class="setting-value"><input class="input" type="number" :aria-label="field.label" :min="field.min" :max="field.max" step="1" v-model.number="card.form[field.key]" /><span>{{ field.unit }}</span></span>
          <span class="effect">{{ field.effect }}</span>
        </label>
        <button class="btn btn-primary btn-sm" type="submit">{{ card.busy ? '处理中…' : `保存${card.title}设置` }}</button>
      </fieldset></form>
      <p v-if="card.error" class="error" role="alert">{{ card.error }}</p>
      <p v-if="card.note" class="note" role="status">{{ card.note }}</p>
    </section>
  </div>
</template>

<script setup>
import { onMounted, ref } from 'vue'
import { api } from '../api'
const cards = ref([]), error = ref('')
function apply(card, data) {
  card.title = data.title; card.fields = data.fields; card.expected = data.values
  card.form = structuredClone(data.values)
}
async function reload(card) {
  card.busy = true; card.error = ''; card.note = ''
  try { apply(card, await api.callExtension(card.id, 'settings.get')) } catch (e) { card.error = e.message } finally { card.busy = false }
}
async function load() {
  error.value = ''
  try {
    const rep = await api.listExtensions()
    const results = await Promise.allSettled((rep.extensions || []).filter(e => e.state === 'running').map(async e => {
      const capabilities = await api.getExtensionCapabilities(e.id)
      const actions = new Set((capabilities.actions || []).map(a => a.action))
      if (!capabilities.running || !actions.has('settings.get') || !actions.has('settings.save')) return null
      const card = { id:e.id, title:e.name || e.id, fields:null, form:{}, expected:null, busy:false, error:'', note:'' }
      await reload(card)
      return card
    }))
    cards.value = results.filter(r => r.status === 'fulfilled' && r.value).map(r => r.value)
    if (results.some(r => r.status === 'rejected')) error.value = '部分插件设置读取失败。'
  } catch (e) { error.value = `读取插件设置失败：${e.message}` }
}
async function save(card) {
  if (card.busy) return
  card.error = ''; card.note = ''
  const invalid = card.fields.find(f => !Number.isInteger(card.form[f.key]) || card.form[f.key] < f.min || card.form[f.key] > f.max)
  if (invalid) { card.error = `${invalid.label}须为 ${invalid.min}～${invalid.max} 的整数`; return }
  card.busy = true
  try {
    apply(card, await api.callExtension(card.id, 'settings.save', { settings:card.form, expected:card.expected }))
    card.note = '已保存，下次运行生效；正在运行的任务保持原值。'
  } catch (e) { card.error = e.message } finally { card.busy = false }
}
onMounted(load)
</script>

<style scoped>
.plugin-settings{padding:16px;margin-bottom:16px}.settings-head{display:flex;align-items:center;justify-content:space-between;margin-bottom:12px}fieldset{border:0;padding:0;margin:0;min-width:0}.setting-row{display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin-bottom:14px}.setting-label{flex:1;min-width:180px;font-size:13px}.setting-label small{display:block;color:var(--text-2);font-size:12px;line-height:1.6;margin-top:4px}.setting-value{display:flex;align-items:center;gap:6px;font-size:12px}.setting-value input{width:100px}.effect{color:var(--text-2);font-size:12px}.error{color:var(--danger);font-size:13px}.note{color:var(--green);font-size:13px}
</style>
