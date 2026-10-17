// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { defineComponent, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'
import { installDialogAccessibility } from './components/ui/dialog-accessibility'
import { vBackdropDismiss } from '../../plugins/ui-shared/backdrop-dismiss'

let wrapper, dispose, previous
beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'getClientRects').mockImplementation(function () {
    return this.isConnected && !this.hidden && getComputedStyle(this).display !== 'none' ? [{}] : []
  })
  previous = document.createElement('button'); previous.textContent = '打开设置'
  document.body.append(previous); previous.focus()
  dispose = installDialogAccessibility()
})
afterEach(() => { dispose?.(); wrapper?.unmount(); previous?.remove(); vi.restoreAllMocks() })

function mountDialog(blocked = false) {
  const open = ref(true), refuse = ref(blocked), close = vi.fn(() => { if (!refuse.value) open.value = false })
  wrapper = mount(defineComponent({
    directives: { backdropDismiss: vBackdropDismiss },
    setup: () => ({ open, close }),
    template: '<div v-if="open" class="modal-mask" v-backdrop-dismiss="close"><form role="dialog" aria-label="设置"><input value="editable"/><button type="button" @click="close">取消</button><button>保存</button></form></div>',
  }), { attachTo: document.body })
  return { close, refuse }
}

it('Escape uses the existing close action despite the guarded backdrop and restores focus only after dismissal', async () => {
  const { close } = mountDialog(); await flushPromises()
  await wrapper.get('input').trigger('keydown', { key: 'Escape' }); await flushPromises()
  expect(close).toHaveBeenCalledTimes(1)
  expect(wrapper.find('.modal-mask').exists()).toBe(false)
  expect(document.activeElement).toBe(previous)
})

it('Escape respects a refused close and retains the dialog focus', async () => {
  const { close, refuse } = mountDialog(true); await flushPromises()
  const input = wrapper.get('input'); input.element.focus()
  await input.trigger('keydown', { key: 'Escape' }); await flushPromises()
  expect(close).toHaveBeenCalledTimes(1); expect(wrapper.find('.modal-mask').exists()).toBe(true)
  expect(document.activeElement).toBe(input.element)
  refuse.value = false
  await input.trigger('keydown', { key: 'Escape' }); await flushPromises()
  expect(wrapper.find('.modal-mask').exists()).toBe(false)
})

it('ordinary editing shortcuts remain native and Tab wraps within the dialog', async () => {
  const { close } = mountDialog(); await flushPromises()
  const input = wrapper.get('input')
  for (const key of ['a', 'Backspace', 'Delete', 'ArrowLeft']) {
    const event = new KeyboardEvent('keydown', { key, ctrlKey: key === 'a', bubbles: true, cancelable: true })
    input.element.dispatchEvent(event); expect(event.defaultPrevented).toBe(false)
  }
  input.element.focus(); await input.trigger('keydown', { key: 'Tab', shiftKey: true })
  expect(document.activeElement).toBe(wrapper.findAll('button').at(-1).element)
  await wrapper.findAll('button').at(-1).trigger('keydown', { key: 'Tab' })
  expect(document.activeElement).toBe(input.element); expect(close).not.toHaveBeenCalled()
})
