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
        <span v-if="decl.remark" class="pf-remark" :title="decl.remark">{{ decl.remark }}</span>
        <span class="pf-spacer"></span>
        <!-- 三态之一「使用默认值」：始终显示当前声明默认值（缓存建议不遮蔽），不进 args -->
        <span
          v-if="decl.default !== null && !isActive(decl.name)"
          class="pf-default mono"
          :title="'使用脚本默认值（提交时省略）'"
        >默认: {{ fmtLiteral(decl.default) }}</span>
        <label v-if="decl.default !== null" class="pf-toggle" title="切换为显式覆盖（该值将随请求发送）">
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
        <CellEditor
          :cell="{ lit: values[decl.name] }"
          :type="cellType(decl.type)"
          :allow-ref="false"
          :label="decl.name"
          :templates="templates"
          :error="rowErrors(decl.name)[0] || ''"
          @change="(c) => onEdit(decl.name, c)"
        />
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
import { checkCellLiteral } from '../schema'
import {
  ARG_DEFAULT_LITERALS, ARG_TYPE_LABELS, cloneArg, fmtLiteral,
  type ArgFieldError,
} from '../params'
import CellEditor from './CellEditor.vue'

/** 参数类型 → CellEditor 控件类型（canonical 五类 + 历史别名）。 */
function cellType(type: string): string {
  switch (type) {
    case 'number': case 'integer': case 'float': case 'int': return 'number'
    case 'boolean': case 'bool': return 'bool'
    case 'tmpl': return 'tmpl'
    case 'key': return 'key'
    case 'coord': return 'coord'
    case 'color': return 'text'
    case 'time': return 'time'
    default: return 'text' // string/enum/text
  }
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
  return [...(clientErrors[name] || []), ...(props.serverErrors[name] || [])]
}

/** 声明列表/初始覆盖变化 → 重建表单态（覆盖建议只影响初始预填，不反向写回 prop）。 */
function rebuild(): void {
  for (const k of Object.keys(active)) delete active[k]
  for (const k of Object.keys(values)) delete values[k]
  for (const k of Object.keys(clientErrors)) delete clientErrors[k]
  for (const decl of props.params) {
    const init = props.initialArgs?.[decl.name]
    if (decl.default === null || init !== undefined) {
      active[decl.name] = true
      values[decl.name] = init !== undefined
        ? cloneArg(init)
        : cloneArg(props.suggestions?.[decl.name] ?? ARG_DEFAULT_LITERALS[decl.type] ?? '')
    }
  }
  emitChange()
}

watch(() => [props.params, props.initialArgs], rebuild, { immediate: true })

function toggleOverride(decl: ParamDecl, on: boolean): void {
  if (on) {
    // 覆盖态初始值优先级：已有编辑 > 覆盖建议 > 当前声明默认值
    values[decl.name] = cloneArg(props.suggestions?.[decl.name] ?? decl.default)
    active[decl.name] = true
  } else {
    delete values[decl.name]
    active[decl.name] = false
  }
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
  const err = checkCellLiteral(decl.type, values[decl.name])
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
.pf-err-msg { font-size: 11px; color: var(--danger); }
.mono { font-family: var(--mono); }
</style>
