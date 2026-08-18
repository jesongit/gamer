<template>
  <div class="console">
    <!-- 左：画面区 -->
    <div class="stage">
      <!-- 顶部工具条 -->
      <div class="toolbar">
        <button class="btn btn-sm" @click="shot">📷 截图</button>
        <button class="btn btn-sm" @click="rotate">🔄 旋转</button>
        <button class="btn btn-sm" @click="key('HOME')">🏠 Home</button>
        <button class="btn btn-sm" @click="key('BACK')">⬅ 返回</button>
        <button class="btn btn-sm" @click="key('APP_SWITCH')">🪟 最近</button>
        <button class="btn btn-sm" @click="key('VOL_UP')">🔊＋</button>
        <button class="btn btn-sm" @click="key('VOL_DOWN')">🔊－</button>
        <button class="btn btn-sm" @click="toggleAudio" :title="audioMuted ? '取消静音（听游戏声音）' : '静音'">{{ audioMuted ? '🔇' : '🔊' }}</button>
        <button class="btn btn-sm" @click="launchGame" :title="'启动到虚拟屏：' + (currentPkg || '未配置应用')">🚀 启动应用</button>
        <div class="tb-sep"></div>
        <button class="btn btn-sm" @click="clipboard">📋 剪贴板</button>
        <span class="tb-tip">鼠标左键=触控 · 滚轮=滑动 · 支持多点触控</span>
      </div>

      <div class="video-wrap" ref="videoWrap">
        <video
          ref="videoElement"
          autoplay
          playsinline
          :muted="audioMuted"
          class="video-stream"
          @mousedown="onMouseDown"
          @mousemove="onMouseMove"
          @mouseup="onMouseUp"
          @wheel.prevent="onWheel"
          @contextmenu.prevent
          @mouseleave="onVideoMouseLeave"
        ></video>

        <!-- 找图命中框演示（模板测试） -->
        <div v-if="showHit" class="hit-box" :style="hitStyle">
          <span class="hit-label">{{ hitLabel }}</span>
        </div>

        <!-- 框选模板 -->
        <div v-if="selecting" class="select-box" :style="selStyle"></div>

        <!-- alt 模式点击/滑动反馈 -->
        <div v-if="altFeedback.show && altFeedback.kind === 'tap'" class="alt-tap" :style="altTapStyle">
          <span class="alt-label">tap</span>
        </div>
        <div v-if="altFeedback.show && altFeedback.kind === 'region'" class="alt-region" :style="altFeedbackStyle">
          <span class="alt-label">region</span>
        </div>

        <!-- 放大预览镜 -->
        <div class="loupe" v-show="loupe.show" :style="{ left: loupe.x + 'px', top: loupe.y + 'px' }">
          <canvas ref="loupeCanvas" width="300" height="300"></canvas>
          <span class="loupe-tag mono">{{ loupe.zoom }}×</span>
        </div>

        <div class="v-overlay" v-if="!connected">
          <div class="v-connecting" v-if="connecting">
            <span class="dot run"></span> 正在建立 WebRTC 连接…
          </div>
          <div v-else>
            <div class="v-empty-icon">📴</div>
            <div class="v-empty-text">{{ errorMsg || '未连接设备' }}</div>
            <button class="btn btn-primary" @click="flushAndConnect">连接 {{ currentName }}</button>
          </div>
        </div>

        <div class="v-stats" v-if="connected">
          <span class="st">{{ fps }} fps</span>
          <span class="st">延迟 {{ delay }}ms</span>
          <span class="st">{{ res }}</span>
          <span class="st">码率 {{ bitrate }}</span>
          <span class="st">H.264 · WebRTC</span>
        </div>

        <button class="v-fs" @click="fullscreen" title="全屏">⛶</button>
      </div>
    </div>

    <!-- 右：控制面板（页签切换） -->
    <aside class="panel">
      <div class="panel-tabs">
        <button v-for="t in tabs" :key="t.key" class="tab-btn" :class="{ active: activeTab === t.key }" @click="activeTab = t.key">
          {{ t.icon }}<span class="tab-label">{{ t.label }}</span>
        </button>
      </div>

      <div class="tab-body">
        <!-- 设备（设备管理 + 配置，融合原设备列表功能） -->
        <div v-show="activeTab === 'info'" class="panel-sec">
          <!-- 设备选择：下拉框 + 刷新扫描 + 手动新增 -->
          <div class="dev-pick">
            <select v-model="store.deviceId" class="select mono dev-select" @change="onDeviceSelect">
              <option value="">选择设备…</option>
              <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }} · {{ d.status === 'online' ? '在线' : '离线' }}</option>
            </select>
            <button class="btn btn-sm" :disabled="scanning" @click="refreshDevices" title="扫描 adb 设备（新设备自动入库）">🔄 刷新</button>
            <button class="btn btn-sm" @click="startAdd" title="手动新增设备">＋ 新增</button>
          </div>

          <!-- 新增模式：完整创建表单（接入方式 / ADB 地址仅新增时可改） -->
          <div v-if="mode === 'add'" class="cfg-form">
            <div class="cfg-form-head">
              <span>新增设备</span>
              <span class="cfg-form-sub">填写信息后确认添加</span>
            </div>
            <div class="form-item">
              <label>设备名称</label>
              <input v-model="form.name" class="input" placeholder="例如：红米 Note12 挂机号" />
            </div>
            <div class="form-item">
              <label>接入方式</label>
              <div class="type-picker">
                <div v-for="t in types" :key="t.key" class="type-opt" :class="{ sel: form.kind === t.key }" @click="form.kind = t.key">
                  <span class="type-icon">{{ t.icon }}</span>
                  <span>{{ t.label }}</span>
                </div>
              </div>
            </div>
            <div class="form-item">
              <label>ADB 地址 <span class="muted">（redroid / 无线 adb / 模拟器需要填写）</span></label>
              <input v-model="form.addr" class="input mono" placeholder="redroid:5555 或 192.168.1.88:5555" />
            </div>
            <div class="form-item">
              <label>屏幕模式</label>
              <div class="mode-picker">
                <div class="mode-opt" :class="{ sel: form.screen_mode === 'mirror' }" @click="form.screen_mode = 'mirror'">
                  <div class="mode-title">🖥️ 镜像主屏</div>
                  <div class="mode-desc">投屏设备物理屏幕，各设备分辨率不同</div>
                </div>
                <div class="mode-opt" :class="{ sel: form.screen_mode === 'virtual' }" @click="form.screen_mode = 'virtual'">
                  <div class="mode-title">🖥️ 虚拟屏</div>
                  <div class="mode-desc">统一分辨率虚拟屏幕，模板跨设备通用</div>
                </div>
              </div>
            </div>
            <template v-if="form.screen_mode === 'virtual'">
              <div class="form-item">
                <label>虚拟屏分辨率</label>
                <div class="vd-presets">
                  <div v-for="p in vdPresets" :key="p.res" class="vd-opt" :class="{ sel: form.vd_res === p.res && String(form.vd_dpi) === String(p.dpi) }" @click="form.vd_res = p.res; form.vd_dpi = p.dpi">
                    <span class="vd-res mono">{{ p.res }}</span>
                    <span class="vd-dpi">@{{ p.dpi }}dpi</span>
                  </div>
                </div>
              </div>
              <div class="form-row">
                <div class="form-item">
                  <label>宽 × 高</label>
                  <input v-model="form.vd_res" class="input mono" placeholder="1920x1080" />
                </div>
                <div class="form-item">
                  <label>DPI <span class="muted">（0=自动）</span></label>
                  <div class="dpi-box">
                    <input v-model.number="form.vd_dpi" class="input mono" type="number" placeholder="0" />
                    <button class="btn btn-sm" :class="{ active: !form.vd_dpi }" @click="form.vd_dpi = 0" title="DPI 自动（跟随屏幕）">自动</button>
                  </div>
                </div>
              </div>
              <div class="form-item">
                <label>应用 <span class="muted">（可选，连接成功后自动启动到虚拟屏）</span></label>
                <div class="app-box">
                  <input v-model="pkgDraft" class="input mono" placeholder="搜索应用或输入包名…（点击下拉选择，回车确认）" @focus="appOpen = true" @input="appOpen = true" @keydown.enter="commitPkg" @blur="appOpen = false" />
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
                  <div v-for="f in fpsPresets" :key="f" class="fps-opt mono" :class="{ sel: form.fps === f }" @click="form.fps = f">{{ f }}</div>
                </div>
              </div>
            </template>
            <div class="cfg-actions">
              <button class="btn btn-primary" :disabled="configApplying" @click="addDevice">
                {{ configApplying ? '添加中…' : '确认添加' }}
              </button>
              <button class="btn btn-sm" @click="cancelAdd">取消</button>
            </div>
          </div>

          <!-- 编辑模式：连接概览 + 可折叠配置 -->
          <template v-else>
            <div v-if="current" class="dev-summary">
              <div class="ps-head">
                <span class="dot" :class="connected ? 'ok' : 'off'"></span>
                <span class="ps-title">{{ current.name }}</span>
                <span class="tag" :class="connected ? 'info' : ''">{{ connected ? '已连接' : (current.status === 'online' ? '在线' : '离线') }}</span>
              </div>
              <div class="sum-row">
                <span class="sum-label">接入</span>
                <span class="sum-value"><span class="kind-badge">{{ kindInfo(current.kind).icon }} {{ kindInfo(current.kind).label }}</span></span>
              </div>
              <div class="sum-row">
                <span class="sum-label">地址</span>
                <span class="sum-value mono">{{ current.addr || '—' }}</span>
              </div>
              <div class="sum-row">
                <span class="sum-label">屏幕</span>
                <span class="sum-value">{{ screenSummary }}</span>
              </div>
              <div class="sum-actions">
                <button v-if="!connected" class="btn btn-primary" :disabled="!store.deviceId || connecting" @click="flushAndConnect">
                  {{ connecting ? '连接中…' : '🔌 连接' }}
                </button>
                <button v-else class="btn" @click="disconnect">断开连接</button>
                <button class="btn btn-danger" @click="removeDevice" title="删除设备">删除</button>
              </div>

              <!-- 设备配置（接入方式 / ADB 地址只读展示，不在表单内） -->
              <div class="cfg-form">
                <div class="cfg-form-head">
                  <span>设备配置</span>
                  <span class="cfg-form-sub">{{ configApplying ? '保存中…' : (formDirty ? '有未保存的修改（自动保存）' : (savedVisible ? '已自动保存' : '')) }}</span>
                </div>
                <div class="form-item">
                  <label>设备名称</label>
                  <input v-model="form.name" class="input" placeholder="例如：红米 Note12 挂机号" />
                </div>
                <div class="form-item">
                  <label>屏幕模式</label>
                  <div class="mode-picker">
                    <div class="mode-opt" :class="{ sel: form.screen_mode === 'mirror' }" @click="form.screen_mode = 'mirror'">
                      <div class="mode-title">🖥️ 镜像主屏</div>
                      <div class="mode-desc">投屏设备物理屏幕，各设备分辨率不同</div>
                    </div>
                    <div class="mode-opt" :class="{ sel: form.screen_mode === 'virtual' }" @click="form.screen_mode = 'virtual'">
                      <div class="mode-title">🖥️ 虚拟屏</div>
                      <div class="mode-desc">统一分辨率虚拟屏幕，模板跨设备通用</div>
                    </div>
                  </div>
                </div>
                <template v-if="form.screen_mode === 'virtual'">
                  <div class="form-item">
                    <label>虚拟屏分辨率</label>
                    <div class="vd-presets">
                      <div v-for="p in vdPresets" :key="p.res" class="vd-opt" :class="{ sel: form.vd_res === p.res && String(form.vd_dpi) === String(p.dpi) }" @click="form.vd_res = p.res; form.vd_dpi = p.dpi">
                        <span class="vd-res mono">{{ p.res }}</span>
                        <span class="vd-dpi">@{{ p.dpi }}dpi</span>
                      </div>
                    </div>
                  </div>
                  <div class="form-row">
                    <div class="form-item">
                      <label>宽 × 高</label>
                      <input v-model="form.vd_res" class="input mono" placeholder="1920x1080" />
                    </div>
                    <div class="form-item">
                      <label>DPI <span class="muted">（0=自动）</span></label>
                      <div class="dpi-box">
                        <input v-model.number="form.vd_dpi" class="input mono" type="number" placeholder="0" />
                        <button class="btn btn-sm" :class="{ active: !form.vd_dpi }" @click="form.vd_dpi = 0" title="DPI 自动（跟随屏幕）">自动</button>
                      </div>
                    </div>
                  </div>
                  <div class="form-item">
                    <label>应用 <span class="muted">（可选，连接成功后自动启动到虚拟屏）</span></label>
                    <div class="app-box">
                      <input v-model="pkgDraft" class="input mono" placeholder="搜索应用或输入包名…（点击下拉选择，回车确认）" @focus="appOpen = true" @input="appOpen = true" @keydown.enter="commitPkg" @blur="appOpen = false" />
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
                      <div v-for="f in fpsPresets" :key="f" class="fps-opt mono" :class="{ sel: form.fps === f }" @click="form.fps = f">{{ f }}</div>
                    </div>
                  </div>
                </template>
              </div>
              <div class="cfg-hint">{{ connected ? '已连接：修改自动保存并实时生效（自动重连）' : '未连接：修改自动保存，连接后按新配置生效' }}</div>
            </div>

            <!-- 无设备 -->
            <div v-else class="dev-empty">
              <div class="dev-empty-icon">📴</div>
              <div class="dev-empty-text">未选择设备</div>
              <button class="btn btn-sm" @click="startAdd">＋ 新增设备</button>
            </div>
          </template>
        </div>

        <!-- 脚本（模板功能 + 脚本运行/编辑） -->
        <div v-show="activeTab === 'script'" class="panel-sec script-tab">
          <!-- 模板功能（放上面） -->
          <div class="script-tpl">
            <!-- 模板文件列表（非裁切时） -->
            <template v-if="!crop.active">
              <div class="tpl-top">
                <input v-model.number="testThreshold" class="input input-sm mono" type="number" min="0" max="1" step="0.01" placeholder="测试阈值 0~1" title="模板测试阈值，默认 0.8" />
                <button class="btn btn-sm" :class="{ active: picking }" @click="togglePick" :disabled="!connected" title="在画面上框选区域保存为模板">✂️ 框选</button>
                <button class="btn btn-sm" @click="$refs.tplUpload.click()" title="上传图片模板">⬆️ 上传</button>
                <input ref="tplUpload" type="file" accept="image/png,image/jpeg" hidden @change="onTplUpload" />
              </div>
              <div class="tpl-list-wrap">
                <div class="tpl-list-head">
                  <span class="tpl-cell thumb">缩略图</span>
                  <span class="tpl-cell name">文件名</span>
                  <span class="tpl-cell ops">操作</span>
                </div>
                <div class="tpl-list">
                  <div v-for="t in templates" :key="t.name" class="tpl-row" :class="{ 'del-confirm': confirmDelTpl === t.name }" @click="onTplRowClick($event, t)">
                    <span class="tpl-cell thumb">
                      <span class="tpl-thumb"><img :src="tplThumbUrl(t.name)" alt="" loading="lazy" @error="e => e.target.style.visibility = 'hidden'" /></span>
                    </span>
                    <span class="tpl-cell name mono" :title="t.name">{{ t.name }}</span>
                    <span class="tpl-cell ops">
                      <button class="btn btn-sm" @click.stop="openTplView(t.name)">查看</button>
                      <button class="btn btn-sm" :class="{ 'tpl-del-confirm': confirmDelTpl === t.name }" @click.stop="onTplDeleteClick(t)">{{ confirmDelTpl === t.name ? '确认' : '删除' }}</button>
                      <button class="btn btn-sm" @click.stop="onTplMatchClick(t)">匹配</button>
                    </span>
                  </div>
                  <div v-if="!templates.length" class="tpl-empty">暂无模板，点击「框选」或「上传」创建</div>
                </div>
              </div>
              <div class="tpl-tools">
                <span class="ps-sub">{{ scriptMode === 'edit' ? '点击模板 → 测试匹配 / Alt 或 alt 模式 → 生成记录' : '点击模板 → 在当前画面测试匹配' }}</span>
              </div>
            </template>

            <!-- 二次裁切（框选后占满整个模板区域） -->
            <div v-else class="crop-panel crop-panel-full" ref="cropSec">
              <div class="ps-head">
                <span class="ps-title">✂️ 二次裁切</span>
                <span class="ps-sub mono">{{ cropSize }}</span>
              </div>
              <div class="crop-stage">
                <canvas ref="cropCanvas" class="crop-canvas" @mousedown="cropMouseDown" @mousemove="cropMouseMove" @mouseup="cropMouseUp" @mouseleave="cropMouseLeave"></canvas>
                <div class="crop-hint">拖动边框/角调整选框（只动遮罩框）· 拖框内移动位置</div>
              </div>
              <input v-model="crop.name" class="input mono" placeholder="模板名称（默认自动生成）" @keydown.enter="saveTemplate" />
              <div class="crop-actions">
                <button class="btn btn-sm" @click="cancelCrop">取消</button>
                <button class="btn btn-sm btn-ghost" @click="repick">重新框选</button>
                <button class="btn btn-sm btn-primary" :disabled="saving" @click="saveTemplate">{{ saving ? '保存中…' : '💾 保存模板' }}</button>
              </div>
            </div>

            <!-- 模板查看大图 -->
            <div v-if="viewTpl" class="tpl-view-mask" @click.self="closeTplView">
              <div class="tpl-view-modal">
                <button class="tpl-view-close" @click="closeTplView" title="关闭">✕</button>
                <img :src="tplThumbUrl(viewTpl)" alt="模板预览" />
                <div class="tpl-view-name mono">{{ viewTpl }}</div>
              </div>
            </div>
          </div>
          <!-- 脚本功能：运行模式 -->
          <div v-if="scriptMode === 'run'" class="script-run">
            <div class="auto-run">
              <select v-model="selScript" class="select mono">
                <option value="">选择要运行的脚本…</option>
                <option v-for="s in scripts" :key="s.id" :value="s.id">{{ s.name }}</option>
              </select>
              <select v-model="logLevel" class="select mono log-level" title="日志级别">
                <option value="info">info</option>
                <option value="debug">debug</option>
              </select>
            </div>
            <div class="run-actions">
              <button class="btn btn-primary" :disabled="!selScript || !store.deviceId" @click="runScript">▶ 运行</button>
              <button class="btn" @click="startNewScript">新建</button>
              <button class="btn" :disabled="!selScript" @click="editCurrentScript">编辑</button>
              <button class="btn btn-danger" :disabled="!selScript" @click="deleteCurrentScript">删除</button>
            </div>

            <div ref="logBox" class="live-logs script-logs mono">
              <div v-for="(l, i) in liveLogs" :key="i" class="ll" :class="l.level">
                <span class="ll-time">{{ l.time }}</span>
                <span class="ll-msg">{{ l.msg }}</span>
              </div>
            </div>
          </div>

          <!-- 脚本功能：编辑模式（新建脚本） -->
          <div v-else class="script-edit">
            <div class="edit-name-row">
              <input v-model="editScriptName" class="input mono" placeholder="脚本名称（可省略 .yml 后缀）" @keydown.enter="saveEditScript" />
            </div>
            <div class="edit-actions">
              <button class="btn btn-primary" :disabled="scriptSaving" @click="saveEditScript">{{ scriptSaving ? '保存中…' : '💾 保存' }}</button>
              <button class="btn" @click="cancelEditScript">取消</button>
              <button class="btn" :class="{ active: altMode }" @click="toggleAltMode" title="开启后点击模板/投屏只生成操作记录，不发送控制指令">⌥ alt 模式</button>
            </div>
            <div class="edit-interval">
              <span>操作间隔</span>
              <input v-model.number="stepInterval" class="input input-sm mono" type="number" min="0" step="50" title="每个操作后自动追加 wait 的毫秒数" />
              <span>ms</span>
            </div>
            <div class="op-record">
              <div v-if="!opRecords.length" class="op-record-empty">请在alt模式下进行操作生成记录</div>
              <div v-for="r in opRecords" :key="r.id" class="op-record-line mono" @click="applyOpRecord(r)">
                {{ r.text }}
              </div>
            </div>
            <textarea v-model="editScriptCode" class="script-editor mono" spellcheck="false" placeholder="# YAML 脚本&#10;name: 脚本名&#10;&#10;steps:&#10;  - log: hello"></textarea>
          </div>
        </div>
      </div>
    </aside>
  </div>
</template>

<script>
// 应用列表缓存：设备/地址 -> { list, ts }，应用列表不常变，避免每次重复读取
const appCache = new Map()
const APP_CACHE_TTL = 5 * 60 * 1000
</script>

<script setup>
import { ref, reactive, computed, watch, nextTick, onMounted, onUnmounted } from 'vue'
import { load as yamlLoad } from 'js-yaml'
import { useRouter } from 'vue-router'
import { store, devicesData, scriptsData, templatesData, useToast } from '../store'
import { api } from '../api'

const router = useRouter()
const toast = useToast()

const videoWrap = ref(null)
const videoElement = ref(null)

const connected = ref(false)
const connecting = ref(false)
const errorMsg = ref('')
const fps = ref(0)
const delay = ref(0)
const res = ref('—')
const bitrate = ref('—')
const selScript = ref('')
// 脚本页签：运行/编辑模式 + 日志级别
const DEFAULT_SCRIPT_CODE = `name: 新脚本

steps:
  - wait: 1000
  - log: "脚本开始运行"
`
const scriptMode = ref('run')
const logLevel = ref('info')
const editScriptName = ref('新脚本')
const editScriptCode = ref(DEFAULT_SCRIPT_CODE)
// 编辑模式当前编辑的脚本 id（null=新建）
const editScriptId = ref(null)
const scriptSaving = ref(false)
// 每个操作后自动追加的默认间隔（可配置）
const stepInterval = ref(200)
// alt 模式：仅在脚本编辑模式生效；开启后模板/投屏点击只生成操作记录
const altMode = ref(false)
// 操作记录区：最多展示 3 行，每行可点击追加到编辑区
const opRecords = ref([])
let opRecordSeq = 0
// alt 手势（点击/滑动投屏时记录，不发送控制指令）
const altGesture = reactive({ active: false, moved: false, start: { x: 0, y: 0 }, last: { x: 0, y: 0 } })
// alt 模式点击/滑动画面反馈（点击圆点 / 滑动 region 框）
const altFeedback = reactive({ show: false, kind: '', x: 0, y: 0, w: 0, h: 0 })
let altFeedbackTimer = null
// 日志原始数据（未过滤），用于按级别切换显示
let rawLogs = []
// 本次运行开始时间：清空日志区后只显示本次运行产生的日志
let runStartTime = 0
const picking = ref(false)
const testThreshold = ref(0.8)
// 模板列表：查看大图 / 删除二次确认
const viewTpl = ref(null)
const confirmDelTpl = ref(null)
const selecting = ref(false)
const selStart = reactive({ x: 0, y: 0 })
const selEnd = reactive({ x: 0, y: 0 })
const showHit = ref(false)
const hit = reactive({ x: 0, y: 0, w: 0, h: 0 })
const hitLabel = ref('')
let hitTimer = null
const liveLogs = ref([])
const logBox = ref(null)
// 二次裁切（右侧面板）
const crop = reactive({ active: false, imgW: 0, imgH: 0, baseW: 0, baseH: 0, originX: 0, originY: 0, rect: { x: 0, y: 0, w: 0, h: 0 }, preview: '', name: '' })
const cropCanvas = ref(null)
const cropSec = ref(null)
// 二次裁切底图：框选时冻结的初始画面，拖动时只动遮罩框
let cropBaseCanvas = null
const cropDrag = reactive({ mode: null, startX: 0, startY: 0, orig: null })
const saving = ref(false)
// 放大预览镜
const loupe = reactive({ show: false, x: 0, y: 0, zoom: 2.5 })
const loupeCanvas = ref(null)

// WebRTC 状态
let ws = null
let pc = null
let controlChannel = null
let mediaStream = null
let statsTimer = null
let logTimer = null
// 连接同步锁：防止并发 connect() 创建多个 PeerConnection（双连接 → 串流 → 画面定格）
let connectLock = false

// ---------- 多页面互斥锁 + 自动重连 ----------
// 同一设备同一时刻只允许一个页面持有 WebRTC 连接（服务端单 viewer 设计）。
// 多个浏览器页面/标签页同时操作时会互相踢连接导致黑屏：
//  - 连接成功即持有 localStorage 锁并心跳续期
//  - 被踢（被动断开）时检查锁：他人持有 → 提示且不重连（避免互踢死循环）；
//    锁在自己手里/已过期 → 自动重连（3/6/12s 退避，上限 3 次）
//  - 用户手动操作（点连接 / 切换配置）→ 强制抢锁
const LOCK_KEY = 'gb_webrtc_lock'
const LOCK_TTL = 15000
const lock = reactive({ pageId: Math.random().toString(36).slice(2, 10), deviceId: null, ts: 0 })
let lockTimer = null
let reconnectTimer = null
let reconnectAttempts = 0
let manualClose = false

function readLock() {
  try { return JSON.parse(localStorage.getItem(LOCK_KEY) || 'null') } catch (e) { return null }
}

function acquireLock(force = false) {
  const cur = readLock()
  const heldByOther = cur && cur.deviceId === store.deviceId && cur.pageId !== lock.pageId && Date.now() - cur.ts < LOCK_TTL
  if (heldByOther && !force) return false
  const l = { pageId: lock.pageId, deviceId: store.deviceId, ts: Date.now() }
  localStorage.setItem(LOCK_KEY, JSON.stringify(l))
  lock.deviceId = store.deviceId
  lock.ts = l.ts
  return true
}

function releaseLock() {
  const cur = readLock()
  if (cur && cur.pageId === lock.pageId) localStorage.removeItem(LOCK_KEY)
  lock.deviceId = null
}

function startLockHeartbeat() {
  stopLockHeartbeat()
  lockTimer = setInterval(() => {
    if (connected.value) acquireLock(false)
  }, 8000)
}

function stopLockHeartbeat() {
  if (lockTimer) { clearInterval(lockTimer); lockTimer = null }
}

/** 被动断开后的自动重连调度：他人持锁则不重连，否则按退避时间重连 */
function scheduleReconnect() {
  if (reconnectTimer || !store.deviceId) return
  const cur = readLock()
  if (cur && cur.deviceId === store.deviceId && cur.pageId !== lock.pageId && Date.now() - cur.ts < LOCK_TTL) {
    errorMsg.value = '设备正在其他页面使用，画面已断开'
    return
  }
  const delay = [3000, 6000, 12000][Math.min(reconnectAttempts, 2)]
  reconnectAttempts++
  toast(`连接已断开，${delay / 1000} 秒后自动重连…`, 'warn')
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null
    // 延迟后二次检查锁：被踢后其他页面可能已抢锁（连接竞态窗口），
    // 此时不再自动重连，避免多页面互踢死循环
    const cur2 = readLock()
    if (cur2 && cur2.deviceId === store.deviceId && cur2.pageId !== lock.pageId && Date.now() - cur2.ts < LOCK_TTL) {
      errorMsg.value = '设备正在其他页面使用，画面已断开'
      return
    }
    connect(false) // 自动重连不强抢锁（锁在自己手里会成功；在别人手里已在上方拦截）
  }, delay)
}

function onChannelOpen() {
  connected.value = true
  connecting.value = false
  reconnectAttempts = 0
  videoConnectTs = Date.now()
  acquireLock(true) // 能建立连接说明服务端已踢掉旧 viewer，锁归本页
  startLockHeartbeat()
  toast('WebRTC 连接建立', 'success')
}

function onChannelClose() {
  connected.value = false
  stopLockHeartbeat()
  if (!manualClose) scheduleReconnect()
  manualClose = false
}

// 右侧面板页签
const activeTab = ref('info')
const tabs = [
  { key: 'info', icon: 'ℹ️', label: '设备' },
  { key: 'script', icon: '📜', label: '脚本' }
]

// ---------- 设备管理（融合原设备列表：下拉选择 / 连接 / 手动新增 / 配置编辑） ----------
const vdPresets = [
  { res: '1920x1080', dpi: 420 },
  { res: '1080x1920', dpi: 420 },
  { res: '1280x720', dpi: 320 },
  { res: '2340x1080', dpi: 440 }
]
// 帧率仅提供具体数值（无"自动"选项），默认 30
const fpsPresets = [15, 30, 60, 120]
const types = [
  { key: 'redroid', label: 'redroid 容器', icon: '🐳' },
  { key: 'usb', label: 'USB 直连', icon: '🔌' },
  { key: 'wifi', label: '无线 adb', icon: '📶' },
  { key: 'emu', label: '模拟器', icon: '🖥️' }
]
// 表单状态：'edit' 编辑现有设备 / 'add' 手动新增
// 默认配置：分辨率 1920x1080 · 帧率 30 · DPI 自动（0）
const mode = ref('edit')
// “已自动保存”提示：保存成功后只短暂显示几秒
const savedVisible = ref(false)
let savedTimer = null
const form = reactive({ name: '', kind: 'redroid', addr: '', screen_mode: 'virtual', vd_res: '1920x1080', vd_dpi: 0, pkg: '', fps: 30 })
const scanning = ref(false)
// 配置保存串行化标志：防止连续保存叠加触发多次重连
const configApplying = ref(false)

// 应用下拉（应用选择）
const appList = ref([])
const pkgDraft = ref('')
const appLoading = ref(false)
const appOpen = ref(false)
const appHint = ref('')

const devices = computed(() => devicesData.value)
const scripts = computed(() => scriptsData.value)
const templates = computed(() => templatesData.value)

const current = computed(() => devices.value.find(d => d.id === store.deviceId) || null)
const currentName = computed(() => current.value?.name || '未选择设备')
const currentPkg = computed(() => current.value?.pkg || '')

/** 接入方式展示（新增时可选，编辑时只读徽章） */
function kindInfo(k) {
  return types.find(t => t.key === k) || { key: k, label: k || '未知', icon: '📱' }
}

/** 编辑模式概览里的屏幕摘要（与配置表单区分开，避免重复） */
const screenSummary = computed(() => {
  const d = current.value
  if (!d) return '—'
  if (d.screen_mode === 'virtual') {
    const res = d.vd_res || '1920x1080'
    const dpi = d.vd_dpi ? ` @${d.vd_dpi}dpi` : ' · DPI 自动'
    return `🖥️ 虚拟屏 · ${res}${dpi}`
  }
  return '🖥️ 镜像主屏'
})

const appFiltered = computed(() => {
  const q = (pkgDraft.value || '').trim().toLowerCase()
  return appList.value
    .filter(a => !q || a.label.toLowerCase().includes(q) || a.pkg.toLowerCase().includes(q))
    .slice(0, 50)
})

/** 当前应用列表缓存 key（编辑按设备 id，新增按 ADB 地址） */
function appCacheKey() {
  return mode.value === 'edit' && store.deviceId ? `device:${store.deviceId}` : `addr:${form.addr.trim()}`
}

/** 从缓存恢复应用列表（切换设备时避免重新读取） */
function restoreAppCache(id) {
  const cached = appCache.get(`device:${id}`)
  appList.value = cached?.list || []
  appOpen.value = false
  appHint.value = cached ? `已缓存 ${cached.list.length} 个应用` : ''
}

/** 隐藏“已自动保存”提示（切换设备/取消新增时清除残留） */
function hideSavedHint() {
  savedVisible.value = false
  if (savedTimer) { clearTimeout(savedTimer); savedTimer = null }
}

/** 显示“已自动保存”提示，3 秒后自动消失 */
function showSavedHint() {
  savedVisible.value = true
  if (savedTimer) { clearTimeout(savedTimer); savedTimer = null }
  savedTimer = setTimeout(() => { savedVisible.value = false }, 3000)
}

/** 把设备记录载入表单（编辑模式） */
function loadForm(d) {
  mode.value = 'edit'
  hideSavedHint()
  form.name = d.name || ''
  form.kind = d.kind || 'redroid'
  form.addr = d.addr || ''
  form.screen_mode = d.screen_mode || 'virtual'
  form.vd_res = d.vd_res || '1920x1080'
  form.vd_dpi = d.vd_dpi || 0
  form.pkg = d.pkg || ''
  pkgDraft.value = d.pkg || ''
  form.fps = d.fps || 30
  restoreAppCache(d.id)
}

/** 表单相对已保存配置是否有未保存修改 */
const formDirty = computed(() => {
  const d = current.value
  if (!d || mode.value !== 'edit') return false
  const norm = (v, fb) => (v === '' || v === null || v === undefined ? fb : v)
  // 接入方式 / ADB 地址是新增时确定的连接属性，编辑时只读，不参与 dirty 判断
  return !(
    d.name === norm(form.name, '') &&
    (d.screen_mode || 'virtual') === norm(form.screen_mode, 'virtual') &&
    (d.vd_res || '1920x1080') === norm(form.vd_res, '1920x1080') &&
    Number(d.vd_dpi || 0) === Number(norm(form.vd_dpi, 0)) &&
    (d.pkg || '') === norm(form.pkg, '') &&
    (d.fps || 30) === Number(norm(form.fps, 30))
  )
})

/** 手动新增：重置为默认配置（1920x1080 / 30fps / DPI 自动） */
function startAdd() {
  mode.value = 'add'
  hideSavedHint()
  form.name = ''
  form.kind = 'redroid'
  form.addr = ''
  form.screen_mode = 'virtual'
  form.vd_res = '1920x1080'
  form.vd_dpi = 0
  form.pkg = ''
  pkgDraft.value = ''
  form.fps = 30
  appList.value = []
  appOpen.value = false
  appHint.value = ''
  errorMsg.value = ''
}

/** 取消新增：回到当前已选设备（或空状态） */
function cancelAdd() {
  const d = current.value
  if (d) loadForm(d)
  else {
    mode.value = 'edit'
    hideSavedHint()
    store.deviceId = null
    pkgDraft.value = ''
    appList.value = []
    appHint.value = ''
  }
  errorMsg.value = ''
}

/** 下拉框切换设备：手动断开旧连接（不自动重连），等待用户点连接 */
function onDeviceSelect() {
  if (connected.value || reconnectTimer) {
    if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null }
    cleanup(true)
    releaseLock()
  }
  reconnectAttempts = 0
  errorMsg.value = ''
  const d = current.value
  if (d) loadForm(d)
  else { mode.value = 'edit'; pkgDraft.value = ''; appList.value = []; appHint.value = '' }
}

/** 刷新：扫描 adb 自动入库新设备，再拉列表 */
async function refreshDevices() {
  if (scanning.value) return
  scanning.value = true
  try {
    const r = await api.scanDevices()
    const list = r.devices && Array.isArray(r.devices) ? r.devices : await api.listDevices()
    devicesData.value = list
    // 当前设备已不存在（被删）→ 选中第一台
    if (!list.some(x => x.id === store.deviceId)) {
      store.deviceId = list[0]?.id || null
    }
    const d = current.value
    // 仅编辑模式重新载入表单（不覆盖进行中的"新增"表单）
    if (d && mode.value === 'edit') loadForm(d)
    else if (!d) { mode.value = 'edit'; pkgDraft.value = ''; appList.value = []; appHint.value = '' }
    toast(r.added > 0 ? `扫描到 ${r.added} 台新设备，已自动添加` : '已刷新设备状态', 'success')
  } catch (e) {
    toast('刷新失败：' + e.message, 'error')
  } finally {
    scanning.value = false
  }
}

/** 表单 → 保存 payload（镜像模式不使用虚拟屏参数） */
function buildPayload() {
  return {
    name: form.name.trim(),
    kind: form.kind,
    addr: form.addr.trim(),
    screen_mode: form.screen_mode,
    vd_res: form.screen_mode === 'virtual' ? form.vd_res.trim() : null,
    vd_dpi: form.screen_mode === 'virtual' ? Number(form.vd_dpi) || 0 : null,
    pkg: form.screen_mode === 'virtual' ? (form.pkg.trim() || null) : null,
    fps: Number(form.fps) || 30
  }
}

/** 自动保存：编辑表单（防抖 800ms）→ PUT 更新配置。
 *  未连接时仅保存修改（连接后按新配置生效）；已连接时保存即实时生效
 *  （服务端踢旧 viewer → onclose → 自动重连，画面自动恢复）。
 *  防抖 + 串行化：保存期间/连接期间的新修改延后重试，避免叠加触发多次重连。 */
let saveTimer = null

watch(form, () => {
  if (mode.value !== 'edit' || !current.value) return
  if (saveTimer) clearTimeout(saveTimer)
  saveTimer = setTimeout(autoSaveConfig, 800)
}, { deep: true })

watch(logLevel, () => applyLogFilter())

async function autoSaveConfig() {
  saveTimer = null
  if (mode.value !== 'edit') return
  const d = current.value
  if (!d) return
  // 保存/连接进行中：稍后重试（表单修改仍在，formDirty 保持）
  if (configApplying.value || connecting.value) {
    saveTimer = setTimeout(autoSaveConfig, 600)
    return
  }
  if (!formDirty.value) return
  const payload = buildPayload()
  if (!payload.name) return toast('请填写设备名称', 'error')
  const wasConnected = connected.value
  configApplying.value = true
  try {
    await api.updateDevice(d.id, payload)
    await loadData()
    const nd = devices.value.find(x => x.id === d.id)
    if (nd) loadForm(nd)
    // 服务端已踢掉本页 viewer（config changed, kicked viewer）→ 触发 onclose →
    // 自动重连逻辑（scheduleReconnect）重新建立会话与 WebRTC，无需在此手动重连，
    // 避免与自动重连并发导致双连接
    toast(wasConnected ? '配置已保存，正在自动重连生效…' : '配置已保存', 'success')
    showSavedHint()
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  } finally {
    configApplying.value = false
  }
}

/** 点连接前先落盘未保存的配置（防抖未到期时），再建立连接 */
async function flushAndConnect() {
  if (mode.value === 'add') return toast('请先取消新增或选择已有设备', 'warn')
  if (mode.value !== 'edit' || !current.value) return connect(true)
  if (saveTimer) { clearTimeout(saveTimer); saveTimer = null }
  if (configApplying.value) return toast('配置保存中，请稍候再连接', 'warn')
  if (!formDirty.value) return connect(true)
  await autoSaveConfig()
  if (formDirty.value) return toast('配置尚未保存成功，请稍后重试', 'error')
  connect(true)
}

/** 手动新增设备（POST 返回 id，创建后自动选中） */
async function addDevice() {
  const payload = buildPayload()
  if (!payload.name) return toast('请填写设备名称', 'error')
  try {
    const r = await api.createDevice(payload)
    await loadData()
    // 新增成功后切换到新设备：先断开旧设备连接，避免画面/控制仍指向旧设备
    if (connected.value || reconnectTimer) {
      if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null }
      cleanup(true)
      releaseLock()
    }
    store.deviceId = r.id
    const nd = devices.value.find(x => x.id === r.id)
    if (nd) loadForm(nd)
    toast('设备已添加，点击连接开始投屏', 'success')
  } catch (e) {
    toast('添加失败：' + e.message, 'error')
  }
}

async function removeDevice() {
  const d = current.value
  if (!d) return
  if (!confirm(`确定删除设备 ${d.name}？`)) return
  try {
    await api.deleteDevice(d.id)
    if (connected.value || reconnectTimer) {
      if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null }
      cleanup(true)
    }
    releaseLock()
    devicesData.value = devices.value.filter(x => x.id !== d.id)
    if (devices.value.length) {
      store.deviceId = devices.value[0].id
      loadForm(devices.value[0])
    } else {
      store.deviceId = null
      startAdd()
    }
    toast('设备已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

/** 主动断开（停止本页 WebRTC + 服务端会话，不触发自动重连） */
function disconnect() {
  if (!store.deviceId) return
  if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null }
  cleanup(true)
  releaseLock()
  api.disconnectDevice(store.deviceId).catch(() => {})
  toast('已断开连接', 'info')
}

/** 从设备读取已安装应用（scrcpy list_apps，带真实软件名），带缓存避免重复读取 */
async function loadApps() {
  if (appLoading.value) return
  const key = appCacheKey()
  const cached = appCache.get(key)
  // 5 分钟内直接用缓存，应用列表不是经常变
  if (cached && Date.now() - cached.ts < APP_CACHE_TTL) {
    appList.value = cached.list
    appHint.value = cached.list.length ? `已加载缓存（共 ${cached.list.length} 个应用）` : '设备上未发现第三方应用（缓存）'
    return
  }
  appLoading.value = true
  appHint.value = '正在读取设备应用…'
  try {
    const list = mode.value === 'edit' && store.deviceId
      ? await api.listApps(store.deviceId)
      : await api.listAppsByAddr(form.addr.trim())
    appList.value = list || []
    appCache.set(key, { list: appList.value, ts: Date.now() })
    appHint.value = appList.value.length ? `共 ${appList.value.length} 个应用，输入关键字搜索` : '设备上未发现第三方应用'
  } catch (e) {
    appList.value = []
    appHint.value = '读取失败：' + e.message + '（可直接手动输入包名后回车确认）'
  } finally {
    appLoading.value = false
  }
}

function pickApp(a) {
  form.pkg = a.pkg
  pkgDraft.value = a.pkg
  appOpen.value = false
}

/** 手动输入包名不会自动保存；按回车确认后才写入配置并触发保存 */
function commitPkg() {
  const pkg = pkgDraft.value.trim()
  form.pkg = pkg
  appOpen.value = false
}

const selStyle = computed(() => ({
  left: Math.min(selStart.x, selEnd.x) + 'px',
  top: Math.min(selStart.y, selEnd.y) + 'px',
  width: Math.abs(selEnd.x - selStart.x) + 'px',
  height: Math.abs(selEnd.y - selStart.y) + 'px'
}))

const hitStyle = computed(() => {
  const vw = videoWrap.value
  if (!vw) return {}
  const rect = vw.getBoundingClientRect()
  const vw_ = rect.width, vh = rect.height
  const sw = videoElement.value?.videoWidth || 1920
  const sh = videoElement.value?.videoHeight || 1080
  const ratio = Math.min(vw_ / sw, vh / sh)
  const w = hit.w * ratio, h = hit.h * ratio
  const x = (hit.x * ratio) + (vw_ - sw * ratio) / 2
  const y = (hit.y * ratio) + (vh - sh * ratio) / 2
  return { left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' }
})

/** alt 模式点击圆点位置（设备坐标 → 显示坐标） */
const altTapStyle = computed(() => {
  if (!altFeedback.show || altFeedback.kind !== 'tap') return {}
  const vw = videoWrap.value
  if (!vw) return {}
  const rect = vw.getBoundingClientRect()
  const vw_ = rect.width, vh = rect.height
  const sw = videoElement.value?.videoWidth || 1920
  const sh = videoElement.value?.videoHeight || 1080
  const ratio = Math.min(vw_ / sw, vh / sh)
  const x = (altFeedback.x * ratio) + (vw_ - sw * ratio) / 2
  const y = (altFeedback.y * ratio) + (vh - sh * ratio) / 2
  return { left: x + 'px', top: y + 'px' }
})

/** alt 模式滑动 region 框位置（设备坐标 → 显示坐标） */
const altFeedbackStyle = computed(() => {
  if (!altFeedback.show || altFeedback.kind !== 'region') return {}
  const vw = videoWrap.value
  if (!vw) return {}
  const rect = vw.getBoundingClientRect()
  const vw_ = rect.width, vh = rect.height
  const sw = videoElement.value?.videoWidth || 1920
  const sh = videoElement.value?.videoHeight || 1080
  const ratio = Math.min(vw_ / sw, vh / sh)
  const w = altFeedback.w * ratio
  const h = altFeedback.h * ratio
  const x = (altFeedback.x * ratio) + (vw_ - sw * ratio) / 2
  const y = (altFeedback.y * ratio) + (vh - sh * ratio) / 2
  return { left: x + 'px', top: y + 'px', width: w + 'px', height: h + 'px' }
})

async function loadData() {
  try {
    devicesData.value = await api.listDevices()
  } catch (e) { console.warn('load devices:', e.message) }
  try {
    scriptsData.value = await api.listScripts()
  } catch (e) { console.warn('load scripts:', e.message) }
  try {
    templatesData.value = await api.listTemplates()
  } catch (e) { console.warn('load templates:', e.message) }
}

// ---------- WebRTC 连接 ----------

// 声音默认静音（虚拟屏音频已接入 WebRTC，用户可点工具栏 🔊 按钮开启）
const audioMuted = ref(true)
function toggleAudio() {
  audioMuted.value = !audioMuted.value
  const v = videoElement.value
  if (v) {
    v.muted = audioMuted.value
    // 取消静音时浏览器要求用户手势后播放（已处于点击事件内，直接 play 即可）
    if (!audioMuted.value) v.play().catch(() => {})
  }
}

async function connect(force = false) {
  // 幂等：同步锁 + 状态检查，杜绝并发/重复调用创建多个 PC
  // （服务端会因多连接出现多推流，video.srcObject 被串流覆盖 → 画面定格）
  if (connectLock || connecting.value || connected.value) {
    console.warn('[webrtc] connect ignored (lock/connecting/connected)')
    return
  }
  // force=true 仅限用户手动操作（点连接按钮）：强制抢锁；
  // 自动重连（force=false）不抢锁——锁在他人手里时已由 scheduleReconnect 拦截
  if (!acquireLock(force)) {
    errorMsg.value = '设备正在其他页面使用'
    return
  }
  connectLock = true
  console.log('[webrtc] connect called (pc exists:', !!pc, ')')
  try {
    await doConnect()
  } finally {
    connectLock = false
  }
}

async function doConnect() {
  if (!store.deviceId) return toast('请先选择设备（设备页签下拉框）', 'error')
  // 重连场景：若有残留 pc（连接失败但未清理干净），先释放（主动关闭，不触发自动重连）
  if (pc) cleanup(true)
  errorMsg.value = ''
  connecting.value = true

  try {
    // 1. 服务端建立 scrcpy 会话
    await api.connectDevice(store.deviceId)
  } catch (e) {
    connecting.value = false
    errorMsg.value = '设备连接失败：' + e.message
    return
  }

  try {
    // 2. 信令 WebSocket
    const wsProto = location.protocol === 'https:' ? 'wss:' : 'ws:'
    ws = new WebSocket(`${wsProto}//${location.host}/ws/device/${store.deviceId}`)

    await new Promise((resolve, reject) => {
      ws.onopen = resolve
      ws.onerror = () => reject(new Error('信令连接失败'))
    })

    // 3. 创建 PeerConnection（接收视频轨 + 控制 DataChannel）
    pc = new RTCPeerConnection({ iceServers: [{ urls: 'stun:stun.l.google.com:19302' }] })
    // 调试：统计本页创建的 PC 数量（验证无双连接）
    window.__pcCount = (window.__pcCount || 0) + 1
    console.log('[webrtc] PC #' + window.__pcCount)
    pc.addTransceiver('video', { direction: 'recvonly' })
    pc.addTransceiver('audio', { direction: 'recvonly' })

    // 控制 DataChannel 必须由 offerer 创建：否则 offer 里没有 m=application，
    // answer 也不会有（webrtc-rs 只镜像 offer 的 media section），SCTP 永不建立
    controlChannel = pc.createDataChannel('control')
    controlChannel.onopen = onChannelOpen
    controlChannel.onclose = onChannelClose

    pc.ontrack = (e) => {
      // 只接受当前 pc 的轨道：残留/旧连接的 ontrack 不得覆盖 srcObject（串流 → 定格）
      if (e.target !== pc) return
      // 兜底：对端 SDP 无 a=msid 时 e.streams 可能为空，用 track 自建 MediaStream
      mediaStream = e.streams[0] || new MediaStream([e.track])
      if (videoElement.value) {
        videoElement.value.srcObject = mediaStream
        videoElement.value.play().catch(() => {})
      }
      console.log('[webrtc] ontrack', e.track.kind, 'streams=', e.streams.length, 'codec=', e.track.getSettings?.())
      // 视频元信息
      const v = e.track
      v.addEventListener('unmute', () => {
        setTimeout(() => {
          const w = videoElement.value?.videoWidth || 0
          const h = videoElement.value?.videoHeight || 0
          if (w) res.value = `${w}x${h}`
        }, 200)
      })
    }

    pc.ondatachannel = (e) => {
      controlChannel = e.channel
      controlChannel.onopen = onChannelOpen
      controlChannel.onclose = onChannelClose
    }

    // 4. offer 交换
    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    const answer = await new Promise((resolve, reject) => {
      ws.onmessage = (evt) => {
        try {
          const msg = JSON.parse(evt.data)
          if (msg.type === 'answer') resolve(msg.sdp)
          else if (msg.type === 'error') reject(new Error(msg.error || '信令错误'))
        } catch (e) { reject(e) }
      }
      ws.send(JSON.stringify({ type: 'offer', sdp: offer }))
      setTimeout(() => reject(new Error('信令超时')), 10000)
    })
    await pc.setRemoteDescription(new RTCSessionDescription(answer))

    // 5. 统计定时器
    startStats()
    startLogPolling()
  } catch (e) {
    console.error('webrtc connect:', e)
    connecting.value = false
    errorMsg.value = e.message
    cleanup(true)
  }
}

/** 释放 WebRTC 资源；manual=true 表示主动关闭（不触发自动重连） */
function cleanup(manual = false) {
  if (statsTimer) { clearInterval(statsTimer); statsTimer = null }
  if (logTimer) { clearInterval(logTimer); logTimer = null }
  stopLockHeartbeat()
  if (pc) {
    manualClose = manual // 主动关闭时标记：pc.close() 触发的 onclose 不触发自动重连
    try { pc.close() } catch (e) {}
    pc = null
  }
  if (ws) { try { ws.close() } catch (e) {} ws = null }
  controlChannel = null
  mediaStream = null
  connected.value = false
  hadVideo = false
  stillFrames = 0
  lastBytesReceived = 0
  lastBitrateTs = 0
  bitrate.value = '—'
  // 清空画面：断开后避免旧帧定格残留（overlay 会显示连接提示）
  if (videoElement.value) videoElement.value.srcObject = null
  hideLoupe()
}

// ---------- 视频静默检测 ----------
// 被踢/断流时服务端不再向本页推帧（或已推给其他页面），video.currentTime 停止推进
// → 判定断流，走与 onclose 相同的自动重连逻辑（带页面锁检查）。
// 服务端有静止补帧（RTP 时间戳单调推进），正常连接下 currentTime 持续前进，
// 静止画面不会误判；仅真正无帧流入（被踢/断流/看门狗重建窗口）才会触发。
let lastVideoTime = 0
let stillFrames = 0
let hadVideo = false
// 传输码率统计（按两次 getStats 的 bytesReceived 差值计算）
let lastBytesReceived = 0
let lastBitrateTs = 0
// 画面延迟统计：jitterBufferDelay 增量 / 新播出帧数 = 每帧在 jitter buffer 的平均停留
// 时间（≈ 画面滞后于设备的时间下限；服务端推流节奏正常时 ~100-300ms）
let lastJbd = 0
let lastJbe = 0
// 连接建立时间：用于"连接后长时间无视频帧（黑屏）"看门狗
let videoConnectTs = 0

function handleVideoSilence() {
  if (manualClose || !connected.value || !store.deviceId) return
  console.warn('[webrtc] video stream silent, treating as disconnected')
  connected.value = false
  stopLockHeartbeat()
  scheduleReconnect()
}

/** 格式化传输码率 */
function formatBitrate(bps) {
  if (!bps || bps <= 0) return '—'
  if (bps >= 1000000) return (bps / 1000000).toFixed(1) + ' Mbps'
  if (bps >= 1000) return Math.round(bps / 1000) + ' Kbps'
  return Math.round(bps) + ' bps'
}

function startStats() {
  if (statsTimer) clearInterval(statsTimer)
  statsTimer = setInterval(async () => {
    if (!pc) return
    const v = videoElement.value
    // 黑屏看门狗：连接建立后 8s 内一直没有可解码视频帧（videoWidth 仍为 0，
    // 如服务端未重放出 SPS/PPS+IDR）→ 判定异常，自动重连（重连时服务端会
    // 强制设备出关键帧并重放初始帧，恢复画面）
    if (connected.value && v && !hadVideo && v.videoWidth === 0 && Date.now() - videoConnectTs > 8000) {
      console.warn('[webrtc] no decodable video after 8s, reconnecting')
      handleVideoSilence()
      return
    }
    // 视频静默检测：仅在见过画面后启用（连接初期 currentTime=0 不误判）
    if (connected.value && v && v.videoWidth > 0) {
      hadVideo = true
      if (Math.abs(v.currentTime - lastVideoTime) < 0.001) {
        if (++stillFrames >= 2) { // 连续 ~4s 无新帧
          stillFrames = 0
          handleVideoSilence()
        }
      } else {
        stillFrames = 0
        lastVideoTime = v.currentTime
      }
    }
    try {
      const stats = await pc.getStats()
      let fpsCount = 0
      stats.forEach(s => {
        if (s.type === 'inbound-rtp' && s.kind === 'video') {
          if (s.framesPerSecond) fpsCount = Math.round(s.framesPerSecond)
          // 画面延迟：jitterBufferDelay 规范单位为秒（个别 Chromium 版本报 ms，自适应：
          // 单帧均值 >50s 视为 ms 直读，否则按秒换算）。只统计增量窗口，避免累计均值失真
          if (typeof s.jitterBufferDelay === 'number' && s.jitterBufferEmittedCount > 0) {
            if (lastJbe > 0 && s.jitterBufferEmittedCount > lastJbe) {
              const perFrame = (s.jitterBufferDelay - lastJbd) / (s.jitterBufferEmittedCount - lastJbe)
              if (perFrame >= 0 && perFrame < 50) {
                delay.value = Math.round(perFrame * 1000)
              }
            }
            lastJbd = s.jitterBufferDelay
            lastJbe = s.jitterBufferEmittedCount
          }
          // 传输码率：按字节增量 / 时间增量估算
          if (typeof s.bytesReceived === 'number') {
            const now = Date.now()
            if (lastBytesReceived > 0 && lastBitrateTs > 0) {
              const dt = (now - lastBitrateTs) / 1000
              if (dt > 0) bitrate.value = formatBitrate(((s.bytesReceived - lastBytesReceived) * 8) / dt)
            }
            lastBytesReceived = s.bytesReceived
            lastBitrateTs = now
          }
          // 诊断：每 3 次打印一次接收统计
          if (!window.__rtpStatsCount) window.__rtpStatsCount = 0
          if (++window.__rtpStatsCount % 3 === 0) {
            const v = videoElement.value
            console.log('[webrtc] inbound-rtp', JSON.stringify({
              bytesReceived: s.bytesReceived, packetsReceived: s.packetsReceived,
              framesDecoded: s.framesDecoded, framesDropped: s.framesDropped,
              framesPerSecond: s.framesPerSecond, keyFramesDecoded: s.keyFramesDecoded,
              pliCount: s.pliCount, nackCount: s.nackCount,
              codecId: s.codecId, decoder: s.decoderImplementation,
              videoWidth: v?.videoWidth, videoHeight: v?.videoHeight, readyState: v?.readyState
            }))
          }
        }
      })
      if (fpsCount) fps.value = fpsCount
    } catch (e) {}
  }, 2000)
}

function parseLogTime(s) {
  if (!s) return 0
  const d = new Date(s.replace(' ', 'T'))
  return d.getTime() || 0
}

function scrollLogsToBottom() {
  nextTick(() => {
    const el = logBox.value
    if (el) el.scrollTop = el.scrollHeight
  })
}

function applyLogFilter() {
  const filtered = (rawLogs || []).filter(l => {
    // info 模式只显示 YAML log（level=info）；debug 模式显示全部日志
    if (logLevel.value === 'info' && l.level !== 'info') return false
    if (runStartTime && parseLogTime(l.time) < runStartTime) return false
    return true
  })
  liveLogs.value = filtered.map(l => ({ time: l.time.slice(11, 23), level: l.level, msg: l.msg })).reverse()
  scrollLogsToBottom()
}

async function refreshLogs() {
  if (!store.deviceId) return
  try {
    const logs = await api.listLogs(store.deviceId, null, 50)
    rawLogs = logs || []
    applyLogFilter()
  } catch (e) {}
}

function startLogPolling() {
  if (logTimer) clearInterval(logTimer)
  refreshLogs()
  logTimer = setInterval(refreshLogs, 1000)
}

// ---------- 控制（走 DataChannel） ----------

function sendControl(obj) {
  if (controlChannel && controlChannel.readyState === 'open') {
    controlChannel.send(JSON.stringify(obj))
    return true
  }
  console.warn('[control] channel not open, fallback REST', JSON.stringify(obj))
  // fallback：REST API
  api.control(store.deviceId, obj).catch(e => toast('控制失败：' + e.message, 'error'))
  return false
}

/** 鼠标坐标 → 设备坐标（object-fit: contain 换算） */
function toDeviceCoord(clientX, clientY) {
  const video = videoElement.value
  const rect = video.getBoundingClientRect()
  const vw = video.videoWidth || 1920
  const vh = video.videoHeight || 1080
  const ratio = Math.min(rect.width / vw, rect.height / vh)
  const dispW = vw * ratio, dispH = vh * ratio
  const offX = (rect.width - dispW) / 2, offY = (rect.height - dispH) / 2
  const x = Math.round((clientX - rect.left - offX) / dispW * vw)
  const y = Math.round((clientY - rect.top - offY) / dispH * vh)
  return { x: Math.max(0, Math.min(vw, x)), y: Math.max(0, Math.min(vh, y)) }
}

// 触控状态
const touchState = reactive({ active: false, lastX: 0, lastY: 0 })

// 拖动 move 事件合并：鼠标高频事件（数百 Hz）逐条发送会打爆 DataChannel/服务端日志，
// 这里按 rAF（约 60Hz）合并发送，拖拽手感不受影响，但延迟和负载大幅下降。
let pendingMove = null
let moveRaf = 0
function flushPendingMove() {
  moveRaf = 0
  if (pendingMove) {
    const p = pendingMove
    pendingMove = null
    sendControl(p)
  }
}
function scheduleMove(x, y) {
  pendingMove = { type: 'touch', action: 'move', x, y }
  if (!moveRaf) moveRaf = requestAnimationFrame(flushPendingMove)
}
function cancelPendingMove() {
  if (moveRaf) { cancelAnimationFrame(moveRaf); moveRaf = 0 }
  pendingMove = null
}

function onMouseDown(e) {
  // alt 模式/按住 Alt：点击/滑动只生成操作记录，不发送控制指令
  if (isAltAction(e) && connected.value) {
    const { x, y } = toDeviceCoord(e.clientX, e.clientY)
    altGesture.active = true
    altGesture.moved = false
    altGesture.start = { x, y }
    altGesture.last = { x, y }
    // 先显示点击位置，滑动时再切换成 region 框
    if (altFeedbackTimer) clearTimeout(altFeedbackTimer)
    altFeedback.show = true
    altFeedback.kind = 'tap'
    altFeedback.x = x
    altFeedback.y = y
    altFeedback.w = 0
    altFeedback.h = 0
    return
  }
  if (picking.value && connected.value) {
    const rect = videoWrap.value.getBoundingClientRect()
    selStart.x = e.clientX - rect.left
    selStart.y = e.clientY - rect.top
    selEnd.x = selStart.x; selEnd.y = selStart.y
    selecting.value = true
    return
  }
  if (!connected.value) return
  cancelPendingMove()
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  touchState.active = true
  touchState.lastX = x; touchState.lastY = y
  // 按下：发 DOWN（拖动时后续 move 事件组成轨迹，up 时收尾）
  sendControl({ type: 'touch', action: 'down', x, y })
}

function onMouseMove(e) {
  if (altGesture.active) {
    const { x, y } = toDeviceCoord(e.clientX, e.clientY)
    if (Math.abs(x - altGesture.last.x) + Math.abs(y - altGesture.last.y) > 6) {
      altGesture.last = { x, y }
      altGesture.moved = true
    }
    // 拖动时实时显示 region 框（起点 → 当前点）
    if (altGesture.moved) {
      altFeedback.show = true
      altFeedback.kind = 'region'
      altFeedback.x = Math.min(altGesture.start.x, x)
      altFeedback.y = Math.min(altGesture.start.y, y)
      altFeedback.w = Math.abs(x - altGesture.start.x)
      altFeedback.h = Math.abs(y - altGesture.start.y)
    }
    return
  }
  if (selecting.value) {
    const rect = videoWrap.value.getBoundingClientRect()
    selEnd.x = e.clientX - rect.left
    selEnd.y = e.clientY - rect.top
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 2.5, [selToDeviceRect()])
    return
  }
  if (picking.value) {
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 2.5, [])
    return
  }
  if (!touchState.active || !connected.value) return
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  if (Math.abs(x - touchState.lastX) + Math.abs(y - touchState.lastY) > 6) {
    touchState.lastX = x; touchState.lastY = y
    scheduleMove(x, y)
  }
}

function togglePick() {
  confirmDelTpl.value = null
  if (!connected.value) return toast('请先连接设备', 'error')
  picking.value = !picking.value
  if (!picking.value) hideLoupe()
}

function onMouseUp(e) {
  if (altGesture.active) {
    const { x, y } = toDeviceCoord(e.clientX, e.clientY)
    const start = altGesture.start
    const moved = altGesture.moved || Math.hypot(x - start.x, y - start.y) > 8
    altGesture.active = false
    if (moved) setSwipeRecords(start, { x, y })
    else setTapRecord({ x, y })
    return
  }
  if (selecting.value) {
    selecting.value = false
    picking.value = false
    hideLoupe()
    const rect = selToDeviceRect()
    if (rect.w >= 8 && rect.h >= 8) openCrop(rect)
    else toast('框选区域太小，请重新框选', 'warn')
    return
  }
  if (!touchState.active) return
  cancelPendingMove()
  touchState.active = false
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  sendControl({ type: 'touch', action: 'up', x, y })
}

/** 鼠标离开投屏区域时终止未完成的 alt 手势，避免卡在记录模式 */
function onVideoMouseLeave() {
  hideLoupe()
  if (altGesture.active) altGesture.active = false
  if (altFeedbackTimer) clearTimeout(altFeedbackTimer)
  altFeedback.show = false
}

// ---------- 框选保存模板 ----------

/** 框选矩形（容器 CSS 坐标）→ 设备像素坐标，自动裁剪 letterbox 黑边并夹取到画面内 */
function selToDeviceRect() {
  const video = videoElement.value
  const vw = video?.videoWidth || 1920
  const vh = video?.videoHeight || 1080
  const rect = videoWrap.value.getBoundingClientRect()
  const ratio = Math.min(rect.width / vw, rect.height / vh)
  const dispW = vw * ratio, dispH = vh * ratio
  const offX = (rect.width - dispW) / 2, offY = (rect.height - dispH) / 2
  const toDev = p => ({ x: (p.x - offX) / dispW * vw, y: (p.y - offY) / dispH * vh })
  const p1 = toDev(selStart), p2 = toDev(selEnd)
  const x = Math.round(Math.min(p1.x, p2.x)), y = Math.round(Math.min(p1.y, p2.y))
  const w = Math.round(Math.abs(p2.x - p1.x)), h = Math.round(Math.abs(p2.y - p1.y))
  const cx = Math.max(0, Math.min(vw, x)), cy = Math.max(0, Math.min(vh, y))
  return { x: cx, y: cy, w: Math.min(w, vw - cx), h: Math.min(h, vh - cy) }
}

function randomTplBase() {
  return 'tpl_' + Math.random().toString(36).slice(2, 8)
}

/** 生成默认模板名：随机名字#x1_y1_x2_y2（相对坐标 0~1，去掉小数点、固定 4 位，不带 .png 后缀） */
function defaultTplName(rect) {
  const vw = videoElement.value?.videoWidth || 1920
  const vh = videoElement.value?.videoHeight || 1080
  const toFixed4 = v => String(Math.round(v * 10000)).padStart(4, '0')
  const x1 = toFixed4(rect.x / vw)
  const y1 = toFixed4(rect.y / vh)
  const x2 = toFixed4((rect.x + rect.w) / vw)
  const y2 = toFixed4((rect.y + rect.h) / vh)
  return `${randomTplBase()}#${x1}_${y1}_${x2}_${y2}`
}

// ---------- 二次裁切 ----------

const cropSize = computed(() => `${Math.round(crop.rect.w)}×${Math.round(crop.rect.h)} px`)

/** 框选完成后打开右侧裁切区 */
function openCrop(rect) {
  confirmDelTpl.value = null
  const video = videoElement.value
  if (!video?.videoWidth) return toast('无法截取画面，请稍后重试', 'error')
  crop.imgW = video.videoWidth
  crop.imgH = video.videoHeight
  crop.originX = Math.round(rect.x)
  crop.originY = Math.round(rect.y)
  crop.baseW = Math.round(rect.w)
  crop.baseH = Math.round(rect.h)
  // 冻结初始框选画面，二次裁切时底图不动，只动遮罩框
  cropBaseCanvas = document.createElement('canvas')
  cropBaseCanvas.width = crop.baseW
  cropBaseCanvas.height = crop.baseH
  cropBaseCanvas.getContext('2d').drawImage(video, crop.originX, crop.originY, crop.baseW, crop.baseH, 0, 0, crop.baseW, crop.baseH)
  crop.rect = { x: 0, y: 0, w: crop.baseW, h: crop.baseH }
  crop.name = defaultTplName(rect)
  crop.active = true
  activeTab.value = 'script'
  nextTick(() => {
    renderCropFrame()
    refreshCropPreview()
    cropSec.value?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  })
}

function cancelCrop() {
  crop.active = false
  cropBaseCanvas = null
  hideLoupe()
}

function repick() {
  crop.active = false
  cropBaseCanvas = null
  picking.value = true
  toast('在画面上重新框选', 'info')
}

/** 画布适配尺寸：展示冻结的初始框选画面，可适当放大 */
function cropFit() {
  const w = Math.max(1, crop.baseW)
  const h = Math.max(1, crop.baseH)
  const scale = Math.min(260 / w, 220 / h, 3)
  return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)), scale: Math.round(w * scale) / w }
}

/** 在裁切画布上绘制冻结的框选画面 + 可拖动的遮罩框（拖动时只改框，不动底图） */
function renderCropFrame() {
  const canvas = cropCanvas.value
  const base = cropBaseCanvas
  if (!canvas || !base || base.width < 1 || crop.rect.w < 1 || crop.rect.h < 1) return
  const bw = base.width
  const bh = base.height
  const fit = cropFit()
  canvas.width = fit.w
  canvas.height = fit.h
  canvas.style.width = fit.w + 'px'
  canvas.style.height = fit.h + 'px'
  const ctx = canvas.getContext('2d')
  ctx.clearRect(0, 0, fit.w, fit.h)
  ctx.drawImage(base, 0, 0, bw, bh, 0, 0, fit.w, fit.h)

  const sx = fit.w / bw
  const sy = fit.h / bh
  const rx = crop.rect.x * sx
  const ry = crop.rect.y * sy
  const rw = crop.rect.w * sx
  const rh = crop.rect.h * sy

  // 遮罩框外压暗，拖动时只改变这个遮罩
  ctx.fillStyle = 'rgba(0,0,0,.45)'
  ctx.fillRect(0, 0, fit.w, ry)
  ctx.fillRect(0, ry + rh, fit.w, Math.max(0, fit.h - ry - rh))
  ctx.fillRect(0, ry, rx, rh)
  ctx.fillRect(rx + rw, ry, Math.max(0, fit.w - rx - rw), rh)

  // 边框
  ctx.strokeStyle = 'rgba(34,211,165,.95)'
  ctx.lineWidth = 1.5
  ctx.strokeRect(rx, ry, rw, rh)

  // 角点手柄
  ctx.fillStyle = '#fff'
  const hs = 5
  for (const [hx, hy] of [[rx, ry], [rx + rw, ry], [rx, ry + rh], [rx + rw, ry + rh]]) {
    ctx.fillRect(hx - hs / 2, hy - hs / 2, hs, hs)
  }

  // 尺寸标注
  ctx.fillStyle = 'rgba(34,211,165,.95)'
  ctx.font = '10px monospace'
  ctx.fillText(cropSize.value, rx + 6, ry + 14)
}

/** 按当前遮罩框从冻结底图重新生成裁剪结果预览（全分辨率） */
function refreshCropPreview() {
  const base = cropBaseCanvas
  if (!base || base.width < 1) return
  const r = crop.rect
  if (r.w < 1 || r.h < 1) return
  const canvas = document.createElement('canvas')
  canvas.width = Math.round(r.w)
  canvas.height = Math.round(r.h)
  canvas.getContext('2d').drawImage(base, r.x, r.y, r.w, r.h, 0, 0, Math.round(r.w), Math.round(r.h))
  crop.preview = canvas.toDataURL('image/png')
}

/** 鼠标事件 → 冻结底图上的像素坐标 */
function cropEventDev(e) {
  const canvas = cropCanvas.value
  const rect = canvas.getBoundingClientRect()
  const scale = canvas.width / crop.baseW
  return {
    x: (e.clientX - rect.left) / scale,
    y: (e.clientY - rect.top) / scale
  }
}

function cropMouseDown(e) {
  const p = cropEventDev(e)
  const r = crop.rect
  const HIT = 12 / (cropCanvas.value.width / crop.baseW) // 底图像素命中半径
  const corners = { nw: [r.x, r.y], ne: [r.x + r.w, r.y], sw: [r.x, r.y + r.h], se: [r.x + r.w, r.y + r.h] }
  let mode = null
  for (const [k, [hx, hy]] of Object.entries(corners)) {
    if (Math.hypot(p.x - hx, p.y - hy) <= HIT) { mode = k; break }
  }
  if (!mode) {
    const edges = {
      n: [r.x + r.w / 2, r.y], s: [r.x + r.w / 2, r.y + r.h],
      w: [r.x, r.y + r.h / 2], e: [r.x + r.w, r.y + r.h / 2]
    }
    for (const [k, [hx, hy]] of Object.entries(edges)) {
      const onSeg = (k === 'n' || k === 's') ? (p.x >= r.x && p.x <= r.x + r.w) : (p.y >= r.y && p.y <= r.y + r.h)
      if (Math.hypot(p.x - hx, p.y - hy) <= HIT && onSeg) { mode = k; break }
    }
  }
  if (!mode && p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h) mode = 'move'
  if (!mode) return
  cropDrag.mode = mode
  cropDrag.startX = p.x
  cropDrag.startY = p.y
  cropDrag.orig = { ...r }
  e.preventDefault()
}

function cropMouseMove(e) {
  const p = cropEventDev(e)
  // 放大镜仍按完整画面坐标显示（底图坐标 + 初始框选偏移）
  updateLoupe(e.clientX, e.clientY, { x: p.x + crop.originX, y: p.y + crop.originY }, 3, [{ x: crop.rect.x + crop.originX, y: crop.rect.y + crop.originY, w: crop.rect.w, h: crop.rect.h }])
  if (!cropDrag.mode) return
  const o = cropDrag.orig
  const r = crop.rect
  const MIN = 8
  const dx = p.x - cropDrag.startX
  const dy = p.y - cropDrag.startY
  const clamp = (v, lo, hi) => Math.max(lo, Math.min(hi, v))
  switch (cropDrag.mode) {
    case 'move':
      r.x = clamp(o.x + dx, 0, crop.baseW - o.w)
      r.y = clamp(o.y + dy, 0, crop.baseH - o.h)
      break
    case 'nw':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.w = o.x + o.w - r.x; r.h = o.y + o.h - r.y
      break
    case 'ne':
      r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.h = o.y + o.h - r.y
      break
    case 'sw':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.w = o.x + o.w - r.x
      r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
      break
    case 'se':
      r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
      r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
      break
    case 'n':
      r.y = clamp(o.y + dy, 0, o.y + o.h - MIN)
      r.h = o.y + o.h - r.y
      break
    case 's':
      r.h = clamp(o.h + dy, MIN, crop.baseH - o.y)
      break
    case 'w':
      r.x = clamp(o.x + dx, 0, o.x + o.w - MIN)
      r.w = o.x + o.w - r.x
      break
    case 'e':
      r.w = clamp(o.w + dx, MIN, crop.baseW - o.x)
      break
  }
  renderCropFrame()
}

function cropMouseUp() {
  if (!cropDrag.mode) return
  cropDrag.mode = null
  refreshCropPreview()
}

function cropMouseLeave() {
  hideLoupe()
  if (cropDrag.mode) { cropDrag.mode = null; refreshCropPreview() }
}

async function saveTemplate() {
  const raw = crop.name.trim()
  if (!raw) return toast('请输入模板名称', 'warn')
  const name = raw.toLowerCase().endsWith('.png') ? raw : raw + '.png'
  saving.value = true
  try {
    await api.uploadTemplate(name, crop.preview.split(',')[1])
    templatesData.value = await api.listTemplates()
    crop.active = false
    cropBaseCanvas = null
    hideLoupe()
    toast(`模板 ${name} 已保存`, 'success')
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  } finally {
    saving.value = false
  }
}

// ---------- 放大预览镜 ----------

/** 以光标为中心放大当前视频帧：devPt 为放大中心（设备像素），rects 为要叠加显示的选区（设备像素坐标） */
function updateLoupe(clientX, clientY, devPt, zoom, rects) {
  const video = videoElement.value
  const canvas = loupeCanvas.value
  if (!video?.videoWidth || !canvas) return
  const c = devPt
  const L = canvas.width
  const half = L / zoom / 2
  const ctx = canvas.getContext('2d')
  ctx.clearRect(0, 0, L, L)
  ctx.imageSmoothingEnabled = true
  ctx.drawImage(video, c.x - half, c.y - half, half * 2, half * 2, 0, 0, L, L)
  // 十字准星：贯穿全幅的长线
  ctx.strokeStyle = 'rgba(255,255,255,.3)'
  ctx.lineWidth = 1
  ctx.beginPath()
  ctx.moveTo(L / 2, 0); ctx.lineTo(L / 2, L)
  ctx.moveTo(0, L / 2); ctx.lineTo(L, L / 2)
  ctx.stroke()
  // 中心点
  ctx.fillStyle = 'rgba(255,255,255,.95)'
  ctx.beginPath()
  ctx.arc(L / 2, L / 2, 2, 0, Math.PI * 2)
  ctx.fill()
  // 选区轮廓
  ctx.strokeStyle = 'rgba(34,211,165,.95)'
  ctx.lineWidth = 1.5
  for (const r of rects || []) {
    ctx.strokeRect((r.x - (c.x - half)) * zoom, (r.y - (c.y - half)) * zoom, r.w * zoom, r.h * zoom)
  }
  // 定位：跟随光标，越界自动翻转
  loupe.zoom = zoom
  loupe.show = true
  const W = 160, G = 14
  let x = clientX + G, y = clientY + G
  if (x + W > window.innerWidth - 6) x = clientX - W - G
  if (y + W > window.innerHeight - 6) y = clientY - W - G
  loupe.x = Math.max(6, x)
  loupe.y = Math.max(6, y)
}

function hideLoupe() { loupe.show = false }

function onWheel(e) {
  if (!connected.value) return
  const { x, y } = toDeviceCoord(e.clientX, e.clientY)
  sendControl({ type: 'scroll', x, y, scroll_x: e.deltaX, scroll_y: e.deltaY })
}

function key(k) {
  if (!connected.value) return
  const codes = { HOME: 3, BACK: 4, APP_SWITCH: 187, VOL_UP: 24, VOL_DOWN: 25 }
  sendControl({ type: 'press', keycode: codes[k] || 0 })
}

function shot() {
  if (!connected.value) return toast('请先连接设备', 'error')
  api.screenshot(store.deviceId).then(dataUrl => {
    const a = document.createElement('a')
    a.href = dataUrl
    a.download = `screenshot-${Date.now()}.png`
    a.click()
    toast('截图已保存', 'success')
  }).catch(e => toast('截图失败：' + e.message, 'error'))
}

function rotate() { if (connected.value) sendControl({ type: 'rotate' }) }
function clipboard() {
  if (!connected.value) return
  const text = prompt('输入要发送到设备的剪贴板内容')
  if (text !== null) sendControl({ type: 'clipboard', text, paste: true })
}

function launchGame() {
  if (!connected.value) return toast('请先连接设备', 'error')
  if (!currentPkg.value) return toast('该设备未配置应用包名', 'warn')
  sendControl({ type: 'start_app', app: currentPkg.value })
  toast(`正在启动 ${currentPkg.value}…`, 'info')
}

function openScripts() { router.push('/scripts') }

function tplThumbUrl(name) { return `/api/templates/${encodeURIComponent(name)}/image` }

/** 模板列表：查看大图 */
function openTplView(name) {
  confirmDelTpl.value = null
  viewTpl.value = name
}
function closeTplView() {
  viewTpl.value = null
}

/** 模板列表：点击行（非按钮区域）→ 原模板点击行为（alt 模式生成记录 / 否则测试匹配） */
function onTplRowClick(e, t) {
  confirmDelTpl.value = null
  onTemplateChipClick(e, t.name)
}

/** 模板列表：匹配按钮（原测试匹配） */
function onTplMatchClick(t) {
  confirmDelTpl.value = null
  testMatch(t.name)
}

/** 模板列表：删除按钮（第一次变确认，第二次删除；其他操作自动取消） */
async function onTplDeleteClick(t) {
  if (confirmDelTpl.value === t.name) {
    confirmDelTpl.value = null
    try {
      await api.deleteTemplate(t.name)
      templatesData.value = await api.listTemplates()
      if (viewTpl.value === t.name) viewTpl.value = null
      toast('模板已删除', 'success')
    } catch (e) {
      toast('删除失败：' + e.message, 'error')
    }
  } else {
    confirmDelTpl.value = t.name
  }
}

/** 模板列表：上传图片模板 */
async function onTplUpload(e) {
  confirmDelTpl.value = null
  const file = e.target.files[0]
  e.target.value = ''
  if (!file) return
  let name = file.name
  if (!/\.(png|jpe?g)$/i.test(name)) name += '.png'
  try {
    const b64 = await fileToBase64(file)
    await api.uploadTemplate(name, b64)
    templatesData.value = await api.listTemplates()
    toast('模板已上传', 'success')
  } catch (err) {
    toast('上传失败：' + err.message, 'error')
  }
}

function fileToBase64(file) {
  return new Promise((resolve, reject) => {
    const fr = new FileReader()
    fr.onload = () => resolve(fr.result.split(',')[1])
    fr.onerror = reject
    fr.readAsDataURL(file)
  })
}

/** 全局按键：Esc 关闭模板大图 / 取消删除确认 */
function onGlobalKeydown(e) {
  if (e.key !== 'Escape') return
  if (viewTpl.value) {
    closeTplView()
  } else if (confirmDelTpl.value) {
    confirmDelTpl.value = null
  }
}

function pushLog(level, msg) {
  const now = new Date()
  const t = now.toTimeString().slice(0, 8) + '.' + String(now.getMilliseconds()).padStart(3, '0')
  liveLogs.value.push({ time: t, level, msg })
  if (liveLogs.value.length > 30) liveLogs.value.shift()
  scrollLogsToBottom()
}

/** 重置 alt 模式相关状态（进入/退出编辑模式时调用） */
function resetAltState() {
  altMode.value = false
  opRecords.value = []
  altGesture.active = false
  if (altFeedbackTimer) clearTimeout(altFeedbackTimer)
  altFeedback.show = false
}

/** alt 模式切换按钮：只在编辑模式生效 */
function toggleAltMode() {
  if (scriptMode.value !== 'edit') return
  altMode.value = !altMode.value
}

/** 当前是否应把模板/投屏操作转为操作记录（编辑模式 + 按住 Alt 或 alt 模式开启） */
function isAltAction(e) {
  return scriptMode.value === 'edit' && (altMode.value || (e && e.altKey))
}

/** 模板点击：编辑模式下按 Alt 或 alt 模式开启时生成 find/click/until 三行记录 */
function onTemplateChipClick(e, name) {
  if (isAltAction(e)) {
    opRecords.value = [
      { id: ++opRecordSeq, text: `- find: ${name}`, yaml: buildFindYaml(name) },
      { id: ++opRecordSeq, text: `- click: ${name}`, yaml: buildClickYaml(name) },
      { id: ++opRecordSeq, text: `- until: ${name}`, yaml: buildUntilYaml(name) }
    ]
    return
  }
  testMatch(name)
}

/** 从模板名解析 #x1_y1_x2_y2（只认新格式 0125_0481_0469_1222，÷10000 还原），返回 [x1,y1,x2,y2] 或 null */
function parseTplRegion(name) {
  const base = name.replace(/\.(png|jpe?g)$/i, '')
  const idx = base.lastIndexOf('#')
  if (idx < 0) return null
  const parts = base.slice(idx + 1).split('_')
  if (parts.length !== 4) return null
  const nums = parts.map(s => /^\d{4,5}$/.test(s) ? Number(s) / 10000 : NaN)
  if (!nums.every(n => Number.isFinite(n) && n >= 0 && n <= 1) || !(nums[2] > nums[0]) || !(nums[3] > nums[1])) return null
  return nums
}

/** 从模板名解析 #x1_y1_x2_y2，返回带缩进的 region 行；无匹配时回退 region: a */
function templateRegionLine(name) {
  const nums = parseTplRegion(name)
  if (nums) return `  region: [${nums.map(n => String(n)).join(', ')}]`
  return '  region: a'
}

/** 从模板名解析 #x1_y1_x2_y2 并转为设备像素搜索区域 [x, y, w, h]；无匹配时返回 null */
function templateRegionPixels(name) {
  const nums = parseTplRegion(name)
  if (!nums) return null
  const vw = current.value?.width || videoElement.value?.videoWidth || 1920
  const vh = current.value?.height || videoElement.value?.videoHeight || 1080
  const x = Math.round(nums[0] * vw)
  const y = Math.round(nums[1] * vh)
  const w = Math.round((nums[2] - nums[0]) * vw)
  const h = Math.round((nums[3] - nums[1]) * vh)
  return [x, y, w, h]
}

/** 当前配置的操作间隔 wait 片段（<=0 时为空） */
function intervalWaitYaml() {
  const ms = Number(stepInterval.value) || 0
  return ms > 0 ? `- wait: ${ms}` : ''
}

function buildFindYaml(name) {
  return [
    `- find: ${name}`,
    '  threshold: 0.8',
    templateRegionLine(name),
    '  then:',
    '    - tap: [0.500, 0.500]',
    '  else:',
    '    - log: "未找到"',
    intervalWaitYaml()
  ].filter(Boolean).join('\n')
}

function buildClickYaml(name) {
  return [
    `- click: ${name}`,
    '  threshold: 0.8',
    templateRegionLine(name),
    '  log: "点击成功"',
    '  else:',
    '    - log: "点击失败"',
    intervalWaitYaml()
  ].filter(Boolean).join('\n')
}

function buildUntilYaml(name) {
  return [
    `- until: ${name}`,
    '  timeout: 0',
    '  threshold: 0.8',
    templateRegionLine(name),
    '  else:',
    '    - log: "等待超时"',
    intervalWaitYaml()
  ].filter(Boolean).join('\n')
}

/** 把生成的 YAML 片段以 2 空格缩进追加到脚本的 steps 列表里 */
function appendYamlToScript(snippet) {
  const lines = editScriptCode.value.split('\n')
  const indented = snippet.split('\n').map(l => (l ? '  ' + l : l)).join('\n')
  const stepsIdx = lines.findIndex(l => /^steps\s*:/.test(l))
  // 没有 steps 时补一个最小可运行脚本结构
  if (stepsIdx === -1) {
    const base = editScriptCode.value.trim()
    const block = `name: 新脚本\n\nsteps:\n${indented}`
    editScriptCode.value = base ? base + '\n\n' + block : block
    return
  }
  // 找到 steps 列表的结束位置：下一个非空且不缩进的根级键之前
  let insertIdx = lines.length
  for (let i = stepsIdx + 1; i < lines.length; i++) {
    const line = lines[i]
    if (line.trim() && !/^\s/.test(line)) {
      insertIdx = i
      break
    }
  }
  const before = lines.slice(0, insertIdx)
  const after = lines.slice(insertIdx)
  while (before.length && before[before.length - 1].trim() === '') before.pop()
  const text = before.join('\n') + (before.length ? '\n' : '') + indented + '\n' + after.join('\n')
  editScriptCode.value = text.replace(/\n{3,}/g, '\n\n')
}

/** 点击操作记录行：把对应的 YAML 追加到编辑区 */
function applyOpRecord(r) {
  if (scriptMode.value !== 'edit') return
  appendYamlToScript(r.yaml)
  toast('已追加：' + r.text, 'success')
}

/** 显示 alt 模式画面反馈（2 秒后自动消失） */
function showAltFeedback(kind, x, y, w = 0, h = 0) {
  altFeedback.show = true
  altFeedback.kind = kind
  altFeedback.x = x
  altFeedback.y = y
  altFeedback.w = w
  altFeedback.h = h
  if (altFeedbackTimer) clearTimeout(altFeedbackTimer)
  altFeedbackTimer = setTimeout(() => { altFeedback.show = false }, 2000)
}

/** 投屏点击 → 生成 tap 记录 */
function setTapRecord(p) {
  const vw = videoElement.value?.videoWidth || 1920
  const vh = videoElement.value?.videoHeight || 1080
  const rx = (p.x / vw).toFixed(4)
  const ry = (p.y / vh).toFixed(4)
  const records = [
    { id: ++opRecordSeq, text: `- tap [${rx}, ${ry}]`, yaml: `- tap: [${rx}, ${ry}]` }
  ]
  const wait = intervalWaitYaml()
  if (wait) records.push({ id: ++opRecordSeq, text: `- wait ${stepInterval.value}ms`, yaml: wait })
  opRecords.value = records
  showAltFeedback('tap', p.x, p.y)
}

/** 投屏滑动 → 生成 swipe + region + wait 记录 */
function setSwipeRecords(from, to) {
  const vw = videoElement.value?.videoWidth || 1920
  const vh = videoElement.value?.videoHeight || 1080
  const fx = (from.x / vw).toFixed(4)
  const fy = (from.y / vh).toFixed(4)
  const tx = (to.x / vw).toFixed(4)
  const ty = (to.y / vh).toFixed(4)
  opRecords.value = [
    {
      id: ++opRecordSeq,
      text: `- swipe [${fx}, ${fy}] -> [${tx}, ${ty}] 1000ms`,
      yaml: [
        '- swipe:',
        `    from: [${fx}, ${fy}]`,
        `    to: [${tx}, ${ty}]`,
        '    time: 1000'
      ].join('\n')
    },
    {
      id: ++opRecordSeq,
      text: `  region [${fx}, ${fy}, ${tx}, ${ty}]`,
      yaml: `  region: [${fx}, ${fy}, ${tx}, ${ty}]`
    }
  ]
  const wait = intervalWaitYaml()
  if (wait) opRecords.value.push({ id: ++opRecordSeq, text: `- wait ${stepInterval.value}ms`, yaml: wait })
  const rx = Math.min(from.x, to.x)
  const ry = Math.min(from.y, to.y)
  const rw = Math.abs(to.x - from.x)
  const rh = Math.abs(to.y - from.y)
  showAltFeedback('region', rx, ry, rw, rh)
}

function cancelEditScript() {
  editScriptId.value = null
  scriptMode.value = 'run'
  resetAltState()
}

function startNewScript() {
  editScriptId.value = null
  editScriptName.value = '新脚本'
  editScriptCode.value = DEFAULT_SCRIPT_CODE
  scriptMode.value = 'edit'
  resetAltState()
}

/** 运行模式：编辑当前选中的脚本 */
function editCurrentScript() {
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return toast('请先选择脚本', 'error')
  editScriptId.value = s.id
  editScriptName.value = s.name.replace(/\.(ya?ml)$/i, '')
  editScriptCode.value = s.content
  scriptMode.value = 'edit'
  resetAltState()
}

/** 运行模式：删除当前选中的脚本 */
async function deleteCurrentScript() {
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return toast('请先选择脚本', 'error')
  if (!confirm(`删除脚本 ${s.name}？`)) return
  try {
    await api.deleteScript(s.id)
    await loadData()
    if (selScript.value === s.id) selScript.value = ''
    toast('脚本已删除', 'success')
  } catch (e) {
    toast('删除失败：' + e.message, 'error')
  }
}

/** 保存前校验 YAML：语法 / steps / 坐标范围 / 模板存在 */
function validateScriptCode(content) {
  const errors = []
  let doc
  try {
    doc = yamlLoad(content)
  } catch (e) {
    return ['YAML 语法错误：' + e.message]
  }
  if (!doc || typeof doc !== 'object' || Array.isArray(doc)) return ['脚本必须是 YAML 对象']
  if (!Array.isArray(doc.steps)) return ['缺少 steps 根节点']

  const vw = current.value?.width || 1920
  const vh = current.value?.height || 1080
  const tplNames = new Set((templatesData.value || []).map(t => t.name))
  const inRange = (x, y) => Number.isFinite(x) && Number.isFinite(y) && x >= 0 && y >= 0 && x <= vw && y <= vh
  const checkCoord = (label, x, y) => { if (!inRange(x, y)) errors.push(`${label} 坐标超出画面范围 (${x}, ${y})`) }
  const checkRel = (label, x, y) => {
    if (!Number.isFinite(x) || !Number.isFinite(y) || x < 0 || x > 1 || y < 0 || y > 1) {
      errors.push(`${label} 相对坐标需在 0~1 之间 (${x}, ${y})`)
    }
  }
  const REGION_CODES = ['a', 'u', 'd', 'l', 'r', 'ul', 'ur', 'dl', 'dr']
  const checkRegion = (at, region) => {
    if (region === undefined) return
    if (typeof region === 'string') {
      if (!REGION_CODES.includes(region)) errors.push(`${at} region 无效：${region}`)
    } else if (Array.isArray(region)) {
      if (region.length !== 4) {
        errors.push(`${at} region 数组需要 [x1, y1, x2, y2]`)
      } else {
        const [x1, y1, x2, y2] = region.map(Number)
        if (![x1, y1, x2, y2].every(v => Number.isFinite(v) && v >= 0 && v <= 1)) {
          errors.push(`${at} region 相对坐标需在 0~1 之间`)
        } else if (x2 <= x1 || y2 <= y1) {
          errors.push(`${at} region 需要 x2 > x1 且 y2 > y1`)
        }
      }
    } else {
      errors.push(`${at} region 只支持 a/u/d/l/r/ul/ur/dl/dr 或 [x1, y1, x2, y2]`)
    }
  }

  doc.steps.forEach((step, idx) => {
    const at = `第 ${idx + 1} 步`
    if (!step || typeof step !== 'object' || Array.isArray(step)) {
      errors.push(`${at}格式错误`)
      return
    }
    if (step.tap !== undefined) {
      const v = step.tap
      if (Array.isArray(v) && v.length >= 2) {
        checkRel(`${at} tap`, Number(v[0]), Number(v[1]))
      } else if (v && typeof v === 'object') {
        checkRel(`${at} tap`, Number(v.x), Number(v.y))
      } else {
        errors.push(`${at} tap 需要 [x, y] 相对坐标`)
      }
    }
    if (step.click !== undefined) {
      const v = step.click
      if (typeof v !== 'string') {
        errors.push(`${at} click 只支持模板字符串写法，如 click: shop.png`)
      } else {
        if (!tplNames.has(v)) errors.push(`${at} 模板不存在：${v}`)
        if (step.timeout !== undefined) errors.push(`${at} timeout 只支持 until`)
        checkRegion(at, step.region)
      }
    }
    if (step.swipe) {
      if (step.swipe.duration !== undefined) errors.push(`${at} swipe 请使用 time，不支持 duration`)
      const from = step.swipe.from, to = step.swipe.to
      if (Array.isArray(from) && from.length >= 2) checkRel(`${at} swipe from`, Number(from[0]), Number(from[1]))
      if (Array.isArray(to) && to.length >= 2) checkRel(`${at} swipe to`, Number(to[0]), Number(to[1]))
    }
    for (const key of ['find', 'until']) {
      const v = step[key]
      if (v === undefined) continue
      if (typeof v !== 'string') {
        errors.push(`${at} ${key} 只支持模板字符串写法，如 ${key}: shop.png`)
        continue
      }
      if (!tplNames.has(v)) errors.push(`${at} 模板不存在：${v}`)
      if (key === 'find' && step.timeout !== undefined) errors.push(`${at} timeout 只支持 until`)
      checkRegion(at, step.region)
    }
  })
  return errors
}

/** 保存新建脚本：先校验再保存，名称自动补 .yml */
async function saveEditScript() {
  const rawName = editScriptName.value.trim()
  if (!rawName) return toast('请填写脚本名称', 'error')
  const name = /\.(ya?ml)$/i.test(rawName) ? rawName : rawName + '.yml'
  const errors = validateScriptCode(editScriptCode.value)
  if (errors.length) return toast('校验未通过：' + errors.slice(0, 3).join('；'), 'error')
  scriptSaving.value = true
  try {
    const r = await api.saveScript({ id: editScriptId.value, name, content: editScriptCode.value })
    await loadData()
    editScriptId.value = null
    scriptMode.value = 'run'
    resetAltState()
    selScript.value = r.id
    toast('脚本已保存', 'success')
  } catch (e) {
    toast('保存失败：' + e.message, 'error')
  } finally {
    scriptSaving.value = false
  }
}

function runScript() {
  if (!selScript.value) return
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return
  // 每次运行清空日志区域，只显示本次运行产生的日志
  runStartTime = Date.now()
  rawLogs = []
  liveLogs.value = []
  store.running = true
  store.runScript = s.name
  api.runScript(s.id, store.deviceId).then(() => {
    setTimeout(() => { store.running = false }, 1500)
  }).catch(e => {
    store.running = false
    pushLog('error', `执行失败：${e.message}`)
    toast('脚本执行失败', 'error')
  })
}

function stopScript() {
  if (!selScript.value) return
  api.stopScript(selScript.value).catch(() => {})
  store.running = false
  toast('脚本已停止', 'warn')
}

async function testMatch(name) {
  if (!connected.value) return toast('请先连接设备', 'error')
  if (hitTimer) { clearTimeout(hitTimer); hitTimer = null }
  showHit.value = false
  try {
    const region = templateRegionPixels(name)
    const r = await api.testTemplate(name, store.deviceId, Number(testThreshold.value) || 0.8, region)
    if (r.hit) {
      hit.x = r.x; hit.y = r.y; hit.w = r.width; hit.h = r.height
      hitLabel.value = `${name} ${r.score.toFixed(2)}`
      showHit.value = true
      // 匹配框只展示 3 秒，避免一直留在画面上
      hitTimer = setTimeout(() => { showHit.value = false }, 3000)
      toast(`匹配成功：${name} 置信度 ${r.score.toFixed(2)}`, 'success')
    } else {
      toast(`未找到：${name}`, 'warn')
    }
  } catch (e) {
    toast('匹配失败：' + e.message, 'error')
  }
}

function fullscreen() {
  if (videoWrap.value?.requestFullscreen) videoWrap.value.requestFullscreen()
}

onMounted(async () => {
  // 进入控制台前是否已选定设备（从其他页面跳转/会话恢复 → 自动重连恢复画面；
  // 首次进入仅选中第一台设备，等待用户点连接）
  const preselected = !!store.deviceId
  await loadData()
  if (!store.deviceId && devices.value.length) {
    store.deviceId = devices.value[0].id
  }
  const d = current.value
  if (d) loadForm(d)
  else { mode.value = 'edit'; store.deviceId = null }
  // 页面关闭时释放页面锁（其他页面才能接管）
  window.addEventListener('beforeunload', releaseLock)
  window.addEventListener('keydown', onGlobalKeydown)
  if (preselected && store.deviceId) connect(false)
})

onUnmounted(() => {
  window.removeEventListener('beforeunload', releaseLock)
  window.removeEventListener('keydown', onGlobalKeydown)
  if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null }
  if (saveTimer) { clearTimeout(saveTimer); saveTimer = null }
  if (savedTimer) { clearTimeout(savedTimer); savedTimer = null }
  if (hitTimer) { clearTimeout(hitTimer); hitTimer = null }
  if (altFeedbackTimer) { clearTimeout(altFeedbackTimer); altFeedbackTimer = null }
  releaseLock()
  cleanup(true)
})
</script>

<style scoped>
.console { display: flex; height: 100%; padding: 14px; gap: 14px; }

/* ===== 画面区 ===== */
.stage { flex: 1; display: flex; flex-direction: column; gap: 10px; min-width: 0; }

.video-wrap {
  flex: 1; position: relative; background: #000;
  border: 1px solid var(--border); border-radius: var(--radius);
  overflow: hidden; min-height: 300px;
}

.video-stream { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; user-select: none; }

.hit-box {
  position: absolute; border: 2px solid var(--accent);
  box-shadow: 0 0 12px rgba(34,211,165,.5); border-radius: 4px;
  pointer-events: none; z-index: 5;
}
.hit-label {
  position: absolute; top: -22px; left: 0; background: var(--accent); color: #06251c;
  font-size: 10px; font-weight: 700; padding: 1px 6px; border-radius: 4px; white-space: nowrap;
}

.select-box {
  position: absolute; border: 2px dashed var(--accent-2);
  background: rgba(56,189,248,.12); pointer-events: none; z-index: 5;
}

/* alt 模式点击/滑动反馈 */
.alt-tap {
  position: absolute; z-index: 6; width: 14px; height: 14px;
  border-radius: 50%; border: 2px solid var(--accent-2);
  background: rgba(56,189,248,.28);
  transform: translate(-50%, -50%); pointer-events: none;
  box-shadow: 0 0 8px rgba(56,189,248,.6);
}
.alt-region {
  position: absolute; z-index: 6; border: 2px dashed var(--accent-2);
  background: rgba(56,189,248,.12); pointer-events: none;
  box-shadow: 0 0 10px rgba(56,189,248,.25);
}
.alt-label {
  position: absolute; top: -20px; left: 0; font-size: 10px;
  color: var(--accent-2); background: rgba(8,10,16,.7);
  padding: 1px 5px; border-radius: 4px; white-space: nowrap;
  font-family: var(--mono);
}

/* 放大预览镜 */
.loupe {
  position: fixed; z-index: 200; width: 150px; height: 150px;
  border: 1px solid rgba(34,211,165,.5); border-radius: 10px; overflow: hidden;
  background: #000; box-shadow: 0 8px 30px rgba(0,0,0,.6);
  pointer-events: none;
}
.loupe canvas { width: 100%; height: 100%; display: block; }
.loupe-tag {
  position: absolute; right: 6px; bottom: 4px; font-size: 10px;
  color: #fff; background: rgba(0,0,0,.55); padding: 1px 5px; border-radius: 6px;
}

/* 二次裁切区 */
.crop-stage { display: flex; flex-direction: column; align-items: center; gap: 6px; }
.crop-canvas {
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  cursor: crosshair; background: #000; touch-action: none;
}
.crop-hint { font-size: 10px; color: var(--text-2); align-self: flex-start; }
.crop-panel {
  display: flex; flex-direction: column; gap: 10px;
  border-top: 1px solid var(--border); padding-top: 12px;
}
.crop-actions { display: flex; gap: 8px; }
.crop-actions .btn-primary { margin-left: auto; }

.v-overlay {
  position: absolute; inset: 0; z-index: 10; display: flex;
  align-items: center; justify-content: center;
  background: rgba(8,10,16,.72); backdrop-filter: blur(2px);
}
.v-connecting { display: flex; align-items: center; gap: 10px; color: var(--accent); font-size: 14px; }
.v-empty-icon { font-size: 44px; text-align: center; opacity: .6; }
.v-empty-text { color: var(--text-1); margin: 10px 0 16px; max-width: 320px; text-align: center; }

.v-stats {
  position: absolute; left: 12px; top: 12px; z-index: 6;
  display: flex; gap: 8px; background: rgba(8,10,16,.6);
  border: 1px solid rgba(255,255,255,.08); border-radius: 20px; padding: 4px 10px;
}
.st { font-size: 11px; color: var(--text-1); font-family: var(--mono); }

.v-fs {
  position: absolute; right: 12px; top: 12px; z-index: 6;
  background: rgba(8,10,16,.6); border: 1px solid rgba(255,255,255,.08);
  color: var(--text-1); border-radius: 8px; width: 30px; height: 30px; cursor: pointer;
}
.v-fs:hover { color: var(--accent); border-color: var(--accent); }

/* 工具条 */
.toolbar {
  display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 8px 10px;
  min-height: 45px; box-sizing: border-box;
}
.tb-sep { width: 1px; height: 22px; background: var(--border); margin: 0 4px; }
.tb-tip { margin-left: auto; font-size: 11px; color: var(--text-2); }
.btn.active { border-color: var(--accent-2); color: var(--accent-2); }

/* ===== 右侧面板 ===== */
.panel {
  width: 340px; flex-shrink: 0; display: flex; flex-direction: column; gap: 10px;
  overflow: hidden;
}
.panel-tabs {
  display: flex; gap: 4px; flex-shrink: 0;
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 4px 6px;
  height: 45px; box-sizing: border-box;
}
.tab-btn {
  flex: 1; display: flex; align-items: center; justify-content: center; gap: 4px;
  padding: 7px 2px; border: none; border-radius: var(--radius-sm);
  background: transparent; color: var(--text-2); font-size: 12px; cursor: pointer;
  transition: all .15s; white-space: nowrap;
}
.tab-btn:hover { color: var(--text-0); background: var(--bg-3); }
.tab-btn.active { background: rgba(34,211,165,.1); color: var(--accent); font-weight: 600; }
.tab-btn.hidden { display: none; }
.tab-body { flex: 1; display: flex; flex-direction: column; gap: 12px; overflow: auto; min-height: 0; }
.ps-stats { display: flex; gap: 8px; flex-wrap: wrap; }

/* ===== 设备页签（设备管理 + 配置表单） ===== */
.dev-pick { display: flex; gap: 6px; align-items: center; }
.dev-pick .dev-select { flex: 1; min-width: 0; padding: 5px 8px; font-size: 12px; }
.btn-block { width: 100%; }

.cfg-form { display: flex; flex-direction: column; gap: 10px; }
.cfg-form-head {
  display: flex; align-items: baseline; justify-content: space-between;
  font-size: 12px; font-weight: 600; padding-bottom: 2px;
  border-bottom: 1px solid var(--border);
}
.cfg-form-sub { font-size: 10px; font-weight: 400; color: var(--text-2); }
.cfg-actions { display: flex; gap: 8px; }
.cfg-actions .btn-primary { flex: 1; }
.cfg-hint { font-size: 10px; color: var(--text-2); }

/* 编辑模式：连接概览 */
.dev-summary { display: flex; flex-direction: column; gap: 8px; }
.sum-row { display: flex; align-items: center; gap: 8px; font-size: 12px; }
.sum-label { width: 34px; flex-shrink: 0; font-size: 11px; color: var(--text-2); }
.sum-value { min-width: 0; word-break: break-all; color: var(--text-1); }
.sum-actions { display: flex; align-items: center; gap: 8px; }
.sum-actions .btn { flex: 1; }
.sum-actions .btn-danger { flex: 0 0 auto; }
.kind-badge {
  display: inline-flex; align-items: center; gap: 4px; padding: 2px 8px;
  background: var(--bg-3); border: 1px solid var(--border);
  border-radius: 12px; font-size: 11px; white-space: nowrap;
}
.dev-empty { display: flex; flex-direction: column; align-items: center; gap: 8px; padding: 24px 0; text-align: center; }
.dev-empty-icon { font-size: 34px; opacity: .5; }
.dev-empty-text { color: var(--text-2); font-size: 12px; }

.form-row { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
.form-row .form-item { min-width: 0; }
.dpi-box { display: flex; gap: 6px; }
.dpi-box .input { flex: 1; min-width: 0; }
.dpi-box .btn { flex-shrink: 0; }
.dpi-box .btn.active { border-color: var(--accent-2); color: var(--accent-2); }

.muted { color: var(--text-2); font-weight: 400; }
.small { font-size: 11px; margin-top: 4px; }

.type-picker { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; }
.type-opt {
  display: flex; flex-direction: column; align-items: center; gap: 5px;
  padding: 10px 4px; border-radius: var(--radius-sm); border: 1px solid var(--border);
  cursor: pointer; font-size: 11px; color: var(--text-1); transition: all .15s;
  text-align: center;
}
.type-opt:hover { border-color: #33405e; }
.type-opt.sel { border-color: var(--accent); color: var(--accent); background: rgba(34,211,165,.06); }
.type-icon { font-size: 18px; }

.mode-picker { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.mode-opt {
  padding: 10px; border-radius: var(--radius-sm); border: 1px solid var(--border);
  cursor: pointer; transition: all .15s; display: flex; flex-direction: column; gap: 3px;
}
.mode-opt:hover { border-color: #33405e; }
.mode-opt.sel { border-color: var(--accent); background: rgba(34,211,165,.06); }
.mode-title { font-size: 12px; font-weight: 600; }
.mode-desc { font-size: 10px; color: var(--text-2); }

.vd-presets { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; }
.vd-opt {
  display: flex; flex-direction: column; align-items: center; gap: 2px;
  padding: 8px 2px; border-radius: var(--radius-sm); border: 1px solid var(--border);
  cursor: pointer; transition: all .15s;
}
.vd-opt:hover { border-color: #33405e; }
.vd-opt.sel { border-color: var(--accent-2); background: rgba(56,189,248,.06); }
.vd-res { font-size: 11px; color: var(--text-0); }
.vd-dpi { font-size: 10px; color: var(--text-2); }

/* 应用下拉 */
.app-box { position: relative; display: flex; gap: 6px; }
.app-box .btn { flex-shrink: 0; }
.app-box .input { flex: 1; min-width: 0; }
.app-menu {
  position: absolute; left: 0; right: 0; top: calc(100% + 4px); z-index: 30;
  background: var(--bg-1); border: 1px solid var(--border); border-radius: var(--radius-sm);
  max-height: 200px; overflow: auto; box-shadow: 0 8px 24px rgba(0,0,0,.45);
}
.app-opt {
  display: flex; flex-direction: column; gap: 2px; padding: 7px 10px;
  cursor: pointer; border-bottom: 1px solid rgba(255,255,255,.04);
}
.app-opt:hover { background: var(--bg-3); }
.app-label { font-size: 12px; color: var(--text-0); }
.app-pkg { font-size: 10px; color: var(--text-2); }
.app-empty { padding: 10px; font-size: 11px; color: var(--text-2); text-align: center; }

/* 帧率选择（无"自动"选项） */
.fps-presets { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; }
.fps-opt {
  text-align: center; padding: 8px 4px; border-radius: var(--radius-sm);
  border: 1px solid var(--border); cursor: pointer; font-size: 12px;
  color: var(--text-1); transition: all .15s;
}
.fps-opt:hover { border-color: #33405e; }
.fps-opt.sel { border-color: var(--accent-2); background: rgba(56,189,248,.06); color: var(--accent-2); }

.panel-sec {
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 14px; display: flex; flex-direction: column; gap: 10px;
  flex-shrink: 0;
}
.ps-head { display: flex; align-items: center; gap: 8px; }
.ps-title { font-size: 13px; font-weight: 600; }
.ps-sub { font-size: 11px; color: var(--text-2); }
.mono { font-family: var(--mono); font-size: 11px; color: var(--text-1); }

.auto-run { display: flex; flex-wrap: wrap; gap: 8px; }
.auto-run .select { flex: 1; min-width: 120px; }
.auto-run .log-level { flex: 0 0 90px; }
.run-actions { display: flex; gap: 8px; }
.run-actions .btn { flex: 1; }

/* 脚本页签 */
.panel-sec.script-tab { flex: 1; min-height: 0; overflow: hidden; }
.script-tpl { flex: 4; min-height: 0; display: flex; flex-direction: column; gap: 8px; border-bottom: 1px solid var(--border); padding-bottom: 10px; }
.tpl-top { display: flex; align-items: center; gap: 8px; }
.tpl-top .input { flex: 1; min-width: 0; }
.tpl-top .btn { flex-shrink: 0; }
.tpl-tools { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.script-run { flex: 6; display: flex; flex-direction: column; gap: 10px; min-height: 0; }
.script-logs { flex: 1; min-height: 120px; max-height: none; }
.script-edit { flex: 6; display: flex; flex-direction: column; gap: 10px; min-height: 0; }
.edit-name-row { display: flex; }
.edit-name-row .input { flex: 1; min-width: 0; width: 100%; }
.edit-actions { display: flex; gap: 8px; }
.edit-actions .btn { flex: 1; justify-content: center; }
.edit-actions .btn.active { border-color: var(--accent-2); color: var(--accent-2); background: rgba(56,189,248,.08); }
.edit-interval { display: flex; align-items: center; gap: 6px; font-size: 11px; color: var(--text-2); }
.edit-interval .input { width: 80px; padding: 4px 8px; }
.op-record {
  flex-shrink: 0; height: 77px; display: flex; flex-direction: column;
  background: var(--bg-0); border: 1px solid var(--border);
  border-radius: var(--radius-sm); padding: 3px; overflow: hidden;
}
.op-record-line {
  flex: 0 0 auto; height: 23px; display: flex; align-items: center; padding: 0 8px;
  font-size: 11px; line-height: 1.4; color: var(--text-1); cursor: pointer;
  border-radius: 4px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
}
.op-record-line:hover { background: var(--bg-3); color: var(--accent); }
.op-record-empty {
  height: 100%; display: flex; align-items: center; justify-content: center;
  font-size: 11px; color: var(--text-2); text-align: center; padding: 0 8px;
}
.script-editor {
  flex: 1; min-height: 160px; resize: none; background: var(--bg-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  color: #c9d4e8; font-size: 12px; line-height: 1.65; padding: 12px;
  font-family: var(--mono); outline: none;
}

.run-progress { display: flex; flex-direction: column; gap: 6px; }
.rp-head { display: flex; justify-content: space-between; font-size: 12px; }
.rp-script { color: var(--accent); }
.rp-pct { color: var(--text-1); }
.rp-bar { height: 5px; background: var(--bg-3); border-radius: 3px; overflow: hidden; }
.rp-fill { height: 100%; background: linear-gradient(90deg, var(--accent), var(--accent-2)); border-radius: 3px; transition: width .4s; }
.rp-step { font-size: 11px; color: var(--text-1); }

.live-logs {
  max-height: 180px; overflow: auto; background: var(--bg-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 8px; display: flex; flex-direction: column; gap: 3px;
}
.live-logs.script-logs { max-height: none; }
.ll { display: flex; gap: 8px; font-size: 11px; line-height: 1.5; }
.ll-time { color: var(--text-2); flex-shrink: 0; }
.ll.info .ll-msg { color: var(--text-1); }
.ll.success .ll-msg { color: var(--ok); }
.ll.warn .ll-msg { color: var(--warn); }
.ll.error .ll-msg { color: var(--danger); }

/* 模板文件列表 */
.tpl-list-wrap { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 4px; }
.tpl-list-head, .tpl-row { display: flex; align-items: center; gap: 8px; padding: 3px 8px; }
.tpl-list-head { font-size: 11px; color: var(--text-2); border-bottom: 1px solid var(--border); flex-shrink: 0; }
.tpl-list { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 2px; min-height: 0; }
.tpl-row {
  cursor: pointer; border-radius: var(--radius-sm); border: 1px solid transparent;
  transition: background .15s;
}
.tpl-row:hover { background: var(--bg-3); }
.tpl-row.del-confirm { background: rgba(248,113,113,.08); border-color: rgba(248,113,113,.35); }
.tpl-empty { padding: 16px 8px; text-align: center; font-size: 11px; color: var(--text-2); }
.tpl-cell.thumb { width: 30px; flex-shrink: 0; display: flex; align-items: center; }
.tpl-cell.name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; color: var(--text-0); }
.tpl-cell.ops { display: flex; gap: 6px; flex-shrink: 0; }
.tpl-cell.ops .btn { padding: 2px 8px; font-size: 11px; }
.tpl-thumb { font-size: 12px; position: relative; display: inline-flex; }
.tpl-thumb::before { content: '▦'; }
.tpl-thumb img {
  position: relative; z-index: 1; width: 24px; height: 24px; object-fit: contain;
}
.tpl-del-confirm {
  background: var(--danger); border-color: var(--danger); color: #fff;
}
.tpl-del-confirm:hover { background: #ef4444; color: #fff; }

/* 模板查看大图 */
.tpl-view-mask {
  position: fixed; inset: 0; z-index: 100; display: flex; align-items: center; justify-content: center;
  background: rgba(8,10,16,.78); backdrop-filter: blur(2px);
}
.tpl-view-modal {
  position: relative; display: flex; flex-direction: column; gap: 8px;
  max-width: 92vw; max-height: 92vh;
}
.tpl-view-modal img {
  max-width: 92vw; max-height: 82vh; object-fit: contain;
  border-radius: var(--radius-sm); border: 1px solid var(--border); background: #000;
}
.tpl-view-close {
  position: absolute; top: 8px; right: 8px; width: 28px; height: 28px;
  display: flex; align-items: center; justify-content: center;
  background: var(--bg-2); border: 1px solid var(--border); border-radius: 50%;
  color: var(--text-1); cursor: pointer; font-size: 13px; z-index: 1;
}
.tpl-view-close:hover { color: var(--danger); border-color: var(--danger); }
.tpl-view-name { text-align: center; font-size: 12px; color: var(--text-1); word-break: break-all; }

/* 二次裁切占满整个模板区域 */
.crop-panel-full { flex: 1; min-height: 0; border-top: none; padding-top: 0; }
.crop-panel-full .crop-stage { flex: 1; min-height: 0; justify-content: center; }
</style>
