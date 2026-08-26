<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">脚本编辑</div>
        <div class="page-sub">YAML 自动化脚本 · 支持 find（找图等待 + block 障碍）/ color 颜色分支 / loop / func 自定义函数（$N 传参 + return + cond 条件）/ 跨文件函数调用（脚本名:函数名）/ tap / swipe / text / key / call / throw / str_app / cls_app / wait；时间参数一律带单位（1ms / 2s / 1m / 30min / 1h / 1d），间隔与阈值用 config: 段配置</div>
      </div>
      <div class="head-actions">
        <button class="btn" @click="validate">✔ 校验</button>
        <button v-if="!store.running" class="btn btn-primary" @click="run">▶ 运行</button>
        <button v-else class="btn btn-danger" @click="stop">■ 停止</button>
        <button class="btn" @click="save">💾 保存</button>
      </div>
    </div>

    <div class="script-layout">
      <!-- 左：脚本列表 -->
      <div class="script-list card">
        <button class="btn btn-sm btn-primary new-btn" @click="newScript">＋ 新建脚本</button>
        <div class="sl-items">
          <div v-for="s in scripts" :key="s.id" class="sl-item" :class="{ sel: s.id === sel?.id }" @click="select(s)">
            <div class="sl-name">{{ s.name }}</div>
            <div class="sl-meta mono"><span class="sl-pkg">{{ s.package }}</span> · {{ fmtTime(s.updated_at) }}</div>
            <button class="sl-del" @click.stop="removeScript(s)" title="删除">🗑</button>
          </div>
        </div>
        <div class="sl-foot">
          <span>YAML 语法说明</span>
          <button class="btn btn-sm btn-ghost" @click="showHelp = true">?</button>
        </div>
      </div>

      <!-- 右：编辑器 -->
      <div class="editor-wrap card">
        <div class="ed-head">
          <div class="ed-file mono">
            <select v-model="edPkg" class="select mono ed-pkg" title="应用分区（保存到 data/<应用包名>/yaml；编辑已有脚本时切换分区，保存后即移动）">
              <option v-if="!pkgOptions.length" value="">（无分区）</option>
              <option v-for="p in pkgOptions" :key="p" :value="p">{{ p }}</option>
            </select>
            <span>{{ sel?.name || '未命名.yml' }}</span>
          </div>
          <div class="ed-status">
            <span class="tag" :class="valid ? 'ok' : 'err'">{{ valid ? '✓ 语法正确' : '✗ 语法错误' }}</span>
          </div>
        </div>

        <div class="editor">
          <div class="gutter mono"><div v-for="(_, i) in codeLines" :key="i">{{ i + 1 }}</div></div>
          <textarea v-model="code" class="code-area mono" spellcheck="false" @input="valid = null" @keydown.tab.prevent="onEditorTab"></textarea>
        </div>
      </div>
    </div>

    <!-- 语法帮助弹窗 -->
    <div v-if="showHelp" class="modal-mask" @click.self="showHelp = false">
      <div class="modal help-modal">
        <div class="modal-head">
          <span class="title">YAML 脚本语法</span>
          <button class="btn btn-ghost btn-sm" @click="showHelp = false">✕</button>
        </div>
        <div class="modal-body">
          <div class="help-block">
            <div class="hb-title">⚙ 顶层结构（只允许 config / func / steps）</div>
            <pre class="hb-code mono"># 脚本按应用分区存放（data/&lt;应用包名&gt;/yaml，分区由编辑器顶部分区下拉决定）
config:                 # 可选：覆盖 config.toml 默认（也可写成映射列表按序覆盖）
  interval: 500ms       # 轮询类间隔（find 每轮重试 / verify 复查）；步骤间不等待
  threshold: 0.85       # 模板匹配阈值
  log_level: info       # debug / info（默认）/ warn / error，低于等级的日志丢弃
func:                   # 可选：自定义函数（见下）
steps:                  # 必需：步骤列表
  - log: "开始"</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🎬 基础动作</div>
            <pre class="hb-code mono">- wait: 2s               # 等待（也可 [1s, 3s] 随机区间）；时间一律带单位
- tap: [0.500, 0.500]    # 相对坐标点击
- swipe:
    fm: [0.500, 0.800]   # 滑动起点
    to: [0.500, 0.200]   # 滑动终点
    time: 800ms          # 滑动时长（省略默认 500ms）
- text: "hello world"    # 输入文本
- key: HOME              # HOME/BACK/APP_SWITCH/VOL_UP…
- log: "输出到运行日志"
- str_app                # 冷启动应用（只写裸名，包名 = 设备分区）
- cls_app                # 关闭应用（adb force-stop，不碰投屏）
- throw                  # 结束整个任务（跨 call）；- throw: 体力不足 带原因</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🔍 找图 find（超时内轮询等模板出现并点击，恒点模板中心）</div>
            <pre class="hb-code mono">- find: sign_btn.png   # 单个主模板（多目标拆成多步；挡路的写 block）
  timeout: 30min        # 超时执行 else（默认 30min，必须 > 0）
  block:                # 障碍模板：主模板未命中后依序匹配，命中即点击其中心
    - pop.png           # 并结束本轮；单个可写 block: pop.png
    - ad.png
  verify: true          # 生效验证（默认 false）：命中点击后等 interval 重匹配主模板，
                         # 仍命中再补一击（共两击，不循环）
  then:                 # 命中执行（^1 = 主模板名、^2.. = block 名，可传参引用）
    - log: "找到 ^1"
  else:                 # 超时执行
    - log: "等待 ^1 超时"
# 每轮：主模板（新截图）→ 命中点击 + verify + then；未命中 → block 依序 →
# 全未命中等 interval 重开一轮。threshold 用 config 配置；
# 搜索区域由模板名 #后缀 决定：hp#l.png（左半）/ xx#0_0_500_500.png（左上 1/4），
# 无后缀回退全屏（运行日志有提醒）；可写短名 login.png（引擎解析唯一 login#*.png）</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🎨 找色 color（一次截图按序判定，命中即执行其步骤并结束）</div>
            <pre class="hb-code mono">- color: [0.5123, 0.8456]   # 采样相对坐标
  ff8800:                   # 色值键 = 6 位十六进制（容差固定 30），挂命中步骤（可留空）
    - log: "命中颜色"
  ff8811:
  else:                     # 全部未命中执行
    - log: "都没命中"
# 不轮询无超时（重试套 loop）。^1 = "[x, y]" 坐标串、^2.. = 色值键（书写顺序）
# 二次裁切区 Alt/alt 模式点击任意处 → 自动生成 color 记录（所见即所得取色）</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🔁 loop / call / func 自定义函数</div>
            <pre class="hb-code mono">- loop:                # times 省略或 0 = 无限循环
  times: 3
  steps:
    - log: "每一轮"
- call: 子脚本.yml a.png [0.5, 0.6]   # 空格分隔实参（[x, y] 括号内不切分），
                                       # 子脚本内 $1/$2… 引用（替换全部字符串，
                                       # 嵌套 call 转发 $N 同样生效）

func:                   # 自定义函数定义（本脚本内调用；cond 可选执行条件）
  - wait_tpl:           # 函数名不能是保留字；体内 $N 指函数实参
    cond: gate.png      # 可选：必须匹配条件模板才执行函数体，否则函数返回 false
    steps:              # 函数体（与 cond 同级；多模板写 cond: [a.png, b.png]）
      - find: $1
        timeout: 6s
      - return: true    # return 仅函数内合法：立即返回；函数体执行完未 return = true
steps:
  - wait_tpl: sign_btn.png   # 调用：空格分隔实参 + then（返回 true）/ else（false）
    then:
      - log: "出现了"
    else:
      - log: "没等到"
  - 通用日常:fun1: a.png     # 跨文件调用：脚本名:函数名 + 实参（解析同 call）
# 嵌套函数调用上限 32 层；throw 在函数内同样结束整个任务</pre>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, nextTick, watch, onMounted, onUnmounted } from 'vue'
import { scriptsData, devicesData, store, useToast } from '../store'
import { api } from '../api'

const toast = useToast()
const scripts = scriptsData
const devices = devicesData
const sel = ref(null)
const code = ref('')
const valid = ref(null)
const showHelp = ref(false)
// 保存目标应用分区（= 应用包名）：编辑已有脚本=其所在分区，新建=当前设备 pkg
const edPkg = ref('')

const DEFAULT_CODE = `config:
  interval: 500ms

func:
  - wait_tpl:
    - find: $1
      timeout: 6s
    - return: true

steps:
  - wait: [300ms, 800ms]
  - wait_tpl: sign_btn.png
    then:
      - log: "点击签到按钮"
    else:
      - log: "未找到签到按钮，滑动后重试"
  - loop:
    times: 3
    steps:
      - swipe:
          fm: [0.500, 0.800]
          to: [0.500, 0.200]
          time: 800ms
      - wait: [300ms, 900ms]
  - key: HOME
  - log: "签到完成"
`

const codeLines = computed(() => code.value.split('\n'))
const fmtTime = s => (s || '').slice(0, 16)

/** 分区下拉选项：当前设备配置的应用包名 ∪ 已有脚本分区 */
const pkgOptions = computed(() => {
  const set = new Set()
  const dp = (devices.value.find(d => d.id === store.deviceId)?.pkg || '').trim()
  if (dp) set.add(dp)
  for (const s of scripts.value) if (s.package) set.add(s.package)
  return [...set].sort()
})
watch(pkgOptions, list => { if (!edPkg.value) edPkg.value = list[0] || '' }, { immediate: true })

function select(s) {
  sel.value = s
  code.value = s.content
  edPkg.value = s.package || ''
  valid.value = null
}

function newScript() {
  sel.value = { id: null, name: '新脚本.yml', content: DEFAULT_CODE, updated_at: '' }
  code.value = DEFAULT_CODE
  const dp = (devices.value.find(d => d.id === store.deviceId)?.pkg || '').trim()
  edPkg.value = dp || pkgOptions.value[0] || ''
  valid.value = null
}

/** 编辑区 Tab 键：插入 2 个空格（代替切换焦点）；多行选中时逐行缩进，Shift+Tab 行首退格 */
function onEditorTab(e) {
  const ta = e.target
  const start = ta.selectionStart
  const end = ta.selectionEnd
  const v = code.value
  if (start === end) {
    const lineStart = v.lastIndexOf('\n', start - 1) + 1
    const before = v.slice(lineStart, start)
    if (e.shiftKey) {
      // Shift+Tab：删除行首 1~2 个空格
      const m = before.match(/^ {1,2}/)
      if (m) {
        code.value = v.slice(0, lineStart) + v.slice(lineStart + m[0].length)
        nextTick(() => { ta.selectionStart = ta.selectionEnd = start - m[0].length })
      }
      return
    }
    code.value = v.slice(0, start) + '  ' + v.slice(end)
    nextTick(() => { ta.selectionStart = ta.selectionEnd = start + 2 })
    return
  }
  const sel = v.slice(start, end)
  if (sel.includes('\n')) {
    // 多行选中：每行前插 2 空格
    const lineStart = v.lastIndexOf('\n', start - 1) + 1
    const indented = v.slice(lineStart, end).split('\n').map(l => '  ' + l).join('\n')
    code.value = v.slice(0, lineStart) + indented + v.slice(end)
    const newEnd = lineStart + indented.length
    nextTick(() => { ta.selectionStart = lineStart; ta.selectionEnd = newEnd })
  } else {
    code.value = v.slice(0, start) + '  ' + v.slice(end)
    nextTick(() => { ta.selectionStart = ta.selectionEnd = start + 2 })
  }
}

function validate() {
  valid.value = code.value.includes('steps:')
  toast(valid.value ? '语法校验通过' : '缺少 steps: 根节点', valid.value ? 'success' : 'error')
}

async function save() {
  if (!sel.value) return toast('请先选择或新建脚本', 'error')
  if (!sel.value.name) return toast('请填写脚本名称', 'error')
  if (!edPkg.value) return toast('请先选择应用分区', 'warn')
  try {
    const r = await api.saveScript({ id: sel.value.id, name: sel.value.name, content: code.value, pkg: edPkg.value })
    await loadScripts()
    // 分区/名称可能变化（id 变化），按返回 id 重新定位
    sel.value = scripts.value.find(s => s.id === r.id) || sel.value
    toast('已保存', 'success')
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  }
}

// 运行状态轮询：服务端异步执行脚本（run 接口立即返回），
// 轮询 status 直到脚本真正结束，才复位运行状态（按钮/顶栏芯片随之恢复）
let runStatusTimer = null

function startRunStatusPoll() {
  if (runStatusTimer) clearInterval(runStatusTimer)
  checkRunStatus()
  runStatusTimer = setInterval(checkRunStatus, 1000)
}

function stopRunStatusPoll() {
  if (runStatusTimer) { clearInterval(runStatusTimer); runStatusTimer = null }
}

async function checkRunStatus() {
  if (!store.running || !store.runScriptId) { stopRunStatusPoll(); return }
  try {
    const st = await api.scriptStatus(store.runScriptId)
    if (!st.running) {
      store.running = false
      store.runScriptId = null
      stopRunStatusPoll()
      toast('脚本已结束', 'info')
    }
  } catch (e) {}
}

async function run() {
  if (!sel.value?.id) return toast('请先保存脚本', 'error')
  if (!store.deviceId) return toast('请先选择设备（投屏控制 → 设备页签）', 'error')
  try {
    store.running = true
    store.runScript = sel.value.name
    store.runScriptId = sel.value.id
    await api.runScript(sel.value.id, store.deviceId)
    toast('脚本已开始运行', 'success')
    startRunStatusPoll()
  } catch (e) {
    store.running = false
    store.runScriptId = null
    toast('运行失败：' + e.message, 'error')
  }
}

function stop() {
  if (!store.runScriptId) return
  api.stopScript(store.runScriptId).catch(() => {})
  store.running = false
  store.runScriptId = null
  stopRunStatusPoll()
  toast('已发送停止指令，脚本将在当前步骤结束后停止', 'warn')
}

async function removeScript(s) {
  if (!s.id) return
  if (!confirm(`删除脚本 ${s.name}？`)) return
  try {
    await api.deleteScript(s.id)
    scripts.value = scripts.value.filter(x => x.id !== s.id)
    if (sel.value?.id === s.id) sel.value = null
    toast('脚本已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

async function loadScripts() {
  try { scripts.value = await api.listScripts() } catch (e) {}
}
async function loadDevices() {
  try { devices.value = await api.listDevices() } catch (e) {}
}

onMounted(() => {
  loadScripts(); loadDevices()
  // 其他页面已启动脚本时，本页接管状态轮询（脚本结束后复位运行状态）
  if (store.running && store.runScriptId) startRunStatusPoll()
})
onUnmounted(() => stopRunStatusPoll())
</script>

<style scoped>
.head-actions { display: flex; gap: 10px; }

.script-layout { display: grid; grid-template-columns: 260px 1fr; gap: 14px; flex: 1; min-height: 0; }

.script-list { display: flex; flex-direction: column; gap: 10px; padding: 12px; overflow: hidden; }
.new-btn { justify-content: center; }
.sl-items { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 6px; }
.sl-item {
  padding: 10px 12px; border-radius: var(--radius-sm); cursor: pointer;
  border: 1px solid transparent; position: relative; transition: all .15s;
}
.sl-item:hover { background: var(--bg-3); }
.sl-item.sel { background: rgba(34,211,165,.08); border-color: rgba(34,211,165,.35); }
.sl-name { font-size: 13px; font-weight: 600; padding-right: 32px; }
.sl-meta { font-size: 11px; color: var(--text-2); margin-top: 4px; }
.sl-pkg { color: var(--accent-2); }
.sl-del {
  position: absolute; right: 8px; top: 10px; background: none; border: none;
  color: var(--text-2); cursor: pointer; font-size: 12px;
}
.sl-del:hover { color: var(--danger); }
.sl-foot { display: flex; align-items: center; justify-content: space-between; font-size: 11px; color: var(--text-2); border-top: 1px solid var(--border); padding-top: 10px; }

.editor-wrap { display: flex; flex-direction: column; gap: 10px; min-height: 0; padding: 12px; }
.ed-head { display: flex; justify-content: space-between; align-items: center; }
.ed-file { display: flex; align-items: center; gap: 8px; min-width: 0; }
.ed-file > span { font-size: 13px; color: var(--text-0); font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ed-pkg { max-width: 220px; font-size: 12px; }

.editor { flex: 1; position: relative; display: flex; background: var(--bg-0); border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: auto; min-height: 200px; }
.gutter {
  padding: 12px 10px; text-align: right; color: var(--text-2);
  font-size: 12px; line-height: 1.65; user-select: none;
  border-right: 1px solid var(--border); flex-shrink: 0;
}
.code-area {
  flex: 1; padding: 12px; border: none; outline: none; background: transparent;
  color: #c9d4e8; font-size: 12px; line-height: 1.65; resize: none; min-width: 0;
}

.help-modal { min-width: 560px; }
.help-block { display: flex; flex-direction: column; gap: 6px; }
.hb-title { font-size: 13px; font-weight: 600; color: var(--accent); }
.hb-code {
  background: var(--bg-0); border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 10px 12px; font-size: 12px; line-height: 1.6; color: #c9d4e8; overflow: auto;
}
</style>
