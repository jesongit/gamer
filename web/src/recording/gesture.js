/**
 * 录制手势分类（plan §11.4 点击录制 / §11.5 滑动录制的纯判定部分）。
 *
 * 录制是透传模式：触摸事件始终立即发送给设备，本模块只在抬起时把
 * 「按下→抬起」轨迹归类为 click / swipe / longpress，供录制服务决定生成
 * find 语义草稿、match→swipe 语义草稿，还是「未支持手势」失败草稿。
 */

/** 点击判定最大时长（毫秒，含边界）：位移在阈值内且 ≤600ms 视为点击。 */
export const CLICK_MAX_MS = 600

/**
 * 移动阈值（帧像素）：短边的 0.5%，最小 8px。
 * 高分辨率下按比例放宽，小分辨率下保底 8px，避免手抖被误判成滑动。
 */
export function moveThresholdPx(frameW, frameH) {
  return Math.max(8, Math.min(frameW, frameH) * 0.005)
}

/**
 * 按位移（帧原始像素）与时长分类一次按下-抬起：
 * - 位移大于 moveThresholdPx → 'swipe'（与时长无关，慢速拖动也是滑动）；
 * - 位移在阈值内（含等值）且 durationMs ≤ CLICK_MAX_MS → 'click'；
 * - 位移在阈值内但 > CLICK_MAX_MS → 'longpress'
 *   （录制不支持长按，调用方生成失败草稿，不静默转成点击）。
 *
 * @param {{dxPx: number, dyPx: number, durationMs: number, frameW: number, frameH: number}} g
 * @returns {'click'|'swipe'|'longpress'}
 */
export function classifyGesture({ dxPx, dyPx, durationMs, frameW, frameH }) {
  const threshold = moveThresholdPx(frameW, frameH)
  const moved = Math.sqrt(dxPx * dxPx + dyPx * dyPx)
  if (moved > threshold) return 'swipe'
  return durationMs <= CLICK_MAX_MS ? 'click' : 'longpress'
}
