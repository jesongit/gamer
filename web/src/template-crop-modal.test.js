// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import TemplateCropModal from './components/console/TemplateCropModal.vue'

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
