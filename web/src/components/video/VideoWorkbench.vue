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
        :load-error="mediaLoadError"
        @select="onSelect"
        @refresh="refresh"
        @changed="refresh"
        @recording-finished="onRecordingFinished"
        @recording-selected="onRecordingSelected"
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
        :project="openProject"
        :media-list="mediaList"
        :selected-media-id="selectedId"
        :asset-busy="saving || mediaRefSyncing"
        @open="openProjectById"
        @create="createProject"
        @rename="renameProject"
        @rename-name="renameProjectName"
        @delete="deleteProject"
        @refresh="loadProjects"
        @view-asset="viewProjectAsset"
        @asset-change="onAssetChange"
        @asset-add="onAssetChange"
        @asset-remove="onAssetChange"
        @asset-primary="onAssetChange"
        @asset-replace="onAssetChange"
        @asset-relink="onAssetChange"
      />

      <div v-if="pendingProjectSwitch" class="switch-protect" role="dialog" aria-live="polite" data-testid="project-switch-protect">
        <div class="switch-protect-title">当前项目有未保存修改</div>
        <div class="switch-protect-text">请保留当前编辑继续工作，或放弃修改后切换项目。</div>
        <div class="switch-protect-actions">
          <button class="mini-btn" type="button" data-testid="project-switch-retain" @click="resolveProjectSwitch('retain')">保留当前项目</button>
          <button class="mini-btn danger-btn" type="button" data-testid="project-switch-discard" @click="resolveProjectSwitch('discard')">放弃并切换</button>
          <button class="mini-btn" type="button" data-testid="project-switch-cancel" @click="resolveProjectSwitch('cancel')">取消</button>
        </div>
      </div>

      <div v-if="pendingPackageSwitch" class="switch-protect" role="dialog" aria-live="polite" data-testid="package-switch-protect">
        <div class="switch-protect-title">当前项目有未保存修改</div>
        <div class="switch-protect-text">Package 切换已暂缓。放弃当前项目修改后才能切换到 {{ pendingPackageSwitch.to }}。</div>
        <div class="switch-protect-actions">
          <button class="mini-btn" type="button" data-testid="package-switch-retain" @click="resolvePackageSwitch('retain')">保留并取消切换</button>
          <button class="mini-btn danger-btn" type="button" data-testid="package-switch-discard" @click="resolvePackageSwitch('discard')">放弃并切换</button>
        </div>
      </div>

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
        <div v-else-if="mediaRefSyncState === 'failed'" class="zone-error" role="alert" data-testid="media-ref-sync-error">
          {{ mediaRefSyncError }}
          <button
            class="mini-btn"
            type="button"
            data-testid="media-ref-sync-retry"
            :disabled="mediaRefSyncing"
            @click="retryMediaRefs"
          >{{ mediaRefSyncing ? '同步中…' : '重试引用同步' }}</button>
        </div>

        <div class="project-toolbar" data-testid="project-toolbar">
          <span class="project-title" data-testid="open-project-name">{{ openProject.name }}</span>
          <span class="mono project-state" data-testid="project-dirty" :class="{ dirty: projectDirty }">
            {{ projectDirty ? '未保存改动' : '已保存' }}
          </span>
          <span v-if="projectSaveState === 'saved'" class="mono project-ref-state" data-testid="project-save-status">
            <template v-if="mediaRefSyncState === 'syncing'">项目已保存；媒体引用同步中…</template>
            <template v-else-if="mediaRefSyncState === 'complete'">项目已保存；媒体引用已同步</template>
            <template v-else-if="mediaRefSyncState === 'failed'">项目已保存；媒体引用同步失败</template>
            <template v-else>项目已保存</template>
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
          :yaml-ready="yamlReady"
          @add-marker="onAddMarker"
          @remove-marker="onRemoveMarker"
          @update-marker="onUpdateMarker"
          @save-calibration="onSaveCalibration"
          @create-template="openTemplateStudio"
        />
      </template>
      <div v-else class="zone-empty" data-testid="project-detail-empty">选择一个项目进行制作（打开项目会联动左侧画面来源）</div>
    </template>

    <template v-else>
      <VideoDraft
        :recording-id="draftRecordingId"
        :package-id="packageId"
        :device-id="draftDeviceId"
        :android-package-name="draftAndroidPackageName"
        :yaml-ready="yamlReady"
        @update:recording-id="onRecordingIdUpdate"
      />
    </template>

    <!-- 模板工作台弹窗（§10.2：确定帧 → 模板创建/离线测试；经 gamer.yaml 动作清单缝） -->
    <TemplateStudio
      :open="!!studio"
      :media="studio?.media || null"
      :frame="studio?.frame || null"
      :calibration="openProject?.calibration || null"
      :package-id="packageId"
      :yaml-ready="yamlReady"
      @close="studio = null"
      @saved="onStudioSaved"
    />
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
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import MediaLibrary from './MediaLibrary.vue'
import TemplateStudio from './TemplateStudio.vue'
import VideoDraft from './VideoDraft.vue'
import VideoProjects from './VideoProjects.vue'
import VideoTimeline from './VideoTimeline.vue'
import { requestStageMedia } from '../console/useConsoleStage'
import { api } from '../../api'
import { devicesData, store, templatesData } from '../../store'
import { packageStore, selectPackage } from '../../package-store'
import { GAMER_VIDEO_PLUGIN_ID } from '../../gamer-plugin-ids'
import { videoApi } from './videoApi'
import { useYamlCapability } from './yamlCapability'
import { assetStatus, newProject, parseProject, projectMediaIds, projectIdFromPath, serializeProject, validateProject, withCalibration, withMarker, withoutMarker, withMarkerText } from './videoProject'

const TABS = [
  { key: 'library', label: '素材库' },
  { key: 'projects', label: '项目' },
  { key: 'draft', label: '草稿' },
]

// 当前 Package（数据上下文，plan §39）：面板自取（registry 契约 = 自包含组件，
// 无宿主 context 注入）；切换 Package 后项目列表随 loadProjects 联动。
const packageId = computed(() => packageStore.currentPackageId)

// Device/App 是运行上下文，不从 Package 或项目资源 id 推导。录制来源若带有
// 真实 device_id，则优先沿用该设备；否则仅使用当前控制台设备的实际配置 pkg。
const currentDevice = computed(() => devicesData.value.find(device => (
  String(device?.id || '') === String(store.deviceId || '')
)) || null)
const currentDeviceId = computed(() => String(store.deviceId || '').trim())
const currentAndroidPackageName = computed(() => String(currentDevice.value?.pkg || '').trim())
const recordingContext = ref(null) // { recordingId, deviceId, androidPackageName }
const draftDeviceId = computed(() => {
  const source = recordingContext.value
  return source?.recordingId === recordingId.value && source.deviceId
    ? source.deviceId
    : currentDeviceId.value
})
const draftAndroidPackageName = computed(() => {
  const source = recordingContext.value
  return source?.recordingId === recordingId.value && source.androidPackageName
    ? source.androidPackageName
    : currentAndroidPackageName.value
})

const activeTab = ref('library')
const mediaList = ref([])
const loading = ref(false)
const mediaLoadError = ref('')
let mediaRequestSeq = 0
const selectedId = ref('')
const recordingId = ref('') // 最近一次录制会话（停止后自动带入草稿区）
// VideoDraft 的首次非空 prop 不会自动触发事件加载；草稿区切换时先以空
// 来源挂载，再在下一渲染周期交付真实 recording id，确保走其代次保护路径。
const draftRecordingId = ref('')
let draftSourceRevision = 0

// ---- 项目状态 ----
const projectsLoading = ref(false)
const projectSummaries = ref([]) // [{id, name, valid, markerCount, assetCount, entry}]
const openId = ref('')
const openProject = ref(null) // 已解析项目对象（编辑中的本地工作副本）
const projectVersion = ref('') // 打开/最近保存时的资源 version 短码（乐观并发）
const loadedMediaIds = ref([]) // 打开（或最近保存）时服务端内容的媒体引用快照（引用同步的 before 基准）
const projectDirty = ref(false)
const saving = ref(false)
const staleSaveError = ref('')
const projectSaveState = ref('idle') // idle | saving | saved | failed（只描述项目资源 PUT）
const mediaRefSyncState = ref('idle') // idle | syncing | complete | failed（独立于项目保存）
const mediaRefSyncError = ref('')
const mediaRefSyncing = ref(false)
const pendingMediaRefSync = ref(null)
const projectEditRevision = ref(0)
const pendingProjectSwitch = ref(null)
const pendingPackageSwitch = ref(null)
let projectContextRevision = 0
let projectRequestSeq = 0
let lastAssetEvent = null
let restoringPackage = false

// ---------- gamer.yaml 依赖门禁（§10.1）+ 模板工作台（§10.2） ----------
// gamer.yaml 未 Running：模板创建/离线测试/草稿生成保存禁用并提示依赖；
// 视频导入/录制/播放/标记/项目/校准不受影响。
const { ready: yamlReady, start: startYamlWatch, stop: stopYamlWatch } = useYamlCapability()
const studio = ref(null) // {media, frame:{frameIndex, ptsUs}}

function openTemplateStudio(frame) {
  if (!primaryMedia.value || !yamlReady.value) return
  studio.value = {
    media: primaryMedia.value,
    frame: { frameIndex: frame?.frameIndex ?? null, ptsUs: frame?.ptsUs ?? null },
  }
}

async function onStudioSaved(result) {
  // 模板已落当前 Package 的 gamer.yaml templates/：刷新共享模板缓存（Console
  // 模板页签/编辑器下拉共用 templatesData，与 useConsoleTemplates 同一 store）。
  try {
    templatesData.value = await api.listTemplates(packageId.value)
  } catch { /* 缓存刷新失败不影响保存结果 */ }
  void result
}

const openAsset = computed(() => (openProject.value
  ? (openProject.value.assets || []).find(asset => asset.role === 'primary') || null
  : null))
const primaryAssetId = computed(() => openAsset.value?.media_id || '')
const primaryMedia = computed(() => mediaList.value.find(media => media.id === primaryAssetId.value) || null)
const primaryMissing = computed(() => !!openProject.value && !primaryMedia.value)

// ---------- 素材库 ----------

async function refresh() {
  const requestSeq = ++mediaRequestSeq
  loading.value = true
  mediaLoadError.value = ''
  try {
    const next = await videoApi.listMedia()
    if (requestSeq !== mediaRequestSeq) return
    mediaList.value = Array.isArray(next) ? next : []
    // 选中项被删除后回落到空态，不自动跳选其它素材
    if (selectedId.value && !mediaList.value.some(media => media.id === selectedId.value)) {
      selectedId.value = ''
    }
  } catch (error) {
    if (requestSeq === mediaRequestSeq) {
      // 失败不是空列表：保留上一份可见数据，交给 MediaLibrary 展示可重试错误。
      mediaLoadError.value = describe(error, '素材读取失败')
    }
  } finally {
    if (requestSeq === mediaRequestSeq) loading.value = false
  }
}

function onSelect(id) {
  selectedId.value = id
}

function onRecordingFinished(meta) {
  const id = normalizeId(meta && typeof meta === 'object' ? meta.id : meta)
  if (!id) return
  setRecordingSource(id, meta, { openDraft: true })
  // 录制停止后先刷新素材列表；该调用不影响已冻结的录制来源。
  void refresh()
}

function onRecordingSelected(record) {
  const id = normalizeId(record?.id || record?.recordingId || record?.sessionId)
  // MediaLibrary 只有在后端确实返回 session id 时才发出该事件；不在这里
  // 根据 media_id 或名称拼造 recording id。
  if (!id) return
  setRecordingSource(id, record, { openDraft: true })
}

function setRecordingSource(id, meta = {}, { openDraft = false } = {}) {
  const previous = recordingContext.value?.recordingId === id ? recordingContext.value : null
  const deviceId = normalizeId(meta?.device_id || meta?.deviceId || previous?.deviceId)
  const device = devicesData.value.find(item => String(item?.id || '') === deviceId)
  recordingContext.value = {
    recordingId: id,
    deviceId,
    androidPackageName: normalizeId(device?.pkg || previous?.androidPackageName),
  }
  recordingId.value = id
  if (openDraft) {
    const revision = ++draftSourceRevision
    activeTab.value = 'draft'
    nextTick(() => {
      if (revision === draftSourceRevision && activeTab.value === 'draft') draftRecordingId.value = id
    })
  } else {
    draftRecordingId.value = id
  }
}

function onRecordingIdUpdate(value) {
  const next = normalizeId(value)
  recordingId.value = next
  draftRecordingId.value = next
  if (recordingContext.value?.recordingId !== next) recordingContext.value = null
}

// ---------- 项目加载 / 持久化 ----------

async function loadProjects(targetPackageId = packageId.value, { preserveOpen = false } = {}) {
  const scopePackageId = targetPackageId
  const requestSeq = ++projectRequestSeq
  if (!scopePackageId) {
    if (requestSeq === projectRequestSeq) projectSummaries.value = []
    return []
  }
  projectsLoading.value = true
  try {
    const entries = await videoApi.listProjectEntries(scopePackageId)
    const summaries = entries
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
            mediaIds: projectMediaIds(project),
            entry,
          }
        } catch (error) {
          // 损坏/外部产生的项目文件：列表可见、可诊断、可删除，不可打开
          return { id, name: id, valid: false, markerCount: 0, assetCount: 0, entry, diagnostics: error.diagnostics }
        }
      })
      .filter(Boolean)
    // 异步刷新可能属于已经切走的 Package；旧响应不能污染当前列表。
    if (requestSeq === projectRequestSeq && scopePackageId === packageId.value) {
      projectSummaries.value = summaries
      // 打开中的项目被删除/重命名后回落空态
      if (!preserveOpen && openId.value && !summaries.some(summary => summary.id === openId.value)) {
        closeOpenProject()
      }
    }
    return summaries
  } catch {
    // 刷新失败不应拿空列表重算引用，否则会错误解除其他项目的保护。
    return null
  } finally {
    if (requestSeq === projectRequestSeq) projectsLoading.value = false
  }
}

function closeOpenProject() {
  projectContextRevision += 1
  pendingProjectSwitch.value = null
  openId.value = ''
  openProject.value = null
  projectVersion.value = ''
  loadedMediaIds.value = []
  projectDirty.value = false
  staleSaveError.value = ''
  projectSaveState.value = 'idle'
  mediaRefSyncState.value = 'idle'
  mediaRefSyncError.value = ''
  mediaRefSyncing.value = false
  pendingMediaRefSync.value = null
  projectEditRevision.value = 0
}

async function openProjectById(id, { force = false } = {}) {
  if (!force && openId.value && openId.value !== id && projectDirty.value) {
    pendingProjectSwitch.value = { projectId: id }
    return
  }
  const summary = projectSummaries.value.find(item => item.id === id)
  if (!summary) return
  if (!summary.valid) {
    staleSaveError.value = '项目数据校验失败，无法打开（可删除后重建）'
    return
  }
  const scopePackageId = packageId.value
  const contextRevision = ++projectContextRevision
  staleSaveError.value = ''
  try {
    // 打开前重读一次（拿最新 version；列表 content 可能已过期）
    const entry = await videoApi.getProject(scopePackageId, id)
    const project = parseProject(entry.content)
    if (contextRevision !== projectContextRevision || scopePackageId !== packageId.value) return
    openId.value = id
    openProject.value = project
    projectVersion.value = entry.version
    loadedMediaIds.value = projectMediaIds(project)
    projectDirty.value = false
    projectEditRevision.value = 0
    projectSaveState.value = 'idle'
    mediaRefSyncState.value = 'idle'
    mediaRefSyncError.value = ''
    mediaRefSyncing.value = false
    pendingMediaRefSync.value = null
    // 联动左侧舞台（主素材存在时）；只动舞台来源，不动设备/包身份
    if (assetStatus(project, mediaList.value).primary) {
      requestStageMedia(primaryAssetIdOf(project))
    }
  } catch (error) {
    if (contextRevision !== projectContextRevision || scopePackageId !== packageId.value) return
    staleSaveError.value = describe(error, '项目打开失败')
  }
}

function resolveProjectSwitch(action) {
  const pending = pendingProjectSwitch.value
  if (!pending) return
  pendingProjectSwitch.value = null
  if (action === 'discard') void openProjectById(pending.projectId, { force: true })
}

function primaryAssetIdOf(project) {
  return (project.assets || []).find(asset => asset.role === 'primary')?.media_id || ''
}

function reloadOpenProject() {
  if (openId.value) void openProjectById(openId.value)
}

function viewProjectAsset(mediaId) {
  const id = normalizeId(mediaId)
  if (!id) return
  selectedId.value = id
  if (mediaList.value.some(media => String(media.id) === id)) requestStageMedia(id)
}

/**
 * VideoProjects 的所有素材动作都先产生本地项目副本，再由这里进入与标记/
 * 校准相同的显式保存流。组件同时发出 asset-change 和 operation-specific
 * 事件；用同一 detail 对象去重，避免一次点击重复标脏或重复提交。
 */
function onAssetChange(detail) {
  if (!detail || detail === lastAssetEvent) return
  lastAssetEvent = detail
  queueMicrotask(() => {
    if (lastAssetEvent === detail) lastAssetEvent = null
  })
  const next = detail.project
  if (!openProject.value || !next || next.id !== openId.value || next.package_id !== packageId.value) return
  const diagnostics = validateProject(next)
  if (diagnostics.length) {
    staleSaveError.value = `项目素材调整被拒绝：${diagnostics[0].message}`
    return
  }
  openProject.value = next
  projectEditRevision.value += 1
  projectDirty.value = true
  staleSaveError.value = ''
  projectSaveState.value = 'idle'
  mediaRefSyncState.value = 'idle'
  mediaRefSyncError.value = ''
  pendingMediaRefSync.value = null

  const operation = String(detail.operation || '')
  if (['primary', 'replace', 'relink'].includes(operation)) {
    const primaryId = primaryAssetIdOf(next)
    if (primaryId && mediaList.value.some(media => String(media.id) === primaryId)) {
      selectedId.value = primaryId
      requestStageMedia(primaryId)
    }
  }
}

function renameProjectName({ id, name } = {}) {
  if (!openProject.value || id !== openId.value) return
  const nextName = String(name || '').trim()
  if (!nextName) return
  const next = {
    ...openProject.value,
    name: nextName,
    updated_at: new Date().toISOString(),
  }
  const diagnostics = validateProject(next)
  if (diagnostics.length) {
    staleSaveError.value = `项目名称修改被拒绝：${diagnostics[0].message}`
    return
  }
  openProject.value = next
  projectEditRevision.value += 1
  projectDirty.value = true
  staleSaveError.value = ''
  projectSaveState.value = 'idle'
  mediaRefSyncState.value = 'idle'
  mediaRefSyncError.value = ''
  pendingMediaRefSync.value = null
}

async function createProject({ id, name }) {
  const media = mediaList.value.find(item => item.id === selectedId.value)
  if (!packageId.value || !media) return
  const scopePackageId = packageId.value
  const project = newProject({ id, name, packageId: packageId.value, media })
  const submittedMediaIds = projectMediaIds(project)
  const diagnostics = validateProject(project)
  if (diagnostics.length) {
    staleSaveError.value = `项目创建被拒绝：${diagnostics[0].message}`
    return
  }
  saving.value = true
  staleSaveError.value = ''
  const createContextRevision = projectContextRevision
  const fallbackSummaries = projectSummaries.value
  try {
    const entry = await videoApi.putProject(scopePackageId, id, serializeProject(project))
    const committedAfter = mediaIdsFromPutResponse(entry, submittedMediaIds)
    // 先重读成功创建后的项目集合；同步任务本身带固定 Package 作用域，即使
    // 创建完成时用户已经切换了 Package，也不能把引用算到新上下文。
    const summaries = await loadProjects(scopePackageId, { preserveOpen: true })
    const pending = createMediaRefSync(
      scopePackageId,
      id,
      [],
      committedAfter,
      createContextRevision,
    )
    const result = await runMediaRefSync(pending, summaries || fallbackSummaries)
    if (packageId.value !== scopePackageId || !summaries?.some(summary => summary.id === id)) return
    await openProjectById(id)
    if (openProject.value?.id === id && packageId.value === scopePackageId) {
      projectSaveState.value = 'saved'
      mediaRefSyncState.value = result.ok ? 'complete' : 'failed'
      mediaRefSyncError.value = ''
      const uiPending = { ...pending, contextRevision: projectContextRevision }
      pendingMediaRefSync.value = result.ok ? null : uiPending
      if (!result.ok) applyMediaRefSyncResult(uiPending, result)
    }
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
  const scopePackageId = packageId.value
  // 删除前记下被删项目引用的素材：删除后解除其 gamer.video/project 引用
  //（全量替换语义：按剩余项目并集重算，不再被引用的素材解除删除保护）
  const summary = projectSummaries.value.find(item => item.id === id)
  const before = [...(summary?.mediaIds || [])]
  try {
    await videoApi.deleteProject(scopePackageId, id)
    if (openId.value === id) closeOpenProject()
    const summaries = await loadProjects(scopePackageId, { preserveOpen: true })
    if (before.length) {
      const pending = createMediaRefSync(scopePackageId, id, before, [], projectContextRevision)
      const result = await runMediaRefSync(pending, summaries || projectSummaries.value)
      // 删除后的项目没有可供重试的详情面板；仍完成同步并保留其它 Package/plugin
      // 引用，失败只在仍处于同一打开项目上下文时显示。
      if (!result.ok && openId.value && packageId.value === scopePackageId) {
        staleSaveError.value = `项目已删除，但媒体引用同步失败：${result.failures[0]?.error?.message || result.failures[0]?.error || '未知错误'}`
      }
    }
  } catch (error) {
    staleSaveError.value = describe(error, '项目删除失败')
  }
}

async function saveProject() {
  if (!openProject.value || !packageId.value || saving.value) return
  const scopePackageId = packageId.value
  const scopeProjectId = openProject.value.id
  const contextRevision = projectContextRevision
  const editRevision = projectEditRevision.value
  const before = [...loadedMediaIds.value]
  const content = serializeProject(openProject.value)
  const submittedAfter = projectMediaIds(openProject.value)
  const fallbackSummaries = projectSummaries.value

  saving.value = true
  staleSaveError.value = ''
  projectSaveState.value = 'saving'
  mediaRefSyncState.value = 'idle'
  mediaRefSyncError.value = ''
  pendingMediaRefSync.value = null
  try {
    const entry = await videoApi.putProject(scopePackageId, scopeProjectId, content, {
      expectedVersion: projectVersion.value,
    })
    // 只有 PUT 成功才推进项目版本和引用同步基准。若保存期间用户继续编辑，
    // 保留新的本地 dirty 状态，但待同步内容仍是本次已成功提交的快照。
    const currentContext = contextRevision === projectContextRevision
      && packageId.value === scopePackageId && openId.value === scopeProjectId
    const after = mediaIdsFromPutResponse(entry, submittedAfter)
    if (currentContext) {
      projectVersion.value = entry.version
      projectDirty.value = projectEditRevision.value !== editRevision
      projectSaveState.value = 'saved'
      loadedMediaIds.value = [...after]
    }
    const pending = createMediaRefSync(scopePackageId, scopeProjectId, before, after, contextRevision)
    if (currentContext) {
      pendingMediaRefSync.value = pending
      mediaRefSyncState.value = 'syncing'
    }

    // 先读取 PUT 成功后的 Package 项目集合，再计算并集；当前提交的项目始终
    // 由 afterIds 覆盖，避免列表响应短暂过期把旧引用带回去。
    const summaries = await loadProjects(scopePackageId, { preserveOpen: true })
    const result = await runMediaRefSync(pending, summaries || fallbackSummaries)
    if (contextRevision === projectContextRevision && packageId.value === scopePackageId && openId.value === scopeProjectId) {
      applyMediaRefSyncResult(pending, result)
    }
  } catch (error) {
    if (contextRevision === projectContextRevision && packageId.value === scopePackageId && openId.value === scopeProjectId) {
      projectSaveState.value = 'failed'
      staleSaveError.value = isVersionConflict(error)
        ? '项目已被其他页面修改（保存冲突）：请重新加载后再编辑'
        : describe(error, '项目保存失败')
    }
  } finally {
    saving.value = false
  }
}

/**
 * 媒体引用全量替换同步（Phase 8 契约 §2.1）：对「本次成功提交前后引用的媒体」
 * 逐一按指定 Package 内**全部项目**的引用并集重算 gamer.video/project 引用，
 * 经 POST /api/media/:id/refs 全量替换（其余包/插件条目保留，多项目共享同一
 * 素材时不互踩）。素材已删除（404）静默跳过；其他失败返回给调用方，由独立
 * 重试按钮重放，不要求重新修改或再次 PUT 项目。
 */
function createMediaRefSync(scopePackageId, scopeProjectId, beforeIds, afterIds, contextRevision) {
  return {
    packageId: scopePackageId,
    projectId: scopeProjectId,
    beforeIds: [...new Set(beforeIds || [])],
    afterIds: [...new Set(afterIds || [])],
    contextRevision,
  }
}

/**
 * PUT 成功响应若带回资源正文，以服务端实际提交的正文作为引用状态来源；
 * 测试桩/旧响应没有正文时回退到本次送出的已校验快照。JSON 仍由视频扩展
 * 的前端模型解析，Core 只承载不透明资源文本。
 */
function mediaIdsFromPutResponse(entry, fallback) {
  if (typeof entry?.content !== 'string') return [...fallback]
  try {
    return projectMediaIds(parseProject(entry.content))
  } catch {
    return [...fallback]
  }
}

function committedMediaIds(summaries, projectId, afterIds) {
  const referenced = new Set()
  for (const summary of Array.isArray(summaries) ? summaries : []) {
    // 即便 listProjectEntries 返回的是保存前缓存，也不能把该项目旧素材算回去。
    if (summary.id === projectId) continue
    for (const mediaId of summary.mediaIds || []) referenced.add(String(mediaId))
  }
  for (const mediaId of afterIds || []) referenced.add(String(mediaId))
  return referenced
}

async function runMediaRefSync(pending, summaries) {
  if (!pending) return { ok: false, failures: [{ error: new Error('媒体引用同步上下文缺失') }] }
  const currentContext = pending.contextRevision === projectContextRevision
    && packageId.value === pending.packageId
    && openId.value === pending.projectId
  if (currentContext && mediaRefSyncing.value) {
    return { ok: false, failures: [{ error: new Error('媒体引用同步已在进行中') }] }
  }
  const affected = [...new Set([...pending.beforeIds, ...pending.afterIds])]
  const referencedByPackage = committedMediaIds(summaries, pending.projectId, pending.afterIds)
  const failures = []
  if (currentContext) mediaRefSyncing.value = true
  try {
    for (const mediaId of affected) {
      try {
        const meta = await videoApi.getMedia(mediaId)
        const existing = Array.isArray(meta?.refs) ? meta.refs : []
        const kept = existing.filter(entry => !(
          entry.plugin_id === GAMER_VIDEO_PLUGIN_ID && entry.kind === 'project' && entry.package_id === pending.packageId
        ))
        const next = referencedByPackage.has(mediaId)
          ? [...kept, { package_id: pending.packageId, plugin_id: GAMER_VIDEO_PLUGIN_ID, kind: 'project' }]
          : kept
        const same = next.length === existing.length && next.every(entry => existing.some(other =>
          other.package_id === entry.package_id && other.plugin_id === entry.plugin_id && other.kind === entry.kind))
        if (!same) await videoApi.setMediaRefs(mediaId, next)
      } catch (e) {
        if (e?.status !== 404) failures.push({ mediaId, error: e })
      }
    }
  } finally {
    if (pending.contextRevision === projectContextRevision
      && packageId.value === pending.packageId && openId.value === pending.projectId) {
      mediaRefSyncing.value = false
    }
  }
  return { ok: failures.length === 0, failures }
}

function applyMediaRefSyncResult(pending, result) {
  if (result?.ok) {
    mediaRefSyncState.value = 'complete'
    mediaRefSyncError.value = ''
    pendingMediaRefSync.value = null
    return
  }
  mediaRefSyncState.value = 'failed'
  const first = result?.failures?.[0]
  const suffix = first?.mediaId ? `（${first.mediaId}）` : ''
  mediaRefSyncError.value = `项目已保存，但媒体引用同步失败${suffix}：${first?.error?.message || first?.error || '未知错误'}（可直接重试）`
  pendingMediaRefSync.value = pending
}

async function retryMediaRefs() {
  const pending = pendingMediaRefSync.value
  if (!pending || mediaRefSyncing.value || !openProject.value || packageId.value !== pending.packageId
    || openId.value !== pending.projectId || pending.contextRevision !== projectContextRevision) return
  mediaRefSyncState.value = 'syncing'
  mediaRefSyncError.value = ''
  const summaries = await loadProjects(pending.packageId, { preserveOpen: true })
  const result = await runMediaRefSync(pending, summaries || projectSummaries.value)
  if (pending.contextRevision === projectContextRevision && packageId.value === pending.packageId && openId.value === pending.projectId) {
    applyMediaRefSyncResult(pending, result)
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
  projectEditRevision.value += 1
  projectDirty.value = true
}

function onRemoveMarker(markerId) {
  if (!openProject.value) return
  openProject.value = withoutMarker(openProject.value, markerId)
  projectEditRevision.value += 1
  projectDirty.value = true
}

function onUpdateMarker(markerId, text) {
  if (!openProject.value) return
  openProject.value = withMarkerText(openProject.value, markerId, text)
  projectEditRevision.value += 1
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
  projectEditRevision.value += 1
  projectDirty.value = true
}

function openDraft() {
  if (openProject.value?.recording?.recording_id) {
    setRecordingSource(
      normalizeId(openProject.value.recording.recording_id),
      { device_id: openProject.value.recording.device_id },
    )
    activeTab.value = 'draft'
  }
}

function describe(error, fallback) {
  return `${fallback}：${error?.message || error}`
}

onMounted(() => {
  void refresh()
  void loadProjects()
  startYamlWatch()
})

onUnmounted(() => {
  stopYamlWatch()
})

// Package 切换（§38 Package-aware UI 自动联动）：项目属数据上下文，切换后
// 关闭打开态（跨包引用失效）并重拉列表。
watch(packageId, (next, prev) => {
  if (next === prev) return
  if (restoringPackage) {
    restoringPackage = false
    return
  }
  if (openProject.value && projectDirty.value) {
    pendingPackageSwitch.value = { from: prev || '', to: next || '' }
    // packageStore 已先更新；回滚当前选择，使旧项目和它的保存作用域保持
    // 一致，待用户明确放弃后再切换。这样不会把旧项目写入新 Package。
    restoringPackage = true
    if (prev && !packageStore.packages.some(item => item.id === prev)) {
      // 仅用于容忍测试/宿主尚未完成包列表加载的瞬态；正常 Package 选择
      // 仍统一走 selectPackage 的校验与持久化路径。
      packageStore.currentPackageId = prev
    } else {
      selectPackage(prev || null)
    }
    return
  }
  pendingPackageSwitch.value = null
  closeOpenProject()
  void loadProjects()
}, { flush: 'sync' })

function resolvePackageSwitch(action) {
  const pending = pendingPackageSwitch.value
  if (!pending) return
  pendingPackageSwitch.value = null
  if (action !== 'discard') return
  // 当前作用域仍是 from；关闭本地工作副本后，显式选择目标 Package。watch
  // 会负责清理项目态并按目标作用域重新加载列表。
  closeOpenProject()
  selectPackage(pending.to || null)
}

function normalizeId(value) {
  return String(value || '').trim()
}
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
.project-ref-state { color: var(--accent-2); font-size: 10px; }
.switch-protect { padding: 7px 8px; border: 1px solid rgba(251,191,36,.45); border-radius: var(--radius-sm); background: rgba(251,191,36,.08); }
.switch-protect-title { color: var(--warn); font-size: 11px; font-weight: 700; }
.switch-protect-text { margin-top: 3px; color: var(--text-1); font-size: 10px; line-height: 1.45; }
.switch-protect-actions { display: flex; justify-content: flex-end; gap: 4px; margin-top: 6px; }
.danger-btn { border-color: rgba(248,113,113,.45); color: var(--danger); }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 2px 6px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mono { font-family: var(--mono); }
</style>
