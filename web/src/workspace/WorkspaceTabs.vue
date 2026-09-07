<template>
  <nav class="workspace-tabs" aria-label="功能面板">
    <template v-for="panel in panels" :key="panel.key">
      <!-- 下拉二级菜单页签（市场/插件）：点页签弹菜单，菜单项才真正切换面板 -->
      <div v-if="panel.children && panel.children.length" class="workspace-dd">
        <button
          type="button"
          class="workspace-tab"
          :class="{ active: panel.key === activePanel }"
          :aria-selected="panel.key === activePanel"
          :aria-expanded="openKey === panel.key"
          :title="panel.title"
          role="tab"
          @click="toggle(panel.key)"
        >
          <span v-if="panel.icon" class="workspace-tab-icon" aria-hidden="true">{{ panel.icon }}</span>
          <span>{{ panel.title }}</span>
          <span class="workspace-dd-caret" aria-hidden="true">▾</span>
        </button>
        <span v-if="openKey === panel.key" class="workspace-dd-mask" @click="openKey = ''"></span>
        <div v-if="openKey === panel.key" class="workspace-dd-menu" role="menu">
          <button
            v-for="child in panel.children"
            :key="child.key"
            type="button"
            class="workspace-dd-item"
            :class="{ active: child.key === activeChild }"
            role="menuitem"
            @click="pick(child.key)"
          >
            <span class="workspace-dd-item-title">{{ child.title }}</span>
            <span v-if="child.hint" class="workspace-dd-item-hint">{{ child.hint }}</span>
          </button>
        </div>
      </div>
      <button
        v-else
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
    </template>
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
import { ref } from 'vue'

defineProps({
  panels: { type: Array, default: () => [] },
  activePanel: { type: String, default: '' },
  // 当前命中的二级菜单项 key（市场分区 / 业务面板 key），用于菜单项高亮
  activeChild: { type: String, default: '' },
})
const emit = defineEmits(['select', 'open-plugin-center'])
const openKey = ref('')

function toggle(key) {
  openKey.value = openKey.value === key ? '' : key
}
function pick(key) {
  openKey.value = ''
  emit('select', key)
}
</script>

<style scoped>
.workspace-tabs { display:flex; flex-shrink:0; border:1px solid var(--border); border-radius:var(--radius-sm); background:var(--bg-2); }
/* 下拉菜单要溢出页签条渲染，overflow 不再裁切；首尾页签补回圆角 */
.workspace-tab:first-child { border-top-left-radius:var(--radius-sm); }
.workspace-tab-add { border-top-right-radius:var(--radius-sm); }
.workspace-tab { flex:1; min-width:0; padding:7px 3px; border:0; border-left:1px solid var(--border); background:transparent; color:var(--text-1); font-size:12px; cursor:pointer; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
.workspace-tab:first-child { border-left:0; }
.workspace-tab:hover { color:var(--text-0); background:var(--bg-3); }
.workspace-tab.active { color:var(--accent); background:rgba(34,211,165,.14); font-weight:600; }
.workspace-tab-icon { margin-right:2px; }
.workspace-tab-add { flex:0 0 34px; color:var(--accent-2); font-size:17px; font-weight:600; }
.workspace-tab-add:hover { color:var(--text-0); background:rgba(56,189,248,.12); }

/* 下拉二级菜单（市场/插件），样式对齐 ScriptRunner more-dropdown */
.workspace-dd { flex:1; min-width:0; position:relative; display:flex; }
.workspace-dd .workspace-tab { border-left:1px solid var(--border); }
.workspace-dd-caret { margin-left:3px; font-size:9px; color:var(--text-2); }
.workspace-dd-mask { position:fixed; inset:0; z-index:20; }
.workspace-dd-menu { position:absolute; right:0; top:calc(100% + 4px); z-index:30; display:flex; flex-direction:column; min-width:132px; padding:4px; gap:2px; background:var(--bg-2); border:1px solid var(--border); border-radius:var(--radius-sm); box-shadow:0 8px 24px rgba(0,0,0,.4); }
.workspace-dd-item { display:flex; align-items:baseline; justify-content:space-between; gap:10px; text-align:left; white-space:nowrap; padding:6px 10px; border:none; background:none; border-radius:var(--radius-sm); color:var(--text-0); font-size:12px; cursor:pointer; }
.workspace-dd-item:hover { background:var(--bg-3); }
.workspace-dd-item.active { color:var(--accent); font-weight:600; }
.workspace-dd-item-hint { font-size:10px; color:var(--text-2); }
</style>
