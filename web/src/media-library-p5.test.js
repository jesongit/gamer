// @vitest-environment happy-dom

import { describe, expect, it, vi, beforeEach } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'

vi.mock('../../plugins/gamer-video/ui/src/components/video/videoApi', () => ({
  videoApi: {
    recordingHistory: vi.fn(async () => []),
    activeRecording: vi.fn(async () => null),
    recordingStart: vi.fn(async () => ({ id: 'rec-new', state: 'recording' })),
    recordingStop: vi.fn(async () => ({ id: 'rec-new', state: 'completed' })),
    recordingCancel: vi.fn(async () => ({ id: 'rec-new', state: 'cancelled' })),
    recordingEvents: vi.fn(async () => []),
    importMedia: vi.fn(async () => ({})),
    deleteMedia: vi.fn(async () => null),
  },
}))

import MediaLibrary from '../../plugins/gamer-video/ui/src/components/video/MediaLibrary.vue'
import { videoApi } from '../../plugins/gamer-video/ui/src/components/video/videoApi'
import { devicesData } from './store'

const MEDIA = [
  {
    id: 'recording-a', name: '战斗-上午.mp4', source: 'recording', state: 'ready',
    created_at: '2026-09-08T08:00:00Z', duration_us: 30e6, device_id: 'dev-a',
    refs: [{ package_id: 'pkg-a', plugin_id: 'gamer-video', kind: 'project' }],
  },
  {
    id: 'recording-b', name: '战斗-长录制.mp4', source: 'recording', state: 'completed',
    created_at: '2026-09-07T08:00:00Z', duration_us: 400e6, device_id: 'dev-b', refs: [],
  },
  { id: 'import-a', name: '外部视频.mp4', source: 'import', state: 'ready', duration_us: 2e6 },
]

beforeEach(() => {
  devicesData.value = []
  vi.clearAllMocks()
  videoApi.recordingHistory.mockResolvedValue(MEDIA.slice(0,2).map((media,index) => ({ id: `session-${index}`, device_id: media.device_id, started_at: media.created_at, state: 'completed', event_count: 0, segments: [{media_id:media.id,duration_us:media.duration_us,reason:'normal'}], missing_media: [] })))
})

describe('P5-MEDIA 素材库与录制历史', () => {
  it('历史使用真实会话查询，并可按名称/时间/设备/时长/状态筛选', async () => {
    const wrapper = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    await flushPromises()
    expect(wrapper.findAll('[data-testid="recording-history-row"]')).toHaveLength(2)

    await wrapper.find('[data-testid="recording-history-search"]').setValue('长录制')
    expect(wrapper.findAll('[data-testid="recording-history-row"]')).toHaveLength(1)

    await wrapper.find('[data-testid="recording-history-search"]').setValue('')
    await wrapper.find('[data-testid="recording-history-date"]').setValue('2026-09-08')
    await wrapper.find('[data-testid="recording-history-device"]').setValue('dev-a')
    await wrapper.find('[data-testid="recording-history-duration"]').setValue('short')
    await wrapper.find('[data-testid="recording-history-status"]').setValue('completed')
    expect(wrapper.findAll('[data-testid="recording-history-row"]')).toHaveLength(1)
    expect(wrapper.find('[data-testid="recording-history-row"]').text()).toContain('战斗-上午')
    wrapper.unmount()
  })

  it('历史条目选择素材并传递真实会话 ID', async () => {
    const wrapper = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    await flushPromises()
    await wrapper.findAll('[data-testid="recording-history-row"]')[0].trigger('click')
    expect(wrapper.emitted('select')).toEqual([['recording-a']])
    expect(wrapper.emitted('recording-selected')[0][0].id).toBe('session-0')
    expect(wrapper.find('[data-testid="recording-id-input"]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('区分素材/历史读取失败与空列表，并保留最小重试入口', async () => {
    videoApi.recordingHistory.mockResolvedValueOnce([])
    const empty = mount(MediaLibrary, { props: { mediaList: [] } })
    await flushPromises()
    expect(empty.find('[data-testid="recording-history-empty"]').exists()).toBe(true)
    empty.unmount()

    videoApi.recordingHistory.mockRejectedValueOnce(new Error('网络不可用'))
    const failed = mount(MediaLibrary, { props: { mediaList: [], loadError: '网络不可用' } })
    await flushPromises()
    expect(failed.find('[data-testid="media-load-error"]').text()).toContain('网络不可用')
    expect(failed.find('[data-testid="recording-history-failed"]').exists()).toBe(true)
    await failed.find('[data-testid="media-retry"]').trigger('click')
    expect(failed.emitted('refresh')).toHaveLength(1)
    failed.unmount()
  })

  it('删除被引用素材时展示服务端原因和实际引用，不绕过 409', async () => {
    videoApi.deleteMedia.mockRejectedValueOnce(Object.assign(new Error('media_referenced'), {
      status: 409, code: 'media_referenced',
    }))
    const wrapper = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    const button = wrapper.findAll('[data-testid="media-row"]')[0].find('button')
    await button.trigger('click')
    await button.trigger('click')
    await flushPromises()
    expect(wrapper.find('[data-testid="media-error"]').text()).toContain('pkg-a / gamer-video / project')
    expect(videoApi.deleteMedia).toHaveBeenCalledWith('recording-a')
    wrapper.unmount()
  })

  it('当前录制停止/取消使用后端返回的会话，不向用户暴露手填 ID', async () => {
    const session = { id: 'rec-live', device_id: 'dev-a', state: 'recording', event_count: 1 }
    videoApi.activeRecording.mockResolvedValue(session)
    videoApi.recordingStop.mockResolvedValue({ ...session, state: 'completed' })
    devicesData.value = [{ id: 'dev-a', name: '设备 A' }]
    const wrapper = mount(MediaLibrary, { props: { mediaList: MEDIA } })
    await wrapper.find('[data-testid="record-device"]').setValue('dev-a')
    await flushPromises()
    await wrapper.find('[data-testid="record-stop"]').trigger('click')
    await flushPromises()
    expect(videoApi.recordingStop).toHaveBeenCalledWith('rec-live')
    expect(wrapper.find('[data-testid="recording-id-input"]').exists()).toBe(false)
    wrapper.unmount()
  })
})

