// 业务插件 id 的基础配置点：Core 层资源寻址（api.js 的
// `/api/packages/:pkg/plugins/:plugin/resources` URL 拼装）与扩展前端契约点
// （gamer-yaml-runner.js / gamer-keymap-extension.js）共用的最小字面量知识。
// 归属：ADR-11——插件 id 是扩展知识；本模块只承载 id 字面量本身，运行行为
// （runner 包装 / 运行态判定）仍归各扩展前端契约点。

/** gamer.yaml 扩展（YAML 自动化）：插件 id = runner 注册 id。 */
export const GAMER_YAML_PLUGIN_ID = 'gamer.yaml'

/** gamer.keymap 扩展（按键映射 WASM 运行时）：插件 id = 扩展注册 id。 */
export const KEYMAP_PLUGIN_ID = 'gamer.keymap'

/** gamer.video 扩展（视频工作台宿主预置插件）：Package 资源域插件 id。 */
export const GAMER_VIDEO_PLUGIN_ID = 'gamer.video'

/** gamer.yaml 插件内目录（plan §3：目录语义归插件定义，Core 不解释）。 */
export const AUTOMATION_DIR = 'automations'
export const FUNCTION_DIR = 'functions'
export const TEMPLATE_DIR = 'templates'
/** gamer.keymap 插件内目录。 */
export const KEYMAP_DIR = 'mappings'
/** gamer.video 插件内项目目录（Video Project schema 归插件定义，Core 不解释）。 */
export const VIDEO_PROJECT_DIR = 'projects'
