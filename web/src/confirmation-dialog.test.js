// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { effectScope, nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import ConfirmDialogHost from './components/ui/ConfirmDialogHost.vue'
import { confirmation, finishConfirmation, useConfirmDialog } from './components/ui/useConfirmDialog'

let host, scope
afterEach(() => { scope?.stop(); host?.unmount(); finishConfirmation(false); document.body.innerHTML = ''; vi.restoreAllMocks() })
function setup() {
  scope = effectScope()
  const ask = scope.run(() => useConfirmDialog())
  host = mount(ConfirmDialogHost, { attachTo: document.body })
  return ask
}
function click(label) { [...document.querySelectorAll('.confirmation-dialog button')].find(button => button.textContent === label).click() }

it('默认聚焦取消，约束焦点，Esc 取消并恢复原焦点', async () => {
  const trigger = document.createElement('button'); document.body.append(trigger); trigger.focus()
  const ask = setup()
  const pending = ask('更新测试插件', { title: '确认更新插件', confirmText: '确认更新' })
  await flushPromises()
  expect(document.activeElement.textContent).toBe('取消')
  expect(trigger.inert).toBe(true)
  const buttons = [...document.querySelectorAll('.confirmation-dialog button')]
  buttons.at(-1).focus()
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', cancelable: true }))
  expect(document.activeElement).toBe(buttons[0])
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', cancelable: true }))
  expect(await pending).toBe(false)
  await nextTick()
  expect(document.activeElement).toBe(trigger)
  expect(trigger.inert).toBe(false)
})

it('显式确认才返回 true；内容保持纯文本，重复请求不绕过确认', async () => {
  const ask = setup()
  const pending = ask('<img src=x onerror=alert(1)>', { confirmText: '删除', danger: true, fields: [{ label: '版本', value: '1.0 → 1.1' }] })
  await flushPromises()
  expect(document.querySelector('.confirmation-dialog img')).toBeNull()
  expect(document.querySelector('.confirmation-body').textContent).toContain('<img')
  expect(await ask('第二次操作')).toBe(false)
  click('删除')
  expect(await pending).toBe(true)
  expect(confirmation.value).toBeNull()
})

it('关闭按钮、遮罩和调用方销毁均取消；已销毁调用方不能再开框', async () => {
  const ask = setup()
  let pending = ask('第一次'); await flushPromises()
  document.querySelector('[aria-label="关闭确认框"]').click()
  expect(await pending).toBe(false)
  await flushPromises()
  pending = ask('第二次'); await flushPromises()
  document.querySelector('.confirmation-mask').click()
  expect(await pending).toBe(false)
  await flushPromises()
  pending = ask('第三次'); await flushPromises()
  scope.stop()
  expect(await pending).toBe(false)
  expect(await ask('失效页面')).toBe(false)
  await nextTick()
  expect(document.querySelector('.confirmation-dialog')).toBeNull()
})

it('已有确认不接受其他调用方插队或代为取消', async () => {
  const ask = setup(), other = useConfirmDialog()
  const pending = ask('第一操作')
  expect(await other('第二操作')).toBe(false)
  other.cancel()
  expect(confirmation.value.message).toBe('第一操作')
  ask.cancel()
  expect(await pending).toBe(false)
})
