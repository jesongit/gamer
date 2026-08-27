// YAML 脚本校验（原样自 Console.vue 抽离，2026-08-27，除依赖注入包装外零行为变化）。
// 语法真源：docs/YAML.md 与 server/src/engine.rs；修改引擎时必须同步本文件。
import { load as yamlLoad } from 'js-yaml'

/** 创建绑定数据源的校验函数。三个依赖均为 { value } 形态（Vue ref 或普通对象均可）：
 *  - templatesData.value: 模板列表（跨分区全量，条目 { name, pkg }）
 *  - scriptsData.value:   脚本列表（条目 { id, package, name, content }，供跨文件调用解析）
 *  - activePkg.value:     当前应用分区名 */
export function createScriptValidator({ templatesData, scriptsData, activePkg }) {
  /** 解析 YAML 为普通对象。数组键（- [x, y]: 色值）是已删除的 color/cond 旧语法，
   *  js-yaml object 构造直接抛 "complex keys"——换成明确的迁移提示 */
  function yamlParse(content) {
    try {
      return yamlLoad(content)
    } catch (e) {
      if (/complex keys/.test(e.reason || e.message || '')) {
        throw new Error('数组键写法（- [x, y]: 色值）已删除：颜色判断写 `- color: [x, y]` + 色值键（如 ff8800: 挂命中步骤）')
      }
      throw e
    }
  }

  /** func 段值拆分为逐项映射列表（与引擎 parse_funcs 对 Mapping/Sequence 的展开
   *  一致）：列表原样；映射逐键拆成单键条目（一个映射里可定义多个函数）；
   *  标量等其他形态原样包装由调用方报错 */
  function splitFuncItems(funcVal) {
    if (Array.isArray(funcVal)) return funcVal
    if (funcVal && typeof funcVal === 'object') {
      return Object.entries(funcVal).map(([k, v]) => ({ [k]: v }))
    }
    return [funcVal]
  }

  /** 跨文件调用的函数名提取：与引擎 exec_cross_func 一致，先做 normalize_top
   *  归一化再取 func 段——顶层映射且不含 config/func/steps 任何键 = 整体视为
   *  func（省略 func: 的纯函数库简写同样可被跨文件调用）；顶层序列 = steps（无
   *  func）。函数名逐项收集（排除 cond / steps 参数键），不校验函数体结构 */
  function extractCrossFileFuncNames(subContent) {
    const sdoc = yamlParse(subContent)
    const hasSection = d => !!d && typeof d === 'object' && !Array.isArray(d)
      && ('config' in d || 'func' in d || 'steps' in d)
    const funcVal = !sdoc || typeof sdoc !== 'object' || Array.isArray(sdoc)
      ? undefined
      : hasSection(sdoc) ? sdoc.func : sdoc // 省略 func: 的纯函数库简写 → 整体即 func
    const names = new Set()
    if (!funcVal || typeof funcVal !== 'object') return names
    for (const it of splitFuncItems(funcVal)) {
      if (!it || typeof it !== 'object') continue
      for (const k of Object.keys(it)) {
        if (k !== 'cond' && k !== 'steps') names.add(k.trim())
      }
    }
    return names
  }

  /** 保存前校验 YAML：语法 / steps / 坐标范围 / 模板存在（模板按当前应用分区校验） */
  function validateScriptCode(content) {
    const errors = []
    let doc
    try {
      doc = yamlParse(content)
    } catch (e) {
      const lines = content.split('\n')
      // 常见笔误提示：`region:l` / `timeout:0` 冒号后缺空格
      const bad = lines.map((l, i) => ({ l, i })).find(({ l }) => /^\s*-?\s*[\w\u4e00-\u9fa5-]+:(?!\s|$)/.test(l))
      if (bad) {
        const line = bad.l.trim()
        return [`YAML 语法错误（第 ${bad.i + 1} 行）：${line} 冒号后缺少空格，应为 "${line.replace(/:(?!\s|$)/, ': ')}"`]
      }
      // 标量列表项后跟更深缩进行：子内容挂不到标量上——多半是 then 按模板分支
      // 的「- 模板名」漏写冒号（js-yaml 会误报到下一行的缩进上）
      for (let i = 0; i < lines.length; i++) {
        const m = lines[i].match(/^(\s*)-\s+(.+?)\s*$/)
        if (!m || m[2].includes(':')) continue
        let j = i + 1
        while (j < lines.length && !lines[j].trim()) j++
        if (j >= lines.length) break
        const nm = lines[j].match(/^(\s*)\S/)
        if (nm && nm[1].length > m[1].length) {
          return [`YAML 语法错误（第 ${i + 1} 行）："- ${m[2]}" 后要接子步骤需带冒号，应为 "- ${m[2]}:"（如 - find: 主模板 / then 步骤同理：键: 换行缩进内容）`]
        }
      }
      // 「键: 值」后跟更深缩进的 "- " 步骤行：子步骤不挂在任何键上——多半是漏写
      // then/else 分支键（如 - find: x 的 else 子步骤必须写在 else: 正下方一层）
      for (let i = 0; i < lines.length; i++) {
        const m = lines[i].match(/^(\s*)[\w\u4e00-\u9fa5-]+:\s*([^#\s].*)$/)
        if (!m) continue
        let j = i + 1
        while (j < lines.length && !lines[j].trim()) j++
        if (j >= lines.length) continue // 末尾「键: 值」行 + 空行到 EOF：越界防御，落通用兜底提示
        const nm = lines[j].match(/^(\s*)-\s/)
        if (nm && nm[1].length > m[1].length) {
          return [`YAML 语法错误（第 ${i + 1} 行）：「${m[0].trim()}」后跟着更深缩进的 "- " 步骤行——子步骤不挂在任何键上（漏写分支键）：find 的步骤须写在 then:/else: 键正下方，如
      - find: task_list.png
        timeout: 1s
        else:
          - throw: 未知界面`]
        }
      }
      // 「键: 值」后直接跟同列 "- " 步骤行（bad indentation，映射键间不能插同列
      // dash 行）：子内容须挂在分支键（then/else）或 steps: 键正下方
      for (let i = 0; i < lines.length; i++) {
        const m = lines[i].match(/^(\s*)[\w\u4e00-\u9fa5-]+:\s*([^#\s].*)$/)
        if (!m) continue
        let j = i + 1
        while (j < lines.length && !lines[j].trim()) j++
        if (j >= lines.length) continue // 同上：越界防御
        const nm = lines[j].match(/^(\s*)-\s/)
        if (nm && nm[1].length === m[1].length) {
          return [`YAML 语法错误（第 ${i + 1} 行）：「${m[0].trim()}」后不能直接跟同列 "- " 步骤行（bad indentation）——子步骤缩进时须挂在分支键（then / else）或步骤列表键正下方，如
      - find: task_list.png
        timeout: 1s
        else:
          - throw: 未知界面`]
        }
      }
      return ['YAML 语法错误：' + e.message]
    }
    // 顶层段落归一化（与引擎 normalize_top 一致）：单段脚本可省略段落键——
    // 顶层序列 = steps；顶层映射且不含 config/func/steps 任何键 = func（纯函数库
    // 简写，函数定义直接写在顶层）；config 不能省略
    if (Array.isArray(doc)) {
      doc = { steps: doc }
    } else if (!doc || typeof doc !== 'object') {
      return ['脚本必须是 YAML 对象或步骤列表']
    } else if (!('config' in doc) && !('func' in doc) && !('steps' in doc)) {
      for (const k of Object.keys(doc)) {
        if (k === 'action_wait') return ['顶层 action_wait 已删除：操作间隔统一为 config interval（仅轮询类等待，步骤间不再等待）']
        if (k === 'log_level') return ['顶层 log_level 已删除：改用 config: 段（config.toml 可配全局默认）']
        if (k === 'name') return ['顶层 name 已删除（脚本名即文件名）']
        if (k === 'interval' || k === 'threshold') {
          return [`顶层 ${k} 是 config: 段参数（省略段落键的简写只支持纯 steps 序列或纯 func 函数定义，config 必须写 config: 键）`]
        }
      }
      doc = { func: doc }
    } else {
      // 顶层键白名单（与引擎 run 一致）：只允许 config / func / steps
      for (const k of Object.keys(doc)) {
        if (k === 'config' || k === 'func' || k === 'steps') continue
        if (k === 'action_wait') errors.push('顶层 action_wait 已删除：操作间隔统一为 config interval（仅轮询类等待，步骤间不再等待）')
        else if (k === 'log_level') errors.push('顶层 log_level 已删除：改用 config: 段（config.toml 可配全局默认）')
        else if (k === 'name') errors.push('顶层 name 已删除（脚本名即文件名）')
        else errors.push(`未知顶层键 ${k}（只支持 config / func / steps；单段简写：顶层序列 = steps，无段落键的顶层映射 = func）`)
      }
    }
    // steps 可缺省：纯函数库脚本（只有 func）供其他脚本通过 脚本名:函数名 调用；
    // steps 与 func 都没有 → 报错
    const hasFuncs = doc.func !== undefined && doc.func !== null
      && (Array.isArray(doc.func) ? doc.func.length > 0 : (typeof doc.func === 'object' && Object.keys(doc.func).length > 0))
    if (!Array.isArray(doc.steps) && !hasFuncs) {
      return ['脚本需要 steps 或 func 根节点（纯函数库脚本也至少要定义一个函数，供其他脚本通过 脚本名:函数名 调用）']
    }

    const tplNames = new Set((templatesData.value || []).filter(t => t.pkg === activePkg.value).map(t => t.name))
    // 短名支持（与引擎 resolve_template_file 一致）：login.png 可引用 login#*.png，
    // 区域后缀照常生效；同基名多个后缀文件 → 短名歧义，要求写全名消歧
    const shortOf = n => n.replace(/#[^#./\\]+(\.(png|jpe?g))$/i, '$1')
    const byShort = new Map()
    for (const f of tplNames) {
      const s = shortOf(f)
      if (s !== f) byShort.set(s, (byShort.get(s) || []).concat(f))
    }
    const tplCheck = n => {
      if (tplNames.has(n)) return null
      const cands = byShort.get(n)
      if (cands && cands.length === 1) return null
      if (cands && cands.length > 1) return `模板 ${n} 匹配到多个：${cands.join('、')}，请用完整文件名`
      return `模板不存在：${n}`
    }
    // call/函数传参占位：$1/$2… 引用实参模板名，在本脚本无法校验存在性（find/color 模板共用）
    const argOrTplCheck = n => (/^\$\d+$/.test(n) ? null : tplCheck(n))
    // 模板名列表解析（与引擎 parse_tpl_names 一致）：字符串可逗号分隔多模板，或 YAML 字符串列表
    const parseTplNames = (v, key) => {
      const names = typeof v === 'string'
        ? v.split(',').map(s => s.trim())
        : Array.isArray(v) && v.every(x => typeof x === 'string')
          ? v.map(s => s.trim())
          : null
      if (!names || names.length === 0 || names.some(n => !n)) return null
      return names
    }
    const checkRel = (label, x, y) => {
      if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || x > 1 || y < 0 || y > 1) {
        errors.push(`${label} 相对坐标需在 0~1 之间 (${x}, ${y})`)
      }
    }
    // 时长强制带单位（与引擎 parse_duration 一致）：1ms / 2s / 1m / 30min / 1h / 1d；裸数字不再接受
    const DUR_RE = /^\s*(\d+(?:\.\d+)?)\s*(ms|min|m|[shd])\s*$/i
    const isDur = v => typeof v === 'string' && DUR_RE.test(v)
    const isPosDur = v => isDur(v) && Number(v.match(DUR_RE)[1]) > 0
    const HEX_RE = /^#?(0x)?[0-9a-fA-F]{6}$/
    const ACTION_KEYS = ['log', 'key', 'text', 'tap', 'swipe', 'find', 'color', 'loop', 'call', 'throw', 'str_app', 'cls_app', 'wait', 'return']
    const FUNC_RESERVED = new Set([...ACTION_KEYS, 'then', 'else', 'steps', 'times', 'block', 'verify', 'timeout', 'config', 'func', 'until', 'cond', 'exit'])

    // config: 段（mapping 或 mapping 列表按序覆盖；键 = interval / threshold / log_level）
    const cfgMaps = []
    if (doc.config !== undefined && doc.config !== null) {
      if (Array.isArray(doc.config)) cfgMaps.push(...doc.config)
      else if (typeof doc.config === 'object') cfgMaps.push(doc.config)
      else errors.push('config 需要 mapping（或 mapping 列表按序覆盖）')
    }
    cfgMaps.forEach((m, i) => {
      if (!m || typeof m !== 'object' || Array.isArray(m)) {
        errors.push(`config 第 ${i + 1} 项需要映射（键值按序覆盖）`)
        return
      }
      for (const [k, v] of Object.entries(m)) {
        if (k === 'interval') {
          if (!isPosDur(v)) errors.push('config.interval 需要带单位时长且 > 0（如 500ms；裸数字不再接受）')
        } else if (k === 'threshold') {
          if (typeof v !== 'number' || v <= 0 || v > 1) errors.push('config.threshold 需要在 (0, 1] 之间的数字')
        } else if (k === 'log_level') {
          if (!['debug', 'info', 'warn', 'error'].includes(String(v))) errors.push('config.log_level 需要 debug / info / warn / error')
        } else {
          errors.push(`config 不支持的键 ${k}（可用：interval / threshold / log_level）`)
        }
      }
    })

    // func: 段（函数名保留字 / 函数体结构；cond 条件 + steps 键与函数名同级——
    // 与 loop 的 times/steps 同构，也兼容映射形式嵌套在函数名值里；
    // 函数体步骤递归校验在主 steps 之后，return 合法）
    const funcs = []
    if (doc.func !== undefined && doc.func !== null) {
      // 映射形式逐键拆分为多个单键函数定义（与引擎 parse_funcs 一致：一个映射
      // 里可定义多个函数——含省略 func: 的纯函数库简写归一化后的形态）
      const items = splitFuncItems(doc.func)
      items.forEach((it, i) => {
        const at = `func 第 ${i + 1} 项`
        if (!it || typeof it !== 'object' || Array.isArray(it)) {
          errors.push(`${at} 需要 函数名: 步骤列表`)
          return
        }
        const ks = Object.keys(it).filter(k => k !== 'cond' && k !== 'steps')
        if (ks.length !== 1) {
          errors.push(`${at} 需要恰好一个 函数名: 键（收到 ${ks.length} 个；cond / steps 是函数定义参数键）`)
          return
        }
        const name = ks[0].trim()
        if (FUNC_RESERVED.has(name)) {
          errors.push(`${at} 函数名 ${name} 是保留字（动作键 / 结构键）——若这是函数体的步骤，说明函数体缩进不对：函数体要比 "- 函数名:" 行多缩进（如 4 空格）`)
        }
        else if (funcs.some(f => f.name === name)) errors.push(`函数 ${name} 重复定义`)
        // cond / steps 取值：列表形式是 it 的兄弟键；映射形式嵌套在函数名值里
        let condVal = it.cond
        let body = it[name]
        if (body && typeof body === 'object' && !Array.isArray(body) && Object.keys(body).every(k => k === 'cond' || k === 'steps')) {
          if (condVal === undefined) condVal = body.cond
          body = body.steps
        }
        if (it.steps !== undefined) body = it.steps
        if (body !== null && body !== undefined && !Array.isArray(body)) {
          errors.push(`${at} ${name} 的函数体需要步骤列表（函数名键值或 steps 键）`)
        }
        // cond 条件模板：字符串（可逗号分隔）或字符串列表；模板存在性按分区校验
        if (condVal !== undefined && condVal !== null) {
          const condNames = parseTplNames(condVal, 'cond')
          if (!condNames) {
            errors.push(`${at} ${name} 的 cond 需要模板名字符串（多模板逗号分隔）或列表，如 cond: a.png, b.png`)
          } else {
            for (const n of condNames) {
              const terr = argOrTplCheck(n)
              if (terr) errors.push(`${at} ${name} 的 cond ${terr}`)
            }
          }
        }
        funcs.push({ name, body: Array.isArray(body) ? body : [] })
      })
    }

    // 步骤递归校验（steps / then / else / loop steps / func 函数体共用；与引擎 exec_step 一致）
    function validateStep(rawStep, at, inFunc) {
      // 带值动作漏写冒号（`- throw 未知界面` 被解析成标量步骤）→ 定向提示
      if (typeof rawStep === 'string') {
        const m = rawStep.match(/^(\w+)\s+(.+)$/)
        if (m && ACTION_KEYS.includes(m[1])) {
          errors.push(`${at} "${rawStep}" 是标量步骤（YAML 把 "- ${rawStep}" 解析成字符串）——带值/带原因的动作需写冒号：应为 "- ${m[1]}: ${m[2]}"（裸写仅限无参动作，如 - str_app / - throw）`)
          return
        }
      }
      // 裸标量步骤（- str_app / - throw）等价 {键: null}，与引擎 exec_step 的规范化一致
      const step = typeof rawStep === 'string' ? { [rawStep]: null } : rawStep
      if (!step || typeof step !== 'object' || Array.isArray(step)) {
        errors.push(`${at}格式错误`)
        return
      }
      const ks = Object.keys(step)
      // 已删除动作/参数守卫（与引擎 exec_step 一致，显式报错引导迁移）
      if ('until' in step) errors.push(`${at} until 已改名 find：- find: 主模板 + block: 障碍模板`)
      if ('cond' in step) {
        errors.push(inFunc
          ? `${at} cond 是函数级条件（写在 "- 函数名:" 行下、与 steps 键同级），函数体步骤不支持 cond；旧颜色判断用 color`
          : `${at} cond 已改名 color：颜色判断写 - color: [x, y] + 色值键步骤；模板分支用 find + then/else`)
      }
      if ('exit' in step) errors.push(`${at} exit 已改名 throw`)
      if ('goto' in step || 'label' in step) errors.push(`${at} goto/label 已删除：循环重试用 loop`)
      for (const k of ['check', 'count', 'cnt_ivl', 'cnt_chk', 'img_ivl', 'and_or', 'click', 'before', 'after']) {
        if (k in step) {
          errors.push(k === 'check'
            ? `${at} check 已改名 block（find 的障碍模板）`
            : `${at} ${k} 已删除（2026-08-26 语法精简）`)
        }
      }
      if ('threshold' in step) errors.push(`${at} threshold 步骤参数已删除：匹配阈值全局配置（config: 段或 config.toml）`)
      if ('region' in step) errors.push(`${at} region 步骤参数已删除：搜索区域由模板名 #后缀 决定（无后缀回退全屏）`)

      const actions = ACTION_KEYS.filter(k => k in step)
      if (actions.length > 1) {
        errors.push(`${at} 一个步骤只能有一个动作键（${actions.join('、')}）${actions.includes('wait') ? '；wait 是独立动作，操作后等待参数已删除' : ''}`)
        return
      }
      // 步骤列表参数（then/else）：非列表报错，列表则递归校验
      const recurse = (key, label) => {
        const v = step[key]
        if (v === undefined) return
        if (!Array.isArray(v)) errors.push(`${at} ${label} 需要步骤列表`)
        else validateSteps(v, `${at} ${label} `, inFunc)
      }
      if (actions.length === 1) {
        const a = actions[0]
        // 各动作的兄弟键白名单（色值键在 color 分支单独校验）
        const allowed = {
          log: ['log'], key: ['key'], text: ['text'], tap: ['tap'], swipe: ['swipe'], wait: ['wait'],
          throw: ['throw'], str_app: ['str_app'], cls_app: ['cls_app'], return: ['return'], call: ['call'],
          find: ['find', 'verify', 'timeout', 'block', 'then', 'else'],
          loop: ['loop', 'times', 'steps'],
        }
        if (a !== 'color') {
          for (const k of ks) {
            if (!allowed[a].includes(k)) errors.push(`${at} ${a} 不支持参数 ${k}（可用：${allowed[a].join(' / ')}）`)
          }
        }
        if (a === 'tap') {
          const v = step.tap
          if (Array.isArray(v) && v.length >= 2) checkRel(`${at} tap`, Number(v[0]), Number(v[1]))
          else if (v && typeof v === 'object') checkRel(`${at} tap`, Number(v.x), Number(v.y))
          else errors.push(`${at} tap 需要 [x, y] 相对坐标`)
        }
        if (a === 'swipe') {
          const v = step.swipe
          if (!v || typeof v !== 'object' || Array.isArray(v)) {
            errors.push(`${at} swipe 需要 {fm, to, time} 映射`)
          } else {
            for (const k of Object.keys(v)) {
              if (k === 'fm' || k === 'to' || k === 'time') continue
              errors.push(k === 'from' ? `${at} swipe 的 from 已改名 fm` : `${at} swipe 不支持参数 ${k}`)
            }
            if (Array.isArray(v.fm) && v.fm.length >= 2) checkRel(`${at} swipe fm`, Number(v.fm[0]), Number(v.fm[1]))
            if (Array.isArray(v.to) && v.to.length >= 2) checkRel(`${at} swipe to`, Number(v.to[0]), Number(v.to[1]))
            if (v.time !== undefined && !isDur(v.time)) errors.push(`${at} swipe time 需要带单位时长（如 500ms；裸数字不再接受）`)
          }
        }
        if (a === 'wait') {
          const v = step.wait
          if (Array.isArray(v)) {
            if (v.length !== 2) errors.push(`${at} wait 区间需要 [最小, 最大] 两个带单位时长（如 [1s, 3s]）`)
            else if (!isDur(v[0]) || !isDur(v[1])) errors.push(`${at} wait 区间需要带单位时长（裸数字不再接受）`)
          } else if (!isDur(v)) {
            errors.push(`${at} wait 需要带单位时长（如 2s）或 [1s, 3s] 区间；裸数字不再接受`)
          }
        }
        if (a === 'call') {
          if (typeof step.call !== 'string' || !step.call.trim()) {
            errors.push(`${at} call 需要 "子脚本名 [实参...]" 字符串（如 - call: test2.yml a.png [0.5, 0.6]）`)
          }
        }
        // throw 值与引擎对齐：非字符串标量（YAML 把 404/true 解析成数字/布尔）
        // 引擎按无原因处理，不报错；仅数组/映射属明显笔误
        if (a === 'throw' && step.throw !== null && step.throw !== undefined && (Array.isArray(step.throw) || typeof step.throw === 'object')) {
          errors.push(`${at} throw 只需裸写或带结束原因字符串（如 - throw: 体力不足）`)
        }
        if ((a === 'str_app' || a === 'cls_app') && step[a] !== null && step[a] !== undefined && String(step[a]).trim() !== '') {
          errors.push(`${at} ${a} 不支持参数：应用包名固定为设备分区（只写 - ${a}）`)
        }
        if (a === 'return') {
          if (typeof step.return !== 'boolean') errors.push(`${at} return 需要 true / false`)
          if (!inFunc) errors.push(`${at} return 仅可在自定义函数内使用`)
        }
        if (a === 'find') {
          const v = step.find
          const main = typeof v === 'string' ? v.trim() : ''
          if (!main || v.includes(',')) {
            errors.push(`${at} find 只支持单个主模板字符串（多个目标请拆成多步；挡路的模板写 block）`)
          } else {
            const terr = argOrTplCheck(main)
            if (terr) errors.push(`${at} ${terr}`)
          }
          if (step.block !== undefined) {
            const blocks = parseTplNames(step.block, 'block')
            if (blocks === null) {
              errors.push(`${at} block 只支持模板名字符串（多模板逗号分隔）或列表，如 block: pop.png, ad.png`)
            } else {
              const dup = blocks.find(b => b === main)
              if (dup) errors.push(`${at} block 模板 ${dup} 与 find 主模板重复`)
              for (const n of blocks) {
                const terr = argOrTplCheck(n)
                if (terr) errors.push(`${at} ${terr}`)
              }
            }
          }
          if (step.verify !== undefined && typeof step.verify !== 'boolean') {
            errors.push(`${at} find verify 需要 true / false（true=点击后等 interval 重匹配，仍命中补点一次）`)
          }
          if (step.timeout !== undefined && !isPosDur(step.timeout)) {
            errors.push(`${at} find timeout 需要带单位时长且 > 0（默认 30min；裸数字不再接受）`)
          }
          recurse('then', 'then')
          recurse('else', 'else')
        }
        if (a === 'color') {
          const v = step.color
          if (Array.isArray(v) && v.length === 2) checkRel(`${at} color`, Number(v[0]), Number(v[1]))
          else errors.push(`${at} color 需要 [x, y] 相对坐标`)
          const hexKeys = ks.filter(k => k !== 'color' && k !== 'else')
          if (hexKeys.length === 0) errors.push(`${at} color 至少需要一个色值键（如 ff8800: 挂命中步骤）`)
          for (const k of hexKeys) {
            if (!HEX_RE.test(k.trim())) errors.push(`${at} color 的色值键 ${k} 需要 6 位十六进制（如 ff8800）`)
            const sv = step[k]
            if (sv !== null && sv !== undefined && !Array.isArray(sv)) {
              errors.push(`${at} 色值键 ${k} 的值需要步骤列表（- 色值: 换行缩进步骤）或留空`)
            } else if (Array.isArray(sv)) {
              validateSteps(sv, `${at} ${k} `, inFunc)
            }
          }
          recurse('else', 'else')
        }
        if (a === 'loop') {
          // times/steps 两种缩进均可：loop 值内映射或与 loop 同级的步骤兄弟键（与引擎 exec_loop 一致）
          const inner = step.loop && typeof step.loop === 'object' && !Array.isArray(step.loop) ? step.loop : {}
          for (const k of Object.keys(inner)) {
            if (k !== 'times' && k !== 'steps') errors.push(`${at} loop 不支持参数 ${k}（可用：times / steps）`)
          }
          const times = inner.times !== undefined ? inner.times : step.times
          const sub = inner.steps !== undefined ? inner.steps : step.steps
          if (times !== undefined && !(Number.isInteger(times) && times >= 0)) {
            errors.push(`${at} loop times 需要非负整数（0 或省略 = 无限循环）`)
          }
          if (!Array.isArray(sub)) errors.push(`${at} loop 需要 steps 步骤列表`)
          else validateSteps(sub, `${at} loop `, inFunc)
        }
        return
      }
      // 无动作键：自定义函数调用（- 函数名: 实参…）/ 跨文件函数调用（- 脚本名:函数名: 实参…）
      // 或未知动作
      if (ks.length === 0) return
      const cand = ks.find(k => funcs.some(f => f.name === k.trim()))
      if (cand) {
        for (const k of ks) {
          if (k !== cand && k !== 'then' && k !== 'else') errors.push(`${at} ${cand} 调用不支持参数 ${k}（可用：then / else）`)
        }
        const v = step[cand]
        if (v !== null && v !== undefined && typeof v !== 'string') {
          errors.push(`${at} ${cand} 的实参需要空格分隔字符串（坐标写 [x, y]，整体不用引号）`)
        }
        recurse('then', 'then')
        recurse('else', 'else')
      } else {
        // 跨文件函数调用：- 脚本名:函数名: 实参…（脚本名解析与 call 一致：
        // 同分区优先 → 跨分区；缺 .yaml/.yml 扩展名自动补全）
        const cross = ks.find(k => k !== 'then' && k !== 'else' && k.includes(':'))
        if (cross) {
          for (const k of ks) {
            if (k !== cross && k !== 'then' && k !== 'else') errors.push(`${at} ${cross} 调用不支持参数 ${k}（可用：then / else）`)
          }
          const parts = cross.split(':').map(x => x.trim())
          const v = step[cross]
          if (v !== null && v !== undefined && typeof v !== 'string') {
            errors.push(`${at} ${cross} 的实参需要空格分隔字符串（坐标写 [x, y]，整体不用引号）`)
          }
          if (parts.length !== 2 || !parts[0] || !parts[1]) {
            errors.push(`${at} 跨文件函数调用需要 "脚本名:函数名"（如 - test1:fun1: 实参…）`)
          } else {
            const [sName, fName] = parts
            const findSub = name => {
              const all = scriptsData.value || []
              const pick = cond => all.find(cond)
              const isYaml = /\.(ya?ml)$/i.test(name)
              const own = pick(s => s.package === activePkg.value && s.name === name)
              if (own) return own
              const any = pick(s => s.name === name)
              if (any) return any
              if (!isYaml) {
                for (const ext of ['.yaml', '.yml']) {
                  const c2 = pick(s => s.package === activePkg.value && s.name === name + ext)
                  if (c2) return c2
                  const c3 = pick(s => s.name === name + ext)
                  if (c3) return c3
                }
              }
              return null
            }
            const sub = findSub(sName)
            if (!sub) {
              errors.push(`${at} 子脚本不存在：${sName}`)
            } else {
              // 函数存在性：与引擎 exec_cross_func 一致，先 normalize_top 归一化
              // 再取 func 段收集函数名（省略 func: 的纯函数库简写同样可被调用）
              try {
                const subFuncs = extractCrossFileFuncNames(sub.content)
                if (!subFuncs.has(fName)) errors.push(`${at} 子脚本 ${sub.name} 未定义函数 ${fName}`)
              } catch (e) {
                errors.push(`${at} 子脚本 ${sub.name || sName} 解析失败：${e.message}`)
              }
            }
          }
          recurse('then', 'then')
          recurse('else', 'else')
        } else {
          errors.push(`${at} 未知动作 ${ks.join('、')}（可用：find / color / loop / tap / swipe / key / text / log / call / throw / str_app / cls_app / wait / return / 自定义函数 / 脚本名:函数名 跨文件调用）`)
        }
      }
    }

    function validateSteps(list, at, inFunc) {
      list.forEach((s, i) => validateStep(s, `${at}第 ${i + 1} 步`, inFunc))
    }

    if (Array.isArray(doc.steps)) validateSteps(doc.steps, '', false)
    for (const f of funcs) validateSteps(f.body, `函数 ${f.name} `, true)
    return errors
  }

  return validateScriptCode
}
