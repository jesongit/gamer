import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'

const root = new URL('./', import.meta.url)
const read = path => readFileSync(new URL(path, root), 'utf8')

describe('投屏键盘控制接线', () => {
  const consoleSource = read('./views/Console.vue')
  const stageSource = read('./components/console/ConsoleVideoStage.vue')

  it('投屏区域可聚焦并将键盘事件交给父层', () => {
    expect(stageSource).toContain('tabindex="0"')
    expect(stageSource).toContain('aria-label="投屏画面，可接收键盘控制"')
    expect(stageSource).toContain('@keydown="props.onKeyDown"')
    expect(stageSource).toContain('@keyup="props.onKeyUp"')
    expect(stageSource).toContain('@focus="props.onFocus"')
    expect(stageSource).toContain('@blur="props.onBlur"')
    expect(stageSource).toContain('@click="focusStageOnClick"')
    expect(stageSource).toContain("'keyboard-active': props.keyboardFocused")
    expect(stageSource).toContain('键盘控制已启用')
    expect(stageSource).toContain('target.closest(')
    expect(stageSource).toContain('button, input, select, textarea, a, [contenteditable]')
  })

  it('父层只用 DataChannel 发送键盘，并在焦点/页面生命周期变化时释放', () => {
    expect(consoleSource).toContain('createKeyboardController({ send: sendKeyboardControl })')
    expect(consoleSource).toContain(':on-key-down="onVideoKeyDown"')
    expect(consoleSource).toContain(':on-key-up="onVideoKeyUp"')
    expect(consoleSource).toContain("toast('键盘控制通道未连接', 'warn')")
    expect(consoleSource).toContain("window.addEventListener('blur', onWindowBlur)")
    expect(consoleSource).toContain("document.addEventListener('visibilitychange', onVisibilityChange)")
    expect(consoleSource).toContain('keyboard.releaseAll()')
    expect(consoleSource).toContain("channel.readyState === 'open'")
  })
})
