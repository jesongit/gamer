// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, ref } from 'vue'
import { mount } from '@vue/test-utils'
import { vBackdropDismiss } from '../../plugins/ui-shared/backdrop-dismiss'

let wrapper
afterEach(() => { wrapper?.unmount(); wrapper = null; vi.restoreAllMocks() })
function setup() {
  const dismiss = vi.fn()
  wrapper = mount(defineComponent({
    directives: { backdropDismiss: vBackdropDismiss },
    setup: () => ({ dismiss, open: ref(true) }),
    template: '<div v-if="open" class="mask" v-backdrop-dismiss="dismiss"><section><canvas @pointerdown.stop /><input value="unsaved" /></section></div>',
  }))
  return { mask: wrapper.element, panel: wrapper.get('section').element, canvas: wrapper.get('canvas').element, dismiss }
}
function pointer(target, type, options = {}) {
  target.dispatchEvent(new PointerEvent(type, { bubbles: true, button: 0, pointerId: 7, isPrimary: true, clientX: 20, clientY: 20, ...options }))
}
function click(target, options = {}) {
  target.dispatchEvent(new MouseEvent('click', { bubbles: true, button: 0, clientX: 20, clientY: 20, ...options }))
}

describe('弹窗遮罩关闭手势', () => {
  it('只在遮罩上按下和松开时关闭；没有完整手势的点击不关闭', () => {
    const { mask, dismiss } = setup()
    click(mask)
    expect(dismiss).not.toHaveBeenCalled()
    pointer(mask, 'pointerdown')
    pointer(mask, 'pointerup')
    click(mask)
    expect(dismiss).toHaveBeenCalledTimes(1)
    click(mask)
    expect(dismiss).toHaveBeenCalledTimes(1)
  })

  it('框选从弹窗内拖到遮罩后，重定向到遮罩的 click 不关闭；子控件 stop 也安全', () => {
    const { mask, canvas, dismiss } = setup()
    pointer(canvas, 'pointerdown')
    pointer(mask, 'pointerup', { clientX: 200 })
    click(mask, { clientX: 200 })
    expect(dismiss).not.toHaveBeenCalled()
    expect(wrapper.get('input').element.value).toBe('unsaved')
  })

  it('从遮罩拖入弹窗不关闭', () => {
    const { mask, panel, dismiss } = setup()
    pointer(mask, 'pointerdown')
    pointer(panel, 'pointerup')
    click(mask)
    expect(dismiss).not.toHaveBeenCalled()
  })

  it('遮罩上明显拖动后即使回到起点也不关闭，小幅手抖仍可关闭', () => {
    const { mask, dismiss } = setup()
    pointer(mask, 'pointerdown')
    pointer(mask, 'pointermove', { clientX: 200 })
    pointer(mask, 'pointermove')
    pointer(mask, 'pointerup')
    click(mask)
    expect(dismiss).not.toHaveBeenCalled()
    pointer(mask, 'pointerdown')
    pointer(mask, 'pointerup', { clientX: 22 })
    click(mask, { clientX: 22 })
    expect(dismiss).toHaveBeenCalledTimes(1)
  })

  it.each(['pointercancel', 'blur'])('手势 %s 后不关闭，之后正常点击仍有效', reason => {
    const { mask, dismiss } = setup()
    pointer(mask, 'pointerdown')
    if (reason === 'blur') window.dispatchEvent(new Event('blur'))
    else pointer(mask, reason)
    pointer(mask, 'pointerup')
    click(mask)
    expect(dismiss).not.toHaveBeenCalled()
    pointer(mask, 'pointerdown')
    pointer(mask, 'pointerup')
    click(mask)
    expect(dismiss).toHaveBeenCalledTimes(1)
  })

  it('右键、非主触点或不匹配的指针不关闭', () => {
    const { mask, dismiss } = setup()
    for (const options of [{ button: 2 }, { isPrimary: false }]) {
      pointer(mask, 'pointerdown', options)
      pointer(mask, 'pointerup', options)
      click(mask)
    }
    pointer(mask, 'pointerdown')
    pointer(mask, 'pointerup', { pointerId: 8 })
    click(mask)
    expect(dismiss).not.toHaveBeenCalled()
  })

  it.each(['touch', 'pen'])('主 %s 触点的正常遮罩点击可以关闭', pointerType => {
    const { mask, dismiss } = setup()
    pointer(mask, 'pointerdown', { pointerType })
    pointer(mask, 'pointerup', { pointerType })
    click(mask)
    expect(dismiss).toHaveBeenCalledTimes(1)
  })

  it('使用更新后的回调；卸载后释放事件监听', () => {
    const { mask, dismiss } = setup()
    const next = vi.fn()
    vBackdropDismiss.updated(mask, { value: next })
    pointer(mask, 'pointerdown')
    pointer(mask, 'pointerup')
    click(mask)
    expect(next).toHaveBeenCalledTimes(1)
    expect(dismiss).not.toHaveBeenCalled()
    wrapper.unmount(); wrapper = null
    pointer(mask, 'pointerdown')
    pointer(mask, 'pointerup')
    click(mask)
    expect(next).toHaveBeenCalledTimes(1)
  })
})
