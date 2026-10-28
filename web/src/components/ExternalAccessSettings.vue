<script setup>
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { api } from '../api'
import { useConfirmDialog } from './ui/useConfirmDialog'

const confirm = useConfirmDialog()
const tokens = ref([]), endpoint = ref(''), secrets = ref({})
const label = ref('外部 Agent'), lifetime = ref(0)
const loading = ref(false), busy = ref(false), error = ref(''), note = ref(''), readFailed = ref(false)
let disposed = false

async function refresh() {
  if (loading.value) return
  loading.value = true
  const results = await Promise.allSettled([api.listMcpTokens(), api.getMcpCatalog()])
  if (!disposed) {
    if (results[0].status === 'fulfilled') {
      tokens.value = results[0].value.tokens || []
      const ids = new Set(tokens.value.map(token => token.token_id))
      secrets.value = Object.fromEntries(Object.entries(secrets.value).filter(([id]) => ids.has(id)))
    }
    if (results[1].status === 'fulfilled') endpoint.value = results[1].value.endpoint || ''
    const failed = results.find(result => result.status === 'rejected')
    readFailed.value = !!failed
    error.value = failed ? `读取失败：${failed.reason?.message || '请重试'}` : ''
  }
  loading.value = false
}
async function operation(action) {
  if (busy.value) return
  busy.value = true; error.value = ''; note.value = ''; readFailed.value = false
  try { await action() }
  catch (failure) { if (!disposed) error.value = failure.message || '操作失败，请重试。' }
  finally { busy.value = false }
}
async function create() {
  if (!label.value.trim() || !Number.isInteger(lifetime.value) || lifetime.value < 0 || lifetime.value > 8784) {
    error.value = '请填写名称，有效期为 0～8784 小时，0 表示不过期。'; return
  }
  await operation(async () => {
    const created = await api.createMcpToken({ label: label.value.trim(), expires_in_seconds: lifetime.value * 3600 })
    if (disposed) return
    secrets.value[created.metadata.token_id] = created.token
    note.value = '令牌已添加。'
    await refresh()
  })
}
async function view(token) {
  if (secrets.value[token.token_id]) { delete secrets.value[token.token_id]; return }
  await operation(async () => {
    const result = await api.viewMcpToken(token.token_id)
    if (!disposed) secrets.value[token.token_id] = result.token
  })
}
async function remove(token) {
  if (!await confirm(`删除“${token.label}”后将无法再使用，并停止它的直接控制和临时试跑。`, { title: '删除令牌', confirmText: '删除', danger: true })) return
  await operation(async () => {
    await api.revokeMcpToken(token.token_id)
    if (disposed) return
    delete secrets.value[token.token_id]
    tokens.value = tokens.value.filter(item => item.token_id !== token.token_id)
    note.value = '令牌已删除。'
  })
}
async function copy(value) {
  error.value = ''; note.value = ''
  try {
    if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(value)
    else {
      const previous = document.activeElement
      const field = document.createElement('textarea')
      field.value = value; field.style.cssText = 'position:fixed;left:-9999px;top:0'
      document.body.append(field)
      try { field.select(); if (!document.execCommand('copy')) throw new Error('请选中内容后手动复制。') }
      finally { field.remove(); previous?.focus() }
    }
    if (!disposed) note.value = '已复制。'
  } catch { if (!disposed) error.value = '复制失败，请选中内容后手动复制。' }
}
function expiry(token) {
  if (token.revoked) return '已撤销'
  if (!token.expires_at) return '不过期'
  if (token.expires_at * 1000 <= Date.now()) return '已过期'
  return `有效至 ${new Date(token.expires_at * 1000).toLocaleString()}`
}
onMounted(refresh)
onBeforeUnmount(() => { disposed = true; secrets.value = {} })
</script>

<template>
  <section class="card external-access" aria-label="MCP 与外部控制">
    <div class="section-head"><h3>MCP 与外部控制</h3><button class="btn btn-sm" :disabled="loading || busy" @click="refresh">{{ loading ? '读取中…' : '刷新' }}</button></div>
    <div class="endpoint"><span>连接地址</span><code>{{ endpoint || '正在读取…' }}</code><button class="btn btn-sm" :disabled="!endpoint" @click="copy(endpoint)">复制地址</button></div>
    <p class="hint">令牌可使用当前 MCP 开放功能。使用 Streamable HTTP，认证为 Authorization: Bearer &lt;令牌&gt;。客户端须运行在 Gamer 所在主机。</p>
    <form class="token-form" @submit.prevent="create">
      <fieldset :disabled="busy">
        <label>名称<input v-model="label" class="input" maxlength="80" required autocomplete="off" /></label>
        <label>有效期（小时）<input v-model.number="lifetime" class="input" type="number" min="0" max="8784" step="1" required aria-describedby="token-lifetime-help" /><span id="token-lifetime-help" class="hint">0 表示不过期</span></label>
        <button class="btn btn-primary btn-sm" type="submit">{{ busy ? '处理中…' : '添加令牌' }}</button>
      </fieldset>
    </form>
    <p v-if="!tokens.length && !loading" class="hint">尚未添加令牌。</p>
    <ul class="tokens" aria-label="接入令牌">
      <li v-for="token in tokens" :key="token.token_id">
        <div class="token-info"><b>{{ token.label }}</b><span class="hint">{{ expiry(token) }}</span>
          <input v-if="secrets[token.token_id]" :value="secrets[token.token_id]" class="input token-secret" :aria-label="`令牌内容：${token.label}`" readonly spellcheck="false" autocomplete="off" />
          <span v-if="!token.can_view && !token.revoked && !secrets[token.token_id]" class="hint">旧令牌无法查看原文，可创建新令牌。</span>
        </div>
        <div class="actions"><button class="btn btn-sm" :disabled="busy || (!token.can_view && !secrets[token.token_id])" @click="view(token)">{{ secrets[token.token_id] ? '隐藏' : '查看' }}</button><button v-if="secrets[token.token_id]" class="btn btn-sm" @click="copy(secrets[token.token_id])">复制</button><button class="btn btn-sm danger-text" :disabled="busy" @click="remove(token)">删除</button></div>
      </li>
    </ul>
    <p v-if="error" class="error" role="alert">{{ error }}<button v-if="readFailed && !busy" class="btn btn-sm" @click="refresh">重试读取</button></p>
    <p v-if="note" class="note" role="status">{{ note }}</p>
  </section>
</template>

<style scoped>
.external-access{padding:16px;margin-bottom:16px}.section-head,.endpoint,.actions{display:flex;align-items:center;gap:10px}.section-head{justify-content:space-between}.section-head h3{font-size:14px;margin:0}.endpoint{flex-wrap:wrap;margin-top:18px;font-size:13px}.endpoint code{min-width:0;overflow-wrap:anywhere;user-select:all}.hint{font-size:12px;color:var(--text-2);line-height:1.6}.token-form{margin:18px 0 6px;padding:16px 0;border-top:1px solid var(--border)}fieldset{display:flex;align-items:flex-start;gap:12px;flex-wrap:wrap;border:0;margin:0;padding:0;min-width:0}label{display:flex;flex-direction:column;gap:6px;font-size:13px;flex:1;min-width:160px}fieldset>.btn{margin-top:24px;min-height:32px}.tokens{list-style:none;padding:0;margin:0}.tokens>li{display:flex;align-items:flex-start;justify-content:space-between;gap:12px;padding:14px 0;border-top:1px solid var(--border)}.token-info{min-width:0;flex:1}.token-info>b,.token-info>.hint{display:block;overflow-wrap:anywhere}.token-info>b{font-size:13px;line-height:1.6}.token-secret{width:100%;margin-top:8px;font-family:var(--mono);user-select:all}.actions{flex-wrap:wrap;justify-content:flex-end}.error,.note{font-size:13px;line-height:1.6;overflow-wrap:anywhere}.error{color:var(--danger)}.error .btn{margin-left:8px}.note{color:var(--ok)}@media(max-width:600px){fieldset>label{flex-basis:100%}fieldset>.btn{margin-top:0}.tokens>li{flex-direction:column}.token-info{width:100%}.actions{justify-content:flex-start}.endpoint{align-items:flex-start}}
</style>
