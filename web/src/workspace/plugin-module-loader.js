// Trusted host UI is an explicit permission. Sandboxed iframe plugins never use
// this loader. The SDK and module instance are fixed for this page's lifetime:
// updates take effect on refresh, without replacing an unsaved editor in place.
import { installPluginModuleSdk } from './plugin-module-sdk'
import { pluginModules as modules } from './plugin-module-registry'
import { h } from 'vue'

const pending = new Map()
const failures = new Map()
const official = ['gamer-yaml', 'gamer-keymap', 'gamer-video']

export function moduleAssetUrl(id, entry, version) {
  if (!/^ui\/(?:[a-zA-Z0-9_-]+\/)*[a-zA-Z0-9_.-]+\.js$/.test(entry)) throw new Error('插件 UI 模块路径无效')
  return `/api/extensions/${encodeURIComponent(id)}/${entry}?v=${encodeURIComponent(version || '')}`
}

export function authorizedModuleContributions(response) {
  return (response?.ui_contributions || []).filter(item => {
    const extension = response.extensions?.find(value => value.id === item.plugin_id)
    return item.runtime === 'core' && !!item.entry && extension?.state === 'running'
      && extension.permissions?.includes('ui.host') === true
  })
}

async function loadModule(id, url) {
  if (modules.has(id)) return modules.get(id)
  if (!pending.has(id)) pending.set(id, (async () => {
    const value = await import(/* @vite-ignore */ url)
    if (value.sdkVersion !== 1) throw new Error(`插件 ${id} 的 UI SDK 版本不受支持`)
    const css = new URL('style.css', new URL(url, location.origin))
    css.search = new URL(url, location.origin).search
    const link = document.createElement('link')
    link.rel = 'stylesheet'
    link.href = css.href
    document.head.appendChild(link)
    modules.set(id, value)
    return value
  })().catch(error => { pending.delete(id); throw error }))
  return pending.get(id)
}

export async function loadInstalledModules(response) {
  installPluginModuleSdk()
  for (const item of authorizedModuleContributions(response)) {
    if (failures.has(item.plugin_id)) continue
    try { await loadModule(item.plugin_id, moduleAssetUrl(item.plugin_id, item.entry, item.version)) }
    catch (error) {
      failures.set(item.plugin_id, String(error.message || error))
      console.error(`插件 ${item.plugin_id} 界面加载失败`, error)
    }
  }
}

export async function preparePluginModules() {
  const response = await fetch('/api/extensions', { headers: { Accept: 'application/json' } })
  if (!response.ok) throw new Error(`读取插件失败：HTTP ${response.status}`)
  await loadInstalledModules(await response.json())
  // These deployment-owned assets provide console integration when an official
  // plugin is absent/disabled. They are built independently of the shell bundle.
  for (const id of official) if (!modules.has(id)) await loadModule(id, `/plugin-ui/${id}/plugin.js`)
}

export function pluginModule(id) {
  const value = modules.get(id)
  if (!value) throw new Error(`插件 ${id} 的界面尚未加载，请刷新页面`)
  return value
}

export function pluginPanel(id, key) {
  if (failures.has(id)) return { component: () => h('p', { class: 'workspace-empty' }, '插件界面加载失败。请在插件页检查或重新安装，然后刷新页面。') }
  return modules.get(id)?.panels?.[key] || null
}
