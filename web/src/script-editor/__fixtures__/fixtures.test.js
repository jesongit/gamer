import { describe, expect, it } from 'vitest'
import { load as yamlLoad } from 'js-yaml'
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

/**
 * script_v2 契约 fixture 前端断言。
 *
 * 本目录是 server/tests/fixtures/script_v2/ 的只读副本：
 * - yaml/ 与服务端逐字节一致（下方有专门的漂移测试守护）；
 * - json/ 为 golden / expected 期望副本；
 * - 契约字段定义见 docs/SCRIPT_EDITOR_CONTRACT.md 第 3 节五方对照表。
 *
 * 服务端（saphyr-parser 事件层）负责权威解析与非法拒绝；本测试用 js-yaml
 * 在前端侧做同一套结构断言，保证「同一份 YAML、前后端读出同一形态」。
 */

const here = path.dirname(fileURLToPath(import.meta.url))
const yamlDir = path.join(here, 'yaml')
const jsonDir = path.join(here, 'json')
// monorepo 内服务端 fixture 源目录：__fixtures__ → script-editor → src → web → 仓库根
const serverFixtureDir = path.resolve(here, '..', '..', '..', '..', 'server', 'tests', 'fixtures', 'script_v2')

const VALID_IDS = [
  'v01_minimal_script',
  'v02_all_actions',
  'v03_function_library',
  'v04_params_all_defaults',
  'v05_params_all_required',
  'v06_nested_if_loop',
  'v07_match_compact',
  'v08_color_branch',
  'v09_call_script',
  'v10_func_call_cross_file',
  'v11_record_output',
  'v12_task_args_snapshot',
  'v13_check_step',
  'v14_branch_click',
]

const INVALID_IDS = [
  'i02_params_unquoted',
  'i03_default_type_mismatch',
  'i04_match_candidate_duplicate',
  'i05_func_path_traversal',
  'i06_call_cycle',
  'i07_unknown_top_key',
  'i08_else_in_candidates',
  'i09_empty_default',
  'i10_branch_click_type',
]

const readJson = (dir, name) => JSON.parse(readFileSync(path.join(dir, name), 'utf8'))
const readYaml = (dir, name) => yamlLoad(readFileSync(path.join(dir, name), 'utf8'))

// ---------- Cell / 参数声明断言 ----------

/** 取值单元格：$name → 参数引用；字面量按字段类型比对。 */
function assertCell(rawVal, cell, type, ctx) {
  if (cell && cell.ref !== undefined) {
    expect(rawVal, ctx).toBe(`$${cell.ref}`)
    return
  }
  const lit = cell.lit
  if (type === 'coord') {
    expect(Array.isArray(rawVal), ctx).toBe(true)
    expect(rawVal[0], ctx).toBeCloseTo(lit[0])
    expect(rawVal[1], ctx).toBeCloseTo(lit[1])
  } else if (type === 'bool') {
    expect(rawVal, ctx).toBe(lit)
  } else {
    expect(rawVal, ctx).toBe(lit)
  }
}

/** 候选键位置（match 模板 / color 颜色）：lit → 原名；ref → $名。 */
function expectCandidateKey(cell) {
  return cell.ref !== undefined ? `$${cell.ref}` : cell.lit
}

/** 由 Model 的 ParamDecl 重构规范 YAML 原始声明串（规范形态冻结在 CONTRACT.md 3.2 节）。 */
function paramRawDecl(decl) {
  const base = `${decl.type}:${decl.name}:${decl.remark}`
  if (decl.default === null || decl.default === undefined) return base
  let rawDefault
  switch (decl.type) {
    case 'bool':
      rawDefault = decl.default ? 'true' : 'false'
      break
    case 'coord':
      rawDefault = `[${decl.default[0]}, ${decl.default[1]}]`
      break
    case 'text':
      rawDefault = `"${decl.default}"`
      break
    default:
      rawDefault = String(decl.default)
  }
  return `${base}:${rawDefault}`
}

function assertParams(rawParams, modelParams, ctx) {
  expect(rawParams ?? [], ctx).toHaveLength(modelParams.length)
  modelParams.forEach((decl, i) => {
    expect(rawParams[i], `${ctx}.params[${i}]`).toBe(paramRawDecl(decl))
  })
}

function assertConfig(rawConfig, modelConfig, ctx) {
  if (modelConfig === null) {
    expect(rawConfig, ctx).toBeUndefined()
    return
  }
  expect(rawConfig.interval, `${ctx}.interval`).toBe(modelConfig.interval)
  expect(rawConfig.threshold, `${ctx}.threshold`).toBeCloseTo(modelConfig.threshold)
  expect(rawConfig.log_level, `${ctx}.log_level`).toBe(modelConfig.log_level)
}

// ---------- 步骤断言 ----------

const ACTION_KEYS = new Set([
  'str_app', 'cls_app', 'tap', 'swipe', 'key', 'text', 'log', 'wait',
  'find', 'match', 'check', 'color', 'if', 'loop', 'call', 'func', 'throw', 'return',
])

function findActionKey(step) {
  const keys = Object.keys(step)
  // check 的 throw 是兄弟字段（未命中终止原因），与 throw 动作键同名词：
  // 存在 check 键时 throw 不算动作键（与前后端 loader 同规则）。
  const hasCheck = keys.includes('check')
  return keys.find((k) => ACTION_KEYS.has(k) && !(hasCheck && k === 'throw'))
}

function assertSteps(rawSteps, modelSteps, ctx) {
  expect(Array.isArray(rawSteps), ctx).toBe(true)
  expect(rawSteps, ctx).toHaveLength(modelSteps.length)
  modelSteps.forEach((m, i) => {
    assertStep(rawSteps[i], m, `${ctx}.steps[${i}]`)
  })
}

function assertStep(s, m, ctx) {
  if (m.kind === 'str_app' || m.kind === 'cls_app' || (m.kind === 'throw' && m.message === null)) {
    expect(typeof s, ctx).toBe('string')
    expect(s, ctx).toBe(m.kind)
    return
  }
  const action = findActionKey(s)
  expect(action, `${ctx}: 动作键应为 ${m.kind}`).toBe(m.kind)
  const sub = s[action]
  switch (m.kind) {
    case 'tap':
      assertCell(sub, m.at, 'coord', `${ctx}.at`)
      break
    case 'swipe':
      assertCell(sub.fm, m.from, 'coord', `${ctx}.fm`)
      assertCell(sub.to, m.to, 'coord', `${ctx}.to`)
      assertCell(sub.time, m.time, 'str', `${ctx}.time`)
      break
    case 'key':
      assertCell(sub, m.key, 'str', `${ctx}.key`)
      break
    case 'text':
      assertCell(sub, m.value, 'str', `${ctx}.value`)
      break
    case 'log':
      assertCell(sub, m.message, 'str', `${ctx}.message`)
      break
    case 'wait':
      if (m.duration_max === null) {
        assertCell(sub, m.duration, 'str', `${ctx}.duration`)
      } else {
        expect(Array.isArray(sub), ctx).toBe(true)
        assertCell(sub[0], m.duration, 'str', `${ctx}.duration`)
        assertCell(sub[1], m.duration_max, 'str', `${ctx}.duration_max`)
      }
      break
    case 'find':
      assertCell(sub, m.template, 'str', `${ctx}.template`)
      expect(s.block ?? [], `${ctx}.block`).toHaveLength(m.block.length)
      m.block.forEach((b, i) => assertCell(s.block[i], b, 'str', `${ctx}.block[${i}]`))
      expect(s.verify ?? false, `${ctx}.verify`).toBe(m.verify)
      if (m.timeout === null) expect(s.timeout ?? null, `${ctx}.timeout`).toBeNull()
      else assertCell(s.timeout, m.timeout, 'str', `${ctx}.timeout`)
      assertSteps(s.then ?? [], m.then, `${ctx}.then`)
      assertSteps(s.else ?? [], m.else, `${ctx}.else`)
      break
    case 'match': {
      // 紧凑缩进：候选列表 = match 键的值（无缩进序列），else/timeout 是兄弟键。
      expect(Array.isArray(sub), ctx).toBe(true)
      expect(sub, `${ctx}.candidates`).toHaveLength(m.candidates.length)
      m.candidates.forEach((c, i) => {
        const cand = sub[i]
        const keys = Object.keys(cand)
        expect(keys, `${ctx}.candidates[${i}]`).toHaveLength(1)
        expect(keys[0], `${ctx}.candidates[${i}].template`).toBe(expectCandidateKey(c.template))
        // 候选值双形态：列表 = 不点击（原形态）；映射 {click: true, steps} = 命中点击。
        const branch = cand[keys[0]]
        if (c.click) {
          expect(Array.isArray(branch), `${ctx}.candidates[${i}] 应为映射形态`).toBe(false)
          expect(branch.click, `${ctx}.candidates[${i}].click`).toBe(true)
          assertSteps(branch.steps ?? [], c.steps, `${ctx}.candidates[${i}].steps`)
        } else {
          expect(Array.isArray(branch), `${ctx}.candidates[${i}] 应为列表形态`).toBe(true)
          assertSteps(branch, c.steps, `${ctx}.candidates[${i}].steps`)
        }
      })
      assertSteps(s.else ?? [], m.else, `${ctx}.else`)
      if (m.timeout === null) expect(s.timeout ?? null, `${ctx}.timeout`).toBeNull()
      else assertCell(s.timeout, m.timeout, 'str', `${ctx}.timeout`)
      break
    }
    case 'color': {
      assertCell(sub.at, m.at, 'coord', `${ctx}.at`)
      // expect 是有序列表（每项单键映射 颜色→步骤）。不用颜色做映射键：
      // js-yaml 载入 plain object 时纯数字色键（如 '123456'）会被 JS 按
      // 整数形键重排到最前，映射形态在前端丢候选顺序（实测踩坑，已冻结为列表形态）。
      expect(Array.isArray(sub.expect), `${ctx}.expect 应为列表`).toBe(true)
      expect(sub.expect, `${ctx}.expect`).toHaveLength(m.expect.length)
      m.expect.forEach((e, i) => {
        const cand = sub.expect[i]
        const keys = Object.keys(cand)
        expect(keys, `${ctx}.expect[${i}]`).toHaveLength(1)
        expect(keys[0], `${ctx}.expect[${i}].color`).toBe(expectCandidateKey(e.color))
        // 候选值双形态：列表 = 不点击；映射 {click: true, steps} = 命中点击。
        const branch = cand[keys[0]]
        if (e.click) {
          expect(Array.isArray(branch), `${ctx}.expect[${i}] 应为映射形态`).toBe(false)
          expect(branch.click, `${ctx}.expect[${i}].click`).toBe(true)
          assertSteps(branch.steps ?? [], e.steps, `${ctx}.expect[${i}].steps`)
        } else {
          expect(Array.isArray(branch), `${ctx}.expect[${i}] 应为列表形态`).toBe(true)
          assertSteps(branch, e.steps, `${ctx}.expect[${i}].steps`)
        }
      })
      assertSteps(s.else ?? [], m.else, `${ctx}.else`)
      break
    }
    case 'if':
      assertCell(sub, m.cond, 'bool', `${ctx}.cond`)
      assertSteps(s.then ?? [], m.then, `${ctx}.then`)
      assertSteps(s.else ?? [], m.else, `${ctx}.else`)
      break
    case 'loop':
      if (m.times === null) expect(sub.times ?? null, `${ctx}.times`).toBeNull()
      else expect(sub.times, `${ctx}.times`).toBe(m.times)
      assertSteps(sub.steps, m.steps, `${ctx}.steps`)
      break
    case 'call':
      expect(sub, `${ctx}.target`).toBe(m.target)
      assertArgs(s.args ?? {}, m.args, ctx)
      break
    case 'func':
      expect(sub, `${ctx}.target`).toBe(m.target)
      assertArgs(s.args ?? {}, m.args, ctx)
      assertSteps(s.then ?? [], m.then, `${ctx}.then`)
      assertSteps(s.else ?? [], m.else, `${ctx}.else`)
      break
    case 'throw':
      expect(sub, `${ctx}.message`).toBe(m.message)
      break
    case 'check':
      assertCell(sub, m.template, 'str', `${ctx}.template`)
      expect(s.throw, `${ctx}.throw`).toBe(m.throw)
      break
    case 'return':
      assertCell(sub, m.value, 'bool', `${ctx}.value`)
      break
    default:
      throw new Error(`${ctx}: 未知步骤类型 ${m.kind}`)
  }
}

function assertArgs(rawArgs, modelArgs, ctx) {
  const modelKeys = Object.keys(modelArgs)
  expect(Object.keys(rawArgs).sort(), `${ctx}.args 键集`).toEqual([...modelKeys].sort())
  modelKeys.forEach((k) => {
    const cell = modelArgs[k]
    const type = cell.ref !== undefined ? 'str' : typeof cell.lit === 'boolean' ? 'bool' : 'str'
    assertCell(rawArgs[k], cell, type, `${ctx}.args.${k}`)
  })
}

// ---------- 模型形态断言 ----------

function assertScriptShape(raw, model, ctx) {
  expect(Object.keys(raw).every((k) => ['params', 'config', 'steps'].includes(k)), `${ctx} 顶层键`).toBe(true)
  expect(raw.steps, `${ctx} steps 必需`).toBeDefined()
  assertParams(raw.params, model.params, ctx)
  assertConfig(raw.config, model.config, ctx)
  assertSteps(raw.steps, model.steps, ctx)
}

function assertFunctionLibraryShape(raw, model, ctx) {
  const names = Object.keys(raw)
  expect(names, `${ctx} 函数名`).toEqual(model.functions.map((f) => f.name))
  model.functions.forEach((f) => {
    const rec = raw[f.name]
    expect(Object.keys(rec).every((k) => ['params', 'steps'].includes(k)), `${ctx}.${f.name} 记录键`).toBe(true)
    assertParams(rec.params, f.params, `${ctx}.${f.name}`)
    assertSteps(rec.steps, f.steps, `${ctx}.${f.name}`)
  })
}

// ---------- 任务参数签名（psig1，算法冻结在 CONTRACT.md 4.5 节） ----------

function fmtNum(x) {
  return String(x)
}

function signatureDefault(type, d) {
  switch (type) {
    case 'bool':
      return d ? 'true' : 'false'
    case 'coord':
      return `[${fmtNum(d[0])},${fmtNum(d[1])}]`
    case 'color':
      return String(d).toLowerCase()
    case 'key':
      return String(d).toUpperCase()
    case 'time':
      return String(d).toLowerCase().replace(/min$/, 'm')
    case 'text':
      return String(d).replace(/\\/g, '\\\\').replace(/,/g, '\\,').replace(/\|/g, '\\|')
    case 'tmpl':
      return String(d)
    default:
      throw new Error(`未知参数类型 ${type}`)
  }
}

function paramSignature(model) {
  const entries = model.params.map((p) => {
    const required = p.default === null ? '1' : '0'
    return `${p.type},${p.name},${required},${p.default === null ? '' : signatureDefault(p.type, p.default)}`
  })
  return `psig1|${entries.join('|')}`
}

// ---------- 测试 ----------

describe('script_v2 fixtures 与服务端逐字节一致', () => {
  it('yaml/ 与 json/ 副本和 server/tests/fixtures/script_v2/ 完全相同', () => {
    for (const dir of ['yaml', 'json']) {
      const files = readdirSync(path.join(here, dir))
      expect(files.length, `${dir}/ 副本数量`).toBeGreaterThan(0)
      for (const f of files) {
        const local = readFileSync(path.join(here, dir, f))
        const remote = readFileSync(path.join(serverFixtureDir, f))
        expect(
          local.equals(remote),
          `${dir}/${f} 与服务端副本不一致（两目录必须逐字节一致，改任一侧需同步）`,
        ).toBe(true)
      }
    }
  })
})

describe('script_v2 golden 合法样例（js-yaml 读 YAML 断言 golden 模型）', () => {
  for (const id of VALID_IDS) {
    it(`${id}`, () => {
      const golden = readJson(jsonDir, `${id}.golden.json`)
      expect(golden.kind).toBe('valid')
      for (const entry of golden.files) {
        const raw = readYaml(yamlDir, entry.file)
        if (entry.model_kind === 'script') assertScriptShape(raw, entry.model, `${id}/${entry.file}`)
        else assertFunctionLibraryShape(raw, entry.model, `${id}/${entry.file}`)
      }
    })
  }
})

describe('script_v2 定时任务参数快照形态（v12）', () => {
  it('args 为全量类型化默认值快照，param_signature 可复算', () => {
    const golden = readJson(jsonDir, 'v12_task_args_snapshot.golden.json')
    const snapshot = golden.task_snapshot
    expect(snapshot.script_id).toBe('demo/v12_task_args_snapshot.yaml')
    expect(paramSignature(golden.files[0].model)).toBe(snapshot.param_signature)
    const params = golden.files[0].model.params
    expect(Object.keys(snapshot.args)).toHaveLength(params.length)
    for (const p of params) {
      expect(snapshot.args[p.name]).toEqual(p.default)
    }
  })
})

describe('script_v2 非法样例期望（服务端权威拒绝；前端只校验样例与期望形态）', () => {
  it('expected JSON 结构齐全（code/step_path/field）', () => {
    for (const id of INVALID_IDS) {
      const expected = readJson(jsonDir, `${id}.expected.json`)
      expect(expected.kind, id).toBe('invalid')
      expect(expected.errors.length, id).toBeGreaterThan(0)
      for (const e of expected.errors) {
        expect(typeof e.code, `${id}.code`).toBe('string')
        expect(e.code.length, `${id}.code`).toBeGreaterThan(0)
        expect(typeof e.step_path, `${id}.step_path`).toBe('string')
        expect(typeof e.field, `${id}.field`).toBe('string')
      }
    }
  })

  it('违规形态抽查：i02 无引号 / i08 - else 进候选 / i09 空默认值', () => {
    const i02 = readFileSync(path.join(yamlDir, 'i02_params_unquoted.yaml'), 'utf8')
    expect(i02).toContain('- bool:enable:开关:true')
    expect(i02).not.toContain("- 'bool:enable:开关:true'")
    const i08 = readFileSync(path.join(yamlDir, 'i08_else_in_candidates.yaml'), 'utf8')
    expect(i08).toMatch(/^\s+- else:\s*$/m)
    const i09 = readFileSync(path.join(yamlDir, 'i09_empty_default.yaml'), 'utf8')
    expect(i09).toContain("- 'text:message:提示文本:'")
  })
})
