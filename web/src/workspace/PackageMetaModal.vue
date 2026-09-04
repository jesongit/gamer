<template>
  <div v-if="open" class="modal-mask" @click.self="emit('close')">
    <div class="modal pkg-meta-modal">
      <div class="modal-head">
        <span class="title">初始化游戏包信息</span>
        <button class="btn btn-ghost btn-sm" :disabled="saving" @click="emit('close')">✕</button>
      </div>
      <div class="modal-body">
        <p class="pkg-meta-desc">当前编辑区还没有导出过游戏包。先补齐 package.toml 元数据（保存后即可导出为 .gamerpkg）。</p>
        <label class="pkg-field">
          <span class="pkg-label">游戏包 ID</span>
          <input v-model="form.id" class="input mono" placeholder="如 com.example.mygame" :disabled="saving" />
        </label>
        <label class="pkg-field">
          <span class="pkg-label">名称（可选）</span>
          <input v-model="form.name" class="input" placeholder="展示用名称，可留空" :disabled="saving" />
        </label>
        <label class="pkg-field">
          <span class="pkg-label">版本</span>
          <input v-model="form.version" class="input mono" placeholder="1.0.0" :disabled="saving" />
        </label>
        <label class="pkg-field">
          <span class="pkg-label">Android Packages（逗号或换行分隔）</span>
          <textarea
            v-model="form.androidPackagesText"
            class="input mono pkg-android-input"
            rows="3"
            placeholder="com.example.mygame"
            :disabled="saving"
          ></textarea>
        </label>
        <div v-if="error" class="pkg-meta-error">{{ error }}</div>
      </div>
      <div class="modal-foot">
        <button class="btn" :disabled="saving" @click="emit('close')">取消</button>
        <button class="btn btn-primary" :disabled="saving" @click="emit('submit')">{{ saving ? '保存中…' : '保存' }}</button>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * 游戏包元数据初始化弹窗（导出前置步骤）：本地编辑区没有 package.toml 时，
 * 「导出」先弹本弹窗补齐元数据（PUT /api/workspace），保存成功后由宿主继续导出确认流程。
 * form 对象由宿主（useWorkspacePackages.metaModal.form）持有并直接双向绑定，与
 * DeviceSettingsModal 的 ctx.form 模式一致；400 服务端校验错误经 error prop 整体展示。
 */
defineProps({
  open: { type: Boolean, default: false },
  saving: { type: Boolean, default: false },
  error: { type: String, default: '' },
  form: { type: Object, required: true },
})

const emit = defineEmits(['submit', 'close'])
</script>

<style scoped>
.pkg-meta-modal { width: 440px; max-width: calc(100vw - 32px); }
.pkg-meta-desc { margin: 0; font-size: 12px; color: var(--text-2); line-height: 1.6; }
.pkg-field { display: flex; flex-direction: column; gap: 4px; }
.pkg-label { font-size: 12px; color: var(--text-1); }
.pkg-field .input { width: 100%; box-sizing: border-box; }
.pkg-android-input { resize: vertical; font-size: 12px; }
.pkg-meta-error {
  font-size: 12px; color: var(--danger);
  border: 1px solid var(--danger); border-radius: var(--radius-sm);
  padding: 5px 8px; word-break: break-all;
}
</style>
