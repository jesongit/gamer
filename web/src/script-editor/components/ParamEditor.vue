<template>
  <div class="param-editor" data-testid="param-editor" @click.stop>
    <div class="pe-head">
      <span class="pe-title">脚本参数</span>
      <span class="pe-sub">{{ rows.length }} 个参数 · {{ rows.filter((p) => p.default !== null).length }} 个有默认值</span>
      <button type="button" class="mini-btn add" @click="addParam">+ 添加参数</button>
    </div>

    <div v-if="isFunctionLibrary" class="pe-hint warn">函数库没有文件级 params——请在具体函数下编辑（命令栈扩展后开放）</div>

    <template v-else>
      <div v-if="rows.length === 0" class="pe-hint">暂无参数。脚本可不声明参数；声明后可在步骤与运行表单中引用 $名称。</div>

      <div v-for="(decl, i) in rows" :key="i" class="param-row" :class="{ 'row-error': rowErrors(decl, i).length }">
        <div class="row-main">
          <select
            class="cell-input" :value="decl.type" aria-label="参数类型"
            @change="setType(i, ($event.target as HTMLSelectElement).value as ParamType)"
          >
            <option v-for="t in PARAM_TYPES" :key="t" :value="t">{{ TYPE_LABELS[t] }}</option>
          </select>
          <input
            class="cell-input" :value="decl.name" placeholder="变量名" aria-label="变量名"
            @change="setName(i, ($event.target as HTMLInputElement).value)"
          />
          <input
            class="cell-input grow" :value="decl.remark" placeholder="备注（不能用半角冒号）" aria-label="备注"
            @change="setRemark(i, ($event.target as HTMLInputElement).value)"
          />
          <label class="field-check" title="开启后调用/运行可省略此参数">
            <input
              type="checkbox" :checked="decl.default !== null"
              @change="toggleDefault(i, ($event.target as HTMLInputElement).checked)"
            />
            有默认值
          </label>
          <span class="row-actions">
            <button type="button" class="mini-btn" title="上移" :disabled="i === 0" @click="moveRow(i, -1)">↑</button>
            <button type="button" class="mini-btn" title="下移" :disabled="i >= rows.length - 1" @click="moveRow(i, 1)">↓</button>
            <button type="button" class="mini-btn danger" title="删除参数" @click="removeParam(i)">✕</button>
          </span>
        </div>
        <div v-if="decl.default !== null" class="row-default">
          <span class="field-label">默认值</span>
          <CellEditor
            :cell="lit(decl.default)" :type="decl.type" :allow-ref="false"
            :label="`${decl.name} 默认值`" :error="defaultError(decl)"
            @change="(c) => setDefault(i, c)"
          />
        </div>
        <div v-for="(msg, ei) in rowErrors(decl, i)" :key="ei" class="row-err-msg">{{ msg }}</div>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
/**
 * 参数编辑器（plan §9 params 行）：类型下拉 / 变量名 / 备注 / 有无默认值开关 /
 * 类型化默认值控件（复用 CellEditor 七类控件，禁止引用参数）/ 上下移排序。
 * 即时命名/重复/默认值校验提示；全部写操作经 CommandStack
 * （insert_param / update_param / remove_param / set_params）。
 *
 * 注意：命令栈的 params 命令只覆盖脚本文件级参数（函数库逐函数 params 待阶段 4 扩展），
 * 函数库模型挂载时显示提示。
 */
import { computed, type PropType } from 'vue'
import type { EditorModel } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { lit, PARAM_TYPES, type ParamDecl, type ParamType, type ScriptModel } from '../model'
import { checkCellLiteral, PARAM_NAME_RE } from '../schema'
import CellEditor from './CellEditor.vue'

const props = defineProps({
  model: { type: Object as PropType<EditorModel>, required: true },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
  /** 外部诊断（可传 validateScript 结果，step_path 形如 params[0]；此处仅标红整行）。 */
  diagnostics: { type: Array as PropType<Diagnostic[]>, default: () => [] },
})

const TYPE_LABELS: Record<ParamType, string> = {
  tmpl: '模板', coord: '坐标', color: '颜色', time: '时间', key: '按键', text: '文本', bool: '布尔',
}

const DEFAULT_LITERALS: Record<ParamType, unknown> = {
  tmpl: '', coord: [0.5, 0.5], color: 'ff8800', time: '1s', key: 'BACK', text: '', bool: true,
}

const isFunctionLibrary = computed(() => 'functions' in props.model)
const rows = computed<ParamDecl[]>(() => (isFunctionLibrary.value ? [] : (props.model as ScriptModel).params))

function updateParam(index: number, decl: ParamDecl): boolean {
  return props.stack.apply({ type: 'update_param', index, decl }, '编辑参数')
}

function addParam(): void {
  props.stack.apply(
    { type: 'insert_param', index: rows.value.length, decl: { type: 'text', name: '', remark: '', default: null } },
    '添加参数',
  )
}
function removeParam(index: number): void {
  props.stack.apply({ type: 'remove_param', index }, '删除参数')
}
function moveRow(index: number, dir: -1 | 1): void {
  const next = [...rows.value]
  const tmp = next[index]!
  next[index] = next[index + dir]!
  next[index + dir] = tmp
  props.stack.apply({ type: 'set_params', params: next as ParamDecl[] }, '参数排序')
}
function setName(i: number, raw: string): void {
  updateParam(i, { ...rows.value[i]!, name: raw.trim() })
}
function setRemark(i: number, raw: string): void {
  updateParam(i, { ...rows.value[i]!, remark: raw })
}
function setType(i: number, type: ParamType): void {
  if (type === rows.value[i]!.type) return
  // 类型切换：默认值按新类型不再合法，重置为无默认值（引用与调用实参由校验器报错传播）
  updateParam(i, { ...rows.value[i]!, type, default: null })
}
function toggleDefault(i: number, on: boolean): void {
  const decl = rows.value[i]!
  updateParam(i, { ...decl, default: on ? (DEFAULT_LITERALS[decl.type] as ParamDecl['default']) : null })
}
function setDefault(i: number, cell: { lit?: unknown; ref?: string }): void {
  updateParam(i, { ...rows.value[i]!, default: (cell.lit ?? null) as ParamDecl['default'] })
}

// ---------- 即时校验提示 ----------

function rowErrors(decl: ParamDecl, i: number): string[] {
  const errs: string[] = []
  if (!PARAM_NAME_RE.test(decl.name)) {
    errs.push(`变量名 ${decl.name || '(空)'} 不符合 [A-Za-z_][A-Za-z0-9_]*`)
  }
  if (decl.name && rows.value.some((p, j) => j !== i && p.name === decl.name)) {
    errs.push(`变量名 ${decl.name} 重复`)
  }
  const de = defaultError(decl)
  if (de) errs.push(de)
  return errs
}

function defaultError(decl: ParamDecl): string {
  if (decl.default === null) return ''
  const err = checkCellLiteral(decl.type, decl.default)
  return err ? `默认值不合法：${err.message}` : ''
}
</script>

<style scoped>
.param-editor {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  padding: 8px 10px;
  margin: 6px 0;
}
.pe-head { display: flex; align-items: center; gap: 8px; margin-bottom: 6px; }
.pe-title { font-weight: 600; font-size: 13px; }
.pe-sub { font-size: 12px; color: var(--text-2); flex: 1; }
.pe-hint { font-size: 12px; color: var(--text-2); padding: 4px 0; }
.pe-hint.warn { color: var(--warn); }
.param-row {
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 6px 8px; margin: 6px 0; display: flex; flex-direction: column; gap: 4px;
}
.param-row.row-error { border-color: var(--danger); }
.row-main { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.row-default { display: flex; align-items: center; gap: 6px; padding-left: 4px; }
.row-actions { display: inline-flex; gap: 3px; margin-left: auto; }
.row-err-msg { font-size: 11px; color: var(--danger); }
.field-label { font-size: 12px; color: var(--text-2); min-width: 44px; }
.field-check { display: inline-flex; align-items: center; gap: 4px; font-size: 12px; color: var(--text-1); cursor: pointer; }
.cell-input {
  background: var(--bg-2); color: var(--text-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 3px 6px; font-size: 12px; min-width: 60px;
}
.cell-input:focus { outline: none; border-color: var(--accent); }
.cell-input.grow { flex: 1; min-width: 120px; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 2px 6px; cursor: pointer;
}
.mini-btn:hover:not(:disabled) { color: var(--accent); border-color: var(--accent); }
.mini-btn:disabled { opacity: .35; cursor: not-allowed; }
.mini-btn.danger:hover:not(:disabled) { color: var(--danger); border-color: var(--danger); }
.mini-btn.add { color: var(--accent-2); }
</style>
