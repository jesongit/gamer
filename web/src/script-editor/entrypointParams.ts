/**
 * 服务端参数 schema descriptor（V1）→ ParamsForm 可渲染 ParamDecl[] 的纯适配层。
 * GET /api/runners/:runner_id/entrypoint 返回 `{kind, format:"yaml-params-v1", schema}`；
 * V1 的 schema 已是参数声明数组（name/type/required/default/desc），本层只做
 * 类型别名归一与形态收窄（前端不为取参数而解析 YAML）。旧 psig1 signature 字段已删除。
 */
import type { ParamDecl } from './model'
import { normalizeParamType, PARAM_TYPES } from './schema'

/** descriptor.schema 内单个参数声明（服务端 decls_schema_json 形态）。 */
export interface SchemaParamDecl {
  name?: string
  type?: string
  required?: boolean
  default?: unknown
  desc?: string
}

/** descriptor 内层载荷（API 外壳还带 runner_id/entrypoint，本适配层不消费）。 */
export interface EntrypointParamsDescriptor {
  kind?: string
  format?: string
  schema?: SchemaParamDecl[]
}

/** descriptor.schema → ParamDecl[]；schema 缺失/形态不符 → []（按「无参数」处理）。 */
export function schemaToParamDecls(schema: SchemaParamDecl[] | null | undefined): ParamDecl[] {
  if (!Array.isArray(schema)) return []
  const decls: ParamDecl[] = []
  for (const item of schema) {
    if (!item || typeof item !== 'object' || typeof item.name !== 'string' || item.name === '') continue
    const rawType = typeof item.type === 'string' ? normalizeParamType(item.type.trim()) : 'string'
    const type = (PARAM_TYPES as readonly string[]).includes(rawType) ? rawType : 'string'
    const hasDefault = item.default !== null && item.default !== undefined
    decls.push({
      name: item.name,
      type,
      required: item.required === true && !hasDefault,
      default: hasDefault ? item.default : null,
      desc: typeof item.desc === 'string' ? item.desc : '',
    })
  }
  return decls
}
