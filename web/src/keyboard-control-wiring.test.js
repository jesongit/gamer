import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'

const root = new URL('./', import.meta.url)
const read = path => readFileSync(new URL(path, root), 'utf8')

describe('投屏键盘控制接线', () => {
  const consoleSource = read('./views/Console.vue')
  const stageSource = read('./components/console/ConsoleVideoStage.vue')

  it('工具条与投屏区域共用键盘焦点容器', () => {
    expect(consoleSource).toContain('ref="stageFocusEl"')
    expect(consoleSource).toContain('tabindex="0"')
    expect(consoleSource).toContain('aria-label="投屏控制区，可接收键盘控制"')
    expect(consoleSource).toContain('@keydown="onStageKeyDown"')
    expect(consoleSource).toContain('@keyup="onStageKeyUp"')
    expect(consoleSource).toContain('@focusin="onStageFocusIn"')
    expect(consoleSource).toContain('@focusout="onStageFocusOut"')
    expect(consoleSource).toContain('@click="onStageClick"')
    expect(consoleSource).toContain('data-keyboard-ignore="true"')
    expect(stageSource).toContain("'keyboard-active': props.keyboardFocused")
    expect(stageSource).toContain('键盘控制已启用')
  })

  it('父层只用 DataChannel 发送键盘，并在焦点/页面生命周期变化时释放', () => {
    expect(consoleSource).toContain('createKeyboardController({')
    expect(consoleSource).toContain('send: sendKeyboardControl')
    expect(consoleSource).toContain('onText: sendControl')
    expect(consoleSource).toContain('mode: keyboardMode')
    expect(consoleSource).toContain("{ type: 'text', text: chunk }")
    expect(consoleSource).toContain('navigator.clipboard.readText()')
    expect(consoleSource).toContain('toggleKeyboardMode')
    expect(consoleSource).toContain("toast('键盘控制通道未连接', 'warn')")
    expect(consoleSource).toContain("window.addEventListener('blur', onWindowBlur)")
    expect(consoleSource).toContain("document.addEventListener('visibilitychange', onVisibilityChange)")
    expect(consoleSource).toContain('keyboard.releaseAll()')
    expect(consoleSource).toContain("channel.readyState === 'open'")
  })
})
