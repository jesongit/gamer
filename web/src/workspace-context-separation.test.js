import { describe, expect, it, vi, afterEach } from 'vitest'
import { ref } from 'vue'
import { createWorkspaceContext } from './workspace/context'
import { api } from './api'
import { scriptsData, templatesData } from './store'
import { useFunctionLibrary } from './composables/useFunctionLibrary'
import {
  ensureGamerYamlResources,
  gamerYamlEntrypointOptions,
} from './components/task/gamer-yaml-resources'
import { resolveGamerYamlAppPackages } from './components/task/builtin-runner-editors'

afterEach(() => {
  vi.restoreAllMocks()
  scriptsData.value = []
  templatesData.value = []
})

describe('P3-CONTEXT：Device/App/Package/Plugin/Stage 分离', () => {
  it('WorkspaceContext 显式暴露四个上下文和独立 Stage 快照，不互相推导', async () => {
    const device = ref({ id: 'device-1', name: '测试设备', pkg: 'com.android.game' })
    const androidPackageName = ref('com.android.game')
    const currentPackageId = ref('content.daily')
    const activePluginId = ref('gamer.yaml')
    const connected = ref(true)
    const stage = ref({
      kind: ref('media'),
      sourceId: ref('media-1'),
      generation: ref(3),
      displaySize: ref({ width: 1280, height: 720 }),
      referenceSize: ref({ width: 1280, height: 720 }),
      canDeviceInput: ref(false),
      stageReady: ref(true),
      mediaId: ref('media-1'),
    })
    const context = createWorkspaceContext({
      device,
      deviceId: ref('device-1'),
      androidPackageName,
      currentPackageId,
      activePluginId,
      connected,
      stageContext: stage,
    })

    expect(await context.uiBridge.context.get()).toEqual({
      device: {
        id: 'device-1', name: '测试设备', status: null, kind: null, addr: null,
      },
      deviceId: 'device-1',
      app: {
        package: 'com.android.game',
        packageName: 'com.android.game',
        android_package: 'com.android.game',
      },
      androidPackageName: 'com.android.game',
      package: { id: 'content.daily', content_package: 'content.daily' },
      currentPackageId: 'content.daily',
      plugin: { id: 'gamer.yaml' },
      activePluginId: 'gamer.yaml',
      connected: true,
      stage: {
        kind: 'media',
        sourceId: 'media-1',
        generation: 3,
        displaySize: { width: 1280, height: 720 },
        referenceSize: { width: 1280, height: 720 },
        canDeviceInput: false,
        stageReady: true,
        mediaId: 'media-1',
      },
    })

    androidPackageName.value = 'com.other.game'
    currentPackageId.value = 'content.other'
    expect(await context.uiBridge.context.get()).toMatchObject({
      androidPackageName: 'com.other.game',
      currentPackageId: 'content.other',
      app: { package: 'com.other.game' },
      package: { id: 'content.other' },
    })
  })

  it('gamer.yaml 任务保存分别使用设备 Android 包名和当前 Package ID', () => {
    expect(resolveGamerYamlAppPackages('content.daily/main.yaml', {
      packageId: 'content.daily',
      deviceId: 'device-1',
      androidPackageName: 'com.android.game',
    })).toEqual({
      android_package: 'com.android.game',
      content_package: 'content.daily',
    })
  })

  it('脚本候选只返回 currentPackageId 对应资源', () => {
    scriptsData.value = [
      { id: 'content.daily/main.yaml', package: 'content.daily', name: 'main.yaml' },
      { id: 'content.other/main.yaml', package: 'content.other', name: 'main.yaml' },
    ]
    expect(gamerYamlEntrypointOptions({ packageId: 'content.daily', deviceId: 'device-1' }))
      .toEqual([{ value: 'content.daily/main.yaml', label: 'main.yaml' }])
    expect(gamerYamlEntrypointOptions({ packageId: null, deviceId: 'device-1' })).toEqual([])
  })

  it('函数库 Package 切换时旧响应不能覆盖新候选', async () => {
    const deferred = new Map()
    const api = {
      listFunctions: vi.fn((packageId) => new Promise((resolve) => deferred.set(packageId, resolve))),
    }
    const library = useFunctionLibrary({ api })
    const oldRequest = library.refresh('content.daily')
    const newRequest = library.refresh('content.other')

    deferred.get('content.other')([{ pkg: 'content.other', file: '_function.yaml' }])
    await newRequest
    deferred.get('content.daily')([{ pkg: 'content.daily', file: '_function.yaml' }])
    await oldRequest

    expect(library.list).toEqual([{ pkg: 'content.other', file: '_function.yaml' }])
  })

  it('脚本/模板资源切换时旧 Package 响应不能污染候选 store', async () => {
    const scriptDeferred = new Map()
    const templateDeferred = new Map()
    vi.spyOn(api, 'listScripts').mockImplementation((packageId) =>
      new Promise((resolve) => scriptDeferred.set(packageId, resolve)))
    vi.spyOn(api, 'listTemplates').mockImplementation((packageId) =>
      new Promise((resolve) => templateDeferred.set(packageId, resolve)))

    const oldRequest = ensureGamerYamlResources('content.old')
    const newRequest = ensureGamerYamlResources('content.new')
    scriptDeferred.get('content.new')([{ id: 'content.new/main.yaml', package: 'content.new', name: 'main.yaml' }])
    templateDeferred.get('content.new')([{ pkg: 'content.new', name: 'account.png' }])
    await newRequest
    scriptDeferred.get('content.old')([{ id: 'content.old/main.yaml', package: 'content.old', name: 'old.yaml' }])
    templateDeferred.get('content.old')([{ pkg: 'content.old', name: 'old.png' }])
    await oldRequest

    expect(scriptsData.value.map((script) => script.package)).toEqual(['content.new'])
    expect(templatesData.value.map((template) => template.pkg)).toEqual(['content.new'])
  })
})
