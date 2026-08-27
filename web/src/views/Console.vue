<template>
  <div class="console" :class="{ 'sb-collapsed': sidebarCollapsed }">
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
        <div v-if="showHit" class="hit-box" :class="{ 'hit-miss': hitMiss }" :style="hitStyle">
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

        <!-- 脚本运行可视化：引擎 tap/swipe/匹配命中（服务端经 control DataChannel 推送，样式复用 alt/hit） -->
        <div v-if="scriptFx.tap.show" class="alt-tap" :style="fxTapStyle">
          <span class="alt-label">tap</span>
        </div>
        <div v-if="scriptFx.swipe.show" class="alt-region" :style="fxSwipeStyle">
          <span class="alt-label">swipe</span>
        </div>
        <div v-if="scriptFx.hit.show" class="hit-box" :class="{ 'hit-miss': scriptFx.hit.miss }" :style="fxHitStyle">
          <span class="hit-label">{{ scriptFx.hit.label }}</span>
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

        <!-- 脚本（模板功能 + 脚本运行/编辑），按应用分区存储（data/<pkg>/tmpl|yaml） -->
        <div v-show="activeTab === 'script'" class="panel-sec script-tab">
          <!-- 应用分区切换：模板与脚本数据随分区切换（默认跟随设备页签的应用包名） -->
          <div class="pkg-bar">
            <span class="pkg-label">应用</span>
            <select v-model="activePkg" class="select mono pkg-select" title="应用分区（data/<应用包名>/tmpl|yaml），默认跟随设备页签配置的应用包名">
              <option v-if="!pkgOptions.length" value="">（未配置应用包名）</option>
              <option v-for="p in pkgOptions" :key="p" :value="p">{{ p }}</option>
            </select>
            <button class="btn btn-sm" :disabled="!activePkg" @click="exportPartition" title="导出当前应用分区的全部脚本与模板（zip 分区快照，导入导出同构）">⬆ 导出</button>
            <button class="btn btn-sm" :disabled="!activePkg" @click="$refs.impFile.click()" title="导入分区快照 zip 到当前应用分区（同名文件替换前二次确认）">⬇ 导入</button>
            <input ref="impFile" type="file" accept=".zip" hidden @change="onImportFile" />
          </div>
          <div v-if="!activePkg" class="pkg-empty">暂无应用分区：请先在「设备」页签配置应用包名（模板与脚本按应用分区存储）</div>
          <template v-else>
          <!-- 模板功能（放上面） -->
          <div class="script-tpl">
            <!-- 模板文件列表（非裁切时） -->
            <template v-if="!crop.active">
              <div class="tpl-top">
                <input v-model.number="testThreshold" class="input input-sm mono" type="number" min="0" max="1" step="0.01" placeholder="测试阈值 0~1" title="模板测试阈值，默认 0.8" />
                <select v-model="testRegion" class="select mono tpl-region" title="模板匹配区域：默认=按模板名自动识别（名字带 #x1_y1_x2_y2 用对应矩形，带 #a/#u/#d/#l/#r/#ul/#ur/#dl/#dr 用对应半区，否则等价 a 全屏）；手动选择后，测试匹配与生成记录都使用该区域">
                  <option value="">默认</option>
                  <option value="a">a · 全屏</option>
                  <option value="u">u · 上半屏</option>
                  <option value="d">d · 下半屏</option>
                  <option value="l">l · 左半屏</option>
                  <option value="r">r · 右半屏</option>
                  <option value="ul">ul · 左上</option>
                  <option value="ur">ur · 右上</option>
                  <option value="dl">dl · 左下</option>
                  <option value="dr">dr · 右下</option>
                </select>
                <input v-model="tplSearch" class="input input-sm mono tpl-search" placeholder="🔍 模糊/拼音首字母搜索…" title="模糊搜索模板名（短名/带 #后缀 全名均可，按匹配位置排序）；中文名支持拼音首字母，如 rcyq 命中 日常遗器.png；三类命中并列展示，文字命中排前" />
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
                    <div v-for="t in templates" :key="t.name" class="tpl-row" :class="{ 'del-confirm': confirmDelTpl === t.name, renaming: renaming === t.name }" @click="onTplRowClick($event, t)">
                    <span class="tpl-cell thumb" @click.stop="onTplThumbClick($event, t)" title="点击查看大图；Alt/alt 模式点击复制模板名">
                      <span class="tpl-thumb"><img :src="tplThumbUrl(t.name)" alt="" loading="lazy" @error="e => e.target.style.visibility = 'hidden'" /></span>
                    </span>
                    <span class="tpl-cell name mono" :title="`${t.name}（点击查看大图；Alt/alt 模式点击生成 find 记录）`" @click.stop="onTplNameClick($event, t)">
                      <input v-if="renaming === t.name" :ref="el => renameInputEl = el" v-model="renameVal" class="input rename-input mono" @keydown.enter="confirmRename(t)" @keydown.esc="cancelRename" @blur="cancelRename" @click.stop />
                      <template v-else>{{ tplShortName(t.name) }}<span v-if="tplRegionBadge(t.name)" class="tpl-region-badge" :title="`${t.name}（区域后缀，脚本可写短名 ${tplShortName(t.name)}）`">{{ tplRegionBadge(t.name) }}</span></template>
                    </span>
                    <span class="tpl-cell ops">
                      <button v-if="renaming === t.name" class="btn btn-sm btn-primary" @mousedown.prevent @click.stop="confirmRename(t)">确认</button>
                      <button v-else class="btn btn-sm" @click.stop="startRename(t)">重命名</button>
                      <button class="btn btn-sm" :class="{ 'tpl-del-confirm': confirmDelTpl === t.name }" @click.stop="onTplDeleteClick(t)">{{ confirmDelTpl === t.name ? '确认' : '删除' }}</button>
                      <button class="btn btn-sm" @click.stop="onTplMatchClick(t)">匹配</button>
                    </span>
                  </div>
                  <div v-if="!templates.length" class="tpl-empty">{{ tplSearch.trim() ? '没有匹配的模板' : '暂无模板，点击「框选」或「上传」创建' }}</div>
                </div>
              </div>
              <div class="tpl-tools">
                <span class="ps-sub">缩略图 → 查看大图（Alt / alt 模式 → 复制模板名）· alt 模式点文件名 → 生成 find 记录 · 匹配 → 测试匹配 · 重命名 → 修改模板名</span>
              </div>
            </template>

            <!-- 二次裁切（框选后占满整个模板区域） -->
            <div v-else class="crop-panel crop-panel-full" ref="cropSec">
              <div class="ps-head">
                <span class="ps-title">✂️ 二次裁切</span>
                <span class="ps-sub mono">{{ cropSize }} · {{ cropZoomPct }}</span>
              </div>
              <div class="crop-stage">
                <canvas ref="cropCanvas" class="crop-canvas" @mousedown="cropMouseDown" @mousemove="cropMouseMove" @mouseup="cropMouseUp" @mouseleave="cropMouseLeave" @wheel="cropWheel"></canvas>
              </div>
              <div class="crop-hint">滚轮缩放（50%~800%）· 拖动边框/角调整选框（只动遮罩框）· 拖框内移动位置 · Alt/alt 模式点击任意处 → 取色生成 color 记录</div>
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
                <div class="tpl-view-img">
                  <img :src="tplThumbUrl(viewTpl)" alt="模板预览" />
                </div>
                <div class="tpl-view-name mono">{{ viewTpl }}</div>
              </div>
            </div>
          </div>
          <!-- 脚本功能：运行模式 -->
          <div v-if="scriptMode === 'run'" class="script-run">
            <div class="auto-run">
              <ScriptPicker v-model="selScript" :package="activePkg" />
            </div>
            <div class="run-actions">
              <button v-if="!store.running" class="btn btn-primary" :disabled="!selScript || !store.deviceId" @click="runScript">▶ 运行</button>
              <button v-else class="btn btn-danger" @click="stopScript">■ 停止</button>
              <button class="btn" :disabled="!selScript" @click="editCurrentScript">编辑</button>
              <div class="more-wrap">
                <button class="btn" :class="{ active: moreOpen }" @click="moreOpen = !moreOpen">更多 ▾</button>
                <div v-if="moreOpen" class="more-mask" @click="moreOpen = false"></div>
                <div v-if="moreOpen" class="more-dropdown">
                  <button class="more-item" @click="moreOpen = false; startNewScript()">＋ 新建</button>
                  <button class="more-item danger" :disabled="!selScript" @click="moreOpen = false; deleteCurrentScript()">🗑 删除</button>
                </div>
              </div>
            </div>

            <!-- 运行中：实时日志；其他情况：脚本内容（只读） -->
            <div v-if="store.running" ref="logBox" class="live-logs script-logs mono">
              <div v-for="(l, i) in liveLogs" :key="i" class="ll" :class="l.level">
                <span class="ll-time">{{ l.time }}</span>
                <span class="ll-msg">{{ l.msg }}</span>
              </div>
            </div>
            <template v-else>
              <div v-if="!selScript" class="script-view-empty">请选择脚本</div>
              <div v-else class="script-view-wrap">
                <div class="run-hint">点击「- 」开头的逻辑行（含函数体内步骤）→ 从该步骤开始运行；点击函数名行 → 从头运行整个函数（先判 cond 再跑函数体）；再次点击选中行取消（从头运行）</div>
                <div class="script-view mono">
                  <div
                    v-for="(line, idx) in scriptLines"
                    :key="idx"
                    class="sv-line"
                    :class="{ sel: selectedLine === idx, selectable: !!runLineMap[idx] }"
                    @click="onScriptLineClick(idx)"
                  ><!-- sv-line 为 white-space:pre，插值必须紧贴标签，避免格式化空白泄入渲染 -->
                    <template v-if="callLinks[idx]">{{ callLinks[idx].prefix }}<span class="call-link" title="点击预览脚本内容" @click.stop="openCallPreview(callLinks[idx].name)">{{ callLinks[idx].label || callLinks[idx].name }}</span>{{ callLinks[idx].suffix }}</template>
                    <template v-else>{{ line || ' ' }}</template>
                  </div>
                </div>
              </div>
            </template>
          </div>

          <!-- 脚本功能：编辑模式（新建脚本） -->
          <div v-else class="script-edit">
            <div class="edit-name-row">
              <input v-model="editScriptName" class="input mono" placeholder="脚本名称（可省略 .yml 后缀）" @keydown.enter="saveEditScript" />
            </div>
            <div class="edit-actions">
              <button class="btn btn-primary" :disabled="scriptSaving" @click="saveEditScript">{{ scriptSaving ? '保存中…' : '💾 保存' }}</button>
              <button class="btn" @click="cancelEditScript">取消</button>
              <button class="btn" :class="{ active: altMode }" @click="toggleAltMode" title="开启后投屏点击/滑动只生成操作记录，不发送控制指令">⌥ alt 模式</button>
            </div>
            <div class="op-record">
              <div v-if="!opRecords.length" class="op-record-empty">请在alt模式下进行操作生成记录</div>
              <div v-for="r in opRecords" :key="r.id" class="op-record-line mono" @click="applyOpRecord(r)">
                {{ r.text }}
              </div>
            </div>
            <textarea ref="scriptEditor" v-model="editScriptCode" class="script-editor mono" spellcheck="false" placeholder="# YAML 脚本&#10;config:&#10;  interval: 500ms&#10;&#10;steps:&#10;  - find: 模板名.png&#10;    block: 障碍模板.png" @keydown.tab.prevent="onEditorTab"></textarea>
          </div>
          </template>
        </div>
      </div>
    </aside>

    <!-- call 子脚本预览弹窗（ESC / ✕ / 点遮罩关闭） -->
    <div v-if="previewScript" class="modal-mask" @click.self="closeCallPreview">
      <div class="modal preview-modal">
        <div class="modal-head">
          <span class="title mono">{{ previewScript.name }}</span>
          <button class="btn btn-ghost btn-sm" @click="closeCallPreview">✕</button>
        </div>
        <div class="modal-body">
          <pre class="preview-code mono">{{ previewScript.content }}</pre>
        </div>
      </div>
    </div>
  </div>
</template>

<script>
// 应用列表缓存：设备/地址 -> { list, ts }，应用列表不常变，避免每次重复读取
const appCache = new Map()
const APP_CACHE_TTL = 5 * 60 * 1000
</script>

<script setup>
import { ref, reactive, computed, watch, nextTick, onMounted, onUnmounted, inject } from 'vue'
import { pinyin } from 'pinyin-pro'
import { useRouter } from 'vue-router'
import { store, devicesData, scriptsData, templatesData, useToast } from '../store'
import { api } from '../api'
import ScriptPicker from '../components/ScriptPicker.vue'
import { createScriptValidator } from '../script-language/validate'
import { computeRunLineMap } from '../script-language/line-map'

const router = useRouter()
const toast = useToast()

// 侧边栏收起状态（MainLayout provide）：收起时释放的宽度让给右侧操作区，投屏区保持不变
const sidebarCollapsed = inject('sidebarCollapsed', ref(false))

const videoWrap = ref(null)
const videoElement = ref(null)

const connected = ref(false)
const connecting = ref(false)
const errorMsg = ref('')
const fps = ref(0)
const delay = ref(0)
// 延迟看门狗计数：连续两次采样（~4s）延迟超阈值才重连，防瞬时抖动误触发
let delaySpikes = 0
const res = ref('—')
const bitrate = ref('—')
const selScript = ref('')
// 脚本页签：运行/编辑模式 + 日志级别
const DEFAULT_SCRIPT_CODE = `config:
  interval: 500ms

steps:
  - wait: 1s
  - log: "脚本开始运行"
`
// 脚本页签当前应用分区（= 应用包名）：默认/自动跟随设备页签配置的 pkg，可手动切换；
// 模板列表、脚本选择、模板/脚本读写都按该分区进行（后端 data/<pkg>/tmpl|yaml）
const activePkg = ref('')
const scriptMode = ref('run')
const editScriptName = ref('新脚本')
const editScriptCode = ref(DEFAULT_SCRIPT_CODE)
// 编辑区 textarea：追加操作记录时读取光标位置
const scriptEditor = ref(null)
// 编辑模式当前编辑的脚本 id（null=新建）
const editScriptId = ref(null)
const scriptSaving = ref(false)
// 操作记录 YAML 模板：alt 模式把操作追加到编辑区时使用的格式。
// 由服务端 config.toml 的 [op_templates] 配置，前端启动时拉取；失败时用内置默认。
// 占位符：{name} 模板名 · {x}/{y} 点击坐标 · {fx}/{fy}/{tx}/{ty} 滑动起终点 ·
//         {time} 滑动实际时长 ms · {color} 二次裁切区点击处采样的十六进制颜色
//         搜索区域不再有占位符：由模板名 #后缀（hp#l / xx#0_0_500_500）决定，引擎自动解析
// 生成的操作记录不写等待参数：步骤间不再统一等待，轮询间隔由 config interval 控制
const DEFAULT_OP_TPL = {
  find: '- find: {name}',
  tap: '- tap: [{x}, {y}]',
  color: '- color: [{x}, {y}]\n  {color}:',
  swipe: '- swipe:\n    fm: [{fx}, {fy}]\n    to: [{tx}, {ty}]\n    time: {time}ms'
}
const opTpls = reactive({ ...DEFAULT_OP_TPL })
api.getOpTemplates().then(t => {
  if (!t || typeof t !== 'object') return
  for (const k of Object.keys(DEFAULT_OP_TPL)) {
    if (typeof t[k] === 'string' && t[k].trim()) opTpls[k] = t[k]
  }
}).catch(e => {
  console.warn('op-templates 拉取失败，使用内置默认模板（服务端未重启或接口缺失）', e)
})
/** 用变量渲染操作记录模板；未提供的占位符保留原样。
 *  值为空的占位符若独占一行（如省略 region）则整行删除；
 *  多行值（如 {region} 的 fm/to 续行）缩进跟随占位符所在行 +2（吞掉值自带的续行缩进），保证嵌套层级正确 */
function renderOpTpl(tpl, vars) {
  let out = tpl || ''
  for (const [k, v] of Object.entries(vars)) {
    const val = v ?? ''
    out = out.split('\n').map(line => {
      if (!line.includes('{' + k + '}')) return line
      if (String(val).trim() === '' && line.replace('{' + k + '}', '').trim() === '') return null
      const indent = (line.match(/^(\s*)/) || ['', ''])[1]
      return line.split('{' + k + '}').join(val.replace(/\n\s*/g, '\n' + indent + '  '))
    }).filter(l => l !== null).join('\n')
  }
  return out.replace(/\n{3,}/g, '\n\n').trim()
}
// alt 模式：仅在脚本编辑模式生效；开启后模板/投屏点击只生成操作记录
const altMode = ref(false)
// 操作记录区：最多展示 3 行，每行可点击追加到编辑区
const opRecords = ref([])
let opRecordSeq = 0
// alt 手势（点击/滑动投屏时记录，不发送控制指令）
const altGesture = reactive({ active: false, moved: false, start: { x: 0, y: 0 }, last: { x: 0, y: 0 }, startT: 0 })
// alt 模式点击/滑动画面反馈（点击圆点 / 滑动 region 框）
const altFeedback = reactive({ show: false, kind: '', x: 0, y: 0, w: 0, h: 0 })
let altFeedbackTimer = null
// 脚本运行可视化效果：服务端经 control DataChannel 推送 tap/swipe/hit/miss 事件（设备像素坐标），
// 与手动 alt 反馈状态独立（脚本运行时用户仍可手动操作，两类效果互不覆盖）
const scriptFx = reactive({
  tap: { show: false, x: 0, y: 0 },
  swipe: { show: false, x: 0, y: 0, w: 0, h: 0 },
  hit: { show: false, x: 0, y: 0, w: 0, h: 0, label: '', miss: false },
})
let fxTapTimer = null
let fxSwipeTimer = null
let fxHitTimer = null
// 日志原始数据（未过滤），用于按级别切换显示
let rawLogs = []
// 本次运行开始时间：清空日志区后只显示本次运行产生的日志
let runStartTime = 0
const picking = ref(false)
const testThreshold = ref(0.8)
// 模板匹配区域：'' = 默认（按模板名自动识别），否则 a/u/d/l/r/ul/ur/dl/dr（测试匹配与生成记录共用）
const testRegion = ref('')
// 模板列表：查看大图 / 删除二次确认 / 重命名
const viewTpl = ref(null)
const confirmDelTpl = ref(null)
const renaming = ref(null)   // 正在重命名的模板名（null=不在重命名）
const renameVal = ref('')    // 重命名输入框内容
let renameInputEl = null     // 重命名输入框元素（自动聚焦/全选）
const selecting = ref(false)
const selStart = reactive({ x: 0, y: 0 })
const selEnd = reactive({ x: 0, y: 0 })
const showHit = ref(false)
const hit = reactive({ x: 0, y: 0, w: 0, h: 0 })
const hitLabel = ref('')
// true = 展示的是未命中的搜索区域框（虚线红），false = 命中框（实线绿）
const hitMiss = ref(false)
let hitTimer = null
const liveLogs = ref([])
const logBox = ref(null)
// 二次裁切（右侧面板）
const crop = reactive({ active: false, imgW: 0, imgH: 0, baseW: 0, baseH: 0, originX: 0, originY: 0, rect: { x: 0, y: 0, w: 0, h: 0 }, preview: '', name: '', zoom: 1 })
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

// ---------- 多页面互斥 + 自动重连 ----------
// 同一设备同一时刻只有一个活跃 viewer，由服务端 viewers 注册表仲裁
// （取代旧 localStorage 锁——它只能管同一浏览器，跨浏览器/跨 PC 管不到）：
//  - 新页面连接时若已有活跃 viewer：服务端回 conflict → 手动连接（点连接按钮）
//    弹窗确认后带 force 重发 offer 顶替；仅换浏览器↔服务端链路，设备 scrcpy
//    会话保持不断，新 viewer 无缝接管
//  - 被顶替页面经信令 ws 收到 taken_over → 直接断开且不再自动重连（防互顶）
//  - 自动重连（非手动）遇 conflict → 直接放弃并提示（不弹窗、不抢连接）
let reconnectTimer = null
let reconnectAttempts = 0
let manualClose = false
// 被其他页面 force 顶替（已收 taken_over）：断开后不再自动重连
let superseded = false
// 本次 offer 是否带 force（conflict 确认后顶替重连用）
let forceTakeover = false

/** 被动断开后的自动重连调度：被顶替不重连，否则按退避时间重连 */
function scheduleReconnect() {
  if (reconnectTimer || !store.deviceId) return
  if (superseded) {
    errorMsg.value = '连接已被其他页面接管'
    return
  }
  const delay = [3000, 6000, 12000][Math.min(reconnectAttempts, 2)]
  reconnectAttempts++
  toast(`连接已断开，${delay / 1000} 秒后自动重连…`, 'warn')
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null
    if (superseded) {
      errorMsg.value = '连接已被其他页面接管'
      return
    }
    connect(false) // 自动重连非 force：他页已连接时服务端回 conflict → 放弃
  }, delay)
}

function onChannelOpen() {
  connected.value = true
  connecting.value = false
  reconnectAttempts = 0
  videoConnectTs = Date.now()
  // 音频按需发送：告知服务端本页是否要音频（默认静音 → 服务端零音频包）。
  // 仅靠 track.enabled=false 不够：部分浏览器内核仍把音频流选为 A/V 同步
  // 主时钟，虚拟屏音频时钟慢漂会把视频延迟单调拉高（见 toggleAudio 注释）
  sendControl({ type: 'audio', on: !audioMuted.value })
  toast('WebRTC 连接建立', 'success')
}

function onChannelClose() {
  connected.value = false
  // 被顶替（taken_over）不自动重连，防互顶死循环
  if (!manualClose && !superseded) scheduleReconnect()
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
// 模板模糊搜索词（短名/带 #后缀 全名均可命中）
const tplSearch = ref('')
// 模板名拼音首字母缓存（非汉字字符原样保留）：「日常遗器.png」→ "rcyq.png"，供搜索匹配
const tplPyCache = new Map()
function tplPinyinInitials(name) {
  let s = tplPyCache.get(name)
  if (s === undefined) {
    s = pinyin(name, { pattern: 'first', toneType: 'none', type: 'array' })
      .join('').replace(/\s+/g, '').toLowerCase()
    tplPyCache.set(name, s)
  }
  return s
}

// 模板列表：当前应用分区过滤（templatesData 为跨分区全量，条目带 pkg 字段）；
// 有搜索词时三口径并列匹配（全名/短名子串 + 中文名拼音首字母），任一命中即展示，
// 排序按最早命中位置（拼音命中加偏移恒排文字命中之后），同级按修改时间倒序；无搜索词按修改时间倒序
const templates = computed(() => {
  let list = templatesData.value.filter(t => t.pkg === activePkg.value)
  const q = tplSearch.value.trim().toLowerCase()
  if (q) {
    // 首字母串不含中文，查询词含中文时跳过该口径（必然无交集）
    const pyAble = !/[\u4e00-\u9fff]/.test(q)
    const PY_OFFSET = 1e4
    list = list.map(t => {
      let idx = t.name.toLowerCase().indexOf(q)
      const si = tplShortName(t.name).toLowerCase().indexOf(q)
      if (idx === -1 || (si !== -1 && si < idx)) idx = si
      if (idx === -1 && pyAble) {
        const pi = tplPinyinInitials(t.name).indexOf(q)
        if (pi !== -1) idx = PY_OFFSET + pi
      }
      return idx === -1 ? null : { t, idx }
    }).filter(Boolean).sort((a, b) => a.idx - b.idx || (b.t.mtime || 0) - (a.t.mtime || 0)).map(x => x.t)
  } else {
    list = list.sort((a, b) => (b.mtime || 0) - (a.mtime || 0))
  }
  return list
})

/** 应用分区下拉选项：设备页签配置的包名 ∪ 脚本分区 ∪ 模板分区（字典序） */
const pkgOptions = computed(() => {
  const set = new Set()
  const dp = (form.pkg || '').trim()
  if (dp) set.add(dp)
  for (const s of scripts.value) if (s.package) set.add(s.package)
  for (const t of templatesData.value) if (t.pkg) set.add(t.pkg)
  return [...set].sort((a, b) => a.localeCompare(b))
})

// 设备页签应用包名变化（含未保存草稿、切换设备）→ 分区自动跟随；
// 清空包名时保持当前分区（磁盘分区仍在），仅从未选择时兜底选第一个分区
watch(() => form.pkg, v => {
  const t = (v || '').trim()
  if (t) activePkg.value = t
  else if (!activePkg.value) activePkg.value = pkgOptions.value[0] || ''
})
watch(pkgOptions, list => {
  if (!activePkg.value) activePkg.value = list[0] || ''
  else if (!list.includes(activePkg.value)) activePkg.value = list[0] || ''
})

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

/** 主动断开（只停本页 WebRTC，不拆服务端↔设备会话，不触发自动重连；
 *  设备会话由服务端空闲低功耗统一管理：无 viewer 无脚本 5 分钟后
 *  虚拟屏拆会话/镜像关屏） */
function disconnect() {
  if (!store.deviceId) return
  if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null }
  cleanup(true)
  toast('已断开投屏（设备会话保留）', 'info')
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

/** 设备像素矩形 → 显示坐标样式（object-fit: contain 的 letterbox 映射；脚本事件效果用） */
function deviceRectStyle(x, y, w = 0, h = 0) {
  const vw = videoWrap.value
  if (!vw) return {}
  const rect = vw.getBoundingClientRect()
  const vw_ = rect.width, vh = rect.height
  const sw = videoElement.value?.videoWidth || 1920
  const sh = videoElement.value?.videoHeight || 1080
  const ratio = Math.min(vw_ / sw, vh / sh)
  return {
    left: (x * ratio) + (vw_ - sw * ratio) / 2 + 'px',
    top: (y * ratio) + (vh - sh * ratio) / 2 + 'px',
    width: w * ratio + 'px',
    height: h * ratio + 'px',
  }
}

/** 脚本运行可视化效果位置（tap 圆点居中偏移由 .alt-tap 的 transform 处理） */
const fxTapStyle = computed(() => (scriptFx.tap.show ? deviceRectStyle(scriptFx.tap.x, scriptFx.tap.y) : {}))
const fxSwipeStyle = computed(() => (scriptFx.swipe.show
  ? deviceRectStyle(scriptFx.swipe.x, scriptFx.swipe.y, scriptFx.swipe.w, scriptFx.swipe.h)
  : {}))
const fxHitStyle = computed(() => (scriptFx.hit.show
  ? deviceRectStyle(scriptFx.hit.x, scriptFx.hit.y, scriptFx.hit.w, scriptFx.hit.h)
  : {}))

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
    // 静音时禁用音频轨（关键）：音频轨参与浏览器 A/V 同步（主时钟），scrcpy
    // 虚拟屏音频流在 Chrome 侧播放时钟异常会把视频 jitter buffer 目标延迟单调
    // 拉高——挂机（静止画面）时延迟从 ~87ms 累积到 3s+ 且不回落（见 AGENTS.md
    // 已知坑）。禁用该轨后视频独立播放，延迟不再累积；要听声音再启用。
    if (mediaStream) {
      for (const t of mediaStream.getAudioTracks()) t.enabled = !audioMuted.value
    }
    // 取消静音时浏览器要求用户手势后播放（已处于点击事件内，直接 play 即可）
    if (!audioMuted.value) v.play().catch(() => {})
  }
  // 同步服务端音频转发开关（默认不发音频，开启后才开始转发）
  if (connected.value) sendControl({ type: 'audio', on: !audioMuted.value })
}

async function connect(manual = false) {
  // 幂等：同步锁 + 状态检查，杜绝并发/重复调用创建多个 PC
  // （服务端会因多连接出现多推流，video.srcObject 被串流覆盖 → 画面定格）
  if (connectLock || connecting.value || connected.value) {
    console.warn('[webrtc] connect ignored (lock/connecting/connected)')
    return
  }
  connectLock = true
  console.log('[webrtc] connect called (pc exists:', !!pc, ')')
  try {
    await doConnect()
  } catch (e) {
    if (e && e.conflict) {
      // 已有其他页面在投屏：手动连接（点连接按钮）→ 弹窗确认后 force 顶替；
      // 自动重连 → 直接放弃（不弹窗、不抢连接，防互顶死循环）
      if (manual) {
        if (confirm(`设备 ${currentName.value} 正在其他页面投屏。\n\n确认接管连接？对方页面将断开且不会自动重连。`)) {
          forceTakeover = true
          try {
            await doConnect()
          } finally {
            forceTakeover = false
          }
        } else {
          connecting.value = false
          errorMsg.value = '设备正在其他页面使用'
        }
      } else {
        connecting.value = false
        errorMsg.value = '设备已在其他页面连接，本页已停止重连'
        toast(errorMsg.value, 'warn')
      }
    }
    // 常规错误：doConnect 内部已置 errorMsg 并 cleanup
  } finally {
    connectLock = false
  }
}

async function doConnect() {
  if (!store.deviceId) return toast('请先选择设备（设备页签下拉框）', 'error')
  // 重连场景：若有残留 pc（连接失败但未清理干净），先释放（主动关闭，不触发自动重连）
  if (pc) cleanup(true)
  superseded = false
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
    controlChannel.onmessage = onControlMessage

    pc.ontrack = (e) => {
      // 只接受当前 pc 的轨道：残留/旧连接的 ontrack 不得覆盖 srcObject（串流 → 定格）
      if (e.target !== pc) return
      // 兜底：对端 SDP 无 a=msid 时 e.streams 可能为空，用 track 自建 MediaStream
      mediaStream = e.streams[0] || new MediaStream([e.track])
      // 默认静音场景直接禁用音频轨（A/V 同步拖延迟问题，见 toggleAudio 注释）
      if (e.track.kind === 'audio') e.track.enabled = !audioMuted.value
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
      controlChannel.onmessage = onControlMessage
    }

    // 4. offer 交换（force: conflict 确认后的顶替重连；首轮 false——
    //    他页已连接时服务端回 conflict，由 connect() 弹窗协商）
    const offer = await pc.createOffer()
    await pc.setLocalDescription(offer)
    const answer = await new Promise((resolve, reject) => {
      ws.onmessage = (evt) => {
        try {
          const msg = JSON.parse(evt.data)
          if (msg.type === 'answer') resolve(msg.sdp)
          else if (msg.type === 'conflict') reject({ conflict: true })
          else if (msg.type === 'error') reject(new Error(msg.error || '信令错误'))
        } catch (e) { reject(e) }
      }
      ws.send(JSON.stringify({ type: 'offer', sdp: offer, force: forceTakeover }))
      setTimeout(() => reject(new Error('信令超时')), 10000)
    })
    await pc.setRemoteDescription(new RTCSessionDescription(answer))

    // 信令 ws 后续消息：被其他页面 force 顶替的通知（收到后本页断开且不再
    // 自动重连，防互顶；随后 peer close 由 onChannelClose 兜底清理）
    ws.onmessage = (evt) => {
      try {
        const msg = JSON.parse(evt.data)
        if (msg.type === 'taken_over') {
          superseded = true
          console.warn('[webrtc] taken over by another page')
          toast('连接已被其他页面接管', 'warn')
        }
      } catch (e) {}
    }

    // 5. 统计定时器
    startStats()
    startLogPolling()
  } catch (e) {
    console.error('webrtc connect:', e)
    connecting.value = false
    if (e && e.conflict) {
      cleanup(true)
      throw e // conflict 上抛给 connect()：手动连接弹窗确认接管 / 自动重连放弃
    }
    errorMsg.value = e.message
    cleanup(true)
  }
}

/** 释放 WebRTC 资源；manual=true 表示主动关闭（不触发自动重连） */
function cleanup(manual = false) {
  if (statsTimer) { clearInterval(statsTimer); statsTimer = null }
  if (logTimer) { clearInterval(logTimer); logTimer = null }
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
  renderFpLast = ''
  renderFpFrozen = 0
  stallResetSent = false
  lastBytesReceived = 0
  lastBitrateTs = 0
  bitrate.value = '—'
  // 清空画面：断开后避免旧帧定格残留（overlay 会显示连接提示）
  if (videoElement.value) videoElement.value.srcObject = null
  hideLoupe()
}

// ---------- 视频静默检测 ----------
// 被踢/断流时服务端不再向本页推帧（或已推给其他页面）→ 判定断流，走与
// onclose 相同的自动重连逻辑（带页面锁检查）。
// 双条件（currentTime 冻结 && 统计窗口零新增字节）：静止屏下服务端补帧重发
// 的是重复 P 帧——H.264 相同 frame_num 的重复 slice 会被 Chrome 静默丢弃
// （不解码、currentTime 不推进），画面定格本就是静态屏的正确渲染，字节仍在
// 到达 = 链路活着，不算断流；真断流（被踢/断链/scrcpy 死）两者同时冻结
let lastVideoTime = 0
let stillFrames = 0
let hadVideo = false
// 上一统计窗口视频轨是否有新增字节（链路活性，见上方注释）
let videoBytesAdvanced = false
// 传输码率统计（按两次 getStats 的 bytesReceived 差值计算）
let lastBytesReceived = 0
let lastBitrateTs = 0
// 画面延迟统计：jitterBufferDelay 增量 / 新播出帧数 = 每帧在 jitter buffer 的平均停留
// 时间（≈ 画面滞后于设备的时间下限；服务端推流节奏正常时 ~100-300ms）
let lastJbd = 0
let lastJbe = 0
// 连接建立时间：用于"连接后长时间无视频帧（黑屏）"看门狗
let videoConnectTs = 0
// PLI 自愈：浏览器解码器失步（花屏/卡顿）时自动发 RTCP PLI 请求关键帧，
// 但服务端（webrtc-rs）不响应 PLI，只能等设备固定 IDR（i-frame-interval=2s）——
// 花屏最长要 2s 才恢复。检测 inbound-rtp.pliCount 增量 → 经 control DataChannel
// 通知服务端 reset_video（scrcpy 控制消息 17）→ 设备立即输出新 config+IDR → ~200ms 恢复
let lastPliCount = 0
let lastPliResetAt = 0
// PLI reset 退避：reset 后 pliCount 仍在涨 = reset 无效（黑屏/静态屏编码器
// 不吐 IDR），连续无效就指数退避（2s → 15s → 60s），避免每 3s 重启一次编码器
let pliResetStreak = 0

// 画面停滞看门狗（2026-08-23）：长时间静止补帧 + 运动突发后，Chrome jitter
// buffer 的目标延迟可膨胀到秒级（实测静止 23min 后 676ms，滚动突发后飙到 4.9s），
// 表现为"包在到、framesDecoded 在涨、画面却逐位冻结或残缺（花屏）"——此时
// currentTime 照常 1.0x 推进、bytesReceived 照常增长、pliCount 不动（静默参考
// 链损坏不触发 PLI），静默/延迟/PLI 三个现有看门狗全部失明，用户只能手动刷新。
// 唯一彻底解药是重连（重建 jitter buffer，实测重连后同场景恢复正常渲染）。
// 检测：用户刚做过拖动/滚轮（预期画面变化）但渲染像素指纹连续 ~5s 逐位未变
// → 先 reset_video 请求 IDR；仍冻结则重连。指纹取 24x14 亮度哈希，开销可忽略。
let renderFpLast = ''
let renderFpFrozen = 0
let lastDragInputAt = 0
let stallResetSent = false
let fpCanvas = null
let fpCtx = null

function handleVideoSilence() {
  if (manualClose || !connected.value || !store.deviceId) return
  console.warn('[webrtc] video stream silent, treating as disconnected')
  connected.value = false
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
      if (Math.abs(v.currentTime - lastVideoTime) < 0.001 && !videoBytesAdvanced) {
        if (++stillFrames >= 2) { // 连续 ~4s：currentTime 冻结且零新增字节
          stillFrames = 0
          handleVideoSilence()
        }
      } else {
        stillFrames = 0
        lastVideoTime = v.currentTime
      }
      // 画面停滞看门狗（原理见变量处注释）：渲染像素指纹连续未变 + 近期有
      // 拖动/滚轮输入（画面本应变化）。先 reset_video，5s 后仍冻结才重连，
      // 避免对"拖不动/本就静止"的界面频繁误重连
      if (connected.value && hadVideo) {
        let fp = ''
        try {
          if (!fpCanvas) {
            fpCanvas = document.createElement('canvas')
            fpCanvas.width = 24; fpCanvas.height = 14
            fpCtx = fpCanvas.getContext('2d', { willReadFrequently: true })
          }
          fpCtx.drawImage(v, 0, 0, 24, 14)
          const d = fpCtx.getImageData(0, 0, 24, 14).data
          let h = 5381
          for (let i = 0; i < d.length; i += 4) h = ((h * 33) ^ (d[i] + d[i+1] + d[i+2])) >>> 0
          fp = String(h)
        } catch (err) { /* drawImage 失败（如视频未就绪）跳过本轮 */ }
        if (fp && fp === renderFpLast) {
          renderFpFrozen++
        } else {
          renderFpFrozen = 0
          if (fp) stallResetSent = false
        }
        renderFpLast = fp
        if (renderFpFrozen >= 5 && Date.now() - lastDragInputAt < 8000) {
          renderFpFrozen = 0
          if (!stallResetSent) {
            stallResetSent = true
            console.warn('[webrtc] picture frozen after drag/scroll input, requesting IDR via reset_video')
            sendControl({ type: 'reset_video' })
          } else {
            stallResetSent = false
            console.warn('[webrtc] picture still frozen after reset_video, reconnecting to rebuild jitter buffer')
            handleVideoSilence()
          }
        }
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
              // 延迟看门狗：音频轨 A/V 同步异常会把 jitter buffer 目标延迟单调拉高
              // （挂机静止画面 87ms → 3s+ 且不回落，见 AGENTS.md 已知坑）。连续两次
              // 采样超阈值（~4s）→ 走断流重连路径重置缓冲（含页面锁二次检查）
              if (delay.value > 1500) {
                if (++delaySpikes >= 2) {
                  delaySpikes = 0
                  console.warn('[webrtc] latency watchdog: delay=' + delay.value + 'ms, reconnecting')
                  handleVideoSilence()
                  return
                }
              } else {
                delaySpikes = 0
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
            // 链路活性（静默检测双条件用，见检测处注释）；重连后计数回退
            videoBytesAdvanced = s.bytesReceived > lastBytesReceived
            if (s.bytesReceived < lastBytesReceived) lastBytesReceived = 0
            lastBytesReceived = s.bytesReceived
            lastBitrateTs = now
          }
          // 花屏自愈：解码器失步（PLI 增量）→ 请求设备立即出关键帧。
          // 限频 2s：持续丢包（WiFi 差）时最多每 2s 重置一次，不会打爆编码器
          if (typeof s.pliCount === 'number') {
            if (s.pliCount < lastPliCount) lastPliCount = s.pliCount // 重连后回退
            if (s.pliCount > lastPliCount) {
              lastPliCount = s.pliCount
              // 连接初期（~6s 内）Chrome 加入流时会例行发 PLI 请求关键帧，不是失步：
              // 静态屏（无应用/挂机静止）编码器对 reset 响应极慢（MTK 要多次才吐
              // IDR），reset 反而打断静止补帧 → 浏览器断供 4s 被静默检测杀掉 →
              // "连上一会儿就断"死循环。真失步（解码中突发花屏）不受此窗口限制
              const joinWindow = Date.now() - videoConnectTs < 6000
              const now = Date.now()
              const backoff = pliResetStreak >= 4 ? 60000 : pliResetStreak >= 2 ? 15000 : 2000
              if (!joinWindow && connected.value && now - lastPliResetAt > backoff) {
                lastPliResetAt = now
                pliResetStreak++
                console.warn('[webrtc] decoder desync (pliCount=' + s.pliCount + ', streak=' + pliResetStreak + '), requesting IDR via reset_video')
                sendControl({ type: 'reset_video' })
              }
            } else if (s.pliCount === lastPliCount && lastPliCount > 0) {
              // 一整个统计周期无新 PLI：解码器已满足，退避复位
              pliResetStreak = 0
            }
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
    // 1s 轮询：比 2s 更快发现花屏（PLI 自愈延迟减半）与延迟/静默异常
  }, 1000)
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
  // 日志级别由脚本顶层 log_level 在服务端过滤（debug/info），前端只按运行开始时间截取
  const filtered = (rawLogs || []).filter(l => {
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
  // 拖动/滚轮类输入打标（画面停滞看门狗用）：这类操作预期画面变化，
  // 若随后渲染指纹持续冻结则流已病态（见 startStats 处注释）
  if ((obj.type === 'touch' && obj.action === 'move') || obj.type === 'scroll' || obj.type === 'swipe') {
    lastDragInputAt = Date.now()
  }
  if (controlChannel && controlChannel.readyState === 'open') {
    controlChannel.send(JSON.stringify(obj))
    return true
  }
  console.warn('[control] channel not open, fallback REST', JSON.stringify(obj))
  // fallback：REST API
  api.control(store.deviceId, obj).catch(e => toast('控制失败：' + e.message, 'error'))
  return false
}

/** 服务端→浏览器脚本可视化事件（{"type":"se","ev":"tap"|"swipe"|"hit"|"miss", ...}，设备像素坐标）：
 *  引擎执行 tap/swipe、模板匹配命中/未命中时推送到投屏画面
 *  （样式复用 alt 反馈/测试匹配命中框；miss 显示搜索区域，虚线红框）
 *  同一轮匹配的多个模板事件会互相顶替，显示的是最新一次 */
function onControlMessage(e) {
  let msg
  try { msg = JSON.parse(e.data) } catch (err) { return }
  if (!msg || msg.type !== 'se') return
  if (msg.ev === 'tap') {
    scriptFx.tap.x = msg.x || 0
    scriptFx.tap.y = msg.y || 0
    scriptFx.tap.show = true
    if (fxTapTimer) clearTimeout(fxTapTimer)
    fxTapTimer = setTimeout(() => { scriptFx.tap.show = false }, 2000)
  } else if (msg.ev === 'swipe') {
    const { x1 = 0, y1 = 0, x2 = 0, y2 = 0 } = msg
    scriptFx.swipe.x = Math.min(x1, x2)
    scriptFx.swipe.y = Math.min(y1, y2)
    scriptFx.swipe.w = Math.abs(x2 - x1)
    scriptFx.swipe.h = Math.abs(y2 - y1)
    scriptFx.swipe.show = true
    if (fxSwipeTimer) clearTimeout(fxSwipeTimer)
    fxSwipeTimer = setTimeout(() => { scriptFx.swipe.show = false }, 2000)
  } else if (msg.ev === 'hit') {
    scriptFx.hit.x = msg.x || 0
    scriptFx.hit.y = msg.y || 0
    scriptFx.hit.w = msg.w || 0
    scriptFx.hit.h = msg.h || 0
    scriptFx.hit.label = `${msg.tpl || ''} ${Number(msg.score || 0).toFixed(2)}`
    scriptFx.hit.miss = false
    scriptFx.hit.show = true
    if (fxHitTimer) clearTimeout(fxHitTimer)
    fxHitTimer = setTimeout(() => { scriptFx.hit.show = false }, 3000)
  } else if (msg.ev === 'miss') {
    // 未命中：显示本次搜索区域（引擎无 #后缀回退全屏时推 [0,0,w,h] 全屏框）
    scriptFx.hit.x = msg.x || 0
    scriptFx.hit.y = msg.y || 0
    scriptFx.hit.w = msg.w || 0
    scriptFx.hit.h = msg.h || 0
    scriptFx.hit.label = `${msg.tpl || ''} 未命中`
    scriptFx.hit.miss = true
    scriptFx.hit.show = true
    if (fxHitTimer) clearTimeout(fxHitTimer)
    fxHitTimer = setTimeout(() => { scriptFx.hit.show = false }, 3000)
  }
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
    altGesture.startT = Date.now()
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
    const dur = Math.max(50, Date.now() - altGesture.startT)
    altGesture.active = false
    if (moved) setSwipeRecords(start, { x, y }, dur)
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

/** 生成默认模板名：随机名字#x1_y1_x2_y2（相对坐标 0~1，×1000 存 3 位整数，如 0.123→123，不带 .png 后缀） */
function defaultTplName(rect) {
  const vw = videoElement.value?.videoWidth || 1920
  const vh = videoElement.value?.videoHeight || 1080
  // ×1000 取整，3 位定宽补零；1.0 边缘收敛到 999，避免出现 4 位数与旧格式混淆
  const toInt3 = v => String(Math.min(999, Math.round(v * 1000))).padStart(3, '0')
  const x1 = toInt3(rect.x / vw)
  const y1 = toInt3(rect.y / vh)
  const x2 = toInt3((rect.x + rect.w) / vw)
  const y2 = toInt3((rect.y + rect.h) / vh)
  return `${randomTplBase()}#${x1}_${y1}_${x2}_${y2}`
}

// ---------- 二次裁切 ----------

const cropSize = computed(() => `${Math.round(crop.rect.w)}×${Math.round(crop.rect.h)} px`)
/** 当前显示缩放（100% = 自适应适配），滚轮调整 */
const cropZoomPct = computed(() => `${Math.round(crop.zoom * 100)}%`)

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
  crop.zoom = 1
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
  crop.zoom = 1
  hideLoupe()
}

function repick() {
  crop.active = false
  cropBaseCanvas = null
  crop.zoom = 1
  picking.value = true
  toast('在画面上重新框选', 'info')
}

/** 画布适配尺寸：展示冻结的初始框选画面，可适当放大（再乘滚轮缩放 crop.zoom） */
function cropFit() {
  const w = Math.max(1, crop.baseW)
  const h = Math.max(1, crop.baseH)
  const scale = Math.min(260 / w, 220 / h, 3) * crop.zoom
  return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)), scale: Math.round(w * scale) / w }
}

/** 滚轮缩放裁切底图：以光标下的图像点为锚点放大/缩小，缩放后画布超出区域可滚动查看 */
function cropWheel(e) {
  const canvas = cropCanvas.value
  const stage = cropSec.value?.querySelector('.crop-stage')
  if (!canvas || !stage) return
  e.preventDefault()
  const factor = e.deltaY < 0 ? 1.15 : 1 / 1.15
  const next = Math.max(0.5, Math.min(8, crop.zoom * factor))
  if (next === crop.zoom) return
  const cr = canvas.getBoundingClientRect()
  const sr = stage.getBoundingClientRect()
  // 光标在画布内的位置（画布 CSS 像素 = 画布像素）
  const mx = e.clientX - cr.left
  const my = e.clientY - cr.top
  // 画布原点在滚动内容中的位置
  const ox = cr.left - sr.left + stage.scrollLeft
  const oy = cr.top - sr.top + stage.scrollTop
  const oldW = canvas.width
  const oldH = canvas.height
  crop.zoom = next
  renderCropFrame()
  const kx = canvas.width / oldW
  const ky = canvas.height / oldH
  // 保持光标下的图像点不动：margin:auto 居中时原点 = max(0, (区域宽 - 画布宽)/2)
  const ox1 = Math.max(0, (stage.clientWidth - canvas.width) / 2)
  const oy1 = Math.max(0, (stage.clientHeight - canvas.height) / 2)
  stage.scrollLeft += ox1 + mx * kx - ox - mx
  stage.scrollTop += oy1 + my * ky - oy - my
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

/** 二次裁切底图（冻结的框选画面）上 alt 点击 → 取色生成 color 颜色判断记录：
 *  颜色直接从 cropBaseCanvas 采样（所见即所得，同步生成无延迟）；
 *  点击点在底图上的设备坐标 = p + (originX, originY)，换算成相对坐标写进 color 坐标 */
function cropPickColor(e) {
  const p = cropEventDev(e)
  const base = cropBaseCanvas
  const px = Math.max(0, Math.min(base.width - 1, Math.round(p.x)))
  const py = Math.max(0, Math.min(base.height - 1, Math.round(p.y)))
  const g = base.getContext('2d', { willReadFrequently: true })
  const d = g.getImageData(px, py, 1, 1).data
  const hex = [d[0], d[1], d[2]].map(v => v.toString(16).padStart(2, '0')).join('')
  const vw = crop.imgW || 1920
  const vh = crop.imgH || 1080
  const rx = ((crop.originX + px) / vw).toFixed(4)
  const ry = ((crop.originY + py) / vh).toFixed(4)
  opRecords.value = [
    { id: ++opRecordSeq, text: `- color #${hex} @ (${rx}, ${ry})`, yaml: renderOpTpl(opTpls.color, { x: rx, y: ry, color: hex }) }
  ]
  toast(`已生成 ${hex} 的颜色判断记录，点击选择追加`, 'success')
}

function cropMouseDown(e) {
  // Alt/alt 模式点击 → 取色生成 color 颜色判断记录（底图坐标 = 冻结的框选画面，
  // 颜色直接从 cropBaseCanvas 采样——与服务端截图同源 YUV→RGB 体系有差异，
  // 但二次裁切底图就是浏览器画面本身，此处取的是"所见即所得"）
  if (isAltAction(e) && cropBaseCanvas) {
    cropPickColor(e)
    e.preventDefault()
    return
  }
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

/** 上传响应的体积提示：823KB → 96KB（服务端灰度 PNG 重编码） */
function tplSizeHint(rep) {
  if (!rep?.size || !rep?.orig_size) return ''
  const fmt = n => n >= 1024 * 1024 ? (n / 1024 / 1024).toFixed(1) + 'MB' : n >= 1024 ? Math.round(n / 1024) + 'KB' : n + 'B'
  return `（${fmt(rep.orig_size)} → ${fmt(rep.size)}）`
}

async function saveTemplate() {
  const raw = crop.name.trim()
  if (!raw) return toast('请输入模板名称', 'warn')
  if (!activePkg.value) return toast('请先选择应用分区', 'warn')
  const name = raw.toLowerCase().endsWith('.png') ? raw : raw + '.png'
  saving.value = true
  try {
    const rep = await api.uploadTemplate(name, crop.preview.split(',')[1], activePkg.value)
    templatesData.value = await api.listTemplates()
    crop.active = false
    cropBaseCanvas = null
    hideLoupe()
    toast(`模板 ${name} 已保存${tplSizeHint(rep)}`, 'success')
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

function tplThumbUrl(name) { return api.tplImageUrl(name, activePkg.value) }

/** 模板列表：行空白区点击 → 查看大图（缩略图/文件名单元格有各自的交互） */
function onTplRowClick(e, t) {
  confirmDelTpl.value = null
  openTplView(t.name)
}

/** 模板列表缩略图：alt（按住 Alt / alt 模式）→ 复制模板名；普通 → 查看大图 */
async function onTplThumbClick(e, t) {
  confirmDelTpl.value = null
  if (isAltAction(e)) {
    const ok = await copyText(t.name)
    toast(ok ? `已复制 ${t.name}` : '复制失败', ok ? 'success' : 'warn')
    return
  }
  openTplView(t.name)
}

/** 模板列表文件名：alt → 生成 find 操作记录；普通 → 查看大图 */
function onTplNameClick(e, t) {
  if (renaming.value === t.name) return
  confirmDelTpl.value = null
  if (isAltAction(e)) {
    // 生成的记录写短名（login.png）：引擎自动解析到带 #后缀 的文件，区域照常生效
    const name = tplShortName(t.name)
    opRecords.value = [
      { id: ++opRecordSeq, text: `- find ${name}（等到出现+点击）`, yaml: renderOpTpl(opTpls.find, { name }) }
    ]
    toast(`已生成 ${name} 的 find 记录，点击选择追加`, 'success')
    return
  }
  openTplView(t.name)
}

/** 复制文本到剪贴板：navigator.clipboard 需安全上下文（localhost），
 *  LAN http 访问时回退 execCommand（临时 textarea） */
async function copyText(text) {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch { /* 回退 execCommand */ }
  const ta = document.createElement('textarea')
  ta.value = text
  ta.style.position = 'fixed'
  ta.style.opacity = '0'
  document.body.appendChild(ta)
  ta.select()
  const ok = document.execCommand('copy')
  ta.remove()
  return ok
}

/** 模板列表：查看大图 */
function openTplView(name) {
  confirmDelTpl.value = null
  viewTpl.value = name
}
function closeTplView() {
  viewTpl.value = null
}

// 模板查看：悬停坐标读数已随 until 的 click 参数删除一并移除（命中恒点模板中心）

// ---------- 模板重命名 ----------

/** 重命名输入框初始值：去掉图片后缀 */
function renameBase(name) {
  return name.replace(/\.(png|jpe?g)$/i, '')
}

function startRename(t) {
  confirmDelTpl.value = null
  renaming.value = t.name
  renameVal.value = renameBase(t.name)
  nextTick(() => renameInputEl?.select())
}

/** 输入框失焦 / Esc → 取消重命名（不保存） */
function cancelRename() {
  renaming.value = null
}

/** 确认重命名：名称去空格、自动补 .png 后缀、重名校验，成功后刷新列表 */
async function confirmRename(t) {
  const raw = renameVal.value.trim()
  if (!raw) return toast('名称不能为空', 'warn')
  const newName = /\.(png|jpe?g)$/i.test(raw) ? raw : raw + '.png'
  renaming.value = null
  if (newName === t.name) return
  if (templatesData.value.some(x => x.pkg === activePkg.value && x.name === newName)) return toast(`已存在同名模板：${newName}`, 'warn')
  try {
    await api.renameTemplate(t.name, newName, activePkg.value)
    templatesData.value = await api.listTemplates()
    toast(`模板已重命名为 ${newName}`, 'success')
  } catch (e) {
    toast('重命名失败：' + e.message, 'error')
  }
}

/** 模板列表：匹配按钮（测试匹配） */
function onTplMatchClick(t) {
  confirmDelTpl.value = null
  testMatch(t.name)
}

/** 模板列表：删除按钮（第一次变确认，第二次删除；其他操作自动取消） */
async function onTplDeleteClick(t) {
  if (confirmDelTpl.value === t.name) {
    confirmDelTpl.value = null
    try {
      await api.deleteTemplate(t.name, activePkg.value)
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
    const rep = await api.uploadTemplate(name, b64, activePkg.value)
    templatesData.value = await api.listTemplates()
    toast(`模板已上传${tplSizeHint(rep)}`, 'success')
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

/** 全局按键：Esc 关闭 call 子脚本预览 / 模板大图 / 取消删除确认 */
function onGlobalKeydown(e) {
  if (e.key !== 'Escape') return
  if (previewScript.value) {
    closeCallPreview()
  } else if (viewTpl.value) {
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

/** 从模板名解析 #x1_y1_x2_y2（相对坐标 ×1000 存 3 位整数，如 123→0.123），返回 [x1,y1,x2,y2] 或 null */
function parseTplRegion(name) {
  const base = name.replace(/\.(png|jpe?g)$/i, '')
  const idx = base.lastIndexOf('#')
  if (idx < 0) return null
  const parts = base.slice(idx + 1).split('_')
  if (parts.length !== 4) return null
  const nums = parts.map(s => /^\d{1,3}$/.test(s) ? Number(s) / 1000 : NaN)
  if (!nums.every(n => Number.isFinite(n) && n >= 0 && n <= 1) || !(nums[2] > nums[0]) || !(nums[3] > nums[1])) return null
  return nums
}

/** 从模板名解析半区代码后缀（#a/#u/#d/#l/#r/#ul/#ur/#dl/#dr），如 task_item#l.png → 'l'；无 → null */
function parseTplRegionCode(name) {
  const base = name.replace(/\.(png|jpe?g)$/i, '')
  const idx = base.lastIndexOf('#')
  if (idx < 0) return null
  const code = base.slice(idx + 1).toLowerCase()
  return ['a', 'u', 'd', 'l', 'r', 'ul', 'ur', 'dl', 'dr'].includes(code) ? code : null
}

/** 模板短名：去掉 #区域后缀（login#0_0_500_500.png → login.png），无后缀原样返回。
 *  脚本里写短名即可，引擎自动解析到唯一匹配的带后缀文件（区域照常生效） */
function tplShortName(name) {
  return name.replace(/#[^#./\\]+(\.(png|jpe?g))$/i, '$1')
}

/** 模板名区域徽标文本：半区码直接显示码字（l/r/dr…），数字坐标显示 ◧（悬停看全名） */
function tplRegionBadge(name) {
  const base = name.replace(/\.(png|jpe?g)$/i, '')
  const idx = base.lastIndexOf('#')
  if (idx < 0) return ''
  const s = base.slice(idx + 1).toLowerCase()
  if (['a', 'u', 'd', 'l', 'r', 'ul', 'ur', 'dl', 'dr'].includes(s)) return s
  if (/^\d{1,3}(_\d{1,3}){3}$/.test(s)) return '◧'
  return ''
}

/** 模板名半区代码 → 设备像素搜索区域 [x, y, w, h] */
function regionCodePixels(code, vw, vh) {
  const hw = Math.round(vw / 2)
  const hh = Math.round(vh / 2)
  const map = {
    a: null,
    u: [0, 0, vw, hh],
    d: [0, vh - hh, vw, hh],
    l: [0, 0, hw, vh],
    r: [vw - hw, 0, hw, vh],
    ul: [0, 0, hw, hh],
    ur: [vw - hw, 0, hw, hh],
    dl: [0, vh - hh, hw, hh],
    dr: [vw - hw, vh - hh, hw, hh]
  }
  return map[code] ?? null
}

/** 测试匹配的搜索区域：下拉框手动选择优先，否则按模板名自动识别
 *  （#x1_y1_x2_y2 → 对应矩形区域；#l/#r/... → 对应半区；无 → 全屏） */
function templateRegionPixels(name) {
  // 实际视频尺寸优先：虚拟屏分辨率/方向会被游戏改变，设备配置里的 width/height 可能过期
  const vw = videoElement.value?.videoWidth || current.value?.width || 1920
  const vh = videoElement.value?.videoHeight || current.value?.height || 1080
  if (testRegion.value) return regionCodePixels(testRegion.value, vw, vh)
  const nums = parseTplRegion(name)
  if (nums) {
    const x = Math.round(nums[0] * vw)
    const y = Math.round(nums[1] * vh)
    const w = Math.round((nums[2] - nums[0]) * vw)
    const h = Math.round((nums[3] - nums[1]) * vh)
    return [x, y, w, h]
  }
  const code = parseTplRegionCode(name)
  if (code) return regionCodePixels(code, vw, vh)
  return null
}

/** 把生成的 YAML 片段以对应缩进插入到脚本的光标下一行：
 *  - 光标在 steps 列表内 → 2 空格缩进插到光标所在行的下一行
 *  - 光标在 func 函数体内 → 以函数体缩进（默认 defIndent+2；cond+steps 写法取
 *    steps 列表项缩进）插到光标所在行的下一行；光标在「- 函数名:」行 / func: 行 →
 *    插到该函数体末尾（无函数定义时回到 steps 逻辑）
 *  - 省略段落键的简写脚本同样支持：顶层 steps 序列按序列项缩进追加；省略 func:
 *    的顶层函数定义按定义缩进 0 定位函数体（兜底追加到最后一个函数体末尾）
 *  - 其余（steps 之外或无光标）→ 追加到 steps 列表末尾；没有 steps 时补一个
 *    最小可运行脚本结构。插入后光标移到新记录之后，方便连续追加 */
function appendYamlToScript(snippet) {
  const lines = editScriptCode.value.split('\n')
  const stepsIdx = lines.findIndex(l => /^steps\s*:/.test(l))
  const funcIdx = lines.findIndex(l => /^func\s*:/.test(l))
  // 省略段落键的简写（与引擎 normalize_top 判定一致）：无 steps:/func: 根键时
  // 顶层序列 = steps、顶层映射 = func（函数定义在缩进 0；config 不能省略）
  const firstContent = lines.find(l => l.trim() && !/^\s*#/.test(l)) || ''
  const impliedSteps = stepsIdx === -1 && funcIdx === -1 && /^\s*-\s/.test(firstContent)
  const impliedFunc = stepsIdx === -1 && funcIdx === -1 && !impliedSteps && !!firstContent
  const ta = scriptEditor.value
  let cursorLine = -1
  if (ta && typeof ta.selectionStart === 'number') {
    cursorLine = editScriptCode.value.slice(0, ta.selectionStart).split('\n').length - 1
  }
  const indOf = l => (l.match(/^(\s*)/) || ['', ''])[1].length

  let insertIdx = -1
  let indent = '  '
  // —— 光标在 func 段内（显式 func: 或省略简写的顶层映射）：插入到光标所属函数体 ——
  if ((funcIdx !== -1 || impliedFunc) && cursorLine >= funcIdx) {
    const defIndent = impliedFunc ? 0 : 2 // 函数定义行缩进（简写 = 顶层）
    // 函数定义行（「名称:」或「- 名称:」，值留空；cond/steps 挂在下一层）
    const isDef = l => {
      const t = l.trim()
      return indOf(l) === defIndent && /^(- )?[\w.-]+\s*:\s*(#.*)?$/.test(t)
    }
    // 函数体末尾 = 下一个缩进 ≤defIndent 的非空行（函数定义 / 根级键）之前；
    // 函数体缩进 = 第一个「- 」项行（cond+steps 写法里 steps 列表项的缩进），
    // 无项行时若紧跟 steps: 键则取其 +2，默认 defIndent+2
    const insertIntoFuncBody = defIdx => {
      let bodyEnd = defIdx
      for (let i = defIdx + 1; i < lines.length; i++) {
        const l = lines[i]
        if (l.trim() && indOf(l) <= defIndent) break
        if (l.trim()) bodyEnd = i
      }
      let bi = defIndent + 2
      for (let i = defIdx + 1; i <= bodyEnd; i++) {
        const l = lines[i]
        if (!l.trim()) continue
        const ind = indOf(l)
        if (ind <= defIndent) break
        if (/^\s*-\s/.test(l)) { bi = ind; break }
        if (/^steps\s*:\s*$/.test(l.trim())) bi = ind + 2
      }
      indent = ' '.repeat(bi)
      // 光标在函数体内（含函数体行/嵌套行）→ 光标下一行；否则函数体末尾
      insertIdx = cursorLine > defIdx && cursorLine <= bodyEnd ? cursorLine + 1 : bodyEnd + 1
    }
    let inFunc = false
    for (let i = cursorLine; i >= 0; i--) {
      if (lines[i].trim() && indOf(lines[i]) === 0) {
        // 简写：顶层函数定义行即段起点；显式：func: 行
        inFunc = impliedFunc ? isDef(lines[i]) : /^func\s*:/.test(lines[i])
        break
      }
    }
    if (inFunc) {
      // 光标所属函数；光标在 func: 行 / 首个函数之前 → 第一个函数
      let defIdx = -1
      for (let i = cursorLine; i > funcIdx; i--) {
        if (isDef(lines[i])) { defIdx = i; break }
      }
      if (defIdx === -1) {
        for (let i = impliedFunc ? 0 : funcIdx + 1; i < lines.length; i++) {
          if (!impliedFunc && lines[i].trim() && indOf(lines[i]) === 0) break
          if (isDef(lines[i])) { defIdx = i; break }
        }
      }
      if (defIdx !== -1) insertIntoFuncBody(defIdx)
    } else if (impliedFunc) {
      // 简写库兜底（光标在函数外/注释区）：追加到最后一个函数体末尾
      let lastDef = -1
      for (let i = 0; i < lines.length; i++) if (isDef(lines[i])) lastDef = i
      if (lastDef !== -1) insertIntoFuncBody(lastDef)
    }
  }
  // —— 省略 steps: 的顶层序列：按序列项缩进追加（光标行后优先，否则列表末尾）——
  if (impliedSteps && insertIdx === -1) {
    const firstDash = lines.findIndex(l => /^\s*-\s/.test(l))
    indent = ' '.repeat(indOf(lines[firstDash]))
    insertIdx = lines.length
    if (cursorLine >= firstDash) {
      const cur = lines[cursorLine] || ''
      if (!cur.trim() || /^\s/.test(cur) || /^\s*#/.test(cur)) insertIdx = cursorLine + 1
    }
  }
  // —— 没有 steps 时补一个最小可运行脚本结构 ——
  if (stepsIdx === -1 && insertIdx === -1) {
    const indented = snippet.split('\n').map(l => (l ? '  ' + l : l)).join('\n')
    const base = editScriptCode.value.trim()
    const block = `steps:\n${indented}`
    editScriptCode.value = base ? base + '\n\n' + block : block
    return
  }
  // —— steps 列表（或兜底）：光标在 steps 内部 → 光标下一行，否则 steps 末尾 ——
  if (insertIdx === -1) {
    insertIdx = lines.length
    for (let i = stepsIdx + 1; i < lines.length; i++) {
      const line = lines[i]
      if (line.trim() && !/^\s/.test(line)) {
        insertIdx = i
        break
      }
    }
    if (cursorLine > stepsIdx) {
      const cur = lines[cursorLine] || ''
      if (!cur.trim() || /^\s/.test(cur) || /^\s*#/.test(cur)) {
        insertIdx = cursorLine + 1
      }
    }
  }
  const indented = snippet.split('\n').map(l => (l ? indent + l : l)).join('\n')
  const before = lines.slice(0, insertIdx)
  const after = lines.slice(insertIdx)
  while (before.length && before[before.length - 1].trim() === '') before.pop()
  while (after.length && after[0].trim() === '') after.shift()
  const text = before.join('\n') + (before.length ? '\n' : '') + indented + (after.length ? '\n' + after.join('\n') : '')
  editScriptCode.value = text
  // 光标移到插入的记录之后，连续追加时依次往下排
  nextTick(() => {
    const ta2 = scriptEditor.value
    if (!ta2) return
    const pos = (before.join('\n').length + (before.length ? 1 : 0)) + indented.length
    ta2.selectionStart = ta2.selectionEnd = pos
  })
}

/** 点击操作记录行：把对应的 YAML 追加到编辑区 */
function applyOpRecord(r) {
  if (scriptMode.value !== 'edit') return
  appendYamlToScript(r.yaml)
  toast('已追加：' + r.text, 'success')
}

/** 编辑区 Tab 键：插入 2 个空格（代替切换焦点）；Shift+Tab 反向缩进（每行行首退 1~2 个空格） */
function onEditorTab(e) {
  const ta = e.target
  const start = ta.selectionStart
  const end = ta.selectionEnd
  const v = editScriptCode.value
  if (start === end) {
    const lineStart = v.lastIndexOf('\n', start - 1) + 1
    const before = v.slice(lineStart, start)
    if (e.shiftKey) {
      // Shift+Tab：当前行行首退 1~2 个空格（光标在行内任意位置均可）
      const m = before.match(/^ {1,2}/)
      if (m) {
        editScriptCode.value = v.slice(0, lineStart) + v.slice(lineStart + m[0].length)
        nextTick(() => { ta.selectionStart = ta.selectionEnd = start - m[0].length })
      }
      return
    }
    editScriptCode.value = v.slice(0, start) + '  ' + v.slice(end)
    nextTick(() => { ta.selectionStart = ta.selectionEnd = start + 2 })
    return
  }
  const sel = v.slice(start, end)
  if (sel.includes('\n')) {
    const lineStart = v.lastIndexOf('\n', start - 1) + 1
    if (e.shiftKey) {
      // Shift+Tab 多行选中：各行行首退 1~2 个空格
      const lines = v.slice(lineStart, end).split('\n')
      const removed = lines.map(l => (l.match(/^ {1,2}/) || ['', ''])[0].length)
      const dedented = lines.map((l, i) => l.slice(removed[i])).join('\n')
      editScriptCode.value = v.slice(0, lineStart) + dedented + v.slice(end)
      const shrink = removed.reduce((a, b) => a + b, 0)
      const newEnd = Math.max(lineStart, end - shrink)
      const newStart = Math.min(start - Math.min(removed[0], start - lineStart), newEnd)
      nextTick(() => { ta.selectionStart = newStart; ta.selectionEnd = newEnd })
      return
    }
    // 多行选中：每行前插 2 空格
    const indented = v.slice(lineStart, end).split('\n').map(l => '  ' + l).join('\n')
    editScriptCode.value = v.slice(0, lineStart) + indented + v.slice(end)
    const newEnd = lineStart + indented.length
    nextTick(() => { ta.selectionStart = lineStart; ta.selectionEnd = newEnd })
  } else {
    // 单点插入：光标处插 2 空格
    editScriptCode.value = v.slice(0, start) + '  ' + v.slice(end)
    nextTick(() => { ta.selectionStart = ta.selectionEnd = start + 2 })
  }
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

/** 投屏点击（alt 模式）→ 生成 tap 记录（color 取色记录改在二次裁切区内生成，见 cropPickColor） */
function setTapRecord(p) {
  const vw = videoElement.value?.videoWidth || 1920
  const vh = videoElement.value?.videoHeight || 1080
  const rx = (p.x / vw).toFixed(4)
  const ry = (p.y / vh).toFixed(4)
  opRecords.value = [
    { id: ++opRecordSeq, text: `- tap [${rx}, ${ry}]`, yaml: renderOpTpl(opTpls.tap, { x: rx, y: ry }) }
  ]
  showAltFeedback('tap', p.x, p.y)
}

/** 投屏滑动 → 生成 swipe 记录（time 用实际滑动时长，模板自带 ms 单位） */
function setSwipeRecords(from, to, durationMs) {
  const vw = videoElement.value?.videoWidth || 1920
  const vh = videoElement.value?.videoHeight || 1080
  const fx = (from.x / vw).toFixed(4)
  const fy = (from.y / vh).toFixed(4)
  const tx = (to.x / vw).toFixed(4)
  const ty = (to.y / vh).toFixed(4)
  const dur = Math.max(1, Math.round(durationMs || 1000))
  opRecords.value = [
    {
      id: ++opRecordSeq,
      text: `- swipe [${fx}, ${fy}] -> [${tx}, ${ty}] ${dur}ms`,
      yaml: renderOpTpl(opTpls.swipe, { fx, fy, tx, ty, time: String(dur) })
    }
  ]
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
  if (!activePkg.value) return toast('请先选择应用分区（设备页签配置应用包名）', 'warn')
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

// ---------- 更多菜单（新建 / 删除）；导入/导出在脚本页签顶部应用下拉旁 ----------
const moreOpen = ref(false)

/** 导出当前应用分区快照（yaml/ + tmpl/ 全量）→ zip 下载 */
async function exportPartition() {
  if (!activePkg.value) return toast('请先选择应用分区', 'warn')
  try {
    const { blob, filename } = await api.exportPartition(activePkg.value)
    const name = filename || `${activePkg.value}.zip`
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = name
    a.click()
    setTimeout(() => URL.revokeObjectURL(a.href), 5000)
    toast(`已导出 ${name}`, 'success')
  } catch (e) {
    toast('导出失败：' + e.message, 'error')
  }
}

/** 导入分区快照 zip（模板+脚本）到当前应用分区：先探测冲突，同名文件替换前二次确认 */
async function onImportFile(e) {
  const file = e.target.files?.[0]
  e.target.value = ''
  if (!file) return
  if (!activePkg.value) return toast('请先选择应用分区', 'warn')
  try {
    const dry = await api.importScripts(file, false, activePkg.value)
    if (dry.conflicts?.length) {
      const list = dry.conflicts.join('\n')
      if (!confirm(`导入到 ${activePkg.value} 会替换以下 ${dry.conflicts.length} 个同名文件：\n\n${list}\n\n确认替换导入？`)) return
    }
    const rep = await api.importScripts(file, true, activePkg.value)
    await loadData()
    toast(`导入完成：新增 ${rep.imported.length} 个，替换 ${rep.replaced.length} 个`, 'success')
  } catch (err) {
    toast('导入失败：' + err.message, 'error')
  }
}

/** 脚本校验（实现已抽离至 src/script-language/validate.js）：绑定当前分区/模板/脚本数据源 */
const validateScriptCode = createScriptValidator({ templatesData, scriptsData, activePkg })

/** 保存新建/编辑脚本：先校验再保存（落盘到当前应用分区），名称自动补 .yml */
async function saveEditScript() {
  const rawName = editScriptName.value.trim()
  if (!rawName) return toast('请填写脚本名称', 'error')
  if (!activePkg.value) return toast('请先选择应用分区', 'warn')
  const name = /\.(ya?ml)$/i.test(rawName) ? rawName : rawName + '.yml'
  const errors = validateScriptCode(editScriptCode.value)
  if (errors.length) return toast('校验未通过：' + errors.slice(0, 3).join('；'), 'error')
  scriptSaving.value = true
  try {
    const r = await api.saveScript({ id: editScriptId.value, name, content: editScriptCode.value, pkg: activePkg.value })
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

// 运行状态轮询：服务端异步执行脚本（run 接口立即返回），
// 轮询 status 直到脚本真正结束，才复位运行状态（按钮/顶栏芯片随之恢复）
let runStatusTimer = null

function startRunStatusPoll() {
  if (runStatusTimer) clearInterval(runStatusTimer)
  checkRunStatus()
  runStatusTimer = setInterval(checkRunStatus, 1000)
}

function stopRunStatusPoll() {
  if (runStatusTimer) { clearInterval(runStatusTimer); runStatusTimer = null }
}

async function checkRunStatus() {
  if (!store.running || !store.runScriptId) { stopRunStatusPoll(); return }
  try {
    const st = await api.scriptStatus(store.runScriptId)
    if (!st.running) {
      store.running = false
      store.runScriptId = null
      stopRunStatusPoll()
      toast('脚本已结束', 'info')
    }
  } catch (e) {}
}

// ---------- 运行模式：只读脚本内容 + 逻辑行选中 ----------
// 未运行/非编辑时展示脚本内容（不可编辑）。可点击选中的「逻辑行」：
// steps: 段内与首项同缩进的 "- " 行（从该步骤起跑顶层）+ func: 段内每个
// 函数名行（从头运行整函数：引擎先判 cond 再跑函数体）与函数体中与函数体
// 首项同缩进的 "- " 行（直接运行该函数体，从该步骤起）。
// then/else/loop 子步骤、config 段不可选；索引按所在段落各自计数，
// 不再受 func/config 段条目数量的偏移影响。
// 省略段落键的简写脚本（顶层序列/顶层映射直接写内容）同样支持行选中。
const selectedLine = ref(null)

const scriptContent = computed(() => {
  const s = scripts.value.find(x => x.id === selScript.value)
  return s ? s.content : ''
})
const scriptLines = computed(() => scriptContent.value.split('\n'))

/** call / 跨文件函数调用行解析（与 scriptLines 平行）：
 *  `- call: test.yaml` → { prefix, name, suffix }；`- test1:fun1: 实参…` →
 *  { prefix, name: 脚本名, label: 完整键, suffix }，其余行 → null */
const callLinks = computed(() => scriptLines.value.map(line => {
  // call 传参后行内还有实参（- call: 通用日常.yml act_136.png）：分隔空格划入
  // suffix（m[3] 以 \s+ 开头），否则渲染时脚本名和实参贴在一起
  const m = line.match(/^(\s*(?:-\s+)?call:\s*)(\S+)((?:\s+.*)?)$/)
  if (m) {
    const name = m[2].replace(/^["']|["']$/g, '')
    return name ? { prefix: m[1], name, suffix: m[3] } : null
  }
  // 跨文件函数调用 - 脚本名:函数名: 实参…：链接预览子脚本内容
  const x = line.match(/^(\s*(?:-\s+)?)(\S+:[^\s:]+)((?:\s+.*)?)$/)
  if (x) {
    const script = x[2].split(':')[0]
    return script ? { prefix: x[1], name: script, label: x[2], suffix: x[3] } : null
  }
  return null
}))

// call 子脚本预览弹窗（点脚本名打开；ESC / ✕ / 点遮罩关闭）
const previewScript = ref(null)

function openCallPreview(name) {
  // 与引擎 resolve_call 一致：缺 .yaml/.yml 扩展名自动补全
  const find = n => scripts.value.find(x => x.name === n)
  let s = find(name)
  if (!s && !/\.(ya?ml)$/i.test(name)) {
    for (const ext of ['.yaml', '.yml']) {
      s = find(name + ext)
      if (s) break
    }
  }
  if (!s) return toast(`子脚本不存在：${name}`, 'warn')
  previewScript.value = s
}

function closeCallPreview() {
  previewScript.value = null
}

const runLineMap = computed(() => computeRunLineMap(scriptLines.value))

/** 点击行：可选逻辑行选中；再次点击已选中行取消（从头运行）；
 *  点击 then/else 等非逻辑行不改变当前选中 */
function onScriptLineClick(idx) {
  if (!runLineMap.value[idx]) return
  selectedLine.value = selectedLine.value === idx ? null : idx
}

// 切换脚本时清除行选中
watch(() => selScript.value, () => { selectedLine.value = null })

function runScript() {
  if (!selScript.value) return
  const s = scripts.value.find(x => x.id === selScript.value)
  if (!s) return
  // 选中行 → 运行目标：顶层 steps 序号，或函数体（func + 体内序号）
  const target = selectedLine.value != null ? runLineMap.value[selectedLine.value] : null
  const startIndex = target ? target.index : 0
  const funcName = target?.func || null
  // 每次运行清空日志区域，只显示本次运行产生的日志
  runStartTime = Date.now()
  rawLogs = []
  liveLogs.value = []
  store.running = true
  store.runScript = funcName ? `${s.name} · ${funcName}()` : s.name
  store.runScriptId = s.id
  api.runScript(s.id, store.deviceId, startIndex, funcName).then(() => {
    toast('脚本已开始运行', 'success')
    // POST 成功（服务端已登记 run_stops 条目）后才开始轮询，
    // 避免设备离线时 connect_device 耗时较长、status 先于 run 返回导致状态被提前复位
    startRunStatusPoll()
  }).catch(e => {
    store.running = false
    store.runScriptId = null
    pushLog('error', `执行失败：${e.message}`)
    toast('脚本执行失败', 'error')
  })
}

function stopScript() {
  const id = store.runScriptId || selScript.value
  if (!id) return
  api.stopScript(id).catch(() => {})
  store.running = false
  store.runScriptId = null
  stopRunStatusPoll()
  pushLog('warn', '已发送停止指令，脚本将在当前步骤结束后停止')
  toast('已发送停止指令', 'warn')
}

async function testMatch(name) {
  if (!connected.value) return toast('请先连接设备', 'error')
  if (hitTimer) { clearTimeout(hitTimer); hitTimer = null }
  showHit.value = false
  try {
    const region = templateRegionPixels(name)
    const r = await api.testTemplate(name, store.deviceId, Number(testThreshold.value) || 0.8, region, activePkg.value)
    if (r.hit) {
      hit.x = r.x; hit.y = r.y; hit.w = r.width; hit.h = r.height
      hitLabel.value = `${name} ${r.score.toFixed(2)}`
      hitMiss.value = false
      showHit.value = true
      // 匹配框只展示 3 秒，避免一直留在画面上
      hitTimer = setTimeout(() => { showHit.value = false }, 3000)
      toast(`匹配成功：${name} 置信度 ${r.score.toFixed(2)}`, 'success')
    } else {
      // 未命中也画框：显示本次搜索区域（与引擎 miss 可视化同语义，便于发现区域配错）
      const vw2 = videoElement.value?.videoWidth || current.value?.width || 1920
      const vh2 = videoElement.value?.videoHeight || current.value?.height || 1080
      const [rx, ry, rw2, rh2] = region || [0, 0, vw2, vh2]
      hit.x = rx; hit.y = ry; hit.w = rw2; hit.h = rh2
      hitLabel.value = `${name} 未命中`
      hitMiss.value = true
      showHit.value = true
      hitTimer = setTimeout(() => { showHit.value = false }, 3000)
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
  // SPA 内跳转（store 存活）→ 自动重连恢复画面；页面刷新 → localStorage 恢复设备选择；
  // 首次进入仅选中第一台设备，等待用户点连接（不主动建会话，尊重空闲低功耗）
  const spaPreselected = !!store.deviceId
  await loadData()
  if (!store.deviceId) {
    const saved = localStorage.getItem('gb_device_id')
    store.deviceId = (saved && devices.value.find(d => d.id === saved)) ? saved : (devices.value[0]?.id || null)
  }
  const d = current.value
  if (d) loadForm(d)
  else { mode.value = 'edit'; store.deviceId = null }
  window.addEventListener('keydown', onGlobalKeydown)

  // 刷新恢复运行态：刷新前页面发起的脚本在服务端继续执行——按设备查询运行中的
  // 脚本，恢复运行状态/选中脚本/状态轮询与日志（不依赖投屏连接是否恢复成功）
  if (store.deviceId) {
    try {
      const run = await api.deviceRun(store.deviceId)
      if (run && run.running && run.script_id && !store.running) {
        store.running = true
        store.runScriptId = run.script_id
        store.runScript = run.script_name || run.script_id
        selScript.value = run.script_id
        scriptMode.value = 'run'
        runStartTime = 0   // 不按开始时间过滤，恢复最近日志
        startLogPolling()
        startRunStatusPoll()
        toast(`检测到 ${store.runScript} 正在运行，已恢复状态`, 'info')
      }
    } catch (e) { /* 恢复失败不影响进入页面 */ }
  }
  // 画面恢复：SPA 内返回（store 存活）或刷新后脚本运行中/设备会话在线（此前正在
  // 投屏）→ 自动连接；设备空闲离线则保持首次进入行为；遇 conflict 不抢（connect 内处理）
  if (store.deviceId && (spaPreselected || store.running || current.value?.status === 'online')) connect(false)
  // 其他页面已启动脚本时，本页接管状态轮询（脚本结束后复位运行状态）
  if (store.running && store.runScriptId) startRunStatusPoll()
})

// 设备选择持久化：刷新后自动恢复选中设备（运行态/画面恢复的前提）
watch(() => store.deviceId, id => {
  if (id) localStorage.setItem('gb_device_id', id)
})

onUnmounted(() => {
  window.removeEventListener('keydown', onGlobalKeydown)
  if (reconnectTimer) { clearTimeout(reconnectTimer); reconnectTimer = null }
  if (saveTimer) { clearTimeout(saveTimer); saveTimer = null }
  if (savedTimer) { clearTimeout(savedTimer); savedTimer = null }
  if (hitTimer) { clearTimeout(hitTimer); hitTimer = null }
  if (altFeedbackTimer) { clearTimeout(altFeedbackTimer); altFeedbackTimer = null }
  if (fxTapTimer) { clearTimeout(fxTapTimer); fxTapTimer = null }
  if (fxSwipeTimer) { clearTimeout(fxSwipeTimer); fxSwipeTimer = null }
  if (fxHitTimer) { clearTimeout(fxHitTimer); fxHitTimer = null }
  stopRunStatusPoll()
  cleanup(true)
})
</script>

<style scoped>
.console {
  display: flex; height: 100%; padding: 14px; gap: 14px;
  /* 侧边栏收起时释放的宽度（展开 200px - 收起 52px，见 MainLayout.vue） */
  --sb-free-w: 148px;
}

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
/* 未命中的搜索区域框：虚线红、无光晕（区别于命中实线绿），大区域时半透明填充提示范围 */
.hit-miss { border-style: dashed; border-color: var(--danger); box-shadow: none; background: rgba(239,68,68,.06); }
.hit-miss .hit-label { background: var(--danger); color: #fff; }

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
.crop-stage {
  display: flex; overflow: auto;
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  background: #000;
}
.crop-stage .crop-canvas { margin: auto; }
.crop-canvas {
  border-radius: var(--radius-sm);
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
  overflow: hidden; transition: width .18s ease;
}
/* 侧边栏收起：释放宽度全部给右侧操作区（340 + 148），中间投屏区宽度保持不变 */
.console.sb-collapsed .panel { width: calc(340px + var(--sb-free-w)); }
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
.auto-run .spicker { flex: 1 1 auto; }
.auto-run .select { flex: 1; min-width: 120px; }
.run-actions { display: flex; gap: 8px; }
.run-actions .btn { flex: 1; }
.run-actions .more-wrap { position: relative; flex: 1; }
.run-actions .more-wrap .btn { width: 100%; }
.more-mask { position: fixed; inset: 0; z-index: 20; }
.more-dropdown {
  position: absolute; right: 0; top: calc(100% + 4px); z-index: 30;
  display: flex; flex-direction: column; min-width: 120px; padding: 4px; gap: 2px;
  background: var(--bg-2); border: 1px solid var(--border); border-radius: var(--radius-sm);
  box-shadow: 0 8px 24px rgba(0, 0, 0, .4);
}
.more-item {
  display: flex; align-items: center; gap: 6px; text-align: left; white-space: nowrap;
  padding: 6px 10px; border: none; background: none; border-radius: var(--radius-sm);
  color: var(--text-0); font-size: 12px; cursor: pointer;
}
.more-item:hover { background: var(--bg-3); }
.more-item:disabled { color: var(--text-2); opacity: .5; cursor: not-allowed; }
.more-item.danger:hover { color: var(--danger); }

/* 脚本页签 */
.panel-sec.script-tab { flex: 1; min-height: 0; overflow: hidden; }
/* 应用分区下拉：模板/脚本数据随分区切换（默认跟随设备页签的应用包名） */
.pkg-bar { flex: none; display: flex; align-items: center; gap: 8px; padding-bottom: 8px; border-bottom: 1px solid var(--border); }
.pkg-bar .pkg-label { flex: none; font-size: 12px; color: var(--text-2); }
.pkg-bar .pkg-select { flex: 1; min-width: 0; }
.pkg-bar .btn { flex: none; }
.pkg-empty { flex: none; padding: 24px 10px; text-align: center; font-size: 12px; color: var(--text-2); }
.script-tpl { flex: 4; min-height: 0; display: flex; flex-direction: column; gap: 8px; border-bottom: 1px solid var(--border); padding-bottom: 10px; }
.tpl-top { display: flex; align-items: center; gap: 8px; }
/* 阈值输入 : 区域下拉 : 搜索框 : 框选按钮 : 上传按钮 = 2:4:5:3:3 */
.tpl-top .input { flex: 2 1 0%; min-width: 0; }
.tpl-top .tpl-region { flex: 4 1 0%; min-width: 0; padding: 4px 6px; font-size: 11px; }
.tpl-top .tpl-search { flex: 5 1 0%; min-width: 0; font-size: 11px; }
.tpl-top .btn { flex: 3 1 0%; min-width: 0; }
.tpl-tools { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.script-run { flex: 6; display: flex; flex-direction: column; gap: 10px; min-height: 0; }
.script-logs { flex: 1; min-height: 120px; max-height: none; }
.run-hint { font-size: 11px; color: var(--text-2); flex-shrink: 0; }
.script-view-wrap { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 6px; }
.script-view {
  flex: 1; min-height: 0; overflow: auto; background: var(--bg-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 10px 12px; font-size: 12px; line-height: 1.65; color: #c9d4e8;
  user-select: none;
}
.sv-line { white-space: pre; border-radius: 4px; padding: 0 6px; margin: 0 -6px; }
.sv-line.selectable { cursor: pointer; }
.sv-line.selectable:hover { background: var(--bg-3); }
.sv-line.sel {
  background: rgba(34,211,165,.12); color: var(--accent);
  box-shadow: inset 2px 0 0 var(--accent);
}
/* call 子脚本名链接：悬停下划线，点击弹窗预览（脚本视图 user-select:none，需单独放开） */
.call-link { color: var(--accent-2); cursor: pointer; }
.call-link:hover { text-decoration: underline; }

/* call 子脚本预览弹窗（modal-mask/.modal/.modal-head/.modal-body 为全局样式） */
.preview-modal { min-width: 520px; width: 520px; }
.preview-code {
  background: var(--bg-0); border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 10px 12px; font-size: 12px; line-height: 1.65; color: #c9d4e8;
  overflow: auto; max-height: 60vh; white-space: pre; margin: 0;
}
.script-view-empty {
  flex: 1; display: flex; align-items: center; justify-content: center;
  color: var(--text-2); font-size: 12px; background: var(--bg-0);
  border: 1px dashed var(--border); border-radius: var(--radius-sm);
}
.script-edit { flex: 6; display: flex; flex-direction: column; gap: 10px; min-height: 0; }
.edit-name-row { display: flex; }
.edit-name-row .input { flex: 1; min-width: 0; width: 100%; }
.edit-actions { display: flex; gap: 8px; }
.edit-actions .btn { flex: 1; justify-content: center; }
.edit-actions .btn.active { border-color: var(--accent-2); color: var(--accent-2); background: rgba(56,189,248,.08); }
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
.tpl-row.renaming { background: rgba(56,189,248,.08); border-color: rgba(56,189,248,.35); }
.tpl-empty { padding: 16px 8px; text-align: center; font-size: 11px; color: var(--text-2); }
.tpl-cell.thumb { width: 40px; flex-shrink: 0; display: flex; align-items: center; }
.tpl-list-head .tpl-cell.thumb { white-space: nowrap; }
.tpl-cell.name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; color: var(--text-0); }
.tpl-cell.ops { display: flex; gap: 6px; flex-shrink: 0; }
.tpl-cell.ops .btn { padding: 2px 8px; font-size: 11px; }
.rename-input { width: 100%; min-width: 0; padding: 2px 6px; font-size: 12px; }
.tpl-region-badge {
  display: inline-block; margin-left: 6px; padding: 0 5px; border-radius: 4px;
  background: var(--bg-3); border: 1px solid var(--border);
  color: var(--accent); font-size: 10px; line-height: 16px; vertical-align: 1px;
  cursor: help; user-select: none;
}
.tpl-thumb { display: inline-flex; }
.tpl-thumb img {
  width: 24px; height: 24px; object-fit: contain;
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
.tpl-view-img { position: relative; align-self: center; }
.tpl-view-img img {
  display: block; max-width: 92vw; max-height: 82vh; object-fit: contain;
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
.crop-panel-full .crop-stage { flex: 1; min-height: 0; min-width: 0; }
</style>
