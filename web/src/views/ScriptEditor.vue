<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">脚本编辑</div>
        <div class="page-sub">YAML 自动化脚本 · 支持 find / until / color / tap / swipe / text / key / str_app / cls_app / loop / goto / call / wait，以及 click+check 简写；每个操作可用 wait 参数控制操作后等待（默认取脚本顶层 action_wait，500ms；str_app 默认 3000ms）</div>
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
            <div class="hb-title">⚙ 顶层配置</div>
            <pre class="hb-code mono"># 脚本按应用分区存放（data/&lt;应用包名&gt;/yaml，分区由编辑器顶部分区下拉决定）
action_wait: 500       # 每个操作后的默认等待 ms（默认 500）
log_level: info        # 日志级别：info=精简（默认） / debug=详细</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🎬 动作</div>
            <pre class="hb-code mono">- wait: [500, 1500]                   # 随机延时
- tap: [0.500, 0.500]                # 相对坐标点击
  wait: 200                          # 操作后等待（默认取脚本 action_wait，0 不等待）
- swipe:
    fm: [0.500, 0.800]               # 滑动起点
    to: [0.500, 0.200]               # 滑动终点
    time: 800                        # 滑动时长 ms
- text: "hello world"                # 输入文本
- key: HOME                          # HOME/BACK/APP_SWITCH/VOL_UP…</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🔍 找图 find</div>
            <pre class="hb-code mono">- find: sign_btn.png     # 查找模板（timeout 必须 > 0）
  interval: 500          # 检测间隔 ms（默认 500，一轮未命中后隔此重试）
  timeout: 6000          # 超时 ms（默认 6000，一直找请用 until）
  click: true            # 找到后点击模板中心（默认 true，false 不点击）
  threshold: 0.85        # 匹配阈值（默认 0.8）
  region: a              # 搜索区域（默认 a=全屏；模板名可带 #后缀 区域各自指定）
  then:                  # 找到后执行
    - log: "找到并点击"
  else:                  # 超时未找到执行
    - log: "等待超时"

- find: dialog.png       # click 也可以是模板名或相对坐标
  click: close_btn.png   # 在 dialog.png 区域内找 close_btn.png，找到点击其中心
  else:
    - log: "对话框没出现"

- find: a.png, b.png     # 多模板：逗号分隔（或列表 [a.png, b.png]）
  and_or: and            # and=全部找到才命中（默认，未命中即停不匹配后面）
  click: true            # and 点第一个模板；or（until 默认）点命中的那个
                         # 各模板区域不同 → 名字带 #后缀：hp#l.png（左半）xx#0_0_500_500.png（左上 1/4）
                         # 脚本也可写短名 login.png（引擎自动解析唯一 login#*.png，区域照常生效）

- find: a.png, b.png     # then 按命中模板分支：单键「模板名: 步骤列表」= 专属分支，and/or 通用
  and_or: or             # or：命中谁走谁；and：全命中取书写顺序第一个分支
  then:
    - b.png:             # 命中的是 b.png 走这里
        - log: "命中了 b"
    - log: "兜底"        # 命中的模板没有专属分支时执行（即原 then 语义）

- until: page_a.png, page_b.png   # 等到模板出现（and_or 默认 or 任一命中）
  timeout: 1800000       # 超时 ms（默认 30 分钟，0=永不超时）
  interval: 500          # 其余参数与 find 一致

- click: login.png       # 简写：等 login.png 出现并点击（无 find/until 键时触发）
  check: act_cls.png     # 可选：check 模板先出现时点击关闭（弹窗等障碍），再继续等 click 目标
                         # 等价 until: login.png, act_cls.png + then 分支 act_cls.png: - until: login.png
  wait: 200              # wait/timeout/interval/threshold/region/else/then 等参数照常透传
                         # click/check 均支持逗号分隔多模板或列表 [a.png, b.png]</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🎨 取点比色 color</div>
            <pre class="hb-code mono">- color: [0.5123, 0.8456]   # 采样点相对坐标 0~1（同 tap；alt 模式点投屏自动生成记录+采样）
  check: ff8800             # 期望颜色：6 位十六进制 RRGGBB（也接受 "#ff8800" / [255, 136, 0]）
  tol: 30                   # 每通道容差 |实际-期望| ≤ tol 判命中（默认 30：H.264 有损压缩帧间像素会抖动）
  count: 1                  # 检测次数（默认 1）：最多检测 count 次，任一次命中走 then
  cnt_ivl: 50               # 相邻检测间隔 ms（默认 50，支持 100 / "100ms"）
  then:                     # 命中执行
    - click: buy_btn.png
  else:                     # 全部未命中执行
    - log: "体力没恢复"</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🔁 逻辑</div>
            <pre class="hb-code mono">- loop: {times: 3, steps: [...]}
- goto: label_name
- label: label_name
- call: 子脚本.yml
- log: "输出到运行日志"</pre>
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

const DEFAULT_CODE = `action_wait: 500
log_level: info

steps:
  - wait: [300, 800]
  - find: sign_btn.png
    threshold: 0.85
    then:
      - log: "点击签到按钮"
    else:
      - log: "未找到签到按钮，重试"
      - goto: retry
  - label: retry
  - loop:
      times: 3
      steps:
        - swipe:
            fm: [0.500, 0.800]
            to: [0.500, 0.200]
            time: 800
        - wait: [300, 900]
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
