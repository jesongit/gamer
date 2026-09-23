import { api } from '../../../../../web/src/api'
import { GAMER_YAML_PLUGIN_ID, TEMPLATE_DIR } from '../../../../../web/src/gamer-plugin-ids'

/** 模板 base64 原始字节解码；服务端资源钩子负责 PNG 校验与归一化。 */
export function templateBytes(dataB64) {
  const binary = atob(String(dataB64 || ''))
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i)
  return bytes
}

/** 与 api.js / gamer-yaml 动作保持一致的模板完整文件名组合规则。 */
export function composeTemplateName(shortName, region, preserveColor) {
  const raw = String(shortName || '').trim()
  const stem = raw.toLowerCase().endsWith('.png') ? raw.slice(0, -4) : raw
  let name = stem
  if (Array.isArray(region) && region.length === 4) {
    const toInt3 = v => String(Math.min(999, Math.round(v * 1000))).padStart(3, '0')
    name += `#${region.map(toInt3).join('_')}`
  }
  if (preserveColor) name += '#1'
  return `${name}.png`
}

/** 去掉模板区域/颜色后缀，供视频制作入口按短名查找已有模板。 */
export function templateShortName(name) {
  const withoutColor = String(name || '').replace(/#1(\.(png|jpe?g))$/i, '$1')
  return withoutColor.replace(/#[^#./\\]+(\.(png|jpe?g))$/i, '$1')
}

/**
 * 模板统一写入原语：创建不 force，替换必须携带当前资源版本。
 * 服务端 PUT 会先执行 gamer-yaml 字节校验/灰度归一化，成功后再条件写入。
 */
export function putTemplateBytes(name, dataOrB64, packageId, expectedVersion, newName) {
  const options = expectedVersion ? { expectedVersion } : {}
  if (newName && newName !== name) options.newPath = `${TEMPLATE_DIR}/${newName}`
  return api.putPluginResourceBytes(
    packageId,
    GAMER_YAML_PLUGIN_ID,
    `${TEMPLATE_DIR}/${name}`,
    dataOrB64 instanceof Uint8Array ? dataOrB64 : templateBytes(dataOrB64),
    options,
  )
}

/**
 * 列表接口对二进制 PNG 可能没有 version；GET 返回的 ETag 即服务端内容
 * 版本，后续 PUT 做条件检查，不要求浏览器具备 WebCrypto 安全上下文。
 */
export async function resolveTemplateVersion(name, packageId, knownVersion) {
  if (knownVersion) return knownVersion
  const response = await api.getPluginResource(
    packageId,
    GAMER_YAML_PLUGIN_ID,
    `${TEMPLATE_DIR}/${name}`,
  )
  const etag = response?.headers?.get?.('ETag')?.replace(/^"|"$/g, '')
  if (/^[0-9a-f]{12}$/.test(etag || '')) return etag
  throw new Error('服务端未返回模板版本，请更新服务后重试')
}
