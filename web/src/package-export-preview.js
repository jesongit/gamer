import { strFromU8, unzipSync } from 'fflate'
import {
  GAMER_YAML_PLUGIN_ID, KEYMAP_PLUGIN_ID, GAMER_VIDEO_PLUGIN_ID,
  AUTOMATION_DIR, TEMPLATE_DIR, KEYMAP_DIR, VIDEO_PROJECT_DIR, isFunctionLibraryFile,
} from './gamer-plugin-ids'

// 仅为清单提供显示名称；归档内容与是否收录均以服务端生成的文件为准。
function category(path) {
  if (path === 'package.toml') return '配置包信息'
  if (path === 'media/index.json') return '素材引用清单'
  if (path.startsWith('media/files/')) return '媒体原文件'
  if (path.startsWith('shared/')) return '共享文件'
  const [, plugin, directory] = path.split('/')
  if (plugin === GAMER_YAML_PLUGIN_ID && directory === AUTOMATION_DIR) {
    return isFunctionLibraryFile(path) ? '函数库文件' : '自动化脚本'
  }
  if (plugin === GAMER_YAML_PLUGIN_ID && directory === TEMPLATE_DIR) return '模板图片'
  if (plugin === KEYMAP_PLUGIN_ID && directory === KEYMAP_DIR) return '键盘映射'
  if (plugin === GAMER_VIDEO_PLUGIN_ID && directory === VIDEO_PROJECT_DIR) return '视频项目'
  return path.startsWith('plugins/') ? `插件资源 · ${plugin}` : '其他文件'
}

/** 读取实际归档的目录，只解压小型引用清单，不展开图片和视频。 */
export async function inspectPackageExport(blob) {
  const files = []
  const extracted = unzipSync(new Uint8Array(await blob.arrayBuffer()), {
    filter(entry) {
      if (!entry.name.endsWith('/')) {
        files.push({ path: entry.name, size: entry.originalSize, category: category(entry.name) })
      }
      return entry.name === 'media/index.json'
    },
  })
  if (!files.some(file => file.path === 'package.toml')) throw new Error('导出文件缺少配置包信息，请重试')
  const index = extracted['media/index.json']
  const entries = index ? JSON.parse(strFromU8(index)).entries : []
  if (!Array.isArray(entries)) throw new Error('素材清单格式错误，请重试')
  const groups = new Map()
  for (const file of files) groups.set(file.category, (groups.get(file.category) || 0) + 1)
  const seen = new Set()
  const totalBytes = entries.reduce((sum, entry) => {
    if (seen.has(entry.sha256)) return sum
    seen.add(entry.sha256)
    return sum + (Number(entry.size) || 0)
  }, 0)
  return {
    files, entries, totalBytes,
    groups: [...groups].map(([label, count]) => ({ label, count })),
    archiveBytes: blob.size,
  }
}
