// gamer.keymap 扩展（按键映射 WASM 运行时）的前端契约点：扩展注册 id 的唯一
// 配置源 + 远端映射运行态判定。
//
// 归属：ADR-11/13——扩展 id 是扩展知识，Core 壳（Console 视图 / workspace 接线）
// 一律不出现该字面量；输入路由层只消费本模块导出的判定结果（远端运行中 →
// 鼠标/滚轮/手柄输入交 keymap 控制器，否则直通 scrcpy）。本模块与
// gamer-yaml-runner.js（yaml 扩展的 runner id 配置点）同属扩展前端侧契约点。
// 注意：本模块不承载任何面板注册——映射面板由扩展 manifest（runtime = "core"
// + component = "console.keymaps"）经 core-component-registry 驱动。

/** gamer.keymap 扩展的注册 id（服务端 extensions 列表 / 生命周期动作目标）。 */
export const GAMER_KEYMAP_EXTENSION_ID = 'gamer.keymap'

/**
 * 扩展列表快照（GET /api/extensions 的 extensions 数组）→ gamer.keymap 是否
 * 处于 running。扩展禁用/卸载/未安装时列表中无该项或非 running，恒返回
 * false（输入自动回落直通 scrcpy，无需壳做任何特判清理）。
 */
export function isRemoteKeymapRunning(extensions) {
  const list = Array.isArray(extensions) ? extensions : []
  const snapshot = list.find(item => item?.id === GAMER_KEYMAP_EXTENSION_ID)
  return snapshot?.state === 'running'
}
