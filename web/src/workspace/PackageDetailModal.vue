<template>
  <div v-if="ctx.detailModal.open" class="modal-mask" @click.self="ctx.closeDetail">
    <div class="modal modal-wide">
      <h3>Package 详情<template v-if="ctx.detailModal.packageId"> · <span class="mono">{{ ctx.detailModal.packageId }}</span></template></h3>

      <p v-if="ctx.detailModal.loading" class="detail-hint">加载中…</p>
      <template v-else-if="ctx.detailModal.error">
        <p class="form-error">{{ ctx.detailModal.error }}</p>
        <div class="modal-actions">
          <button class="btn" @click="ctx.reloadDetail()">重试</button>
          <button class="btn" @click="ctx.closeDetail">关闭</button>
        </div>
      </template>

      <template v-else-if="pkg">
        <!-- 元数据（§37：Package Metadata 可以直接编辑）——只读摘要 / 编辑表单二选一 -->
        <form v-if="ctx.detailModal.editing" class="detail-edit" @submit.prevent>
          <label class="field">
            <span>名称（留空保持不变）</span>
            <input v-model="ctx.detailModal.form.name" class="input" placeholder="显示名" />
          </label>
          <label class="field">
            <span>版本（留空保持不变）</span>
            <input v-model="ctx.detailModal.form.version" class="input mono" />
          </label>
          <label class="field">
            <span>作者（留空保持不变）</span>
            <input v-model="ctx.detailModal.form.author" class="input" />
          </label>
          <label class="field">
            <span>Android 兼容目标（逗号分隔，留空 = 通用包）</span>
            <input v-model="ctx.detailModal.form.androidPackagesText" class="input mono" spellcheck="false" />
          </label>
          <div class="field">
            <span>插件依赖（逐行 id + required；允许声明未安装插件）</span>
            <div v-for="(dep, i) in ctx.detailModal.form.plugins" :key="`dep-${i}`" class="plugin-dep-row">
              <input v-model="dep.id" class="input mono" placeholder="插件 id，如 gamer.yaml" spellcheck="false" />
              <label class="dep-required"><input v-model="dep.required" type="checkbox" />必需</label>
              <button type="button" class="btn btn-sm" @click="ctx.removePluginDep(i)">移除</button>
            </div>
            <button type="button" class="btn btn-sm dep-add" @click="ctx.addPluginDep">＋ 添加插件依赖</button>
          </div>
        </form>
        <dl v-else class="summary">
          <dt>ID</dt><dd class="mono">{{ pkg.id }}</dd>
          <dt>名称</dt><dd>{{ pkg.name || '—' }}</dd>
          <dt>版本</dt><dd class="mono">{{ pkg.version || '—' }}</dd>
          <dt>作者</dt><dd>{{ pkg.author || '—' }}</dd>
          <dt>Revision</dt><dd class="mono">{{ pkg.revision }}</dd>
          <dt>Android Targets</dt><dd class="mono">{{ targetsText }}</dd>
        </dl>

        <!-- 插件依赖五态（§18）：available=绿 / disabled=黄 / missing_required=红 /
             missing_optional=灰 / unknown=灰；数据始终保留 -->
        <div class="detail-section">
          <span class="section-label">插件依赖</span>
          <p v-if="!pluginStates.length" class="detail-hint">未声明插件依赖</p>
          <div v-for="ps in pluginStates" :key="`ps-${ps.plugin}`" class="plugin-state-row">
            <span class="state-badge" :class="stateClass(ps.state)">{{ stateLabel(ps.state) }}</span>
            <span class="mono plugin-id">{{ ps.plugin }}</span>
            <span v-if="ps.required" class="req-tag">必需</span>
            <span v-if="ps.state === 'missing_required'" class="miss-hint">部分功能不可用，可安装后恢复</span>
          </div>
        </div>

        <!-- stats：文件数 / 字节 / 各插件统计 -->
        <div class="detail-section">
          <span class="section-label">内容统计</span>
          <dl class="summary stats">
            <dt>文件数</dt><dd>{{ stats.files }}</dd>
            <dt>总大小</dt><dd>{{ formatBytes(stats.bytes) }}</dd>
            <template v-for="psp in stats.plugins" :key="`stat-${psp.plugin}`">
              <dt>· {{ psp.plugin }}</dt>
              <dd>{{ psp.files }} 个文件 / {{ formatBytes(psp.bytes) }}</dd>
            </template>
          </dl>
        </div>

        <!-- 兼容性检查（§17）：不兼容仅黄条提示，不禁止使用 -->
        <div class="detail-section">
          <span class="section-label">兼容性检查</span>
          <div class="compat-row">
            <input
              v-model="ctx.detailModal.compat.input"
              class="input mono"
              list="pkg-compat-candidates"
              placeholder="Android 包名，如 com.miHoYo.hkrpg"
              spellcheck="false"
              @keydown.enter.prevent="runCompat"
            />
            <datalist id="pkg-compat-candidates">
              <option v-for="c in androidCandidates" :key="`cand-${c}`" :value="c" />
            </datalist>
            <button
              class="btn btn-sm"
              :disabled="!ctx.detailModal.compat.input.trim() || ctx.detailModal.compat.checking"
              @click="runCompat"
            >{{ ctx.detailModal.compat.checking ? '检查中…' : '检查' }}</button>
          </div>
          <p v-if="!targets.length" class="compat-info">通用包，兼容所有应用</p>
          <p v-if="compatResult && compatResult.compatible" class="compat-ok">
            ✔ {{ compatResult.android_package }} 在 Package 声明的兼容列表中
          </p>
          <p v-if="compatResult && !compatResult.compatible" class="compat-warning">
            ⚠ 该应用不在 Package 声明的兼容列表中（仍可继续运行）
          </p>
          <p v-if="ctx.detailModal.compat.error" class="form-error">{{ ctx.detailModal.compat.error }}</p>
        </div>

        <p v-if="ctx.detailModal.editConflict" class="conflict-warning">
          ⚠ {{ ctx.detailModal.editError }}
        </p>
        <p v-else-if="ctx.detailModal.editError" class="form-error">{{ ctx.detailModal.editError }}</p>

        <div class="modal-actions">
          <template v-if="ctx.detailModal.editing">
            <button class="btn" :disabled="ctx.detailModal.saving" @click="ctx.cancelEdit">取消</button>
            <button v-if="ctx.detailModal.editConflict" class="btn" :disabled="ctx.detailModal.loading" @click="ctx.retryEdit">重试</button>
            <button v-if="ctx.detailModal.editConflict" class="btn btn-danger" :disabled="ctx.detailModal.saving" @click="ctx.saveEdit({ force: true })">
              {{ ctx.detailModal.saving ? '强制保存中…' : '强制保存' }}
            </button>
            <button v-else class="btn btn-primary" :disabled="ctx.detailModal.saving" @click="ctx.saveEdit()">
              {{ ctx.detailModal.saving ? '保存中…' : '保存' }}
            </button>
          </template>
          <template v-else>
            <button class="btn" @click="ctx.startEdit">✏️ 编辑</button>
            <button class="btn btn-primary" @click="ctx.closeDetail">关闭</button>
          </template>
        </div>
      </template>
    </div>
  </div>
</template>

<script setup>
/**
 * Package 详情弹窗（plan §18/§21/§37 + §17）：manifest 全字段 + 插件依赖五态
 * 徽章 + 内容统计 + 兼容性检查 + 元数据编辑表单。状态与动作全部收敛在
 * usePackageContext（detailModal），本组件只做呈现；androidCandidates 由宿主
 * 注入（如设备配置的 Android 包名），缺省手输。
 */
import { computed } from 'vue'
import { formatBytes } from '../composables/usePackageContext'

const props = defineProps({
  context: { type: Object, required: true },
  androidCandidates: { type: Array, default: () => [] },
})

const ctx = props.context

const pkg = computed(() => ctx.detailModal.detail?.package || null)
const stats = computed(() => ctx.detailModal.detail?.stats || { files: 0, bytes: 0, plugins: [] })
const pluginStates = computed(() => ctx.detailModal.detail?.pluginStates || [])
const targets = computed(() => pkg.value?.targets?.android?.packages || [])
const targetsText = computed(() => targets.value.join(', ') || '通用包（兼容所有应用）')
const compatResult = computed(() => ctx.detailModal.compat.result)

function runCompat() {
  ctx.checkCompatibility()
}

// 五态词表（plan §18）：available / disabled / missing_required / missing_optional / unknown
const STATE_META = {
  available: { label: '可用', cls: 'st-available' },
  disabled: { label: '未启用', cls: 'st-disabled' },
  missing_required: { label: '缺失', cls: 'st-missing-required' },
  missing_optional: { label: '缺失', cls: 'st-missing-optional' },
  unknown: { label: '未声明', cls: 'st-unknown' },
}

function stateMeta(state) {
  return STATE_META[state] || { label: state || '未知', cls: 'st-unknown' }
}
function stateClass(state) {
  return stateMeta(state).cls
}
function stateLabel(state) {
  return stateMeta(state).label
}
</script>

<style scoped>
.modal-wide { width: min(560px, 94vw); }
.detail-hint { margin: 0; font-size: 12px; color: var(--text-2); }
.section-label { font-size: 12px; color: var(--text-2); }
.detail-section { display: flex; flex-direction: column; gap: 6px; }

.state-badge { display: inline-block; padding: 1px 8px; border-radius: 999px; font-size: 11px; border: 1px solid; }
.state-badge.st-available { color: var(--ok, #34d399); border-color: rgba(52, 211, 153, 0.35); background: rgba(52, 211, 153, 0.08); }
.state-badge.st-disabled { color: var(--warn, #fbbf24); border-color: rgba(251, 191, 36, 0.35); background: rgba(251, 191, 36, 0.08); }
.state-badge.st-missing-required { color: var(--danger, #f87171); border-color: rgba(248, 113, 113, 0.35); background: rgba(248, 113, 113, 0.08); }
.state-badge.st-missing-optional,
.state-badge.st-unknown { color: var(--text-2, #9ca3af); border-color: var(--border, #374151); background: transparent; }

.plugin-state-row { display: flex; align-items: center; gap: 8px; font-size: 12px; flex-wrap: wrap; }
.plugin-state-row .plugin-id { word-break: break-all; }
.req-tag { font-size: 11px; color: var(--text-2); border: 1px solid var(--border); border-radius: 4px; padding: 0 4px; }
.miss-hint { font-size: 12px; color: var(--danger, #f87171); }

.detail-edit { display: flex; flex-direction: column; gap: 10px; }
.plugin-dep-row { display: flex; align-items: center; gap: 6px; }
.plugin-dep-row .input { flex: 1; min-width: 0; }
.dep-required { display: flex; align-items: center; gap: 4px; font-size: 12px; color: var(--text-2); white-space: nowrap; }
.dep-add { align-self: flex-start; }

.stats dd, .stats dt { font-size: 12px; }

.compat-row { display: flex; align-items: center; gap: 6px; }
.compat-row .input { flex: 1; min-width: 0; }
.compat-info { margin: 0; font-size: 12px; color: var(--text-2); }
.compat-ok { margin: 0; font-size: 12px; color: var(--ok, #34d399); }
.compat-warning { margin: 0; font-size: 12px; color: var(--warn, #fbbf24); }
.conflict-warning { margin: 0; font-size: 12px; color: var(--warn, #fbbf24); }

.field { display: flex; flex-direction: column; gap: 4px; font-size: 12px; color: var(--text-2); }
.field .input { font-size: 13px; }
.form-error { margin: 0; color: var(--danger, #f87171); font-size: 12px; }
.summary { margin: 0; display: grid; grid-template-columns: auto 1fr; gap: 4px 12px; font-size: 12px; }
.summary dt { color: var(--text-2); }
.summary dd { margin: 0; word-break: break-all; }
.modal-actions { display: flex; justify-content: flex-end; gap: 8px; flex-wrap: wrap; }
</style>
