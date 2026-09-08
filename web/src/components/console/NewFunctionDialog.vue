<template>
  <div v-if="open" class="nfd-mask" @click.self="cancel">
    <div class="nfd-modal" role="dialog" aria-label="新建函数">
      <div class="nfd-title">新建函数</div>
      <div class="nfd-field">
        <span class="nfd-label">分类</span>
        <select
          v-if="mode === 'existing'"
          :value="category"
          class="select mono"
          @change="onSelectCategory"
        >
          <option v-for="c in categories" :key="c" :value="c">{{ c }}</option>
          <option value="__new__">＋ 新分类…</option>
        </select>
        <input
          v-else
          ref="newCategoryEl"
          v-model="category"
          class="input mono"
          placeholder="新分类名称"
          @keydown.enter.prevent="submit"
          @keydown.esc="backToExisting"
        />
      </div>
      <div class="nfd-field">
        <span class="nfd-label">函数名</span>
        <input ref="nameEl" v-model="name" class="input mono" placeholder="函数名（同分类内唯一）" @keydown.enter.prevent="submit" @keydown.esc="cancel" />
      </div>
      <div class="nfd-actions">
        <button type="button" class="btn btn-primary" :disabled="!canSubmit" @click="submit">{{ submitting ? '创建中…' : '创建并编辑' }}</button>
        <button type="button" class="btn" @click="cancel">取消</button>
      </div>
    </div>
  </div>
</template>

<script setup>
/**
 * 新建函数弹窗（函数面板去文件化）：用户只面对「分类 + 函数名」两个概念——
 * 分类即底层函数库文件（同类函数存同一 <分类>.yaml），此处不暴露文件概念。
 * 分类下拉选已有分类或切「＋ 新分类…」输入；提交经 create 事件交回函数面板
 * （已有分类 → 追加函数；新分类 → 建分类并落首函数）。
 */
import { computed, nextTick, ref, watch } from 'vue'

const props = defineProps({
  open: { type: Boolean, default: false },
  categories: { type: Array, default: () => [] },
  // 打开时默认选中的分类（通常 = 当前浏览的分类）
  defaultCategory: { type: String, default: '' },
  submitting: { type: Boolean, default: false },
})
const emit = defineEmits(['create', 'close'])

const mode = ref('existing') // existing（下拉选已有）| new（输入新分类）
const category = ref('')
const name = ref('')
const nameEl = ref(null)
const newCategoryEl = ref(null)

const canSubmit = computed(() => !!category.value.trim() && !!name.value.trim() && !props.submitting)

watch(() => props.open, openFlag => {
  if (!openFlag) return
  mode.value = props.categories.length ? 'existing' : 'new'
  category.value = props.defaultCategory || props.categories[0] || ''
  name.value = ''
  nextTick(() => {
    if (mode.value === 'new') newCategoryEl.value?.focus()
    else nameEl.value?.focus()
  })
}, { immediate: true })

function onSelectCategory(e) {
  if (e.target.value === '__new__') {
    mode.value = 'new'
    category.value = ''
    nextTick(() => newCategoryEl.value?.focus())
    return
  }
  category.value = e.target.value
}

/** 新分类输入 Esc：退回下拉（仍有可选分类时） */
function backToExisting() {
  if (!props.categories.length) return cancel()
  mode.value = 'existing'
  category.value = props.defaultCategory || props.categories[0]
}

function submit() {
  if (!canSubmit.value) return
  emit('create', { category: category.value.trim(), name: name.value.trim() })
}

function cancel() {
  emit('close')
}
</script>

<style scoped>
.nfd-mask { position: fixed; inset: 0; z-index: 90; display: flex; align-items: center; justify-content: center; background: rgba(8, 10, 16, .7); }
.nfd-modal { display: flex; flex-direction: column; gap: 12px; width: min(420px, 92vw); padding: 16px; background: var(--bg-1); border: 1px solid var(--border); border-radius: var(--radius); box-shadow: 0 12px 36px rgba(0, 0, 0, .5); }
.nfd-title { font-size: 14px; font-weight: 600; color: var(--text-0); }
.nfd-field { display: flex; align-items: center; gap: 8px; }
.nfd-label { flex: none; width: 44px; font-size: 12px; color: var(--text-2); }
.nfd-field .select, .nfd-field .input { flex: 1; min-width: 0; }
.nfd-actions { display: flex; gap: 8px; justify-content: flex-end; }
.mono { font-family: var(--mono); font-size: 12px; }
</style>
