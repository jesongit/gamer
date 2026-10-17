<template>
  <div class="target-editor">
    <span>适用的游戏或网页</span>
    <label class="target-row"><span>Android 包名</span><input :value="form.androidPackagesText" class="input mono" placeholder="com.example.game（多个用逗号分隔）" @input="form.androidPackagesText = $event.target.value" /></label>
    <label class="target-row"><span>网页 URL 前缀</span><textarea :value="form.webUrlPrefixesText" class="input mono" rows="2" placeholder="https://game.example/play（每行一个）" @input="form.webUrlPrefixesText = $event.target.value" /></label>
    <span class="target-hint">至少填写一项；Android 的 * 只匹配 Android。网页按站点和路径匹配，不含 ? 参数或 # 片段。</span>
    <button type="button" class="btn btn-sm" :disabled="busy" @click="$emit('add-current')">{{ busy ? '读取中…' : '从当前目标添加' }}</button>
    <button type="button" class="btn btn-sm" @click="form.androidPackagesText = '*'">用于全部 Android 应用</button>
  </div>
</template>
<script setup>
defineProps({ form: { type: Object, required: true }, busy: Boolean })
defineEmits(['add-current'])
</script>
<style scoped>
.target-editor{display:flex;flex-direction:column;gap:7px}.target-row{display:grid;grid-template-columns:108px minmax(0,1fr);align-items:start;gap:8px}.target-row>span{padding-top:7px}.target-hint{font-size:12px;color:var(--text-muted,#888)}.target-editor>.btn{align-self:flex-start}
.target-hint{color:var(--text-2)}@media(max-width:480px){.target-row{grid-template-columns:1fr;gap:4px}.target-row>span{padding-top:0}}
</style>
