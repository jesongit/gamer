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
    expect(runner).toContain('props.onEditorMounted(scriptEditor.value)')
    expect(runner).toContain('<RunLogPanel ')
    expect(logs).toContain('props.onMounted(logBox.value)')
  })

  it('Console 仍保留唯一页面级清理入口，未伪造真机 WebRTC 冒烟', () => {
    expect(consoleSource).toContain('onUnmounted(() => {')
    expect(consoleSource).toContain('cleanup(true)')
    expect(consoleSource).toContain('useWebRtcLifecycle')
  })
})
