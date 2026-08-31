// 安装/回滚确认流程（WEB-004）——契约 release/contracts/system-api-v1.md §4.1/§5.1 冻结语义：
// - install/rollback 202 = 已受理进入后台协调器；install 期间 HTTP 服务会重启、连接会断开。
//   断连是「正常路径」，不得判失败：有界轮询重连，服务恢复后以 GET /api/system/update 的
//   state 为主、app.version / startup.boot_id 变化为证据判定结果（计划 §11.6）。
// - 409 update_busy：只提示等待，不做自动重试轰炸（是否重试由用户再次操作决定）。
// - 409 update_not_ready：展示 details.blocking 门禁列表（标签见 states.BLOCKING_LABELS）。
// 依赖全部可注入（api/pollOnce/sleep），纯逻辑可在 node 单测覆盖，不触真网。
import { reactive } from 'vue'
import { systemApi } from './api'
import { isUpdateState } from './states'

export const FLOW_POLL_MS = 2000  // 受理后的轮询间隔
export const FLOW_MAX_TRIES = 150 // 有界等待上限（默认 ~5 分钟），超过按 timeout 提示而非误报失败

/** 从 /api/system/info 响应提取重启判定证据快照（提交动作前的 version/boot_id） */
export function snapshotOf(info) {
  const app = info && typeof info === 'object' ? info.app : null
  const startup = info && typeof info === 'object' ? info.startup : null
  return {
    version: app ? String(app.version ?? '') : '',
    bootId: startup ? String(startup.boot_id ?? '') : '',
  }
}

/**
 * 重连后结果判定（纯函数）：
 * - installing/restarting/rolling_back → waiting（服务还没回来或仍在协调，继续等）；
 * - failed → failed（展示 last_error）；manual_recovery → manual_recovery；
 * - idle → 成功。install（requireEvidence=true）要求重启证据：boot_id 变化、版本变化、
 *   或 detail ∈ {committed, cleaning}——防「事务被取消回到 idle」被误判成安装成功；
 *   rollback 无此要求（回滚成功即回到 idle，此时版本应回到旧值，boot_id 才是主要证据）。
 * 断连期间 poll 失败不进入本函数（调用方按 waiting 处理）。
 */
export function judgeAfterRestart({ before, after, requireEvidence = true } = {}) {
  const out = { verdict: 'waiting', restarted: false, versionChanged: false }
  const update = after && after.update
  if (!update || !isUpdateState(update.state)) return out
  const info = after.info
  const b = before || {}
  const bootId = info && info.startup ? String(info.startup.boot_id ?? '') : ''
  const version = info && info.app ? String(info.app.version ?? '') : ''
  out.restarted = !!b.bootId && !!bootId && bootId !== b.bootId
  out.versionChanged = !!b.version && !!version && version !== b.version
  const state = update.state
  if (state === 'installing' || state === 'restarting' || state === 'rolling_back') return out
  if (state === 'failed') { out.verdict = 'failed'; return out }
  if (state === 'manual_recovery') { out.verdict = 'manual_recovery'; return out }
  if (state === 'idle') {
    const detail = String(update.detail ?? '')
    if (!requireEvidence || out.restarted || out.versionChanged
      || detail === 'committed' || detail === 'cleaning') {
      out.verdict = 'success'
    }
    return out
  }
  // checking/available/downloading/staged/waiting：非本事务终态（异常回跳），继续等
  return out
}

function lastErrorOf(update) {
  const le = update && update.last_error
  if (le && typeof le === 'object' && le.code) {
    return { code: String(le.code), message: String(le.message ?? '更新事务失败'), details: null }
  }
  return { code: (update && update.state) || 'update_failed', message: '更新事务失败', details: null }
}

function defaultPollOnce(client) {
  return async () => {
    try {
      const [info, update] = await Promise.all([client.getSystemInfo(), client.getUpdateStatus()])
      return { ok: true, info, update }
    } catch (e) {
      return { ok: false, error: e } // 断连（含 installing/restarting 期服务重启）：正常路径
    }
  }
}

/**
 * 创建安装/回滚流程控制器。
 *   flow.phase: idle → submitting →（受理成功）waiting → done
 *   flow.verdict（done 时）: success | failed | manual_recovery | timeout | aborted
 *   flow.error: 归一化错误 {status?, code, message, details?}（同步拒绝 / failed 终态的 last_error）
 *   flow.busy: 409 update_busy 时的「等待」提示位（不自动重试）
 */
export function createUpdateFlow({ api, pollOnce, sleep, pollMs = FLOW_POLL_MS, maxTries = FLOW_MAX_TRIES } = {}) {
  const client = api || systemApi
  const wait = sleep || ((ms) => new Promise((res) => setTimeout(res, ms)))
  const doPoll = pollOnce || defaultPollOnce(client)

  const flow = reactive({
    kind: null,        // 'install' | 'rollback'
    phase: 'idle',     // idle | submitting | waiting | done
    verdict: null,
    error: null,
    busy: false,
    before: null,      // 提交前的 {version, boot_id} 快照
    after: null,       // 重连后最近一次成功轮询结果
    tries: 0,          // 已尝试轮询次数
    restarted: false,
    versionChanged: false,
    _token: 0,         // 复位/取消令牌：使在途循环失效
  })

  function reset() {
    flow._token++
    Object.assign(flow, {
      kind: null, phase: 'idle', verdict: null, error: null, busy: false,
      before: null, after: null, tries: 0, restarted: false, versionChanged: false,
    })
  }

  /** 受理后的有界等待循环：断连静默重试，恢复后按 update.state + 重启证据判定 */
  async function runWaitLoop(token) {
    for (let i = 1; i <= maxTries; i++) {
      if (token !== flow._token) return
      flow.tries = i
      const res = await doPoll()
      if (token !== flow._token) return
      if (res && res.ok) {
        flow.after = res
        const j = judgeAfterRestart({
          before: flow.before,
          after: res,
          requireEvidence: flow.kind === 'install',
        })
        flow.restarted = j.restarted
        flow.versionChanged = j.versionChanged
        if (j.verdict === 'success') { flow.phase = 'done'; flow.verdict = 'success'; return }
        if (j.verdict === 'failed') {
          flow.phase = 'done'; flow.verdict = 'failed'; flow.error = lastErrorOf(res.update); return
        }
        if (j.verdict === 'manual_recovery') {
          flow.phase = 'done'; flow.verdict = 'manual_recovery'; flow.error = lastErrorOf(res.update); return
        }
        // waiting：继续轮询（断连/协调中均为正常路径）
      }
      await wait(pollMs)
    }
    if (token !== flow._token) return
    flow.phase = 'done'
    flow.verdict = 'timeout'
  }

  async function submit(kind, info, actionFn) {
    if (flow.phase === 'submitting' || flow.phase === 'waiting') {
      return { ok: false, code: 'flow_busy', message: '已有进行中的更新流程' }
    }
    const token = ++flow._token
    Object.assign(flow, {
      kind, phase: 'submitting', verdict: null, error: null, busy: false,
      before: snapshotOf(info), after: null, tries: 0, restarted: false, versionChanged: false,
    })
    let rep
    try {
      rep = await actionFn() // 202 {update_id, state, accepted:true}
    } catch (e) {
      if (token !== flow._token) return { ok: false, aborted: true }
      if (e && e.code === 'update_busy') {
        // 只提示等待，不自动重试（重试轰炸只会持续撞 409）
        flow.busy = true
        flow.phase = 'idle'
        return { ok: false, code: 'update_busy', error: e }
      }
      flow.phase = 'idle'
      flow.error = {
        status: e && e.status,
        code: (e && e.code) || 'unknown_error',
        message: (e && e.message) || '操作失败',
        details: (e && e.details) || null,
      }
      return { ok: false, code: flow.error.code, error: flow.error }
    }
    if (token !== flow._token) return { ok: false, aborted: true }
    flow.phase = 'waiting'
    await runWaitLoop(token)
    if (token !== flow._token) return { ok: false, aborted: true }
    return { ok: true, accepted: true, update_id: rep && rep.update_id, state: rep && rep.state }
  }

  /** 确认安装：info 为提交前的 /api/system/info 响应（取 version/boot_id 做重启证据） */
  const submitInstall = (info) => submit('install', info, () => client.installUpdate())

  /** 确认回滚：同样以提交前 info 做快照 */
  const submitRollback = (info) => submit('rollback', info, () => client.rollbackUpdate())

  /** 用户主动停止等待（不影响后台事务；界面回 done/aborted，后续以状态卡片为准） */
  function cancel() {
    if (flow.phase !== 'waiting') return
    flow._token++
    flow.phase = 'done'
    flow.verdict = 'aborted'
  }

  return { flow, submitInstall, submitRollback, cancel, reset }
}
