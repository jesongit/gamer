<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">脚本编辑</div>
        <div class="page-sub">YAML 自动化脚本 · 支持 find / click / swipe / text / key / loop / if / goto / call / random_delay</div>
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
          <span>YAML v1 语法说明</span>
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
          <textarea v-model="code" class="code-area mono" spellcheck="false" @input="valid = null"></textarea>
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
            <pre class="hb-code mono">- wait: 2000                          # 等待毫秒
- click: {x: 540, y: 1680}           # 点击坐标
- click: "@found"                    # 点击上一个 find 的命中点
- swipe: {from: [500,1800], to: [500,600], duration: 800}
- text: "hello world"                # 输入文本
- key: HOME                          # HOME/BACK/APP_SWITCH/VOL_UP…
- start_app: {package: "com.game.xxx"}
- random_delay: {min: 500, max: 1500} # 随机延时（模拟人工）</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🔍 找图</div>
            <pre class="hb-code mono">- find: {template: sign_btn.png, timeout: 10000, threshold: 0.85}
  then:
    - click: "@found"
  else:
    - log: "未找到签到按钮"

- loop_until_find: {template: done.png, timeout: 30000, steps:
    - wait: 1000
    - swipe: {from: [500,1800], to: [500,600], duration: 500}}</pre>
          </div>
          <div class="help-block">
            <div class="hb-title">🔁 逻辑</div>
            <pre class="hb-code mono">- loop: {times: 3, steps: [...]}
- goto: label_name
- label: label_name
- call: 子脚本.yml
- if_find: {template: x.png, then: [...], else: [...]}
- log: "输出到运行日志"</pre>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, onMounted } from 'vue'
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
device: auto

steps:
  - wait: 2000
  - find:
      template: sign_btn.png
      timeout: 10000
      threshold: 0.85
    then:
      - click: "@found"
    else:
      - log: "未找到签到按钮，重试"
      - goto: retry
  - label: retry
  - loop:
      times: 3
      steps:
        - swipe: {from: [500, 1800], to: [500, 600], duration: 800}
        - random_delay: {min: 300, max: 900}
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
  if (!store.deviceId) return toast('请先在设备列表选择设备', 'error')
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
