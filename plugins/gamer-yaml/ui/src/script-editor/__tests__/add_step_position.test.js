// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import StepCanvas from '../components/StepCanvas.vue'
import { setupFunctions, setupScript } from './component_helpers'

let wrapper
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks() })

function mountCanvas(created) {
  const anchor = { left: 400, top: 100, bottom: 128 }
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function () {
    if (this.classList.contains('add-step-panel')) return { height: Math.min(900, parseFloat(this.style.maxHeight) || 480) }
    return anchor
  })
  vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockImplementation(function () {
    return this.classList.contains('step-menu') ? 800 : 0
  })
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(function () {
    return this.classList.contains('step-menu') ? Math.max(0, (parseFloat(this.parentElement.style.maxHeight) || 480) - 100) : 0
  })
  wrapper = mount(StepCanvas, { props: { model: created.model, stack: created.stack }, attachTo: document.body })
  return anchor
}

describe('添加步骤浮层的滚动与视口定位', () => {
  it.each(['script', 'function'])('%s：内部滚动和外部重复定位不清掉高度，选择后正常插入', async kind => {
    const created = kind === 'script' ? setupScript('run: []') : setupFunctions('functions:\n  demo:\n    run: []')
    mountCanvas(created)
    await wrapper.get('.add-btn').trigger('click')
    await flushPromises()
    const panel = document.body.querySelector('.add-step-panel')
    expect(panel.parentElement.parentElement).toBe(document.body)
    const height = panel.style.maxHeight
    expect(parseFloat(height)).toBeLessThanOrEqual(480)
    for (let n = 0; n < 3; n++) {
      panel.querySelector('.step-menu').dispatchEvent(new Event('scroll'))
      window.dispatchEvent(new Event('scroll'))
      await flushPromises()
      expect(panel.style.maxHeight).toBe(height)
    }
    panel.querySelector('[data-kind="repeat"]').click()
    await flushPromises()
    const steps = kind === 'script' ? created.model.run : created.model.functions[0].run
    expect(steps[0].kind).toBe('repeat')
    expect(document.body.querySelector('.add-step-panel')).toBeNull()
  })

  it('靠近底部向上展开，受限后移回宽敞区域可以恢复高度', async () => {
    const anchor = mountCanvas(setupScript('run: []'))
    anchor.top = window.innerHeight - 44
    anchor.bottom = window.innerHeight - 16
    await wrapper.get('.add-btn').trigger('click')
    await flushPromises()
    const panel = document.body.querySelector('.add-step-panel')
    expect(parseFloat(panel.style.top) + parseFloat(panel.style.maxHeight)).toBeLessThan(anchor.top)
    anchor.top = window.innerHeight / 2
    anchor.bottom = anchor.top + 28
    window.dispatchEvent(new Event('resize'))
    await flushPromises()
    expect(parseFloat(panel.style.maxHeight)).toBeLessThan(480)
    anchor.top = 100
    anchor.bottom = 128
    window.dispatchEvent(new Event('resize'))
    await flushPromises()
    expect(panel.style.maxHeight).toBe('480px')
  })
})
