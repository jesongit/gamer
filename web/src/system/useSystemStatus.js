// System/Update 状态 store + 轮询 composable（WEB-001）。
// 契约 §4.1：动作 202 受理后以轮询 GET /api/system/update 取进展（浏览器不等长请求）。
// 轮询节奏（WEB-003 验收）：活跃更新态（checking/downloading/installing/restarting/
// rolling_back——进度快速推进、随时可能跃迁）高频；其余（含 idle/failed 等驻留态）低频。
// 组件内使用（useSystemStatus）卸载时自动停止轮询；页面外单测可直接用 createSystemStatus。
import { reactive, getCurrentInstance, onUnmounted } from 'vue'
import { systemApi } from './api'
import { isUpdateState } from './states'

export const POLL_FAST_MS = 2000   // 活跃更新态轮询间隔
export const POLL_SLOW_MS = 30000  // 驻留态（含 idle）轮询间隔

/** 高频轮询的活跃状态集合（契约 §5.1 中会快速推进/跃迁的 5 个瞬态） */
export const FAST_POLL_STATES = Object.freeze([
  'checking', 'downloading', 'installing', 'restarting', 'rolling_back',
])

export function isActiveUpdateState(state) {
  return FAST_POLL_STATES.includes(state)
}

/**
 * 创建一套独立的 system/update 状态控制器（响应式 st + refresh + 轮询开关）。
 * fetchInfo/fetchUpdate 可注入替换（单测不触网）；默认走 systemApi。
 * refresh 对两个端点分别落成败（allSettled），单端点故障不影响另一端点的展示。
 */
export function createSystemStatus({ fetchInfo, fetchUpdate, fastMs = POLL_FAST_MS, slowMs = POLL_SLOW_MS } = {}) {
  const getInfo = fetchInfo || systemApi.getSystemInfo
  const getUpdate = fetchUpdate || systemApi.getUpdateStatus
  const st = reactive({
    info: null,        // GET /api/system/info 成功响应
    update: null,      // GET /api/system/update 成功响应
    infoError: null,   // 最近一次 info 拉取失败对象（ApiError）
    updateError: null, // 最近一次 update 拉取失败对象（ApiError）
    loading: false,
    polling: false,
  })
  let timer = null
  let stopped = true

  async function refresh() {
    st.loading = true
    const [i, u] = await Promise.allSettled([getInfo(), getUpdate()])
    st.info = i.status === 'fulfilled' ? i.value : st.info
    st.infoError = i.status === 'fulfilled' ? null : i.reason
    st.update = u.status === 'fulfilled' ? u.value : st.update
    st.updateError = u.status === 'fulfilled' ? null : u.reason
    st.loading = false
    return st
  }

  function nextDelay() {
    return isActiveUpdateState(st.update && st.update.state) ? fastMs : slowMs
  }

  async function tick() {
    if (stopped) return
    await refresh()
    if (stopped) return
    timer = setTimeout(tick, nextDelay())
  }

  function startPolling() {
    if (!stopped) return
    stopped = false
    st.polling = true
    timer = setTimeout(tick, 0)
  }

  function stopPolling() {
    stopped = true
    st.polling = false
    if (timer) { clearTimeout(timer); timer = null }
  }

  return { st, refresh, startPolling, stopPolling }
}

/**
 * 页面级便捷封装：默认立即 refresh 并开始轮询（auto:false 只取不轮，poll:false 只刷不轮）；
 * 在组件 setup 内使用时，卸载自动停止轮询（防泄漏/防后台轰炸）。
 */
export function useSystemStatus(options = {}) {
  const ctl = createSystemStatus(options)
  if (getCurrentInstance()) onUnmounted(() => ctl.stopPolling())
  if (options.auto !== false) {
    ctl.refresh()
    if (options.poll !== false) ctl.startPolling()
  }
  return ctl
}
