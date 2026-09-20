import type { ParamDecl } from './model'

/** 离线预览的中文名称；与原生目录 name 参数默认值由契约测试校验。 */
export const NATIVE_CALL_NAMES: Record<string, string> = {
  tap: '点击', swipe: '滑动', key: '按键', input_text: '输入文本',
  launch: '启动应用', stop_app: '停止应用', sleep: '等待', log: '日志',
  find: '查找模板', wait_find: '等待模板出现', tap_template: '点击模板',
  wait_disappear: '等待模板消失', eq: '等于', ne: '不等于',
  gt: '大于', ge: '大于等于', lt: '小于', le: '小于等于',
}

export function functionCallParams(fn: { name: string; description?: string; params?: ParamDecl[] }): ParamDecl[] {
  const params = fn.params ?? []
  if (params.some(p => p.name === 'name')) return params
  return [...params, {
    name: 'name', type: 'string', required: false,
    default: fn.description?.trim() || NATIVE_CALL_NAMES[fn.name] || fn.name,
    desc: '可视化显示名称',
  }]
}
