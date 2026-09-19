import { createApp } from 'vue'
import App from './App.vue'
import router from './router'
import { installDialogAccessibility } from './components/ui/dialog-accessibility'

createApp(App).use(router).mount('#app')

const disposeDialogAccessibility = installDialogAccessibility()
if (import.meta.hot) import.meta.hot.dispose(disposeDialogAccessibility)
