// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { frameSource } from './console/frame-source'

afterEach(() => vi.restoreAllMocks())

it('画面冻结在调用时复制像素，尺寸与快照绑定，不持有动态预览元素', () => {
  const draw = vi.fn()
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({ drawImage: draw })
  const preview = { videoWidth: 640, videoHeight: 360 }
  const frames = frameSource(() => preview)
  const frame = frames.captureFrame()
  preview.videoWidth = 1280
  expect(frame.source).not.toBe(preview)
  expect(frame.width).toBe(640)
  expect(frame.source.width).toBe(640)
  expect(frames.displaySize().width).toBe(1280)
  expect(draw).toHaveBeenCalledWith(preview, 0, 0, 640, 360)
})

it('插件画面工具不解释预览元素类型或原生尺寸，目标业务不访问 Android 会话', () => {
  for (const path of [
    '../../plugins/gamer-yaml/ui/src/components/console/useConsoleTemplates.js',
    '../../plugins/gamer-keymap/ui/src/components/console/useConsoleKeymap.js',
  ]) {
    const source = readFileSync(resolve(process.cwd(), 'src', path), 'utf8')
    expect(source).not.toMatch(/\.(naturalWidth|naturalHeight|videoWidth|videoHeight|tagName)\b/)
  }
  const runtime = readFileSync(resolve(process.cwd(), '../plugins/gamer-live/host/runtime.rs'), 'utf8')
  expect(runtime).not.toMatch(/devices\.(snapshot|session)\(/)
  expect(runtime).toContain('targets::app_context')
})
