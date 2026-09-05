/**
 * 服务端参数 schema descriptor（P12.3 / 契约 §7）→ ParamsForm 可渲染 ParamDecl[]
 * 的纯适配层。GET /api/runners/:runner_id/entrypoint 返回
 * `{kind, format:"yaml-params-v1", schema, signature}`；schema 是参数唯一来源
 * （前端不为取参数而解析 YAML，T6 后端）。signature（psig1 参数签名）本期仅透传，
 * 供后续过期预检使用。
 *
 * 映射规则（schema property → ParamDecl）：
 * - type：优先透传 param_type（服务端原样回传的声明 ty 名，ParamsForm 的
 *   控件选择/标签/校验层按名字各司其职）；仅做两处规范化——
 *   `boolean`→`bool`、`template`→`tmpl`（checkCellLiteral / CellEditor 控件
 *   / ARG_DEFAULT_LITERALS 的键是 v2 七类名）；param_type 缺失（第三方 schema）
 *   时按 JSON Schema type 兜底：boolean→bool、integer/number 原名、
 *   array→coord（gamer.yaml 仅 coord 产出二元数值数组）、其余 string。
 * - default：property 带 default 键 → 值透传（服务端 normalize_v3_default_json
 *   已保证 JSON 形态：coord=[x,y]、bool=布尔、time=带单位串；非标量/非 [x,y]
 *   形态清洗为无默认）；否则 default=null（ParamsForm 视为必填恒覆盖态；
 *   schema.required 亦为权威来源，两者任一命中即必填）。
 * - description → remark（说明文案，行内展示）。
 * - enum：gamer.yaml 仅 key 型产出（服务端 KEY_NAMES ⊆ 前端 KEY_ENUM），由
 *   type=key 的既有枚举下拉承载，无需额外控件。
 * - 顺序：按 properties 键序（服务端按声明序插入，JS 对象保序）。
 */
import type { ParamDecl, ParamLiteral } from './model'

/** descriptor.schema 内单个参数 property（JSON Schema 形态 + param_type 扩展）。 */
export interface SchemaProperty {
  type?: string
  param_type?: string
  default?: unknown
  description?: string
  enum?: unknown[]
  items?: { type?: string; minItems?: number; maxItems?: number }
}

/** descriptor.schema（契约 §7：object + properties + required）。 */
export interface EntrypointSchema {
  type?: string
  properties?: Record<string, SchemaProperty>
  required?: string[]
}

/** descriptor 内层载荷（API 外壳还带 runner_id/entrypoint，本适配层不消费）。 */
export interface EntrypointParamsDescriptor {
  kind?: string
  format?: string
  schema?: EntrypointSchema
  signature?: string
}

/** param_type / JSON Schema type → ParamDecl.type（规则见模块头注释）。 */
export function schemaParamType(prop: SchemaProperty): string {
  const pt = typeof prop?.param_type === 'string' ? prop.param_type.trim() : ''
  if (pt) {
    if (pt === 'boolean') return 'bool'
    if (pt === 'template') return 'tmpl'
    return pt
  }
  switch (prop?.type) {
    case 'boolean': return 'bool'
    case 'integer': return 'integer'
    case 'number': return 'number'
    case 'array': return 'coord'
    default: return 'string'
  }
}

/** default 值清洗：仅保留 JSON 标量 / [x,y] 数组形态，其余（对象/null）按无默认处理。 */
function sanitizeDefault(value: unknown): ParamLiteral | [number, number] | null {
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return value
  if (Array.isArray(value) && value.length === 2 && value.every((n) => Number.isFinite(n))) {
    return [value[0], value[1]]
  }
  return null
}

/** schema → 参数声明列表；schema 缺失/形态不符 → []（调用方按「无参数」处理）。 */
export function schemaToParamDecls(schema: EntrypointSchema | null | undefined): ParamDecl[] {
  const properties = schema?.properties
  if (!properties || typeof properties !== 'object') return []
  const required = Array.isArray(schema?.required) ? schema.required : []
  const decls: ParamDecl[] = []
  for (const [name, prop] of Object.entries(properties)) {
    if (!prop || typeof prop !== 'object') continue
    const hasDefault = Object.prototype.hasOwnProperty.call(prop, 'default')
    const isRequired = required.includes(name) || !hasDefault
    decls.push({
      type: schemaParamType(prop),
      name,
      remark: typeof prop.description === 'string' ? prop.description : '',
      default: isRequired ? null : sanitizeDefault(prop.default),
      rawForm: false,
    })
  }
  return decls
}
