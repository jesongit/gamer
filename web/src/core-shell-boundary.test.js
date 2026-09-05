import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * Core 壳知识边界（ADR-11 / P11.5 §10.6）——源码级断言，锁死回归：
 *
 * Core 壳（Console 视图 + workspace 注册层 + Core 通用模块）不认识任何具体
 * 扩展（gamer.yaml / gamer.keymap）、不引用业务编辑组件（ScriptPicker /
 * ParamsForm）、不在壳内预取业务资源（listScripts / listTemplates）、不出现
 * script_id 执行句柄字面量。业务面板实现归扩展前端侧
 * （components/console/ 的面板 composable、components/task/ 的任务贡献），
 * 其中的业务知识不在本断言范围。
 *
 * 断言手法与 task-board.test.js 的 TaskBoard 边界一致（readFileSync + 包含判断）。
 */

const root = new URL('./', import.meta.url)
const read = (path) => readFileSync(new URL(path, root), 'utf8')

// 扩展注册 id：壳与注册层一律不得出现
const EXTENSION_IDS = ['gamer.yaml', 'gamer.keymap']
// 业务编辑组件：只有扩展面板实现/贡献文件可引用
const BUSINESS_COMPONENTS = ['ScriptPicker', 'ParamsForm']
// 业务资源预取与执行句柄：壳只认识设备/日志等 Core 资源
const BUSINESS_FETCH = ['listScripts', 'listTemplates']

describe('Console 壳视图（views/Console.vue）', () => {
  const source = read('./views/Console.vue')
  for (const banned of [...EXTENSION_IDS, ...BUSINESS_COMPONENTS, ...BUSINESS_FETCH, 'script_id']) {
    it(`不出现 ${banned}`, () => {
      expect(source).not.toContain(banned)
    })
  }
})

describe('workspace 注册层（PanelRegistry 驱动，壳不解释面板实现）', () => {
  // core-component-registry.ts 是「组件键 → 宿主组件」的唯一解析表（组件名
  // 例外），但仍不得出现扩展 id 与业务编辑组件。
  const files = [
    './workspace/registry.ts',
    './workspace/core-contributions.ts',
    './workspace/context.ts',
    './workspace/lifecycle.ts',
    './workspace/contribution-manager.ts',
    './workspace/core-component-registry.ts',
    './workspace/PluginWorkspace.vue',
    './workspace/CorePanelHost.vue',
    './workspace/WorkspaceTabs.vue',
    './workspace/WorkspaceContextBar.vue',
  ]
  for (const file of files) {
    const source = read(file)
    for (const banned of [...EXTENSION_IDS, ...BUSINESS_COMPONENTS]) {
      it(`${file} 不出现 ${banned}`, () => {
        expect(source).not.toContain(banned)
      })
    }
  }
})

describe('Core 通用模块', () => {
  it('api.js 不含 runner 注册 id（运行入口只提供 runner 无关的 api.run）', () => {
    const source = read('./api.js')
    for (const banned of EXTENSION_IDS) {
      expect(source).not.toContain(banned)
    }
    expect(source).not.toContain('runScript')
    expect(source).toContain('run: async ({ runner_id, entrypoint, device_id, payload }')
  })

  it('useConsoleRuntime 不预取脚本/模板（业务资源由面板实现自加载）', () => {
    const source = read('./composables/useConsoleRuntime.js')
    for (const banned of [...EXTENSION_IDS, ...BUSINESS_FETCH, 'script_id']) {
      expect(source).not.toContain(banned)
    }
  })

  it('LogsPanel（gamer.core:logs）不预取脚本/模板、不消费 scriptsData', () => {
    const source = read('./components/LogsPanel.vue')
    for (const banned of [...EXTENSION_IDS, ...BUSINESS_FETCH, 'scriptsData']) {
      expect(source).not.toContain(banned)
    }
  })

  it('store.js/runs.js 只讲 RunRecord/Runner 语义', () => {
    const store = read('./store.js')
    expect(store).not.toContain('script_name')
    const runs = read('./runs.js')
    // 409 冲突展示优先 entrypoint；script_id 仅作为服务端兼容字段回退
    expect(runs).toContain('d.entrypoint || d.script_id')
  })

  it('useConsoleWorkspacePanels 不做本地面板回退注册', () => {
    const source = read('./components/console/useConsoleWorkspacePanels.js')
    expect(source).not.toContain('register')
    expect(source).not.toContain('gamer.yaml')
  })
})

describe('yaml 扩展前端侧契约点（归属正确性）', () => {
  it('runner 注册 id 唯一配置点在 gamer-yaml-runner.js；api.runScript/runFunction 包装已迁出', () => {
    expect(read('./gamer-yaml-runner.js')).toContain("export const GAMER_YAML_RUNNER_ID = 'gamer.yaml'")
    expect(() => read('./workspace/yaml-extension.ts')).toThrow()
  })

  it('面板作用域上下文成对装配（脚本/函数互不串台的壳侧证据）', () => {
    expect(read('./views/Console.vue')).toContain('scriptRunner: { scripts: scriptPanel, functions: functionsPanel }')
  })
})
