// The console consumes a fixed integration contract, never plugin source files.
import { pluginModule } from './plugin-module-loader'
import { GAMER_YAML_PLUGIN_ID, KEYMAP_PLUGIN_ID } from '../gamer-plugin-ids'
import { defineAsyncComponent } from 'vue'

export const TemplateCropModal = defineAsyncComponent(async () => pluginModule(GAMER_YAML_PLUGIN_ID).TemplateCropModal)
export const RunParamsModal = defineAsyncComponent(async () => pluginModule(GAMER_YAML_PLUGIN_ID).RunParamsModal)
export const useConsoleTemplates = options => pluginModule(GAMER_YAML_PLUGIN_ID).useConsoleTemplates(options)
export const useConsoleScriptRunner = options => pluginModule(GAMER_YAML_PLUGIN_ID).useConsoleScriptRunner(options)
export const useConsoleKeymap = options => pluginModule(KEYMAP_PLUGIN_ID).useConsoleKeymap(options)
export const pushRunEvent = (...args) => pluginModule(GAMER_YAML_PLUGIN_ID).pushRunEvent(...args)
export const isRemoteKeymapRunning = extensions => extensions?.some(item => item.id === KEYMAP_PLUGIN_ID && item.state === 'running') === true
