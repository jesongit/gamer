/**
 * 录制裁切数学（plan §11.6 自动模板 A / 手动模板 M / 搜索区域 S）。
 *
 * - 全部矩形使用 {x,y,w,h} 原始像素，以冻结帧的原始尺寸为基准
 *   （不涉及浏览器 CSS 尺寸或裁切预览缩放尺寸，坐标一致性由调用方保证传入原始像素）；
 * - 输入输出均裁剪到帧边界；
 * - 居中取整统一用 Math.round（.5 向 +∞ 取整），保证横竖屏/任意坐标下结果可复现。
 *
 * 语义（plan §11.6 规则）：
 * - 无手动裁切：最终模板 = A；S = 以 A 为中心的 100×100 裁剪到边界；
 * - 有手动裁切：最终模板 = M；S = union(A, M) 四周各扩 25px 后裁剪到边界。
 */

/** @typedef {{x: number, y: number, w: number, h: number}} Rect */

/** 自动模板边长（plan §11.4：默认 50×50）。 */
export const AUTO_TEMPLATE_SIZE = 50
/** 无手动裁切时的搜索区域边长（以 A 为中心）。 */
export const AUTO_SEARCH_SIZE = 100
/** 有手动裁切时 union(A, M) 每侧外扩像素。 */
export const MANUAL_SEARCH_EXPAND = 25

/**
 * 把矩形裁剪到帧边界内：负起点平移归零（宽高同步收缩）、右/下越界收缩、
 * 完全越出右/下边界的钳到边上、宽高钳到 ≥0。
 * @param {Rect | null | undefined} rect
 * @returns {Rect | null | undefined}
 */
export function clampRect(rect, frameW, frameH) {
  if (!rect) return rect
  let { x, y, w, h } = rect
  if (x < 0) {
    w += x
    x = 0
  }
  if (y < 0) {
    h += y
    y = 0
  }
  if (x > frameW) x = frameW
  if (y > frameH) y = frameH
  if (x + w > frameW) w = frameW - x
  if (y + h > frameH) h = frameH - y
  return { x, y, w: Math.max(0, w), h: Math.max(0, h) }
}

/** 两矩形的并集（不裁剪边界，调用方按需 clamp）。 */
export function unionRect(a, b) {
  const x1 = Math.min(a.x, b.x)
  const y1 = Math.min(a.y, b.y)
  const x2 = Math.max(a.x + a.w, b.x + b.w)
  const y2 = Math.max(a.y + a.h, b.y + b.h)
  return { x: x1, y: y1, w: x2 - x1, h: y2 - y1 }
}

/** 矩形中心（帧原始像素，可能为 .5 半像素）。 */
export function rectCenterPx(rect) {
  return [rect.x + rect.w / 2, rect.y + rect.h / 2]
}

/** 内部：以 (cx, cy) 为中心的 w×h 矩形，中心钳进帧内后裁剪到边界。 */
function centeredRect(cx, cy, w, h, frameW, frameH) {
  const px = Math.min(Math.max(cx, 0), frameW)
  const py = Math.min(Math.max(cy, 0), frameH)
  return clampRect({ x: Math.round(px - w / 2), y: Math.round(py - h / 2), w, h }, frameW, frameH)
}

/**
 * A：以 (cx, cy) 为中心的 size×size 自动模板矩形，裁剪到帧边界。
 * 中心点本身越界时先钳进帧内（按下点理论上已在帧内，防御性处理）。
 */
export function autoTemplateRect(frameW, frameH, cx, cy, size = AUTO_TEMPLATE_SIZE) {
  return centeredRect(cx, cy, size, size, frameW, frameH)
}

/**
 * S（无手动裁切）：以 A 为中心的 100×100，裁剪到帧边界。
 * 二次裁切面板至少加载 S 对应范围，否则无法把模板移动到自动区域之外。
 */
export function searchRectAuto(aRect, frameW, frameH) {
  const a = clampRect(aRect, frameW, frameH)
  const [cx, cy] = rectCenterPx(a)
  return centeredRect(cx, cy, AUTO_SEARCH_SIZE, AUTO_SEARCH_SIZE, frameW, frameH)
}

/**
 * S（有手动裁切）：union(A, M) 四周各扩 25px，裁剪到帧边界。
 * @param {Rect} aRect 自动模板区 A
 * @param {Rect} mRect 用户确认的手动模板区 M
 */
export function searchRectManual(aRect, mRect, frameW, frameH) {
  const u = unionRect(clampRect(aRect, frameW, frameH), clampRect(mRect, frameW, frameH))
  const e = MANUAL_SEARCH_EXPAND
  return clampRect({ x: u.x - e, y: u.y - e, w: u.w + e * 2, h: u.h + e * 2 }, frameW, frameH)
}

/**
 * 原始像素 → 0~1 相对坐标（越界输入钳到帧内，输出恒在 0~1）。
 * @returns {[number, number]}
 */
export function toRelative(px, py, frameW, frameH) {
  const x = Math.min(Math.max(px, 0), frameW)
  const y = Math.min(Math.max(py, 0), frameH)
  return [frameW > 0 ? x / frameW : 0, frameH > 0 ? y / frameH : 0]
}
