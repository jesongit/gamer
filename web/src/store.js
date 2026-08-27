// 轻量全局状态（鉴权会话在 ./auth.js：Cookie 会话只存内存态，
// 不再有 localStorage 伪 token；authed 判定以 session.username 为准）
import { reactive, ref } from 'vue'

export const store = reactive({
  deviceId: null,           // 当前控制的设备
  running: false,           // 当前设备脚本运行状态
  runScript: null,          // 正在运行的脚本名
  runScriptId: null,        // 正在运行的脚本 id（停止 / 状态轮询用）
  runStep: '',              // 当前步骤描述
  runProgress: 0,           // 0-100
})

// 简易 toast
export function useToast() {
  const wrap = () => document.querySelector('.toast-wrap')
  return (msg, type = 'info') => {
    let w = wrap()
    if (!w) {
      w = document.createElement('div')
      w.className = 'toast-wrap'
      document.body.appendChild(w)
    }
    const el = document.createElement('div')
    el.className = `toast ${type}`
    el.textContent = msg
    w.appendChild(el)
    setTimeout(() => { el.style.opacity = '0'; el.style.transition = 'opacity .3s'; setTimeout(() => el.remove(), 320) }, 2600)
  }
}

// 统一设备数据（由各页面从 API 拉取后写入）
export const devicesData = ref([])
export const scriptsData = ref([])
export const templatesData = ref([])
export const tasksData = ref([])
export const logsData = ref([])
