import { GAMER_NOTIFY_PLUGIN_ID } from '../../gamer-plugin-ids'

export const notificationResults = [
  { key: 'success', label: '成功', enabled: true },
  { key: 'failed', label: '失败', enabled: true },
  { key: 'cancelled', label: '取消', enabled: false },
  { key: 'skipped', label: '跳过', enabled: false },
]
export function taskNotificationPolicy(extensions = {}) {
  const saved = extensions[GAMER_NOTIFY_PLUGIN_ID] || {}
  return {
    ...saved,
    enabled: saved.enabled === true,
    results: Object.fromEntries(notificationResults.map(({ key, enabled }) => {
      const rule = saved.results?.[key] || {}
      return [key, {
        enabled: typeof rule.enabled === 'boolean' ? rule.enabled : enabled,
        channels: Array.isArray(rule.channels) ? rule.channels.filter(id => typeof id === 'string') : [],
        title: typeof rule.title === 'string' ? rule.title : '',
        content: typeof rule.content === 'string' ? rule.content : '',
      }]
    })),
  }
}
export function updateTaskNotificationPolicy(extensions, policy) {
  return { ...extensions, [GAMER_NOTIFY_PLUGIN_ID]: policy }
}
