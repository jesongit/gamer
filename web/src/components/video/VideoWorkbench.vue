<template>
  <section class="video-workbench" data-testid="video-workbench">
    <!-- 面板内子导航（非 Core 主页签；计划 §9.3：右侧内部按真实职责分区） -->
    <nav class="workbench-tabs" data-testid="workbench-tabs">
      <button
        v-for="tab in TABS"
        :key="tab.key"
        class="tab-btn"
        :class="{ active: activeTab === tab.key }"
        type="button"
        :data-testid="`workbench-tab-${tab.key}`"
        @click="activeTab = tab.key"
      >{{ tab.label }}</button>
    </nav>

    <div v-if="!packageId" class="zone-error" role="alert" data-testid="package-missing-banner">
      没有当前 Package：制作项目保存在 Package 数据上下文中，请先在顶部 Package 条选择或新建一个 Package
    </div>

    <template v-if="activeTab === 'library'">
      <MediaLibrary
        :media-list="mediaList"
        :loading="loading"
        :selected-id="selectedId"
        @select="onSelect"
        @refresh="refresh"
        @changed="refresh"
        @recording-finished="onRecordingFinished"
      />
    </template>

    <template v-else-if="activeTab === 'projects'">
      <div v-if="!mediaList.length" class="zone-note" role="status" data-testid="projects-no-media-note">
        素材库为空：项目需要引用至少一个素材，请先在「素材库」导入或录制
      </div>
      <VideoProjects
        :projects="projectSummaries"
        :open-id="openId"
        :loading="projectsLoading"
        :can-create="!!packageId && !!selectedId"
        @open="openProjectById"
        @create="createProject"
        @rename="renameProject"
        @delete="deleteProject"
        @refresh="loadProjects"
      />

      <!-- 项目详情：素材缺失状态 + 时间轴（标记/校准/事件）+ 保存 -->
      <template v-if="openProject">
        <div v-if="primaryMissing" class="zone-error" role="alert" data-testid="asset-missing-banner">
          素材缺失：主素材 {{ primaryAssetId }} 不在媒体库中（原视频未复制进 Package，可能已被删除或属其他环境）。
          项目数据完好，可重新导入同名素材后继续
        </div>
        <div v-else-if="staleSaveError" class="zone-error" role="alert" data-testid="project-conflict-banner">
          {{ staleSaveError }}
          <button class="mini-btn" type="button" data-testid="project-reload" @click="reloadOpenProject">重新加载</button>
        </div>

        <div class="project-toolbar" data-testid="project-toolbar">
          <span class="project-title" data-testid="open-project-name">{{ openProject.name }}</span>
          <span class="mono project-state" data-testid="project-dirty" :class="{ dirty: projectDirty }">
            {{ projectDirty ? '未保存改动' : '已保存' }}
          </span>
          <button
            class="btn btn-sm btn-primary"
            type="button"
            :disabled="!projectDirty || saving || !!primaryMissing"
            data-testid="project-save"
            @click="saveProject"
          >{{ saving ? '保存中…' : '保存项目' }}</button>
          <button v-if="openProject.recording?.recording_id" class="btn btn-sm" type="button" data-testid="project-open-draft" @click="openDraft">
            → 草稿区
          </button>
        </div>

        <VideoTimeline
          :media="primaryMedia"
          :markers="openProject.markers"
          :calibration="openProject.calibration"
          :recording-id="openProject.recording?.recording_id || ''"
          @add-marker="onAddMarker"
          @remove-marker="onRemoveMarker"
          @update-marker="onUpdateMarker"
          @save-calibration="onSaveCalibration"
        />
      </template>
      <div v-else class="zone-empty" data-testid="project-detail-empty">选择一个项目进行制作（打开项目会联动左侧画面来源）</div>
    </template>

    <template v-else>
      <VideoDraft v-model:recording-id="recordingId" />
    </template>
  </section>
</template>

<script setup>
// 视频工作台宿主（gamer.video core 面板，Phase 6 重组）：
// - 子导航「素材库 | 项目 | 草稿」——按真实职责拆分，不新增 Core 永久页签；
// - 项目 = Package 资源（projects/<id>.json），乐观并发保存；损坏项目可诊断；
// - 打开项目联动左侧舞台切到主媒体（requestStageMedia：只动舞台来源，
//   不改 deviceId/androidPackageName/currentPackageId 四 Context）；
// - 状态 UI：缺 Package / 素材缺失 / 保存冲突（version_conflict 可重载）。
// 面板自取数据（videoApi），纯离线制作，不发送任何设备输入。
import { computed, onMounted, ref, watch } from 'vue'
import MediaLibrary from './MediaLibrary.vue'
import VideoDraft from './VideoDraft.vue'
import VideoProjects from './VideoProjects.vue'
import VideoTimeline from './VideoTimeline.vue'
import { requestStageMedia } from '../console/useConsoleStage'
import { packageStore } from '../../package-store'
import { videoApi } from './videoApi'
import { assetStatus, newProject, parseProject, projectIdFromPath, serializeProject, validateProject, withCalibration, withMarker, withoutMarker, withMarkerText } from './videoProject'

const TABS = [
  { key: 'library', label: '素材库' },
  { key: 'projects', label: '项目' },
  { key: 'draft', label: '草稿' },
]

// 当前 Package（数据上下文，plan §39）：面板自取（registry 契约 = 自包含组件，
// 无宿主 context 注入）；切换 Package 后项目列表随 loadProjects 联动。
const packageId = computed(() => packageStore.currentPackageId)

const activeTab = ref('library')
const mediaList = ref([])
const loading = ref(false)
const selectedId = ref('')
const recordingId = ref('') // 最近一次录制会话（停止后自动带入草稿区）

// ---- 项目状态 ----
const projectsLoading = ref(false)
const projectSummaries = ref([]) // [{id, name, valid, markerCount, assetCount, entry}]
const openId = ref('')
const openProject = ref(null) // 已解析项目对象（编辑中的本地工作副本）
const projectVersion = ref('') // 打开/最近保存时的资源 version 短码（乐观并发）
const projectDirty = ref(false)
const saving = ref(false)
const staleSaveError = ref('')

const openAsset = computed(() => (openProject.value
  ? (openProject.value.assets || []).find(asset => asset.role === 'primary') || null
  : null))
const primaryAssetId = computed(() => openAsset.value?.media_id || '')
const primaryMedia = computed(() => mediaList.value.find(media => media.id === primaryAssetId.value) || null)
const primaryMissing = computed(() => !!openProject.value && !primaryMedia.value)

// ---------- 素材库 ----------

async function refresh() {
  loading.value = true
  try {
    mediaList.value = await videoApi.listMedia()
    // 选中项被删除后回落到空态，不自动跳选其它素材
    if (selectedId.value && !mediaList.value.some(media => media.id === selectedId.value)) {
      selectedId.value = ''
    }
  } catch {
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

// ---------- 项目加载 / 持久化 ----------

async function loadProjects() {
  if (!packageId.value) {
    projectSummaries.value = []
    return
  }
  projectsLoading.value = true
  try {
    const entries = await videoApi.listProjectEntries(packageId.value)
    projectSummaries.value = entries
      .map(entry => {
        const id = projectIdFromPath(entry.path)
        if (!id) return null
        try {
          const project = parseProject(entry.content)
          return {
            id,
            name: project.name,
            valid: true,
            markerCount: project.markers.length,
            assetCount: project.assets.length,
            entry,
          }
        } catch (error) {
          // 损坏/外部产生的项目文件：列表可见、可诊断、可删除，不可打开
          return { id, name: id, valid: false, markerCount: 0, assetCount: 0, entry, diagnostics: error.diagnostics }
        }
      })
      .filter(Boolean)
    // 打开中的项目被删除/重命名后回落空态
    if (openId.value && !projectSummaries.value.some(summary => summary.id === openId.value)) {
      closeOpenProject()
    }
  } catch {
    projectSummaries.value = []
  } finally {
    projectsLoading.value = false
  }
}

function closeOpenProject() {
  openId.value = ''
  openProject.value = null
  projectVersion.value = ''
  projectDirty.value = false
  staleSaveError.value = ''
}

async function openProjectById(id) {
  const summary = projectSummaries.value.find(item => item.id === id)
  if (!summary) return
  if (!summary.valid) {
    staleSaveError.value = '项目数据校验失败，无法打开（可删除后重建）'
    return
  }
  staleSaveError.value = ''
  try {
    // 打开前重读一次（拿最新 version；列表 content 可能已过期）
    const entry = await videoApi.getProject(packageId.value, id)
    const project = parseProject(entry.content)
    openId.value = id
    openProject.value = project
    projectVersion.value = entry.version
    projectDirty.value = false
    // 联动左侧舞台（主素材存在时）；只动舞台来源，不动设备/包身份
    if (assetStatus(project, mediaList.value).primary) {
      requestStageMedia(primaryAssetIdOf(project))
    }
  } catch (error) {
    staleSaveError.value = describe(error, '项目打开失败')
  }
}

function primaryAssetIdOf(project) {
  return (project.assets || []).find(asset => asset.role === 'primary')?.media_id || ''
}

function reloadOpenProject() {
  if (openId.value) void openProjectById(openId.value)
}

async function createProject({ id, name }) {
  const media = mediaList.value.find(item => item.id === selectedId.value)
  if (!packageId.value || !media) return
  const project = newProject({ id, name, packageId: packageId.value, media })
  const diagnostics = validateProject(project)
  if (diagnostics.length) {
    staleSaveError.value = `项目创建被拒绝：${diagnostics[0].message}`
    return
  }
  saving.value = true
  try {
    await videoApi.putProject(packageId.value, id, serializeProject(project))
    await loadProjects()
    await openProjectById(id)
  } catch (error) {
    staleSaveError.value = describe(error, '项目创建失败')
  } finally {
    saving.value = false
  }
}

async function renameProject(id, newId) {
  if (!packageId.value) return
  try {
    await videoApi.renameProject(packageId.value, id, newId)
    if (openId.value === id) {
      // 资源已原子移动：同步打开态指向新 id（内容不变）
      openId.value = newId
      if (openProject.value) openProject.value = { ...openProject.value, id: newId }
      projectDirty.value = true // 项目内 id 字段需随文件名更新后重存
    }
    await loadProjects()
  } catch (error) {
    staleSaveError.value = describe(error, '项目重命名失败')
  }
}

async function deleteProject(id) {
  if (!packageId.value) return
  try {
    await videoApi.deleteProject(packageId.value, id)
    if (openId.value === id) closeOpenProject()
    await loadProjects()
  } catch (error) {
    staleSaveError.value = describe(error, '项目删除失败')
  }
}

async function saveProject() {
  if (!openProject.value || !packageId.value || saving.value) return
  saving.value = true
  staleSaveError.value = ''
  try {
    const content = serializeProject(openProject.value)
    const entry = await videoApi.putProject(packageId.value, openProject.value.id, content, {
      expectedVersion: projectVersion.value,
    })
    projectVersion.value = entry.version
    projectDirty.value = false
    await loadProjects()
  } catch (error) {
    staleSaveError.value = isVersionConflict(error)
      ? '项目已被其他页面修改（保存冲突）：请重新加载后再编辑'
      : describe(error, '项目保存失败')
  } finally {
    saving.value = false
  }
}

/** 保存冲突判定：资源写路径版本冲突 = HTTP 409 + version_conflict 语义。 */
function isVersionConflict(error) {
  return error?.status === 409 && String(error?.code || '').includes('version_conflict')
}

// ---------- 编辑动作（改本地工作副本 + 标脏；显式保存持久化） ----------

function onAddMarker({ label, frame }) {
  if (!openProject.value) return
  openProject.value = withMarker(openProject.value, { label, frame })
  projectDirty.value = true
}

function onRemoveMarker(markerId) {
  if (!openProject.value) return
  openProject.value = withoutMarker(openProject.value, markerId)
  projectDirty.value = true
}

function onUpdateMarker(markerId, text) {
  if (!openProject.value) return
  openProject.value = withMarkerText(openProject.value, markerId, text)
  projectDirty.value = true
}

function onSaveCalibration(next) {
  if (!openProject.value) return
  const before = openProject.value
  openProject.value = withCalibration(before, next)
  if (openProject.value.calibration.version === before.calibration.version) {
    // 值未变化：不标脏（withCalibration 等值短路）
    return
  }
  projectDirty.value = true
}

function openDraft() {
  if (openProject.value?.recording?.recording_id) {
    recordingId.value = openProject.value.recording.recording_id
    activeTab.value = 'draft'
  }
}

function describe(error, fallback) {
  return `${fallback}：${error?.message || error}`
}

onMounted(() => {
  void refresh()
  void loadProjects()
})

// Package 切换（§38 Package-aware UI 自动联动）：项目属数据上下文，切换后
// 关闭打开态（跨包引用失效）并重拉列表。
watch(packageId, (next, prev) => {
  if (next === prev) return
  closeOpenProject()
  void loadProjects()
})
</script>

<style scoped>
.video-workbench { display: flex; flex: 1; min-height: 0; flex-direction: column; gap: 12px; overflow: auto; }
.workbench-tabs { display: flex; gap: 4px; flex-shrink: 0; border-bottom: 1px solid var(--border); padding-bottom: 6px; }
.tab-btn { border: 1px solid transparent; background: transparent; color: var(--text-2); font-size: 12px; padding: 4px 10px; border-radius: 6px; cursor: pointer; }
.tab-btn:hover { color: var(--text-0); }
.tab-btn.active { border-color: var(--border); background: var(--bg-2); color: var(--text-0); font-weight: 700; }
.zone-error { padding: 6px 8px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; line-height: 1.6; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.zone-note { color: var(--accent-2); font-size: 11px; }
.zone-empty { padding: 14px 10px; text-align: center; color: var(--text-2); font-size: 12px; }
.project-toolbar { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.project-title { color: var(--text-0); font-size: 13px; font-weight: 700; }
.project-state { color: var(--text-2); font-size: 10px; }
.project-state.dirty { color: var(--warning, #d9a13c); }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 2px 6px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mono { font-family: var(--mono); }
</style>
