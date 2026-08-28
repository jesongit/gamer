import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'

const root = new URL('./', import.meta.url)
const read = (path) => readFileSync(new URL(path, root), 'utf8')

describe('Console 视觉组件拆分静态回归', () => {
  const consoleSource = read('./views/Console.vue')
  const template = consoleSource.slice(0, consoleSource.indexOf('</template>'))

  it('Console 只编排视觉子组件，原设备/模板/脚本大块模板已移出', () => {
    expect(consoleSource).toContain("import DevicePanel from '../components/console/DevicePanel.vue'")
    expect(consoleSource).toContain("import TemplateCapture from '../components/console/TemplateCapture.vue'")
    expect(consoleSource).toContain("import ScriptRunner from '../components/console/ScriptRunner.vue'")
    expect(template).toContain('<DevicePanel ')
    expect(template).toContain('<TemplateCapture ')
    expect(template).toContain('<ScriptRunner ')
    expect(template).not.toContain('class="dev-pick"')
    expect(template).not.toContain('class="script-tpl"')
    expect(template).not.toContain('class="script-run"')
  })

  it('子组件保留关键交互入口和挂载回调契约', () => {
    const device = read('./components/console/DevicePanel.vue')
    const capture = read('./components/console/TemplateCapture.vue')
    const runner = read('./components/console/ScriptRunner.vue')
    const logs = read('./components/console/RunLogPanel.vue')

    expect(device).toContain('DeviceVirtualFields')
    expect(device).toContain('ctx.flushAndConnect')
    expect(device).toContain('ctx.removeDevice')
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
