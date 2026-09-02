<template>
  <ConsoleVideoStage v-bind="$attrs" />
</template>

<script setup>
import { provide } from 'vue'
import ConsoleVideoStage from '../components/console/ConsoleVideoStage.vue'
import { DEVICE_STAGE_BRIDGE_KEY } from './context'

defineOptions({ inheritAttrs: false })
const props = defineProps({ bridge: { type: Object, default: null } })
provide(DEVICE_STAGE_BRIDGE_KEY, props.bridge)
</script>
