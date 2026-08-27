import { createRouter, createWebHashHistory } from 'vue-router'
import { probeSession, resolveGuardTarget } from './auth'

const routes = [
  { path: '/login', name: 'Login', component: () => import('./views/Login.vue') },
  {
    path: '/',
    component: () => import('./layouts/MainLayout.vue'),
    children: [
      { path: '', redirect: '/console' },
      { path: 'console', name: 'Console', component: () => import('./views/Console.vue') },
      { path: 'templates', name: 'Templates', component: () => import('./views/TemplateManager.vue') },
      { path: 'scripts', name: 'Scripts', component: () => import('./views/ScriptEditor.vue') },
      { path: 'tasks', name: 'Tasks', component: () => import('./views/TaskScheduler.vue') },
      { path: 'logs', name: 'Logs', component: () => import('./views/RunLogs.vue') },
      { path: 'settings', name: 'Settings', component: () => import('./views/Settings.vue') },
      { path: '/:pathMatch(.*)*', redirect: '/console' }
    ]
  }
]

const router = createRouter({
  history: createWebHashHistory(),
  routes
})

// 会话守卫：以 GET /api/session 的服务端结论为准（结论缓存在 auth.js，
// 登录/登出/401 拦截会刷新它），不再读 localStorage 伪 token。
router.beforeEach(async (to) => {
  const authed = await probeSession()
  return resolveGuardTarget(authed, to.name, to.fullPath)
})

export default router
