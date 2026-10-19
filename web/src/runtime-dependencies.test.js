// @vitest-environment happy-dom
import { mount, flushPromises } from '@vue/test-utils'
import { afterEach, describe, expect, it, vi } from 'vitest'
import RuntimeDependencies from './components/RuntimeDependencies.vue'
const row = { id:'tesseract', status:'missing', source:'auto', configured_path:'', detail:'未找到 Tesseract', auto_install:true, path:null }
const view = { revision:'r1', dependencies:[row] }
function response(body, status=200) { return new Response(JSON.stringify(body), {status, headers:{'content-type':'application/json'}}) }
afterEach(() => vi.unstubAllGlobals())
describe('统一运行依赖', () => {
  it('缺失时准备并显示验证结果，不要求安装 OCR 插件', async () => {
    const fetch = vi.fn(async (_url, options={}) => response(options.method==='POST' ? {...view,dependencies:[{...row,status:'ready',source:'managed',path:'C:/runtime/tesseract.exe'}]}:view))
    vi.stubGlobal('fetch',fetch)
    const w=mount(RuntimeDependencies);await flushPromises()
    expect(w.text()).toContain('文字识别已内置')
    await w.get('.runtime-actions button').trigger('click');await flushPromises()
    expect(fetch.mock.calls[1][0]).toBe('/api/system/dependencies/tesseract/prepare')
    expect(w.text()).toContain('依赖已准备并通过验证')
    expect(w.find('.runtime-actions').exists()).toBe(false);w.unmount()
  })
  it('保存冲突保留输入，并携带读取时的 revision', async () => {
    const fetch = vi.fn(async (_url, options={}) => options.method==='PUT' ? response({error:'配置已变化，请重新读取'},409):response(view))
    vi.stubGlobal('fetch',fetch)
    const w=mount(RuntimeDependencies);await flushPromises()
    await w.get('input').setValue('D:/user/tesseract.exe')
    await w.get('form').trigger('submit');await flushPromises()
    expect(JSON.parse(fetch.mock.calls[1][1].body)).toEqual({path:'D:/user/tesseract.exe',expected_revision:'r1'})
    expect(w.get('input').element.value).toBe('D:/user/tesseract.exe')
    expect(w.get('[role="alert"]').text()).toContain('配置已变化');w.unmount()
  })
  it('显式损坏配置不会显示自动替换按钮', async () => {
    vi.stubGlobal('fetch', vi.fn(async()=>response({...view,dependencies:[{...row,status:'broken',configured_path:'D:/missing.exe'}]})))
    const w=mount(RuntimeDependencies);await flushPromises()
    expect(w.find('.runtime-actions').exists()).toBe(false);expect(w.text()).toContain('配置需修正');w.unmount()
  })
})
