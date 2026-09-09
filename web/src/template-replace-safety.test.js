// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { flushPromises, mount } from '@vue/test-utils'

const mocks = vi.hoisted(() => ({
  listTemplates: vi.fn(),
  getPluginResource: vi.fn(),
  putPluginResourceBytes: vi.fn(),
  importTemplateBytes: vi.fn(),
  deleteTemplate: vi.fn(),
  createTemplate: vi.fn(),
  replaceTemplateImage: vi.fn(),
}))
const videoMocks = vi.hoisted(() => ({
  mediaFrameUrl: vi.fn(() => '/api/media/m1/frame?index=5'),
  createTemplateFromFrame: vi.fn(),
  visionTestTemplate: vi.fn(),
}))

vi.mock('./api', () => ({ api: mocks }))
vi.mock('./components/video/videoApi', () => ({ videoApi: videoMocks }))

import { useConsoleTemplates } from './components/console/useConsoleTemplates'
import TemplateStudio from './components/video/TemplateStudio.vue'
import { templateShortName } from './console/template-resource'

function createTemplate(name = 'hero#100_200_500_500.png', version = 'v-old') {
  return { name, pkg: 'pkg-a', version, updated_at: '', size: 3 }
}

function mountTemplates() {
  let templates
  const wrapper = mount(defineComponent({
    setup() {
      templates = useConsoleTemplates({
        toast: vi.fn(),
        store: reactiveStore(),
        templatesData: ref([]),
        packageId: ref('pkg-a'),
        connected: ref(false),
        videoElement: ref(null),
        videoWrap: ref(null),
        current: ref({ width: 100, height: 100 }),
      })
      return () => h('div')
    },
  }))
  return { templates, wrapper }
}

function reactiveStore() {
  return { deviceId: 'device-a' }
}

function prepareCrop(templates, conflict = null) {
  Object.assign(templates.crop, {
    active: true,
    imgW: 100,
    imgH: 100,
    baseW: 40,
    baseH: 30,
    originX: 10,
    originY: 20,
    preview: 'data:image/png;base64,QUJD',
    name: 'hero.png',
    preserveColor: false,
    conflict,
  })
}

describe('模板资源替换安全性（P1-T）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mocks.listTemplates.mockResolvedValue([])
    mocks.getPluginResource.mockResolvedValue({ arrayBuffer: async () => new Uint8Array([79, 76, 68]).buffer })
    mocks.putPluginResourceBytes.mockResolvedValue({ name: 'hero.png', version: 'v-new' })
  })

  it('短名匹配同时剥离区域和颜色标记', () => {
    expect(templateShortName('hero#100_200_500_500#1.png')).toBe('hero.png')
  })

  it('框选覆盖使用同一路径的条件 PUT，绝不先删除旧模板', async () => {
    const existing = createTemplate()
    mocks.listTemplates.mockResolvedValue([existing])
    const { templates, wrapper } = mountTemplates()
    await flushPromises()
    prepareCrop(templates, { name: existing.name, shortName: 'hero.png', version: existing.version })

    await templates.overwriteTemplate()

    expect(mocks.deleteTemplate).not.toHaveBeenCalled()
    expect(mocks.createTemplate).not.toHaveBeenCalled()
    expect(mocks.putPluginResourceBytes).toHaveBeenCalledWith(
      'pkg-a',
      'gamer.yaml',
      `templates/${existing.name}`,
      new Uint8Array([65, 66, 67]),
      { expectedVersion: 'v-old' },
    )
    expect(templates.crop.active).toBe(false)
    expect(templates.crop.conflict).toBeNull()
    wrapper.unmount()
  })

  it('替换上传也携带列表中的资源版本，上传失败时不触碰旧模板', async () => {
    const existing = createTemplate()
    mocks.listTemplates.mockResolvedValue([existing])
    const { templates, wrapper } = mountTemplates()
    await flushPromises()
    const OriginalFileReader = globalThis.FileReader
    vi.stubGlobal('FileReader', class {
      readAsDataURL() {
        this.result = 'data:image/png;base64,REVG'
        this.onload?.()
      }
    })
    mocks.putPluginResourceBytes.mockRejectedValueOnce(new Error('图片非法'))

    await templates.replaceTemplateImage(existing, new Blob(['new-image'], { type: 'image/png' }))

    expect(mocks.putPluginResourceBytes).toHaveBeenCalledWith(
      'pkg-a',
      'gamer.yaml',
      `templates/${existing.name}`,
      new Uint8Array([68, 69, 70]),
      { expectedVersion: 'v-old' },
    )
    expect(mocks.deleteTemplate).not.toHaveBeenCalled()
    expect(mocks.createTemplate).not.toHaveBeenCalled()
    vi.stubGlobal('FileReader', OriginalFileReader)
    wrapper.unmount()
  })

  it('列表缺少二进制模板版本时先读取并计算版本，再执行条件 PUT', async () => {
    const existing = createTemplate()
    existing.version = null
    mocks.listTemplates.mockResolvedValue([existing])
    const { templates, wrapper } = mountTemplates()
    await flushPromises()
    prepareCrop(templates, { name: existing.name, shortName: 'hero.png', version: null })

    await templates.overwriteTemplate()

    expect(mocks.getPluginResource).toHaveBeenCalledWith(
      'pkg-a', 'gamer.yaml', `templates/${existing.name}`,
    )
    expect(mocks.putPluginResourceBytes).toHaveBeenCalledWith(
      'pkg-a', 'gamer.yaml', `templates/${existing.name}`,
      new Uint8Array([65, 66, 67]),
      { expectedVersion: expect.stringMatching(/^[0-9a-f]{12}$/) },
    )
    expect(mocks.deleteTemplate).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('框选区域或颜色标记变化时不把新元数据写进旧模板路径', async () => {
    const existing = createTemplate()
    mocks.listTemplates.mockResolvedValue([existing])
    const { templates, wrapper } = mountTemplates()
    await flushPromises()
    prepareCrop(templates, { name: existing.name, shortName: 'hero.png', version: existing.version })
    templates.crop.baseW = 30

    await templates.overwriteTemplate()

    expect(mocks.putPluginResourceBytes).not.toHaveBeenCalled()
    expect(mocks.deleteTemplate).not.toHaveBeenCalled()
    expect(templates.crop.active).toBe(true)
    expect(templates.crop.conflict).toEqual({
      name: existing.name,
      shortName: 'hero.png',
      version: existing.version,
    })
    wrapper.unmount()
  })

  it('新建框选模板不使用 force，服务端失败时保留未提交裁切态', async () => {
    const { templates, wrapper } = mountTemplates()
    await flushPromises()
    prepareCrop(templates)
    mocks.putPluginResourceBytes.mockRejectedValueOnce(new Error('PNG 校验失败'))

    await templates.saveTemplate()

    expect(mocks.putPluginResourceBytes).toHaveBeenCalledWith(
      'pkg-a',
      'gamer.yaml',
      'templates/hero#100_200_500_500.png',
      new Uint8Array([65, 66, 67]),
      {},
    )
    expect(templates.crop.active).toBe(true)
    expect(templates.crop.preview).toBe('data:image/png;base64,QUJD')
    wrapper.unmount()
  })

  it('模板页批量导入只创建，不调用 force=true 兼容封装', async () => {
    const { templates, wrapper } = mountTemplates()
    await flushPromises()

    await templates.onTplUpload({
      target: {
        files: [new File([new Uint8Array([1, 2, 3])], 'new.png', { type: 'image/png' })],
        value: 'selected',
      },
    })

    expect(mocks.importTemplateBytes).not.toHaveBeenCalled()
    expect(mocks.putPluginResourceBytes).toHaveBeenCalledWith(
      'pkg-a', 'gamer.yaml', 'templates/new.png', new Uint8Array([1, 2, 3]), {},
    )
    wrapper.unmount()
  })

  it('版本冲突只刷新冲突态，不删除或关闭旧模板', async () => {
    const stale = createTemplate('hero#100_200_500_500.png', 'v-old')
    const current = createTemplate('hero#100_200_500_500.png', 'v-current')
    mocks.listTemplates
      .mockResolvedValueOnce([stale])
      .mockResolvedValueOnce([stale])
      .mockResolvedValueOnce([current])
    const { templates, wrapper } = mountTemplates()
    await flushPromises()
    prepareCrop(templates, { name: stale.name, shortName: 'hero.png', version: stale.version })
    const conflict = Object.assign(new Error('版本冲突'), { status: 409 })
    mocks.putPluginResourceBytes.mockRejectedValueOnce(conflict)

    await templates.overwriteTemplate()

    expect(mocks.deleteTemplate).not.toHaveBeenCalled()
    expect(templates.crop.active).toBe(true)
    expect(templates.crop.conflict).toMatchObject({
      name: current.name,
      shortName: 'hero.png',
      version: 'v-current',
    })
    wrapper.unmount()
  })

  it('视频模板制作覆盖复用条件 PUT，保留既有文件名/区域元数据', async () => {
    const existing = createTemplate('hero#100_200_500_500.png', 'v-old')
    mocks.listTemplates.mockResolvedValue([existing])
    videoMocks.createTemplateFromFrame.mockRejectedValueOnce(
      new Error('模板短名冲突: hero#100_200_500_500.png（确认覆盖请带 overwrite:true）'),
    )
    const context = {
      getContext: vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
        imageSmoothingEnabled: false,
        drawImage: vi.fn(),
      }),
      toBlob: vi.spyOn(HTMLCanvasElement.prototype, 'toBlob').mockImplementation(callback => {
        callback(new Blob(['png'], { type: 'image/png' }))
      }),
    }
    const OriginalFileReader = globalThis.FileReader
    vi.stubGlobal('FileReader', class {
      readAsDataURL() {
        this.result = 'data:image/png;base64,QUJD'
        this.onload?.()
      }
    })
    const wrapper = mount(TemplateStudio, {
      props: {
        open: true,
        media: { id: 'm1', width: 100, height: 100 },
        frame: { frameIndex: 5, ptsUs: 5000 },
        calibration: { version: 1 },
        packageId: 'pkg-a',
        yamlReady: true,
      },
    })
    const image = wrapper.find('[data-testid="studio-frame-image"]')
    Object.defineProperty(image.element, 'naturalWidth', { configurable: true, value: 100 })
    Object.defineProperty(image.element, 'naturalHeight', { configurable: true, value: 100 })
    image.element.getBoundingClientRect = () => ({ left: 0, top: 0, width: 100, height: 100 })
    await image.trigger('load')
    await wrapper.find('[data-testid="studio-template-name"]').setValue('hero')
    await image.trigger('mousedown', { button: 0, clientX: 10, clientY: 20 })
    await image.trigger('mousemove', { clientX: 50, clientY: 50 })
    await image.trigger('mouseup')
    await wrapper.vm.$.setupState.saveTemplate()
    await flushPromises()

    expect(wrapper.find('[data-testid="studio-overwrite"]').exists()).toBe(true)
    await wrapper.find('[data-testid="studio-overwrite"]').setValue(true)
    await wrapper.vm.$.setupState.saveTemplate()
    await flushPromises()

    expect(videoMocks.createTemplateFromFrame).toHaveBeenCalledTimes(1)
    expect(mocks.deleteTemplate).not.toHaveBeenCalled()
    expect(mocks.putPluginResourceBytes).toHaveBeenCalledWith(
      'pkg-a',
      'gamer.yaml',
      `templates/${existing.name}`,
      new Uint8Array([65, 66, 67]),
      { expectedVersion: 'v-old' },
    )
    expect(wrapper.text()).toContain('模板已保存')

    context.getContext.mockRestore()
    context.toBlob.mockRestore()
    vi.stubGlobal('FileReader', OriginalFileReader)
    wrapper.unmount()
  })
})
