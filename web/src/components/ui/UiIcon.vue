<template>
  <svg class="ui-icon" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path :d="paths[name] || paths.more" /></svg>
</template>
<script setup>
defineProps({ name: { type: String, default: 'more' } })
const paths = {
  edit: 'm4 16-1 5 5-1L21 7l-4-4ZM14 6l4 4',
  pause: 'M7 4v16M17 4v16',
  upload: 'M12 16V3m-5 5 5-5 5 5M3 15v6h18v-6',
  record: 'M12 4a8 8 0 1 0 0 16 8 8 0 0 0 0-16',
  phone: 'M6 2h12v20H6ZM10 18h4',
  connect: 'M8 3v4m8-4v4M5 7h14v3a7 7 0 0 1-14 0ZM12 17v4',
  disconnect: 'M8 3v3m8-3v3M6 10a6 6 0 0 0 10 4M18 10V7h-6M12 17v4M3 3l18 18',
  keyboard: 'M3 5h18v14H3ZM6 9h.01M10 9h.01M14 9h.01M18 9h.01M6 12h.01M10 12h.01M14 12h.01M18 12h.01M7 16h10',
  more: 'M5 12h.01M12 12h.01M19 12h.01',
  play: 'm8 5 11 7-11 7Z', stop: 'M6 6h12v12H6Z',
  save: 'M5 3h12l4 4v14H3V3h2Zm2 0v7h10V3M7 21v-7h10v7',
  refresh: 'M20 7v5h-5M4 17v-5h5M5.5 7a8 8 0 0 1 13-2L20 8M4 16l1.5 3a8 8 0 0 0 13-2',
  settings: 'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8M12 2v3m0 14v3M2 12h3m14 0h3M5 5l2 2m10 10 2 2M5 19l2-2M17 7l2-2',
  image: 'M3 3h18v18H3ZM3 16l6-6 5 5 3-3 4 4M16 7h.01',
  copy: 'M9 9h12v12H9ZM15 9V3H3v12h6',
  close: 'm6 6 12 12M6 18 18 6', plus: 'M12 5v14M5 12h14',
  up: 'm6 14 6-6 6 6', down: 'm6 10 6 6 6-6',
  trash: 'M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7m4-7v7',
  crop: 'M7 3v14h14M3 7h14v14', point: 'M12 2v5m0 10v5M2 12h5m10 0h5M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10',
  expand: 'M8 3H3v5m13-5h5v5M3 16v5h5m8 0h5v-5',
  info: 'M12 8h.01M12 11v6M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20',
  back: 'm10 5-7 7 7 7M3 12h18',
}
</script>
<style scoped>.ui-icon{width:16px;height:16px;flex:none;vertical-align:middle}</style>
