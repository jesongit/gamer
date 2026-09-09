// RunnerEditorContribution 注册表单测（P11.1 §6.7 轻量 V1 契约）：
// - 按 runner_id 注册/获取/枚举/反注册；同 id 重复注册以最后一次为准；
// - gamer.yaml 内置贡献形状：title/entrypoints（异步候选，保障 store 就绪）/
//   entrypointEditor（ScriptPicker + 纯受控 autoPick:false）/payloadEditor/resolveAppPackages。
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  registerRunnerEditor, unregisterRunnerEditor, getRunnerEditor,
  listRunnerEditors, resetRunnerEditorsForTests,
} from './components/task/runner-editors'
import { registerGamerYamlRunnerEditor, GAMER_YAML_RUNNER_ID } from './components/task/builtin-runner-editors'
import { scriptsData, templatesData } from './store'

function stubFetch(routes) {
  vi.stubGlobal('fetch', vi.fn(async (url) => {
    const hit = routes.find((r) => String(url) === r.url || String(url).split('?')[0] === r.url)
    if (!hit) throw new Error(`unexpected fetch: ${url}`)
    return {
      ok: true, status: 200,
      headers: { get: (k) => (String(k).toLowerCase() === 'content-type' ? 'application/json' : null) },
      json: async () => (typeof hit.body === 'function' ? hit.body() : hit.body),
      blob: async () => new Blob(),
    }
  }))
}

afterEach(() => {
  vi.unstubAllGlobals()
  resetRunnerEditorsForTests()
})

describe('runner-editors 注册表', () => {
  it('注册后可按 runner_id 获取；未注册返回 undefined', () => {
    const contrib = { runnerId: 'thirdparty.x', title: 'X', payloadEditor: {} }
    const unregister = registerRunnerEditor(contrib)
    expect(getRunnerEditor('thirdparty.x')).toBe(contrib)
    expect(getRunnerEditor('nope')).toBeUndefined()
    unregister()
    expect(getRunnerEditor('thirdparty.x')).toBeUndefined()
  })

  it('同 runnerId 重复注册以最后一次为准；listRunnerEditors 按 id 稳定排序', () => {
    registerRunnerEditor({ runnerId: 'b.x', title: 'A', payloadEditor: {} })
    const b = { runnerId: 'b.x', title: 'B', payloadEditor: {} }
    registerRunnerEditor(b)
    expect(getRunnerEditor('b.x')).toBe(b)
    registerRunnerEditor({ runnerId: 'a.x', title: 'C', payloadEditor: {} })
    expect(listRunnerEditors().map((c) => c.runnerId)).toEqual(['a.x', 'b.x'])
  })

  it('unregisterRunnerEditor 删除指定 id；reset 清空全部（仅测试用）', () => {
    registerRunnerEditor({ runnerId: 'x1', title: '', payloadEditor: {} })
    registerRunnerEditor({ runnerId: 'x2', title: '', payloadEditor: {} })
    unregisterRunnerEditor('x1')
    expect(getRunnerEditor('x1')).toBeUndefined()
    resetRunnerEditorsForTests()
    expect(listRunnerEditors()).toEqual([])
  })
})

describe('gamer.yaml 内置贡献', () => {
  it('形状契约：title/entrypoints/entrypointEditor/payloadEditor/resolveAppPackages 齐备', () => {
    const unregister = registerGamerYamlRunnerEditor()
    const contrib = getRunnerEditor(GAMER_YAML_RUNNER_ID)
    expect(contrib).toBeTruthy()
    expect(contrib.title).toBe('YAML 脚本')
    expect(typeof contrib.entrypoints).toBe('function')
    expect(contrib.entrypointEditor).toBeTruthy()
    expect(contrib.payloadEditor).toBeTruthy()
    expect(typeof contrib.resolveAppPackages).toBe('function')
    unregister()
    expect(getRunnerEditor(GAMER_YAML_RUNNER_ID)).toBeUndefined()
  })

  it('resolveAppPackages 不从 entrypoint 推导；显式 ctx 分离 Android 包名与 Package ID', () => {
    const unregister = registerGamerYamlRunnerEditor()
    const contrib = getRunnerEditor(GAMER_YAML_RUNNER_ID)
    // entrypoint 只是 Package 内资源寻址；缺少显式 ctx 时不能从它的首段推导任何包名。
    expect(contrib.resolveAppPackages('com.demo/main.yml', {}))
      .toEqual({ android_package: '', content_package: null })

    // Android 运行目标来自设备 Context，内容 Package 来自 Package Context，二者严格分离。
    expect(contrib.resolveAppPackages('com.demo/main.yml', {}, {
      packageId: 'content.daily',
      deviceId: 'device-1',
      androidPackageName: 'com.android.game',
    }))
      .toEqual({ android_package: 'com.android.game', content_package: 'content.daily' })
    unregister()
  })

  it('entrypointEditorProps：ctx.packageId 锁定当前 Package；autoPick=false 纯受控', () => {
    const unregister = registerGamerYamlRunnerEditor()
    const contrib = getRunnerEditor(GAMER_YAML_RUNNER_ID)
    expect(contrib.entrypointEditorProps({ packageId: 'com.demo', deviceId: 'd1' }))
      .toEqual({ package: 'com.demo', lockPackage: true, autoPick: false })
    expect(contrib.entrypointEditorProps({ packageId: null, deviceId: '' }))
      .toEqual({ package: '', lockPackage: false, autoPick: false })
    unregister()
  })

  it('entrypoints(ctx)：拉取脚本进 store 后返回候选（value=脚本 id）', async () => {
    stubFetch([
      { url: '/api/packages/com.demo/plugins/gamer.yaml/resources?prefix=automations', body: { resources: [{ package: 'com.demo', path: 'automations/main.yml', content: 'steps: []', version: 'v1', updated_at: '', size: 1 }] } },
      { url: '/api/packages/com.demo/plugins/gamer.yaml/resources?prefix=templates', body: { resources: [] } },
    ])
    const unregister = registerGamerYamlRunnerEditor()
    const contrib = getRunnerEditor(GAMER_YAML_RUNNER_ID)
    const opts = await contrib.entrypoints({ packageId: 'com.demo', deviceId: '' })
    expect(opts).toEqual([{ value: 'com.demo/main.yml', label: 'main.yml' }])
    expect(scriptsData.value.map((s) => s.id)).toEqual(['com.demo/main.yml'])
    expect(templatesData.value).toEqual([])
    unregister()
  })
})
