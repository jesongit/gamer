import { computed, ref } from 'vue'

const STORAGE_KEY = 'gamer.workbench.tab-order'

export function useWorkbenchTabOrder(panels) {
  const order = ref([])
  try {
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) || '[]')
    if (Array.isArray(stored)) order.value = [...new Set(stored.filter(key => typeof key === 'string' && key.length <= 200))]
  } catch { /* 存储不可用时仍支持当前会话排序。 */ }
  const sortedPanels = computed(() => {
    const rank = new Map(order.value.map((key, index) => [key, index]))
    return [...panels.value].sort((a, b) => (rank.get(a.key) ?? Infinity) - (rank.get(b.key) ?? Infinity))
  })
  function persist() {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(order.value)) } catch {}
  }
  function move(source, target, after = false) {
    const visible = panels.value.map(panel => panel.key)
    if (source === target || !visible.includes(source) || !visible.includes(target)) return false
    // 保留暂时隐藏/停用插件的占位，恢复后沿用用户位置；新增面板追加到末尾。
    const next = [...new Set([...order.value, ...visible])].filter(key => key !== source)
    next.splice(next.indexOf(target) + Number(after), 0, source)
    order.value = next
    persist()
    return true
  }
  function reset() { order.value = []; persist() }
  return { sortedPanels, move, reset, customized: computed(() => order.value.length > 0) }
}
