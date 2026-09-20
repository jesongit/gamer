import { pluginModule } from '../../workspace/plugin-module-loader'
import { GAMER_YAML_PLUGIN_ID } from '../../gamer-plugin-ids'
export const GAMER_YAML_RUNNER_ID = GAMER_YAML_PLUGIN_ID
export const resolveGamerYamlAppPackages = (_entrypoint, context) => ({
  android_package: String(context?.androidPackageName || '').trim(),
  content_package: String(context?.packageId || '').trim() || null,
})
export const registerGamerYamlRunnerEditor = () => pluginModule(GAMER_YAML_PLUGIN_ID).registerGamerYamlRunnerEditor()
export const registerBuiltinRunnerEditors = registerGamerYamlRunnerEditor
