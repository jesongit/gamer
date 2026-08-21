<template>
  <div class="spicker">
    <select v-if="!locked" v-model="innerPkg" class="select mono sp-pkg" title="应用分区（=设备配置的应用包名）">
      <option v-if="!packages.length" value="">（无脚本）</option>
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
  modelValue: { type: String, default: '' },
  // 传入则锁定该分区并隐藏包名下拉（Console 脚本页签：分区由页签顶部下拉统一控制）
  package: { type: String, default: '' }
})
const emit = defineEmits(['update:modelValue'])
const scripts = scriptsData

const locked = computed(() => !!props.package)
// 锁定时分区跟随 prop，否则用内部下拉选中值
const innerPkg = ref('')
const pkg = computed(() => props.package || innerPkg.value)

const packages = computed(() =>
  [...new Set(scripts.value.map(s => s.package))].sort((a, b) => a.localeCompare(b)))
const pkgScripts = computed(() => scripts.value.filter(s => s.package === pkg.value))
const sel = computed({
  get: () => (pkgScripts.value.some(s => s.id === props.modelValue) ? props.modelValue : ''),
  set: v => emit('update:modelValue', v)
})

// 初始分区跟随已选脚本（id 形如 "<pkg>/<name>.yaml"）
watch(() => props.modelValue, v => { if (v) innerPkg.value = v.split('/')[0] || '' }, { immediate: true })
// 无 prop 锁定且当前分区不在列表（含初始空值）时：跟随第一个分区
watch([packages, () => props.package], ([list, lock]) => {
  if (lock || list.includes(innerPkg.value)) return
  innerPkg.value = list[0] || ''
}, { immediate: true })
// 切分区 / 列表刷新后：当前选择不在分区内时自动选中该分区第一个脚本
watch(pkgScripts, list => {
  if (!list.some(s => s.id === props.modelValue)) emit('update:modelValue', list[0]?.id || '')
}, { immediate: true })
</script>

<style scoped>
.spicker { display: flex; gap: 8px; align-items: center; width: 100%; min-width: 0; }
/* 双下拉按 4:6 占满整行（flex-basis 0 时 grow 比例即宽度比例）；锁定分区时仅脚本下拉占满 */
.sp-pkg { flex: 4 1 0%; min-width: 0; }
.sp-name { flex: 6 1 0%; min-width: 0; }
.spicker > .sp-name:only-child { flex: 1 1 0%; }
</style>
