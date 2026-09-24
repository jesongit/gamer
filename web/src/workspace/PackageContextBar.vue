<template>
  <div class="func-pkg-row package-context-bar" :class="{ compact, management }">
    <span v-if="!compact" class="pkg-bar-label" title="脚本、模板等插件资源的数据上下文">{{ management ? '当前配置' : '配置包' }}</span>
    <select
      :value="ctx.currentId || ''"
      class="select mono func-pkg"
      title="当前配置：右侧所有非全局功能的统一数据上下文"
      @change="ctx.onPackageChange"
    >
      <option v-if="!ctx.pkgOptions.length" value="">（暂无配置）</option>
      <option v-for="id in ctx.pkgOptions" :key="id" :value="id">{{ ctx.optionLabel(id) }}</option>
    </select>
    <button class="btn btn-sm" :disabled="!ctx.currentId || ctx.busy" title="配置包详情" aria-label="配置包详情" @click="ctx.openDetail()"><UiIcon name="info" />详情</button>
    <component :is="management ? 'div' : 'details'" class="pkg-more">
      <summary v-if="!management" class="btn btn-sm btn-icon menu-summary" title="配置包操作" aria-label="配置包操作"><UiIcon name="more" /></summary>
      <div class="pkg-menu" :class="{ 'action-menu': !management }" @click="closeMenu">
        <button :class="management ? 'btn btn-sm' : 'action-menu-item'" :disabled="ctx.busy" title="导入 .gamerpkg 为本地配置" @click="ctx.pickImportFile(fileInput)">导入</button>
        <button :class="management ? 'btn btn-sm' : 'action-menu-item'" :disabled="!ctx.currentId || ctx.busy" title="导出当前配置为 .gamerpkg" @click="ctx.exportPackage">导出</button>
        <button :class="management ? 'btn btn-sm btn-primary' : 'action-menu-item'" :disabled="ctx.busy" title="新建空配置" @click="ctx.openCreate">新建</button>
        <button :class="management ? 'btn btn-sm' : 'action-menu-item'" :disabled="!ctx.currentId || ctx.busy" title="复制当前配置为新配置（保留自己的修改）" @click="ctx.openDuplicate">复制</button>
        <span v-if="!management" class="action-menu-separator" role="separator"></span>
        <button :class="management ? 'btn btn-sm btn-danger' : 'action-menu-item danger'" :disabled="!ctx.currentId || ctx.busy" title="删除当前配置（数据不可恢复）" @click="ctx.openDelete" aria-label="删除配置">删除</button>
      </div>
    </component>
    <span v-if="compact" class="pkg-bar-label" title="脚本、模板等插件资源的数据上下文">配置</span>
    <!-- .gamerpkg 归档选择：change 后清空 value 支持重选同文件 -->
    <input ref="fileInput" class="pkg-import-input" type="file" accept=".gamerpkg,.zip" @change="ctx.onImportPicked" />
  </div>

  <Teleport v-if="dialogs" to="body">
  <!-- 新建 / 复制弹窗（共用表单） -->
  <div v-if="ctx.formModal.open" class="modal-mask" @click.self="ctx.closeForm">
    <div class="modal">
      <h3>{{ ctx.formModal.mode === 'create' ? '新建配置' : '复制配置' }}</h3>
      <label class="field">
        <span>配置 ID（小写字母/数字开头，含 . _ -）</span>
        <input v-model="ctx.formModal.form.id" class="input mono" placeholder="如 user.hsr.daily" spellcheck="false" />
      </label>
      <label class="field">
        <span>名称（可选）</span>
        <input v-model="ctx.formModal.form.name" class="input" placeholder="显示名" />
      </label>
      <label class="field">
        <span>版本</span>
        <input v-model="ctx.formModal.form.version" class="input mono" />
      </label>
      <label class="field">
        <span>Android 兼容目标（逗号分隔；`*` 或留空 = 通用配置）</span>
        <input v-model="ctx.formModal.form.androidPackagesText" class="input mono" placeholder="* 或 com.miHoYo.hkrpg, com.HoYoverse.hkrpgoversea" spellcheck="false" />
      </label>
      <p v-if="ctx.formModal.error" class="form-error">{{ ctx.formModal.error }}</p>
      <div class="modal-actions">
        <button v-if="ctx.formModal.mode === 'create'" class="btn" :disabled="ctx.formModal.submitting" @click="ctx.fillCurrentApp">填入当前应用</button>
        <button class="btn" @click="ctx.closeForm">取消</button>
        <button class="btn btn-primary" :disabled="ctx.formModal.submitting" @click="ctx.submitForm">
          {{ ctx.formModal.submitting ? '保存中…' : '保存' }}
        </button>
      </div>
    </div>
  </div>

  <!-- 覆盖导入确认（plan §9：明示整体替换，避免半安装状态由服务端原子替换保证） -->
  <div v-if="ctx.overwriteModal.open" class="modal-mask" @click.self="ctx.closeOverwrite">
    <div class="modal">
      <h3>配置已存在</h3>
      <p class="overwrite-warning">继续导入将<b>覆盖该配置当前数据</b>，包括用户直接修改或新增的内容。</p>
      <dl class="summary">
        <dt>ID</dt><dd class="mono">{{ ctx.overwriteModal.summary.id }}</dd>
        <dt>名称</dt><dd>{{ ctx.overwriteModal.summary.name || '—' }}</dd>
        <dt>版本</dt><dd class="mono">{{ ctx.overwriteModal.summary.version || '—' }}</dd>
        <dt>兼容目标</dt><dd class="mono">{{ ctx.overwriteModal.summary.androidTargets.join(', ') || '通用配置' }}</dd>
      </dl>
      <!-- plan §36：缺 Required Plugin 只黄条提示，允许继续导入（安装插件后自动恢复） -->
      <div v-if="ctx.overwriteModal.missingRequired.length" class="missing-required-warning" role="alert">
        ⚠ 缺少插件 {{ ctx.overwriteModal.missingRequired.join('、') }}，导入后部分功能不可用，安装插件后自动恢复。
      </div>
      <p v-if="ctx.overwriteModal.error" class="form-error">{{ ctx.overwriteModal.error }}</p>
      <div class="modal-actions">
        <button class="btn" @click="ctx.closeOverwrite">取消</button>
        <button class="btn btn-danger" :disabled="ctx.overwriteModal.submitting" @click="ctx.confirmOverwrite">
          {{ ctx.overwriteModal.submitting ? '覆盖中…' : '覆盖导入' }}
        </button>
      </div>
    </div>
  </div>

  <!-- 删除确认 -->
  <div v-if="ctx.deleteModal.open" class="modal-mask" @click.self="ctx.closeDelete">
    <div class="modal">
      <h3>删除配置</h3>
      <p>将删除 <b class="mono">{{ ctx.deleteModal.target?.id }}</b> 的全部数据（含插件数据与 shared/），绑定任务将被挂起（不删除）。此操作不可恢复。</p>
      <p v-if="ctx.deleteModal.error" class="form-error">{{ ctx.deleteModal.error }}</p>
      <div class="modal-actions">
        <button class="btn" @click="ctx.closeDelete">取消</button>
        <button class="btn btn-danger" :disabled="ctx.deleteModal.submitting" @click="ctx.confirmDelete">
          {{ ctx.deleteModal.submitting ? '删除中…' : '确认删除' }}
        </button>
      </div>
    </div>
  </div>

  <!-- 清单来自待下载的实际归档；取消只关闭预览，不触发下载。 -->
  <div v-if="ctx.exportModal.open" class="modal-mask" @click.self="ctx.closeExport">
    <div class="modal export-modal" role="dialog" aria-modal="true" aria-labelledby="package-export-title" :aria-busy="ctx.exportModal.loading">
      <h3 id="package-export-title">导出配置包</h3>
      <p class="export-subtitle">{{ ctx.exportModal.packageId }} · 仅包含已保存的内容</p>
      <p v-if="ctx.exportModal.loading" class="export-loading" role="status">正在准备导出文件与内容清单…</p>
      <template v-if="ctx.exportModal.ready">
        <div class="export-overview">
          <strong>{{ ctx.exportModal.files.length }} 个文件</strong>
          <span>下载大小 {{ formatBytes(ctx.exportModal.archiveBytes) }}</span>
        </div>
        <div class="export-groups">
          <span v-for="group in ctx.exportModal.groups" :key="group.label">{{ group.label }} <b>{{ group.count }}</b></span>
        </div>
        <div class="export-list-heading">
          <h4>本次包含</h4>
          <input v-model="exportSearch" class="input" type="search" placeholder="搜索文件名或类型" aria-label="搜索导出文件" />
        </div>
        <div class="export-files">
          <table>
            <thead><tr><th>文件</th><th>类型</th><th>大小</th></tr></thead>
            <tbody>
              <tr v-for="file in exportFiles" :key="file.path">
                <td class="export-path">{{ file.path }}</td><td>{{ file.category }}</td><td>{{ formatBytes(file.size) }}</td>
              </tr>
              <tr v-if="!exportFiles.length"><td colspan="3">没有符合搜索条件的文件</td></tr>
            </tbody>
          </table>
        </div>
        <p class="export-note">列表大小为文件原始大小，下载大小为压缩后的大小。函数库按文件计数，一个文件可包含多个函数。</p>
      </template>
      <section v-if="ctx.exportModal.entries.length" class="export-media">
        <label class="include-media">
          <input v-model="ctx.exportModal.includeMedia" type="checkbox" :disabled="ctx.exportModal.loading || ctx.exportModal.submitting" @change="ctx.refreshExport" />
          包含媒体原文件（{{ formatBytes(ctx.exportModal.totalBytes) }}）
        </label>
        <p class="export-note">{{ ctx.exportModal.includeMedia ? '素材随包携带，方便在其他电脑使用。' : '当前只包含素材引用；接收端若没有对应素材，将显示素材缺失。模板图片始终包含。' }}</p>
        <ul v-if="ctx.exportModal.ready" class="media-list">
          <li v-for="entry in ctx.exportModal.entries" :key="entry.id + entry.plugin_id + entry.kind" class="media-item">
            <span class="media-name">{{ entry.name || entry.id }}</span>
            <span class="media-meta">{{ formatBytes(entry.size) }} · {{ entry.included ? '已包含原文件' : '仅引用' }}</span>
          </li>
        </ul>
        <p v-if="ctx.exportModal.includeMedia && ctx.exportModal.ready && ctx.exportModal.entries.some(entry => !entry.included)" class="form-error">部分素材不可用，原文件未能包含在内，见上方“仅引用”条目。</p>
        <p v-if="ctx.exportModal.includeMedia" class="export-note">媒体可能含有账号等画面，分享前请检查内容。</p>
      </section>
      <div class="export-excluded">
        <h4>不包含</h4>
        <p>未保存的修改、设备配置、系统设置、任务列表中的定时任务、运行记录和日志、插件程序。包内预设文件会随资源导出。</p>
      </div>
      <p v-if="ctx.exportModal.error" class="form-error" role="alert">{{ ctx.exportModal.error }}</p>
      <div class="modal-actions">
        <button v-if="ctx.exportModal.error && !ctx.exportModal.ready" class="btn" :disabled="ctx.exportModal.loading" @click="ctx.refreshExport">重新准备</button>
        <button class="btn" :disabled="ctx.exportModal.submitting" @click="ctx.closeExport">取消</button>
        <button class="btn btn-primary" :disabled="!ctx.exportModal.ready || ctx.exportModal.loading || ctx.exportModal.submitting" @click="ctx.confirmExport">
          {{ ctx.exportModal.submitting ? '下载中…' : '确认导出' }}
        </button>
      </div>
    </div>
  </div>

  <!-- Package 详情弹窗（plan §18/§21/§37 + §17）：依赖五态 / stats / 元数据编辑 / 兼容性 -->
  <PackageDetailModal
    v-if="ctx.detailModal.open"
    :context="ctx"
    :android-candidates="androidCandidates"
  />
  </Teleport>
</template>

<script setup>
/**
 * 右侧面板顶部的 Package Context 栏（plan §28）：当前 Package 下拉 +
 * 导入/导出/新建/复制/删除/详情。状态与动作收敛在 usePackageContext
 * （§38 Current Package 由 Core Store 统一管理），本组件只做呈现与文件
 * input 承载；详情弹窗（§18/§37）同样消费注入的 ctx。
 */
import { computed, reactive, ref, watch } from 'vue'
import { useToast, devicesData } from '../store'
import { usePackageContext, formatBytes } from '../composables/usePackageContext'
import UiIcon from '../components/ui/UiIcon.vue'
import PackageDetailModal from './PackageDetailModal.vue'

const props = defineProps({ context: { type: Object, default: null }, compact: Boolean, management: Boolean, dialogs: { type: Boolean, default: true } })
const toast = useToast()
const fileInput = ref(null)
function closeMenu(event) {
  const button = event.target.closest('button')
  if (!button || button.disabled) return
  const menu = event.currentTarget.closest('details')
  if (menu) menu.open = false
}
// context 由 Console 装配（注入 refreshAll 全量刷新）；缺省自建（测试/独立使用）。
// reactive() 解包对象内的 ref/computed（模板 ctx.busy/ctx.currentId 直接可用）。
const ctx = reactive(props.context || usePackageContext({ toast }))
const exportSearch = ref('')
watch(() => ctx.exportModal.open, () => { exportSearch.value = '' })
const exportFiles = computed(() => {
  const search = exportSearch.value.trim().toLocaleLowerCase()
  return (ctx.exportModal.files || []).filter(file => !search || `${file.path} ${file.category}`.toLocaleLowerCase().includes(search))
})


// 兼容性检查候选（§17）：设备配置的 Android 包名注入 datalist，缺省手输。
const androidCandidates = computed(() => {
  const ids = new Set()
  for (const d of devicesData.value || []) {
    if (d?.pkg) ids.add(d.pkg)
  }
  return [...ids]
})
</script>

<style scoped>
.package-context-bar { display:flex; align-items:center; gap:6px; flex-shrink:0; }
.package-context-bar .pkg-bar-label { flex:none; font-size:12px; color:var(--text-2); white-space:nowrap; }
.package-context-bar .func-pkg { flex:1; min-width:0; font-size:12px; }
.package-context-bar .btn { flex:none; }
.pkg-import-input { display:none; }

.modal-mask { position:fixed; inset:0; background:rgba(0,0,0,.45); display:flex; align-items:center; justify-content:center; z-index:100; }
.modal { width:min(440px, 92vw); max-height:86vh; overflow:auto; background:var(--bg-1); border:1px solid var(--border); border-radius:var(--radius); padding:16px; display:flex; flex-direction:column; gap:10px; }
.modal h3 { margin:0; font-size:15px; }
.field { display:flex; flex-direction:column; gap:4px; font-size:12px; color:var(--text-2); }
.field .input, .field select { font-size:13px; }
.form-error { margin:0; color:var(--danger, #f87171); font-size:12px; }
.modal-actions { display:flex; justify-content:flex-end; gap:8px; }
.overwrite-warning { margin:0; font-size:13px; line-height:1.5; }
.media-list { margin:0; padding:0; list-style:none; max-height:180px; overflow:auto; display:flex; flex-direction:column; gap:4px; }
.media-item { display:flex; justify-content:space-between; gap:8px; font-size:12px; }
.media-name { word-break:break-all; }
.media-meta { color:var(--text-2); white-space:nowrap; }
.export-modal { width:min(720px, 94vw); gap:12px; }
.export-modal h4 { margin:0; font-size:13px; }
.export-subtitle, .export-note { margin:0; font-size:12px; line-height:1.6; color:var(--text-2); overflow-wrap:anywhere; }
.export-overview { display:flex; align-items:baseline; flex-wrap:wrap; gap:8px 16px; font-size:13px; }
.export-overview strong { font-size:18px; }
.export-groups { display:flex; flex-wrap:wrap; gap:6px; }
.export-groups span { padding:5px 8px; background:var(--bg-2); border:1px solid var(--border); border-radius:var(--radius-sm); font-size:12px; }
.export-groups b { margin-left:5px; color:var(--accent); }
.export-list-heading { display:flex; align-items:center; justify-content:space-between; gap:12px; }
.export-list-heading .input { min-width:0; width:210px; max-width:65%; font-size:12px; }
.export-files { max-height:260px; overflow:auto; flex-shrink:0; border:1px solid var(--border); border-radius:var(--radius-sm); }
.export-files table { width:100%; border-collapse:collapse; font-size:12px; }
.export-files th, .export-files td { padding:8px 10px; text-align:left; border-bottom:1px solid var(--border); }
.export-files th { position:sticky; top:0; background:var(--bg-2); color:var(--text-2); font-weight:500; }
.export-files td:last-child { white-space:nowrap; text-align:right; }
.export-files tr:last-child td { border-bottom:0; }
.export-path { overflow-wrap:anywhere; }
.export-media, .export-excluded { display:flex; flex-direction:column; gap:8px; padding:12px; background:var(--bg-2); border-radius:var(--radius-sm); }
.export-excluded p { margin:0; font-size:12px; line-height:1.6; color:var(--text-2); }
.export-loading { padding:20px 0; text-align:center; color:var(--text-2); font-size:13px; }
.export-modal .modal-actions { position:sticky; bottom:-16px; padding:12px 0; background:var(--bg-1); }

.privacy-note { margin:0; font-size:12px; line-height:1.5; color:var(--warn, #fbbf24); border:1px solid rgba(251,191,36,.35); background:rgba(251,191,36,.08); border-radius: var(--radius-sm); padding:8px 10px; }
.include-media { display:flex; align-items:center; gap:6px; font-size:13px; }
.missing-required-warning { margin:0; font-size:12px; line-height:1.5; color:var(--warn, #fbbf24); border:1px solid rgba(251,191,36,.35); background:rgba(251,191,36,.08); border-radius: var(--radius-sm); padding:8px 10px; }
.summary { margin:0; display:grid; grid-template-columns:auto 1fr; gap:4px 12px; font-size:12px; }
.summary dt { color:var(--text-2); }
.summary dd { margin:0; word-break:break-all; }
.compact{max-width:100%;min-width:0;flex-wrap:wrap;justify-content:flex-end}.compact .func-pkg{field-sizing:content;flex:0 1 auto;width:auto;min-width:0;max-width:100%}.compact .pkg-bar-label{font-size:12px}.pkg-more{position:relative}.pkg-menu{position:absolute;top:calc(100% + 4px);right:0;z-index:35}
.management { flex-wrap:wrap; gap:8px; padding-bottom:16px; border-bottom:1px solid var(--border); }
.management .func-pkg { flex:0 1 280px; }.management .pkg-more { margin-left:auto; }
.management .pkg-menu { position:static; display:flex; flex-direction:row; flex-wrap:wrap; gap:6px; padding:0; background:transparent; border:0; box-shadow:none; min-width:0; }
.management .pkg-menu .btn:not(.btn-primary) { background:transparent; border-color:var(--border); }.management .pkg-menu .btn-danger { color:var(--text-1); }.management .pkg-menu .btn-danger:hover { color:var(--danger); border-color:var(--danger); }
.management .pkg-menu .btn-primary { background:var(--accent); border-color:var(--accent); color:#202015; }
</style>
