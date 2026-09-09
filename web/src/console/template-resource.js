import { api } from '../api'
import { GAMER_YAML_PLUGIN_ID, TEMPLATE_DIR } from '../gamer-plugin-ids'

/** 模板 base64 原始字节解码；服务端资源钩子负责 PNG 校验与归一化。 */
export function templateBytes(dataB64) {
  const binary = atob(String(dataB64 || ''))
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i)
  return bytes
}

/** 与 api.js / gamer.yaml 动作保持一致的模板完整文件名组合规则。 */
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
 * 服务端 PUT 会先执行 gamer.yaml 字节校验/灰度归一化，成功后再条件写入。
 */
export function putTemplateBytes(name, dataOrB64, packageId, expectedVersion) {
  const options = expectedVersion ? { expectedVersion } : {}
  return api.putPluginResourceBytes(
    packageId,
    GAMER_YAML_PLUGIN_ID,
    `${TEMPLATE_DIR}/${name}`,
    dataOrB64 instanceof Uint8Array ? dataOrB64 : templateBytes(dataOrB64),
    options,
  )
}

/**
 * 列表接口对二进制 PNG 可能没有 version；读取当前字节后计算与服务端
 * PackageStore::write_binary 相同的短 SHA-256，仍由后续 PUT 做最终条件检查。
 */
export async function resolveTemplateVersion(name, packageId, knownVersion) {
  if (knownVersion) return knownVersion
  const response = await api.getPluginResource(
    packageId,
    GAMER_YAML_PLUGIN_ID,
    `${TEMPLATE_DIR}/${name}`,
  )
  if (!response || typeof response.arrayBuffer !== 'function') {
    throw new Error('无法读取模板当前内容以确认版本')
  }
  if (!globalThis.crypto?.subtle) {
    throw new Error('当前环境不支持模板版本校验')
  }
  const digest = await globalThis.crypto.subtle.digest('SHA-256', await response.arrayBuffer())
  return [...new Uint8Array(digest).slice(0, 6)]
    .map(byte => byte.toString(16).padStart(2, '0'))
    .join('')
}
