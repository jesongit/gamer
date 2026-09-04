<template>
  <div v-if="open" class="modal-mask" @click.self="emit('close')">
    <div class="modal pkg-edit-modal">
      <div class="modal-head">
        <span class="title">编辑游戏包</span>
        <button class="btn btn-ghost btn-sm" :disabled="starting" @click="emit('close')">✕</button>
      </div>
      <div class="modal-body">
        <p class="pkg-edit-msg">
          将 <span class="mono pkg-edit-hl">{{ id }}@{{ version }}</span>
          导入到 <span class="mono pkg-edit-hl">{{ target || '—' }}</span>
          编辑区，当前编辑区中的 Gamer 资源将被替换。
        </p>
        <!-- 当前分区不在该包 android_packages（理论少见）：列出全部合法 target 手动选择 -->
        <div v-if="showTargetPicker" class="pkg-edit-targets">
          <span class="pkg-edit-targets-title">选择导入目标编辑区：</span>
          <label v-for="t in targets" :key="t" class="pkg-edit-target">
            <input
              type="radio"
              name="pkg-edit-target"
              :value="t"
              :checked="target === t"
              :disabled="starting"
              @change="emit('update:target', t)"
            />
            <span class="mono">{{ t }}</span>
          </label>
        </div>
      </div>
      <div class="modal-foot">
        <button class="btn" :disabled="starting" @click="emit('close')">取消</button>
        <button class="btn btn-primary" :disabled="starting || !target" @click="emit('confirm')">
          {{ starting ? '导入中…' : '开始编辑' }}
        </button>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * 编辑确认弹窗：把已激活游戏包（id@version）导入到本地编辑区覆盖现有资源。
 * 常规场景 target 即当前分区，无选择步骤；当前分区不在包的 android_packages 时
 * （showTargetPicker）以 radio 列出全部合法 target（手写 label+input，无现成组件）。
 */
defineProps({
  open: { type: Boolean, default: false },
  starting: { type: Boolean, default: false },
  id: { type: String, default: '' },
  version: { type: String, default: '' },
  target: { type: String, default: '' },
  targets: { type: Array, default: () => [] },
  showTargetPicker: { type: Boolean, default: false },
})

const emit = defineEmits(['confirm', 'close', 'update:target'])
</script>

<style scoped>
.pkg-edit-modal { width: 440px; max-width: calc(100vw - 32px); }
.pkg-edit-msg { margin: 0; font-size: 13px; color: var(--text-0); line-height: 1.7; }
.pkg-edit-hl { color: var(--accent-2); word-break: break-all; }
.pkg-edit-targets { display: flex; flex-direction: column; gap: 6px; }
.pkg-edit-targets-title { font-size: 12px; color: var(--text-2); }
.pkg-edit-target {
  display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--text-0);
  cursor: pointer;
}
</style>
