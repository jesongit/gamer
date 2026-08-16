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
        <button class="btn btn-primary login-btn" type="submit">登 录</button>
      </form>

      <div class="login-hint">默认账号 admin / admin123</div>
    </div>
    <div class="login-foot">GameBot v0.1.0 · 基于 scrcpy + WebRTC 的游戏自动化方案</div>
  </div>
</template>

<script setup>
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { login } from '../store'

const router = useRouter()
const user = ref('admin')
const pass = ref('')

async function onLogin() {
  // 必须 await：login() 是异步的（走 /api/login fetch），
  // 不 await 时 Promise 恒为 truthy，router.push 在 token 写入前执行，
  // 路由守卫（无 token）会把页面弹回登录页 → 登录成功却不跳转
  if (await login(user.value, pass.value)) {
    router.push('/devices')
  } else {
    alert('用户名或密码错误')
  }
}
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
.login-hint { color: var(--text-2); font-size: 11px; margin-top: 14px; }
.login-foot { color: var(--text-2); font-size: 12px; }
</style>
