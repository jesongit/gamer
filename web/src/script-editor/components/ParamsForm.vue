<template>
  <div class="params-form" data-testid="params-form">
    <div v-if="!params.length" class="pf-empty">未声明参数，可直接运行。</div>

    <div
      v-for="decl in params"
      :key="decl.name"
      class="pf-row"
      :class="{ 'pf-row-error': rowErrors(decl.name).length }"
    >
      <div class="pf-head">
        <span class="pf-type">{{ ARG_TYPE_LABELS[decl.type] }}</span>
        <span class="pf-name mono">${{ decl.name }}</span>
        <span v-if="decl.desc" class="pf-remark" :title="decl.desc">{{ decl.desc }}</span>
        <span class="pf-spacer"></span>
        <!-- 三态之一「使用默认值」：始终显示当前声明默认值（缓存建议不遮蔽），不进 args -->
        <span
          v-if="hasParamDefault(decl) && !isActive(decl.name)"
          class="pf-default mono"
          :title="'使用脚本默认值（提交时省略）'"
        >默认: {{ displayLiteral(decl.default) }}</span>
        <label v-if="hasParamDefault(decl)" class="pf-toggle" title="切换为显式覆盖（该值将随请求发送）">
          <input
            type="checkbox"
            :checked="isActive(decl.name)"
            :aria-label="`${decl.name} 覆盖默认值`"
            @change="toggleOverride(decl, ($event.target as HTMLInputElement).checked)"
          />
          覆盖
        </label>
        <span v-else class="pf-required" title="无默认值：必须显式提供">必填</span>
      </div>

      <div v-if="isActive(decl.name)" class="pf-editor">
        <textarea
          v-if="usesJsonEditor(decl.type)"
          class="cell-input json-input" rows="2" spellcheck="false"
          :value="jsonTextFor(decl.name, values[decl.name])" :aria-label="decl.name"
          @input="onJsonEdit(decl, ($event.target as HTMLTextAreaElement).value)"
        ></textarea>
        <input
          v-else-if="decl.type === 'number' || decl.type === 'integer'"
          class="cell-input num" type="number" :step="decl.type === 'integer' ? '1' : 'any'"
          :value="numberText(values[decl.name])" :aria-label="decl.name"
          @input="onNumberEdit(decl, $event)"
        />
        <CellEditor
          v-else
          :cell="{ lit: values[decl.name] }"
          :type="cellType(decl.type)"
          :allow-ref="false"
          :label="decl.name"
          :templates="templates"
          :error="rowErrors(decl.name)[0] || ''"
          @change="(c) => onEdit(decl.name, c)"
        />
        <template v-if="usesJsonEditor(decl.type) || decl.type === 'number' || decl.type === 'integer'">
          <div v-for="(message, index) in rowErrors(decl.name)" :key="`editor-error-${index}`" class="pf-err-msg">
            {{ message }}
          </div>
        </template>
      </div>
      <!-- 非覆盖态没有编辑器行内错误位：错误（如服务端 400 回填）直接列在行下 -->
      <template v-if="!isActive(decl.name)">
        <div v-for="(m, i) in rowErrors(decl.name)" :key="i" class="pf-err-msg">{{ m }}</div>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 运行参数表单（阶段 5，plan §12.1/§12.2/§12.3）：ParamDecl[] → 七类类型化控件。
 *
 * 每字段三态：
 * - 「使用默认值」（default 存在时的初始态）：预填显示当前声明默认值，不进 args；
 * - 显式覆盖（勾选「覆盖」/任务快照带入）：值进入 getArgs() 稀疏映射；
 * - 必填（default === null）：恒为覆盖态，未填/不合规时 validate() 阻断。
 *
 * 覆盖建议（props.suggestions，来自 localStorage 上次显式输入）只在切换/初始为覆盖态时
 * 预填编辑器，绝不遮蔽「默认:」展示的当前声明默认值——默认值变化对用户始终可见。
 * 客户端校验与服务端同规则（schema.checkCellLiteral）；服务端 400 诊断经 serverErrors
 * prop 按参数名标红。纯受控组件：不持有模型，宿主经 getArgs()/validate() 取值。
 */
import { reactive, watch, type PropType } from 'vue'
import type { ParamDecl } from '../model'
import {
  checkLiteral, hasParamDefault, missingLiteralForType, paramControlType,
} from '../schema'
import {
  ARG_TYPE_LABELS, cloneArg, fmtLiteral,
  type ArgFieldError,
} from '../params'
import CellEditor from './CellEditor.vue'

/** V1 参数类型 → CellEditor 控件类型。 */
function cellType(type: string): string {
  return paramControlType(type)
}

/** list/object/any 使用 JSON 值编辑；提交前不会把 JSON 文本写入 args。 */
function usesJsonEditor(type: string): boolean {
  return type === 'list' || type === 'object' || type === 'any'
}

const jsonDrafts = reactive<Record<string, string>>({})
const jsonErrors = reactive<Record<string, string>>({})

function jsonText(value: unknown): string {
  try {
    const text = JSON.stringify(value, null, 2)
    return text === undefined ? 'null' : text
  } catch {
    return 'null'
  }
}

function jsonTextFor(name: string, value: unknown): string {
  return Object.prototype.hasOwnProperty.call(jsonDrafts, name) ? jsonDrafts[name]! : jsonText(value)
}

function jsonShapeError(type: string, value: unknown): string {
  if (type === 'list' && !Array.isArray(value)) return '值必须是 JSON 数组'
  if (type === 'object' && (value === null || typeof value !== 'object' || Array.isArray(value))) {
    return '值必须是 JSON 对象'
  }
  return ''
}

function onJsonEdit(decl: ParamDecl, raw: string): void {
  jsonDrafts[decl.name] = raw
  let value: unknown
  try {
    value = raw.trim() === '' && decl.type === 'any' ? null : JSON.parse(raw)
  } catch {
    jsonErrors[decl.name] = '值必须是合法 JSON'
    return
  }
  const shapeError = jsonShapeError(decl.type, value)
  if (shapeError) {
    jsonErrors[decl.name] = shapeError
    return
  }
  values[decl.name] = value
  delete jsonDrafts[decl.name]
  delete jsonErrors[decl.name]
  delete clientErrors[decl.name]
  emitChange()
}

function numberText(value: unknown): string {
  return typeof value === 'number' && Number.isFinite(value) ? String(value) : ''
}

function onNumberEdit(decl: ParamDecl, event: Event): void {
  const raw = (event.target as HTMLInputElement).value
  const value = raw.trim() === '' ? null : Number(raw)
  if (value !== null && !Number.isFinite(value)) return
  values[decl.name] = value
  delete clientErrors[decl.name]
  emitChange()
}

function displayLiteral(value: unknown): string {
  return value === null ? 'null' : fmtLiteral(value)
}

function missingValue(decl: ParamDecl): unknown {
  // 保持现有文本参数空输入的交互；数字/结构化/布尔等类型使用真正的空值。
  return decl.type === 'string' ? '' : missingLiteralForType(decl.type)
}

function hasOwn(source: Record<string, unknown> | null | undefined, name: string): boolean {
  return !!source && Object.prototype.hasOwnProperty.call(source, name)
}

function suggestedValue(decl: ParamDecl): unknown {
  if (hasOwn(props.suggestions, decl.name)) return cloneArg(props.suggestions[decl.name])
  return hasParamDefault(decl) ? cloneArg(decl.default) : cloneArg(missingValue(decl))
}

const props = defineProps({
  params: { type: Array as PropType<ParamDecl[]>, required: true },
  /** 初始显式覆盖（任务 args 快照/重编辑带入）；键须为已声明参数名才生效。 */
  initialArgs: { type: Object as PropType<Record<string, unknown>>, default: () => ({}) },
  /** 覆盖建议（localStorage 上次显式输入）：仅覆盖态预填，不显示为默认值。 */
  suggestions: { type: Object as PropType<Record<string, unknown>>, default: () => ({}) },
  /** tmpl 控件候选（模板短名 datalist）。 */
  templates: { type: Array as PropType<string[]>, default: () => [] },
  /** 服务端 400 invalid_args 诊断按字段映射结果：参数名 → 消息列表。 */
  serverErrors: { type: Object as PropType<Record<string, string[]>>, default: () => ({}) },
})

const emit = defineEmits(['change'])

// ---- 覆盖态与取值（name → 类型化字面量；仅在 active 集合内才有意义） ----

const active = reactive<Record<string, boolean>>({})
const values = reactive<Record<string, unknown>>({})
const clientErrors = reactive<Record<string, string[]>>({})

function isActive(name: string): boolean {
  return !!active[name]
}

function rowErrors(name: string): string[] {
  return [
    ...(clientErrors[name] || []),
    ...(jsonErrors[name] ? [jsonErrors[name]!] : []),
    ...(props.serverErrors[name] || []),
  ]
}

/** 声明列表/初始覆盖变化 → 重建表单态（覆盖建议只影响初始预填，不反向写回 prop）。 */
function rebuild(): void {
  for (const k of Object.keys(active)) delete active[k]
  for (const k of Object.keys(values)) delete values[k]
  for (const k of Object.keys(clientErrors)) delete clientErrors[k]
  for (const k of Object.keys(jsonDrafts)) delete jsonDrafts[k]
  for (const k of Object.keys(jsonErrors)) delete jsonErrors[k]
  for (const decl of props.params) {
    const init = props.initialArgs?.[decl.name]
    if (!hasParamDefault(decl) || init !== undefined) {
      active[decl.name] = true
      values[decl.name] = init !== undefined
        ? cloneArg(init)
        : suggestedValue(decl)
    }
  }
  emitChange()
}

watch(() => [props.params, props.initialArgs], rebuild, { immediate: true })

function toggleOverride(decl: ParamDecl, on: boolean): void {
  if (on) {
    // 覆盖态初始值优先级：已有编辑 > 覆盖建议 > 当前声明默认值
    values[decl.name] = suggestedValue(decl)
    active[decl.name] = true
  } else {
    delete values[decl.name]
    active[decl.name] = false
  }
  delete jsonDrafts[decl.name]
  delete jsonErrors[decl.name]
  delete clientErrors[decl.name]
  emitChange()
}

function onEdit(name: string, cell: { lit?: unknown; ref?: string }): void {
  values[name] = cell.lit
  delete clientErrors[name]
  emitChange()
}

// ---- 取值 / 校验（宿主提交前调用） ----

/** 稀疏 args：仅覆盖态字段（「使用默认值」按声明省略，由服务端解析默认值）。 */
function getArgs(): Record<string, unknown> {
  const args: Record<string, unknown> = {}
  for (const decl of props.params) {
    if (isActive(decl.name)) args[decl.name] = cloneArg(values[decl.name])
  }
  return args
}

/** 完整采用值视图（任务快照对比用）：覆盖态=当前输入；默认态=当前声明默认值。 */
function effectiveArgs(): Record<string, unknown> {
  const eff: Record<string, unknown> = {}
  for (const decl of props.params) {
    eff[decl.name] = isActive(decl.name) ? cloneArg(values[decl.name]) : cloneArg(decl.default)
  }
  return eff
}

/** 客户端校验（与服务端 invalid_args 同规则）；错误按字段置红并返回。 */
function validate(): ArgFieldError[] {
  for (const k of Object.keys(clientErrors)) delete clientErrors[k]
  const errs: ArgFieldError[] = []
  for (const decl of props.params) {
    if (!isActive(decl.name)) continue
    const err = check(decl)
    if (err) {
      errs.push({ name: decl.name, message: err })
      ;(clientErrors[decl.name] ||= []).push(err)
    }
  }
  return errs
}

function check(decl: ParamDecl): string {
  if (decl.required && (values[decl.name] === '' || values[decl.name] === null || values[decl.name] === undefined)) {
    return `必填参数 ${decl.name} 不能为空`
  }
  const err = checkLiteral(decl.type, values[decl.name])
  return err ? err.message : ''
}

function emitChange(): void {
  emit('change', { args: getArgs(), effective: effectiveArgs() })
}

defineExpose({ getArgs, validate, effectiveArgs })
</script>

<style scoped>
.params-form { display: flex; flex-direction: column; gap: 6px; }
.pf-empty { font-size: 12px; color: var(--text-2); padding: 2px 0; }
.pf-row {
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 5px 8px; display: flex; flex-direction: column; gap: 4px;
  background: var(--bg-0);
}
.pf-row-error { border-color: var(--danger); }
.pf-head { display: flex; align-items: center; gap: 6px; min-width: 0; }
.pf-type {
  font-size: 11px; color: var(--accent-2); background: var(--bg-3);
  border-radius: 4px; padding: 1px 6px; flex: none;
}
.pf-name { font-size: 12px; color: var(--text-0); flex: none; }
.pf-remark {
  font-size: 11px; color: var(--text-2); overflow: hidden;
  text-overflow: ellipsis; white-space: nowrap; min-width: 0;
}
.pf-spacer { flex: 1; }
.pf-default { font-size: 11px; color: var(--text-2); flex: none; }
.pf-toggle {
  display: inline-flex; align-items: center; gap: 3px;
  font-size: 11px; color: var(--text-1); cursor: pointer; flex: none;
}
.pf-required {
  font-size: 11px; color: var(--warn); border: 1px solid var(--warn);
  border-radius: 4px; padding: 0 5px; flex: none;
}
.pf-editor { padding-left: 2px; }
.cell-input {
  background: var(--bg-2); color: var(--text-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 3px 6px; font-size: 12px; min-width: 60px;
}
.cell-input:focus { outline: none; border-color: var(--accent); }
.cell-input.num { width: 74px; }
.json-input { width: min(100%, 420px); min-height: 42px; resize: vertical; font-family: var(--mono); }
.pf-err-msg { font-size: 11px; color: var(--danger); }
.mono { font-family: var(--mono); }
</style>
