import { reactive } from 'vue'

/**
 * 自动化编辑器跨面板导航桥（Phase 7 §10.3，动作清单 automation.open_editor 的
 * 前端契约实现）。
 *
 * 视频工作台的草稿保存成功后请求打开/定位 YAML 编辑器。视频面板与
 * gamer.yaml 面板是两个独立 core 组件（registry 契约 = 自包含、互不持有引用），
 * 因此导航走模块级单例 + 序号去重（与 useConsoleStage 的 stageMediaRequest
 * 同一模式）：
 * - 发起方（video/VideoDraft.vue）：requestAutomationEditor(packageId, scriptId)
 *   + 路由切到 gamer.yaml:automation 面板；
 * - 消费方（useConsoleScriptRunner）：watch 序号 → 刷新脚本列表 → 选中目标
 *   → 进入编辑态。
 * 只动面板/编辑器选择，不改 deviceId / androidPackageName / currentPackageId
 * （四 Context 命名纪律，plan §39）。
 */
export const automationEditorRequest = reactive({ seq: 0, packageId: '', scriptId: '' })

/** 请求打开/定位自动化编辑器到指定脚本（资源 id = `<package-id>/<路径>.yaml`）。 */
export function requestAutomationEditor(packageId, scriptId) {
  const pkg = String(packageId || '')
  const id = String(scriptId || '')
  if (!pkg || !id) return
  automationEditorRequest.seq += 1
  automationEditorRequest.packageId = pkg
  automationEditorRequest.scriptId = id
}
