<template>
  <div class="spicker">
    <select v-model="pkg" class="select mono sp-pkg" title="脚本包（data/scripts 下的文件夹，默认 default）">
      <option v-for="p in packages" :key="p" :value="p">{{ p }}</option>
    </select>
    <select v-model="sel" class="select mono sp-name" title="运行脚本">
      <option value="">选择脚本…</option>
      <option v-for="s in pkgScripts" :key="s.id" :value="s.id">{{ s.name }}</option>
    </select>
  </div>
</template>

<script setup>
import { ref, computed, watch } from 'vue'
import { scriptsData } from '../store'

const props = defineProps({
  modelValue: { type: String, default: '' }
})
const emit = defineEmits(['update:modelValue'])
const scripts = scriptsData

const pkg = ref('default')

const packages = computed(() => {
  const set = new Set(scripts.value.map(s => s.package))
  set.add('default')
  return [...set].sort((a, b) => (a === 'default' ? -1 : b === 'default' ? 1 : a.localeCompare(b)))
})
const pkgScripts = computed(() => scripts.value.filter(s => s.package === pkg.value))
const sel = computed({
  get: () => (pkgScripts.value.some(s => s.id === props.modelValue) ? props.modelValue : ''),
  set: v => emit('update:modelValue', v)
})

// 初始包跟随已选脚本（id 形如 "package/name.yaml"），否则 default
watch(() => props.modelValue, v => { if (v) pkg.value = v.split('/')[0] || 'default' }, { immediate: true })
// 切包 / 列表刷新后：当前选择不在包内时自动选中该包第一个脚本
watch(pkgScripts, list => {
  if (!list.some(s => s.id === props.modelValue)) emit('update:modelValue', list[0]?.id || '')
}, { immediate: true })
</script>

<style scoped>
.spicker { display: flex; gap: 8px; align-items: center; width: 100%; min-width: 0; }
/* 双下拉按 4:6 占满整行（flex-basis 0 时 grow 比例即宽度比例） */
.sp-pkg { flex: 4 1 0%; min-width: 0; }
.sp-name { flex: 6 1 0%; min-width: 0; }
</style>
