// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import ThemedCombobox from '../../plugins/ui-shared/ThemedCombobox.vue'
import CellEditor from '../../plugins/gamer-yaml/ui/src/script-editor/components/CellEditor.vue'
import { installDialogAccessibility } from './components/ui/dialog-accessibility'
let wrapper, uninstall, modal
afterEach(() => { uninstall?.(); wrapper?.unmount(); modal?.remove(); vi.restoreAllMocks() })
function dialog() {
  modal = document.createElement('div'); modal.className = 'modal-mask'; document.body.append(modal)
  vi.spyOn(modal, 'getClientRects').mockReturnValue([{}])
  uninstall = installDialogAccessibility()
  const closed = vi.fn(); modal.addEventListener('click', closed)
  return closed
}
it('弹窗中先用 Escape 关闭候选，下次 Escape 才关闭弹窗', async () => {
  const closed = dialog()
  wrapper = mount(ThemedCombobox, { attachTo: modal, props: { modelValue: '', options: ['one', 'two'] } })
  await wrapper.get('.combo-toggle').trigger('click'); closed.mockClear()
  await wrapper.get('input').trigger('keydown', { key: 'Escape' })
  expect(document.querySelector('[role="listbox"]')).toBeNull()
  expect(closed).not.toHaveBeenCalled()
  await wrapper.get('input').trigger('keydown', { key: 'Escape' })
  expect(closed).toHaveBeenCalledTimes(1)
})
it('弹窗中录入 Escape 和 Tab 不关闭弹窗或移动焦点', async () => {
  const closed = dialog()
  wrapper = mount(CellEditor, { attachTo: modal, props: { cell: { lit: 'BACK' }, type: 'key' } })
  for (const key of ['Escape', 'Tab']) {
    await wrapper.get('.key-record').trigger('click'); closed.mockClear()
    await wrapper.get('input').trigger('keydown', { key, code: key })
    expect(wrapper.emitted('change').at(-1)[0]).toEqual({ lit: key })
    expect(closed).not.toHaveBeenCalled()
  }
})
it('向上选中末项、Enter 选择不提交，输入回车可以提交', async () => {
  wrapper = mount(ThemedCombobox, { attachTo: document.body, props: { options: ['one', 'two'] } })
  await wrapper.get('input').trigger('keydown', { key: 'ArrowUp' })
  await wrapper.get('input').trigger('keydown', { key: 'Enter' })
  expect(wrapper.emitted('update:modelValue')).toEqual([['two']])
  expect(wrapper.emitted('commit')).toBeUndefined()
  await wrapper.get('input').trigger('keydown', { key: 'Enter' })
  expect(wrapper.emitted('commit')).toHaveLength(1)
})
