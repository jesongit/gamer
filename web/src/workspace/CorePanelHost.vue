<template>
  <div class="core-panel-host panel-sec" :class="contribution?.panelClass || ''">
    <component :is="contribution.component" v-if="contribution?.component" v-bind="panelProps" />
    <div v-else class="workspace-empty">内置面板不可用</div>
  </div>
</template>

<script setup>
import { computed } from 'vue'

const props = defineProps({
  contribution: { type: Object, required: true },
  context: { type: Object, default: () => ({}) },
})

const panelProps = computed(() => {
  if (typeof props.contribution?.getProps === 'function') return props.contribution.getProps(props.context) || {}
  return {}
})
</script>

<style scoped>
.core-panel-host { flex:1; min-height:0; overflow:hidden; background:var(--bg-1); border:1px solid var(--border); border-radius:var(--radius); padding:14px; display:flex; flex-direction:column; gap:10px; }
.core-panel-host.tpl-tab, .core-panel-host.script-tab { flex:1; min-height:0; overflow:hidden; }
.core-panel-host.extra-tab { flex:1; min-height:0; overflow:hidden; display:flex; flex-direction:column; }
.workspace-empty { color:var(--text-2); padding:18px; text-align:center; }
</style>
