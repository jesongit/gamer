import KeymapPanel from './src/components/console/KeymapPanel.vue'
export { KeymapPanel }
export { useConsoleKeymap } from './src/components/console/useConsoleKeymap.js'
export const sdkVersion = 1
export const panels = {
  'console.keymaps': { component: KeymapPanel, panelClass: 'extra-tab', aliases: ['keymap'], getProps: context => ({ context: context.keymap }) },
}
