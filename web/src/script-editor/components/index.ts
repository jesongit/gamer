/**
 * 脚本编辑器组件层 barrel（阶段 3 后半）。
 *
 * 组件均为受控组件：model + CommandStack 由页面外壳（阶段 4 接入 Console/独立编辑页）持有，
 * 组件内所有写操作经 stack.apply 提交命令，undo/redo 与 uuid 稳定性由命令栈保证。
 */
export { default as StepCard } from './StepCard.vue'
export { default as CellEditor } from './CellEditor.vue'
export { default as BranchContainer } from './BranchContainer.vue'
export { default as StepCanvas } from './StepCanvas.vue'
export { default as AddStepPanel } from './AddStepPanel.vue'
export { default as ParamEditor } from './ParamEditor.vue'
export { default as ParamsForm } from './ParamsForm.vue'
export { default as DefaultsEditor } from './DefaultsEditor.vue'
export { default as ErrorSummary } from './ErrorSummary.vue'
export { default as YamlPreview } from './YamlPreview.vue'

export {
  KIND_META,
  stepSummary,
  cellShort,
  breadcrumbForContainer,
  basePathOfContainer,
  containerNesting,
  parseStepPath,
  locateDiagnostic,
  type KindMeta,
  type LocateResult,
} from './kinds'
