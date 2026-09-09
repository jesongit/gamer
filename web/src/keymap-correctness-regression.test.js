// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { ref } from 'vue'
import KeymapPanel from './components/console/KeymapPanel.vue'
import { useConsoleKeymap } from './components/console/useConsoleKeymap'

function deferred() {
  let resolve
  let reject
  const promise = new Promise((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function runtimeDeps(apiOverrides = {}, packageId = ref('pkg-a')) {
  return {
    api: {
      listKeymaps: vi.fn(async () => []),
      getKeymap: vi.fn(async () => null),
      updateKeymap: vi.fn(async () => ({ id: 'pkg-a/mapping.yaml' })),
      createKeymap: vi.fn(async () => ({ id: 'pkg-a/mapping.yaml' })),
      ...apiOverrides,
    },
    toast: vi.fn(),
    packageId,
    keyboardMode: ref('game'),
    keymap: { releaseAll: vi.fn() },
    keymapPressed: new Set(),
    videoElement: ref({ videoWidth: 1000, videoHeight: 500 }),
    videoWrap: ref(null),
    deviceRectStyle: (x, y) => ({ left: x + 'px', top: y + 'px' }),
    pickCoord: vi.fn(async () => ({ x: 0.5, y: 0.5 })),
  }
}

function yaml(name, key, x = 0.1, y = 0.2) {
  return [
    'version: 1',
    'name: ' + name,
    'bindings:',
    '  - key: ' + key,
    '    action:',
    '      type: hold',
    '      at: [' + x + ', ' + y + ']',
  ].join('\n')
}

function panelContext(overrides = {}) {
  return {
    pkg: 'pkg-a',
    keymaps: [
      {
        id: 'pkg-a/alpha.yaml',
        name: 'alpha.yaml',
        keymap: {
          version: 1,
          name: 'alpha',
          bindings: [
            { key: 'KeyA', action: { type: 'hold', at: [0.1, 0.2] } },
            { key: 'KeyB', action: { type: 'hold', at: [0.3, 0.4] } },
          ],
        },
      },
      {
        id: 'pkg-a/beta.yaml',
        name: 'beta.yaml',
        keymap: {
          version: 1,
          name: 'beta',
          bindings: [{ key: 'KeyB', action: { type: 'hold', at: [0.3, 0.4] } }],
        },
      },
    ],
    selectedName: 'alpha.yaml',
    usedName: 'alpha.yaml',
    loading: false,
    saving: false,
    error: '',
    ...overrides,
  }
}

describe('Keymap K01-K04 correctness regressions', () => {
  it('renders a hold overlay from action.at', async () => {
    const deps = runtimeDeps({
      getKeymap: vi.fn(async () => ({
        content: yaml('hold-map', 'Space', 0.7, 0.8),
        name: 'hold-map',
        valid: true,
      })),
    })
    const km = useConsoleKeymap(deps)

    await km.loadKeymaps('pkg-a')
    await km.onKeymapChange({ id: 'pkg-a/hold.yaml', name: 'hold.yaml' })

    expect(km.keymapOverlay.value).toEqual([expect.objectContaining({
      type: 'hold',
      label: 'Space',
      style: { left: '700px', top: '400px' },
    })])
  })

  it('ignores an older detail response, including its finally, after a newer selection', async () => {
    const first = deferred()
    const second = deferred()
    const getKeymap = vi.fn((id) => id.includes('alpha') ? first.promise : second.promise)
    const deps = runtimeDeps({
      listKeymaps: vi.fn(async () => [
        { id: 'pkg-a/alpha.yaml', name: 'alpha.yaml' },
        { id: 'pkg-a/beta.yaml', name: 'beta.yaml' },
      ]),
      getKeymap,
    })
    const km = useConsoleKeymap(deps)
    await km.loadKeymaps('pkg-a')

    const alphaRequest = km.onKeymapChange(km.keymaps.value[0])
    const betaRequest = km.onKeymapChange(km.keymaps.value[1])
    expect(km.keymapLoading.value).toBe(true)

    first.resolve({ content: yaml('alpha', 'KeyA'), valid: true })
    await alphaRequest
    expect(km.activeKeymapName.value).toBe('pkg-a/beta.yaml')
    expect(km.activeKeymapModel.value).toBeNull()
    expect(km.keymapLoading.value).toBe(true)

    second.resolve({ content: yaml('beta', 'KeyB'), valid: true })
    await betaRequest
    expect(km.activeKeymapName.value).toBe('pkg-a/beta.yaml')
    expect(km.activeKeymapModel.value.name).toBe('beta')
    expect(km.keymapLoading.value).toBe(false)
    expect(getKeymap).toHaveBeenNthCalledWith(1, 'pkg-a/alpha.yaml', 'pkg-a')
    expect(getKeymap).toHaveBeenNthCalledWith(2, 'pkg-a/beta.yaml', 'pkg-a')
  })

  it('invalidates a pending detail when the Package context changes', async () => {
    const oldDetail = deferred()
    const newDetail = deferred()
    const packageId = ref('pkg-a')
    const getKeymap = vi.fn((id, pkg) => pkg === 'pkg-a' ? oldDetail.promise : newDetail.promise)
    const deps = runtimeDeps({
      listKeymaps: vi.fn(async (pkg) => [{ id: pkg + '/map.yaml', name: 'map.yaml' }]),
      getKeymap,
    }, packageId)
    const km = useConsoleKeymap(deps)
    await km.loadKeymaps('pkg-a')
    const oldRequest = km.onKeymapChange(km.keymaps.value[0])

    packageId.value = 'pkg-b'
    await km.loadKeymaps('pkg-b')
    const newRequest = km.onKeymapChange(km.keymaps.value[0])

    oldDetail.resolve({ content: yaml('old-package', 'KeyA'), valid: true })
    await oldRequest
    expect(km.activeKeymapName.value).toBe('pkg-b/map.yaml')
    expect(km.activeKeymapModel.value).toBeNull()
    expect(km.keymapLoading.value).toBe(true)

    newDetail.resolve({ content: yaml('new-package', 'KeyB'), valid: true })
    await newRequest
    expect(km.activeKeymapModel.value.name).toBe('new-package')
    expect(getKeymap).toHaveBeenNthCalledWith(1, 'pkg-a/map.yaml', 'pkg-a')
    expect(getKeymap).toHaveBeenNthCalledWith(2, 'pkg-b/map.yaml', 'pkg-b')
  })

  it('preserves a valid selection on refresh and saving does not apply the edited model', async () => {
    const currentList = [{ id: 'pkg-a/alpha.yaml', name: 'alpha.yaml' }]
    const getKeymap = vi.fn(async () => ({ content: yaml('alpha-old', 'KeyA'), valid: true }))
    const deps = runtimeDeps({
      listKeymaps: vi.fn(async () => currentList),
      getKeymap,
    })
    const km = useConsoleKeymap(deps)
    await km.loadKeymaps('pkg-a')
    await km.onKeymapChange(km.keymaps.value[0])
    const appliedModel = km.activeKeymapModel.value

    await km.loadKeymaps('pkg-a')
    expect(km.activeKeymapName.value).toBe('pkg-a/alpha.yaml')
    expect(km.activeKeymapModel.value).toBe(appliedModel)

    await km.onKeymapSave({
      pkg: 'pkg-a',
      name: 'alpha.yaml',
      model: { version: 1, name: 'alpha-new', bindings: [] },
      yaml: 'version: 1\nname: alpha-new\nbindings: []\n',
      source: { id: 'pkg-a/alpha.yaml', version: 'old-version' },
      expected_version: 'old-version',
    })
    expect(getKeymap).toHaveBeenCalledTimes(1)
    expect(km.activeKeymapModel.value).toBe(appliedModel)
  })

  it('guards composable saves and exposes an independent saving state', async () => {
    const save = deferred()
    const updateKeymap = vi.fn(() => save.promise)
    const deps = runtimeDeps({ updateKeymap })
    const km = useConsoleKeymap(deps)
    const payload = {
      pkg: 'pkg-a',
      name: 'alpha.yaml',
      model: { version: 1, name: 'alpha', bindings: [] },
      yaml: 'version: 1\nname: alpha\nbindings: []\n',
      source: { id: 'pkg-a/alpha.yaml', version: 'v1' },
    }

    const first = km.onKeymapSave(payload)
    expect(km.keymapPanelContext.saving.value).toBe(true)
    expect(await km.onKeymapSave(payload)).toBe(false)
    expect(updateKeymap).toHaveBeenCalledTimes(1)

    save.resolve({ id: 'pkg-a/alpha.yaml' })
    expect(await first).toBe(true)
    expect(km.keymapPanelContext.saving.value).toBe(false)
  })

  it('uses the saving context to disable panel save and refresh controls', async () => {
    const context = panelContext({
      saving: ref(true),
      onSave: vi.fn(),
      onRefresh: vi.fn(),
    })
    const wrapper = mount(KeymapPanel, { props: { context } })

    await wrapper.get('[data-testid="keymap-scheme-row"]').get('button').trigger('click')
    const saveButton = wrapper.get('.editor-foot button.btn-primary')
    const refreshButton = wrapper.findAll('button').find(button => button.text() === '↻ 刷新')
    expect(saveButton.attributes('disabled')).toBeDefined()
    expect(refreshButton.attributes('disabled')).toBeDefined()
  })

  it('fills a picked point by binding ID after the original row is removed', async () => {
    const point = deferred()
    const onRequestPoint = vi.fn(() => point.promise)
    const wrapper = mount(KeymapPanel, {
      props: { context: panelContext({ onRequestPoint }) },
    })

    await wrapper.get('[data-testid="keymap-scheme-row"]').get('button').trigger('click')
    const bindings = wrapper.findAll('[data-testid="keymap-binding"]')
    await bindings[1].get('button.mini-btn').trigger('click')
    await bindings[0].get('button.icon-btn.danger').trigger('click')

    point.resolve({ x: 0.9, y: 0.8 })
    await new Promise(resolve => setTimeout(resolve, 0))

    const remaining = wrapper.get('[data-testid="keymap-binding"]')
    const inputs = remaining.findAll('input')
    expect(inputs[1].element.value).toBe('0.9')
    expect(inputs[2].element.value).toBe('0.8')
    expect(onRequestPoint).toHaveBeenCalledWith({ pkg: 'pkg-a', index: 1, field: 'at' })
  })

  it('does not close a newer draft when an earlier save resolves', async () => {
    const save = deferred()
    const onSave = vi.fn(() => save.promise)
    const wrapper = mount(KeymapPanel, {
      props: { context: panelContext({ onSave }) },
    })

    const rows = wrapper.findAll('[data-testid="keymap-scheme-row"]')
    await rows[0].get('button').trigger('click')
    await wrapper.get('[data-testid="keymap-name"]').setValue('alpha-edited')
    await wrapper.get('button.btn-primary').trigger('click')

    const refreshedRows = wrapper.findAll('[data-testid="keymap-scheme-row"]')
    await refreshedRows[1].get('button').trigger('click')
    expect(wrapper.get('[data-testid="keymap-name"]').element.value).toBe('beta')

    save.resolve(true)
    await new Promise(resolve => setTimeout(resolve, 0))
    expect(wrapper.get('[data-testid="keymap-editor"]').exists()).toBe(true)
    expect(wrapper.get('[data-testid="keymap-name"]').element.value).toBe('beta')
  })
})
