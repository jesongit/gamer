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
      // 模板管理页已删除，模板并入 脚本管理（/scripts）的模板页签：旧地址重定向
      { path: 'templates', redirect: '/scripts' },
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

// 构建产物更新（gamer.ps1 restart -Build / 重新 npm run build）后，已打开的旧页面再点
// 导航时懒加载旧 hash 的 chunk 会 404 → 导航静默失败，表现为「侧边栏点其他页面没反应」。
// 命中 chunk 加载错误 → 整页刷新一次加载新产物（sessionStorage 标记防刷新循环，
// afterEach 清标记，之后再次部署仍能触发）；其余错误只打日志不劫持。
router.onError((err) => {
  const msg = String((err && err.message) || err)
  if (/dynamically imported module|Importing a module script failed|Loading chunk \d+ failed/i.test(msg)) {
    try {
      if (!sessionStorage.getItem('gb_chunk_reload')) {
        sessionStorage.setItem('gb_chunk_reload', '1')
        window.location.reload()
        return
      }
    } catch { /* 无 sessionStorage 环境（隐私模式等）直接刷新 */ }
    window.location.reload()
    return
  }
  console.error('[router]', err)
})
router.afterEach(() => {
  try { sessionStorage.removeItem('gb_chunk_reload') } catch { /* 忽略 */ }
})

export default router
