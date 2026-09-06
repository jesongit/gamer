// Package Context（plan §28/§38/§39）：Current Package 由 Core Store 统一管理。
//
// 四 Context 命名（§39）：deviceId（设备）、androidPackageName（Android 应用
// 运行目标，归设备配置，不在此处）、currentPackageId（数据上下文，本模块）、
// activePluginId（当前查看的插件，归 workspace 导航）。Package-aware UI 一律
// 消费 currentPackageId，切换 Package 后自动联动，插件面板不各自管理当前包。
import { computed, reactive } from 'vue'
import { api } from './api'

const STORAGE_KEY = 'gamer.currentPackageId'

export const packageStore = reactive({
  packages: [],            // [{id,name,version,revision,targets,plugins}]（列表摘要）
  currentPackageId: null,  // 当前 Package id（数据上下文）
  loaded: false,
  loading: false,
  loadError: null,
})

function readStoredId() {
  try { return window.localStorage.getItem(STORAGE_KEY) || null } catch { return null }
}

function writeStoredId(id) {
  try {
    if (id) window.localStorage.setItem(STORAGE_KEY, id)
    else window.localStorage.removeItem(STORAGE_KEY)
  } catch { /* 隐私模式等存储不可用：仅会话内生效 */ }
}

/** 拉取包列表并校正 currentPackageId（存储的 id 已不存在时回落首包）。 */
export async function loadPackages({ force = false, api: apiOverride } = {}) {
  if (packageStore.loading) return
  if (packageStore.loaded && !force) return
  packageStore.loading = true
  packageStore.loadError = null
  try {
    const rep = await (apiOverride || api).listPackages()
    packageStore.packages = Array.isArray(rep?.packages) ? rep.packages : []
    const ids = new Set(packageStore.packages.map(p => p.id))
    let current = packageStore.currentPackageId || readStoredId()
    if (!current || !ids.has(current)) current = packageStore.packages[0]?.id || null
    packageStore.currentPackageId = current
    writeStoredId(current)
    packageStore.loaded = true
  } catch (error) {
    packageStore.loadError = error
  } finally {
    packageStore.loading = false
  }
}

/** 切换当前 Package（§38：Package-aware UI 自动联动；持久化到本地）。 */
export function selectPackage(packageId) {
  const id = packageId || null
  if (id && !packageStore.packages.some(p => p.id === id)) return
  packageStore.currentPackageId = id
  writeStoredId(id)
}

/** 包列表变化后（新建/导入/删除/复制）刷新并保持/回落当前包。 */
export async function refreshPackages(opts = {}) {
  packageStore.loaded = false
  await loadPackages({ force: true, ...opts })
}

export const currentPackageId = computed(() => packageStore.currentPackageId)
export const currentPackage = computed(
  () => packageStore.packages.find(p => p.id === packageStore.currentPackageId) || null,
)
