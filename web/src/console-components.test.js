import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'

const root = new URL('./', import.meta.url)
const read = (path) => readFileSync(new URL(path, root), 'utf8')

describe('Console 视觉组件拆分静态回归', () => {
  const consoleSource = read('./views/Console.vue')
  const template = consoleSource.slice(0, consoleSource.indexOf('</template>'))

  it('Console 只编排视觉子组件，设备管理收进工具条 + 设置弹窗', () => {
    expect(consoleSource).toContain("import DeviceSettingsModal from '../components/console/DeviceSettingsModal.vue'")
    expect(consoleSource).toContain("import TemplateCapture from '../components/console/TemplateCapture.vue'")
    expect(consoleSource).toContain("import ScriptRunner from '../components/console/ScriptRunner.vue'")
    expect(template).toContain('<DeviceSettingsModal ')
    expect(template).toContain('<TemplateCapture ')
    expect(template).toContain('<ScriptRunner ')
    expect(template).not.toContain('<DevicePanel ')
    expect(template).not.toContain('panel-tabs')
    expect(template).not.toContain('class="dev-pick"')
    expect(template).not.toContain('class="script-tpl"')
    expect(template).not.toContain('class="script-run"')
  })

  it('设备选择/连接/设置/删除等设备控件位于投屏上方工具条', () => {
    const toolbar = template.slice(template.indexOf('class="toolbar"'), template.indexOf('ConsoleVideoStage'))
    expect(toolbar).toContain('v-model="store.deviceId"')
    expect(toolbar).toContain('flushAndConnect')
    expect(toolbar).toContain('refreshDevices')
    expect(toolbar).toContain('startAdd')
    expect(toolbar).toContain('openSettings')
    expect(toolbar).toContain('removeDevice')
    expect(toolbar).toContain('⚙️ 设置')
    // 删除属设备管理：归入设备行（tb-row-dev），不落在投屏控制行
    const devRow = toolbar.slice(toolbar.indexOf('tb-row-dev'), toolbar.indexOf('tb-row-ctrl'))
    expect(devRow).toContain('removeDevice')
  })

  it('子组件保留关键交互入口和挂载回调契约', () => {
    const settings = read('./components/console/DeviceSettingsModal.vue')
    const capture = read('./components/console/TemplateCapture.vue')
    const runner = read('./components/console/ScriptRunner.vue')
    const logs = read('./components/console/RunLogPanel.vue')

    expect(settings).toContain('DeviceVirtualFields')
    expect(settings).toContain('ctx.saveSettings')
    expect(settings).toContain('ctx.cancelSettings')
    expect(settings).toContain('ConsoleDeviceSummary')
    expect(capture).toContain('props.onCropMounted({ canvas: cropCanvas.value, section: cropSec.value })')
    expect(capture).toContain('ctx.cropMouseDown')
    expect(capture).toContain('ctx.onTplUpload')
    // 阶段 4：编辑区换壳——画布锚点提供者注入 shell（Alt 插入与「添加步骤」面板同源），
    // textarea 与 onEditorMounted 挂载回调随旧文本编辑区一并删除
    expect(runner).toContain('ctx.shell.setAnchorProvider')
    expect(runner).not.toContain('onEditorMounted')
    expect(runner).not.toContain('<textarea')
    expect(runner).toContain('<StepCanvas')
    expect(runner).toContain('<ScriptSummary')
    expect(runner).toContain('<SaveConflictModal')
    expect(runner).toContain('<RunLogPanel ')
    expect(logs).toContain('props.onMounted(logBox.value)')
  })

  it('阶段 4：Console 接入共享编辑器外壳，运行视图为只读摘要 + 结构化跳转', () => {
    // 编辑核心收敛在 useScriptEditorShell；旧文本校验器/行扫描模块已删除
    expect(consoleSource).toContain("import { useScriptEditorShell } from '../composables/useScriptEditorShell'")
    expect(consoleSource).not.toContain('script-language/validate')
    expect(consoleSource).not.toContain('script-language/line-map')
    // 运行视图：ScriptSummary 摘要模型 + 「从此运行」直发 startIndexOf；call/func 结构化跳转
    expect(consoleSource).toContain('const summaryModel = computed(')
    expect(consoleSource).toContain('startIndexOf(summaryModel.value')
    expect(consoleSource).toContain('function openScriptTarget(')
    expect(consoleSource).toContain('function runFromStep(')
    // 点击卡片选中/取消运行起点已删（2026-08-30 用户决策：「从此运行」按钮已覆盖，
    // 顶部「运行」恒从头跑），起点只经 run-from 事件直发，不得回潮
    expect(consoleSource).not.toContain('toggleRunStart')
    expect(consoleSource).not.toContain('runStartUuid')
    // 保存走 shell（expected_version + 409 冲突回调）
    expect(consoleSource).toContain('scriptShell.save()')
    expect(consoleSource).toContain('function onConflictReload(')
    expect(consoleSource).toContain('function onConflictOverwrite(')
    // alt 模式已整体移除（2026-08-31 用户决策：投屏 Alt 点击/滑动生成步骤、二次裁切
    // Alt 取色均无使用场景），取值改走步骤编辑器的选坐标/屏幕选色按钮；不得回潮
    expect(consoleSource).not.toContain('scriptShell.insertTapAt(')
    expect(consoleSource).not.toContain('scriptShell.insertSwipeBetween(')
    expect(consoleSource).not.toContain('scriptShell.insertColorCheck(')
    expect(consoleSource).not.toContain('altMode')
    expect(consoleSource).not.toContain('isAltAction')
    // opRecords 文本拼接路径停用：无 opRecords / renderOpTpl / DEFAULT_OP_TPL 残留
    expect(consoleSource).not.toContain('opRecords')
    expect(consoleSource).not.toContain('renderOpTpl')
    expect(consoleSource).not.toContain('DEFAULT_OP_TPL')
  })

  it('阶段 4：独立脚本页为全屏三页签外壳（脚本/函数库/模板 + 右侧错误列表）', () => {
    const editor = read('./views/ScriptEditor.vue')
    expect(editor).toContain("import { useScriptEditorShell } from '../composables/useScriptEditorShell'")
    expect(editor).toContain("import { useFunctionLibrary } from '../composables/useFunctionLibrary'")
    expect(editor).toContain("import SaveConflictModal from '../components/console/SaveConflictModal.vue'")
    expect(editor).toContain('<ErrorSummary ')
    expect(editor).toContain('<StepCanvas')
    // 函数级 params：画布当前函数 → ['functions', 名, 'params']
    expect(editor).toContain(':function-path="fnParamsPath"')
    expect(editor).toContain("['functions', fnName, 'params']")
    // 409 冲突弹窗复用（重载/覆盖）
    expect(editor).toContain('@reload="onConflictReload"')
    expect(editor).toContain('@overwrite="onConflictOverwrite"')
    // 旧文本编辑区（textarea/行号 gutter/Tab 缩进）已删除
    expect(editor).not.toContain('<textarea')
    expect(editor).not.toContain('onEditorTab')
    // 模板页签占位跳 Console；「测试函数」占位（阶段 5）
    expect(editor).toContain('function goConsole(')
    expect(editor).toContain('测试函数')
  })
  it('Console 仍保留唯一页面级清理入口，未伪造真机 WebRTC 冒烟', () => {
    expect(consoleSource).toContain('onUnmounted(() => {')
    expect(consoleSource).toContain('cleanup(true)')
    expect(consoleSource).toContain('useWebRtcLifecycle')
  })

  it('连接成功即刷新设备列表（下拉在线/离线随建链更新，不必手动刷新）', () => {
    // onConnectSuccess（手动连接/自动重连/接管共用成功路径）触发轻量列表刷新
    expect(consoleSource).toContain('refreshDeviceStatus()')
    expect(consoleSource).toContain('async function refreshDeviceStatus()')
    // 只拉列表不扫描（扫描是「🔄 刷新」按钮职责），失败静默不打扰投屏
    const fn = consoleSource.slice(consoleSource.indexOf('async function refreshDeviceStatus()'))
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

  it('波次 2-F：运行与模板资源调用点只使用当前契约', () => {
    const layout = read('./layouts/MainLayout.vue')
    const editor = read('./views/ScriptEditor.vue')
    const scheduler = read('./views/TaskScheduler.vue')
    const capture = read('./components/console/TemplateCapture.vue')
    const sources = [layout, consoleSource, editor, scheduler, capture]

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
    expect(consoleSource).toContain('api.getRun(rid)')
    expect(editor).toContain('api.getRun(rid)')
    expect(scheduler).toContain('const rec = r.value.active ? r.value.run : null')
    expect(editor).toContain('api.createTemplate(')
    expect(editor).toContain('api.replaceTemplateImage(')
    expect(consoleSource).toContain('api.replaceTemplateImage(')
    expect(capture).toContain('ctx.replaceTemplateImage(target, file)')
  })
})
