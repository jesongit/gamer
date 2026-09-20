import { describe, expect, it } from 'vitest'
import { authorizedModuleContributions, moduleAssetUrl, pluginPanel } from './workspace/plugin-module-loader'
import { pluginModules } from './workspace/plugin-module-registry'

describe('installed host UI trust and ownership', () => {
  const contribution = { plugin_id: 'gamer-example', runtime: 'core', entry: 'ui/plugin.js', component: 'sample' }
  it('requires explicit permission and running lifecycle; never imports iframe entries', () => {
    const response = { ui_contributions: [contribution], extensions: [{ id: 'gamer-example', state: 'running', permissions: [] }] }
    expect(authorizedModuleContributions(response)).toEqual([])
    response.extensions[0].permissions.push('ui.host')
    expect(authorizedModuleContributions(response)).toEqual([contribution])
    response.extensions[0].state = 'disabled'
    expect(authorizedModuleContributions(response)).toEqual([])
    response.extensions[0].state = 'running'
    response.ui_contributions = [{ ...contribution, runtime: 'iframe' }]
    expect(authorizedModuleContributions(response)).toEqual([])
  })
  it('only produces same-origin scoped asset URLs, rejects traversal and remote entries', () => {
    expect(moduleAssetUrl('gamer-example', 'ui/plugin.js', '1.2.3')).toBe('/api/extensions/gamer-example/ui/plugin.js?v=1.2.3')
    for (const entry of ['https://other/plugin.js', 'ui/../plugin.js', '/ui/plugin.js', 'ui/%2e%2e/plugin.js', 'ui/page.html']) {
      expect(() => moduleAssetUrl('gamer-example', entry, '1')).toThrow()
    }
  })
  it('one plugin cannot resolve another plugin’s component key', () => {
    const descriptor = { component: {} }
    pluginModules.set('gamer-owner', { panels: { sample: descriptor } })
    try {
      expect(pluginPanel('gamer-owner', 'sample')).toBe(descriptor)
      expect(pluginPanel('gamer-other', 'sample')).toBeNull()
    } finally { pluginModules.delete('gamer-owner') }
  })
})
