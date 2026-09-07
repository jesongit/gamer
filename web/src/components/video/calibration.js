/**
 * 校准（calibration）统一坐标变换（Phase 6，计划 §9.2 / 原计划 §6.3）。
 *
 * 四个坐标空间（与 server/src/extensions/video/project.rs 的 schema 注释同源）：
 *   encoded（文件原始像素）→ oriented（应用旋转 + 像素比例）→
 *   content（去除黑边/录屏边框的有效画面区域）→ reference（制作参考分辨率）。
 *
 * 硬规则：**不静默非等比拉伸**。content → reference 只有单一等比缩放因子
 * （min 比例），纵横比不一致时居中补边（letterbox），绝不各自拉伸两轴。
 * 校准值变化必须递增 `version`（calibration_version）：标记/模板区域/事件坐标
 * 记录其校准版本，版本不一致 = 旧数据标脏提示重新确认，不悄悄变形。
 *
 * 坐标为连续量（非像素索引），旋转以顺时针展示方向为正。全部纯函数，无 Vue 依赖。
 */

export const ROTATIONS = [0, 90, 180, 270]

/** 方像素默认比例。 */
export function identityPixelAspect() {
  return { num: 1, den: 1 }
}

/** 恒等校准（不做任何修正）：参考尺寸 = oriented 尺寸。 */
export function identityCalibration(orientedSize) {
  return {
    version: 1,
    rotation: 0,
    pixel_aspect: identityPixelAspect(),
    content_rect: null,
    reference_size: { width: Math.max(1, Math.round(orientedSize?.width || 1)), height: Math.max(1, Math.round(orientedSize?.height || 1)) },
  }
}

/** 旋转后的 oriented 尺寸（90/270 交换宽高；不乘像素比例——比例属连续变换）。 */
export function orientedSize(encodedSize, rotation) {
  const width = Math.max(0, Number(encodedSize?.width) || 0)
  const height = Math.max(0, Number(encodedSize?.height) || 0)
  return rotation === 90 || rotation === 270
    ? { width: height, height: width }
    : { width, height }
}

/** 像素比例修正后的 oriented 画布（连续量：宽乘 num/den）。 */
export function orientedCanvas(encodedSize, calibration) {
  const size = orientedSize(encodedSize, calibration.rotation)
  const { num, den } = normalizedPixelAspect(calibration.pixel_aspect)
  return { width: size.width * (num / den), height: size.height }
}

function normalizedPixelAspect(pixelAspect) {
  const num = Math.max(1, Math.round(Number(pixelAspect?.num) || 1))
  const den = Math.max(1, Math.round(Number(pixelAspect?.den) || 1))
  return { num, den }
}

/** 有效画面区域（oriented 域）；未校准 = 全画面。 */
export function contentRectOrDefault(calibration, encodedSize) {
  const canvas = orientedCanvas(encodedSize, calibration)
  const rect = calibration.content_rect
  if (rect && rect.w > 0 && rect.h > 0) {
    return { x: Number(rect.x) || 0, y: Number(rect.y) || 0, w: Number(rect.w), h: Number(rect.h) }
  }
  return { x: 0, y: 0, w: canvas.width, h: canvas.height }
}

/** content → reference 的等比缩放因子（min）与居中补边量。
 *  单一因子 = 结构上不可能非等比拉伸；纵横比差以 letterbox 吸收。 */
export function contentToReferenceTransform(calibration, encodedSize) {
  const rect = contentRectOrDefault(calibration, encodedSize)
  const ref = calibration.reference_size
  const scale = Math.min((Number(ref?.width) || 1) / rect.w, (Number(ref?.height) || 1) / rect.h)
  return {
    scale,
    padX: ((Number(ref?.width) || 1) - rect.w * scale) / 2,
    padY: ((Number(ref?.height) || 1) - rect.h * scale) / 2,
  }
}

/** encoded → oriented（旋转 + 像素比例，连续坐标）。
 *  顺时针旋转：90° → (ox, oy) = (eh − ey, ex)；180° → (ew − ex, eh − ey)；270° → (ox, oy) = (ey, ew − ex)。 */
export function encodedToOriented(point, calibration, encodedSize) {
  const ew = Math.max(0, Number(encodedSize?.width) || 0)
  const eh = Math.max(0, Number(encodedSize?.height) || 0)
  const x = Number(point?.x) || 0
  const y = Number(point?.y) || 0
  let ox = x
  let oy = y
  if (calibration.rotation === 90) { ox = eh - y; oy = x } else if (calibration.rotation === 180) { ox = ew - x; oy = eh - y } else if (calibration.rotation === 270) { ox = y; oy = ew - x }
  const { num, den } = normalizedPixelAspect(calibration.pixel_aspect)
  return { x: ox * (num / den), y: oy }
}

/** oriented → encoded（encodedToOriented 的逆变换）。 */
export function orientedToEncoded(point, calibration, encodedSize) {
  const ew = Math.max(0, Number(encodedSize?.width) || 0)
  const eh = Math.max(0, Number(encodedSize?.height) || 0)
  const { num, den } = normalizedPixelAspect(calibration.pixel_aspect)
  const ox = (Number(point?.x) || 0) * (den / num)
  const oy = Number(point?.y) || 0
  if (calibration.rotation === 90) return { x: oy, y: eh - ox }
  if (calibration.rotation === 180) return { x: ew - ox, y: eh - oy }
  if (calibration.rotation === 270) return { x: ew - oy, y: ox }
  return { x: ox, y: oy }
}

/** encoded（原视频帧像素）→ reference（制作坐标）。模板裁切/脚本坐标换算的唯一入口。 */
export function encodedToReference(point, calibration, encodedSize) {
  const oriented = encodedToOriented(point, calibration, encodedSize)
  const rect = contentRectOrDefault(calibration, encodedSize)
  const { scale, padX, padY } = contentToReferenceTransform(calibration, encodedSize)
  return {
    x: (oriented.x - rect.x) * scale + padX,
    y: (oriented.y - rect.y) * scale + padY,
  }
}

/** reference → encoded（encodedToReference 的逆；命中框叠加回放用）。 */
export function referenceToEncoded(point, calibration, encodedSize) {
  const rect = contentRectOrDefault(calibration, encodedSize)
  const { scale, padX, padY } = contentToReferenceTransform(calibration, encodedSize)
  const ox = (Number(point?.x) - padX) / scale + rect.x
  const oy = (Number(point?.y) - padY) / scale + rect.y
  return orientedToEncoded({ x: ox, y: oy }, calibration, encodedSize)
}

/** 结构化校准诊断（与服务端 project.rs 校验规则镜像）。 */
export function calibrationDiagnostics(calibration) {
  const out = []
  const version = Math.round(Number(calibration?.version))
  if (!Number.isFinite(version) || version < 1) out.push({ code: 'calibration.version', message: '校准版本必须 ≥ 1' })
  if (!ROTATIONS.includes(Number(calibration?.rotation))) out.push({ code: 'calibration.rotation', message: '旋转只支持 0|90|180|270' })
  const num = Math.round(Number(calibration?.pixel_aspect?.num))
  const den = Math.round(Number(calibration?.pixel_aspect?.den))
  if (!Number.isFinite(num) || num < 1 || !Number.isFinite(den) || den < 1) out.push({ code: 'calibration.pixel_aspect', message: '像素比例 num/den 必须 ≥ 1' })
  const rect = calibration?.content_rect
  if (rect !== null && rect !== undefined) {
    if (!(Number(rect.w) >= 1 && Number(rect.h) >= 1 && Number(rect.x) >= 0 && Number(rect.y) >= 0)) {
      out.push({ code: 'calibration.content_rect', message: '有效画面区域宽高必须 ≥ 1 且坐标非负' })
    }
  }
  if (!(Number(calibration?.reference_size?.width) >= 1 && Number(calibration?.reference_size?.height) >= 1)) {
    out.push({ code: 'calibration.reference_size', message: '参考分辨率宽高必须 ≥ 1' })
  }
  return out
}

/** 校准显示描述（UI 摘要）。 */
export function describeCalibration(calibration, encodedSize) {
  const rect = contentRectOrDefault(calibration, encodedSize)
  const { num, den } = normalizedPixelAspect(calibration.pixel_aspect)
  const parts = [`旋转 ${calibration.rotation}°`, `参考 ${calibration.reference_size.width}×${calibration.reference_size.height}`]
  if (num !== 1 || den !== 1) parts.push(`像素比 ${num}:${den}`)
  const canvas = orientedCanvas(encodedSize, calibration)
  const cropped = rect.w < canvas.width - 0.5 || rect.h < canvas.height - 0.5
  if (cropped) parts.push(`裁剪 ${Math.round(rect.w)}×${Math.round(rect.h)}@${Math.round(rect.x)},${Math.round(rect.y)}`)
  return parts.join(' · ')
}
