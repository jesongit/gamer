<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">脚本编辑</div>
        <div class="page-sub">可视化步骤画布：卡片编辑 · 撤销重做 · 字段级校验 · 保存版本冲突检测；脚本与函数库按应用分区存放（只读 YAML 在「诊断」中预览）</div>
      </div>
      <div class="head-actions">
        <select v-model="pkg" class="select mono ed-pkg" title="应用分区（脚本/函数库/模板都存放在 data/<应用包名>/ 下）">
          <option v-if="!pkgOptions.length" value="">（无分区）</option>
          <option v-for="p in pkgOptions" :key="p" :value="p">{{ p }}</option>
        </select>
        <button v-if="!store.running" class="btn btn-primary" :disabled="!canRun" @click="run">▶ 运行</button>
        <button v-else class="btn btn-danger" :disabled="runStopping" @click="stop">{{ runStopping ? '■ 停止中…' : '■ 停止' }}</button>
        <button class="btn" :disabled="!shell.hasModel || shell.saving" @click="save">{{ shell.saving ? '保存中…' : '💾 保存' }}</button>
        <span v-if="shell.hasModel && shell.dirty" class="tag dirty-tag">未保存</span>
      </div>
    </div>

    <div class="shell-layout">
      <!-- 左：资源树（脚本 / 函数库 / 模板 三页签，plan §10.2） -->
      <aside class="res-panel card">
        <div class="res-tabs">
          <button type="button" class="res-tab" :class="{ active: tab === 'script' }" @click="tab = 'script'">脚本</button>
          <button type="button" class="res-tab" :class="{ active: tab === 'func' }" @click="tab = 'func'">函数库</button>
          <button type="button" class="res-tab" :class="{ active: tab === 'tmpl' }" @click="tab = 'tmpl'">模板</button>
        </div>
        <div class="res-actions">
          <button v-if="tab === 'script'" class="btn btn-sm" @click="newScript">＋ 新建脚本</button>
          <button v-if="tab === 'func'" class="btn btn-sm" @click="newFunctionFile">＋ 新建函数库</button>
          <button v-if="tab === 'tmpl'" class="btn btn-sm" @click="goConsole">投屏控制台管理 →</button>
        </div>
        <div class="res-items">
          <template v-if="tab === 'script'">
            <div v-for="s in pkgScripts" :key="s.id" class="res-item" :class="{ sel: s.id === selScriptId }" @click="openScript(s)">
              <div class="ri-name">{{ s.name }}</div>
              <div class="ri-meta mono">{{ fmtTime(s.updated_at) }}</div>
              <button class="ri-del" @click.stop="removeScript(s)" title="删除">🗑</button>
            </div>
            <div v-if="!pkgScripts.length" class="res-empty">该分区暂无脚本</div>
          </template>
          <template v-else-if="tab === 'func'">
            <div v-for="f in fnLib.list" :key="f.id" class="res-item" :class="{ sel: f.id === selFnId }" @click="openFunctionFile(f)">
              <div class="ri-name">{{ f.file }}</div>
              <div class="ri-meta mono">{{ (f.functions || []).join('、') || '（无函数）' }}</div>
              <button class="ri-del" @click.stop="removeFunctionFile(f)" title="删除">🗑</button>
            </div>
            <div v-if="!fnLib.list.length" class="res-empty">该分区暂无函数库文件</div>
          </template>
          <template v-else>
            <div v-for="t in templates" :key="t.name" class="res-item readonly">
              <div class="ri-name">{{ t.name }}</div>
              <div class="ri-meta mono">模板图片 · 框选/上传/匹配测试在投屏控制台</div>
            </div>
            <div v-if="!templates.length" class="res-empty">该分区暂无模板</div>
          </template>
        </div>
        <div class="res-foot mono">运行设备：{{ store.deviceId || '未选择（投屏控制台选择）' }}</div>
      </aside>

      <!-- 中：共享编辑画布 -->
      <section class="editor-main card">
        <div v-if="tab === 'tmpl'" class="ed-empty">
          <p>模板的框选截取、上传、二次裁切与匹配测试在投屏控制台完成（依赖设备画面）。</p>
          <button class="btn btn-primary" @click="goConsole">前往投屏控制台</button>
        </div>
        <template v-else-if="shell.hasModel">
          <div class="ed-toolbar">
            <input v-model="shell.name" class="input mono ed-name" :placeholder="tab === 'func' ? '函数库文件短名（缺省 .yaml 自动补）' : '脚本名称（可省略 .yml 后缀）'" />
            <button class="btn btn-sm" :disabled="!shell.canUndo" title="撤销" @click="shell.undo()">↶</button>
            <button class="btn btn-sm" :disabled="!shell.canRedo" title="重做" @click="shell.redo()">↷</button>
            <button class="btn btn-sm" :class="{ active: showExtras }" @click="showExtras = !showExtras">参数/配置</button>
            <button class="btn btn-sm" :class="{ active: showYaml }" title="只读生成 YAML（诊断预览，不可编辑）" @click="showYaml = !showYaml">诊断</button>
            <button v-if="shell.canJumpBack" class="btn btn-sm" @click="shell.jumpBack()">← 返回 {{ shell.jumpBackLabel }}</button>
          </div>
          <div class="ed-body">
            <StepCanvas
              ref="canvasEl"
              :model="shell.model"
              :stack="shell.stack"
              :diagnostics="shell.diagnostics"
              :context="shell.editorContext"
              :templates="templateNames"
              :selected-uuid="shell.selectedUuid"
              :test-from="tab === 'func'"
              @select="(u) => shell.select(u)"
              @test-from="onTestFrom"
            />
            <div v-if="showExtras" class="extras">
              <!-- 脚本 = 文件级 params；函数库 = 当前函数 params（functionPath 指到 functions.<名>.params） -->
              <ParamEditor :model="shell.model" :stack="shell.stack" :diagnostics="shell.diagnostics" :function-path="fnParamsPath" />
              <ConfigEditor v-if="tab === 'script'" :model="shell.model" :stack="shell.stack" />
            </div>
          </div>
        </template>
        <div v-else class="ed-empty">从左侧选择{{ tab === 'func' ? '函数库文件' : '脚本' }}，或新建一个。</div>
      </section>

      <!-- 右：校验错误列表 + 函数测试占位（阶段 5） -->
      <aside class="side-panel card">
        <ErrorSummary :diagnostics="shell.diagnostics" @locate="locateDiag" />
        <div v-if="tab === 'func' && shell.hasModel" class="test-fn">
          <div class="tf-title">测试函数</div>
          <select v-model="testFnName" class="select tf-fn" aria-label="选择要测试的函数">
            <option value="">（画布当前函数）</option>
            <option v-for="f in shell.model.functions" :key="f.name" :value="f.name">{{ f.name }}</option>
          </select>
          <button
            class="btn btn-primary" :disabled="!store.deviceId || testFlow.modal.submitting"
            title="按函数 params 生成参数表单，经函数测试接口运行单个函数"
            @click="beginTestFn()"
          >▶ 测试函数</button>
          <p class="tf-desc">按函数 params 生成参数表单（有默认值可省略）；函数体顶层卡片可用「▶测试」从此步骤开始。</p>
        </div>
      </aside>
    </div>

    <YamlPreview
      v-if="showYaml && shell.hasModel"
      :model="shell.model"
      :filename="shell.name || (tab === 'func' ? 'functions.yaml' : 'script.yaml')"
      @close="showYaml = false"
    />
    <!-- 保存 409 冲突：与 Console 紧凑外壳共用同一弹窗与 shell 冲突状态 -->
    <SaveConflictModal
      :open="!!shell.conflict"
      :resource="shell.conflict?.resource || ''"
      :message="shell.conflict?.message || ''"
      @reload="onConflictReload"
      @overwrite="onConflictOverwrite"
      @close="shell.dismissConflict()"
    />
    <!-- 运行参数弹窗（阶段 5）：脚本运行 / 函数测试共用 ParamsForm（稀疏 args、400 诊断回填标红） -->
    <RunParamsModal
      :open="runFlow.modal.open"
      :title="runFlow.modal.title"
      :desc="runFlow.modal.desc"
      submit-label="▶ 运行"
      :params="runFlow.modal.params"
      :initial-args="runFlow.modal.initialArgs"
      :suggestions="runFlow.modal.suggestions"
      :templates="runFlow.modal.templates"
      :field-errors="runFlow.modal.fieldErrors"
      :general-errors="runFlow.modal.generalErrors"
      :submitting="runFlow.modal.submitting"
      @submit="onRunArgsSubmit"
      @close="runFlow.close()"
    />
    <RunParamsModal
      :open="testFlow.modal.open"
      :title="testFlow.modal.title"
      :desc="testFlow.modal.desc"
      :submit-label="testFlow.modal.submitLabel"
      :params="testFlow.modal.params"
      :initial-args="testFlow.modal.initialArgs"
      :suggestions="testFlow.modal.suggestions"
      :templates="testFlow.modal.templates"
      :field-errors="testFlow.modal.fieldErrors"
      :general-errors="testFlow.modal.generalErrors"
      :submitting="testFlow.modal.submitting"
      @submit="onTestArgsSubmit"
      @close="testFlow.close()"
    />
    <RunConflictModal />
  </div>
</template>

<script setup>
/**
 * 独立脚本页 = 全屏可视化外壳（阶段 4，plan §10.2）：
 * - 左侧资源树三页签：脚本 / 函数库 / 模板（模板只读列表 + 跳转投屏控制台占位）；
 * - 中央共享画布（StepCanvas，与 Console 紧凑外壳同一编辑核心 useScriptEditorShell）；
 * - 右侧常驻错误列表（ErrorSummary，点击经画布 locate 定位展开）；
 * - 函数库：文件 → FunctionLibraryModel，函数级 params 经 ParamEditor functionPath 编辑；
 * - 保存带 expected_version，409 version_conflict → SaveConflictModal（重载/覆盖）；
 * - 「保存并运行」：存在未保存内容先落盘，再以持久化版本启动（不运行浏览器内临时模型）；
 * - 运行/停止/实例恢复沿用统一 RunManager 状态（store + runs.js），与旧页一致；
 * - 「测试函数」（阶段 5）：选函数 → ParamsForm → POST /api/functions/:id/run
 *   （function/start_index/args）；函数体顶层卡片「▶测试」映射 start_index 从该步测试；
 *   400 invalid_args 诊断回填表单字段标红，覆盖建议按函数库文件 id 存 localStorage。
 */
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import {
  scriptsData, devicesData, store, useToast,
  applyRunRecord, beginCancel, findRun, resetStoreRunState, pushRunConflict,
} from '../store'
import { api } from '../api'
import { serialize } from '../script-editor/codec'
import { startIndexOf } from '../script-editor/selection'
import {
  normalizeActiveRunResponse, normalizeStartReply,
  isMissingEndpointError, isDeviceBusyConflict, isTerminalRunState, terminalLabel,
  describeConflict,
} from '../runs'
import RunConflictModal from '../components/RunConflictModal.vue'
import RunParamsModal from '../components/RunParamsModal.vue'
import SaveConflictModal from '../components/console/SaveConflictModal.vue'
import { useScriptEditorShell } from '../composables/useScriptEditorShell'
import { useFunctionLibrary } from '../composables/useFunctionLibrary'
import { useRunArgsFlow } from '../composables/useRunArgsFlow'
import { StepCanvas, ParamEditor, ConfigEditor, YamlPreview, ErrorSummary } from '../script-editor/components/index'

const router = useRouter()
const toast = useToast()
const scripts = scriptsData
const devices = devicesData

// ---------- 外壳与资源 ----------
const shell = useScriptEditorShell({
  api,
  getContext: () => ({
    resolveTemplate: (n) => templates.value.some(t => t.name === n || shortName(t.name) === n),
  }),
})
const fnLib = useFunctionLibrary({ api })

const pkg = ref('')
const tab = ref('script') // 'script' | 'func' | 'tmpl'
const templates = ref([]) // 当前分区模板（只读列表）
const selScriptId = ref(null)
const selFnId = ref(null)
const showExtras = ref(false)
const showYaml = ref(false)
const canvasEl = ref(null)

/** 模板短名（login.png ← login#l.png）：画布 tmpl 控件与存在性校验共用口径。 */
function shortName(name) {
  return String(name || '').replace(/#.*?(\.(png|jpe?g|bmp|webp))?$/i, '$1')
}
const templateNames = computed(() => templates.value.map(t => shortName(t.name)))

const pkgScripts = computed(() => scripts.value.filter(s => !pkg.value || s.package === pkg.value))

/** 分区下拉选项：当前设备配置的应用包名 ∪ 已有脚本分区 */
const pkgOptions = computed(() => {
  const set = new Set()
  const dp = (devices.value.find(d => d.id === store.deviceId)?.pkg || '').trim()
  if (dp) set.add(dp)
  for (const s of scripts.value) if (s.package) set.add(s.package)
  return [...set].sort()
})

/** 函数级 params 容器：画布当前编辑函数名（expose 代理透出，随函数下拉联动）→ ['functions', 名, 'params'] */
const fnParamsPath = computed(() => {
  if (tab.value !== 'func' || !canvasEl.value) return null
  const fnName = canvasEl.value.activeFnName
  return fnName ? ['functions', fnName, 'params'] : null
})

function fmtTime(s) {
  return (s || '').slice(0, 16)
}

// ---------- 资源加载 ----------

async function loadScripts() {
  try { scripts.value = await api.listScripts() } catch (e) { /* 保持旧列表 */ }
}
async function loadDevices() {
  try { devices.value = await api.listDevices() } catch (e) {}
}
async function loadTemplates() {
  if (!pkg.value) { templates.value = []; return }
  try { templates.value = (await api.listTemplates(pkg.value)) || [] } catch (e) { templates.value = [] }
}

/** 切分区：函数库/模板随分区刷新 */
function applyPkg() {
  fnLib.refresh(pkg.value)
  loadTemplates()
}

watch(pkg, () => { applyPkg() })

// ---------- 打开 / 新建 / 删除 ----------

async function confirmDiscardDirty() {
  if (shell.hasModel && shell.dirty && !window.confirm('当前资源有未保存修改，确认放弃？')) return false
  return true
}

async function openScript(s) {
  if (shell.kind === 'script' && shell.resourceId === s.id) return
  if (!(await confirmDiscardDirty())) return
  tab.value = 'script'
  showExtras.value = false
  showYaml.value = false
  try {
    await shell.loadScript(s.id)
    selScriptId.value = s.id
  } catch (e) {
    shell.reset()
    toast('脚本加载失败：' + e.message, 'error')
  }
}

async function openFunctionFile(f) {
  if (shell.kind === 'function_library' && shell.resourceId === f.id) return
  if (!(await confirmDiscardDirty())) return
  tab.value = 'func'
  showExtras.value = false
  showYaml.value = false
  try {
    await shell.loadFunctionFile(f.id)
    selFnId.value = f.id
  } catch (e) {
    shell.reset()
    toast('函数库加载失败：' + e.message, 'error')
  }
}

function newScript() {
  if (!pkg.value) return toast('请先选择应用分区', 'warn')
  if (shell.hasModel && shell.dirty && !window.confirm('当前资源有未保存修改，确认放弃？')) return
  tab.value = 'script'
  showExtras.value = false
  showYaml.value = false
  shell.newScript({ name: '新脚本.yml', pkg: pkg.value })
  selScriptId.value = null
}

function newFunctionFile() {
  if (!pkg.value) return toast('请先选择应用分区', 'warn')
  if (shell.hasModel && shell.dirty && !window.confirm('当前资源有未保存修改，确认放弃？')) return
  const raw = window.prompt('函数库文件短名（缺省 .yaml 自动补）', 'functions')
  if (!raw || !raw.trim()) return
  tab.value = 'func'
  showExtras.value = false
  showYaml.value = false
  shell.newFunctionFile({ file: raw.trim(), pkg: pkg.value })
  selFnId.value = null
}

async function removeScript(s) {
  if (!s.id) return
  if (!window.confirm(`删除脚本 ${s.name}？`)) return
  try {
    await api.deleteScript(s.id)
    await loadScripts()
    if (shell.kind === 'script' && shell.resourceId === s.id) shell.reset()
    if (selScriptId.value === s.id) selScriptId.value = null
    toast('脚本已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

async function removeFunctionFile(f) {
  if (!window.confirm(`删除函数库文件 ${f.file}？（其中函数将被其他脚本的 func 引用失效）`)) return
  try {
    await api.deleteFunction(f.id)
    fnLib.refresh(pkg.value)
    if (shell.kind === 'function_library' && shell.resourceId === f.id) shell.reset()
    if (selFnId.value === f.id) selFnId.value = null
    toast('函数库文件已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

function goConsole() {
  router.push({ name: 'Console' })
}

// ---------- 保存 / 冲突 ----------

/** 保存当前模型（脚本或函数库）；冲突时弹 SaveConflictModal，校验失败提示前几条诊断 */
async function save() {
  if (!shell.hasModel) return { ok: false }
  if (!String(shell.name || '').trim()) { toast('请填写资源名称', 'error'); return { ok: false } }
  if (!pkg.value && !shell.pkg) { toast('请先选择应用分区', 'warn'); return { ok: false } }
  const r = await shell.save()
  if (r.ok) {
    await loadScripts()
    fnLib.refresh(pkg.value)
    if (shell.kind === 'script') selScriptId.value = shell.resourceId
    else selFnId.value = shell.resourceId
    toast('已保存', 'success')
  } else if (r.reason === 'invalid') {
    toast('校验未通过：' + r.diagnostics.slice(0, 3).map(d => d.message).join('；'), 'error')
  } else if (r.reason === 'conflict') {
    // shell.conflict 已置位 → SaveConflictModal
  } else {
    toast('保存失败：' + (r.error?.message || r.error), 'error')
  }
  return r
}

async function onConflictReload() {
  try {
    const r = await shell.reload()
    if (r.ok) toast('已恢复磁盘版本', 'success')
  } catch (e) {
    toast('重载失败：' + e.message, 'error')
  }
}

async function onConflictOverwrite() {
  const r = await shell.overwrite()
  if (r.ok) {
    await loadScripts()
    fnLib.refresh(pkg.value)
    toast('已强制覆盖', 'success')
  } else if (r.reason === 'error') {
    toast('覆盖失败：' + (r.error?.message || r.error), 'error')
  }
}

// ---------- 运行（统一 RunManager 状态，沿用旧页状态机） ----------

const runStopping = computed(() => {
  const rec = store.runId ? findRun(store.runId) : null
  return rec?.state === 'stopping'
})

/** 运行目标：模板页签无运行对象；函数库不能独立运行（「测试函数」走函数测试接口） */
const canRun = computed(() => {
  if (tab.value !== 'script') return false
  return !!pkg.value && (shell.kind === 'script' ? shell.hasModel : !!selScriptId.value)
})

// ---- 运行参数流程（阶段 5，plan §12.1）：脚本声明 params 时先弹表单，稀疏 args 提交 ----
const runFlow = useRunArgsFlow({
  exec: async ({ id, name, startIndex, args }) => {
    const rep = await api.runScript(id, store.deviceId, startIndex, args)
    const started = normalizeStartReply(rep)
    if (started) {
      applyRunRecord({
        run_id: started.run_id,
        state: started.state,
        device_id: store.deviceId,
        script_id: id,
        source: 'manual',
        display: name,
      })
    } else {
      store.running = true
      store.runScript = name
      store.runScriptId = id
    }
    return rep
  },
  notify: ({ summary }) => {
    toast('脚本已开始运行', 'success')
    if (summary) toast(summary, 'info') // resolved_args 摘要（默认继承/显式覆盖标注）
    startRunStatusPoll()
  },
})

function handleRunStartError(e) {
  if (isDeviceBusyConflict(e)) {
    const info = { ...(e.data || {}), device_id: store.deviceId }
    pushRunConflict(info)
    toast(describeConflict(info), 'warn')
  } else {
    toast('运行失败：' + e.message, 'error')
  }
}

function onRunArgsSubmit({ args }) {
  runFlow.confirm(args).catch(handleRunStartError)
}

// ---- 函数测试（阶段 5，plan §12.2）：RunManager 统一 run_id，RunRecord.script_id 仅展示标签 ----

const testFnName = ref('') // 右栏函数选择；'' = 跟随画布当前函数

const testFlow = useRunArgsFlow({
  exec: async ({ id, name, fnName, startIndex, args }) => {
    const rep = await api.runFunction(id, store.deviceId, {
      function: fnName,
      start_index: startIndex || 0,
      args,
    })
    const started = normalizeStartReply(rep)
    const display = `${name || id} · ${fnName}()`
    if (started) {
      applyRunRecord({
        run_id: started.run_id,
        state: started.state,
        device_id: store.deviceId,
        script_id: id,
        source: 'manual',
        display,
      })
    } else {
      store.running = true
      store.runScript = display
      store.runScriptId = id
    }
    return rep
  },
  notify: ({ summary }) => {
    toast('函数测试已开始', 'success')
    if (summary) toast(summary, 'info') // resolved_args 摘要
    startRunStatusPoll()
  },
})

/** 待测函数名解析：右栏下拉 > 画布当前函数 > 第一个函数。 */
function resolveTestFnName() {
  const fns = shell.hasModel && Array.isArray(shell.model.functions) ? shell.model.functions : []
  const name = testFnName.value || canvasEl.value?.activeFnName || fns[0]?.name || ''
  return fns.some((f) => f.name === name) ? name : ''
}

/** 「▶ 测试函数」：从头（start_index 0）运行所选函数；未保存内容先落盘（服务端读磁盘）。 */
async function beginTestFn() {
  if (!store.deviceId) return toast('请先选择设备（投屏控制台 → 设备工具条）', 'error')
  const fnName = resolveTestFnName()
  if (!fnName) return toast('该函数库文件没有可测试的函数', 'warn')
  await startFunctionTest(fnName, 0)
}

async function startFunctionTest(fnName, startIndex) {
  if (shell.dirty || !shell.resourceId) {
    const r = await save()
    if (!r || !r.ok) return // 保存失败（校验/409 冲突）不发起测试
  }
  if (!shell.resourceId) return toast('请先保存函数库文件', 'error')
  try {
    await testFlow.begin({
      id: shell.resourceId,
      name: shell.name || shell.resourceId,
      kind: 'function_library',
      fnName,
      startIndex,
      yaml: serialize(shell.model),
      templates: templateNames.value,
      title: '测试函数',
      submitLabel: '▶ 测试',
      desc: `测试 ${shell.name || shell.resourceId} · ${fnName}()${startIndex ? `（从第 ${startIndex + 1} 步）` : ''}`,
    })
  } catch (e) {
    handleTestStartError(e)
  }
}

function handleTestStartError(e) {
  if (isDeviceBusyConflict(e)) {
    const info = { ...(e.data || {}), device_id: store.deviceId }
    pushRunConflict(info)
    toast(describeConflict(info), 'warn')
  } else {
    toast('测试失败：' + e.message, 'error')
  }
}

function onTestArgsSubmit({ args }) {
  testFlow.confirm(args).catch(handleTestStartError)
}

/** 画布函数体顶层卡片「▶测试」：uuid → start_index 从该步测试（嵌套步骤不支持）。 */
function onTestFrom(uuid) {
  if (!shell.hasModel) return
  const idx = startIndexOf(shell.model, uuid)
  if (idx === null) return toast('仅函数体顶层步骤支持「从此步骤测试」', 'warn')
  const fnName = resolveTestFnName()
  if (!fnName) return toast('该函数库文件没有可测试的函数', 'warn')
  startFunctionTest(fnName, idx)
}

async function run() {
  if (!store.deviceId) return toast('请先选择设备（投屏控制台 → 设备工具条）', 'error')
  if (shell.kind === 'function_library') return toast('函数库不能独立运行（在脚本 func 步骤中调用；可用「测试函数」运行单个函数）', 'warn')
  // 未打开任何脚本时先打开树中选中项
  if (!shell.hasModel) {
    const s = scripts.value.find(x => x.id === selScriptId.value)
    if (!s) return toast('请先选择脚本', 'error')
    await openScript(s)
  }
  if (shell.kind !== 'script') return
  if (shell.dirty || !shell.resourceId) {
    const r = await save()
    if (!r || !r.ok) return // 保存失败（含 409 冲突弹窗挂起）不启动运行
  }
  if (!shell.resourceId) return toast('请先保存脚本', 'error')
  const id = shell.resourceId
  const name = shell.name || id
  try {
    await runFlow.begin({
      id,
      name,
      yaml: serialize(shell.model),
      templates: templateNames.value,
      desc: `运行脚本 ${name}`,
    })
  } catch (e) {
    handleRunStartError(e)
  }
}

function stop() {
  if (store.runId) {
    const rid = store.runId
    beginCancel(rid)
    api.cancelRun(rid).catch(e => {
      if (isMissingEndpointError(e)) {
        const sid = findRun(rid)?.script_id || store.runScriptId
        if (sid) api.stopScript(sid).catch(() => {})
      }
    })
    // 保留 stopping 状态和轮询，直到服务端返回终态；避免停止请求尚未生效时
    // 立即恢复运行按钮造成同设备并行启动。
    toast('已发送停止指令，等待脚本退出…', 'warn')
    return
  }
  // 兼容旧后端会话：没有 run_id 时才使用 script_id 停止并立即恢复旧 UI。
  if (!store.runScriptId) return
  api.stopScript(store.runScriptId).catch(() => {})
  resetStoreRunState()
  stopRunStatusPoll()
  toast('已发送停止指令，脚本将在当前步骤结束后停止', 'warn')
}

// 运行状态轮询：以当前 runId 单次查询，按 record.state 驱动状态机；旧后端降级 script status
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
  if (!store.running) { stopRunStatusPoll(); return }
  if (store.runId) {
    const rid = store.runId
    let rec = null
    try {
      rec = await api.getRun(rid)
    } catch (e) {
      if (!isMissingEndpointError(e)) return
      const sid = findRun(rid)?.script_id || store.runScriptId
      if (!sid) { stopRunStatusPoll(); resetStoreRunState(); return }
      try {
        const st = await api.scriptStatus(sid)
        rec = { run_id: rid, device_id: store.deviceId, script_id: sid, state: st.running ? 'running' : 'cancelled', degraded: true }
      } catch (e2) { return }
    }
    if (!rec || !rec.run_id) return
    const merged = applyRunRecord(rec)
    if (merged && isTerminalRunState(merged.state)) {
      stopRunStatusPoll()
      const detail = merged.degraded ? '' : `：${terminalLabel(merged.state)}${merged.error ? `（${merged.error}）` : ''}`
      toast(`脚本已结束${detail}`, merged.degraded || merged.state === 'success' ? 'info' : 'warn')
    }
    return
  }
  // 兼容旧后端或旧页面会话：此分支没有执行实例 ID，只能使用旧 script_id 接口。
  if (!store.runScriptId) { stopRunStatusPoll(); return }
  try {
    const st = await api.scriptStatus(store.runScriptId)
    if (!st.running) {
      resetStoreRunState()
      stopRunStatusPoll()
      toast('脚本已结束', 'info')
    }
  } catch (e) {}
}

/** 页面刷新后按设备恢复活动运行实例；旧后端响应保留 script_id 降级路径。 */
async function restoreRunState() {
  if (!store.deviceId || store.running) return
  let rep
  try {
    rep = await api.deviceRun(store.deviceId)
  } catch (e) {
    return
  }
  const rec = normalizeActiveRunResponse(rep)
  if (!rec) return

  const script = scripts.value.find(s => s.id === rec.script_id)
  const display = rec.script_name || script?.name || rec.script_id
  if (rec.run_id) {
    applyRunRecord({ ...rec, device_id: store.deviceId, display })
  } else {
    store.running = true
    store.runScript = display
    store.runScriptId = rec.script_id
  }
  if (script) selScriptId.value = script.id
  startRunStatusPoll()
  toast(`检测到 ${display} 正在运行，已恢复状态`, 'info')
}

// ---------- 诊断定位 ----------

/** 右侧错误列表点击 → 画布定位（展开祖先链 + 选中 + 瞬态高亮），面板独立挂载由宿主转发 */
function locateDiag(d) {
  canvasEl.value?.locate(d)
}

// ---------- 生命周期 ----------

onMounted(async () => {
  await Promise.all([loadScripts(), loadDevices()])
  // 直接刷新在脚本页时 store 尚未经过 Console 初始化：复用 Console 保存的设备选择
  if (!store.deviceId) {
    const saved = localStorage.getItem('gb_device_id')
    if (saved && devices.value.some(d => d.id === saved)) store.deviceId = saved
  }
  // 初始分区：设备配置的应用包名优先，否则第一个脚本分区
  if (!pkg.value) {
    const dp = (devices.value.find(d => d.id === store.deviceId)?.pkg || '').trim()
    pkg.value = dp || pkgOptions.value[0] || ''
  }
  applyPkg()
  // 刷新后 store 是空内存态：按当前设备恢复服务端活动 run；SPA 切页时则复用已有 run。
  if (!store.running) await restoreRunState()
  // 其他页面已启动脚本时，本页接管状态轮询（脚本结束后复位运行状态）。
  if (store.running && (store.runId || store.runScriptId)) startRunStatusPoll()
})
onUnmounted(() => stopRunStatusPoll())
</script>

<style scoped>
.head-actions { display: flex; gap: 10px; align-items: center; }
.ed-pkg { max-width: 240px; font-size: 12px; }
.dirty-tag { color: var(--warn); border-color: var(--warn); }

.shell-layout {
  display: grid; grid-template-columns: 250px 1fr 250px;
  gap: 12px; flex: 1; min-height: 0;
}

/* 左：资源树 */
.res-panel { display: flex; flex-direction: column; gap: 8px; padding: 10px; overflow: hidden; }
.res-tabs { display: flex; gap: 4px; }
.res-tab {
  flex: 1; padding: 5px 0; font-size: 12px; text-align: center; cursor: pointer;
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: var(--radius-sm);
}
.res-tab.active { color: var(--accent); border-color: rgba(34, 211, 165, .45); background: rgba(34, 211, 165, .08); }
.res-actions { display: flex; gap: 6px; }
.res-actions .btn { flex: 1; justify-content: center; }
.res-items { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 5px; min-height: 0; }
.res-item {
  position: relative; padding: 7px 10px; border-radius: var(--radius-sm); cursor: pointer;
  border: 1px solid transparent; transition: all .15s;
}
.res-item:hover { background: var(--bg-3); }
.res-item.sel { background: rgba(34, 211, 165, .08); border-color: rgba(34, 211, 165, .35); }
.res-item.readonly { cursor: default; }
.ri-name { font-size: 12px; font-weight: 600; padding-right: 26px; word-break: break-all; }
.ri-meta { font-size: 11px; color: var(--text-2); margin-top: 3px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ri-del {
  position: absolute; right: 6px; top: 8px; background: none; border: none;
  color: var(--text-2); cursor: pointer; font-size: 12px;
}
.ri-del:hover { color: var(--danger); }
.res-empty { padding: 14px 6px; text-align: center; font-size: 12px; color: var(--text-2); }
.res-foot { flex-shrink: 0; font-size: 11px; color: var(--text-2); border-top: 1px solid var(--border); padding-top: 8px; }

/* 中：画布 */
.editor-main { display: flex; flex-direction: column; gap: 8px; padding: 10px; overflow: hidden; }
.ed-toolbar { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
.ed-toolbar .btn.active { border-color: var(--accent-2); color: var(--accent-2); background: rgba(56, 189, 248, .08); }
.ed-name { flex: 1; min-width: 140px; max-width: 320px; }
.ed-body { flex: 1; min-height: 0; overflow: auto; display: flex; flex-direction: column; gap: 8px; }
.ed-body :deep(.se-canvas) { flex: none; }
.extras { display: flex; flex-direction: column; }
.ed-empty {
  flex: 1; display: flex; flex-direction: column; gap: 10px; align-items: center; justify-content: center;
  color: var(--text-2); font-size: 13px; text-align: center; padding: 20px;
}
.ed-empty p { margin: 0; }

/* 右：错误列表 + 函数测试 */
.side-panel { display: flex; flex-direction: column; gap: 10px; padding: 10px; overflow: auto; min-height: 0; }
.side-panel :deep(.error-summary) { margin-top: 0; }
.test-fn { border: 1px dashed var(--border); border-radius: var(--radius-sm); padding: 10px; display: flex; flex-direction: column; gap: 6px; }
.tf-title { font-size: 12px; font-weight: 600; color: var(--text-1); }
.tf-fn { width: 100%; font-size: 12px; }
.tf-desc { margin: 0; font-size: 11px; color: var(--text-2); line-height: 1.6; }

@media (max-width: 1100px) {
  .shell-layout { grid-template-columns: 200px 1fr 200px; }
}
</style>
