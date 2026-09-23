import { computed, onScopeDispose, reactive, ref, watch } from 'vue'
import { api } from '../../../../../../web/src/api'

// Script and function panels share the selection, including after a panel remount.
// Only IDs are shared: each mounted view owns its requests and event buffer.
const selections = reactive(new Map())
const observedLiveRuns = new Map()
const HISTORY_PAGE_SIZE = 30
export function useRunJournal(device, liveRun, visible = ref(true)) {
  const records = ref([]), events = ref([]), error = ref(''), loading = ref(false), hasMore = ref(false)
  const hasOlder = ref(false), olderLoading = ref(false)
  const selected = computed({ get: () => selections.get(device.value) || '', set: id => { if (device.value) selections.set(device.value, id) } })
  const record = computed(() => records.value.find(r => r.run_id === selected.value))
  let scope = 0, generation = 0, cursor = 0, inFlight = null, ticks = 0, historyInFlight = null, olderCursor = null
  function mergeRecords(list) {
    records.value = [...new Map([...records.value, ...list].map(r => [r.run_id, r])).values()]
      .sort((a, b) => b.started_at.localeCompare(a.started_at) || b.run_id.localeCompare(a.run_id))
  }
  async function history() {
    const token = scope
    if (!device.value || historyInFlight === token) return
    historyInFlight = token
    try {
      const list = await api.listRunHistory(device.value)
      if (token !== scope) return
      mergeRecords(list)
      if (olderCursor === null) {
        olderCursor = list.at(-1)?.run_id || null
        hasOlder.value = list.length === HISTORY_PAGE_SIZE
      }
      if (!selected.value) selected.value = list[0]?.run_id || ''
    } catch (e) { if (token === scope) error.value = `读取运行历史失败：${e.message}` }
    finally { if (token === scope) historyInFlight = null }
  }
  async function loadOlder() {
    if (!hasOlder.value || olderLoading.value || !olderCursor) return
    const token = scope
    olderLoading.value = true
    try {
      const list = await api.listRunHistory(device.value, undefined, olderCursor)
      if (token !== scope) return
      mergeRecords(list)
      olderCursor = list.at(-1)?.run_id || olderCursor
      hasOlder.value = list.length === HISTORY_PAGE_SIZE
    } catch (e) { if (token === scope) error.value = `读取更早运行失败：${e.message}` }
    finally { if (token === scope) olderLoading.value = false }
  }
  async function refresh() {
    if (!selected.value || inFlight === generation) return
    const token = generation, id = selected.value
    inFlight = token; loading.value = true
    try {
      const current = await api.getRun(id)
      if (token !== generation) return
      mergeRecords([current])
      for (let page = 0; page < 4; page++) {
        const result = await api.getRunEvents(id, cursor)
        if (token !== generation) return
        events.value.push(...result.events)
        cursor = result.next; hasMore.value = result.has_more
        if (!result.has_more) break
      }
      error.value = ''
    } catch (e) { if (token === generation) error.value = `读取运行详情失败：${e.message}` }
    finally { if (token === generation) { loading.value = false; inFlight = null } }
  }
  watch(device, () => {
    scope++; generation++; records.value = []; events.value = []; error.value = ''; loading.value = false
    cursor = 0; hasMore.value = false; hasOlder.value = false; olderLoading.value = false; olderCursor = null
    history()
  }, { immediate: true, flush: 'sync' })
  watch([device, selected], () => {
    generation++; cursor = 0; events.value = []; hasMore.value = false; error.value = ''; loading.value = false
    refresh()
  }, { immediate: true, flush: 'sync' })
  watch([device, liveRun], ([deviceId, id]) => {
    // Reopening a panel must not take selection away from an older run being inspected.
    if (deviceId && id && observedLiveRuns.get(deviceId) !== id) {
      observedLiveRuns.set(deviceId, id); selected.value = id; history()
    }
  }, { immediate: true })
  watch(visible, value => { if (value) { history(); refresh() } })
  const timer = setInterval(() => { if (visible.value) { refresh(); if (++ticks % 5 === 0) history() } }, 1000)
  onScopeDispose(() => { clearInterval(timer); scope++; generation++ })
  return { records, selected, events, record, error, loading, hasMore, hasOlder, olderLoading, refresh, history, loadOlder }
}
