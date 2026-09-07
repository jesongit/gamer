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
          <span class="project-meta mono">
            {{ project.markerCount }} 标记 · {{ project.assetCount }} 素材
            <template v-if="!project.valid"> · 校验失败</template>
          </span>
        </span>
        <span class="row-actions" @click.stop>
          <button class="mini-btn" type="button" data-testid="project-rename" @click="beginRename(project)">重命名</button>
          <button
            class="mini-btn danger"
            type="button"
            :class="{ armed: armedId === project.id }"
            data-testid="project-delete"
            @click="remove(project)"
          >{{ armedId === project.id ? '确认删除' : '删除' }}</button>
        </span>
        <!-- 行内重命名：新 id（资源原子移动，引用路径不变语义） -->
        <span v-if="renamingId === project.id" class="rename-box" @click.stop>
          <input v-model="renameId" class="input rename-input" type="text" placeholder="新项目 ID" data-testid="project-rename-input" />
          <button class="mini-btn" type="button" :disabled="!renameId" data-testid="project-rename-confirm" @click="$emit('rename', project.id, renameId); renamingId = ''">确定</button>
          <button class="mini-btn" type="button" data-testid="project-rename-cancel" @click="renamingId = ''">取消</button>
        </span>
      </div>
    </div>
  </section>
</template>

<script setup>
// 项目列表区（Phase 6 §9.1）：项目 CRUD 的展示层——创建（基于素材库选中素材）、
// 打开、重命名（资源 rename 原子移动）、删除（不自动删原视频）。持久化动作由
// 宿主 VideoWorkbench 承担（本组件只上抛意图）。
import { computed, ref } from 'vue'
import { isValidProjectId } from './videoProject'

const props = defineProps({
  /** 项目摘要列表：{id, name, valid, markerCount, assetCount} */
  projects: { type: Array, default: () => [] },
  openId: { type: String, default: '' },
  loading: { type: Boolean, default: false },
  /** 是否具备新建条件（有当前 Package + 素材库已选中主素材）。 */
  canCreate: { type: Boolean, default: false },
})
const emit = defineEmits(['open', 'create', 'rename', 'delete', 'refresh'])

const creating = ref(false)
const newId = ref('')
const newName = ref('')
const createError = ref('')
const renamingId = ref('')
const renameId = ref('')
const armedId = ref('')

const createReady = computed(() => isValidProjectId(newId.value) && !!newName.value.trim())

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

function beginRename(project) {
  renamingId.value = renamingId.value === project.id ? '' : project.id
  renameId.value = ''
}

function remove(project) {
  if (armedId.value !== project.id) {
    armedId.value = project.id
    return
  }
  armedId.value = ''
  emit('delete', project.id)
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
.project-meta { color: var(--text-2); font-size: 10px; }
.row-actions { display: flex; gap: 4px; flex-shrink: 0; }
.rename-box { display: flex; gap: 4px; width: 100%; align-items: center; }
.rename-input { max-width: 180px; }
.list-empty { padding: 16px 10px; text-align: center; color: var(--text-2); font-size: 12px; }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 2px 6px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mini-btn.danger:hover, .mini-btn.danger.armed { border-color: var(--danger); color: var(--danger); }
.zone-error { padding: 5px 7px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; }
.mono { font-family: var(--mono); }
</style>
