<template>
  <div class="param-editor" data-testid="param-editor" @click.stop>
    <div class="pe-head">
      <span class="pe-title">{{ functionPath ? '函数参数' : '脚本参数' }}</span>
      <span class="pe-sub">{{ rows.length }} 个参数 · {{ rows.filter((p) => p.default !== null).length }} 个有默认值</span>
      <span v-if="!expanded && errorCount" class="pe-err-badge" title="存在参数问题，展开查看">{{ errorCount }} 处问题</span>
      <button v-if="showAddButton" type="button" class="mini-btn add" @click="addParam">+ 添加参数</button>
      <button type="button" class="mini-btn" :title="expanded ? '收起参数列表' : '展开参数列表'" @click="expanded = !expanded">
        {{ expanded ? '收起 ▴' : '展开 ▾' }}
      </button>
    </div>

    <div v-if="isFunctionLibrary && !functionPath" class="pe-hint warn">函数库没有文件级 params——请先在画布选择函数后按函数编辑参数</div>

    <template v-else-if="expanded">
      <div v-if="rows.length === 0" class="pe-hint">暂无参数。声明后可在步骤 $引用 与运行表单中使用。</div>

      <div v-for="(decl, i) in rows" :key="i" class="param-row" :class="{ 'row-error': rowErrors(decl, i).length }">
        <div class="row-main">
          <select
            class="cell-input" :value="decl.type" aria-label="参数类型"
            @change="setType(i, ($event.target as HTMLSelectElement).value as ParamType)"
          >
            <option v-for="t in PARAM_TYPES" :key="t" :value="t">{{ TYPE_LABELS[t] }}</option>
            <option v-if="!isCanonical(decl)" :value="decl.type">{{ decl.type }}（原文）</option>
          </select>
          <input
            class="cell-input" :value="decl.name" placeholder="变量名" aria-label="变量名"
            @change="setName(i, ($event.target as HTMLInputElement).value)"
          />
          <input
            class="cell-input grow" :value="decl.remark" placeholder="备注" aria-label="备注"
            @change="setRemark(i, ($event.target as HTMLInputElement).value)"
          />
          <label class="field-check" title="开启后调用/运行可省略此参数">
            <input
              type="checkbox" :checked="decl.default !== null"
              @change="toggleDefault(i, ($event.target as HTMLInputElement).checked)"
            />
            有默认值
          </label>
          <button
            type="button" class="mini-btn" :title="decl.rawForm ? '转为映射形态（name/type/default/remark）' : '转为字符串声明形态（type:name:remark[:default]）'"
            @click="flipForm(i)"
          >{{ decl.rawForm ? '串' : '映射' }}</button>
          <span class="row-actions">
            <button type="button" class="mini-btn" title="上移" :disabled="i === 0" @click="moveRow(i, -1)">↑</button>
            <button type="button" class="mini-btn" title="下移" :disabled="i >= rows.length - 1" @click="moveRow(i, 1)">↓</button>
            <button type="button" class="mini-btn danger" title="删除参数" @click="removeParam(i)">✕</button>
          </span>
        </div>
        <div v-if="decl.default !== null" class="row-default">
          <span class="field-label">默认值</span>
          <CellEditor
            :cell="lit(decl.default)" :type="cellTypeOf(decl)" :allow-ref="false"
            :templates="templates"
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
 * 参数编辑器（v3，Program.params 一等公民 / 函数级 params）：
 * canonical 类型下拉（string/number/integer/boolean/enum）/ 变量名 / 备注 /
 * 有无默认值开关 / 类型化默认值控件（复用 CellEditor，禁止引用）/
 * 字符串 ⇄ 映射双形态切换（契约 §1） / 上下移排序。
 * 即时命名/重复/默认值校验提示；全部写操作经 CommandStack
 * （insert_param / update_param / remove_param / set_params）。
 *
 * 传 functionPath（['functions', 函数名, 'params']）时编辑函数级 params；
 * 缺省 = 脚本文件级。
 */
import { computed, ref, type PropType } from 'vue'
import type { EditorModel, Path } from '../commands'
import type { Diagnostic } from '../diagnostics'
import { lit, PARAM_TYPES, type ParamDecl, type ParamType, type Program } from '../model'
import { checkCellLiteral, PARAM_NAME_RE } from '../schema'
import CellEditor from './CellEditor.vue'

const props = defineProps({
  model: { type: Object as PropType<EditorModel>, required: true },
  stack: { type: Object as PropType<{ apply: (c: unknown, n?: string) => boolean }>, required: true },
  /** 外部诊断（可传 validateScript 结果，step_path 形如 params[0]；此处仅标红整行）。 */
  diagnostics: { type: Array as PropType<Diagnostic[]>, default: () => [] },
  /** 函数级 params 容器（['functions', 函数名, 'params']）；缺省 = 脚本文件级。 */
  functionPath: { type: Array as PropType<Path | null>, default: null },
  /** tmpl 参数默认值下拉的候选模板短名（由页面外壳注入，缺省无候选）。 */
  templates: { type: Array as PropType<string[]>, default: () => [] },
  /** 函数编辑外壳把添加参数按钮放到添加步骤按钮前时隐藏内部按钮。 */
  showAddButton: { type: Boolean, default: true },
})

const TYPE_LABELS: Record<ParamType, string> = {
  string: '文本', number: '数字', integer: '整数', boolean: '布尔', enum: '枚举',
}

const DEFAULT_LITERALS: Record<ParamType, unknown> = {
  string: '', number: 0, integer: 0, boolean: true, enum: '',
}

const isFunctionLibrary = computed(() => 'functions' in props.model)

function isCanonical(decl: ParamDecl): boolean {
  return (PARAM_TYPES as readonly string[]).includes(decl.type)
}

/** 参数类型 → 默认值编辑用的 Cell 类型（映射到 CellEditor 控件口径）。 */
function cellTypeOf(decl: ParamDecl): string {
  switch (decl.type) {
    case 'number': case 'integer': case 'float': case 'int': return 'number'
    case 'boolean': case 'bool': return 'bool'
    case 'string': case 'text': case 'enum': return 'text'
    default: return 'text' // 历史别名（time/key/color/coord 等）按字符串编辑
  }
}

/** 参数列表默认收起（头部摘要常驻），展开/收起由头部按钮切换。 */
const expanded = ref(false)
const rows = computed<ParamDecl[]>(() => {
  if (props.functionPath && props.functionPath.length === 3) {
    const fnName = props.functionPath[1]
    const fn = (props.model as { functions: { name: string; params: ParamDecl[] }[] }).functions
      .find((f) => f.name === fnName)
    return fn ? fn.params : []
  }
  return isFunctionLibrary.value ? [] : (props.model as Program).params
})

/** params 命令附加容器：函数级时携带 path，文件级缺省。 */
function paramCmd<T extends object>(cmd: T): T & { path?: Path } {
  return props.functionPath ? { ...cmd, path: props.functionPath } : cmd
}

function updateParam(index: number, decl: ParamDecl): boolean {
  return props.stack.apply(paramCmd({ type: 'update_param', index, decl }), '编辑参数')
}

function addParam(): void {
  expanded.value = true
  props.stack.apply(
    paramCmd({ type: 'insert_param', index: rows.value.length, decl: { type: 'string', name: '', remark: '', default: null, rawForm: false } }),
    '添加参数',
  )
}
function removeParam(index: number): void {
  props.stack.apply(paramCmd({ type: 'remove_param', index }), '删除参数')
}
function moveRow(index: number, dir: -1 | 1): void {
  const next = [...rows.value]
  const tmp = next[index]!
  next[index] = next[index + dir]!
  next[index + dir] = tmp
  props.stack.apply(paramCmd({ type: 'set_params', params: next as ParamDecl[] }), '参数排序')
}
function setName(i: number, raw: string): void {
  updateParam(i, { ...rows.value[i]!, name: raw.trim() })
}
function setRemark(i: number, raw: string): void {
  updateParam(i, { ...rows.value[i]!, remark: raw })
}
function setType(i: number, type: ParamType): void {
  if (type === rows.value[i]!.type) return
  // 类型切换：默认值按新类型不再合法，重置为无默认值
  updateParam(i, { ...rows.value[i]!, type, default: null, rawForm: false })
}
/** 字符串 ⇄ 映射形态切换：字段不变，仅改序列化形态（rawForm 串按 type:name:remark[:default] 重排）。 */
function flipForm(i: number): void {
  const decl = rows.value[i]!
  updateParam(i, { ...decl, rawForm: !decl.rawForm })
}
function toggleDefault(i: number, on: boolean): void {
  const decl = rows.value[i]!
  updateParam(i, { ...decl, default: on ? (DEFAULT_LITERALS[isCanonical(decl) ? decl.type as ParamType : 'string'] as ParamDecl['default']) : null })
}
function setDefault(i: number, cell: { lit?: unknown; ref?: string }): void {
  updateParam(i, { ...rows.value[i]!, default: (cell.lit ?? null) as ParamDecl['default'] })
}

// ---------- 即时校验提示 ----------

/** 收起态行错误不可见，头部以问题数徽标提示。 */
const errorCount = computed(() => rows.value.reduce((n, p, i) => n + rowErrors(p, i).length, 0))

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

defineExpose({ addParam })
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
.pe-err-badge {
  flex: none; font-size: 11px; color: var(--danger);
  border: 1px solid var(--danger); border-radius: 4px; padding: 0 5px;
}
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
