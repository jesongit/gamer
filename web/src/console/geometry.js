// Console 的设备坐标与 letterbox 映射纯函数。
// 这些函数不依赖 Vue 或 DOM，便于在没有浏览器/Android 设备时直接回归。

export const DEFAULT_VIDEO_WIDTH = 1920
export const DEFAULT_VIDEO_HEIGHT = 1080

function containMetrics(rect, videoWidth, videoHeight) {
  const vw = videoWidth || DEFAULT_VIDEO_WIDTH
  const vh = videoHeight || DEFAULT_VIDEO_HEIGHT
  const ratio = Math.min(rect.width / vw, rect.height / vh)
  return {
    vw,
    vh,
    ratio,
    dispW: vw * ratio,
    dispH: vh * ratio,
  }
}

/** object-fit: contain 下把浏览器客户区坐标映射为设备像素坐标。 */
export function toDeviceCoord(clientX, clientY, rect, videoWidth, videoHeight) {
  const { vw, vh, ratio, dispW, dispH } = containMetrics(rect, videoWidth, videoHeight)
  const offX = (rect.width - dispW) / 2
  const offY = (rect.height - dispH) / 2
  const x = Math.round((clientX - rect.left - offX) / dispW * vw)
  const y = Math.round((clientY - rect.top - offY) / dispH * vh)
  return { x: Math.max(0, Math.min(vw, x)), y: Math.max(0, Math.min(vh, y)) }
}

/** 把设备像素矩形转换为叠加层使用的 CSS 样式。 */
export function deviceRectStyle(x, y, w, h, rect, videoWidth, videoHeight) {
  const { vw, vh, ratio } = containMetrics(rect, videoWidth, videoHeight)
  return {
    left: (x * ratio) + (rect.width - vw * ratio) / 2 + 'px',
    top: (y * ratio) + (rect.height - vh * ratio) / 2 + 'px',
    width: w * ratio + 'px',
    height: h * ratio + 'px',
  }
}

/** 把容器内的选择框转换为设备像素矩形，并裁掉 letterbox 黑边。 */
export function selectionToDeviceRect(start, end, rect, videoWidth, videoHeight) {
  const { vw, vh, ratio, dispW, dispH } = containMetrics(rect, videoWidth, videoHeight)
  const offX = (rect.width - dispW) / 2
  const offY = (rect.height - dispH) / 2
  const toDevice = point => ({
    x: (point.x - offX) / dispW * vw,
    y: (point.y - offY) / dispH * vh,
  })
  const p1 = toDevice(start)
  const p2 = toDevice(end)
  const x = Math.round(Math.min(p1.x, p2.x))
  const y = Math.round(Math.min(p1.y, p2.y))
  const w = Math.round(Math.abs(p2.x - p1.x))
  const h = Math.round(Math.abs(p2.y - p1.y))
  const cx = Math.max(0, Math.min(vw, x))
  const cy = Math.max(0, Math.min(vh, y))
  return { x: cx, y: cy, w: Math.min(w, vw - cx), h: Math.min(h, vh - cy) }
}

export function randomTemplateBase() {
  return 'tpl_' + Math.random().toString(36).slice(2, 8)
}

/** 生成带 ×1000 相对坐标区域后缀的默认模板名。 */
export function defaultTemplateName(rect, videoWidth, videoHeight, base = randomTemplateBase()) {
  const vw = videoWidth || DEFAULT_VIDEO_WIDTH
  const vh = videoHeight || DEFAULT_VIDEO_HEIGHT
  const toInt3 = v => String(Math.min(999, Math.round(v * 1000))).padStart(3, '0')
  const x1 = toInt3(rect.x / vw)
  const y1 = toInt3(rect.y / vh)
  const x2 = toInt3((rect.x + rect.w) / vw)
  const y2 = toInt3((rect.y + rect.h) / vh)
  return `${base}#${x1}_${y1}_${x2}_${y2}`
}
