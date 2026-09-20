import ScriptRunner from './src/components/console/ScriptRunner.vue'
import TemplateCapture from './src/components/console/TemplateCapture.vue'
export { ScriptRunner, TemplateCapture }
export { useConsoleScriptRunner } from './src/components/console/useConsoleScriptRunner.js'
export { useConsoleTemplates } from './src/components/console/useConsoleTemplates.js'
export { pushRunEvent } from './src/components/console/useRunEvents.js'
export { registerGamerYamlRunnerEditor } from './src/components/task/runner-editor'
export { default as TemplateCropModal } from './src/components/console/TemplateCropModal.vue'
export { default as RunParamsModal } from './src/components/RunParamsModal.vue'
export const sdkVersion = 1
export const panels = {
  'console.scripts': { component: ScriptRunner, panelClass: 'script-tab', aliases: ['script'], getProps: context => ({ context: context.scriptRunner?.scripts }) },
  'console.functions': { component: ScriptRunner, panelClass: 'script-tab', getProps: context => ({ context: context.scriptRunner?.functions }) },
  'console.templates': { component: TemplateCapture, panelClass: 'tpl-tab', getProps: context => ({ context: context.templateCapture }) },
}
