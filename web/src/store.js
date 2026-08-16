// 轻量全局状态
import { reactive, ref } from 'vue'

export const store = reactive({
  authed: localStorage.getItem('gb_token') === 'demo',
  deviceId: null,           // 当前控制的设备
  running: false,           // 当前设备脚本运行状态
  runScript: null,          // 正在运行的脚本名
  runStep: '',              // 当前步骤描述
  runProgress: 0,           // 0-100
})

export function login(user, pass) {
  return new Promise((resolve, reject) => {
    // 通过 API 登录
    fetch('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ user, password: pass })
    }).then(r => {
      if (r.ok) {
        localStorage.setItem('gb_token', 'demo')
        store.authed = true
        resolve(true)
      } else {
        resolve(false)
      }
    }).catch(() => resolve(false))
  })
}

export function logout() {
  localStorage.removeItem('gb_token')
  store.authed = false
}

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
