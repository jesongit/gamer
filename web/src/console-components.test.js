import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'

const root = new URL('./', import.meta.url)
const read = (path) => readFileSync(new URL(path, root), 'utf8')

describe('Console 视觉组件拆分静态回归', () => {
  const consoleSource = read('./views/Console.vue')
  const template = consoleSource.slice(0, consoleSource.indexOf('</template>'))
  // phase-05 Console 物理拆分后，实现细节分布在 components/console/ 的拆分模块中；
  // 静态回归统一对「Console 壳 + 拆分模块」整体断言，契约不变。
  const consoleModules = [
    './views/Console.vue',
    './components/console/useConsoleDeviceManager.js',
    './components/console/useConsoleTemplates.js',
    './components/console/useConsoleBridgeOverlays.js',
    './components/console/useConsoleScriptRunner.js',
    './components/console/useConsoleKeymap.js',
    './components/console/useWebrtcStats.js',
    './components/console/useConsolePanelResize.js',
    './components/console/useConsoleWorkspacePanels.js',
  ]
  const consoleImpl = consoleModules.map(read).join('\n')

  it('Console 只编排视觉子组件，设备管理收进工具条 + 设置弹窗', () => {
    expect(consoleSource).toContain("import DeviceSettingsModal from '../components/console/DeviceSettingsModal.vue'")
    // P11.5：业务面板组件不再由 Console 壳直接引入——解析收敛到
    // workspace/core-component-registry（runtime=core + component 键）
    expect(consoleSource).not.toContain("import TemplateCapture from")
    expect(consoleSource).not.toContain("import ScriptRunner from")
    expect(consoleSource).not.toContain("import KeymapPanel from")
    expect(consoleSource).not.toContain("import LogsPanel from")
    expect(consoleSource).not.toContain("import TaskBoard from")
    expect(consoleSource).not.toContain("import SystemPanel from")
    expect(template).toContain('<DeviceSettingsModal ')
    expect(template).not.toContain('<TemplateCapture ')
    expect(template).not.toContain('<ScriptRunner ')
    expect(template).not.toContain('<KeymapPanel ')
    expect(template).not.toContain('<LogsPanel ')
    expect(template).not.toContain('<TaskBoard ')
    expect(template).not.toContain('<SystemPanel ')
    expect(template).not.toContain('<DevicePanel ')
    expect(template).not.toContain('panel-tabs')
    expect(template).not.toContain('class="dev-pick"')
    expect(template).not.toContain('class="script-tpl"')
    expect(template).not.toContain('class="script-run"')
    expect(consoleSource).not.toContain('录制')
    expect(consoleSource).not.toContain('recording')
  })

  it('设备选择/连接/设置/删除与投屏控制合并在同一工具条行', () => {
    const toolbar = template.slice(template.indexOf('class="toolbar"'), template.indexOf('ConsoleVideoStage'))
    expect(toolbar).toContain('v-model="store.deviceId"')
    expect(toolbar).toContain('flushAndConnect')
    expect(toolbar).toContain('refreshDevices')
    expect(toolbar).toContain('startAdd')
    expect(toolbar).toContain('openSettings')
    expect(toolbar).toContain('removeDevice')
    expect(toolbar).toContain('⚙️ 设置')
    expect(toolbar).toContain('更多 ▾')
    expect(toolbar).toContain('toolbarMoreOpen')
    expect(toolbar).toContain('🔄 旋转')
    expect(toolbar).toContain("key('APP_SWITCH')")
    expect((toolbar.match(/class="tb-row/g) || [])).toHaveLength(1)

    // 删除与截图之间有分割线；启动应用与粘贴之间不再有分割线。
    const sep = toolbar.indexOf('class="tb-sep"')
    expect(sep).toBeGreaterThan(toolbar.indexOf('removeDevice'))
    expect(sep).toBeLessThan(toolbar.indexOf('>📷 截图</button>'))
    expect(toolbar.lastIndexOf('class="tb-sep"')).toBe(sep)
  })

  it('§27 Android 应用控制收在左侧设备工具条：启动读设备包名，停止置灰待后端 stop_app，徽章显示当前应用', () => {
    // 启动应用：设备配置的应用包名（launchGame），与 Package 数据上下文无关
    expect(consoleImpl).toContain("sendControl({ type: 'start_app', app: androidPkg })")
    // 停止应用：服务端控制消息词表（DataChannel webrtc/mod.rs + REST /control
    // parse_ctl）均无 stop_app——能力仅扩展层可用，前端无可达路径，置灰 + title 说明
    const toolbar = template.slice(template.indexOf('class="toolbar"'), template.indexOf('ConsoleVideoStage'))
    const launchIdx = toolbar.indexOf('🚀 启动应用')
    const stopIdx = toolbar.indexOf('⏹ 停止应用')
    const badgeIdx = toolbar.indexOf('📱 {{ currentAndroidAppLabel }}')
    expect(launchIdx).toBeGreaterThan(-1)
    expect(stopIdx).toBeGreaterThan(launchIdx)
    expect(badgeIdx).toBeGreaterThan(stopIdx)
    expect(toolbar).toContain('stop_app')
    // 停止按钮置灰：开标签内含 disabled + 解释性 title
    const stopBtnOpen = toolbar.slice(toolbar.lastIndexOf('<button', stopIdx), stopIdx)
    expect(stopBtnOpen).toContain('disabled')
    expect(stopBtnOpen).toContain('title=')
    // 当前应用徽章 = 设备配置 pkg + 已读应用列表软件名（appLabelByPkg）
    expect(consoleImpl).toContain('const currentAndroidAppLabel = computed(')
    expect(consoleImpl).toContain('appLabelByPkg.value.get(pkg)')
    // 连接成功后台静默预取应用列表（徽章软件名 + 设置弹包包名候选），失败不打扰投屏
    expect(consoleSource).toContain('loadApps({ silent: true })')
    expect(consoleImpl).toContain('async function loadApps({ silent = false } = {})')
  })

  it('子组件保留关键交互入口和挂载回调契约', () => {
    const settings = read('./components/console/DeviceSettingsModal.vue')
    const virtualFields = read('./components/console/DeviceVirtualFields.vue')
    const capture = read('./components/console/TemplateCapture.vue')
    const runner = read('./components/console/ScriptRunner.vue')
    const logs = read('./components/console/RunLogPanel.vue')

    expect(settings).toContain('DeviceVirtualFields')
    expect(settings).toContain('ctx.saveSettings')
    expect(settings).toContain('ctx.cancelSettings')
    expect(settings).toContain('ConsoleDeviceSummary')
    expect(virtualFields).not.toContain('读取应用')
    expect(virtualFields).not.toContain('ctx.form.pkg')
    // Package 下拉挂在 PackageContextBar（右侧上下文条，plan §28）；Android 启动
    // 目标取设备配置 pkg（§27），与 Package 数据上下文分命名空间
    const contextBar = read('./workspace/PackageContextBar.vue')
    expect(contextBar).toContain(":value=\"ctx.currentId || ''\"")
    expect(contextBar).toContain('@change="ctx.onPackageChange"')
    expect(consoleImpl).toContain("(current?.pkg || '设备未配置应用包名')")
    expect(consoleSource).not.toContain("sendControl({ type: 'start_app', app: activePkg")
    // 二次裁切弹窗独立成 TemplateCropModal：挂在面板层级（任何页签下框选可见，不切页签）
    const cropModal = read('./components/console/TemplateCropModal.vue')
    expect(cropModal).toContain('props.onCropMounted({ canvas: cropCanvas.value, section: cropSec.value })')
    expect(cropModal).toContain('ctx.cropMouseDown')
    expect(cropModal).toContain('ctx.crop.preserveColor')
    expect(cropModal).toContain('保留颜色')
    expect(cropModal).toContain('ctx.crop.conflict')
    expect(cropModal).toContain('ctx.overwriteTemplate')
    expect(cropModal).toContain('ctx.backToCrop')
    expect(cropModal).toContain('当前裁切模板')
    expect(cropModal).toContain('模板库中的')
    expect(consoleImpl).toContain('crop.preserveColor')
    expect(consoleImpl).toContain('function findCropConflict(')
    expect(consoleImpl).toContain('function overwriteTemplate(')
    expect(template).toContain('<TemplateCropModal ')
    expect(template).toContain(':on-crop-mounted="onCropMounted"')
    expect(capture).not.toContain('ctx.cropMouseDown')
    expect(capture).toContain('ctx.onTplUpload')
    // 阶段 4：结构化编辑区换壳；原文编辑区单独保留 textarea，不属于结构化编辑器
    expect(runner).not.toContain('setAnchorProvider')
    expect(runner).not.toContain('onEditorMounted')
    expect(runner).toContain('ctx.raw.content')
    expect(runner).toContain('ctx.saveRawScript')
    expect(runner).toContain('原文编辑')
    expect(runner).toContain('<StepCanvas')
    expect(runner).toContain('<ScriptSummary')
    expect(runner).toContain('<SaveConflictModal')
    expect(runner).toContain('<RunLogPanel ')
    expect(logs).toContain('props.onMounted(logBox.value)')
  })

  it('阶段 4：Console 接入共享编辑器外壳，运行视图为只读摘要 + 结构化跳转', () => {
    // 编辑核心收敛在 useScriptEditorShell；旧文本校验器/行扫描模块已删除
    expect(consoleImpl).toContain("import { useScriptEditorShell } from '../../composables/useScriptEditorShell'")
    expect(consoleSource).not.toContain('script-language/validate')
    expect(consoleSource).not.toContain('script-language/line-map')
    // 运行视图：ScriptSummary 摘要模型 + 「从此运行」直发 startIndexOf；call/func 结构化跳转
    expect(consoleImpl).toContain('const summaryModel = computed(')
    expect(consoleImpl).toContain('startIndexOf(summaryModel.value')
    expect(consoleImpl).toContain('function openScriptTarget(')
    expect(consoleImpl).toContain('function runFromStep(')
    // 点击卡片选中/取消运行起点已删（2026-08-30 用户决策：「从此运行」按钮已覆盖，
    // 顶部「运行」恒从头跑），起点只经 run-from 事件直发，不得回潮
    expect(consoleImpl).not.toContain('toggleRunStart')
    expect(consoleImpl).not.toContain('runStartUuid')
    // 保存走 shell（expected_version + 409 冲突回调）
    expect(consoleImpl).toContain('scriptShell.save()')
    expect(consoleImpl).toContain('function onConflictReload(')
    expect(consoleImpl).toContain('function onConflictOverwrite(')
    // alt 模式已整体移除（2026-08-31 用户决策：投屏 Alt 点击/滑动生成步骤、二次裁切
    // Alt 取色均无使用场景），取值改走步骤编辑器的选坐标/屏幕选色按钮；不得回潮
    expect(consoleImpl).not.toContain('scriptShell.insertTapAt(')
    expect(consoleImpl).not.toContain('scriptShell.insertSwipeBetween(')
    expect(consoleImpl).not.toContain('scriptShell.insertColorCheck(')
    expect(consoleImpl).not.toContain('altMode')
    expect(consoleImpl).not.toContain('isAltAction')
    // opRecords 文本拼接路径停用：无 opRecords / renderOpTpl / DEFAULT_OP_TPL 残留
    expect(consoleImpl).not.toContain('opRecords')
    expect(consoleImpl).not.toContain('renderOpTpl')
    expect(consoleImpl).not.toContain('DEFAULT_OP_TPL')
  })

  it('P11.5：裸 Core 只注册 任务/日志/设置（gamer.core:*），业务面板全部 manifest 驱动', () => {
    const core = read('./workspace/core-contributions.ts')
    // Core 自有 UI 只有 gamer.core:*（ADR-11）；gamer.yaml/gamer.keymap 硬编码注册已删
    expect(core).toContain("pluginId: 'gamer.core', panelId: 'tasks'")
    expect(core).toContain("pluginId: 'gamer.core', panelId: 'logs'")
    expect(core).toContain("pluginId: 'gamer.core', panelId: 'settings'")
    expect(core).not.toContain('gamer.yaml')
    expect(core).not.toContain('gamer.keymap')
    // 本地回退注册模块与 iframe fixture 已删除
    expect(() => read('./workspace/keymap-extension.ts')).toThrow()
    expect(() => read('./workspace/yaml-extension.ts')).toThrow()
    expect(read('./workspace/index.ts')).not.toContain('keymap-extension')
    expect(read('./workspace/index.ts')).not.toContain('yaml-extension')
    // Console 壳不再出现扩展 id 硬编码；面板经 server-ui adapter（runtime=core）驱动
    expect(consoleSource).toContain('registerCoreContributions(panelRegistry')
    expect(consoleSource).toContain('createServerUiContributionAdapter')
    expect(consoleSource).not.toContain('gamer.yaml')
    expect(consoleSource).not.toContain('gamer.keymap')
    expect(consoleSource).not.toContain('registerKeymapExtension')
    expect(consoleSource).not.toContain('registerYamlExtensionPanels')
    // 默认面板 = 裸 Core 的任务页签；旧六页签 panelTab 兼容字段已删除，
    // 面板导航只由 activePanelKey + PanelRegistry（URL panel query）驱动
    expect(read('./workspace/registry.ts')).toContain("DEFAULT_PANEL_KEY = 'gamer.core:tasks'")
    expect(consoleSource).toContain('const activePanelKey = ref(DEFAULT_PANEL_KEY)')
    expect(consoleSource).not.toContain('panelTab')
    expect(consoleImpl).not.toContain('panelTab')
    // 分区下拉挂在工具条（func-pkg-row 仅存样式），面板挂载全部走 PluginWorkspace
    expect(template).not.toContain('<div class="func-pkg-row">')
    expect(consoleSource).toContain('class="panel-resizer"')
    expect(consoleSource).toContain('startPanelResize')
    expect(consoleSource).toContain(':style="{ width: `${panelWidth}px` }"')
    expect(consoleSource).not.toContain('isResPanelTab')
  })

  it('按键映射页签与工具条选择器接入当前应用分区', () => {
    const keymap = read('./components/console/KeymapPanel.vue')
    // 映射面板经 console.keymaps 组件键解析，不再有壳内 keymap 页签模板分支
    expect(read('./workspace/core-component-registry.ts')).toContain('console.keymaps')
    expect(template).toContain('v-model="activeKeymapName"')
    expect(template).toContain('无映射')
    expect(consoleImpl).toContain('loadKeymaps(packageId.value)')
    expect(consoleImpl).toContain('api.getKeymap(activeKeymapName.value, packageId.value)')
    expect(consoleImpl).toContain("onRequestPoint: () => pickCoord()")
    expect(consoleImpl).toContain("pickCoord: () => beginCellPick('coord')")
    expect(keymap).toContain('expected_version')
    // keymap zip 导入/导出随分区快照一起退役：面板不再渲染按钮，context 不再注入回调
    expect(keymap).not.toContain('onExport')
    expect(keymap).not.toContain('onImport')
    expect(consoleImpl).not.toContain('api.exportKeymaps')
    expect(consoleImpl).not.toContain('api.importKeymaps')
    expect(consoleImpl).not.toContain('func-app-hint')
    expect(consoleImpl).not.toContain('已加入包名下拉')
  })

  it('旧侧边栏承载的独立页面与入口已删除', () => {
    const layout = read('./layouts/MainLayout.vue')
    const router = read('./router.js')
    expect(layout).not.toContain('sidebar')
    expect(layout).not.toContain('/scripts')
    expect(router).not.toContain("views/ScriptEditor.vue")
    expect(router).not.toContain("TaskScheduler.vue")
    expect(router).not.toContain("RunLogs.vue")
    expect(router).not.toContain("Settings.vue")
    expect(router).not.toContain("path: 'tasks'")
    expect(router).not.toContain("path: 'logs'")
    expect(router).not.toContain("path: 'settings'")
  })
  it('Console 仍保留唯一页面级清理入口，未伪造真机 WebRTC 冒烟', () => {
    expect(consoleSource).toContain('onUnmounted(() => {')
    expect(consoleSource).toContain('cleanup(true)')
    expect(consoleSource).toContain('useWebRtcLifecycle')
  })

  it('连接成功即刷新设备列表（下拉在线/离线随建链更新，不必手动刷新）', () => {
    // onConnectSuccess（手动连接/自动重连/接管共用成功路径）触发轻量列表刷新
    expect(consoleSource).toContain('refreshDeviceStatus()')
    expect(consoleImpl).toContain('async function refreshDeviceStatus()')
    // 只拉列表不扫描（扫描是「🔄 刷新」按钮职责），失败静默不打扰投屏
    const fn = consoleImpl.slice(consoleImpl.indexOf('async function refreshDeviceStatus()'))
    expect(fn.slice(0, fn.indexOf('/** 刷新：扫描 adb'))).toContain('api.listDevices()')
    expect(fn.slice(0, fn.indexOf('/** 刷新：扫描 adb'))).not.toContain('api.scanDevices()')
  })

  it('被接管（taken_over）提示收敛在 webrtc lifecycle：持久横幅 + 阻断自动重连', () => {
    const lifecycle = read('./composables/useWebRtcLifecycle.js')
    expect(lifecycle).toContain("const TAKEN_OVER_MSG = '本页投屏已被其它页面接管，可手动重新连接'")
    expect(lifecycle).toContain('if (errorMsgRef) errorMsgRef.value = TAKEN_OVER_MSG')
    // Console 不再重复挂 taken_over 分支（旧实现双 toast 且不落持久横幅）
    expect(consoleSource).not.toContain("message?.type === 'taken_over'")
  })

  it('分区快照 zip 导入/导出已退役；框选不切页签且保存后回填；函数摘要直达编辑', () => {
    // 分区快照 zip 导入/导出前后端整体退役（替代入口由后续波次重建），不得回潮
    expect(template).not.toContain('exportPartition')
    expect(template).not.toContain('onImportFile')
    expect(consoleImpl).not.toContain('api.exportPartition')
    expect(consoleImpl).not.toContain('runPartitionImport')
    expect(read('./components/console/TemplateCapture.vue')).not.toContain('pkg-bar')
    // 框选生成模板：不切页签 + captureTemplate 以 Promise 回传模板短名（保存/取消 resolve）
    expect(consoleImpl).toContain('cellCaptureResolve')
    const captureFn = consoleImpl.slice(consoleImpl.indexOf('captureTemplate: () => {'))
    expect(captureFn.slice(0, captureFn.indexOf('\n  },'))).not.toContain('panelTab')
    // 函数模式：无总「编辑」按钮，摘要区逐函数「编辑」直达 + 签名展示；编辑态画布锁函数切换
    const runner = read('./components/console/ScriptRunner.vue')
    expect(runner).toContain(`v-if="ctx.runKind === 'script'"`)
    expect(runner).toContain('ctx.editCurrentTarget(view.name)')
    expect(runner).toContain('function fnSignature(')
    expect(runner).toContain(':initial-fn="ctx.editFocusFn"')
    expect(runner).toContain(`:lock-fn="ctx.shell.kind === 'function_library'"`)
    expect(runner).toContain('class="fn-delete-btn"')
    expect(runner).not.toContain('fn-more')
  })

  it('函数库新建直接进入编辑态；参数入口位于步骤入口之前', () => {
    const runner = read('./components/console/ScriptRunner.vue')
    expect(consoleImpl).toContain("scriptShell.newFunctionFile({ file: '新函数库', pkg: packageId.value })")
    expect(consoleImpl).not.toContain("window.prompt('函数库文件短名'")
    expect(runner).toContain(':autofocus="ctx.shell.kind === \'function_library\'"')
    const toolbar = runner.slice(runner.indexOf('class="function-edit-toolbar"'))
    expect(toolbar.indexOf('＋ 添加参数')).toBeLessThan(toolbar.indexOf('＋ 添加步骤'))
    expect(runner).toContain(':show-add-button="ctx.shell.editorContext !== \'function\'"')
  })

  it('模板字段的匹配预览复用宿主步骤语义且只走匹配接口', () => {
    const cell = read('./script-editor/components/CellEditor.vue')
    expect(cell).toContain('框选')
    expect(cell).toContain('匹配')
    expect(cell).toContain('tools.matchTemplate(name)')
    expect(consoleImpl).toContain('matchTemplate: name => testMatch(name, { stepSemantics: true })')
    expect(consoleImpl).toContain('const region = stepSemantics ? undefined : templateRegionPixels(name)')
  })

  it('波次 2-F：运行与模板资源调用点只使用当前契约', () => {
    const layout = read('./layouts/MainLayout.vue')
    const taskBoard = read('./components/TaskBoard.vue')
    const capture = read('./components/console/TemplateCapture.vue')
    const sources = [layout, consoleImpl, taskBoard, capture]

    for (const source of sources) {
      expect(source).not.toContain('api.stopScript')
      expect(source).not.toContain('api.scriptStatus')
      expect(source).not.toContain('api.uploadTemplate')
      expect(source).not.toContain('runScriptId')
      expect(source).not.toContain('normalizeStartReply')
      expect(source).not.toContain('normalizeActiveRunResponse')
      expect(source).not.toContain('isMissingEndpointError')
      expect(source).not.toContain('no_snapshot')
    }
    expect(layout).toContain('api.cancelRun(rid)')
    expect(consoleImpl).toContain('api.getRun(rid)')
    expect(taskBoard).toContain('api.runTaskNow(')
    expect(consoleImpl).toContain('api.replaceTemplateImage(')
    expect(capture).toContain('ctx.replaceTemplateImage(target, file)')
  })

  it('Package 上下文条（导入/导出/新建/复制/删除）落在 PackageContextBar，逻辑收敛在 usePackageContext', () => {
    const bar = read('./workspace/PackageContextBar.vue')
    expect(bar).toContain('title="导入 .gamerpkg 为本地 Package"')
    expect(bar).toContain('title="导出当前 Package 为 .gamerpkg"')
    expect(bar).toContain('title="新建空 Package"')
    expect(bar).toContain('title="复制当前 Package 为新包（保留自己的修改）"')
    expect(bar).toContain('title="删除当前 Package（数据不可恢复）"')
    expect(bar).toContain('accept=".gamerpkg,.zip"')
    expect(bar).toContain('@change="ctx.onImportPicked"')
    // 覆盖导入确认（§9）/新建复制表单/删除确认三弹窗同域挂载
    expect(bar).toContain('ctx.overwriteModal.open')
    expect(bar).toContain('ctx.formModal.open')
    expect(bar).toContain('ctx.deleteModal.open')
    expect(bar).toContain('覆盖该 Package 当前数据')

    const composable = read('./composables/usePackageContext.js')
    expect(composable).toContain('api.importPackageArchive')
    expect(composable).toContain('api.exportPackageArchive')
    expect(composable).toContain('api.createPackage')
    expect(composable).toContain('api.duplicatePackage')
    expect(composable).toContain('api.deletePackage')
    expect(composable).toContain('refreshPackages({ api })')
    // 旧 Installed/Editable 双层 API 不回潮
    expect(composable).not.toContain('installAppPackage')
    expect(composable).not.toContain('editAppPackage')
    expect(composable).not.toContain('api.getWorkspace')

    // Console 装配：包切换/导入后全量重拉（脚本/模板/函数库/映射/基础数据）
    expect(consoleSource).toContain('refreshAll: async () => {')
    expect(consoleSource).toContain('refreshTemplatesData?.()')
    expect(consoleSource).toContain('loadKeymaps(pkg)')
    expect(consoleSource).toContain("import { usePackageContext } from '../composables/usePackageContext'")
    expect(consoleSource).toContain("import { loadPackages, currentPackageId } from '../package-store'")
  })
})
