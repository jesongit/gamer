<template>
  <nav class="workspace-tabs" aria-label="功能面板">
    <button
      v-for="panel in panels"
      :key="panel.key"
      type="button"
      class="workspace-tab"
      :class="{ active: panel.key === activePanel }"
      :aria-selected="panel.key === activePanel"
      :title="panel.title"
      role="tab"
      @click="$emit('select', panel.key)"
    >
      <span v-if="panel.icon" class="workspace-tab-icon" aria-hidden="true">{{ panel.icon }}</span>
      <span>{{ panel.title }}</span>
    </button>
    <button
      type="button"
      class="workspace-tab workspace-tab-add"
      title="打开插件中心"
      aria-label="打开插件中心"
      @click="$emit('open-plugin-center')"
    >
      <span aria-hidden="true">＋</span>
    </button>
  </nav>
</template>

<script setup>
defineProps({
  panels: { type: Array, default: () => [] },
  activePanel: { type: String, default: '' },
})
defineEmits(['select', 'open-plugin-center'])
</script>

<style scoped>
.workspace-tabs { display:flex; flex-shrink:0; border:1px solid var(--border); border-radius:var(--radius-sm); overflow:hidden; background:var(--bg-2); }
.workspace-tab { flex:1; min-width:0; padding:7px 3px; border:0; border-left:1px solid var(--border); background:transparent; color:var(--text-1); font-size:12px; cursor:pointer; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
.workspace-tab:first-child { border-left:0; }
.workspace-tab:hover { color:var(--text-0); background:var(--bg-3); }
.workspace-tab.active { color:var(--accent); background:rgba(34,211,165,.14); font-weight:600; }
.workspace-tab-icon { margin-right:2px; }
.workspace-tab-add { flex:0 0 34px; color:var(--accent-2); font-size:17px; font-weight:600; }
.workspace-tab-add:hover { color:var(--text-0); background:rgba(56,189,248,.12); }
</style>
