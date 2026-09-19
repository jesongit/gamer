<template>
  <router-view />
  <ConfirmDialogHost />
</template>

<script setup>
import ConfirmDialogHost from './components/ui/ConfirmDialogHost.vue'
</script>

<style>
@import './styles/theme.css';
</style>
