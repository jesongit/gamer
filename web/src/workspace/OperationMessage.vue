<template>
  <span v-if="message" class="operation-content" :class="message.tone">
    <span>{{ message.text }}</span>
    <button v-for="(action, i) in message.actions || []" :key="i" type="button" :title="action.copy !== undefined ? `复制 ${action.label}` : action.label" @click="$emit('perform', action)">{{ action.label }}</button>
  </span>
</template>
<script setup>
defineProps({ message: Object })
defineEmits(['perform'])
</script>
<style scoped>
.operation-content{display:inline-flex;align-items:center;gap:9px;white-space:nowrap;flex:none}.error{color:var(--danger)}.success{color:var(--ok)}.warn{color:var(--warn)}
button{border:0;background:none;color:inherit;font:inherit;padding:2px;cursor:pointer;text-decoration:underline;text-decoration-color:var(--border);text-underline-offset:3px}button:hover{color:var(--accent)}
</style>
