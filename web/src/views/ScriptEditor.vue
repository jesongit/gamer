<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">脚本编辑</div>
        <div class="page-sub">YAML 自动化脚本 · 支持 find / tap / swipe / text / key / loop / goto / call / wait；每个操作可用 wait 参数控制操作后等待（默认取脚本顶层 action_wait，500ms）</div>
      </div>
      <div class="head-actions">
        <button class="btn" @click="validate">✔ 校验</button>
        <button class="btn btn-primary" @click="run">▶ 运行</button>
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
            <div class="sl-meta mono">{{ fmtTime(s.updated_at) }}</div>
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
          <span class="mono">{{ sel?.name || '未命名.yml' }}</span>
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
            <pre class="hb-code mono">- find: sign_btn.png     # 查找模板
  interval: 500          # 检测间隔 ms（默认 500）
  timeout: 6000          # 超时 ms（默认 6000，0=一直找）
  click: true            # 找到后点击模板中心（默认 false）
  threshold: 0.85        # 匹配阈值（默认 0.8）
  region: a              # 搜索区域（默认 a=全屏）
  then:                  # 找到后执行
    - log: "找到并点击"
  else:                  # 超时未找到执行
    - log: "等待超时"

- find: dialog.png       # click 也可以是模板名或相对坐标
  click: close_btn.png   # 在 dialog.png 区域内找 close_btn.png，找到点击其中心
  else:
    - log: "对话框没出现"

- find: dialog.png
  click: [0.5, 0.1]      # 点击 dialog.png 区域内的相对坐标 [0.5,0.1]
  timeout: 0             # 不超时（一直找）</pre>
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
import { ref, computed, nextTick, onMounted } from 'vue'
import { scriptsData, devicesData, store, useToast } from '../store'
import { api } from '../api'

const toast = useToast()
const scripts = scriptsData
const devices = devicesData
const sel = ref(null)
const code = ref('')
const valid = ref(null)
const showHelp = ref(false)

const DEFAULT_CODE = `name: 每日签到
action_wait: 500

steps:
  - wait: [300, 800]
  - find: sign_btn.png
    threshold: 0.85
    click: true
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

function select(s) {
  sel.value = s
  code.value = s.content
  valid.value = null
}

function newScript() {
  sel.value = { id: null, name: '新脚本.yml', content: DEFAULT_CODE, updated_at: '' }
  code.value = DEFAULT_CODE
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
  try {
    await api.saveScript({ id: sel.value.id, name: sel.value.name, content: code.value })
    await loadScripts()
    const saved = scripts.value.find(s => s.name === sel.value.name) || scripts.value[0]
    sel.value = saved || sel.value
    toast('已保存', 'success')
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  }
}

async function run() {
  if (!sel.value?.id) return toast('请先保存脚本', 'error')
  if (!store.deviceId) return toast('请先选择设备（投屏控制 → 设备页签）', 'error')
  try {
    await api.runScript(sel.value.id, store.deviceId)
    store.running = true
    store.runScript = sel.value.name
    toast('脚本已开始运行', 'success')
  } catch (e) {
    toast('运行失败：' + e.message, 'error')
  }
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

onMounted(() => { loadScripts(); loadDevices() })
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
.sl-del {
  position: absolute; right: 8px; top: 10px; background: none; border: none;
  color: var(--text-2); cursor: pointer; font-size: 12px;
}
.sl-del:hover { color: var(--danger); }
.sl-foot { display: flex; align-items: center; justify-content: space-between; font-size: 11px; color: var(--text-2); border-top: 1px solid var(--border); padding-top: 10px; }

.editor-wrap { display: flex; flex-direction: column; gap: 10px; min-height: 0; padding: 12px; }
.ed-head { display: flex; justify-content: space-between; align-items: center; }
.ed-head .mono { font-size: 13px; color: var(--text-0); font-weight: 600; }

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
