import { createRouter, createWebHashHistory } from 'vue-router'

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

router.beforeEach((to) => {
  const authed = localStorage.getItem('gb_token')
  if (to.name !== 'Login' && !authed) return { name: 'Login' }
  if (to.name === 'Login' && authed) return { name: 'Console' }
})

export default router
