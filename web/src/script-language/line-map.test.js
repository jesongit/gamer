// 行映射测试：直接导入 src/script-language/line-map.js（不再从 Console.vue 提取）。
// 覆盖三类运行映射（函数名行 / 函数体行 / 顶层步骤行）+ 省略段落键简写。
import { describe, it, expect } from 'vitest'
import { computeRunLineMap } from './line-map.js'
import { readFileSync } from 'fs'

const fmt = m => (m ? `${m.func ? m.func + '()' : 'steps'}[${m.index}]` : '-')
const sel = map => map.map((m, i) => ({ i, ...m })).filter(x => x.func !== undefined)

describe('三类逻辑行映射', () => {
  it('列表形式 func：函数名行与体内首行同目标(func+0)，then 子步骤不可选，steps 行连续', () => {
    // 2026-08-27 起：函数名行可选 = 从头运行整函数（引擎先判 cond），与函数体首行同为 func+index 0
    const src = `\nfunc:\n  - f:\n    - find: a.png\n      then:\n        - log: hit\nsteps:\n  - f\n`
    const map = computeRunLineMap(src.split('\n'))
    // 行号：0'' 1'func:' 2'- f:'(名行) 3'- find'(体0) 4'then:' 5'- log hit' 6'steps:' 7'- f'
    expect(map[1]).toBeNull()                     // 段落键不可选
    expect(map[2]).toEqual({ func: 'f', index: 0 })   // 函数名行
    expect(map[3]).toEqual({ func: 'f', index: 0 })   // 函数体首步（同目标）
    expect(map[4]).toBeNull()                     // 分支键不可选
    expect(map[5]).toBeNull()                     // then 子步骤不可选
    expect(map[7]).toEqual({ func: null, index: 0 })
    expect(sel(map).map(x => x.i)).toEqual([2, 3, 7])
  })

  it('映射形式 func 体可选；同列 "- " 序列值特例同样可选', () => {
    const mapForm = computeRunLineMap(
      `\nfunc:\n  f1:\n    - find: a.png\n    - log: x\n  f2:\n    - log: y\nsteps:\n  - log: top\n`.split('\n'))
    expect(mapForm[3]?.func).toBe('f1'); expect(mapForm[3]?.index).toBe(0)
    expect(mapForm[4]?.func).toBe('f1'); expect(mapForm[4]?.index).toBe(1)
    expect(mapForm[6]?.func).toBe('f2')
    expect(mapForm[2]).toEqual({ func: 'f1', index: 0 }) // 名行

    const sameCol = computeRunLineMap(
      `\nfunc:\n  f1:\n  - log: a\n  - log: b\nsteps:\n  - log: top\n`.split('\n'))
    expect(sameCol[3]).toEqual({ func: 'f1', index: 0 })
    expect(sameCol[4]).toEqual({ func: 'f1', index: 1 })
    expect(sameCol[6]).toEqual({ func: null, index: 0 })
  })

  it('纯 steps 脚本索引连续且只选顶层项', () => {
    const plain = computeRunLineMap(`\nsteps:\n  - log: a\n  - find: b.png\n    then:\n      - log: c\n  - log: d\n`.split('\n'))
    const picks = sel(plain).map(({ i, ...m }) => ({ i, ...m }))
    expect(picks).toEqual([
      { i: 2, func: null, index: 0 },
      { i: 3, func: null, index: 1 },
      { i: 6, func: null, index: 2 },
    ])
  })

  it('config 段（含列表形式）与函数条目参数键均不可选，不干扰 steps 计数', () => {
    const cfgList = computeRunLineMap(`\nconfig:\n  - interval: 500ms\n  - threshold: 0.9\nsteps:\n  - log: a\n  - log: b\n`.split('\n'))
    expect(sel(cfgList)).toEqual([
      { i: 5, func: null, index: 0 },
      { i: 6, func: null, index: 1 },
    ])

    const condRows = computeRunLineMap(`\nfunc:\n  - g:\n    cond: a.png\n    steps:\n      - log: x\nsteps:\n  - g\n`.split('\n'))
    expect(sel(condRows)).toEqual([
      { i: 2, func: 'g', index: 0 },   // 名行
      { i: 5, func: 'g', index: 0 },   // 体内首步（cond/steps 键行跳过）
      { i: 7, func: null, index: 0 },
    ])
  })

  it('省略段落键简写：顶层序列按 steps、顶层映射按 func 扫描', () => {
    const seq = computeRunLineMap(`- wait: 1s\n- log: done\n`.split('\n'))
    expect(seq[0]).toEqual({ func: null, index: 0 })
    expect(seq[1]).toEqual({ func: null, index: 1 })

    const mappingLib = computeRunLineMap(`noop_fn:\n  - log: $1\n`.split('\n'))
    expect(mappingLib[0]).toEqual({ func: 'noop_fn', index: 0 })
    expect(mappingLib[1]).toEqual({ func: 'noop_fn', index: 0 })
  })

  it('空行 / 注释行跳过不占序号', () => {
    const src = `steps:\n\n  # 注释\n  - log: a\n\n  - log: b\n`
    const map = computeRunLineMap(src.split('\n'))
    expect(sel(map)).toEqual([
      { i: 3, func: null, index: 0 },
      { i: 5, func: null, index: 1 },
    ])
  })
})

describe('业务 fixture 行映射抽查（flow_daily.yaml）', () => {
  const content = readFileSync(new URL('./fixtures/scripts/flow_daily.yaml', import.meta.url), 'utf8')
  const lines = content.split('\n')
  const map = computeRunLineMap(lines)

  it('顶层 steps 行连续编号；跨文件调用行是普通步骤行', () => {
    const stepRows = sel(map).filter(m => m.func === null)
    stepRows.forEach((m, n) => expect(m.index).toBe(n))
    const callRow = lines.findIndex(l => l.trim() === '- lib_utils:mail_recv')
    expect(callRow).toBeGreaterThan(0)
    expect(map[callRow]).toEqual({ func: null, index: 1 })
  })

  it('find 块的 block/timeout/then/else 行与 throw 子步骤不可选', () => {
    const idx = pred => lines.findIndex(pred)
    const blockRow = idx(l => /^\s*block:/.test(l))
    const timeoutRow = idx(l => /^\s*timeout:/.test(l))
    const thenRow = idx(l => /^\s*then:/.test(l))
    for (const i of [blockRow, timeoutRow, thenRow]) {
      expect(i).toBeGreaterThan(-1)
      expect(map[i], `第 ${i + 1} 行 ${lines[i]}`).toBeNull()
    }
    const throwRow = idx(l => /^\s*- throw:/.test(l))
    expect(throwRow).toBeGreaterThan(-1)
    expect(map[throwRow]).toBeNull()
  })

  it('全部非空行的渲染表示可读（防回归快照要点）', () => {
    const summary = lines.map((l, i) => (l.trim() && !/^\s*#/.test(l)) ? `${i}:${fmt(map[i])}` : null)
      .filter(Boolean)
    expect(summary.filter(s => s.includes('undefined')).length).toBe(0)
  })
})
