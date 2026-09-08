// @vitest-environment happy-dom
import { describe, it, expect } from 'vitest'
import { zipSync, strToU8 } from 'fflate'
import {
  normalizeTemplateName, collectTemplateEntries, planTemplateImports, disambiguateName,
  TEMPLATE_MAX_INPUT_BYTES,
} from './console/template-upload'

function zipFile(entries) {
  return new Blob([zipSync(entries)])
}

describe('normalizeTemplateName：模板名清洗', () => {
  it('拍平路径取 stem、统一 .png 后缀', () => {
    expect(normalizeTemplateName('icons/login/button.png')).toBe('button.png')
    expect(normalizeTemplateName('首页.PNG')).toBe('首页.png')
    expect(normalizeTemplateName('no-ext')).toBe('no-ext.png')
  })

  it('文件名原样保留（含 #，按原文件名入库），只统一后缀', () => {
    expect(normalizeTemplateName('item#1.png')).toBe('item#1.png')
    expect(normalizeTemplateName('btn#123_0_456_789.jpg')).toBe('btn#123_0_456_789.png')
  })

  it('空 stem 兜底 template', () => {
    expect(normalizeTemplateName('.png')).toBe('template.png')
    expect(normalizeTemplateName('')).toBe('template.png')
  })
})

describe('collectTemplateEntries：图片/zip 分流提取', () => {
  it('图片文件 → 单条目，zip → 解压出图片条目（目录/隐藏/非图片忽略）', async () => {
    const png = new File([new Uint8Array([1, 2, 3])], '单图.png')
    const entries = await collectTemplateEntries(png)
    expect(entries).toHaveLength(1)
    expect(entries[0].name).toBe('单图.png')
    expect(Array.from(entries[0].bytes)).toEqual([1, 2, 3])

    const zip = zipFile({
      'icons/': new Uint8Array(0),
      'icons/登录.png': strToU8('png-bytes'),
      'icons/deep/btn.JPG': new Uint8Array([9]),
      'icons/.DS_Store': strToU8('junk'),
      'icons/readme.txt': strToU8('not image'),
      'icons/bomb#1.png': new Uint8Array([7]),
    })
    const fromZip = await collectTemplateEntries(new File([zip], '打包.zip'))
    expect(fromZip.map(e => e.name).sort()).toEqual(['bomb#1.png', 'btn.png', '登录.png'])
  })

  it('zip 内没有图片 → 报错；图片超限 → 报错', async () => {
    const empty = zipFile({ 'a.txt': strToU8('x') })
    await expect(collectTemplateEntries(new File([empty], '空.zip')))
      .rejects.toThrow('没有可用图片')

    const big = new File([new Uint8Array(TEMPLATE_MAX_INPUT_BYTES + 1)], 'big.png')
    await expect(collectTemplateEntries(big)).rejects.toThrow('超过单模板大小上限')
  })

  it('不认识的扩展名直接拒绝', async () => {
    await expect(collectTemplateEntries(new File([new Uint8Array([1])], 'virus.exe')))
      .rejects.toThrow('不支持的文件类型')
  })
})

// ---------- zip 文件名编码（中文 Windows 资源管理器 / WinRAR 的 GBK 包） ----------
// fflate zipSync 对非 ASCII 名恒置 UTF-8 标志（bit 11），造不出 GBK 包，须手工
// 按字节打包；GBK 字节常量用 Windows CP936 实测值。

const u16 = v => [v & 0xff, (v >> 8) & 0xff]
const u32 = v => [v & 0xff, (v >> 8) & 0xff, (v >> 16) & 0xff, (v >>> 24) & 0xff]
const cat = (...parts) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0))
  let o = 0
  for (const p of parts) { out.set(p, o); o += p.length }
  return out
}

/** stored（不压缩）zip 手工打包：显式控制每条的文件名字节与通用标志位 */
function rawZip(entries) {
  const locals = []
  const centrals = []
  let offset = 0
  for (const { nameBytes, flags = 0, data } of entries) {
    const local = cat(
      u32(0x04034b50), u16(20), u16(flags), u16(0), u16(0), u16(0),
      u32(0), u32(data.length), u32(data.length), u16(nameBytes.length), u16(0),
      nameBytes, data,
    )
    locals.push(local)
    centrals.push(cat(
      u32(0x02014b50), u16(20), u16(20), u16(flags), u16(0), u16(0), u16(0),
      u32(0), u32(data.length), u32(data.length),
      u16(nameBytes.length), u16(0), u16(0), u16(0), u16(0), u32(0), u32(offset),
      nameBytes,
    ))
    offset += local.length
  }
  const central = cat(...centrals)
  return cat(
    ...locals, central,
    u32(0x06054b50), u16(0), u16(0), u16(entries.length), u16(entries.length),
    u32(central.length), u32(offset), u16(0),
  )
}

// CP936：登录 / 任务 / 开始按钮 / 战斗
const GBK_NAME = {
  登录: [0xB5, 0xC7, 0xC2, 0xBC],
  任务: [0xC8, 0xCE, 0xCE, 0xF1],
  开始按钮: [0xBF, 0xAA, 0xCA, 0xBC, 0xB0, 0xB4, 0xC5, 0xA5],
  战斗: [0xD5, 0xBD, 0xB6, 0xB7],
}

describe('collectTemplateEntries：GBK 文件名 zip 修正', () => {
  it('无 UTF-8 标志的 GBK 名还原为中文（含目录拍平），字节原样保留', async () => {
    const zip = rawZip([
      { nameBytes: new Uint8Array([...GBK_NAME.登录, 0x2e, 0x70, 0x6e, 0x67]), data: new Uint8Array([1, 2, 3]) },
      { nameBytes: new Uint8Array([...GBK_NAME.任务, 0x2f, ...GBK_NAME.开始按钮, 0x2e, 0x70, 0x6e, 0x67]), data: new Uint8Array([9]) },
    ])
    const entries = await collectTemplateEntries(new File([zip], 'gbk.zip'))
    expect(entries.map(e => e.name).sort()).toEqual(['开始按钮.png', '登录.png'])
    const login = entries.find(e => e.name === '登录.png')
    expect(Array.from(login.bytes)).toEqual([1, 2, 3])
  })

  it('ASCII 名不受影响；GBK 解不动（坏字节）退回 Latin-1 名，条目不丢', async () => {
    const zip = rawZip([
      { nameBytes: new Uint8Array([0x62, 0x74, 0x6e, 0x2e, 0x70, 0x6e, 0x67]), data: new Uint8Array([1]) }, // btn.png
      { nameBytes: new Uint8Array([0xFF, 0xFE, 0x2e, 0x70, 0x6e, 0x67]), data: new Uint8Array([2]) }, // 非法 GBK
    ])
    const entries = await collectTemplateEntries(new File([zip], 'mixed.zip'))
    expect(entries.map(e => e.name).sort()).toEqual(['btn.png', 'ÿþ.png'])
  })

  it('中央目录损坏 → 退回 fflate Latin-1 解码（原行为，条目不丢）', async () => {
    const zip = rawZip([
      { nameBytes: new Uint8Array([...GBK_NAME.战斗, 0x2e, 0x70, 0x6e, 0x67]), data: new Uint8Array([7]) },
    ])
    // 破坏中央目录条目签名（EOCD 从尾部扫，fflate 解压不受影响）
    const broken = new Uint8Array(zip)
    const cdSig = [0x50, 0x4b, 0x01, 0x02]
    for (let i = 0; i + 4 <= broken.length; i++) {
      if (cdSig.every((b, j) => broken[i + j] === b)) { broken[i] = 0x00; break }
    }
    const entries = await collectTemplateEntries(new File([broken], 'broken.zip'))
    expect(entries).toHaveLength(1)
    expect(entries[0].name).not.toBe('战斗.png') // 乱码名兜底，不再有中文
    expect(Array.from(entries[0].bytes)).toEqual([7])
  })

  it('有 UTF-8 标志的名按 UTF-8 解（fflate 自产包/7-Zip/macOS 形态）', async () => {
    const nameBytes = new Uint8Array([0xE7, 0x99, 0xBB, 0xE5, 0xBD, 0x95, 0x2e, 0x70, 0x6e, 0x67]) // 登录.png UTF-8
    const zip = rawZip([{ nameBytes, flags: 0x800, data: new Uint8Array([1]) }])
    const entries = await collectTemplateEntries(new File([zip], 'utf8.zip'))
    expect(entries.map(e => e.name)).toEqual(['登录.png'])
  })
})

describe('normalizeTemplateName：控制符清洗', () => {
  it('C0/C1 控制符（GBK 回退名可见）替换为 _', () => {
    expect(normalizeTemplateName('bad\u0095name.png')).toBe('bad_name.png')
    expect(normalizeTemplateName('a\u001bb\u007fc.png')).toBe('a_b_c.png')
  })
})

describe('planTemplateImports：导入计划', () => {
  const e = (name, i = 0) => ({ name, bytes: new Uint8Array([i]), source: name })

  it('库内同名 → skip（不覆盖）；本批次内同名 → 消歧导入（并避开库内已有名）', () => {
    const plan = planTemplateImports(
      [e('login.png'), e('btn.png'), e('btn.png'), e('btn-2.png')],
      ['LOGIN.PNG', 'btn-2.png'],
    )
    expect(plan[0]).toMatchObject({ name: 'login.png', action: 'skip' })
    expect(plan[1]).toMatchObject({ name: 'btn.png', action: 'import' })
    // 第二个 btn.png 消歧为 btn-2 时发现库内已占用 → 顺延 btn-3
    expect(plan[2]).toMatchObject({ name: 'btn-3.png', action: 'import' })
    expect(plan[3]).toMatchObject({ name: 'btn-2.png', action: 'skip' })
  })

  it('disambiguateName 避开已占用名', () => {
    const taken = k => k === 'a-2.png' || k === 'a.png'
    expect(disambiguateName('a.png', taken)).toBe('a-3.png')
  })
})
