<template>
  <div class="func-pkg-row package-context-bar">
    <span class="pkg-bar-label" title="配置：数据上下文（plan §28）">📦 配置</span>
    <select
      :value="ctx.currentId || ''"
      class="select mono func-pkg"
      title="当前配置：右侧所有非全局功能的统一数据上下文"
      @change="ctx.onPackageChange"
    >
      <option v-if="!ctx.pkgOptions.length" value="">（暂无配置）</option>
      <option v-for="id in ctx.pkgOptions" :key="id" :value="id">{{ ctx.optionLabel(id) }}</option>
    </select>
    <button class="btn btn-sm" :disabled="!ctx.currentId || ctx.busy" title="查看配置详情 / 依赖状态 / 编辑元数据（plan §18/§37）" @click="ctx.openDetail()">ℹ️ 详情</button>
    <button class="btn btn-sm" :disabled="ctx.busy" title="导入 .gamerpkg 为本地配置" @click="ctx.pickImportFile(fileInput)">导入</button>
    <button class="btn btn-sm" :disabled="!ctx.currentId || ctx.busy" title="导出当前配置为 .gamerpkg" @click="ctx.exportPackage">导出</button>
    <button class="btn btn-sm" :disabled="ctx.busy" title="新建空配置" @click="ctx.openCreate">新建</button>
    <button class="btn btn-sm" :disabled="!ctx.currentId || ctx.busy" title="复制当前配置为新配置（保留自己的修改）" @click="ctx.openDuplicate">复制</button>
    <button class="btn btn-sm btn-danger" :disabled="!ctx.currentId || ctx.busy" title="删除当前配置（数据不可恢复）" @click="ctx.openDelete">🗑</button>
    <!-- .gamerpkg 归档选择：change 后清空 value 支持重选同文件 -->
    <input ref="fileInput" class="pkg-import-input" type="file" accept=".gamerpkg,.zip" @change="ctx.onImportPicked" />
  </div>

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
        <span>Android 兼容目标（逗号分隔，可留空 = 通用配置）</span>
        <input v-model="ctx.formModal.form.androidPackagesText" class="input mono" placeholder="com.miHoYo.hkrpg, com.HoYoverse.hkrpgoversea" spellcheck="false" />
      </label>
      <p v-if="ctx.formModal.error" class="form-error">{{ ctx.formModal.error }}</p>
      <div class="modal-actions">
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

  <!-- Package 详情弹窗（plan §18/§21/§37 + §17）：依赖五态 / stats / 元数据编辑 / 兼容性 -->
  <PackageDetailModal
    v-if="ctx.detailModal.open"
    :context="ctx"
    :android-candidates="androidCandidates"
  />
</template>

<script setup>
/**
 * 右侧面板顶部的 Package Context 栏（plan §28）：当前 Package 下拉 +
 * 导入/导出/新建/复制/删除/详情。状态与动作收敛在 usePackageContext
 * （§38 Current Package 由 Core Store 统一管理），本组件只做呈现与文件
 * input 承载；详情弹窗（§18/§37）同样消费注入的 ctx。
 */
import { computed, reactive, ref } from 'vue'
import { useToast, devicesData } from '../store'
import { usePackageContext } from '../composables/usePackageContext'
import PackageDetailModal from './PackageDetailModal.vue'

const props = defineProps({ context: { type: Object, default: null } })
const toast = useToast()
const fileInput = ref(null)
// context 由 Console 装配（注入 refreshAll 全量刷新）；缺省自建（测试/独立使用）。
// reactive() 解包对象内的 ref/computed（模板 ctx.busy/ctx.currentId 直接可用）。
const ctx = reactive(props.context || usePackageContext({ toast }))

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
.missing-required-warning { margin:0; font-size:12px; line-height:1.5; color:var(--warn, #fbbf24); border:1px solid rgba(251,191,36,.35); background:rgba(251,191,36,.08); border-radius:6px; padding:8px 10px; }
.summary { margin:0; display:grid; grid-template-columns:auto 1fr; gap:4px 12px; font-size:12px; }
.summary dt { color:var(--text-2); }
.summary dd { margin:0; word-break:break-all; }
</style>
