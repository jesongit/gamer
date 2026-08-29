/**
 * 录制模板命名（plan §11.7 命名和引用）。
 *
 * 前端只生成「默认短名建议」和搜索区域展示串；序号仅为前端建议值，
 * 服务端在当前应用分区内原子检查冲突；完整文件名（# 后缀拼接）由服务端生成。
 */

/**
 * 默认短名：record_<kind>_YYYYMMDD_NNN.png（NNN 三位零填充，seq 从 1 起；
 * seq ≥1000 时自然扩位）。日期取本地年月日。
 *
 * @param {'click'|'swipe'} kind
 * @param {Date} date
 * @param {number} seq
 */
export function defaultShortName(kind, date, seq) {
  if (kind !== 'click' && kind !== 'swipe') throw new Error(`未知录制类型：${kind}`)
  if (!date || typeof date.getFullYear !== 'function') throw new Error('date 必须是 Date')
  const y = date.getFullYear()
  const m = String(date.getMonth() + 1).padStart(2, '0')
  const d = String(date.getDate()).padStart(2, '0')
  const n = String(seq).padStart(3, '0')
  return `record_${kind}_${y}${m}${d}_${n}.png`
}

/**
 * 短名合法性：[A-Za-z0-9_-]+\.png。
 * 不允许 `#`（`#` 是服务端搜索区域元数据分隔符，用户短名不得携带）。
 * @param {unknown} name
 */
export function isValidShortName(name) {
  return typeof name === 'string' && /^[A-Za-z0-9_-]+\.png$/.test(name)
}

/**
 * 搜索区域展示/校验串：`#x_y_w_h`（半区码同源，坐标为冻结帧原始像素）。
 * 仅供 UI 预览与人工核对；拼接完整文件名由服务端完成，前端不落盘拼接。
 * @param {import('./crop').Rect} searchRect
 */
export function buildSearchSuffix(searchRect) {
  const r = searchRect
  return `#${Math.round(r.x)}_${Math.round(r.y)}_${Math.round(r.w)}_${Math.round(r.h)}`
}
