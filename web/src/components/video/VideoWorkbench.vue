<template>
  <section class="video-workbench" data-testid="video-workbench">
    <MediaLibrary
      :media-list="mediaList"
      :loading="loading"
      :selected-id="selectedId"
      @select="onSelect"
      @refresh="refresh"
      @changed="refresh"
      @recording-finished="onRecordingFinished"
    />
    <VideoTimeline :media="selectedMedia" />
    <VideoDraft v-model:recording-id="recordingId" />
  </section>
</template>

<script setup>
// 视频工作台（gamer.video 插件 core 面板，manifest component = "VideoWorkbench"）。
// 内部三区：素材库（列表/导入/删除/录制入口）→ 时间轴（离线预览 + 服务端精确帧）→
// 草稿（勾选操作事件 → automation.create_draft → YAML 文本与诊断）。
// 面板自取数据（videoApi 直调合同 §1/§2 REST），不依赖宿主 context；纯离线面板，
// 不连接设备、不发送任何设备输入（录制入口只调录制服务 REST）。
import { computed, onMounted, ref } from 'vue'
import MediaLibrary from './MediaLibrary.vue'
import VideoDraft from './VideoDraft.vue'
import VideoTimeline from './VideoTimeline.vue'
import { videoApi } from './videoApi'

const mediaList = ref([])
const loading = ref(false)
const selectedId = ref('')
const recordingId = ref('') // 最近一次录制会话（停止后自动带入草稿区）

const selectedMedia = computed(() => mediaList.value.find(m => m.id === selectedId.value) || null)

async function refresh() {
  loading.value = true
  try {
    mediaList.value = await videoApi.listMedia()
    // 选中项被删除后回落到空态，不自动跳选其它素材
    if (selectedId.value && !mediaList.value.some(m => m.id === selectedId.value)) {
      selectedId.value = ''
    }
  } catch (e) {
    mediaList.value = []
  } finally {
    loading.value = false
  }
}

function onSelect(id) {
  selectedId.value = id
}

function onRecordingFinished(meta) {
  const id = meta && typeof meta === 'object' ? meta.id : meta
  if (id) recordingId.value = String(id)
}

onMounted(() => { void refresh() })
</script>

<style scoped>
.video-workbench { display: flex; flex: 1; min-height: 0; flex-direction: column; gap: 12px; overflow: auto; }
</style>
