<template>
  <nav class="workspace-tabs" aria-label="功能面板" role="tablist">
    <button v-for="panel in panels" :key="panel.key" type="button" class="workspace-tab"
      :class="{ active: panel.key === activePanel }" :aria-selected="panel.key === activePanel"
      :title="panel.title" role="tab" @click="$emit('select', panel.key)">{{ panel.title }}</button>
  </nav>
</template>
<script setup>
defineProps({ panels: { type: Array, default: () => [] }, activePanel: { type: String, default: '' } })
defineEmits(['select'])
</script>
<style scoped>
.workspace-tabs { display:flex; flex-shrink:0; height:40px; border-bottom:1px solid var(--border); background:var(--chrome); padding:0 14px; gap:8px; }
.workspace-tab { flex:0 0 auto; min-width:68px; padding:8px 17px; border:0; border-bottom:2px solid transparent; background:transparent; color:var(--text-1); font-size:13px; cursor:pointer; white-space:nowrap; }
.workspace-tab:hover { color:var(--text-0); background:var(--bg-3); }
.workspace-tab.active { color:var(--text-0); border-bottom-color:var(--accent); font-weight:600; }
</style>
