<template>
  <router-view />
  <ConfirmDialogHost />
  <UpdateRecoveryBanner />
</template>

<script setup>
import UpdateRecoveryBanner from './components/UpdateRecoveryBanner.vue'
import ConfirmDialogHost from './components/ui/ConfirmDialogHost.vue'
</script>

<style>
@import './styles/theme.css';
</style>
