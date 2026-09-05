<template>
  <section class="keymap-panel" data-testid="keymap-panel">
    <div class="keymap-head">
      <div>
        <div class="keymap-title">按键映射</div>
        <div class="keymap-sub mono">
          {{ pkg ? '分区：' + pkg + ' · ' + keymaps.length + ' 个方案' : '请先选择应用分区' }}
        </div>
      </div>
      <div class="head-actions">
        <button v-if="hasCallback('onRefresh')" class="btn btn-sm" type="button" :disabled="loading" @click="invoke('onRefresh')">↻ 刷新</button>
        <button class="btn btn-sm btn-primary" type="button" :disabled="!pkg || loading || editing" @click="startNew">＋ 新增映射</button>
      </div>
    </div>

    <div v-if="error" class="keymap-error" role="alert" data-testid="keymap-error">{{ error }}</div>
    <div v-if="note" class="keymap-note" role="status" data-testid="keymap-note">{{ note }}</div>
    <div v-if="!pkg" class="keymap-empty" data-testid="keymap-no-package">暂无应用分区：请先在右侧包名下拉中选择包名</div>

    <template v-else>
      <div class="scheme-list" data-testid="keymap-scheme-list">
        <div class="list-head">
          <span>方案</span><span>绑定</span><span>版本</span><span>操作</span>
        </div>
        <div v-if="loading && !keymaps.length" class="list-empty">读取中…</div>
        <div v-else-if="!keymaps.length" class="list-empty">暂无映射方案，点击「新增映射」创建</div>
        <div
          v-for="item in keymaps"
          :key="itemName(item)"
          class="scheme-row"
          :class="{ selected: itemName(item) === selectedName }"
          data-testid="keymap-scheme-row"
          @click="selectScheme(item)"
        >
          <div class="scheme-name">
            <span class="scheme-label" :title="itemName(item)">{{ itemName(item) }}</span>
            <span v-if="itemName(item) === usedName" class="using-tag">使用中</span>
          </div>
          <span class="mono">{{ bindingCount(item) }}</span>
          <span class="mono">v{{ serverVersion(item) }}</span>
          <span class="row-actions">
            <button class="mini-btn" type="button" @click.stop="startEdit(item)">编辑</button>
            <button class="mini-btn" type="button" @click.stop="startCopy(item)">复制</button>
            <button class="mini-btn danger" type="button" :class="{ armed: deleteName === itemName(item) }" @click.stop="requestDelete(item)">
              {{ deleteName === itemName(item) ? '确认删除' : '删除' }}
            </button>
          </span>
        </div>
      </div>

      <div v-if="editing" class="keymap-editor" data-testid="keymap-editor">
        <div class="editor-toolbar">
          <div class="editor-mode" role="group" aria-label="编辑模式">
            <button type="button" class="mode-btn" :class="{ active: editMode === 'visual' }" @click="switchMode('visual')">可视化编辑</button>
            <button type="button" class="mode-btn" :class="{ active: editMode === 'raw' }" @click="switchMode('raw')">原文 YAML</button>
          </div>
          <span v-if="isNew" class="editor-badge">新方案</span>
        </div>

        <template v-if="editMode === 'visual'">
          <label class="name-field">
            <span>方案名称</span>
            <input v-model="draft.name" class="input" data-testid="keymap-name" placeholder="例如：战斗方案" />
          </label>
          <div class="binding-toolbar">
            <span class="section-title">绑定列表（{{ draft.bindings.length }}）</span>
            <button class="btn btn-sm" type="button" @click="addBinding">＋ 添加绑定</button>
          </div>
          <div class="binding-list" data-testid="keymap-binding-list">
            <div v-if="!draft.bindings.length" class="list-empty">暂无绑定，点击「添加绑定」开始配置</div>
            <div v-for="(binding, index) in draft.bindings" :key="index" class="binding-card" data-testid="keymap-binding">
              <div class="binding-head">
                <span class="binding-index mono">{{ index + 1 }}</span>
                <input
                  :ref="el => setKeyInputRef(index, el)"
                  v-model="binding.key"
                  class="input key-input mono"
                  :placeholder="'KeyboardEvent.code，例如 KeyW'"
                  @keydown="captureIndex === index && captureBindingKey($event, index)"
                />
                <button class="btn btn-sm" type="button" :class="{ active: captureIndex === index }" @click="toggleCapture(index, $event)">
                  {{ captureIndex === index ? '按任意键…' : '录入按键' }}
                </button>
                <button class="icon-btn danger" type="button" title="删除绑定" @click="removeBinding(index)">✕</button>
              </div>
              <div class="action-row">
                <label class="action-type">
                  <span>动作</span>
                  <select v-model="binding.action.type" class="select" @change="changeAction(binding)">
                    <!-- tap 只为读取/编辑旧方案保留；新绑定不会生成 tap。 -->
                    <option v-if="binding.action.type === 'tap'" value="tap">tap · 旧版点击（兼容）</option>
                    <option value="swipe">swipe · 滑动</option>
                    <option value="raw_key">raw_key · 真实 Android 按键</option>
                    <option value="hold">hold · 屏幕触控按住</option>
                  </select>
                </label>

                <template v-if="binding.action.type === 'tap' || binding.action.type === 'hold'">
                  <span class="coord-label">{{ binding.action.type === 'hold' ? '触控点（按下/释放）' : '点击位置' }}</span>
                  <label class="coord"><span>X</span><input v-model.number="binding.action.at[0]" class="input mono" type="number" min="0" max="1" step="0.01" /></label>
                  <label class="coord"><span>Y</span><input v-model.number="binding.action.at[1]" class="input mono" type="number" min="0" max="1" step="0.01" /></label>
                  <button class="mini-btn" type="button" @click="requestPoint(index, 'at')">取点</button>
                  <span v-if="binding.action.type === 'hold'" class="action-hint">快速按键 = touch down → up；按住期间不重复发送</span>
                </template>

                <template v-else-if="binding.action.type === 'swipe'">
                  <span class="coord-label">起点</span>
                  <label class="coord"><span>X</span><input v-model.number="binding.action.from[0]" class="input mono" type="number" min="0" max="1" step="0.01" /></label>
                  <label class="coord"><span>Y</span><input v-model.number="binding.action.from[1]" class="input mono" type="number" min="0" max="1" step="0.01" /></label>
                  <button class="mini-btn" type="button" @click="requestPoint(index, 'from')">取点</button>
                  <span class="coord-label">终点</span>
                  <label class="coord"><span>X</span><input v-model.number="binding.action.to[0]" class="input mono" type="number" min="0" max="1" step="0.01" /></label>
                  <label class="coord"><span>Y</span><input v-model.number="binding.action.to[1]" class="input mono" type="number" min="0" max="1" step="0.01" /></label>
                  <button class="mini-btn" type="button" @click="requestPoint(index, 'to')">取点</button>
                  <label class="duration"><span>时长</span><input v-model.number="binding.action.duration_ms" class="input mono" type="number" min="1" max="600000" /><span>ms</span></label>
                </template>

                <template v-else>
                  <label class="raw-field"><span>code</span><input v-model="binding.action.code" class="input mono" placeholder="KeyA" /></label>
                  <span class="or-label">或</span>
                  <label class="raw-field"><span>keycode</span><input v-model.number="binding.action.keycode" class="input mono" type="number" min="1" max="1000" placeholder="29" /></label>
                </template>
              </div>
            </div>
          </div>
        </template>

        <template v-else>
          <textarea v-model="rawYaml" class="raw-yaml" data-testid="keymap-raw" spellcheck="false" aria-label="按键映射 YAML 原文"></textarea>
          <div class="raw-hint">独立 keymap schema：version / name / bindings；坐标必须是 0~1 归一化值。</div>
        </template>

        <div v-if="draftErrors.length" class="diagnostic-list" data-testid="keymap-diagnostics">
          <div v-for="item in draftErrors" :key="item.path + item.message">{{ item.path }}：{{ item.message }}</div>
        </div>
        <div class="editor-foot">
          <button class="btn btn-sm" type="button" @click="cancelEdit">取消</button>
          <button class="btn btn-sm btn-primary" type="button" :disabled="saving" @click="saveDraft">{{ saving ? '保存中…' : '💾 保存方案' }}</button>
        </div>
      </div>

      <div v-else class="keymap-preview" data-testid="keymap-preview">
        <template v-if="selectedItem">
          <div class="preview-head">
            <span class="section-title">{{ itemName(selectedItem) }}</span>
            <span v-if="itemName(selectedItem) === usedName" class="using-tag">使用中</span>
          </div>
          <div class="preview-sub mono">{{ bindingCount(selectedItem) }} 个绑定 · v{{ serverVersion(selectedItem) }}</div>
          <div class="preview-bindings">
            <div v-for="binding in selectedModel.bindings" :key="binding.key" class="preview-binding">
              <span class="key-chip mono">{{ binding.key }}</span>
              <span>{{ actionLabel(binding.action) }}</span>
            </div>
            <div v-if="!selectedModel.bindings.length" class="list-empty">暂无绑定</div>
          </div>
          <button class="btn btn-sm btn-primary preview-edit" type="button" @click="startEdit(selectedItem)">编辑此方案</button>
        </template>
        <div v-else class="list-empty">选择一个方案查看详情</div>
      </div>
    </template>
  </section>
</template>

<script setup>
import { computed, nextTick, ref, watch } from 'vue'
import { dump, load } from 'js-yaml'
import { normalizeKeymap, validateKeymap } from '../../keymap-control'

const props = defineProps({
  /*
   * Single integration prop. Supported optional fields:
   * pkg, keymaps, selectedName, usedName, model, loading, saving, error,
   * onRefresh, onNew, onEdit, onCopy, onDelete, onSave, onCancel,
   * onSelect, onRequestPoint (resolves to a normalized { x, y } point).
   *
   * keymaps items may contain name/file, version, binding_count, model/keymap,
   * bindings, or yaml/content. onSave receives { pkg, name, model, yaml,
   * expected_version, source }.
   */
  context: { type: Object, required: true },
})

const ctx = props.context
const editing = ref(false)
const isNew = ref(false)
const editMode = ref('visual')
const selectedName = ref(String(read('selectedName', '') || ''))
const draft = ref(emptyModel('新建方案'))
const rawYaml = ref('')
const draftErrors = ref([])
const captureIndex = ref(-1)
const keyInputEls = ref([])
const deleteName = ref('')
const note = ref('')

function read(name, fallback) {
  const value = ctx[name]
  if (value && typeof value === 'object' && 'value' in value) return value.value
  return value === undefined ? fallback : value
}

function callback(name) {
  return typeof ctx[name] === 'function' ? ctx[name] : null
}

function hasCallback(name) {
  return !!callback(name)
}

function invoke(name, payload) {
  const fn = callback(name)
  return fn ? fn(payload) : undefined
}

function clone(value) {
  if (typeof structuredClone === 'function') {
    try { return structuredClone(value) } catch (error) { /* Vue 响应式代理走 JSON 兜底 */ }
  }
  return JSON.parse(JSON.stringify(value))
}

function emptyModel(name) {
  return { version: 1, name: name || '新建方案', bindings: [] }
}

function itemName(item) {
  return String(item && (item.name || item.file || item.id) || '')
}

function serverVersion(item) {
  // 内容版本短码（内容哈希字符串）；空值回退 1，超长截短便于表格展示
  const value = item && item.version
  if (value === undefined || value === null || value === '') return 1
  const text = String(value)
  return text.length > 8 ? text.slice(0, 8) : text
}

function sourceModel(item) {
  const contextModel = read('model', null)
  let source = item && (item.model || item.keymap || item.config)
  if (!source && item && typeof (item.yaml || item.content) === 'string') {
    try { source = load(item.yaml || item.content) } catch (error) { source = null }
  }
  if (!source && item && Array.isArray(item.bindings)) {
    source = { version: 1, name: itemName(item), bindings: item.bindings }
  }
  if (!source && contextModel && typeof contextModel === 'object'
    && (!item || !contextModel.name || contextModel.name === itemName(item))) source = contextModel
  if (!source || typeof source !== 'object' || Array.isArray(source)) source = emptyModel(itemName(item))
  return {
    version: Number.isInteger(source.version) ? source.version : 1,
    name: String(source.name || itemName(item) || '未命名方案'),
    bindings: Array.isArray(source.bindings) ? clone(source.bindings) : [],
  }
}

function publicModel(value) {
  return {
    version: value.version,
    name: value.name,
    bindings: (value.bindings || []).map(binding => ({
      key: binding.key,
      action: clone(binding.action),
    })),
  }
}

const pkg = computed(() => String(read('pkg', read('activePkg', '')) || ''))
const keymaps = computed(() => {
  const value = read('keymaps', [])
  return Array.isArray(value) ? value : []
})
const loading = computed(() => !!read('loading', false))
const saving = computed(() => !!read('saving', false))
const error = computed(() => String(read('error', '') || ''))
const usedName = computed(() => String(read('usedName', read('selectedName', '')) || ''))
const selectedItem = computed(() => keymaps.value.find(item => itemName(item) === selectedName.value) || null)
const selectedModel = computed(() => selectedItem.value ? sourceModel(selectedItem.value) : emptyModel(''))

// Console 的工具条也能切换方案；编辑器空闲时跟随父层选择，编辑草稿期间不被
// 异步详情加载覆盖，避免切换工具条丢失未保存内容。
watch(() => read('selectedName', ''), value => {
  if (!editing.value) selectedName.value = String(value || '')
})
watch(pkg, () => {
  if (!editing.value) selectedName.value = String(read('selectedName', '') || '')
})

function bindingCount(item) {
  if (Number.isInteger(item && item.binding_count)) return item.binding_count
  return sourceModel(item).bindings.length
}

function actionLabel(action) {
  const a = action || {}
  if (a.type === 'tap') return '点击 (' + (a.at?.[0] ?? 0) + ', ' + (a.at?.[1] ?? 0) + ')'
  if (a.type === 'swipe') return '滑动 (' + (a.from?.[0] ?? 0) + ', ' + (a.from?.[1] ?? 0) + ') → (' + (a.to?.[0] ?? 0) + ', ' + (a.to?.[1] ?? 0) + ')'
  if (a.type === 'hold') return '按住 (' + (a.at?.[0] ?? 0) + ', ' + (a.at?.[1] ?? 0) + ')'
  return '原始按键 ' + (a.code || a.keycode || '—')
}

function newDraft(name) {
  const value = emptyModel(name)
  draft.value = value
  rawYaml.value = dump(publicModel(value), { lineWidth: -1 })
  draftErrors.value = []
}

function startNew() {
  selectedName.value = ''
  newDraft('新建方案')
  isNew.value = true
  editing.value = true
  editMode.value = 'visual'
  note.value = ''
  invoke('onNew', { pkg: pkg.value, model: publicModel(draft.value) })
}

function startEdit(item) {
  selectedName.value = itemName(item)
  draft.value = sourceModel(item)
  rawYaml.value = dump(publicModel(draft.value), { lineWidth: -1 })
  draftErrors.value = []
  isNew.value = false
  editing.value = true
  editMode.value = 'visual'
  captureIndex.value = -1
  deleteName.value = ''
  note.value = ''
  invoke('onEdit', item)
}

function startCopy(item) {
  selectedName.value = ''
  draft.value = sourceModel(item)
  draft.value.name = draft.value.name + ' 副本'
  rawYaml.value = dump(publicModel(draft.value), { lineWidth: -1 })
  draftErrors.value = []
  isNew.value = true
  editing.value = true
  editMode.value = 'visual'
  invoke('onCopy', { pkg: pkg.value, source: item, model: publicModel(draft.value) })
}

function selectScheme(item) {
  if (editing.value) return
  selectedName.value = itemName(item)
  deleteName.value = ''
  invoke('onSelect', item)
}

function requestDelete(item) {
  const name = itemName(item)
  if (deleteName.value !== name) {
    deleteName.value = name
    note.value = '再次点击确认删除「' + name + '」'
    return
  }
  deleteName.value = ''
  if (selectedName.value === name) selectedName.value = ''
  invoke('onDelete', { pkg: pkg.value, name, source: item })
}

function addBinding() {
  draft.value.bindings.push({
    key: '',
    action: { type: 'hold', at: [0.5, 0.5] },
  })
}

function removeBinding(index) {
  draft.value.bindings.splice(index, 1)
  if (captureIndex.value === index) captureIndex.value = -1
}

function changeAction(binding) {
  const type = binding.action.type
  if (type === 'tap' || type === 'hold') binding.action = { type, at: [0.5, 0.5] }
  if (type === 'swipe') binding.action = { type, from: [0.35, 0.5], to: [0.65, 0.5], duration_ms: 300 }
  if (type === 'raw_key') binding.action = { type, code: '' }
}

async function requestPoint(index, field) {
  note.value = '请在投屏画面中点击要绑定的位置'
  let point = invoke('onRequestPoint', { pkg: pkg.value, index, field })
  if (point && typeof point.then === 'function') point = await point
  if (!point || !draft.value.bindings[index]) return

  const action = draft.value.bindings[index].action
  const value = [Number(point.x), Number(point.y)]
  if (!Number.isFinite(value[0]) || !Number.isFinite(value[1])) return
  if (field === 'at' && (action.type === 'tap' || action.type === 'hold')) action.at = value
  else if (field === 'from' && action.type === 'swipe') action.from = value
  else if (field === 'to' && action.type === 'swipe') action.to = value
  else return
  note.value = `已取点：${value[0].toFixed(4)}, ${value[1].toFixed(4)}`
}

function setKeyInputRef(index, el) {
  if (el) keyInputEls.value[index] = el
  else delete keyInputEls.value[index]
}

function toggleCapture(index, event) {
  captureIndex.value = captureIndex.value === index ? -1 : index
  if (captureIndex.value >= 0) {
    note.value = '按下实际键盘按键后会保存 KeyboardEvent.code'
    // 点击「录入按键」即进入输入状态；无需再手动点击旁边的输入框。
    const button = event?.currentTarget
    button?.parentElement?.querySelector('.key-input')?.focus()
    nextTick(() => {
      const input = keyInputEls.value[index] || button?.parentElement?.querySelector('.key-input')
      input?.focus()
    })
  }
}

function captureBindingKey(event, index) {
  if (!event.code) return
  event.preventDefault()
  draft.value.bindings[index].key = event.code
  captureIndex.value = -1
}

function parseRaw() {
  try {
    const parsed = load(rawYaml.value)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('YAML 根节点必须是对象')
    draft.value = {
      version: parsed.version,
      name: parsed.name,
      bindings: Array.isArray(parsed.bindings) ? clone(parsed.bindings) : [],
    }
    draftErrors.value = []
    return true
  } catch (parseError) {
    draftErrors.value = [{ path: '$', message: 'YAML 解析失败：' + (parseError.message || parseError) }]
    return false
  }
}

function switchMode(nextMode) {
  if (nextMode === editMode.value) return
  if (nextMode === 'raw') {
    rawYaml.value = dump(publicModel(draft.value), { lineWidth: -1 })
    editMode.value = 'raw'
    return
  }
  if (!parseRaw()) return
  editMode.value = 'visual'
}

async function saveDraft() {
  if (editMode.value === 'raw' && !parseRaw()) return
  const value = publicModel(draft.value)
  const validation = validateKeymap(value)
  draftErrors.value = validation.issues
  if (!validation.valid) return
  const model = normalizeKeymap(value)
  const source = isNew.value ? null : selectedItem.value
  let result
  try {
    result = invoke('onSave', {
      pkg: pkg.value,
      name: model.name,
      model,
      yaml: dump(model, { lineWidth: -1 }),
      expected_version: source ? source.version : undefined,
      source,
    })
    if (result && typeof result.then === 'function') result = await result
  } catch (saveError) {
    note.value = '保存失败：' + (saveError.message || saveError)
    return
  }
  // onSave 在服务端请求完成且成功后必须明确返回 true，避免把“已发起请求”误报成成功。
  if (result === true) {
    editing.value = false
    isNew.value = false
    captureIndex.value = -1
    note.value = '保存成功（服务端已确认）'
    return
  }
  note.value = '保存失败：服务端未确认，请重试'
}

function cancelEdit() {
  invoke('onCancel', { pkg: pkg.value, name: selectedName.value, source: selectedItem.value })
  editing.value = false
  isNew.value = false
  captureIndex.value = -1
  draftErrors.value = []
  note.value = ''
}
</script>

<style scoped>
.keymap-panel { display: flex; flex: 1; min-height: 0; flex-direction: column; gap: 10px; overflow: hidden; }
.keymap-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; flex-shrink: 0; }
.keymap-title { font-size: 16px; font-weight: 700; }
.keymap-sub { margin-top: 3px; color: var(--text-2); }
.head-actions { display: flex; gap: 6px; }
.keymap-empty, .list-empty { padding: 20px 10px; text-align: center; color: var(--text-2); font-size: 12px; }
.keymap-error, .keymap-note, .diagnostic-list { padding: 6px 8px; border-radius: var(--radius-sm); font-size: 11px; line-height: 1.5; }
.keymap-error, .diagnostic-list { color: var(--danger, #ef6b73); border: 1px solid rgba(239, 107, 115, .35); background: rgba(239, 107, 115, .08); }
.keymap-note { color: var(--accent-2); border: 1px solid rgba(56, 189, 248, .3); background: rgba(56, 189, 248, .07); }
.scheme-list { flex: none; min-height: 80px; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden; }
.list-head, .scheme-row { display: grid; grid-template-columns: minmax(0, 1fr) 40px 38px auto; align-items: center; gap: 7px; padding: 6px 8px; font-size: 11px; }
.list-head { color: var(--text-2); border-bottom: 1px solid var(--border); background: var(--bg-2); }
.scheme-row { min-height: 34px; color: var(--text-1); border-bottom: 1px solid rgba(80, 92, 119, .25); cursor: pointer; }
.scheme-row:last-child { border-bottom: 0; }
.scheme-row:hover, .scheme-row.selected { background: var(--bg-3); }
.scheme-row.selected { box-shadow: inset 2px 0 var(--accent); }
.scheme-name { display: flex; align-items: center; gap: 5px; min-width: 0; }
.scheme-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-0); }
.using-tag, .editor-badge { flex: none; padding: 1px 5px; border: 1px solid rgba(34, 211, 165, .4); border-radius: 4px; color: var(--accent); background: rgba(34, 211, 165, .08); font-size: 10px; }
.row-actions { display: flex; gap: 4px; }
.mini-btn, .mode-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 3px 6px; }
.mini-btn:hover, .mode-btn:hover { border-color: var(--accent); color: var(--accent); }
.mini-btn.danger:hover, .mini-btn.danger.armed { border-color: var(--danger); color: var(--danger); }
.keymap-editor, .keymap-preview { display: flex; flex: 1; min-height: 260px; flex-direction: column; gap: 8px; padding: 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-0); overflow: hidden; }
.editor-toolbar, .binding-toolbar, .editor-foot, .preview-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; flex-shrink: 0; }
.editor-mode { display: flex; border: 1px solid var(--border); border-radius: 4px; overflow: hidden; }
.mode-btn { border: 0; border-radius: 0; }
.mode-btn + .mode-btn { border-left: 1px solid var(--border); }
.mode-btn.active { color: var(--accent); background: rgba(34, 211, 165, .12); }
.name-field { display: flex; align-items: center; gap: 8px; color: var(--text-1); font-size: 11px; }
.name-field .input { flex: 1; min-width: 0; }
.section-title { color: var(--text-0); font-size: 12px; font-weight: 600; }
.binding-list, .preview-bindings { display: flex; flex: 1; min-height: 80px; flex-direction: column; gap: 6px; overflow: auto; }
.binding-card { display: flex; flex-direction: column; gap: 7px; padding: 7px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-1); }
.binding-head, .action-row { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.binding-index { width: 18px; color: var(--text-2); text-align: right; }
.key-input { flex: 1 1 130px; min-width: 0; }
.icon-btn { width: 25px; height: 25px; padding: 0; border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; }
.icon-btn:hover { color: var(--danger); border-color: var(--danger); }
.action-row { color: var(--text-2); font-size: 10px; }
.action-hint { color: var(--text-2); }
.action-type, .coord, .duration, .raw-field { display: inline-flex; align-items: center; gap: 5px; }
.action-type .select { min-width: 130px; padding: 4px 6px; font-size: 11px; }
.coord-label { margin-left: 3px; }
.coord .input { width: 56px; padding: 3px 5px; }
.duration .input, .raw-field .input { width: 70px; padding: 3px 5px; }
.or-label { color: var(--text-2); }
.raw-yaml { flex: 1; min-height: 180px; width: 100%; resize: vertical; box-sizing: border-box; padding: 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-1); color: var(--text-0); font: 12px/1.55 var(--mono); }
.raw-hint, .preview-sub { color: var(--text-2); font-size: 10px; }
.diagnostic-list { max-height: 84px; overflow: auto; }
.preview-card { display: flex; flex: 1; min-height: 0; flex-direction: column; gap: 8px; }
.preview-binding { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border: 1px solid var(--border); border-radius: var(--radius-sm); color: var(--text-1); font-size: 11px; }
.key-chip { min-width: 62px; padding: 2px 5px; border: 1px solid var(--border); border-radius: 4px; color: var(--accent); background: var(--bg-2); text-align: center; }
.preview-edit { align-self: flex-start; margin-top: auto; }
.mono { font-family: var(--mono); font-size: 11px; }
</style>
