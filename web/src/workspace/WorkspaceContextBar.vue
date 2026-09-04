<template>
  <div class="func-pkg-row workspace-context-bar">
    <select v-model="ctx.activePkg" class="select mono func-pkg" title="当前包名：脚本/函数库/模板和后续操作都使用此包名">
      <option v-if="!ctx.pkgOptions.length" value="">（请选择包名）</option>
      <option v-for="pkg in ctx.pkgOptions" :key="pkg" :value="pkg">{{ ctx.packageOptionLabel(pkg) }}</option>
    </select>
    <button class="btn btn-sm" :disabled="!ctx.current || ctx.appLoading" title="读取当前设备已安装应用并加入包名下拉" @click="ctx.loadApps">{{ ctx.appLoading ? '读取中…' : '🔄 读取应用' }}</button>
  </div>
</template>

<script setup>
import { reactive } from 'vue'

const props = defineProps({ context: { type: Object, required: true } })
const ctx = reactive(props.context)
</script>

<style scoped>
.workspace-context-bar { display:flex; align-items:center; gap:6px; flex-shrink:0; }
.workspace-context-bar .func-pkg { flex:1; min-width:0; font-size:12px; }
.workspace-context-bar .btn { flex:none; }
</style>
