// 模板上传导入的纯逻辑（无 Vue/网络依赖，UI 层在 useConsoleTemplates）：
// - 输入文件分流：图片直取字节；zip 在浏览器端解压（fflate），压缩包字节本身
//   不上传，服务端不落任何临时文件（「清理压缩包」天然满足）；zip 文件名编码
//   修正见 readZipEntryNames（中文 Windows 打的 GBK 包，fflate 只能解出乱码）；
// - 名字清洗：文件名按原样保留（含 `#`——服务端 sanitize_segment 只把它当
//   普通文件名字符，脚本引用按全名精确解析），仅抹掉路径与 C0、C1 控制符；
//   统一 .png 后缀（服务端字节钩子会把任意格式归一化为 8-bit 灰度 PNG，
//   但不改名）；zip 内目录拍平，只取文件 stem；
// - 导入计划：已存在跳过（批量导入不静默覆盖，覆盖走模板行「替换」），zip 内
//   重名自动加序号，与既有库同名也跳过。
import { unzipSync } from 'fflate'

/** 服务端单模板字节上限（matcher.rs TEMPLATE_MAX_INPUT_BYTES），超限直接本地判失败，省一次请求。 */
export const TEMPLATE_MAX_INPUT_BYTES = 10 * 1024 * 1024

const IMAGE_EXT_RE = /\.(png|jpe?g|webp|bmp|gif|tiff?)$/i

// ---------- zip 文件名编码修正 ----------
// zip 规范里文件名本无编码字段，只有通用标志位 bit 11（0x800）声明 UTF-8。
// 中文 Windows 资源管理器 / WinRAR 打的包不带该标志（文件名字节 = GBK），fflate
// 对无标志条目按 Latin-1 解码 → 乱码名（含 »/½ 等非字母数字与 C1 控制符），服务端
// 资源路径校验（sanitize_segment 只放行 Unicode 字母数字与 . _ - # 空格）逐张 400。
// 这里自行遍历中央目录还原每条文件名的原始字节与标志位：有标志按 UTF-8；无标志
// 按 GBK 严格解码（fatal，坏字节不硬猜）；再失败退回 Latin-1（= fflate 的解码键，
// 保证按 key 取得到条目字节）。中央目录本身解不动（zip64 等）整体返回 null，
// 调用方退回 fflate 解码名（原行为）。

const b2 = (d, p) => d[p] | (d[p + 1] << 8)
const b4 = (d, p) => (d[p] | (d[p + 1] << 8) | (d[p + 2] << 16) | (d[p + 3] << 24)) >>> 0

const latin1 = bytes => {
  let s = ''
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i])
  return s
}

let gbkFatalDecoder = null
function decodeGbkStrict(bytes) {
  // TextDecoder 默认非 fatal（坏字节出 U+FFFD 不报错），必须显式 fatal 才能探测失败
  gbkFatalDecoder ||= new TextDecoder('gbk', { fatal: true })
  return gbkFatalDecoder.decode(bytes)
}

function decodeZipName(bytes, utf8Flag) {
  if (!bytes.some(b => b > 0x7f)) return latin1(bytes)
  if (utf8Flag) return new TextDecoder('utf-8').decode(bytes)
  try {
    return decodeGbkStrict(bytes)
  } catch {
    return latin1(bytes)
  }
}

/** 遍历中央目录：[{key: fflate 解码键, name: 修正后文件名}]；解不动返回 null */
function readZipEntryNames(bytes) {
  try {
    let e = bytes.length - 22
    for (; e >= 0 && b4(bytes, e) !== 0x06054b50; e--) {
      if (bytes.length - e > 65558) return null
    }
    if (e < 0) return null
    const count = b2(bytes, e + 10)
    let off = b4(bytes, e + 16)
    if (count === 0xffff || off === 0xffffffff) return null // zip64：不硬解
    const out = []
    for (let i = 0; i < count; i++) {
      if (off + 46 > bytes.length || b4(bytes, off) !== 0x02014b50) return null
      const fnl = b2(bytes, off + 28)
      const nameBytes = bytes.subarray(off + 46, off + 46 + fnl)
      const utf8Flag = b2(bytes, off + 8) & 0x800
      out.push({
        key: utf8Flag ? new TextDecoder('utf-8').decode(nameBytes) : latin1(nameBytes),
        name: decodeZipName(nameBytes, utf8Flag),
      })
      off += 46 + fnl + b2(bytes, off + 30) + b2(bytes, off + 32)
    }
    return out
  } catch {
    return null
  }
}

/**
 * 清洗一个候选模板名为合法入库名：
 * - 文件名原样保留（含 `#`：服务端资源路径校验把 `#` 与空格当普通字符放行，
 *   模板引用按全名精确解析，不会因 `#` 破坏寻址）；仅把路径/C0、C1 控制符
 *   替换为 `_`（C1 出现在 GBK 包解码失败的回退名里，服务端拒绝控制符）；
 * - 统一小写 .png 后缀；空 stem 兜底 `template`。
 */
export function normalizeTemplateName(fileName) {
  const dot = String(fileName || '').replace(/\\/g, '/').split('/').pop()
  const stem = dot.replace(/\.[^.]+$/, '').replace(/[\u0000-\u001f\u007f-\u009f]/g, '_').trim()
  return `${stem || 'template'}.png`
}

/**
 * 输入文件 → 待导入条目列表 [{name, bytes, source}]。
 * zip 解压失败抛错（UI toast 原始消息）；zip 内非图片/隐藏文件/目录条目忽略；
 * 单图超限抛错（消息带文件名与上限）。
 */
export async function collectTemplateEntries(file) {
  if (/\.zip$/i.test(file.name)) {
    const entries = []
    const raw = new Uint8Array(await file.arrayBuffer())
    const files = unzipSync(raw)
    // 名字以中央目录修正结果为准（GBK 包）；中央目录解不动时退回 fflate 解码名
    const fixed = readZipEntryNames(raw)
    const nameOf = fixed ? new Map(fixed.map(n => [n.key, n.name])) : null
    for (const [rawName, bytes] of Object.entries(files)) {
      const name = (nameOf?.get(rawName) ?? rawName).replace(/\\/g, '/')
      if (name.endsWith('/') || !IMAGE_EXT_RE.test(name)) continue
      const base = name.split('/').pop()
      if (base.startsWith('.') || base.startsWith('__MACOSX')) continue
      if (bytes.length > TEMPLATE_MAX_INPUT_BYTES) {
        throw new Error(`压缩包内图片超过单模板大小上限（${base}，上限 10MB）`)
      }
      entries.push({ name: normalizeTemplateName(base), bytes, source: `${file.name}/${name}` })
    }
    if (!entries.length) throw new Error(`压缩包 ${file.name} 里没有可用图片`)
    return entries
  }
  if (!IMAGE_EXT_RE.test(file.name)) throw new Error(`不支持的文件类型：${file.name}（支持图片或 zip 压缩包）`)
  if (file.size > TEMPLATE_MAX_INPUT_BYTES) {
    throw new Error(`图片超过单模板大小上限（${file.name}，上限 10MB）`)
  }
  return [{ name: normalizeTemplateName(file.name), bytes: new Uint8Array(await file.arrayBuffer()), source: file.name }]
}

/**
 * 计划一批条目的入库名单：与 `existingNames`（当前库内全名，小写比较）撞名的
 * 条目标记 skip（批量导入不静默覆盖，覆盖走模板行「替换」）；本批次内撞名
 * （zip 拍平后同名）自动 `-2` 起序号消歧后照常导入。
 * 返回 [{...entry, action: 'import' | 'skip', reason?}]。
 */
export function planTemplateImports(entries, existingNames = []) {
  const existing = new Set(existingNames.map(n => String(n).toLowerCase()))
  const used = new Set()
  const out = []
  for (const entry of entries) {
    const lower = entry.name.toLowerCase()
    if (existing.has(lower)) {
      out.push({ ...entry, action: 'skip', reason: '已存在同名模板' })
      continue
    }
    let name = entry.name
    if (used.has(lower)) {
      name = disambiguateName(entry.name, key => existing.has(key) || used.has(key))
      used.add(name.toLowerCase())
    } else {
      used.add(lower)
    }
    out.push({ ...entry, name, action: 'import' })
  }
  return out
}

/** zip 内撞名消歧：`login.png` → `login-2.png`（taken 命中即顺延）。 */
export function disambiguateName(name, taken) {
  const m = name.match(/^(.*)(\.[^.]+)$/)
  const stem = m ? m[1] : name
  const ext = m ? m[2] : ''
  let i = 2
  while (taken(`${stem}-${i}${ext}`.toLowerCase())) i++
  return `${stem}-${i}${ext}`
}
