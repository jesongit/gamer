<template>
  <div class="layout">
    <!-- 侧边栏 -->
    <aside class="sidebar">
      <div class="logo">
        <span class="logo-icon">🎮</span>
        <div>
          <div class="logo-name">GameBot</div>
          <div class="logo-sub">游戏自动化助手</div>
        </div>
      </div>

      <nav class="nav">
        <router-link v-for="item in navs" :key="item.path" :to="item.path" class="nav-item" :class="{ active: $route.path === item.path }">
          <span class="nav-icon">{{ item.icon }}</span>
          <span>{{ item.name }}</span>
          <span v-if="item.path === '/logs'" class="nav-badge">3</span>
        </router-link>
      </nav>

      <div class="sidebar-foot">
        <div class="sys-state">
          <span class="dot ok"></span>
          <span>服务运行中</span>
          <span class="sys-ver">v0.1.0</span>
        </div>
        <div class="sys-state">
          <span class="dot ok"></span>
          <span>2 台设备在线</span>
          <span class="sys-ver">任务 2</span>
        </div>
      </div>
    </aside>

    <!-- 主区域 -->
    <div class="main">
      <header class="topbar">
        <div class="tb-left">
          <router-link to="/devices" class="tb-device" :class="{ on: store.deviceId }">
            <span class="dot" :class="store.deviceId ? 'ok' : 'off'"></span>
            {{ store.deviceId ? currentDeviceName : '未选择设备' }}
          </router-link>
        </div>
        <div class="tb-right">
          <div v-if="store.running" class="run-chip">
            <span class="dot run"></span>
            <span>{{ store.runScript }}</span>
            <span class="run-step">{{ store.runStep }}</span>
          </div>
          <router-link to="/console" class="btn btn-sm" :class="{ 'btn-primary': !store.running }">进入控制台</router-link>
          <button class="btn btn-sm btn-ghost" @click="onLogout">退出</button>
        </div>
      </header>

      <main class="content">
        <router-view />
      </main>
    </div>
  </div>
</template>

<script setup>
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { store, logout, devicesData } from '../store'

const router = useRouter()
const navs = [
  { path: '/devices', name: '设备列表', icon: '📱' },
  { path: '/console', name: '投屏控制', icon: '🖥️' },
  { path: '/templates', name: '模板管理', icon: '🖼️' },
  { path: '/scripts', name: '脚本编辑', icon: '📜' },
  { path: '/tasks', name: '定时任务', icon: '⏰' },
  { path: '/logs', name: '运行日志', icon: '📋' },
  { path: '/settings', name: '设置', icon: '⚙️' }
]

const currentDeviceName = computed(() => {
  const d = devicesData.value.find(x => x.id === store.deviceId)
  return d ? d.name : ''
})

function onLogout() {
  logout()
  router.push('/login')
}
</script>

<style scoped>
.layout { display: flex; height: 100%; }

.sidebar {
  width: 200px; flex-shrink: 0; background: var(--bg-1);
  border-right: 1px solid var(--border);
  display: flex; flex-direction: column;
}
.logo { display: flex; align-items: center; gap: 10px; padding: 18px 16px 14px; }
.logo-icon { font-size: 26px; }
.logo-name { font-size: 17px; font-weight: 800; letter-spacing: .5px; }
.logo-sub { font-size: 11px; color: var(--text-2); margin-top: 1px; }

.nav { flex: 1; padding: 8px; display: flex; flex-direction: column; gap: 2px; overflow: auto; }
.nav-item {
  display: flex; align-items: center; gap: 10px; padding: 9px 12px;
  border-radius: var(--radius-sm); color: var(--text-1);
  text-decoration: none; font-size: 13px; transition: all .15s; position: relative;
}
.nav-item:hover { background: var(--bg-3); color: var(--text-0); }
.nav-item.active { background: rgba(34,211,165,.1); color: var(--accent); font-weight: 600; }
.nav-item.active::before {
  content: ''; position: absolute; left: 0; top: 20%; bottom: 20%; width: 3px;
  border-radius: 2px; background: var(--accent);
}
.nav-icon { font-size: 15px; width: 20px; text-align: center; }
.nav-badge {
  margin-left: auto; background: var(--accent); color: #06251c;
  font-size: 10px; font-weight: 700; border-radius: 10px; padding: 1px 6px;
}

.sidebar-foot { padding: 12px 16px; border-top: 1px solid var(--border); display: flex; flex-direction: column; gap: 8px; }
.sys-state { display: flex; align-items: center; gap: 7px; font-size: 12px; color: var(--text-1); }
.sys-ver { margin-left: auto; color: var(--text-2); font-size: 11px; }

.main { flex: 1; display: flex; flex-direction: column; min-width: 0; background: var(--bg-0); }

.topbar {
  height: 52px; flex-shrink: 0; background: var(--bg-1);
  border-bottom: 1px solid var(--border);
  display: flex; align-items: center; justify-content: space-between; padding: 0 16px;
}
.tb-device { display: flex; align-items: center; gap: 8px; color: var(--text-1); text-decoration: none; font-size: 13px; }
.tb-device.on { color: var(--text-0); font-weight: 600; }
.tb-right { display: flex; align-items: center; gap: 10px; }

.run-chip {
  display: flex; align-items: center; gap: 8px; padding: 5px 12px;
  border-radius: 20px; font-size: 12px; color: var(--accent);
  background: rgba(34,211,165,.08); border: 1px solid rgba(34,211,165,.3);
}
.run-step { color: var(--text-1); }

.content { flex: 1; overflow: hidden; }
</style>
