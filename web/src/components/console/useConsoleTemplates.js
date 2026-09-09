import { computed, nextTick, onUnmounted, provide, reactive, ref } from 'vue'
import { pinyin } from 'pinyin-pro'
import { api } from '../../api'
import { collectTemplateEntries, planTemplateImports } from '../../console/template-upload'
import {
  defaultTemplateName,
  deviceRectStyle as mapDeviceRectStyle,
  selectionToDeviceRect,
  toDeviceCoord as mapToDeviceCoord,
} from '../../console/geometry'
import { composeTemplateName, putTemplateBytes, resolveTemplateVersion } from '../../console/template-resource'

/**
 * 模板面板（console.templates 扩展面板实现）：模板列表/模糊搜索、框选与二次裁切、
 * 放大镜、测试匹配、步骤编辑器取值工具（seCellTools：选点/取色/框选回填）与 bridge 框选。
 * 模板数据由本面板实现自加载进共享 store（壳不预拉业务资源，ADR-11 知识边界）。
 */
export function useConsoleTemplates({
  toast,
  store,
  templatesData,
  packageId,
  connected,
  videoElement,
  videoWrap,
  current,
  // 统一舞台来源桥（useConsoleStage.templateBridge，可选）：框选/裁切/放大镜工作在
  // 当前舞台画面上——live = 现有视频帧；media = 服务端指定帧 PNG（绝不在保存时
  // 重抓最新设备画面）。未注入时保持旧行为（仅实时画面）。
  stage = null,
  // 来自设备管理 composable（模板页签顶部的分区行）
  // 来自脚本运行 composable（懒解析箭头，规避组合顺序）
  editorMatchThreshold,
  clearCallParamsCache,
  refreshScripts,
  refreshFnLib,
}) {
  const picking = ref(false)
  let bridgeRegionResolve = null
  const testThreshold = ref(0.8)
  // 模板匹配区域：'' = 默认（按模板名自动识别），否则 a/u/d/l/r/ul/ur/dl/dr（测试匹配与生成记录共用）
  const testRegion = ref('')
  const selecting = ref(false)
  const selStart = reactive({ x: 0, y: 0 })
  const selEnd = reactive({ x: 0, y: 0 })
  const showHit = ref(false)
  const hit = reactive({ x: 0, y: 0, w: 0, h: 0 })
  const hitLabel = ref('')
  // true = 展示的是未命中的搜索区域框（虚线红），false = 命中框（实线绿）
  const hitMiss = ref(false)
  let hitTimer = null
  // 模板列表：查看大图 / 删除二次确认 / 重命名
  const viewTpl = ref(null)
  const confirmDelTpl = ref(null)
  const renaming = ref(null)   // 正在重命名的模板名（null=不在重命名）
  const renameVal = ref('')    // 重命名输入框内容
  let renameInputEl = null     // 重命名输入框元素（自动聚焦/全选）
  // 二次裁切（右侧面板）
  const crop = reactive({ active: false, imgW: 0, imgH: 0, baseW: 0, baseH: 0, originX: 0, originY: 0, rect: { x: 0, y: 0, w: 0, h: 0 }, preview: '', name: '', zoom: 1, preserveColor: false, conflict: null, sourceLabel: '' })
  const cropCanvas = ref(null)
  const cropSec = ref(null)
  // 二次裁切底图：框选时冻结的初始画面，拖动时只动遮罩框
  let cropBaseCanvas = null
  const cropDrag = reactive({ mode: null, startX: 0, startY: 0, orig: null })
  const saving = ref(false)
  // 放大预览镜
  const loupe = reactive({ show: false, x: 0, y: 0, zoom: 2.5 })
  const loupeCanvas = ref(null)
  // 模板模糊搜索词（短名/带 #后缀 全名均可命中）
  const tplSearch = ref('')
  // 模板名拼音首字母缓存（非汉字字符原样保留）：「日常遗器.png」→ "rcyq.png"，供搜索匹配
  const tplPyCache = new Map()

  // 舞台活动画面元素：live = WebRTC video；media = 媒体 <video>（坐标参考随之切换）。
  // 未注入舞台桥时回落实时视频元素（行为与旧实现一致）。
  const surface = () => stage?.surfaceEl?.() || videoElement.value

  function tplPinyinInitials(name) {
    let s = tplPyCache.get(name)
    if (s === undefined) {
      s = pinyin(name, { pattern: 'first', toneType: 'none', type: 'array' })
        .join('').replace(/\s+/g, '').toLowerCase()
      tplPyCache.set(name, s)
    }
    return s
  }

  // 模板列表：当前 Package 过滤（templatesData 全量快照，条目 pkg 字段 = Package id）；
  // 有搜索词时三口径并列匹配（全名/短名子串 + 中文名拼音首字母），任一命中即展示，
  // 排序按最早命中位置（拼音命中加偏移恒排文字命中之后），同级按修改时间倒序；无搜索词按修改时间倒序
  const templates = computed(() => {
    let list = templatesData.value.filter(t => t.pkg === packageId.value)
    const q = tplSearch.value.trim().toLowerCase()
    if (q) {
      // 首字母串不含中文，查询词含中文时跳过该口径（必然无交集）
      const pyAble = !/[\u4e00-\u9fff]/.test(q)
      const PY_OFFSET = 1e4
      list = list.map(t => {
        let idx = t.name.toLowerCase().indexOf(q)
        const si = tplShortName(t.name).toLowerCase().indexOf(q)
        if (idx === -1 || (si !== -1 && si < idx)) idx = si
        if (idx === -1 && pyAble) {
          const pi = tplPinyinInitials(t.name).indexOf(q)
          if (pi !== -1) idx = PY_OFFSET + pi
        }
        return idx === -1 ? null : { t, idx }
      }).filter(Boolean).sort((a, b) => a.idx - b.idx || (b.t.mtime || 0) - (a.t.mtime || 0)).map(x => x.t)
    } else {
      list = list.sort((a, b) => (b.mtime || 0) - (a.mtime || 0))
    }
    return list
  })

  // 模板短名候选（画布 tmpl 控件 datalist）
  const templateNames = computed(() =>
    templatesData.value.filter(t => t.pkg === packageId.value).map(t => tplShortName(t.name)))

  const selStyle = computed(() => ({
    left: Math.min(selStart.x, selEnd.x) + 'px',
    top: Math.min(selStart.y, selEnd.y) + 'px',
    width: Math.abs(selEnd.x - selStart.x) + 'px',
    height: Math.abs(selEnd.y - selStart.y) + 'px'
  }))

  const hitStyle = computed(() => {
    const vw = videoWrap.value
    if (!vw) return {}
    const rect = vw.getBoundingClientRect()
    const vw_ = rect.width, vh = rect.height
    const el = surface()
    const sw = el?.videoWidth || 1920
    const sh = el?.videoHeight || 1080
    const ratio = Math.min(vw_ / sw, vh / sh)
    const w = hit.w * ratio, h = hit.h * ratio
    const x = (hit.x * ratio) + (vw_ - sw * ratio) / 2
    const y = (hit.y * ratio) + (vh - sh * ratio) / 2
    return { left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' }
  })

  /** 设备像素矩形 → 显示坐标样式（object-fit: contain 的 letterbox 映射；脚本事件效果用） */
  function deviceRectStyle(x, y, w = 0, h = 0) {
    const vw = videoWrap.value
    if (!vw) return {}
    const rect = vw.getBoundingClientRect()
    const el = surface()
    return mapDeviceRectStyle(x, y, w, h, rect, el?.videoWidth, el?.videoHeight)
  }

  /** 鼠标坐标 → 设备坐标（object-fit: contain 换算；随舞台来源切换参考尺寸） */
  function toDeviceCoord(clientX, clientY) {
    const el = surface()
    const rect = el.getBoundingClientRect()
    return mapToDeviceCoord(clientX, clientY, rect, el.videoWidth, el.videoHeight)
  }

  // ---------- 框选保存模板 ----------

  /** 框选矩形（容器 CSS 坐标）→ 设备像素坐标，自动裁剪 letterbox 黑边并夹取到画面内 */
  function selToDeviceRect() {
    const el = surface()
    const rect = videoWrap.value.getBoundingClientRect()
    return selectionToDeviceRect(selStart, selEnd, rect, el?.videoWidth, el?.videoHeight)
  }

  /** 生成默认模板名：随机名字#x1_y1_x2_y2（相对坐标 0~1，×1000 存 3 位整数，如 0.123→123，不带 .png 后缀） */
  function defaultTplName(rect) {
    const el = surface()
    return defaultTemplateName(rect, el?.videoWidth, el?.videoHeight)
  }

  // ---------- 二次裁切 ----------

  const cropSize = computed(() => `${Math.round(crop.rect.w)}×${Math.round(crop.rect.h)} px`)
  /** 当前显示缩放（100% = 自适应适配），滚轮调整 */
  const cropZoomPct = computed(() => `${Math.round(crop.zoom * 100)}%`)

  /** 冻结裁切底图（指定帧）：二次裁切时底图不动，只动遮罩框；保存管线
   *  （PUT 模板资源）只消费这份冻结底图，绝不重抓任何画面源 */
  function freezeCropBase(source, imgW, imgH, label, rect) {
    crop.imgW = imgW
    crop.imgH = imgH
    crop.sourceLabel = label || ''
    crop.originX = Math.round(rect.x)
    crop.originY = Math.round(rect.y)
    crop.baseW = Math.round(rect.w)
    crop.baseH = Math.round(rect.h)
    crop.zoom = 1
    crop.preserveColor = false
    cropBaseCanvas = document.createElement('canvas')
    cropBaseCanvas.width = crop.baseW
    cropBaseCanvas.height = crop.baseH
    cropBaseCanvas.getContext('2d').drawImage(source, crop.originX, crop.originY, crop.baseW, crop.baseH, 0, 0, crop.baseW, crop.baseH)
    crop.rect = { x: 0, y: 0, w: crop.baseW, h: crop.baseH }
    crop.name = defaultTplName(rect)
    crop.active = true
    nextTick(() => {
      renderCropFrame()
      refreshCropPreview()
      cropSec.value?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
    })
  }

  /** 框选完成后打开右侧裁切区：底图 = 当前舞台来源的「指定帧」——
   *  live = 现有视频元素当前画面（既有截图路径，同步冻结不变）；media = 服务端
   *  确定帧 PNG（stage.captureFrame）。异步取帧后校验 generation，来源已切换的
   *  过期结果不应用（不得偷偷画到新画面上）。 */
  async function openCrop(rect) {
    confirmDelTpl.value = null
    crop.conflict = null
    crop.sourceLabel = ''
    if (stage && stage.kind?.() === 'live') {
      const video = stage.surfaceEl?.() || videoElement.value
      if (!video?.videoWidth) return toast('无法截取画面，请稍后重试', 'error')
      freezeCropBase(video, video.videoWidth, video.videoHeight, '实时画面当前帧', rect)
      return
    }
    if (stage?.captureFrame) {
      const genBefore = stage.generation()
      const frame = await stage.captureFrame()
      if (!frame) return toast('无法获取画面帧，请稍后重试', 'error')
      if (stage.generation() !== genBefore || frame.generation !== genBefore) {
        return toast('画面来源已切换，请重新框选', 'warn')
      }
      freezeCropBase(frame.source, frame.width, frame.height, frame.label, rect)
      return
    }
    // 未注入舞台桥（旧调用方兼容）：实时视频元素
    const video = videoElement.value
    if (!video?.videoWidth) return toast('无法截取画面，请稍后重试', 'error')
    freezeCropBase(video, video.videoWidth, video.videoHeight, '实时画面当前帧', rect)
  }

  function cancelCrop() {
    crop.active = false
    crop.conflict = null
    cropBaseCanvas = null
    crop.zoom = 1
    hideLoupe()
    // 弹窗取消 → 未完成的回填请求按取消处理
    if (cellCaptureResolve) { cellCaptureResolve(null); cellCaptureResolve = null }
  }

  function repick() {
    crop.active = false
    crop.conflict = null
    cropBaseCanvas = null
    crop.zoom = 1
    picking.value = true
    toast('在画面上重新框选', 'info')
  }

  /** 画布适配尺寸：展示冻结的初始框选画面，可适当放大（再乘滚轮缩放 crop.zoom） */
  function cropFit() {
    const w = Math.max(1, crop.baseW)
    const h = Math.max(1, crop.baseH)
    const scale = Math.min(260 / w, 220 / h, 3) * crop.zoom
    return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)), scale: Math.round(w * scale) / w }
  }

  /** 滚轮缩放裁切底图：以光标下的图像点为锚点放大/缩小，缩放后画布超出区域可滚动查看 */
  function cropWheel(e) {
    const canvas = cropCanvas.value
    const stage = cropSec.value?.querySelector('.crop-stage')
    if (!canvas || !stage) return
    e.preventDefault()
    const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15
    const next = Math.max(0.5, Math.min(8, crop.zoom * factor))
    if (next === crop.zoom) return
    const cr = canvas.getBoundingClientRect()
    const sr = stage.getBoundingClientRect()
    // 光标在画布内的位置（画布 CSS 像素 = 画布像素）
    const mx = e.clientX - cr.left
    const my = e.clientY - cr.top
    // 画布原点在滚动内容中的位置
    const ox = cr.left - sr.left + stage.scrollLeft
    const oy = cr.top - sr.top + stage.scrollTop
    const oldW = canvas.width
    const oldH = canvas.height
    crop.zoom = next
    renderCropFrame()
    const kx = canvas.width / oldW
    const ky = canvas.height / oldH
    // 保持光标下的图像点不动：margin:auto 居中时原点 = max(0, (区域宽 - 画布宽)/2)
    const ox1 = Math.max(0, (stage.clientWidth - canvas.width) / 2)
    const oy1 = Math.max(0, (stage.clientHeight - canvas.height) / 2)
    stage.scrollLeft += ox1 + mx * kx - ox - mx
    stage.scrollTop += oy1 + my * ky - oy - my
  }

  /** 在裁切画布上绘制冻结的框选画面 + 可拖动的遮罩框（拖动时只改框，不动底图） */
  function renderCropFrame() {
    const canvas = cropCanvas.value
    const base = cropBaseCanvas
    if (!canvas || !base || base.width < 1 || crop.rect.w < 1 || crop.rect.h < 1) return
    const bw = base.width
    const bh = base.height
    const fit = cropFit()
    canvas.width = fit.w
    canvas.height = fit.h
    canvas.style.width = fit.w + 'px'
    canvas.style.height = fit.h + 'px'
    const ctx = canvas.getContext('2d')
    ctx.clearRect(0, 0, fit.w, fit.h)
    ctx.drawImage(base, 0, 0, bw, bh, 0, 0, fit.w, fit.h)

    const sx = fit.w / bw
    const sy = fit.h / bh
    const rx = crop.rect.x * sx
    const ry = crop.rect.y * sy
    const rw = crop.rect.w * sx
    const rh = crop.rect.h * sy

    // 遮罩框外压暗，拖动时只改变这个遮罩
    ctx.fillStyle = 'rgba(0,0,0,.45)'
    ctx.fillRect(0, 0, fit.w, ry)
    ctx.fillRect(0, ry + rh, fit.w, Math.max(0, fit.h - ry - rh))
    ctx.fillRect(0, ry, rx, rh)
    ctx.fillRect(rx + rw, ry, Math.max(0, fit.w - rx - rw), rh)

    // 边框
    ctx.strokeStyle = 'rgba(34,211,165,.95)'
    ctx.lineWidth = 1.5
    ctx.strokeRect(rx, ry, rw, rh)

    // 角点手柄
    ctx.fillStyle = '#fff'
    const hs = 5
    for (const [hx, hy] of [[rx, ry], [rx + rw, ry], [rx, ry + rh], [rx + rw, ry + rh]]) {
      ctx.fillRect(hx - hs / 2, hy - hs / 2, hs, hs)
    }

    // 尺寸标注
    ctx.fillStyle = 'rgba(34,211,165,.95)'
    ctx.font = '10px monospace'
    ctx.fillText(cropSize.value, rx + 6, ry + 14)
  }

  /** 按当前遮罩框从冻结底图重新生成裁剪结果预览（全分辨率） */
  function refreshCropPreview() {
    const base = cropBaseCanvas
    if (!base || base.width < 1) return
    const r = crop.rect
    if (r.w < 1 || r.h < 1) return
    const canvas = document.createElement('canvas')
    canvas.width = Math.round(r.w)
    canvas.height = Math.round(r.h)
    canvas.getContext('2d').drawImage(base, r.x, r.y, r.w, r.h, 0, 0, Math.round(r.w), Math.round(r.h))
    crop.preview = canvas.toDataURL('image/png')
  }

  /** 鼠标事件 → 冻结底图上的像素坐标 */
  function cropEventDev(e) {
    const canvas = cropCanvas.value
    const rect = canvas.getBoundingClientRect()
    const scale = canvas.width / crop.baseW
    return {
      x: (e.clientX - rect.left) / scale,
      y: (e.clientY - rect.top) / scale
    }
  }

  function cropMouseDown(e) {
    const p = cropEventDev(e)
    const r = crop.rect
    const HIT = 12 / (cropCanvas.value.width / crop.baseW) // 底图像素命中半径
    const corners = { nw: [r.x, r.y], ne: [r.x + r.w, r.y], sw: [r.x, r.y + r.h], se: [r.x + r.w, r.y + r.h] }
    let mode = null
    for (const [k, [hx, hy]] of Object.entries(corners)) {
      if (Math.hypot(p.x - hx, p.y - hy) <= HIT) { mode = k; break }
    }
    if (!mode) {
      const edges = {
        n: [r.x + r.w / 2, r.y], s: [r.x + r.w / 2, r.y + r.h],
        w: [r.x, r.y + r.h / 2], e: [r.x + r.w, r.y + r.h / 2]
      }
      for (const [k, [hx, hy]] of Object.entries(edges)) {
        const onSeg = (k === 'n' || k === 's') ? (p.x >= r.x && p.x <= r.x + r.w) : (p.y >= r.y && p.y <= r.y + r.h)
        if (Math.hypot(p.x - hx, p.y - hy) <= HIT && onSeg) { mode = k; break }
      }
    }
    if (!mode && p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h) mode = 'move'
    if (!mode) return
    cropDrag.mode = mode
    cropDrag.startX = p.x
    cropDrag.startY = p.y
    cropDrag.orig = { ...r }
    e.preventDefault()
  }

  function cropMouseMove(e) {
    const p = cropEventDev(e)
    // 放大镜仍按完整画面坐标显示（底图坐标 + 初始框选偏移）
    updateLoupe(e.clientX, e.clientY, { x: p.x + crop.originX, y: p.y + crop.originY }, 3, [{ x: crop.rect.x + crop.originX, y: crop.rect.y + crop.originY, w: crop.rect.w, h: crop.rect.h }])
    if (!cropDrag.mode) return
    const o = cropDrag.orig
    const r = crop.rect
    const MIN = 8
    const dx = p.x - cropDrag.startX
    const dy = p.y - cropDrag.startY
    const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v))
    switch (cropDrag.mode) {
      case 'move':
        r.x = clamp(o.x + dx, 0, crop.baseW - o.w)
        r.y = clamp(o.y + dy, 0, crop.baseH - o.h)
        break
      case 'nw':
        r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
        r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
        r.w = o.x + o.w - r.x; r.h = o.y + o.h - r.y
        break
      case 'ne':
        r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
        r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
        r.h = o.y + o.h - r.y
        break
      case 'sw':
        r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
        r.w = o.x + o.w - r.x
        r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
        break
      case 'se':
        r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
        r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
        break
      case 'n':
        r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
        r.h = o.y + o.h - r.y
        break
      case 's':
        r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
        break
      case 'w':
        r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
        r.w = o.x + o.w - r.x
        break
      case 'e':
        r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
        break
    }
    renderCropFrame()
  }

  function cropMouseUp() {
    if (!cropDrag.mode) return
    cropDrag.mode = null
    refreshCropPreview()
  }

  function cropMouseLeave() {
    hideLoupe()
    if (cropDrag.mode) { cropDrag.mode = null; refreshCropPreview() }
  }

  /** 上传响应的体积提示：823KB → 96KB（服务端 PNG 重编码，默认灰度） */
  function tplSizeHint(rep) {
    if (!rep?.size || !rep?.orig_size) return ''
    const fmt = n => n >= 1024 * 1024 ? (n / 1024 / 1024).toFixed(1) + 'MB' : n >= 1024 ? Math.round(n / 1024) + 'KB' : n + 'B'
    return `（${fmt(rep.orig_size)} → ${fmt(rep.size)}）`
  }

  function cropUploadPayload() {
    const raw = crop.name.trim()
    if (!raw) { toast('请输入模板名称', 'warn'); return null }
    if (!packageId.value) { toast('请先在右上选择配置', 'warn'); return null }
    const name = raw.toLowerCase().endsWith('.png') ? raw : raw + '.png'
    const shortName = name.replace(/#[^#]+\.png$/i, '.png')
    const region = [
      crop.originX / crop.imgW,
      crop.originY / crop.imgH,
      (crop.originX + crop.baseW) / crop.imgW,
      (crop.originY + crop.baseH) / crop.imgH,
    ]
    return { shortName, dataB64: crop.preview.split(',')[1], region, pkg: packageId.value, preserveColor: crop.preserveColor }
  }

  function findCropConflict(shortName) {
    const wanted = tplShortName(shortName).toLowerCase()
    return templatesData.value.find(t => t.pkg === packageId.value && tplShortName(t.name).toLowerCase() === wanted) || null
  }

  function showCropConflict(shortName, existing) {
    crop.conflict = { name: existing.name, shortName, version: existing.version || null }
  }

  function backToCrop() {
    crop.conflict = null
    nextTick(() => {
      renderCropFrame()
      refreshCropPreview()
    })
  }

  async function refreshTemplatesData() {
    try {
      templatesData.value = await api.listTemplates(packageId.value)
      return true
    } catch {
      return false
    }
  }
  // 模板列表由本面板实现自加载（Console 壳不再预拉业务资源，ADR-11 知识边界）
  refreshTemplatesData()

  async function finishCropSave(rep, shortName) {
    const refreshed = await refreshTemplatesData()
    crop.conflict = null
    crop.active = false
    cropBaseCanvas = null
    hideLoupe()
    toast(`模板 ${rep?.name || shortName} 已保存${tplSizeHint(rep)}${refreshed ? '' : '（模板列表刷新失败）'}`, refreshed ? 'success' : 'warn')
    // 框选回填：保存成功把模板短名交回发起框选的单元格（CellEditor 自动填入）
    if (cellCaptureResolve) { cellCaptureResolve(shortName); cellCaptureResolve = null }
  }

  async function saveTemplate() {
    if (saving.value) return
    const payload = cropUploadPayload()
    if (!payload) return
    const existing = findCropConflict(payload.shortName)
    if (existing) {
      showCropConflict(payload.shortName, existing)
      return
    }
    saving.value = true
    try {
      const name = composeTemplateName(payload.shortName, payload.region, payload.preserveColor)
      const rep = await putTemplateBytes(name, payload.dataB64, payload.pkg)
      await finishCropSave(rep, payload.shortName)
    } catch (e) {
      // 列表可能在本页打开后被其他页面更新；把服务端 409 也转成同一对比态。
      if (e?.status === 409) {
        try {
          templatesData.value = await api.listTemplates(packageId.value)
          const current = findCropConflict(payload.shortName)
          if (current) { showCropConflict(payload.shortName, current); return }
        } catch { /* 刷新失败时保留原错误提示 */ }
      }
      toast('保存失败：' + e.message, 'error')
    } finally {
      saving.value = false
    }
  }

  async function overwriteTemplate() {
    if (saving.value || !crop.conflict) return
    const payload = cropUploadPayload()
    if (!payload) return
    saving.value = true
    try {
      // 覆盖确认可能停留较久，先拿最新列表，避免第一次操作后仍持有已删除的旧文件名。
      const refreshed = await refreshTemplatesData()
      const existing = refreshed ? findCropConflict(payload.shortName) : crop.conflict
      // 旧模板已被其他页面删除时，退化为普通新建；创建 PUT 不带 force，
      // 若并发页面刚好抢先创建，服务端会拒绝而不会覆盖其内容。
      if (!existing) {
        const name = composeTemplateName(payload.shortName, payload.region, payload.preserveColor)
        const rep = await putTemplateBytes(name, payload.dataB64, payload.pkg)
        await finishCropSave(rep, payload.shortName)
        return
      }
      const targetName = composeTemplateName(payload.shortName, payload.region, payload.preserveColor)
      if (targetName !== existing.name) {
        // 文件名承载搜索区域/颜色标记；直接写到旧路径会让新选区与资源元数据
        // 分离。此场景必须先返回修改，或走明确的新建/重命名流程，不能静默覆盖。
        toast('框选区域或颜色标记已变化，请返回修改后以新模板名保存', 'warn')
        return
      }
      const expectedVersion = await resolveTemplateVersion(existing.name, payload.pkg, existing.version)
      // 覆盖是对同一资源路径的单次条件 PUT。这样模板名、区域和颜色标记
      // 等关联元数据保持不变；需要改变元数据时先走明确的重命名/新建流程。
      const rep = await putTemplateBytes(existing.name, payload.dataB64, payload.pkg, expectedVersion)
      await finishCropSave(rep, payload.shortName)
    } catch (e) {
      if (e?.status === 409) {
        // 条件 PUT 拒绝说明其他页面已经改变了目标；更新冲突态中的完整
        // 文件名/版本，让下一次确认覆盖重新基于最新资源提交。
        const refreshed = await refreshTemplatesData()
        const current = refreshed && findCropConflict(payload.shortName)
        if (current) showCropConflict(payload.shortName, current)
        else if (refreshed) crop.conflict = null
      }
      toast('覆盖失败：' + e.message, 'error')
    } finally {
      saving.value = false
    }
  }

  // ---------- 放大预览镜 ----------

  /** 以光标为中心放大当前画面帧：devPt 为放大中心（设备像素），rects 为要叠加显示的选区（设备像素坐标） */
  function updateLoupe(clientX, clientY, devPt, zoom, rects) {
    const video = surface()
    const canvas = loupeCanvas.value
    if (!video?.videoWidth || !canvas) return
    const c = devPt
    const L = canvas.width
    const half = L / zoom / 2
    const ctx = canvas.getContext('2d')
    ctx.clearRect(0, 0, L, L)
    ctx.imageSmoothingEnabled = true
    ctx.drawImage(video, c.x - half, c.y - half, half * 2, half * 2, 0, 0, L, L)
    // 十字准星：贯穿全幅的长线
    ctx.strokeStyle = 'rgba(255,255,255,.3)'
    ctx.lineWidth = 1
    ctx.beginPath()
    ctx.moveTo(L / 2, 0); ctx.lineTo(L / 2, L)
    ctx.moveTo(0, L / 2); ctx.lineTo(L, L / 2)
    ctx.stroke()
    // 中心点
    ctx.fillStyle = 'rgba(255,255,255,.95)'
    ctx.beginPath()
    ctx.arc(L / 2, L / 2, 2, 0, Math.PI * 2)
    ctx.fill()
    // 选区轮廓
    ctx.strokeStyle = 'rgba(34,211,165,.95)'
    ctx.lineWidth = 1.5
    for (const r of rects || []) {
      ctx.strokeRect((r.x - (c.x - half)) * zoom, (r.y - (c.y - half)) * zoom, r.w * zoom, r.h * zoom)
    }
    // 定位：跟随光标，越界自动翻转
    loupe.zoom = zoom
    loupe.show = true
    const W = 160, G = 14
    let x = clientX + G, y = clientY + G
    if (x + W > window.innerWidth - 6) x = clientX - W - G
    if (y + W > window.innerHeight - 6) y = clientY - W - G
    loupe.x = Math.max(6, x)
    loupe.y = Math.max(6, y)
  }

  function hideLoupe() { loupe.show = false }

  function togglePick() {
    confirmDelTpl.value = null
    // 框选随舞台来源工作：实时需已连接；视频来源需画面就绪（离线可框选）
    if (stage) {
      if (!stage.ready()) {
        return toast(stage.kind() === 'media' ? '请先在视频来源中选择并加载素材' : '请先连接设备', 'error')
      }
    } else if (!connected.value) {
      return toast('请先连接设备', 'error')
    }
    picking.value = !picking.value
    if (!picking.value) {
      hideLoupe()
      if (bridgeRegionResolve) { bridgeRegionResolve(null); bridgeRegionResolve = null }
      // 框选模式被手动关掉且未进入裁切 → 未完成的回填请求按取消处理
      if (cellCaptureResolve) { cellCaptureResolve(null); cellCaptureResolve = null }
    }
  }

  function tplThumbUrl(name) { return api.tplImageUrl(name, packageId.value) }

  /** 模板列表：行空白区点击 → 查看大图（缩略图/文件名单元格有各自的交互） */
  function onTplRowClick(e, t) {
    confirmDelTpl.value = null
    openTplView(t.name)
  }

  /** 模板列表缩略图：点击查看大图 */
  async function onTplThumbClick(e, t) {
    confirmDelTpl.value = null
    openTplView(t.name)
  }

  // ---------- 画布模板下拉悬停缩略图（CellEditor inject；短名 → 当前分区图片 URL） ----------
  provide('tplPreviewUrl', (short) => {
    const full = templatesData.value.find(t => t.pkg === packageId.value && tplShortName(t.name) === short)?.name
    return api.tplImageUrl(full || short, packageId.value)
  })

  // ---------- 步骤编辑器取值工具（CellEditor inject('seCellTools')）：投屏选点/选色/框选 ----------
  // 选点/选色为单次点击模式：进入后下一次画面点击解析坐标/颜色（选色走放大镜），Esc 取消；
  // 框选复用模板页签的既有流程（进入框选 → 二次裁切 → 上传），完成后模板下拉自动可见新模板。

  const cellPick = reactive({ mode: null, resolve: null }) // mode: 'coord' | 'color'
  /** 进行中的「框选生成模板」回填 resolve（CellEditor captureTemplate 等待保存结果） */
  let cellCaptureResolve = null

  /** UI Bridge 的通用区域选择：只把设备像素矩形返回给调用方，不暴露视频 DOM。 */
  function selectRegionForBridge() {
    if (!connected.value) {
      toast('请先连接设备', 'error')
      return Promise.resolve(null)
    }
    if (bridgeRegionResolve) bridgeRegionResolve(null)
    if (cellPick.mode) cancelCellPick()
    if (cellCaptureResolve) { cellCaptureResolve(null); cellCaptureResolve = null }
    picking.value = true
    toast('在画面上框选区域（Esc 取消）', 'info')
    return new Promise(resolve => { bridgeRegionResolve = resolve })
  }

  /** 当前是否有进行中的 bridge 框选请求（onMouseUp 收尾分流用）。 */
  function bridgeRegionSelected() { return !!bridgeRegionResolve }

  /** 结束 bridge 框选：把设备像素矩形交回调用方（太小则按取消处理并提示）。 */
  function finishBridgeRegionSelect(rect) {
    const resolve = bridgeRegionResolve
    bridgeRegionResolve = null
    resolve(rect.w >= 8 && rect.h >= 8
      ? { ...rect, width: videoElement.value?.videoWidth || 0, height: videoElement.value?.videoHeight || 0 }
      : null)
    if (rect.w < 8 || rect.h < 8) toast('框选区域太小，请重新框选', 'warn')
  }

  /** Esc 取消 bridge 框选；返回是否确实取消了进行中的请求。 */
  function cancelBridgeRegionSelect() {
    if (!bridgeRegionResolve) return false
    bridgeRegionResolve(null)
    bridgeRegionResolve = null
    picking.value = false
    hideLoupe()
    return true
  }

  function beginCellPick(mode) {
    if (!connected.value) {
      toast('请先连接设备', 'error')
      return Promise.resolve(null)
    }
    if (cellPick.mode) cellPick.resolve?.(null)
    return new Promise((resolve) => {
      cellPick.mode = mode
      cellPick.resolve = resolve
      toast(mode === 'color' ? '在画面上点击取色（Esc 取消）' : '在画面上点击选点（Esc 取消）', 'info')
    })
  }

  function cancelCellPick() {
    if (cellPick.mode) {
      cellPick.resolve?.(null)
      cellPick.mode = null
      cellPick.resolve = null
    }
    hideLoupe()
  }

  /** 从当前舞台画面帧采样设备像素颜色 → 6 位 hex（画面不可用返回 null） */
  function samplePixelHex(devX, devY) {
    const v = surface()
    if (!v?.videoWidth) return null
    const c = document.createElement('canvas')
    c.width = 1
    c.height = 1
    const ctx = c.getContext('2d')
    ctx.drawImage(v, Math.round(devX), Math.round(devY), 1, 1, 0, 0, 1, 1)
    const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data
    return [r, g, b].map(n => n.toString(16).padStart(2, '0')).join('')
  }

  /** 视频画面点击 → 结束取点模式：coord 回相对坐标，color 回采样 hex */
  function finishCellPick(e) {
    const v = surface()
    const pt = toDeviceCoord(e.clientX, e.clientY)
    const mode = cellPick.mode
    cellPick.mode = null
    const resolve = cellPick.resolve
    cellPick.resolve = null
    hideLoupe()
    if (!v?.videoWidth) {
      resolve?.(null)
      toast('设备画面不可用', 'warn')
      return
    }
    if (mode === 'coord') {
      resolve?.({ x: Number((pt.x / v.videoWidth).toFixed(4)), y: Number((pt.y / v.videoHeight).toFixed(4)) })
    } else if (mode === 'color') {
      const hex = samplePixelHex(pt.x, pt.y)
      if (hex) resolve?.({ hex, x: pt.x, y: pt.y })
      else { resolve?.(null); toast('取色失败：画面不可用', 'warn') }
    }
  }

  provide('seCellTools', {
    pickCoord: () => beginCellPick('coord'),
    pickColor: () => beginCellPick('color'),
    /** 按步骤实际规则匹配当前模板：服务端按短名消歧并解析文件名区域，不发送任何点击。 */
    matchTemplate: name => testMatch(name, { stepSemantics: true }),
    /** 框选生成新模板：不切页签（裁切弹窗挂面板层级，任何页签下可见），用户走既有
     *  二次裁切→保存流程；保存成功后以模板短名 resolve，CellEditor 自动回填该字段 */
    captureTemplate: () => {
      if (!connected.value) {
        toast('请先连接设备', 'error')
        return Promise.resolve(null)
      }
       // 上一次未完成的框选（未保存也未取消）作废
       if (cellCaptureResolve) cellCaptureResolve(null)
       if (bridgeRegionResolve) { bridgeRegionResolve(null); bridgeRegionResolve = null }
      picking.value = true
      toast('在画面上框选模板区域，保存后自动填入', 'info')
      return new Promise((resolve) => { cellCaptureResolve = resolve })
    },
  })

  /** 模板列表文件名点击：复制当前分区可用的模板短名 */
  async function onTplNameClick(e, t) {
    if (renaming.value === t.name) return
    confirmDelTpl.value = null
    const shortName = tplShortName(t.name)
    if (await copyText(shortName)) toast(`已复制模板短名：${shortName}`, 'success')
    else toast('复制模板短名失败', 'warn')
  }

  /** 复制文本到剪贴板：navigator.clipboard 需安全上下文（localhost），
   *  LAN http 访问时回退 execCommand（临时 textarea） */
  async function copyText(text) {
    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(text)
        return true
      }
    } catch { /* 回退 execCommand */ }
    const ta = document.createElement('textarea')
    ta.value = text
    ta.style.position = 'fixed'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    const ok = document.execCommand('copy')
    ta.remove()
    return ok
  }

  /** 模板列表：查看大图 */
  function openTplView(name) {
    confirmDelTpl.value = null
    viewTpl.value = name
  }
  function closeTplView() {
    viewTpl.value = null
  }

  // 模板查看：悬停坐标读数已随 until 的 click 参数删除一并移除（命中恒点模板中心）

  // ---------- 模板重命名 ----------

  /** 重命名输入框初始值：去掉图片后缀 */
  function renameBase(name) {
    return name.replace(/\.(png|jpe?g)$/i, '')
  }

  function startRename(t) {
    confirmDelTpl.value = null
    renaming.value = t.name
    renameVal.value = renameBase(t.name)
    nextTick(() => renameInputEl?.select())
  }

  /** 输入框失焦 / Esc → 取消重命名（不保存） */
  function cancelRename() {
    renaming.value = null
  }

  /** 确认重命名：名称去空格、自动补 .png 后缀、重名校验，成功后刷新列表 */
  async function confirmRename(t) {
    const raw = renameVal.value.trim()
    if (!raw) return toast('名称不能为空', 'warn')
    const newName = /\.(png|jpe?g)$/i.test(raw) ? raw : raw + '.png'
    renaming.value = null
    if (newName === t.name) return
    if (templatesData.value.some(x => x.pkg === packageId.value && x.name === newName)) return toast(`已存在同名模板：${newName}`, 'warn')
    try {
      await api.renameTemplate(t.name, newName, packageId.value)
      // 后端会同步改写当前分区 scripts/functions 中的模板引用；刷新脚本与函数缓存，
      // 让当前摘要、调用参数和后续编辑都立即看到新名称。
      await refreshScripts?.()
      await refreshFnLib?.(packageId.value)
      clearCallParamsCache()
      toast(`模板已重命名为 ${newName}`, 'success')
    } catch (e) {
      toast('重命名失败：' + e.message, 'error')
    }
  }

  /** 模板列表：匹配按钮（测试匹配） */
  function onTplMatchClick(t) {
    confirmDelTpl.value = null
    testMatch(t.name)
  }

  /** 模板列表：更多菜单直接删除，不再二次确认 */
  async function onTplDeleteClick(t) {
    confirmDelTpl.value = null
    try {
      await api.deleteTemplate(t.name, packageId.value)
      templatesData.value = await api.listTemplates(packageId.value)
      if (viewTpl.value === t.name) viewTpl.value = null
      toast('模板已删除', 'success')
    } catch (e) {
      toast('删除失败：' + e.message, 'error')
    }
  }

  /** 批量大小上限提示（与 template-upload.js 同口径） */
  function tplBatchHint(n) {
    return n > 6 ? `（${n} 张）` : ''
  }

  /**
   * 模板上传导入：多选图片或图片压缩包（zip 浏览器端解压，压缩包本身不上传、
   * 服务端不落临时文件）。逐张走模板字节 PUT（服务端钩子统一 8-bit 灰度归一化）；
   * 与库内同名跳过不覆盖（覆盖走模板行「替换」），zip 内同名自动消歧。
   * 结束汇总：导入 N / 跳过 M / 失败 K。
   */
  async function onTplUpload(e) {
    confirmDelTpl.value = null
    const files = [...(e.target.files || [])]
    e.target.value = ''
    if (!files.length) return
    if (!packageId.value) { toast('请先在右上选择配置', 'warn'); return }
    let planned = []
    try {
      let entries = []
      for (const file of files) {
        entries = entries.concat(await collectTemplateEntries(file))
      }
      planned = planTemplateImports(entries, templatesData.value.filter(t => t.pkg === packageId.value).map(t => t.name))
    } catch (err) {
      toast('导入失败：' + err.message, 'error')
      return
    }
    const imports = planned.filter(p => p.action === 'import')
    const skipped = planned.filter(p => p.action === 'skip')
    let ok = 0
    const failed = []
    for (const item of imports) {
      try {
        // 批量导入也是创建路径：不使用兼容封装的 force=true，避免并发
        // 页面在本地快照过期时静默覆盖同名模板。
        await putTemplateBytes(item.name, item.bytes, packageId.value)
        // 逐张入库后立刻登记，后续同批同名冲突判定与列表展示即时可见
        templatesData.value = templatesData.value.concat({
          name: item.name, pkg: packageId.value, version: null, updated_at: '', size: item.bytes.length,
        })
        ok++
      } catch (err) {
        failed.push(`${item.source}：${err.message}`)
      }
    }
    if (ok) {
      const refreshed = await refreshTemplatesData()
      if (!refreshed) toast('模板列表刷新失败，请手动刷新', 'warn')
    }
    const parts = []
    if (ok) parts.push(`导入 ${ok} 张${tplBatchHint(ok)}`)
    if (skipped.length) parts.push(`跳过 ${skipped.length} 张（已存在同名）`)
    if (failed.length) parts.push(`失败 ${failed.length} 张`)
    const level = failed.length ? 'warn' : 'success'
    toast(parts.join('，') || '没有可导入的文件', level)
    for (const line of failed.slice(0, 3)) toast(line, 'error')
  }

  /** 替换已有模板图片：名称/分区来自当前模板，图片替换使用独立当前端点。 */
  async function replaceTemplateImage(t, file) {
    if (!t || !file) return
    try {
      const b64 = await fileToBase64(file)
      const pkg = t.pkg || packageId.value
      const expectedVersion = await resolveTemplateVersion(t.name, pkg, t.version)
      await putTemplateBytes(t.name, b64, pkg, expectedVersion)
      templatesData.value = await api.listTemplates(packageId.value)
      toast(`模板 ${t.name} 图片已替换`, 'success')
    } catch (err) {
      toast('替换失败：' + err.message, 'error')
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

  /** 去掉模板名尾部的颜色标记；#1 不是搜索区域的一部分。 */
  function stripTplColorMarker(name) {
    return String(name || '').replace(/#1(\.(png|jpe?g))$/i, '$1')
  }

  /** 从模板名解析 #x1_y1_x2_y2（相对坐标 ×1000 存 3 位整数，如 123→0.123），返回 [x1,y1,x2,y2] 或 null */
  function parseTplRegion(name) {
    const base = stripTplColorMarker(name).replace(/\.(png|jpe?g)$/i, '')
    const idx = base.lastIndexOf('#')
    if (idx < 0) return null
    const parts = base.slice(idx + 1).split('_')
    if (parts.length !== 4) return null
    const nums = parts.map(s => /^\d{1,3}$/.test(s) ? Number(s) / 1000 : NaN)
    if (!nums.every(n => Number.isFinite(n) && n >= 0 && n <= 1) || !(nums[2] > nums[0]) || !(nums[3] > nums[1])) return null
    return nums
  }

  /** 从模板名解析半区代码后缀（#a/#u/#d/#l/#r/#ul/#ur/#dl/#dr），如 task_item#l.png → 'l'；无 → null */
  function parseTplRegionCode(name) {
    const base = stripTplColorMarker(name).replace(/\.(png|jpe?g)$/i, '')
    const idx = base.lastIndexOf('#')
    if (idx < 0) return null
    const code = base.slice(idx + 1).toLowerCase()
    return ['a', 'u', 'd', 'l', 'r', 'ul', 'ur', 'dl', 'dr'].includes(code) ? code : null
  }

  /** 模板短名：去掉 #区域后缀（login#0_0_500_500.png → login.png），无后缀原样返回。
   *  脚本里写短名即可，引擎自动解析到唯一匹配的带后缀文件（区域照常生效） */
  function tplShortName(name) {
    return stripTplColorMarker(name).replace(/#[^#./\\]+(\.(png|jpe?g))$/i, '$1')
  }

  /** 模板名区域徽标文本：半区码直接显示码字（l/r/dr…），数字坐标显示 ◧（悬停看全名） */
  function tplRegionBadge(name) {
    const base = stripTplColorMarker(name).replace(/\.(png|jpe?g)$/i, '')
    const idx = base.lastIndexOf('#')
    if (idx < 0) return ''
    const s = base.slice(idx + 1).toLowerCase()
    if (['a', 'u', 'd', 'l', 'r', 'ul', 'ur', 'dl', 'dr'].includes(s)) return s
    if (/^\d{1,3}(_\d{1,3}){3}$/.test(s)) return '◧'
    return ''
  }

  /** 模板名半区代码 → 设备像素搜索区域 [x, y, w, h] */
  function regionCodePixels(code, vw, vh) {
    const hw = Math.round(vw / 2)
    const hh = Math.round(vh / 2)
    const map = {
      a: null,
      u: [0, 0, vw, hh],
      d: [0, vh - hh, vw, hh],
      l: [0, 0, hw, vh],
      r: [vw - hw, 0, hw, vh],
      ul: [0, 0, hw, hh],
      ur: [vw - hw, 0, hw, hh],
      dl: [0, vh - hh, hw, hh],
      dr: [vw - hw, vh - hh, hw, hh]
    }
    return map[code] ?? null
  }

  /** 测试匹配的搜索区域：下拉框手动选择优先，否则按模板名自动识别
   *  （#x1_y1_x2_y2 → 对应矩形区域；#l/#r/... → 对应半区；无 → 全屏） */
  function templateRegionPixels(name) {
    // 实际舞台画面尺寸优先：虚拟屏分辨率/方向会被游戏改变，设备配置里的
    // width/height 可能过期（随舞台来源切换：live = 视频，media = 媒体画面）
    const el = surface()
    const vw = el?.videoWidth || current.value?.width || 1920
    const vh = el?.videoHeight || current.value?.height || 1080
    if (testRegion.value) return regionCodePixels(testRegion.value, vw, vh)
    const nums = parseTplRegion(name)
    if (nums) {
      const x = Math.round(nums[0] * vw)
      const y = Math.round(nums[1] * vh)
      const w = Math.round((nums[2] - nums[0]) * vw)
      const h = Math.round((nums[3] - nums[1]) * vh)
      return [x, y, w, h]
    }
    const code = parseTplRegionCode(name)
    if (code) return regionCodePixels(code, vw, vh)
    return null
  }

  async function testMatch(name, { stepSemantics = false } = {}) {
    if (!connected.value) return toast('请先连接设备', 'error')
    if (hitTimer) { clearTimeout(hitTimer); hitTimer = null }
    showHit.value = false
    try {
      // 模板列表测试允许用户用测试区覆盖；步骤预览不覆盖，交给服务端按引擎规则
      // 从实际模板文件名解析 #区域（短名也由服务端统一消歧）。
      const region = stepSemantics ? undefined : templateRegionPixels(name)
      const threshold = stepSemantics ? editorMatchThreshold() : (Number(testThreshold.value) || 0.8)
      const r = await api.testTemplate(name, store.deviceId, threshold, region, packageId.value)
      if (r.hit) {
        hit.x = r.x; hit.y = r.y; hit.w = r.width; hit.h = r.height
        hitLabel.value = `${name} ${r.score.toFixed(2)}`
        hitMiss.value = false
        showHit.value = true
        // 匹配框只展示 3 秒，避免一直留在画面上
        hitTimer = setTimeout(() => { showHit.value = false }, 3000)
        toast(`匹配成功：${name} 置信度 ${r.score.toFixed(2)}`, 'success')
      } else {
        // 未命中也画框：显示本次搜索区域（与引擎 miss 可视化同语义，便于发现区域配错）
        const el = surface()
        const vw2 = el?.videoWidth || current.value?.width || 1920
        const vh2 = el?.videoHeight || current.value?.height || 1080
        const [rx, ry, rw2, rh2] = region || r.region || [0, 0, vw2, vh2]
        hit.x = rx; hit.y = ry; hit.w = rw2; hit.h = rh2
        hitLabel.value = `${name} 未命中`
        hitMiss.value = true
        showHit.value = true
        hitTimer = setTimeout(() => { showHit.value = false }, 3000)
        toast(`未找到：${name}`, 'warn')
      }
    } catch (e) {
      toast('匹配失败：' + e.message, 'error')
    }
  }

  // 视觉组件只接收这两个上下文对象；所有状态、动作和清理由 Console/本 composable 统一持有。
  function onCropMounted({ canvas, section }) {
    cropCanvas.value = canvas
    cropSec.value = section
    if (crop.active) {
      renderCropFrame()
      refreshCropPreview()
    }
  }

  function setRenameInputEl(el) { renameInputEl = el }

  // 卸载清理：测试匹配命中框定时器、进行中的 bridge 框选/单元格回填请求按取消收尾
  onUnmounted(() => {
    if (hitTimer) { clearTimeout(hitTimer); hitTimer = null }
    if (bridgeRegionResolve) { bridgeRegionResolve(null); bridgeRegionResolve = null }
    if (cellCaptureResolve) { cellCaptureResolve(null); cellCaptureResolve = null }
    cancelCellPick()
  })

  // 框选按钮可用性随舞台来源：实时已连接 或 视频来源画面就绪（离线可框选/裁切）
  const stageReady = computed(() => (stage ? stage.ready() : !!connected.value))

  const templateCaptureContext = {
    packageId, crop, testThreshold, testRegion, tplSearch,
    picking, connected, stageReady, togglePick, templates, confirmDelTpl, renaming, onTplRowClick, onTplThumbClick,
    tplThumbUrl, onTplNameClick, setRenameInputEl, renameVal, confirmRename, cancelRename, startRename,
    onTplDeleteClick, onTplMatchClick, onTplUpload, tplShortName, tplRegionBadge, cropSize, cropZoomPct,
    cropMouseDown, cropMouseMove, cropMouseUp, cropMouseLeave, cropWheel, saveTemplate, overwriteTemplate, backToCrop, cancelCrop,
    repick, saving, viewTpl, closeTplView, replaceTemplateImage,
  }

  return {
    // 状态
    picking, selecting, selStart, selEnd, showHit, hitLabel, hitMiss, hitStyle, selStyle,
    testThreshold, testRegion, tplSearch, templates, templateNames, stageReady,
    viewTpl, confirmDelTpl, renaming, renameVal, crop, cropSize, cropZoomPct, saving,
    loupe, loupeCanvas, cellPick,
    // 视图挂载
    onLoupeMounted(el) { loupeCanvas.value = el },
    onCropMounted, setRenameInputEl,
    // 框选/裁切/模板操作
    togglePick, openCrop, selToDeviceRect, hideLoupe, updateLoupe, toDeviceCoord, deviceRectStyle,
    cropMouseDown, cropMouseMove, cropMouseUp, cropMouseLeave, cropWheel,
    saveTemplate, overwriteTemplate, backToCrop, cancelCrop, repick,
    onTplRowClick, onTplThumbClick, onTplNameClick, confirmRename, cancelRename, startRename,
    onTplDeleteClick, onTplMatchClick, onTplUpload, replaceTemplateImage,
    tplShortName, tplRegionBadge, tplThumbUrl, testMatch,
    // bridge/单元格取值工具
    selectRegionForBridge, beginCellPick, cancelCellPick, finishCellPick,
    bridgeRegionSelected, finishBridgeRegionSelect, cancelBridgeRegionSelect,
    closeTplView,
    // 上下文对象
    refreshTemplatesData,
    templateCaptureContext,
  }
}
