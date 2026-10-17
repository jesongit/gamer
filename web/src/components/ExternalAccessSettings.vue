<script setup>
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { api } from '../api'
import { useConfirmDialog } from './ui/useConfirmDialog'

const confirm = useConfirmDialog()
const tokens = ref([]), sessions = ref([]), devices = ref([]), packages = ref([]), plugins = ref([]), runners = ref([]), catalog = ref(null)
const busy = ref(false), loading = ref(false), error = ref(''), note = ref(''), creating = ref(false), secret = ref('')
const label = ref(''), lifetime = ref(24), scopes = ref(['observe'])
const allowed = ref({ devices: [], packages: [], plugins: [], runners: [] })
const objectIds = ref({ runs: '', tasks: '', media: '' })
const extraRunnerIds = ref('')
const tokenId = ref(''), deviceId = ref(''), packageId = ref(''), ttl = ref(300)
let timer, disposed = false
const names = { devices: '目标', packages: '配置包', plugins: '插件', runners: '执行器' }
const choices = computed(() => ({ devices: devices.value, packages: packages.value, plugins: plugins.value, runners: runners.value }))
const availableScopes = computed(() => [...new Set(['observe', 'control', 'resource.read', 'run.read', 'run.submit', 'run.cancel', 'task.read', 'task.write', 'task.control', 'media.read', 'media.record', ...(catalog.value?.tools || []).flatMap(t => t.required_scopes || [])])].sort())
const activeTokens = computed(() => tokens.value.filter(t => !t.revoked && t.expires_at * 1000 > Date.now() && t.scopes.includes('control')))
const selectedToken = computed(() => activeTokens.value.find(t => t.token_id === tokenId.value))
const includes = (values, id) => values?.includes('*') || values?.includes(id)
const controlDevices = computed(() => devices.value.filter(d => includes(selectedToken.value?.allowed?.devices, d.id)))
const controlPackages = computed(() => packages.value.filter(p => includes(selectedToken.value?.allowed?.packages, p.id)))
const phaseNames = { starting: '正在开始', running: '外部控制中', pausing: '正在暂停', paused: '已暂停，可人工操作', resuming: '正在恢复', stopping: '正在收尾', finished: '已结束' }
const canStart = computed(() => !!selectedToken.value && controlDevices.value.some(d => d.id === deviceId.value) && (!packageId.value || controlPackages.value.some(p => p.id === packageId.value)) && !busy.value && Number.isInteger(ttl.value) && ttl.value >= 10 && ttl.value <= 86400)
function lists(value, key) { return Array.isArray(value) ? value : value?.[key] || [] }
function splitIds(value) { return [...new Set(value.split(/[\s,，]+/).map(s => s.trim()).filter(Boolean))] }
async function refresh() {
  if (loading.value) return
  loading.value = true
  try {
    const values = await Promise.allSettled([api.listMcpTokens(), api.listExternalControl(), api.listDevices(), api.listPackages(), api.listExtensions(), api.listRunners(), api.getMcpCatalog()])
    if (disposed) return
    const [t, s, d, p, x, r, c] = values
    if (t.status === 'fulfilled') tokens.value = lists(t.value, 'tokens')
    if (s.status === 'fulfilled') sessions.value = lists(s.value, 'sessions')
    if (d.status === 'fulfilled') devices.value = lists(d.value, 'devices').map(v => ({ id: v.id, name: v.name || v.id }))
    if (p.status === 'fulfilled') packages.value = lists(p.value, 'packages').map(v => ({ id: v.id, name: v.name || v.id }))
    if (x.status === 'fulfilled') plugins.value = lists(x.value, 'extensions').map(v => ({ id: v.id, name: v.name || v.id }))
    if (r.status === 'fulfilled') runners.value = lists(r.value, 'runners').map(v => ({ id: v.runner_id, name: v.runner_id }))
    if (c.status === 'fulfilled') catalog.value = c.value
    const failed = values.find(v => v.status === 'rejected')
    error.value = failed ? `部分接入信息读取失败：${failed.reason?.message || '请重试'}` : ''
  } catch (e) { if (!disposed) error.value = e.message }
  finally { loading.value = false }
}
async function operation(fn, message) {
  if (busy.value) return
  busy.value = true; error.value = ''; note.value = ''
  try { await fn(); if (!disposed) { note.value = message; await refresh() } }
  catch (e) { if (!disposed) error.value = e.message }
  finally { busy.value = false }
}
async function create() {
  if (!label.value.trim() || !scopes.value.length || !Number.isInteger(lifetime.value) || lifetime.value < 1 || lifetime.value > 8784) {
    error.value = '请填写名称、至少一项权限和 1～8784 小时的有效期。'; return
  }
  await operation(async () => {
    const created = await api.createMcpToken({ label: label.value.trim(), scopes: scopes.value,
      allowed: { ...allowed.value, runners:[...new Set([...allowed.value.runners,...splitIds(extraRunnerIds.value)])], ...Object.fromEntries(Object.entries(objectIds.value).map(([key, value]) => [key, splitIds(value)])) }, expires_in_seconds: lifetime.value * 3600 })
    if (!disposed) { secret.value = created.token; creating.value = false }
  }, '令牌已创建，请保存下方仅显示一次的密钥。')
}
async function revoke(token) {
  if (!await confirm(`撤销“${token.label}”后将停止其直接控制和私有试跑。已提交的正式运行与定时任务继续执行，可在任务和日志中查看。`, { title: '撤销 MCP 令牌', confirmText: '撤销令牌', danger: true })) return
  await operation(() => api.revokeMcpToken(token.token_id), '令牌已撤销，直接控制和私有试跑已进入停止收尾。')
}
async function start() {
  if (!canStart.value) return
  await operation(() => api.startExternalControl({ token_id: tokenId.value, device_id: deviceId.value, content_package: packageId.value || null, ttl_seconds: ttl.value }), '已开始外部控制。Agent 需要先读取会话并截图。')
}
async function control(session, action) {
  await operation(() => api.externalControlAction(session.session_id, action, action === 'pause' ? { generation: session.generation } : {}), action === 'resume' ? '已恢复外部控制，Agent 必须重新截图。' : action === 'pause' ? '已暂停；动作收尾完成后允许人工操作。' : '已请求结束；以会话显示的收尾状态为准。')
}
function expiry(token) { return token.revoked ? '已撤销' : new Date(token.expires_at * 1000).toLocaleString() }
async function poll() { await refresh(); if (!disposed) timer = setTimeout(poll, 3000) }
onMounted(poll)
onBeforeUnmount(() => { disposed = true; clearTimeout(timer); secret.value = '' })
</script>

<template>
  <section class="card external-access" aria-label="MCP 与外部控制">
    <div class="section-head"><b>MCP 与外部控制</b><button class="btn btn-sm" :disabled="loading || busy" @click="refresh">{{ loading ? '读取中…' : '刷新接入状态' }}</button></div>
    <p class="hint">在自己的 Agent 中连接 Gamer，按令牌授权读取资料、编辑工作稿和执行操作。MCP 仅接受服务所在电脑的本机连接。</p>
    <p class="endpoint"><span>连接地址</span><code>{{ catalog?.endpoint || '正在读取本机 MCP 地址…' }}</code></p>
    <p class="hint">使用 Streamable HTTP，在 Authorization 请求头中填写 Bearer 令牌。令牌与网页登录独立，正式发布和版本回退仍在 Gamer 中确认。</p>

    <div class="section-head"><h3>接入令牌</h3><button class="btn btn-sm" :disabled="busy" @click="creating = !creating">{{ creating ? '收起创建表单' : '创建令牌' }}</button></div>
    <form v-if="creating" class="token-form" @submit.prevent="create">
      <fieldset :disabled="busy"><div class="form-row"><label>名称<input v-model="label" class="input" required maxlength="80" placeholder="例如：我的自动化 Agent" /></label><label>有效期（小时）<input v-model.number="lifetime" class="input" type="number" min="1" max="8784" step="1" required /></label></div>
      <div class="scope-group"><b>允许的操作</b><div class="checks"><label v-for="scope in availableScopes" :key="scope"><input v-model="scopes" type="checkbox" :value="scope" />{{ scope }}</label></div></div>
      <div v-for="(items, kind) in choices" :key="kind" class="scope-group"><b>{{ names[kind] }}范围</b><div class="checks"><label><input v-model="allowed[kind]" type="checkbox" value="*" />全部{{ names[kind] }}（含以后新增）</label><label v-for="item in items" :key="item.id"><input v-model="allowed[kind]" type="checkbox" :value="item.id" :disabled="allowed[kind].includes('*')" />{{ item.name }}<small v-if="item.name !== item.id">{{ item.id }}</small></label></div><p v-if="!items.length" class="hint">暂无可选{{ names[kind] }}；未勾选时不授予该类对象的访问。</p></div>
      <details><summary>其他执行器与既有对象范围</summary><p class="hint">默认只能管理本令牌创建的运行和任务。访问已有对象请填写准确 ID，以空格或逗号分隔；填写 * 表示全部对象。插件私有试跑执行器可按插件指南填写。</p><label>其他执行器 ID<input v-model="extraRunnerIds" class="input" autocomplete="off" /></label><label v-for="kind in ['runs', 'tasks', 'media']" :key="kind">{{ { runs: '运行 ID', tasks: '任务 ID', media: '媒体／素材 ID' }[kind] }}<input v-model="objectIds[kind]" class="input" autocomplete="off" /></label></details>
      <button class="btn btn-primary btn-sm" type="submit">{{ busy ? '创建中…' : '按所选范围创建' }}</button></fieldset>
    </form>
    <div v-if="secret" class="secret"><label>仅显示一次的令牌<textarea :value="secret" readonly spellcheck="false" autocomplete="off" aria-label="新建 MCP 令牌" /></label><p class="hint">保存到你的 Agent 凭据设置。此处关闭后无法再次读取；不要写进配置包或分享给他人。</p><button class="btn btn-sm" @click="secret = ''">我已保存，隐藏令牌</button></div>
    <p v-if="!tokens.length && !loading" class="hint">尚未创建令牌。先选择需要的操作和对象范围。</p>
    <ul class="tokens"><li v-for="token in tokens" :key="token.token_id"><div><b>{{ token.label }}</b><small>{{ token.token_id }}</small><span>{{ token.revoked ? '已撤销' : `有效至 ${expiry(token)}` }}</span><details><summary>查看授权范围</summary><p>{{ token.scopes.join('、') }}</p><p v-for="(ids, kind) in token.allowed" :key="kind">{{ names[kind] || kind }}：{{ ids.length ? ids.join('、') : '未授予' }}</p></details></div><button class="btn btn-sm" :disabled="busy || token.revoked" @click="revoke(token)">撤销</button></li></ul>

    <h3>外部控制</h3><p class="hint">由你选择令牌和目标后开始。控制中人工输入锁定；暂停完成后可人工接手，但仍占用该目标的运行槽。转为运行脚本前须结束控制并等待收尾。</p>
    <form class="control-form" @submit.prevent="start"><fieldset :disabled="busy"><div class="form-row"><label>令牌<select v-model="tokenId" class="select" @change="deviceId = ''; packageId = ''"><option value="">选择有控制权限的令牌</option><option v-for="token in activeTokens" :key="token.token_id" :value="token.token_id">{{ token.label }}</option></select></label><label>目标<select v-model="deviceId" class="select"><option value="">选择授权目标</option><option v-for="device in controlDevices" :key="device.id" :value="device.id">{{ device.name }}</option></select></label></div><div class="form-row"><label>配置包<select v-model="packageId" class="select"><option value="">无配置包</option><option v-for="pkg in controlPackages" :key="pkg.id" :value="pkg.id">{{ pkg.name }}</option></select></label><label>失联暂停时间（秒）<input v-model.number="ttl" class="input" type="number" min="10" max="86400" step="1" /></label></div><button class="btn btn-primary btn-sm" type="submit" :disabled="!canStart">开始外部控制</button></fieldset></form>
    <p v-if="!sessions.length && !loading" class="hint">当前没有外部控制会话。</p>
    <ul class="sessions"><li v-for="session in sessions" :key="session.session_id"><div><b>{{ session.device_id }} · {{ phaseNames[session.state] || session.state }}</b><span>{{ session.token_label }} · {{ session.content_package || '无配置包' }}</span><small>{{ session.session_id }} · 代次 {{ session.generation }} · 剩余 {{ session.remaining_seconds }} 秒</small><p v-if="session.reason" class="hint">{{ session.reason }}</p></div><div class="actions"><button v-if="session.state === 'running'" class="btn btn-sm" :disabled="busy" @click="control(session, 'pause')">暂停并人工接手</button><button v-if="session.state === 'paused'" class="btn btn-sm" :disabled="busy" @click="control(session, 'resume')">恢复外部控制</button><button v-if="session.state !== 'finished'" class="btn btn-sm" :disabled="busy" @click="control(session, 'stop')">结束控制</button></div></li></ul>
    <details v-if="catalog?.tools?.length" class="tool-catalog"><summary>查看当前可用工具（{{ catalog.tools.length }}）</summary><ul><li v-for="tool in catalog.tools" :key="tool.name"><code>{{ tool.name }}</code><p>{{ tool.description }}</p></li></ul></details>
    <p v-if="error" class="error" role="alert">{{ error }}</p><p v-if="note" class="note" role="status">{{ note }}</p>
  </section>
</template>

<style scoped>
.external-access{padding:16px;margin-bottom:16px}.section-head{display:flex;gap:12px;align-items:center;justify-content:space-between;flex-wrap:wrap}.section-head h3{margin:0}h3{font-size:14px;margin:22px 0 10px}.hint,small{font-size:12px;color:var(--text-2);line-height:1.6}.hint{max-width:75ch}small{display:block}.endpoint{display:flex;gap:12px;flex-wrap:wrap;font-size:13px}code{overflow-wrap:anywhere}.token-form,.control-form,.secret{margin:12px 0;padding:12px 0;border-top:1px solid var(--border);border-bottom:1px solid var(--border)}fieldset{border:0;padding:0;margin:0;min-width:0}label{font-size:13px}.form-row{display:flex;gap:12px;flex-wrap:wrap;margin-bottom:12px}.form-row>label{flex:1;min-width:min(200px,100%)}.form-row input,.form-row select,.token-form details input{display:block;width:100%;margin-top:5px}.scope-group{margin:16px 0}.scope-group>b{font-size:13px}.checks{display:flex;gap:10px 18px;flex-wrap:wrap;margin-top:8px}.checks label{display:flex;align-items:center;gap:6px;min-height:32px;overflow-wrap:anywhere}.checks input{flex:none}.checks small{margin-left:4px}.token-form details{margin-bottom:16px}.token-form details>label{display:block;margin:10px 0}summary{font-size:13px;cursor:pointer;min-height:32px;align-content:center}textarea{display:block;width:100%;resize:vertical;min-height:70px;background:var(--field);color:var(--text-0);border:1px solid var(--border);padding:8px;font-size:13px}.tokens,.sessions,.tool-catalog ul{list-style:none;padding:0;margin:0}.tokens>li,.sessions>li{display:flex;align-items:flex-start;justify-content:space-between;gap:12px;padding:12px 0;border-bottom:1px solid var(--border)}.tokens>li>div,.sessions>li>div{min-width:0}.tokens b,.sessions b,.tokens span,.sessions span{display:block;font-size:13px;line-height:1.6}.tokens p,.sessions p{margin:5px 0;font-size:12px;overflow-wrap:anywhere}.actions{display:flex;gap:8px;flex-wrap:wrap;justify-content:flex-end}.tool-catalog{margin-top:20px}.tool-catalog li{padding:8px 0;font-size:12px}.tool-catalog p{margin:4px 0;color:var(--text-2);line-height:1.6;max-width:75ch}.error{color:var(--danger);font-size:13px}.note{color:var(--ok);font-size:13px}@media(max-width:600px){.tokens>li,.sessions>li{flex-direction:column}.actions{justify-content:flex-start}.form-row>label{flex-basis:100%}.checks{gap:6px 12px}}
</style>
