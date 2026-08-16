<template>
  <div class="page">
    <div class="page-head">
      <div>
        <div class="page-title">设备列表</div>
        <div class="page-sub">管理接入的设备 · 支持 redroid 容器 / USB / 无线 adb / 模拟器</div>
      </div>
      <div class="head-actions">
        <button class="btn" @click="refresh">🔄 刷新</button>
        <button class="btn btn-primary" @click="openAdd">＋ 添加设备</button>
      </div>
    </div>

    <!-- 设备卡片 -->
    <div class="device-grid">
      <div v-for="d in devices" :key="d.id" class="device-card" :class="{ offline: d.status !== 'online' }">
        <div class="dc-head">
          <div class="dc-name">
            <span class="dot" :class="d.status === 'online' ? 'ok' : (d.status === 'connecting' ? 'run' : 'off')"></span>
            <span class="name">{{ d.name }}</span>
          </div>
          <div class="dc-tags">
            <span class="tag" :class="typeTag(d.kind)">{{ typeLabel(d.kind) }}</span>
            <span v-if="d.screen_mode === 'virtual'" class="tag info" :title="`虚拟屏 ${d.vd_res}@${d.vd_dpi}`">🖥️ {{ d.vd_res }}</span>
            <span v-else class="tag">镜像</span>
          </div>
        </div>

        <div class="dc-info">
          <div class="dc-row"><span>地址</span><span class="mono">{{ d.addr || 'USB' }}</span></div>
          <div class="dc-row"><span>分辨率</span><span>{{ d.screen_mode === 'virtual' ? (d.vd_res + '（虚拟屏）') : (d.width ? `${d.width}x${d.height}` : '未知') }}</span></div>
          <div class="dc-row" v-if="d.screen_mode === 'virtual' && d.pkg"><span>游戏</span><span class="mono">{{ d.pkg }}</span></div>
          <div class="dc-row" v-if="d.fps"><span>帧率</span><span class="mono">{{ d.fps }} fps</span></div>
          <div class="dc-row" v-if="d.error"><span>错误</span><span class="err-text">{{ d.error }}</span></div>
        </div>

        <div class="dc-actions">
          <button class="btn btn-primary btn-sm" :disabled="d.status !== 'online'" @click="openConsole(d)">连接控制</button>
          <button v-if="d.status !== 'online'" class="btn btn-sm" @click="connectDev(d)">连接设备</button>
          <button class="btn btn-sm" @click="editDev(d)">✏️ 编辑</button>
          <button class="btn btn-sm btn-danger" @click="removeDev(d)">删除</button>
        </div>
      </div>
    </div>

    <!-- 添加/编辑设备弹窗 -->
    <div v-if="showAdd" class="modal-mask" @click.self="showAdd = false">
      <div class="modal">
        <div class="modal-head">
          <span class="title">{{ editingId ? '编辑设备' : '添加设备' }}</span>
          <button class="btn btn-ghost btn-sm" @click="closeModal">✕</button>
        </div>
        <div class="modal-body">
          <div class="form-item">
            <label>设备名称</label>
            <input v-model="newDev.name" class="input" placeholder="例如：红米 Note12 挂机号" />
          </div>
          <div class="form-item">
            <label>接入方式</label>
            <div class="type-picker">
              <div v-for="t in types" :key="t.key" class="type-opt" :class="{ sel: newDev.kind === t.key }" @click="newDev.kind = t.key">
                <span class="type-icon">{{ t.icon }}</span>
                <span>{{ t.label }}</span>
              </div>
            </div>
          </div>
          <div class="form-item">
            <label>ADB 地址 <span class="muted">（redroid / 无线 adb / 模拟器需要填写）</span></label>
            <input v-model="newDev.addr" class="input mono" placeholder="redroid:5555 或 192.168.1.88:5555" />
          </div>
          <div class="form-item">
            <label>屏幕模式</label>
            <div class="mode-picker">
              <div class="mode-opt" :class="{ sel: newDev.screen_mode === 'mirror' }" @click="newDev.screen_mode = 'mirror'">
                <div class="mode-title">🖥️ 镜像主屏</div>
                <div class="mode-desc">投屏设备物理屏幕，各设备分辨率不同</div>
              </div>
              <div class="mode-opt" :class="{ sel: newDev.screen_mode === 'virtual' }" @click="newDev.screen_mode = 'virtual'">
                <div class="mode-title">🖥️ 虚拟屏</div>
                <div class="mode-desc">统一分辨率虚拟屏幕，模板跨设备通用</div>
              </div>
            </div>
          </div>
          <template v-if="newDev.screen_mode === 'virtual'">
            <div class="form-item">
              <label>虚拟屏分辨率</label>
              <div class="vd-presets">
                <div v-for="p in vdPresets" :key="p.res" class="vd-opt" :class="{ sel: newDev.vd_res === p.res && newDev.vd_dpi === p.dpi }" @click="newDev.vd_res = p.res; newDev.vd_dpi = p.dpi">
                  <span class="vd-res mono">{{ p.res }}</span>
                  <span class="vd-dpi">@{{ p.dpi }}dpi</span>
                </div>
              </div>
            </div>
            <div class="form-row">
              <div class="form-item">
                <label>宽 × 高</label>
                <input v-model="newDev.vd_res" class="input mono" placeholder="1920x1080" />
              </div>
              <div class="form-item">
                <label>DPI（0=自动）</label>
                <input v-model.number="newDev.vd_dpi" class="input mono" type="number" />
              </div>
            </div>
            <div class="form-item">
              <label>游戏 <span class="muted">（可选，连接成功后自动启动到虚拟屏）</span></label>
              <div class="app-box">
                <input v-model="newDev.pkg" class="input mono" placeholder="搜索应用或输入包名…（点击下拉选择）" @focus="appOpen = true" @input="appOpen = true" @blur="appOpen = false" />
                <button class="btn btn-sm" :disabled="appLoading" @click="loadApps" :title="'从设备读取应用列表'">{{ appLoading ? '加载中…' : '🔄 读取应用' }}</button>
                <div class="app-menu" v-if="appOpen && appFiltered.length">
                  <div v-for="a in appFiltered" :key="a.pkg" class="app-opt" @mousedown.prevent="pickApp(a)">
                    <span class="app-label">{{ a.label }}</span>
                    <span class="app-pkg mono">{{ a.pkg }}</span>
                  </div>
                  <div class="app-empty mono" v-if="!appFiltered.length">无匹配应用</div>
                </div>
              </div>
              <div class="muted small" v-if="appHint">{{ appHint }}</div>
            </div>
            <div class="form-item">
              <label>视频帧率 <span class="muted">（scrcpy 帧率上限：越高越流畅、越耗性能）</span></label>
              <div class="fps-presets">
                <div v-for="f in fpsPresets" :key="f" class="fps-opt mono" :class="{ sel: newDev.fps === f }" @click="newDev.fps = f">{{ f }}</div>
                <div class="fps-opt mono" :class="{ sel: !newDev.fps }" @click="newDev.fps = null">自动</div>
              </div>
            </div>
          </template>
        </div>
        <div class="modal-foot">
          <button class="btn" @click="closeModal">取消</button>
          <button class="btn btn-primary" @click="addDevice">{{ editingId ? '保存修改' : '确认添加' }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { ref, reactive, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { store, devicesData, useToast } from '../store'
import { api } from '../api'

const router = useRouter()
const toast = useToast()
const devices = devicesData
const showAdd = ref(false)
const editingId = ref(null)

const types = [
  { key: 'redroid', label: 'redroid 容器', icon: '🐳' },
  { key: 'usb', label: 'USB 直连', icon: '🔌' },
  { key: 'wifi', label: '无线 adb', icon: '📶' },
  { key: 'emu', label: '模拟器', icon: '🖥️' }
]

const newDev = reactive({ name: '', kind: 'redroid', addr: '', screen_mode: 'virtual', vd_res: '1920x1080', vd_dpi: 420, pkg: '', fps: null })

// 应用下拉（游戏选择）
const appList = ref([])
const appLoading = ref(false)
const appOpen = ref(false)
const appHint = ref('')
const fpsPresets = [15, 30, 60, 120]

const appFiltered = computed(() => {
  const q = newDev.pkg.trim().toLowerCase()
  return appList.value
    .filter(a => !q || a.label.toLowerCase().includes(q) || a.pkg.toLowerCase().includes(q))
    .slice(0, 50)
})

const vdPresets = [
  { res: '1920x1080', dpi: 420 },
  { res: '1080x1920', dpi: 420 },
  { res: '1280x720', dpi: 320 },
  { res: '2340x1080', dpi: 440 }
]

const typeMap = { redroid: 'redroid', usb: 'USB', wifi: '无线', emu: '模拟器' }
const typeLabel = k => typeMap[k] || k
const typeTag = k => ({ redroid: 'info', usb: 'ok', wifi: 'warn', emu: '' }[k])

function resetNew() {
  newDev.name = ''
  newDev.kind = 'redroid'
  newDev.addr = ''
  newDev.screen_mode = 'virtual'
  newDev.vd_res = '1920x1080'
  newDev.vd_dpi = 420
  newDev.pkg = ''
  newDev.fps = null
}

function openAdd() {
  editingId.value = null
  resetNew()
  appList.value = []
  appOpen.value = false
  appHint.value = ''
  showAdd.value = true
}

/** 从设备读取已安装应用（scrcpy list_apps，带真实软件名） */
async function loadApps() {
  if (appLoading.value) return
  appLoading.value = true
  appHint.value = '正在读取设备应用…'
  try {
    const list = editingId.value
      ? await api.listApps(editingId.value)
      : await api.listAppsByAddr(newDev.addr)
    appList.value = list || []
    appHint.value = appList.value.length ? `共 ${appList.value.length} 个应用，输入关键字搜索` : '设备上未发现第三方应用'
  } catch (e) {
    appList.value = []
    appHint.value = '读取失败：' + e.message + '（可直接手动输入包名）'
  } finally {
    appLoading.value = false
  }
}

function pickApp(a) {
  newDev.pkg = a.pkg
  appOpen.value = false
}

async function refresh() {
  try {
    // 先扫描 adb devices 自动注册新设备，再拉列表
    const r = await api.scanDevices()
    devices.value = r.devices && Array.isArray(r.devices) ? r.devices : await api.listDevices()
    if (r.added > 0) toast(`扫描到 ${r.added} 台新设备，已自动添加`, 'success')
    else toast('已刷新设备状态', 'success')
  } catch (e) {
    toast('刷新失败：' + e.message, 'error')
  }
}

function openConsole(d) {
  store.deviceId = d.id
  router.push('/console')
}

async function connectDev(d) {
  try {
    await api.connectDevice(d.id)
    toast(`已连接 ${d.name}`, 'success')
    refresh()
  } catch (e) {
    toast(`连接失败：${e.message}`, 'error')
  }
}

async function removeDev(d) {
  if (!confirm(`确定删除设备 ${d.name}？`)) return
  try {
    await api.deleteDevice(d.id)
    devices.value = devices.value.filter(x => x.id !== d.id)
    toast('设备已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

function closeModal() {
  showAdd.value = false
  editingId.value = null
}

function editDev(d) {
  editingId.value = d.id
  newDev.name = d.name
  newDev.kind = d.kind
  newDev.addr = d.addr
  newDev.screen_mode = d.screen_mode
  newDev.vd_res = d.vd_res || '1920x1080'
  newDev.vd_dpi = d.vd_dpi || 420
  newDev.pkg = d.pkg || ''
  newDev.fps = d.fps || null
  appList.value = []
  appOpen.value = false
  appHint.value = ''
  showAdd.value = true
  loadApps()
}

async function addDevice() {
  if (!newDev.name) return toast('请填写设备名称', 'error')
  const payload = {
    name: newDev.name,
    kind: newDev.kind,
    addr: newDev.addr,
    screen_mode: newDev.screen_mode,
    vd_res: newDev.screen_mode === 'virtual' ? newDev.vd_res : null,
    vd_dpi: newDev.screen_mode === 'virtual' ? newDev.vd_dpi : null,
    pkg: newDev.screen_mode === 'virtual' ? (newDev.pkg.trim() || null) : null,
    fps: newDev.fps
  }
  try {
    if (editingId.value) {
      await api.updateDevice(editingId.value, payload)
      toast('设备已更新，配置变更将自动重连', 'success')
    } else {
      await api.createDevice(payload)
      toast('设备已添加', 'success')
    }
    closeModal()
    refresh()
  } catch (e) {
    toast((editingId.value ? '更新失败：' : '添加失败：') + e.message, 'error')
  }
}

onMounted(refresh)
</script>

<style scoped>
.head-actions { display: flex; gap: 10px; }

.device-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 14px; }

.device-card {
  background: var(--bg-1); border: 1px solid var(--border); border-radius: var(--radius);
  padding: 16px; display: flex; flex-direction: column; gap: 12px; transition: all .2s;
}
.device-card:hover { border-color: #33405e; transform: translateY(-1px); }
.device-card.offline { opacity: .55; }

.dc-head { display: flex; align-items: center; justify-content: space-between; }
.dc-name { display: flex; align-items: center; gap: 8px; font-weight: 600; font-size: 14px; }
.dc-info { display: flex; flex-direction: column; gap: 6px; }
.dc-row { display: flex; justify-content: space-between; font-size: 12px; color: var(--text-1); }
.dc-row .mono { font-family: var(--mono); color: var(--text-0); font-size: 11px; }
.err-text { color: var(--danger); font-size: 11px; max-width: 160px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.dc-actions { display: flex; gap: 8px; }

.type-picker { display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px; }
.type-opt {
  display: flex; flex-direction: column; align-items: center; gap: 6px;
  padding: 12px 6px; border-radius: var(--radius-sm); border: 1px solid var(--border);
  cursor: pointer; font-size: 11px; color: var(--text-1); transition: all .15s;
}
.type-opt:hover { border-color: #33405e; }
.type-opt.sel { border-color: var(--accent); color: var(--accent); background: rgba(34,211,165,.06); }
.type-icon { font-size: 20px; }

.mode-picker { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
.mode-opt {
  padding: 12px; border-radius: var(--radius-sm); border: 1px solid var(--border);
  cursor: pointer; transition: all .15s; display: flex; flex-direction: column; gap: 4px;
}
.mode-opt:hover { border-color: #33405e; }
.mode-opt.sel { border-color: var(--accent); background: rgba(34,211,165,.06); }
.mode-title { font-size: 13px; font-weight: 600; }
.mode-desc { font-size: 11px; color: var(--text-2); }

.vd-presets { display: grid; grid-template-columns: repeat(4, 1fr); gap: 8px; }
.vd-opt {
  display: flex; flex-direction: column; align-items: center; gap: 2px;
  padding: 10px 4px; border-radius: var(--radius-sm); border: 1px solid var(--border);
  cursor: pointer; transition: all .15s;
}
.vd-opt:hover { border-color: #33405e; }
.vd-opt.sel { border-color: var(--accent-2); background: rgba(56,189,248,.06); }
.vd-res { font-size: 12px; color: var(--text-0); }
.vd-dpi { font-size: 10px; color: var(--text-2); }
.dc-tags { display: flex; gap: 6px; }

.muted { color: var(--text-2); font-weight: 400; }
.small { font-size: 11px; margin-top: 4px; }

/* 应用下拉 */
.app-box { position: relative; }
.app-box .btn { flex-shrink: 0; }
.app-menu {
  position: absolute; left: 0; right: 0; top: calc(100% + 4px); z-index: 30;
  background: var(--bg-1); border: 1px solid var(--border); border-radius: var(--radius-sm);
  max-height: 220px; overflow: auto; box-shadow: 0 8px 24px rgba(0,0,0,.45);
}
.app-opt {
  display: flex; flex-direction: column; gap: 2px; padding: 7px 10px;
  cursor: pointer; border-bottom: 1px solid rgba(255,255,255,.04);
}
.app-opt:hover { background: var(--bg-3); }
.app-label { font-size: 12px; color: var(--text-0); }
.app-pkg { font-size: 10px; color: var(--text-2); }
.app-empty { padding: 10px; font-size: 11px; color: var(--text-2); text-align: center; }

/* 帧率选择 */
.fps-presets { display: grid; grid-template-columns: repeat(5, 1fr); gap: 8px; }
.fps-opt {
  text-align: center; padding: 8px 4px; border-radius: var(--radius-sm);
  border: 1px solid var(--border); cursor: pointer; font-size: 12px;
  color: var(--text-1); transition: all .15s;
}
.fps-opt:hover { border-color: #33405e; }
.fps-opt.sel { border-color: var(--accent-2); background: rgba(56,189,248,.06); color: var(--accent-2); }
</style>
