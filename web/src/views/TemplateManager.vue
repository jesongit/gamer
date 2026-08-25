<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">模板管理</div>
        <div class="page-sub">图片模板用于 YAML 脚本中的 until 模板匹配</div>
      </div>
      <div class="head-actions">
        <select v-model="uploadPkg" class="select" title="上传目标应用分区">
          <option value="" disabled>目标分区…</option>
          <option v-for="p in partitions" :key="p" :value="p">{{ p }}</option>
        </select>
        <label class="btn" for="tpl-upload">⬆️ 上传模板</label>
        <input id="tpl-upload" type="file" accept="image/png,image/jpeg" hidden @change="onUpload" />
      </div>
    </div>

    <div class="tpl-layout">
      <!-- 左：模板列表 -->
      <div class="tpl-list card">
        <div class="tl-head">
          <span>模板（{{ filtered.length }}）</span>
          <select v-model="pkgFilter" class="select input-sm" title="按应用分区过滤">
            <option value="">全部分区</option>
            <option v-for="p in partitions" :key="p" :value="p">{{ p }}</option>
          </select>
          <input v-model="kw" class="input input-sm" placeholder="搜索…" />
        </div>
        <div class="tl-items">
          <div v-for="t in filtered" :key="t.pkg + '/' + t.name" class="tl-item" :class="{ sel: sel && t.pkg === sel.pkg && t.name === sel.name }" @click="select(t)">
            <div class="tl-thumb">
              <img :src="tplUrl(t)" alt="" loading="lazy" @error="onThumbErr" />
            </div>
            <div class="tl-info">
              <div class="tl-name">{{ t.name }}</div>
              <div class="tl-meta mono">{{ t.pkg }} · {{ fmtSize(t.size) }}</div>
            </div>
          </div>
        </div>
      </div>

      <!-- 右：详情 + 测试 -->
      <div class="tpl-detail card" v-if="sel">
        <div class="td-head">
          <div>
            <div class="td-name">{{ sel.name }}</div>
            <div class="td-meta mono">{{ sel.pkg }} · {{ fmtSize(sel.size) }}<template v-if="dim"> · {{ dim.w }}×{{ dim.h }} px</template></div>
          </div>
          <div class="td-actions">
            <button class="btn btn-danger btn-sm" @click="remove">删除</button>
          </div>
        </div>

        <!-- 预览 -->
        <div class="td-preview">
          <div class="td-preview-img">
            <img :src="tplUrl(sel)" alt="模板预览" @load="onPreviewLoad" @error="dim = null" />
          </div>
        </div>

        <!-- 测试匹配 -->
        <div class="td-test">
          <div class="td-label">测试匹配</div>
          <div class="td-test-row">
            <select v-model="testDev" class="select">
              <option value="">选择设备…</option>
              <option v-for="d in devices" :key="d.id" :value="d.id" :disabled="d.status !== 'online'">{{ d.name }}（{{ d.status === 'online' ? '在线' : '离线' }}）</option>
            </select>
            <button class="btn btn-primary" :disabled="!testDev" @click="testMatch">▶ 测试</button>
          </div>
        </div>

        <!-- 匹配结果 -->
        <div class="td-result" v-if="result">
          <div class="tr-status" :class="result.hit ? 'ok' : 'miss'">
            {{ result.hit ? '✓ 匹配成功' : '✗ 未找到' }}
          </div>
          <div class="tr-detail mono" v-if="result.hit">
            位置 ({{ result.x }}, {{ result.y }}) · 置信度 {{ result.score.toFixed(3) }} · 尺寸 {{ result.width }}x{{ result.height }}
          </div>
        </div>
      </div>

      <div class="tpl-detail card empty-detail" v-else>
        <div class="empty">
          <span class="icon">🖼️</span>
          <span>从左侧选择模板，或点击「上传模板」创建</span>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, computed, watch, onMounted } from 'vue'
import { devicesData, templatesData, useToast } from '../store'
import { api } from '../api'

const toast = useToast()
const templates = templatesData
const devices = devicesData
const sel = ref(null)
const kw = ref('')
const testDev = ref('')
const result = ref(null)
const dim = ref(null)
// 应用分区过滤（''=全部）/ 上传目标分区
const pkgFilter = ref('')
const uploadPkg = ref('')

const partitions = computed(() =>
  [...new Set(templates.value.map(t => t.pkg).filter(Boolean))].sort())
const filtered = computed(() => templates.value.filter(t =>
  (!pkgFilter.value || t.pkg === pkgFilter.value) && t.name.includes(kw.value)))

// 上传目标默认第一个分区；过滤选中具体分区时跟随
watch(partitions, list => { if (!uploadPkg.value && list.length) uploadPkg.value = list[0] })
watch(pkgFilter, v => { if (v) uploadPkg.value = v })

function tplUrl(t) { return api.tplImageUrl(t.name, t.pkg) }

function onThumbErr(e) { e.target.style.visibility = 'hidden' }

function onPreviewLoad(e) {
  const img = e.target
  dim.value = { w: img.naturalWidth, h: img.naturalHeight }
}

function fmtSize(n) {
  if (!n) return '—'
  return n > 1024 * 1024 ? (n / 1024 / 1024).toFixed(1) + ' MB' : (n / 1024).toFixed(1) + ' KB'
}

function select(t) { sel.value = t; result.value = null; dim.value = null }

async function onUpload(e) {
  const file = e.target.files[0]
  e.target.value = ''
  if (!file) return
  if (!uploadPkg.value) return toast('请先选择上传目标分区', 'warn')
  const name = file.name.toLowerCase().endsWith('.png') ? file.name : file.name + '.png'
  try {
    const b64 = await fileToBase64(file)
    await api.uploadTemplate(name, b64, uploadPkg.value)
    await loadTemplates()
    sel.value = templates.value.find(t => t.pkg === uploadPkg.value && t.name === name) || null
    toast(`模板已上传到 ${uploadPkg.value}`, 'success')
  } catch (err) {
    toast('上传失败：' + err.message, 'error')
  }
}

function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const fr = new FileReader()
    fr.onload = () => resolve(fr.result.split(',')[1])
    fr.onerror = reject
    fr.readAsDataURL(file)
  })
}

async function remove() {
  if (!sel.value) return
  if (!confirm(`删除模板 ${sel.value.name}（${sel.value.pkg}）？`)) return
  try {
    await api.deleteTemplate(sel.value.name, sel.value.pkg)
    const { name, pkg } = sel.value
    templates.value = templates.value.filter(t => !(t.pkg === pkg && t.name === name))
    sel.value = null
    toast('模板已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

async function testMatch() {
  if (!testDev.value || !sel.value) return
  result.value = null
  try {
    const r = await api.testTemplate(sel.value.name, testDev.value, 0.8, null, sel.value.pkg)
    result.value = r
    if (r.hit) toast(`匹配成功：置信度 ${r.score.toFixed(2)}`, 'success')
    else toast('未找到匹配', 'warn')
  } catch (e) {
    toast('匹配失败：' + e.message, 'error')
  }
}

async function loadTemplates() {
  try { templates.value = await api.listTemplates() } catch (e) {}
}
async function loadDevices() {
  try { devices.value = await api.listDevices() } catch (e) {}
}

onMounted(() => { loadTemplates(); loadDevices() })
</script>

<style scoped>
.head-actions { display: flex; gap: 10px; align-items: center; }
.head-actions .select { min-width: 180px; }

.tpl-layout { display: grid; grid-template-columns: 300px 1fr; gap: 14px; flex: 1; min-height: 0; }

.tpl-list { display: flex; flex-direction: column; gap: 10px; overflow: hidden; padding: 12px; }
.tl-head { display: flex; align-items: center; justify-content: space-between; gap: 6px; font-size: 13px; font-weight: 600; }
.tl-head .select { max-width: 100px; font-size: 11px; padding: 4px 6px; font-weight: 400; }
.input-sm { width: 110px; padding: 5px 10px; font-size: 12px; }
.tl-items { display: flex; flex-direction: column; gap: 6px; overflow: auto; flex: 1; }
.tl-item {
  display: flex; align-items: center; gap: 10px; padding: 8px 10px;
  border-radius: var(--radius-sm); cursor: pointer; border: 1px solid transparent;
  transition: all .15s;
}
.tl-item:hover { background: var(--bg-3); }
.tl-item.sel { background: rgba(34,211,165,.08); border-color: rgba(34,211,165,.35); }
.tl-thumb {
  width: 46px; height: 46px; border-radius: var(--radius-sm); flex-shrink: 0;
  background: linear-gradient(135deg, #1e2434, #141a28);
  border: 1px solid var(--border); display: flex; align-items: center; justify-content: center;
  font-size: 18px; color: var(--text-2);
  position: relative; overflow: hidden;
}
.tl-thumb::before {
  content: '▦'; position: absolute; font-size: 18px; color: var(--text-2); z-index: 0;
}
.tl-thumb img {
  position: relative; z-index: 1; max-width: 100%; max-height: 100%;
  object-fit: contain; image-rendering: auto;
}
.tl-thumb img[style*="hidden"] { display: none; }
.tl-info { min-width: 0; }
.tl-name { font-size: 13px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.tl-meta { font-size: 11px; color: var(--text-2); margin-top: 3px; }

.tpl-detail { display: flex; flex-direction: column; gap: 18px; overflow: auto; }
.empty-detail { align-items: center; justify-content: center; }
.td-head { display: flex; justify-content: space-between; align-items: flex-start; }
.td-name { font-size: 17px; font-weight: 700; }
.td-meta { font-size: 11px; color: var(--text-2); margin-top: 4px; }
.td-actions { display: flex; gap: 8px; }

.td-preview {
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  background: linear-gradient(135deg, #1e2434, #141a28);
  display: flex; align-items: center; justify-content: center;
  min-height: 160px; padding: 16px; overflow: auto;
}
.td-preview-img { position: relative; display: inline-flex; }
.td-preview-img img { display: block; max-width: 100%; max-height: 320px; object-fit: contain; }

.td-label { font-size: 13px; font-weight: 600; margin-bottom: 8px; }
.td-label .mono { color: var(--accent); font-size: 13px; }
.td-hint { font-size: 11px; color: var(--text-2); margin-top: 6px; }
.td-test-row { display: flex; gap: 10px; }
.td-test-row .select { flex: 1; }

.td-result { border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 14px; }
.tr-status { font-size: 15px; font-weight: 700; }
.tr-status.ok { color: var(--ok); }
.tr-status.miss { color: var(--warn); }
.tr-detail { font-size: 12px; color: var(--text-1); margin-top: 6px; }
</style>
