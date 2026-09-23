<template>
  <section class="card settings-card" aria-label="通用设置">
    <div class="settings-head"><b>通用设置</b><button class="btn btn-sm" :disabled="busy" @click="load">重新读取</button></div>
    <p v-if="loading">正在读取设置…</p>
    <form v-if="form" @submit.prevent="save">
      <fieldset :disabled="busy">
        <label v-for="field in fields" :key="field.key" class="setting-row">
          <span class="setting-label">{{ field.label }}<small>{{ field.help }}</small></span>
          <span class="setting-value"><input class="input" type="number" :aria-label="field.label" :min="field.min" :max="field.max" step="1" :value="getValue(field.key)" @input="setValue(field.key, $event.target.value)" /><span>{{ field.unit }}</span></span>
          <span class="effect">{{ field.effect }}</span>
        </label>
        <p class="hint">普通登录时限适用于未勾选“保持登录”的会话；保持登录仍为 30 天。</p>
        <p v-if="view.restart_required" class="pending" role="status">计算并发已保存，重启服务后生效。当前设置：{{ view.active.compute_max_concurrency || '自动' }}。</p>
        <button class="btn btn-primary btn-sm" type="submit">{{ saving ? '保存中…' : '保存通用设置' }}</button>
      </fieldset>
    </form>
    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <p v-if="note" class="note" role="status">{{ note }}</p>
  </section>
</template>

<script setup>
import { computed, onMounted, ref } from 'vue'
import { api } from '../api'
const view = ref(null), form = ref(null), loading = ref(false), saving = ref(false), error = ref(''), note = ref('')
const busy = computed(() => loading.value || saving.value)
const fields = [
  { key: 'idle_power_secs', label: '空闲省电时间', unit: '秒', min: 0, max: 604800, effect: '即时生效', help: '无人查看、无任务和录制时进入省电；0 为关闭，最多约 10 秒后采用新值。' },
  { key: 'log_retain_days', label: '日志保留天数', unit: '天', min: 0, max: 36500, effect: '下次清理生效', help: '0 为不自动清理；保存时不立即删除已有日志。' },
  { key: 'compute_max_concurrency', label: '计算并发上限', unit: '个', min: 0, max: 256, effect: '重启后生效', help: '模板匹配、图片解码等计算的并发上限；0 为按 CPU 自动选择。' },
  { key: 'session_abs_secs', label: '普通登录有效期', unit: '秒', min: 60, max: 2592000, effect: '下次登录生效', help: '从登录时开始计时，已有会话保持原期限。' },
  { key: 'session_idle_secs', label: '普通登录空闲有效期', unit: '秒', min: 60, max: 604800, effect: '下次登录生效', help: '持续不活动多久后需要重新登录。' },
  { key: 'login_max_fails', label: '登录失败次数上限', unit: '次', min: 1, max: 1000, effect: '即时生效', help: '同一来源在统计窗口内达到上限后，暂时拒绝登录。' },
  { key: 'login_window_secs', label: '登录失败统计窗口', unit: '秒', min: 1, max: 86400, effect: '即时生效', help: '后续登录请求使用新策略。' },
]
function getValue(key) { return key in form.value ? form.value[key] : form.value.auth[key] }
function setValue(key, value) { (key in form.value ? form.value : form.value.auth)[key] = value === '' ? '' : Number(value) }
function apply(data) { view.value = data; form.value = structuredClone(data.saved) }
async function load() {
  loading.value = true; error.value = ''; note.value = ''
  try { apply(await api.getSystemSettings()) } catch (e) { error.value = e.message } finally { loading.value = false }
}
async function save() {
  if (busy.value) return
  error.value = ''; note.value = ''
  const invalid = fields.find(f => !Number.isInteger(getValue(f.key)) || getValue(f.key) < f.min || getValue(f.key) > f.max)
  if (invalid) { error.value = `${invalid.label}须为 ${invalid.min}～${invalid.max} 的整数`; return }
  saving.value = true
  try {
    apply(await api.saveSystemSettings(form.value, view.value.revision))
    note.value = view.value.restart_required ? '已保存。计算并发等待重启，其余设置按标注时机生效。' : '已保存，各项按标注时机生效。'
  } catch (e) { error.value = e.message } finally { saving.value = false }
}
onMounted(load)
</script>

<style scoped>
.settings-card{padding:16px;margin-bottom:16px}.settings-head{display:flex;justify-content:space-between;align-items:center;margin-bottom:12px}fieldset{border:0;padding:0;margin:0;min-width:0}.setting-row{display:flex;gap:12px;align-items:center;flex-wrap:wrap;padding:10px 0;border-bottom:1px solid var(--border)}.setting-label{flex:1;min-width:180px;font-size:13px}.setting-label small{display:block;font-size:12px;color:var(--text-2);line-height:1.6;margin-top:4px}.setting-value{display:flex;align-items:center;gap:6px;font-size:12px}.setting-value input{width:100px}.effect{font-size:12px;color:var(--text-2);min-width:96px}.hint{font-size:12px;color:var(--text-2);line-height:1.6}.pending{font-size:12px;color:var(--warn)}.error{color:var(--danger);font-size:13px}.note{color:var(--green);font-size:13px}
</style>
