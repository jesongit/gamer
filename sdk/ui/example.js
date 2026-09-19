(async () => {
const { connectGamer, applyGamerTheme } = window.GamerUI
const client = connectGamer()
const error = document.querySelector('#error')
try {
  const context = await client.call('context.get')
  applyGamerTheme(context.theme)
  document.querySelector('#connection').textContent = '已连接 · 操作结果显示在宿主右侧状态条'
  document.querySelector('#submit').disabled = false
} catch (e) { error.textContent = e.message }
document.querySelector('#submit').addEventListener('click', async event => {
  event.preventDefault(); error.textContent = ''
  const text = document.querySelector('#message').value
  try {
    const accepted = await client.call('status.set', { text: '文本已准备', actions: [{ label: '复制', copy: text }, { label: '详情', detail: text }] })
    if (!accepted) throw new Error('当前面板已失效，请重新打开')
  }
  catch (e) { error.textContent = e.message }
})
window.addEventListener('pagehide', () => client.dispose(), { once: true })
})()
