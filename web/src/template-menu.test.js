// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import TemplateCapture from '../../plugins/gamer-yaml/ui/src/components/console/TemplateCapture.vue'

let wrapper
afterEach(() => { wrapper?.unmount(); vi.restoreAllMocks() })

it('模板菜单脱离列表裁切、贴近窗口底部向上展开，Esc 恢复焦点且滚动时关闭', async () => {
  const context = {
    packageId: 'default', templates: [{ name: 'button.png' }],
    tplSearch: '', testThreshold: 0.8, testRegion: '', stageReady: false,
    tplThumbUrl: () => '', tplShortName: name => name, tplRegionBadge: () => '',
  }
  const anchorTop = window.innerHeight - 45
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function () {
    if (this.matches('.tpl-more-dropdown')) return { height: 107, width: 140 }
    return { top: anchorTop, bottom: anchorTop + 28, left: 70, right: 130, width: 60, height: 28 }
  })
  wrapper = mount(TemplateCapture, { props: { context }, attachTo: document.body })
  const trigger = wrapper.get('button[aria-haspopup="menu"]')
  await trigger.trigger('click')
  await flushPromises()
  const menu = document.body.querySelector('.tpl-more-dropdown')
  expect(menu.parentElement).toBe(document.body)
  expect(parseFloat(menu.style.top)).toBeLessThan(anchorTop)
  expect(parseFloat(menu.style.top)).toBeGreaterThanOrEqual(8)
  expect(parseFloat(menu.style.left)).toBeGreaterThanOrEqual(8)
  expect(document.activeElement).toBe(menu.querySelector('button'))
  document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
  await flushPromises()
  expect(document.body.querySelector('.tpl-more-dropdown')).toBeNull()
  expect(document.activeElement).toBe(trigger.element)
  await trigger.trigger('click')
  await flushPromises()
  window.dispatchEvent(new Event('scroll'))
  await flushPromises()
  expect(document.body.querySelector('.tpl-more-dropdown')).toBeNull()
})
