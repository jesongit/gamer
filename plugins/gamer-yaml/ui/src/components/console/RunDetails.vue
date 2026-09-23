<template>
  <section class="run-details-view" aria-label="运行详情">
    <div class="details-toolbar">
      <button class="btn" @click="$emit('edit')">返回编辑</button>
      <select v-model="journal.selected" class="select" aria-label="选择运行记录"><option value="" disabled>暂无运行记录</option><option v-for="r in journal.records" :key="r.run_id" :value="r.run_id">{{ new Date(r.started_at).toLocaleString() }} · {{ r.entrypoint }} · {{ stateText(r.state) }}</option></select>
      <button class="btn" :disabled="!journal.record || journal.loading || journal.hasMore" @click="copy">{{ copied ? '已复制' : '复制日志' }}</button><button class="btn" @click="journal.history(); journal.refresh()">刷新</button>
    </div>
    <div class="timeline-hint"><span>当前设备的全部运行 · 切换脚本或函数保留所选日志</span><button v-if="journal.hasOlder" class="btn btn-sm" :disabled="journal.olderLoading" @click="journal.loadOlder()">{{ journal.olderLoading ? '正在加载…' : '加载更早运行记录' }}</button></div>
    <div v-if="journal.record" class="run-summary">
      <strong :class="journal.record.state">{{ stateText(journal.record.state) }}</strong><span class="run-target" :title="journal.record.entrypoint">{{ journal.record.entrypoint }}</span><span>{{ elapsed }} 秒</span>
      <span v-if="target && target !== journal.record.entrypoint" class="other-target">此记录来自其他脚本或函数</span>
      <div class="run-counts"><span>{{ steps.length }} 个执行动作</span><span>成功 {{ counts.success }}</span><span v-if="counts.running">运行中 {{ counts.running }}</span><span v-if="counts.failed" class="failed">失败 {{ counts.failed }}</span><span v-if="counts.cancelled">停止 {{ counts.cancelled }}</span></div>
    </div>
    <div v-if="journal.error" class="failure-text" role="alert">{{ journal.error }}</div>
    <div v-if="journal.record?.error" class="failure-text">{{ journal.record.error }}</div>
    <div class="timeline-toolbar">
      <input v-model="query" class="input log-search" type="search" aria-label="搜索运行日志" placeholder="搜索步骤、函数、模板或日志…" />
      <label class="issue-filter"><input v-model="issuesOnly" type="checkbox" /> 仅异常</label>
      <div class="view-switch" role="group" aria-label="日志显示范围"><button :class="{ active: !includeFlow }" :aria-pressed="!includeFlow" @click="includeFlow = false">执行动作</button><button :class="{ active: includeFlow }" :aria-pressed="includeFlow" @click="includeFlow = true">全部步骤</button></div>
    </div>
    <div class="timeline-hint"><span>{{ query.trim() ? '搜索覆盖全部步骤' : includeFlow ? '包含函数调用、循环等流程步骤' : '省略外层调用与重复的失败记录' }} · 点击记录查看详情</span><span>{{ visibleRows.length }} 条</span></div>
    <div ref="stream" class="details-stream" @scroll="onScroll">
      <table v-if="visibleRows.length" class="journal-table" aria-label="运行执行记录">
        <colgroup><col class="time-column" /><col /><col class="state-column" /><col class="duration-column" /></colgroup>
        <thead><tr><th scope="col">时间</th><th scope="col">动作 / 结果</th><th scope="col">状态</th><th scope="col" class="duration-heading">耗时</th></tr></thead>
        <tbody><RunJournalRow v-for="row in visibleRows" :key="`${journal.selected}:${row.node.id}`" :node="row.node" :now="clock" :selected="selectedEvent === row.node.id" @select="selectEvent" /></tbody>
      </table>
      <p v-if="!visibleRows.length" class="empty">{{ rows.length ? '没有符合筛选条件的记录。' : journal.loading ? '正在读取运行详情…' : journal.record?.state === 'starting' ? '正在准备设备与执行环境…' : journal.record ? '本次运行没有步骤记录。' : '运行脚本后在这里查看步骤、匹配结果和日志。' }}</p>
      <p v-if="journal.hasMore" class="empty">正在读取后续记录…</p>
    </div>
    <RunJournalInspector v-if="selectedRow" :key="journal.selected" :row="selectedRow" :now="clock" @close="selectedEvent = null" @locate="$emit('locate', $event)" />
    <div class="details-footer"><span :title="journal.selected">已加载 {{ journal.events.length }} 条事件{{ journal.hasMore ? ' · 继续加载中' : ' · 服务端保存' }}</span><button class="btn btn-sm" @click="scrollStart">查看开头</button><button class="btn btn-sm" @click="resumeFollow">{{ follow && !filtering ? '正在跟随最新记录' : '回到最新' }}</button></div>
  </section>
</template>
<script setup>
import { computed, nextTick, onScopeDispose, reactive, ref, toRef, watch } from 'vue'
import { useRunJournal } from './useRunJournal'
import { actionRunRows, buildRunTree, filterRunRows, flattenRunTree, stateText } from './run-journal'
import RunJournalRow from './RunJournalRow.vue'
import RunJournalInspector from './RunJournalInspector.vue'
const props = defineProps({ deviceId: String, target: String, liveRun: String, visible: { type: Boolean, default: true } })
defineEmits(['edit', 'locate'])
const journal = reactive(useRunJournal(toRef(props, 'deviceId'), toRef(props, 'liveRun'), toRef(props, 'visible')))
const tree = computed(() => buildRunTree(journal.events, journal.hasMore ? null : journal.record))
const rows = computed(() => flattenRunTree(tree.value))
const steps = computed(() => actionRunRows(rows.value).filter(row => row.node.kind === 'step'))
const counts = computed(() => steps.value.reduce((result, { node }) => { result[node.state]++; return result }, { success: 0, failed: 0, running: 0, cancelled: 0 }))
const query = ref(''), issuesOnly = ref(false), includeFlow = ref(false), selectedEvent = ref(null)
const selectedRow = computed(() => rows.value.find(row => row.node.id === selectedEvent.value))
const filtering = computed(() => !!query.value.trim() || issuesOnly.value)
const visibleRows = computed(() => filterRunRows(rows.value, { query: query.value, issuesOnly: issuesOnly.value, includeFlow: includeFlow.value }))
const clock = ref(Date.now()), stream = ref(null), follow = ref(false), copied = ref(false)
const timer = setInterval(() => { clock.value = Date.now() }, 1000)
onScopeDispose(() => clearInterval(timer))
const elapsed = computed(() => (Math.max(0, (Date.parse(journal.record?.finished_at) || clock.value) - Date.parse(journal.record?.started_at || new Date())) / 1000).toFixed(1))
function selectEvent(id) { selectedEvent.value = id; follow.value = false }
function onScroll() { const el = stream.value; if (el) follow.value = el.scrollHeight - el.scrollTop - el.clientHeight < 48 }
function scrollBottom() { const id = journal.selected; nextTick(() => { if (stream.value && follow.value && id === journal.selected) stream.value.scrollTop = stream.value.scrollHeight }) }
function scrollStart() { follow.value = false; if (stream.value) stream.value.scrollTop = 0 }
function resumeFollow() { query.value = ''; issuesOnly.value = false; follow.value = true; scrollBottom() }
watch(() => journal.events.length, () => { if (follow.value && !filtering.value) scrollBottom() })
watch(() => journal.selected, () => {
  follow.value = !!props.liveRun && journal.selected === props.liveRun
  copied.value = false; query.value = ''; issuesOnly.value = false; selectedEvent.value = null
  if (!follow.value) nextTick(scrollStart)
}, { immediate: true })
async function copy() {
  try { await navigator.clipboard.writeText(JSON.stringify({ run: journal.record, events: journal.events }, null, 2)); copied.value = true }
  catch (e) { journal.error = `复制失败：${e.message}` }
}
</script>
<style scoped>
.run-details-view{display:flex;flex-direction:column;flex:1;min-height:0;min-width:0;gap:8px}.details-toolbar,.details-footer,.run-summary,.timeline-toolbar{display:flex;align-items:center;gap:8px;flex-wrap:wrap}.details-toolbar .select{flex:1;min-width:140px;max-width:100%}.details-stream{flex:1;min-height:0;overflow:auto;overflow-anchor:none;border:1px solid var(--border);border-radius:4px}.run-summary{font-size:13px;padding:10px;background:var(--bg-0);border-radius:6px}.run-target{flex:1;min-width:80px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.other-target{flex-basis:100%;font-size:12px;color:var(--text-2)}.run-counts{display:flex;gap:12px;flex-basis:100%;font-size:12px;color:var(--text-2);flex-wrap:wrap}.failed,.failure-text{color:var(--danger)}.success{color:var(--ok)}.running,.starting{color:var(--accent)}.failure-text{font-size:12px;white-space:pre-wrap;overflow-wrap:anywhere;max-height:100px;overflow:auto}.log-search{flex:1;min-width:160px;width:0}.issue-filter{display:flex;align-items:center;gap:5px;font-size:12px;white-space:nowrap;cursor:pointer}.issue-filter input{accent-color:var(--accent)}.timeline-hint,.details-footer{display:flex;justify-content:space-between;gap:8px;font-size:12px;color:var(--text-2)}.empty{color:var(--text-2);padding:16px;font-size:13px}
.journal-table{width:100%;table-layout:fixed;border-collapse:separate;border-spacing:0}.time-column{width:76px}.state-column{width:62px}.duration-column{width:58px}.journal-table th{position:sticky;top:0;z-index:1;text-align:left;background:var(--bg-2);border-bottom:1px solid var(--border);font-weight:500;font-size:12px;color:var(--text-2);padding:8px}.journal-table .duration-heading{text-align:right}.view-switch{display:flex;flex:none;border:1px solid var(--border);border-radius:4px;overflow:hidden}.view-switch button{font:inherit;font-size:12px;cursor:pointer;border:0;padding:5px 9px;background:var(--bg-0);color:var(--text-2)}.view-switch button.active{background:var(--bg-3);color:var(--accent)}.view-switch button:focus-visible{outline:2px solid var(--accent);outline-offset:-2px}.details-footer>span{margin-right:auto}.timeline-hint>span:last-child{flex:none}.timeline-hint>span:first-child{min-width:0}@container(max-width:420px){.time-column{width:68px}.state-column{width:54px}.duration-column{width:48px}}.run-details-view{container-type:inline-size}
</style>
