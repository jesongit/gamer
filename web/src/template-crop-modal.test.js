// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import TemplateCropModal from '../../plugins/gamer-yaml/ui/src/components/console/TemplateCropModal.vue'

function context(overrides = {}) {
  return {
    crop: {
      active: true,
      conflict: { shortName: 'login.png', name: 'login#100_100_300_300.png' },
      preview: 'data:image/png;base64,current',
    },
    cropSize: '50×50 px',
    cropZoomPct: '100%',
    tplThumbUrl: vi.fn(name => `/api/templates/${name}`),
    saving: false,
    cancelCrop: vi.fn(),
    overwriteTemplate: vi.fn(),
    backToCrop: vi.fn(),
    ...overrides,
  }
}

describe('TemplateCropModal：模板短名冲突', () => {
  it('冲突态展示当前裁切图与旧模板图，并提供覆盖/返回修改', async () => {
    const ctx = context()
    ctx.backToCrop.mockImplementation(() => { ctx.crop.conflict = null })
    const wrapper = mount(TemplateCropModal, {
      props: { context: ctx, onCropMounted: vi.fn() },
    })

    expect(wrapper.text()).toContain('是否覆盖模板 login#100_100_300_300.png')
    expect(wrapper.findAll('.crop-compare-image img')).toHaveLength(2)
    expect(wrapper.find('button.btn-primary').text()).toContain('确认覆盖')
    expect(wrapper.text()).toContain('返回修改')

    const compareImages = wrapper.findAll('.crop-compare-image')
    await compareImages[0].trigger('wheel', { deltaY: -100 })
    await compareImages[1].trigger('wheel', { deltaY: 100 })
    expect(compareImages[0].find('img').attributes('style')).toContain('scale(1.2)')
    expect(compareImages[1].find('img').attributes('style')).toContain('scale(0.8333333333333334)')

    await wrapper.find('button.btn-primary').trigger('click')
    await wrapper.findAll('button').find(button => button.text() === '返回修改').trigger('click')
    expect(ctx.overwriteTemplate).toHaveBeenCalledTimes(1)
    expect(ctx.backToCrop).toHaveBeenCalledTimes(1)
  })
})

it('框选拖出弹窗不取消或丢失名称；正常遮罩点击与取消按钮仍可关闭', async () => {
  const ctx = context({
    crop: { active: true, conflict: null, name: '尚未保存的模板' },
    cropMouseDown: vi.fn(), cropMouseMove: vi.fn(), cropMouseUp: vi.fn(), cropMouseLeave: vi.fn(), cropWheel: vi.fn(),
  })
  const wrapper = mount(TemplateCropModal, { props: { context: ctx, onCropMounted: vi.fn() } })
  const canvas = wrapper.get('canvas').element
  const mask = wrapper.get('.modal-mask').element
  const pointer = (target, type, x = 10) => target.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerId: 1, isPrimary: true, button: 0, clientX: x, clientY: 10 }))
  pointer(canvas, 'pointerdown')
  pointer(mask, 'pointerup', 200)
  mask.dispatchEvent(new MouseEvent('click', { bubbles: true, clientX: 200, clientY: 10 }))
  expect(ctx.cancelCrop).not.toHaveBeenCalled()
  expect(wrapper.get('input.input').element.value).toBe('尚未保存的模板')
  pointer(mask, 'pointerdown')
  pointer(mask, 'pointerup')
  mask.click()
  expect(ctx.cancelCrop).toHaveBeenCalledTimes(1)
  await wrapper.findAll('button').find(button => button.text() === '取消').trigger('click')
  expect(ctx.cancelCrop).toHaveBeenCalledTimes(2)
  wrapper.unmount()
})
