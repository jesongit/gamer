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

import { contentRectOrDefault, contentToReferenceTransform } from './calibration'


/** 像素矩形（帧空间）→ 相对区域 [x1,y1,x2,y2]（0..=1，clamp；面积过小返回 null）。 */
export function regionFromRect(rect, width, height) {
  const w = Math.max(0, Number(width) || 0)
  const h = Math.max(0, Number(height) || 0)
  if (!w || !h) return null
  const x1 = Math.min(Math.max(rect.x, 0), w) / w
  const y1 = Math.min(Math.max(rect.y, 0), h) / h
  const x2 = Math.min(Math.max(rect.x + rect.w, 0), w) / w
  const y2 = Math.min(Math.max(rect.y + rect.h, 0), h) / h
  // 最小有效尺寸：模板/区域至少 4×1000 分度中的 8 个千分位（防误触空选）
  if (x2 - x1 < 0.008 || y2 - y1 < 0.008) return null
  return [x1, y1, x2, y2]
}

/** 相对区域 → 帧像素矩形 {x,y,w,h}（离线测试的 region 参数换算）。 */
export function regionToPixelRect(region, width, height) {
  if (!Array.isArray(region) || region.length !== 4) return null
  const [x1, y1, x2, y2] = region.map(Number)
  if (![x1, y1, x2, y2].every(Number.isFinite)) return null
  if (x2 <= x1 || y2 <= y1) return null // 退化区域（零面积）无效
  return {
    x: Math.round(x1 * width),
    y: Math.round(y1 * height),
    w: Math.max(1, Math.round((x2 - x1) * width)),
    h: Math.max(1, Math.round((y2 - y1) * height)),
  }
}

/**
 * oriented（帧 PNG 像素）→ reference（制作参考坐标，连续量）。
 * calibration.js 提供 encodedToReference（encoded 起点）；抽帧 PNG 已
 * autorotate（oriented），故此处从 oriented 段接入：content 裁剪 + 单一等比
 * 因子 + letterbox 居中补边。
 */
export function orientedToReference(point, calibration, encodedSize) {
  const rect = contentRectOrDefault(calibration, encodedSize)
  const { scale, padX, padY } = contentToReferenceTransform(calibration, encodedSize)
  return {
    x: (Number(point?.x || 0) - rect.x) * scale + padX,
    y: (Number(point?.y || 0) - rect.y) * scale + padY,
  }
}

/** 命中/搜索区像素矩形 → 显示样式（object-fit: contain 的 letterbox 映射）。
 *  imgRect = 图像元素的 bounding rect；natural = 帧像素尺寸。 */
export function pixelRectToStyle(rect, imgRect, naturalWidth, naturalHeight) {
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

