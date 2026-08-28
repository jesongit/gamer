export function formatScreenSummary(device) {
  if (!device) return '—'
  if (device.screen_mode === 'virtual') {
    const res = device.vd_res || '1920x1080'
    const dpi = device.vd_dpi ? ` @${device.vd_dpi}dpi` : ' · DPI 自动'
    return `🖥️ 虚拟屏 · ${res}${dpi}`
  }
  return '🖥️ 镜像主屏'
}
