// DOM decoding belongs to the host. Consumers receive an immutable frame and
// dimensions regardless of how the preview is rendered.
export function frameSource(element) {
  const displaySize = () => {
    const el = element()
    return { width: el?.naturalWidth || el?.videoWidth || el?.width || 0, height: el?.naturalHeight || el?.videoHeight || el?.height || 0 }
  }
  const captureFrame = () => {
    const { width, height } = displaySize()
    if (!width || !height) return null
    const canvas = document.createElement('canvas')
    canvas.width = width
    canvas.height = height
    const context = canvas.getContext('2d')
    if (!context) throw new Error('无法读取当前画面')
    context.drawImage(element(), 0, 0, width, height)
    return { source: canvas, width, height, label: '当前画面帧' }
  }
  return { displaySize, captureFrame, captureDisplayFrame: captureFrame }
}
