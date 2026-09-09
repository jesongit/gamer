<template>
  <section class="video-projects" data-testid="video-projects">
    <div class="zone-head">
      <span class="zone-title">制作项目</span>
      <span class="zone-actions">
        <button class="btn btn-sm" type="button" :disabled="loading" data-testid="projects-refresh" @click="$emit('refresh')">↻ 刷新</button>
        <button
          class="btn btn-sm btn-primary"
          type="button"
          :disabled="!canCreate || creating"
          :title="canCreate ? '基于素材库当前选中素材新建项目' : '先在素材库选择一个素材作为项目主素材'"
          data-testid="project-create"
          @click="beginCreate"
        >＋ 新建项目</button>
      </span>
    </div>

    <!-- 新建表单：id + 名称（基于选中素材） -->
    <div v-if="creating" class="inline-form" data-testid="project-create-form">
      <label class="form-row">
        <span class="form-label">项目 ID</span>
        <input
          v-model="newId"
          class="input"
          type="text"
          placeholder="小写字母/数字/._-（如 daily-login）"
          data-testid="project-create-id"
        />
      </label>
      <label class="form-row">
        <span class="form-label">名称</span>
        <input v-model="newName" class="input" type="text" placeholder="项目显示名" data-testid="project-create-name" />
      </label>
      <div class="form-actions">
        <button class="btn btn-sm btn-primary" type="button" :disabled="!createReady" data-testid="project-create-confirm" @click="confirmCreate">创建</button>
        <button class="btn btn-sm" type="button" data-testid="project-create-cancel" @click="creating = false">取消</button>
      </div>
      <div v-if="createError" class="zone-error" role="alert">{{ createError }}</div>
    </div>

    <div class="project-list" data-testid="project-list">
      <div v-if="loading && !projects.length" class="list-empty">读取中…</div>
      <div v-else-if="!projects.length" class="list-empty">当前 Package 还没有制作项目：选一个素材后「新建项目」</div>
      <div
        v-for="project in projects"
        :key="project.id"
        class="project-row"
        :class="{ selected: project.id === openId, invalid: !project.valid }"
        data-testid="project-row"
        @click="$emit('open', project.id)"
      >
        <span class="project-main">
          <span class="project-name" :title="project.id">{{ project.valid ? project.name : '(损坏的项目)' }}</span>
          <span class="project-id mono">资源 ID：{{ project.id }}</span>
          <span class="project-meta mono">
            {{ project.markerCount }} 标记 · {{ project.assetCount }} 素材
            <template v-if="!project.valid"> · 校验失败</template>
          </span>
        </span>
        <span class="row-actions" @click.stop>
          <button class="mini-btn" type="button" data-testid="project-rename-name" @click="beginRename(project, 'name')">改名称</button>
          <button class="mini-btn" type="button" data-testid="project-rename" @click="beginRename(project, 'id')">改资源 ID</button>
          <button
            class="mini-btn danger"
            type="button"
            :class="{ armed: armedId === project.id }"
            data-testid="project-delete"
            @click="remove(project)"
          >{{ armedId === project.id ? '确认删除' : '删除' }}</button>
        </span>
        <!-- 名称与资源 ID 分开：名称只改 JSON name；ID 才触发资源 rename。 -->
        <span v-if="renamingId === project.id" class="rename-box" @click.stop>
          <input
            v-model="renameValue"
            class="input rename-input"
            type="text"
            :placeholder="renameMode === 'name' ? '项目显示名称' : '新项目资源 ID'"
            data-testid="project-rename-input"
          />
          <button class="mini-btn" type="button" :disabled="!renameReady" data-testid="project-rename-confirm" @click="confirmRename(project)">确定</button>
          <button class="mini-btn" type="button" data-testid="project-rename-cancel" @click="cancelRename">取消</button>
        </span>
      </div>
    </div>

    <!--
      项目素材操作面板是可选接线：宿主传入 project/media-list 后启用。
      这里仅修改本地副本并上抛意图，保存必须由宿主复用 expected_version PUT
      与媒体 refs 全量同步；不会在组件内绕过安全路径直接写 API。
    -->
    <div v-if="project" class="project-assets" data-testid="project-assets">
      <div class="asset-head">
        <span class="zone-title">项目素材</span>
        <span class="project-meta">主素材 + 附加素材</span>
      </div>

      <div v-if="assetOperationError" class="zone-error" role="alert" data-testid="asset-operation-error">
        {{ assetOperationError }}
      </div>
      <div v-if="assetImpactNotice" class="zone-warning" role="status" data-testid="asset-impact-notice">
        {{ assetImpactNotice }}
      </div>

      <div v-for="asset in draftProject.assets" :key="asset.media_id" class="asset-row" :class="{ missing: isMissing(asset) }" data-testid="project-asset-row">
        <span class="asset-main">
          <span class="asset-title">
            <span>{{ mediaName(asset.media_id) }}</span>
            <span v-if="asset.role === 'primary'" class="asset-role">主素材</span>
            <span v-else class="asset-role">附加素材</span>
            <span v-if="isMissing(asset)" class="asset-missing">缺失</span>
          </span>
          <span class="asset-id mono">media_id：{{ asset.media_id }}</span>
        </span>
        <span class="asset-actions">
          <button class="mini-btn" type="button" data-testid="asset-view" @click="viewAsset(asset)">查看</button>
          <button v-if="asset.role !== 'primary'" class="mini-btn" type="button" :disabled="assetBusy" data-testid="asset-set-primary" @click="setPrimary(asset)">设为主素材</button>
          <button v-if="asset.role !== 'primary'" class="mini-btn danger" type="button" :disabled="assetBusy" data-testid="asset-remove" @click="removeAsset(asset)">移除</button>
          <button class="mini-btn" type="button" :disabled="assetBusy || !targetMedia || targetMedia.id === asset.media_id" data-testid="asset-replace" @click="replaceAsset(asset)">替换为所选</button>
          <button v-if="isMissing(asset)" class="mini-btn" type="button" :disabled="assetBusy || !targetMedia || targetMedia.id === asset.media_id" data-testid="asset-relink" @click="relinkAsset(asset)">明确重关联</button>
        </span>
      </div>

      <div class="asset-picker" data-testid="asset-picker">
        <label class="asset-picker-label" for="video-project-asset-target">选择目标素材</label>
        <select id="video-project-asset-target" v-model="assetTargetId" class="input asset-select" data-testid="asset-target">
          <option value="">请选择媒体库素材…</option>
          <option v-for="media in mediaList" :key="media.id" :value="media.id">{{ mediaName(media.id) }}（{{ media.id }}）</option>
        </select>
        <button class="mini-btn" type="button" :disabled="assetBusy || !targetMedia" data-testid="asset-add" @click="addAsset">添加附加素材</button>
      </div>
      <div v-if="assetStatusValue.missingAssets.length" class="asset-help" data-testid="asset-relink-help">
        缺失素材只允许选择并校验同一 sha256 后重关联；无法确认同一文件时请使用“替换为所选”，替换会使相关制作信息失效并需重新验证。
      </div>
    </div>
  </section>
</template>

<script setup>
// 项目列表区（Phase 6 §9.1）：项目 CRUD 的展示层——创建（基于素材库选中素材）、
// 打开、重命名（资源 rename 原子移动）、删除（不自动删原视频）。持久化动作由
// 宿主 VideoWorkbench 承担（本组件只上抛意图）。
import { computed, ref, watch } from 'vue'
import {
  assetIdentityStatus,
  assetStatus,
  isValidProjectId,
  relinkProjectAsset,
  replaceProjectAsset,
  withPrimaryProjectAsset,
  withProjectAsset,
  withoutProjectAsset,
} from './videoProject'

const props = defineProps({
  /** 项目摘要列表：{id, name, valid, markerCount, assetCount} */
  projects: { type: Array, default: () => [] },
  openId: { type: String, default: '' },
  loading: { type: Boolean, default: false },
  /** 是否具备新建条件（有当前 Package + 素材库已选中主素材）。 */
  canCreate: { type: Boolean, default: false },
  /** 打开的完整项目；未传入时仅渲染项目列表（兼容当前父组件接线）。 */
  project: { type: Object, default: null },
  /** 项目素材操作使用的媒体库列表。名称只用于显示，不写入项目 JSON。 */
  mediaList: { type: Array, default: () => [] },
  /** 可选：媒体库当前选中项，作为操作目标的初始值。 */
  selectedMediaId: { type: String, default: '' },
  /** 宿主正在走安全保存/ref 同步时锁定操作。 */
  assetBusy: { type: Boolean, default: false },
})
const emit = defineEmits([
  'open', 'create', 'rename', 'rename-name', 'delete', 'refresh', 'view-asset',
  'asset-change', 'asset-add', 'asset-remove', 'asset-primary', 'asset-replace', 'asset-relink',
])

const creating = ref(false)
const newId = ref('')
const newName = ref('')
const createError = ref('')
const renamingId = ref('')
const renameMode = ref('id')
const renameValue = ref('')
const armedId = ref('')
const draftProject = ref(null)
const assetTargetId = ref('')
const assetOperationError = ref('')
const assetImpactNotice = ref('')

const createReady = computed(() => isValidProjectId(newId.value) && !!newName.value.trim())
const renameReady = computed(() => {
  const value = renameValue.value.trim()
  if (renameMode.value === 'name') return !!value
  return isValidProjectId(value) && value !== renamingId.value
})
const assetStatusValue = computed(() => assetStatus(draftProject.value || props.project || {}, props.mediaList))
const targetMedia = computed(() => props.mediaList.find(media => String(media.id) === String(assetTargetId.value)) || null)

function cloneProject(project) {
  return project ? JSON.parse(JSON.stringify(project)) : null
}

watch(() => props.project, (project) => {
  draftProject.value = cloneProject(project)
  assetOperationError.value = ''
  assetImpactNotice.value = ''
}, { immediate: true, deep: true })

watch(() => props.selectedMediaId, (id) => {
  if (id && props.mediaList.some(media => String(media.id) === String(id))) assetTargetId.value = String(id)
}, { immediate: true })

function beginCreate() {
  if (!props.canCreate || creating.value) return
  creating.value = true
  createError.value = ''
  newId.value = ''
  newName.value = ''
}

function confirmCreate() {
  if (!createReady.value) return
  if (props.projects.some(project => project.id === newId.value)) {
    createError.value = '已存在同名 id 的项目'
    return
  }
  createError.value = ''
  emit('create', { id: newId.value, name: newName.value.trim() })
  creating.value = false
}

function beginRenameMode(project, mode) {
  renamingId.value = renamingId.value === project.id && renameMode.value === mode ? '' : project.id
  renameMode.value = mode
  renameValue.value = mode === 'name' ? String(project.name || '') : ''
}

function beginRename(project, mode = 'id') {
  beginRenameMode(project, mode)
}

function cancelRename() {
  renamingId.value = ''
  renameValue.value = ''
}

function confirmRename(project) {
  const value = renameValue.value.trim()
  if (!renameReady.value) return
  if (renameMode.value === 'name') {
    emit('rename-name', { id: project.id, name: value })
  } else if (props.projects.some(item => item.id === value && item.id !== project.id)) {
    assetOperationError.value = '已存在同名资源 ID 的项目'
    return
  } else {
    emit('rename', project.id, value)
  }
  cancelRename()
}

function remove(project) {
  if (armedId.value !== project.id) {
    armedId.value = project.id
    return
  }
  armedId.value = ''
  emit('delete', project.id)
}

function mediaById(mediaId) {
  return props.mediaList.find(media => String(media.id) === String(mediaId)) || null
}

function mediaName(mediaId) {
  const media = mediaById(mediaId)
  return String(media?.name || media?.filename || media?.original_name || mediaId)
}

function isMissing(asset) {
  return !mediaById(asset.media_id)
}

function viewAsset(asset) {
  emit('view-asset', asset.media_id)
}

function commitAssetChange(operation, nextProject, payload = {}) {
  draftProject.value = nextProject
  assetOperationError.value = ''
  assetImpactNotice.value = payload.impact || ''
  const detail = { operation, project: nextProject, ...payload }
  emit('asset-change', detail)
  emit(`asset-${operation}`, detail)
}

function runAssetOperation(operation, action, payload = {}) {
  if (!draftProject.value || props.assetBusy) return
  try {
    commitAssetChange(operation, action(), payload)
  } catch (error) {
    assetOperationError.value = error?.diagnostics?.[0]?.message || error?.message || '项目素材操作失败'
    assetImpactNotice.value = ''
  }
}

function addAsset() {
  if (!targetMedia.value) return
  runAssetOperation('add', () => withProjectAsset(draftProject.value, targetMedia.value), { media: targetMedia.value })
}

function removeAsset(asset) {
  runAssetOperation('remove', () => withoutProjectAsset(draftProject.value, asset.media_id), { mediaId: asset.media_id })
}

function setPrimary(asset) {
  const media = mediaById(asset.media_id)
  runAssetOperation('primary', () => withPrimaryProjectAsset(draftProject.value, asset.media_id, { media }), {
    mediaId: asset.media_id,
    media,
    impact: '已切换主素材；原有标记与录制事件已失效，需要重新制作或重新验证。',
  })
}

function replaceAsset(asset) {
  if (!targetMedia.value) return
  runAssetOperation('replace', () => replaceProjectAsset(draftProject.value, asset.media_id, targetMedia.value), {
    mediaId: asset.media_id,
    media: targetMedia.value,
    impact: '已替换为另一段视频；相关标记、校准、录制事件和模板含义不得沿用，请重新验证。',
  })
}

function relinkAsset(asset) {
  if (!targetMedia.value) return
  const identity = assetIdentityStatus(asset, targetMedia.value)
  if (identity !== 'match') {
    assetOperationError.value = identity === 'mismatch'
      ? 'sha256 不一致，不能按同一素材重关联；请使用“替换为所选”'
      : '缺少可验证的 sha256，不能确认同一素材；请使用“替换为所选”'
    return
  }
  runAssetOperation('relink', () => relinkProjectAsset(draftProject.value, asset.media_id, targetMedia.value), {
    mediaId: asset.media_id,
    media: targetMedia.value,
    impact: '已按匹配的 sha256 明确重关联，帧身份仅改写 media_id，不改变制作含义。',
  })
}
</script>

<style scoped>
.video-projects { display: flex; flex-direction: column; gap: 8px; min-height: 0; }
.zone-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; flex-shrink: 0; }
.zone-title { color: var(--text-0); font-size: 13px; font-weight: 700; }
.zone-actions { display: flex; gap: 6px; align-items: center; }
.inline-form { display: flex; flex-direction: column; gap: 6px; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); }
.form-row { display: flex; align-items: center; gap: 6px; font-size: 11px; }
.form-label { color: var(--text-2); white-space: nowrap; }
.input { flex: 1; min-width: 0; padding: 4px 7px; font-size: 12px; border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); }
.form-actions { display: flex; gap: 6px; }
.project-list { border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden auto; max-height: 180px; flex-shrink: 0; }
.project-row { position: relative; display: flex; align-items: center; justify-content: space-between; gap: 6px; padding: 5px 8px; font-size: 11px; border-bottom: 1px solid rgba(80,92,119,.25); cursor: pointer; min-height: 28px; flex-wrap: wrap; }
.project-row:last-child { border-bottom: 0; }
.project-row:hover, .project-row.selected { background: var(--bg-3); }
.project-row.selected { box-shadow: inset 2px 0 var(--accent); }
.project-row.invalid { opacity: .75; }
.project-main { display: flex; flex-direction: column; gap: 1px; min-width: 0; flex: 1; }
.project-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-0); font-weight: 600; }
.project-id { color: var(--text-2); font-size: 10px; }
.project-meta { color: var(--text-2); font-size: 10px; }
.row-actions { display: flex; gap: 4px; flex-shrink: 0; }
.rename-box { display: flex; gap: 4px; width: 100%; align-items: center; }
.rename-input { max-width: 180px; }
.list-empty { padding: 16px 10px; text-align: center; color: var(--text-2); font-size: 12px; }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 2px 6px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mini-btn.danger:hover, .mini-btn.danger.armed { border-color: var(--danger); color: var(--danger); }
.zone-error { padding: 5px 7px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; }
.zone-warning { padding: 5px 7px; border: 1px solid rgba(217,161,60,.35); border-radius: var(--radius-sm); background: rgba(217,161,60,.08); color: var(--warning, #d9a13c); font-size: 11px; }
.project-assets { display: flex; flex-direction: column; gap: 7px; padding-top: 4px; }
.asset-head { display: flex; align-items: baseline; justify-content: space-between; gap: 8px; }
.asset-row { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding: 7px 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-1); }
.asset-row.missing { border-color: rgba(248,113,113,.45); }
.asset-main { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
.asset-title { display: flex; gap: 5px; align-items: center; min-width: 0; color: var(--text-0); font-size: 11px; font-weight: 600; }
.asset-title > span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.asset-role, .asset-missing { flex-shrink: 0; padding: 1px 4px; border-radius: 3px; font-size: 10px; color: var(--text-2); background: var(--bg-3); }
.asset-missing { color: var(--danger); background: rgba(248,113,113,.1); }
.asset-id { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-2); font-size: 10px; }
.asset-actions { display: flex; flex-wrap: wrap; gap: 4px; justify-content: flex-end; }
.asset-picker { display: flex; align-items: center; gap: 6px; }
.asset-picker-label { flex-shrink: 0; color: var(--text-2); font-size: 11px; }
.asset-select { min-width: 0; }
.asset-help { color: var(--text-2); font-size: 10px; line-height: 1.45; }
.mono { font-family: var(--mono); }
</style>
