<template>
  <div class="func-pkg-row workspace-context-bar">
    <select v-model="ctx.activePkg" class="select mono func-pkg" title="当前包名：脚本/函数库/模板和后续操作都使用此包名">
      <option v-if="!ctx.pkgOptions.length" value="">（请选择包名）</option>
      <option v-for="pkg in ctx.pkgOptions" :key="pkg" :value="pkg">{{ ctx.packageOptionLabel(pkg) }}</option>
    </select>
    <button class="btn btn-sm" :disabled="!ctx.current || ctx.appLoading" title="读取当前设备已安装应用并加入包名下拉" @click="ctx.loadApps">{{ ctx.appLoading ? '读取中…' : '🔄 读取应用' }}</button>
    <button class="btn btn-sm" :disabled="ctx.busy" title="导入 Gamer 游戏包" @click="ctx.pickImportFile(fileInput)">导入</button>
    <button class="btn btn-sm" :disabled="!ctx.activePkg || ctx.busy" title="导出当前编辑区为 .gamerpkg" @click="ctx.openExport">导出</button>
    <button class="btn btn-sm" :disabled="!ctx.activePkg || ctx.busy" title="编辑已安装的游戏包" @click="ctx.openEdit">编辑</button>
    <!-- 游戏包归档选择：accept 限 .gamerpkg；change 后由 composable 清空 value 支持重选同文件 -->
    <input ref="fileInput" class="pkg-import-input" type="file" accept=".gamerpkg" @change="ctx.onImportPicked" />
  </div>
  <!-- 游戏包三弹窗：元数据初始化（导出前置）→ 导出确认 → 编辑确认；状态与动作全部来自 ctx -->
  <PackageMetaModal
    :open="ctx.metaModal.open"
    :saving="ctx.metaModal.saving"
    :error="ctx.metaModal.error"
    :form="ctx.metaModal.form"
    @submit="ctx.submitMeta"
    @close="ctx.closeMeta"
  />
  <PackageExportModal
    :open="ctx.exportModal.open"
    :exporting="ctx.exportModal.exporting"
    :error-lines="ctx.exportModal.errorLines"
    :info="ctx.exportModal.info"
    @confirm="ctx.confirmExport"
    @close="ctx.closeExport"
  />
  <PackageEditModal
    :open="ctx.editModal.open"
    :starting="ctx.editModal.starting"
    :id="ctx.editModal.id"
    :version="ctx.editModal.version"
    :target="ctx.editModal.target"
    :targets="ctx.editModal.targets"
    :show-target-picker="ctx.editModal.showTargetPicker"
    @update:target="t => { ctx.editModal.target = t }"
    @confirm="ctx.confirmEdit"
    @close="ctx.closeEdit"
  />
</template>

<script setup>
/**
 * 右侧面板顶部的应用分区上下文条：分区下拉 + 读取应用 + 游戏包三入口
 * （导入 .gamerpkg / 导出编辑区 / 编辑已安装包）。动作与弹窗状态全部收敛在
 * useWorkspacePackages composable（Console 装配后并入 context），本组件只透传。
 */
import { reactive, ref } from 'vue'
import PackageMetaModal from './PackageMetaModal.vue'
import PackageExportModal from './PackageExportModal.vue'
import PackageEditModal from './PackageEditModal.vue'

const props = defineProps({ context: { type: Object, required: true } })
const ctx = reactive(props.context)
const fileInput = ref(null)
</script>

<style scoped>
.workspace-context-bar { display:flex; align-items:center; gap:6px; flex-shrink:0; }
.workspace-context-bar .func-pkg { flex:1; min-width:0; font-size:12px; }
.workspace-context-bar .btn { flex:none; }
.pkg-import-input { display:none; }
</style>
