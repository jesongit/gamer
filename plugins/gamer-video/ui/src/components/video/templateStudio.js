/**
 * 模板工作台（TemplateStudio，Phase 7 §10.2）纯函数助手。
 *
 * **坐标系决策（与现有 live 模板坐标系一致）**：
 * - 模板存储空间 = **匹配器的屏幕帧像素空间**——live = 设备显示像素，media =
 *   服务端抽帧 PNG 的 oriented 展示像素（ffmpeg autorotate 后）。搜索区域以
 *   0~1 相对坐标进模板文件名 `#x1_y1_x2_y2`（×1000，3 位），模板 PNG 从该帧
 *   像素直接裁出——与 useConsoleTemplates 的 live 框选完全同空间、同规则。
 * - 工作台校准（calibration.js 的 reference 空间）是制作侧视图坐标：选框在
 *   oriented 帧上直接进行（存储空间即帧空间，恒等）；展示给用户的「参考坐标」
 *   经 `orientedToReference`（content 裁剪 + 等比缩放 + letterbox 补边）换算，
 *   便于与脚本/标记的 reference 坐标对账。identity 校准下两空间重合。
 * - 搜索区与模板取自**同一帧像素空间**的相对坐标 → 不同参考尺寸的帧上做
 *   离线测试时区域随帧尺寸等比换算（不缩放模板而忘搜索区：vision REST 的
 *   region 也按帧像素显式换算后下发）。
 *
 * 全部纯函数，无 Vue/DOM 依赖。
 */

import { contentRectOrDefault, contentToReferenceTransform, identityCalibration } from './calibration'

/**
 * 归一化跨组件传递的确定帧身份。
 *
 * `frameIndex` 与 `ptsUs` 至少要有一个；进入模板制作时通常两者都有，
 * 但这里保留 pts-only 兼容路径，避免把合法的服务端时间寻址误判成“帧 0”。
 * 不接受负数、浮点或超出 JS 安全整数范围的值。
 */
export function normalizeFrameIdentity(frame, fallbackMediaId = '') {
  const mediaId = String(frame?.mediaId ?? frame?.media_id ?? fallbackMediaId ?? '').trim()
  if (!mediaId) return null
  const rawIndex = frame?.frameIndex ?? frame?.frame_index
  const rawPts = frame?.ptsUs ?? frame?.pts_us
  const index = rawIndex === null || rawIndex === undefined || rawIndex === '' ? null : Number(rawIndex)
  const ptsUs = rawPts === null || rawPts === undefined || rawPts === '' ? null : Number(rawPts)
  const hasIndex = index !== null && Number.isSafeInteger(index) && index >= 0
  const hasPts = ptsUs !== null && Number.isSafeInteger(ptsUs) && ptsUs >= 0
  if (!hasIndex && !hasPts) return null
  return {
    mediaId,
    frameIndex: hasIndex ? index : null,
    ptsUs: hasPts ? ptsUs : null,
  }
}

/** 稳定帧身份键：用于冻结制作会话及防止旧图片/请求回写。 */
export function frameIdentityKey(frame) {
  const normalized = normalizeFrameIdentity(frame)
  return normalized
    ? `${normalized.mediaId}|${normalized.frameIndex ?? ''}|${normalized.ptsUs ?? ''}`
    : ''
}


/** 像素矩形（帧空间）→ 相对区域 [x1,y1,x2,y2]（0..=1，clamp；面积过小返回 null）。 */
export function regionFromRect(rect, width, height) {
  const w = Math.max(0, Number(width) || 0)
  const h = Math.max(0, Number(height) || 0)
  if (!w || !h) return null
  const x = Number(rect?.x)
  const y = Number(rect?.y)
  const rectWidth = Number(rect?.w)
  const rectHeight = Number(rect?.h)
  if (![x, y, rectWidth, rectHeight].every(Number.isFinite) || rectWidth <= 0 || rectHeight <= 0) return null
  const x1 = Math.min(Math.max(x, 0), w) / w
  const y1 = Math.min(Math.max(y, 0), h) / h
  const x2 = Math.min(Math.max(x + rectWidth, 0), w) / w
  const y2 = Math.min(Math.max(y + rectHeight, 0), h) / h
  // 最小有效尺寸：模板/区域至少 4×1000 分度中的 8 个千分位（防误触空选）
  if (x2 <= x1 || y2 <= y1 || x2 - x1 < 0.008 || y2 - y1 < 0.008) return null
  return [x1, y1, x2, y2]
}

/** 相对区域 → 帧像素矩形 {x,y,w,h}（离线测试的 region 参数换算）。 */
export function regionToPixelRect(region, width, height) {
  if (!Array.isArray(region) || region.length !== 4) return null
  const [rawX1, rawY1, rawX2, rawY2] = region.map(Number)
  const [x1, y1, x2, y2] = [rawX1, rawY1, rawX2, rawY2].map(value => Math.max(0, Math.min(1, value)))
  if (![rawX1, rawY1, rawX2, rawY2].every(Number.isFinite)) return null
  if (x2 <= x1 || y2 <= y1) return null // 退化区域（零面积）无效
  const pixelWidth = Math.max(0, Number(width) || 0)
  const pixelHeight = Math.max(0, Number(height) || 0)
  if (!pixelWidth || !pixelHeight) return null
  return {
    x: Math.round(x1 * pixelWidth),
    y: Math.round(y1 * pixelHeight),
    w: Math.max(1, Math.round((x2 - x1) * pixelWidth)),
    h: Math.max(1, Math.round((y2 - y1) * pixelHeight)),
  }
}

/**
 * oriented（帧 PNG 像素）→ reference（制作参考坐标，连续量）。
 * calibration.js 提供 encodedToReference（encoded 起点）；抽帧 PNG 已
 * autorotate（oriented），故此处从 oriented 段接入：content 裁剪 + 单一等比
 * 因子 + letterbox 居中补边。
 */
export function orientedToReference(point, calibration, encodedSize) {
  const safeSize = {
    width: Math.max(1, Number(encodedSize?.width) || 1),
    height: Math.max(1, Number(encodedSize?.height) || 1),
  }
  const safeCalibration = calibration || identityCalibration(safeSize)
  const rect = contentRectOrDefault(safeCalibration, safeSize)
  const { scale, padX, padY } = contentToReferenceTransform(safeCalibration, safeSize)
  return {
    x: (Number(point?.x || 0) - rect.x) * scale + padX,
    y: (Number(point?.y || 0) - rect.y) * scale + padY,
  }
}

/** 命中/搜索区像素矩形 → 显示样式（object-fit: contain 的 letterbox 映射）。
 *  imgRect = 图像元素的 bounding rect；natural = 帧像素尺寸。 */
export function pixelRectToStyle(rect, imgRect, naturalWidth, naturalHeight) {
  if (!rect || !imgRect || !(Number(imgRect.width) > 0) || !(Number(imgRect.height) > 0)) return null
  const nw = Math.max(1, Number(naturalWidth) || 1)
  const nh = Math.max(1, Number(naturalHeight) || 1)
  const ratio = Math.min(imgRect.width / nw, imgRect.height / nh)
  const w = rect.w * ratio
  const h = rect.h * ratio
  const x = rect.x * ratio + (imgRect.width - nw * ratio) / 2
  const y = rect.y * ratio + (imgRect.height - nh * ratio) / 2
  return { left: `${x}px`, top: `${y}px`, width: `${w}px`, height: `${h}px` }
}

/** 鼠标事件 → 帧像素坐标（contain 映射；越界 clamp 到画面内）。 */
export function eventToImagePoint(event, imgRect, naturalWidth, naturalHeight) {
  if (!imgRect || !(Number(imgRect.width) > 0) || !(Number(imgRect.height) > 0)) return { x: 0, y: 0 }
  const nw = Math.max(1, Number(naturalWidth) || 1)
  const nh = Math.max(1, Number(naturalHeight) || 1)
  const ratio = Math.min(imgRect.width / nw, imgRect.height / nh)
  const offsetX = (imgRect.width - nw * ratio) / 2
  const offsetY = (imgRect.height - nh * ratio) / 2
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v))
  return {
    x: clamp((event.clientX - imgRect.left - offsetX) / ratio, 0, nw),
    y: clamp((event.clientY - imgRect.top - offsetY) / ratio, 0, nh),
  }
}

/** 帧身份展示标签（回查信息）。 */
export function describeFrameIdentity(frame) {
  if (!frame?.mediaId) return ''
  const at = frame.frameIndex !== undefined && frame.frameIndex !== null
    ? `帧 #${frame.frameIndex}`
    : `pts_us=${frame.ptsUs ?? 0}`
  return `${frame.mediaId} · ${at}`
}
