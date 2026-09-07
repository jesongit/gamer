<template>
  <div v-if="open" class="studio-mask" @click.self="close">
    <section class="studio" role="dialog" aria-modal="true" data-testid="template-studio">
      <header class="studio-head">
        <span class="studio-title">✂️ 帧上做模板 / 离线测试</span>
        <span class="mono studio-frame" data-testid="studio-frame-label">{{ frameLabel }}</span>
        <button class="mini-btn" type="button" aria-label="关闭" data-testid="studio-close" @click="close">✕</button>
      </header>

      <div v-if="!yamlReady" class="studio-dep" role="alert" data-testid="studio-dep-banner">
        需要「自动化」插件（gamer.yaml）处于运行状态：模板创建与离线测试经其公开动作保存/匹配。
        请在「插件」中心安装并启动 gamer.yaml。视频导入/录制/播放/标记不受影响。
      </div>
      <div v-if="error" class="studio-error" role="alert" data-testid="studio-error">{{ error }}</div>

      <div class="studio-body">
        <!-- 左：确定帧 + 框选 + 命中叠加 -->
        <div class="stage-box" ref="stageBox" data-testid="studio-stage">
          <img
            v-if="frameUrl"
            ref="frameImg"
            :src="frameUrl"
            class="frame-img"
            :class="{ picking }"
            alt="服务端确定帧"
            data-testid="studio-frame-image"
            draggable="false"
            @load="onFrameLoad"
            @error="onFrameError"
            @mousedown="onMouseDown"
            @mousemove="onMouseMove"
            @mouseup="onMouseUp"
            @mouseleave="onMouseUp"
          />
          <div v-if="frameError" class="frame-error" data-testid="studio-frame-error">{{ frameError }}</div>
          <div
            v-if="selectStyle"
            class="overlay-select"
            :style="selectStyle"
            data-testid="studio-selection"
          ></div>
          <div
            v-if="hitStyle"
            class="overlay-hit"
            :style="hitStyle"
            data-testid="studio-hit"
          ></div>
          <div
            v-if="missStyle"
            class="overlay-miss"
            :style="missStyle"
            data-testid="studio-miss"
          ></div>
          <div v-if="resultText" class="result-line mono" data-testid="studio-result">{{ resultText }}</div>
        </div>

        <!-- 右：创建 + 测试 -->
        <div class="studio-side">
          <div class="side-block">
            <div class="side-title">从选框创建模板</div>
            <div class="side-hint mono" data-testid="studio-sel-info">{{ selectionInfo }}</div>
            <input v-model.trim="templateName" class="input" placeholder="模板名（默认自动生成，可中文）" data-testid="studio-template-name" />
            <label class="check-row"><input v-model="preserveColor" type="checkbox" /> 保留颜色（文件名自动加 #1）</label>
            <label v-if="nameConflict" class="check-row"><input v-model="overwrite" type="checkbox" data-testid="studio-overwrite" /> 覆盖已有模板 {{ nameConflict }}</label>
            <button
              class="btn btn-sm btn-primary"
              type="button"
              :disabled="!yamlReady || !selection || saving || frameError"
              data-testid="studio-save"
              @click="saveTemplate"
            >{{ saving ? '保存中…' : '💾 保存模板到当前 Package' }}</button>
          </div>

          <div class="side-block">
            <div class="side-title">离线测试匹配</div>
            <div class="side-hint">在当前确定帧上按模板规则匹配（不触设备）；结果记录帧身份与坐标空间。</div>
            <input v-model.trim="testName" class="input mono" list="studio-template-options" placeholder="模板短名（留空 = 刚保存的模板）" data-testid="studio-test-name" />
            <datalist id="studio-template-options">
              <option v-for="name in templateOptions" :key="name" :value="name" />
            </datalist>
            <div class="test-row">
              <label class="check-row"><input v-model="useSelectionAsRegion" type="checkbox" data-testid="studio-region-check" /> 选框作搜索区</label>
              <label class="check-row num">阈值 <input v-model.number="threshold" class="input num" type="number" min="0" max="1" step="0.01" data-testid="studio-threshold" /></label>
            </div>
            <button
              class="btn btn-sm"
              type="button"
              :disabled="!yamlReady || frameError || testing"
              data-testid="studio-test"
              @click="runTest"
            >{{ testing ? '匹配中…' : '🎯 测试匹配' }}</button>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup>
// 模板工作台弹窗（Phase 7 §10.2）：视频模式从确定帧框选创建模板 + 离线匹配测试。
// - 底图 = 服务端确定帧 PNG（按帧身份寻址，字节级可重复；**保存绝不重抓画面**）；
// - 创建经 gamer.yaml 动作清单缝 template.create_from_frame（服务端命名规则 +
//   灰度归一化 + 短名冲突检测），携带帧身份与校准元数据；
// - 离线测试 = vision.test_template（复用 Core REST，media_id+帧身份寻址，类型层面
//   不触设备），命中/搜索区叠加在帧上，结果记录帧身份与坐标空间；
// - 模板存储空间 = oriented 帧像素空间（与 live 模板一致，见 templateStudio.js 注释）。
import { computed, ref, watch } from 'vue'
import { api } from '../../api'
import { videoApi } from './videoApi'
import {
  describeFrameIdentity, eventToImagePoint, orientedToReference,
  pixelRectToStyle, regionFromRect, regionToPixelRect,
} from './templateStudio'

const props = defineProps({
  open: { type: Boolean, default: false },
  /** 媒体库元数据（id/width/height...）。 */
  media: { type: Object, default: null },
  /** 帧身份 {frameIndex, ptsUs}（时间轴解析后的展示序帧；null = 未锁定）。 */
  frame: { type: Object, default: null },
  /** 当前项目校准（含 version；可空 = 恒等）。 */
  calibration: { type: Object, default: null },
  packageId: { type: String, default: '' },
  /** gamer.yaml 是否 Running（动作清单缝的门禁态）。 */
  yamlReady: { type: Boolean, default: false },
})
const emit = defineEmits(['close', 'saved'])

const frameImg = ref(null)
const frameError = ref('')
const picking = ref(false)
const selection = ref(null) // 帧像素 {x,y,w,h}
const dragStart = ref(null)
const templateName = ref('')
const preserveColor = ref(false)
const overwrite = ref(false)
const nameConflict = ref('')
const saving = ref(false)
const error = ref('')
// 离线测试状态
const templateOptions = ref([])
const testName = ref('')
const threshold = ref(0.8)
const useSelectionAsRegion = ref(false)
const testing = ref(false)
const hitRect = ref(null)
const missRect = ref(null)
const resultText = ref('')
const savedShortName = ref('')

const frameUrl = computed(() => {
  if (!props.open || !props.media?.id || !props.frame) return ''
  if (props.frame.frameIndex !== undefined && props.frame.frameIndex !== null) {
    return videoApi.mediaFrameUrl(props.media.id, { index: props.frame.frameIndex })
  }
  return videoApi.mediaFrameUrl(props.media.id, { ptsUs: props.frame.ptsUs ?? 0 })
})

const frameLabel = computed(() => describeFrameIdentity({
  mediaId: props.media?.id,
  frameIndex: props.frame?.frameIndex,
  ptsUs: props.frame?.ptsUs,
}))

function onFrameLoad() {
  frameError.value = ''
}

function onFrameError() {
  frameError.value = '确定帧加载失败：素材可能已缺失或帧参数越界'
}

function imageRect() {
  return frameImg.value?.getBoundingClientRect() || null
}

function naturalSize() {
  return { width: frameImg.value?.naturalWidth || 0, height: frameImg.value?.naturalHeight || 0 }
}

// ---- 框选（帧像素空间） ----

function onMouseDown(event) {
  if (event.button !== 0) return
  const rect = imageRect()
  if (!rect) return
  picking.value = true
  hitRect.value = null
  missRect.value = null
  resultText.value = ''
  dragStart.value = eventToImagePoint(event, rect, naturalSize().width, naturalSize().height)
  selection.value = { x: dragStart.value.x, y: dragStart.value.y, w: 0, h: 0 }
  event.preventDefault()
}

function onMouseMove(event) {
  if (!picking.value || !dragStart.value) return
  const rect = imageRect()
  if (!rect) return
  const point = eventToImagePoint(event, rect, naturalSize().width, naturalSize().height)
  selection.value = {
    x: Math.min(dragStart.value.x, point.x),
    y: Math.min(dragStart.value.y, point.y),
    w: Math.abs(point.x - dragStart.value.x),
    h: Math.abs(point.y - dragStart.value.y),
  }
}

function onMouseUp() {
  if (!picking.value) return
  picking.value = false
  dragStart.value = null
}

const selectStyle = computed(() => {
  if (!selection.value || !frameImg.value) return null
  const { width, height } = naturalSize()
  return pixelRectToStyle(selection.value, imageRect(), width, height)
})

/** 选框 → 相对区域（0..1）+ 参考坐标读数（校准显示）。 */
const selectionRegion = computed(() => {
  if (!selection.value || !frameImg.value) return null
  const { width, height } = naturalSize()
  return regionFromRect(selection.value, width, height)
})

const selectionInfo = computed(() => {
  if (!selectionRegion.value) return '在左侧帧上拖拽框选模板区域'
  const { width, height } = naturalSize()
  const region = selectionRegion.value
  const refPt = orientedToReference(
    { x: (region[0] + region[2]) / 2 * width, y: (region[1] + region[3]) / 2 * height },
    props.calibration,
    { width: props.media?.width || width, height: props.media?.height || height },
  )
  const cal = props.calibration?.version ? ` · 校准 v${props.calibration.version}` : ''
  return `选框 ${Math.round(selection.value.w)}×${Math.round(selection.value.h)}px（帧空间）`
    + ` · 参考 (${refPt.x.toFixed(0)}, ${refPt.y.toFixed(0)})${cal}`
})

// ---- 保存模板（经动作清单缝） ----

async function saveTemplate() {
  if (saving.value || !selectionRegion.value) return
  saving.value = true
  error.value = ''
  nameConflict.value = ''
  try {
    const { width, height } = naturalSize()
    const pngBase64 = await cropSelectionToBase64()
    const name = templateName.value || defaultTemplateName(selectionRegion.value)
    const result = await videoApi.createTemplateFromFrame({
      packageId: props.packageId,
      name,
      pngBase64,
      region: selectionRegion.value,
      preserveColor: preserveColor.value,
      overwrite: overwrite.value,
      frame: {
        media_id: props.media.id,
        frame_index: props.frame?.frameIndex ?? null,
        pts_us: props.frame?.ptsUs ?? null,
      },
      calibration: {
        version: Number(props.calibration?.version) || 1,
        reference_size: props.calibration?.reference_size
          ? [props.calibration.reference_size.width, props.calibration.reference_size.height]
          : null,
        rotation: props.calibration?.rotation ?? null,
      },
    })
    savedShortName.value = String(result?.short_name || '')
    resultText.value = `模板已保存：${result?.name}（${Math.round(width)}×${Math.round(height)} 帧空间，灰度 ${result?.size}B）`
    emit('saved', result)
    await refreshTemplateOptions()
    if (!testName.value) testName.value = savedShortName.value
  } catch (e) {
    if (String(e?.message || '').includes('短名冲突')) {
      nameConflict.value = parseConflictName(e?.message)
      error.value = `模板短名冲突：${nameConflict.value}。勾选「覆盖」后重试，或换个名字。`
    } else {
      error.value = `保存失败：${e?.message || e}`
    }
  } finally {
    saving.value = false
  }
}

/** 从确定帧 <img> 裁选框为 PNG base64（帧像素空间；绝不另抓画面）。 */
function cropSelectionToBase64() {
  const img = frameImg.value
  const rect = selection.value
  if (!img || !rect) return Promise.resolve('')
  const canvas = document.createElement('canvas')
  canvas.width = Math.max(1, Math.round(rect.w))
  canvas.height = Math.max(1, Math.round(rect.h))
  const ctx = canvas.getContext('2d')
  ctx.imageSmoothingEnabled = false
  ctx.drawImage(img, Math.round(rect.x), Math.round(rect.y), Math.round(rect.w), Math.round(rect.h), 0, 0, canvas.width, canvas.height)
  return new Promise((resolve, reject) => {
    try {
      canvas.toBlob(blob => {
        if (!blob) return reject(new Error('选框裁剪失败'))
        const reader = new FileReader()
        reader.onload = () => resolve(String(reader.result).split(',')[1] || '')
        reader.onerror = () => reject(new Error('选框编码失败'))
        reader.readAsDataURL(blob)
      }, 'image/png')
    } catch (e) { reject(e) }
  })
}

function defaultTemplateName(region) {
  const toInt3 = v => String(Math.min(999, Math.round(v * 1000))).padStart(3, '0')
  return `视频模板#${region.map(toInt3).join('_')}`
}

function parseConflictName(message) {
  const match = /模板短名冲突: (.+?)（/.exec(String(message))
  return match ? match[1] : ''
}

// ---- 离线测试（vision REST 复用；media 帧身份寻址） ----

async function refreshTemplateOptions() {
  if (!props.packageId) return
  try {
    const list = await api.listTemplates(props.packageId)
    const names = new Set(list.map(item => String(item.name || '')))
    templateOptions.value = [...names]
      .map(name => name.replace(/#[^#./\\]+(\.png)$/i, '$1').replace(/\.png$/i, ''))
    // 短名去重
    templateOptions.value = [...new Set(templateOptions.value)]
  } catch {
    templateOptions.value = []
  }
}

async function runTest() {
  if (testing.value || frameError.value) return
  testing.value = true
  error.value = ''
  hitRect.value = null
  missRect.value = null
  resultText.value = ''
  try {
    const { width, height } = naturalSize()
    const shortName = testName.value || savedShortName.value
    if (!shortName) throw new Error('请填写要测试的模板短名（或先保存一个模板）')
    const region = useSelectionAsRegion.value && selectionRegion.value
      ? regionToPixelRect(selectionRegion.value, width, height)
      : null
    const result = await videoApi.visionTestTemplate({
      packageId: props.packageId,
      name: shortName,
      threshold: threshold.value,
      region,
      frame: {
        mediaId: props.media.id,
        frameIndex: props.frame?.frameIndex ?? null,
        ptsUs: props.frame?.ptsUs ?? null,
      },
    })
    const identity = result?.frame ? `帧 #${result.frame.frame_index} · pts_us=${result.frame.pts_us}` : ''
    if (result?.hit) {
      hitRect.value = { x: result.x, y: result.y, w: result.width, h: result.height }
      const center = orientedToReference(
        { x: result.x + result.width / 2, y: result.y + result.height / 2 },
        props.calibration,
        { width: props.media?.width || width, height: props.media?.height || height },
      )
      resultText.value = `命中：${shortName} 置信度 ${Number(result.score).toFixed(3)}`
        + ` · 帧空间 (${Math.round(result.x + result.width / 2)}, ${Math.round(result.y + result.height / 2)})`
        + ` · 参考 (${center.x.toFixed(0)}, ${center.y.toFixed(0)})${identity ? ` · ${identity}` : ''}`
    } else {
      const shown = result?.region || region || { x: 0, y: 0, w: width, h: height }
      const [rx, ry, rw, rh] = Array.isArray(shown) ? shown : [shown.x, shown.y, shown.w, shown.h]
      missRect.value = { x: rx, y: ry, w: rw, h: rh }
      resultText.value = `未命中：${shortName}（红框 = 本次搜索区域）${identity ? ` · ${identity}` : ''}`
    }
  } catch (e) {
    error.value = `测试失败：${e?.message || e}`
  } finally {
    testing.value = false
  }
}

const hitStyle = computed(() => {
  if (!hitRect.value || !frameImg.value) return null
  const { width, height } = naturalSize()
  return pixelRectToStyle(hitRect.value, imageRect(), width, height)
})

const missStyle = computed(() => {
  if (!missRect.value || !frameImg.value) return null
  const { width, height } = naturalSize()
  return pixelRectToStyle(missRect.value, imageRect(), width, height)
})

function close() {
  emit('close')
}

// 打开时重置交互态 + 拉模板候选；素材/帧变化时清选框
watch(() => props.open, open => {
  if (!open) return
  selection.value = null
  hitRect.value = null
  missRect.value = null
  resultText.value = ''
  error.value = ''
  nameConflict.value = ''
  overwrite.value = false
  frameError.value = ''
  void refreshTemplateOptions()
})
watch(() => [props.media?.id, props.frame?.frameIndex, props.frame?.ptsUs], () => {
  selection.value = null
  hitRect.value = null
  missRect.value = null
  resultText.value = ''
  frameError.value = ''
})
</script>

<style scoped>
.studio-mask { position: fixed; inset: 0; z-index: 200; display: flex; align-items: center; justify-content: center; padding: 20px; background: rgba(4,6,10,.76); backdrop-filter: blur(4px); }
.studio { width: min(980px, 96vw); max-height: 92vh; display: flex; flex-direction: column; gap: 8px; padding: 14px 16px; background: var(--bg-2); border: 1px solid var(--border); border-radius: 14px; box-shadow: var(--shadow); overflow: hidden; }
.studio-head { display: flex; align-items: center; gap: 10px; }
.studio-title { font-size: 14px; font-weight: 700; color: var(--text-0); margin-right: auto; }
.studio-frame { color: var(--text-2); font-size: 11px; }
.studio-dep { padding: 6px 9px; border: 1px solid rgba(251,191,36,.4); border-radius: var(--radius-sm); background: rgba(251,191,36,.08); color: var(--warn); font-size: 11px; line-height: 1.5; }
.studio-error { padding: 5px 8px; border: 1px solid rgba(248,113,113,.35); border-radius: var(--radius-sm); background: rgba(248,113,113,.08); color: var(--danger); font-size: 11px; line-height: 1.5; word-break: break-all; }
.studio-body { display: grid; grid-template-columns: minmax(0, 1fr) 250px; gap: 12px; min-height: 0; flex: 1; }
.stage-box { position: relative; min-height: 220px; overflow: auto; border: 1px solid var(--border); border-radius: var(--radius-sm); background: #000; display: flex; }
.frame-img { display: block; margin: auto; max-width: 100%; max-height: 62vh; object-fit: contain; user-select: none; }
.frame-img.picking { cursor: crosshair; }
.frame-error { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; color: var(--danger); font-size: 12px; padding: 16px; text-align: center; }
.overlay-select { position: absolute; border: 1.5px solid rgba(34,211,165,.95); background: rgba(34,211,165,.12); pointer-events: none; }
.overlay-hit { position: absolute; border: 1.5px solid rgba(74,222,128,.95); box-shadow: 0 0 0 1px rgba(0,0,0,.6); pointer-events: none; }
.overlay-miss { position: absolute; border: 1.5px dashed rgba(248,113,113,.9); pointer-events: none; }
.result-line { position: absolute; left: 0; right: 0; bottom: 0; padding: 5px 8px; background: rgba(8,10,16,.82); color: var(--text-0); font-size: 11px; line-height: 1.5; word-break: break-all; }
.studio-side { display: flex; flex-direction: column; gap: 10px; min-height: 0; overflow: auto; }
.side-block { display: flex; flex-direction: column; gap: 6px; padding: 9px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-1); }
.side-title { font-size: 12px; font-weight: 700; color: var(--text-0); }
.side-hint { color: var(--text-2); font-size: 10px; line-height: 1.5; }
.side-block .input { width: 100%; padding: 4px 7px; font-size: 11px; }
.input.num { width: 56px; }
.check-row { display: flex; align-items: center; gap: 5px; font-size: 11px; color: var(--text-1); user-select: none; }
.check-row.num { margin-left: auto; }
.test-row { display: flex; align-items: center; justify-content: space-between; gap: 6px; flex-wrap: wrap; }
.mini-btn { border: 1px solid var(--border); border-radius: 4px; background: var(--bg-2); color: var(--text-1); cursor: pointer; font-size: 11px; padding: 2px 7px; }
.mini-btn:hover { border-color: var(--accent); color: var(--accent); }
.mono { font-family: var(--mono); }
@media (max-width: 860px) { .studio-body { grid-template-columns: 1fr; } }
</style>
