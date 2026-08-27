<template>
  <div class="login-wrap">
    <div class="login-card">
      <div class="login-logo">🎮</div>
      <h1>GameBot</h1>
      <p class="login-sub">游戏自动化助手 · Web 控制台</p>

      <form class="login-form" @submit.prevent="onLogin">
        <div class="form-item">
          <label>用户名</label>
          <input v-model="user" class="input" placeholder="admin" autocomplete="username" />
        </div>
        <div class="form-item">
          <label>密码</label>
          <input v-model="pass" class="input" type="password" placeholder="••••••••" autocomplete="current-password" />
        </div>
        <button class="btn btn-primary login-btn" type="submit" :disabled="busy || countdown > 0">
          {{ busy ? '登录中…' : (countdown > 0 ? `请稍候（${countdown}s）` : '登 录') }}
        </button>
        <div v-if="errMsg" class="login-err">{{ errMsg }}</div>
      </form>

      <div class="login-hint">默认账号 admin / admin123</div>
    </div>
    <div class="login-foot">GameBot v0.1.0 · 基于 scrcpy + WebRTC 的游戏自动化方案</div>
  </div>
</template>

<script setup>
import { ref, onBeforeUnmount } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import { login, sanitizeRedirect, formatRetryCountdown } from '../auth'

const router = useRouter()
const route = useRoute()
const user = ref('admin')
const pass = ref('')
const busy = ref(false)
const errMsg = ref('')
const countdown = ref(0)
let timer = null

// 登录失败 → 倒计时期间禁用提交按钮（429 too_many_attempts 携带 retry_after 秒数）
function startCountdown(seconds) {
  stopCountdown()
  countdown.value = seconds
  timer = setInterval(() => {
    if (--countdown.value <= 0) {
      stopCountdown()
      errMsg.value = ''
    }
  }, 1000)
}
function stopCountdown() {
  if (timer) { clearInterval(timer); timer = null }
  countdown.value = 0
}

async function onLogin() {
  if (busy.value || countdown.value > 0) return
  busy.value = true
  errMsg.value = ''
  const res = await login(user.value.trim(), pass.value)   // 必须 await：凭 Set-Cookie 回包后才能放行路由
  busy.value = false
  if (res.ok) {
    // 服务端会话已建立（Cookie 同源自动携带）；回跳到被 401/守卫拦下的原目标
    const target = sanitizeRedirect(route.query.redirect)
    if (route.query.redirect) router.replace(target)       // replace：避免回退键再次经过未认证页
    else router.push('/console')
    return
  }
  switch (res.code) {
    case 'invalid_credentials':
      errMsg.value = '账号或密码错误'
      break
    case 'too_many_attempts':
      errMsg.value = `尝试过于频繁，请 ${formatRetryCountdown(res.retryAfter)} 后重试`
      startCountdown(res.retryAfter)
      break
    case 'network_error':
      errMsg.value = '无法连接服务端，请确认后端已启动'
      break
    default:
      errMsg.value = '登录失败，请稍后再试'
  }
}

onBeforeUnmount(stopCountdown)
</script>

<style scoped>
.login-wrap {
  height: 100%; display: flex; flex-direction: column;
  align-items: center; justify-content: center; gap: 24px;
  background: radial-gradient(1200px 600px at 50% -10%, #16233b 0%, var(--bg-0) 60%);
}
.login-card {
  width: 360px; background: var(--bg-1); border: 1px solid var(--border);
  border-radius: 16px; padding: 36px 32px; box-shadow: var(--shadow);
  display: flex; flex-direction: column; align-items: center; gap: 6px;
}
.login-logo { font-size: 48px; }
.login-card h1 { font-size: 24px; letter-spacing: 2px; }
.login-sub { color: var(--text-2); font-size: 12px; margin-bottom: 18px; }
.login-form { width: 100%; display: flex; flex-direction: column; gap: 14px; }
.login-btn { justify-content: center; padding: 10px; font-size: 14px; margin-top: 6px; }
.login-btn:disabled { opacity: .55; cursor: not-allowed; }
.login-err { color: #ff6b6b; font-size: 12px; text-align: center; margin-top: 2px; }
.login-hint { color: var(--text-2); font-size: 11px; margin-top: 14px; }
.login-foot { color: var(--text-2); font-size: 12px; }
</style>
