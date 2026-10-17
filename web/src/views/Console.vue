<template>
  <div ref="consoleEl" class="console" :class="{ 'is-panel-resizing': panelResizing, 'global-page': isGlobalPage, 'panel-expanded': paneMode === 'expanded', 'panel-collapsed': paneMode === 'collapsed' }">
      <!-- 常用上下文一行展示，应用与投屏工具按需展开。 -->
      <div ref="toolbarEl" class="toolbar" data-keyboard-ignore="true" @click="onToolbarClick">
        <div class="tb-row tb-context-row">
          <div v-if="!isGlobalPage" class="tb-group tb-device-group" role="group" aria-label="设备连接">
            <span class="tb-label">游戏目标</span>
            <select :value="store.deviceId" class="select mono tb-dev-select" :disabled="forceReconnecting" aria-label="游戏目标列表" @change="addAndSelectTarget($event.target.value)">
              <option :value="null">选择目标…</option>
              <option v-for="d in devices" :key="d.id" :value="d.id">{{ d.name }} · {{ d.status === 'online' ? '在线' : '离线' }}</option>
            </select>
            <button v-if="!connected" class="btn btn-sm btn-icon" title="连接画面" aria-label="连接画面" :disabled="!store.deviceId || connecting || forceReconnecting" @click="flushAndConnect"><UiIcon name="connect" /></button>
            <button v-else class="btn btn-sm btn-icon" title="关闭预览，自动化继续运行" aria-label="关闭预览" @click="closeSelectedPreview"><UiIcon name="disconnect" /></button>
            <button class="btn btn-sm btn-icon" :disabled="scanning || forceReconnecting" @click="refreshDevices" title="刷新设备" aria-label="刷新设备"><UiIcon name="refresh" /></button>
            <div class="tb-more-wrap">
              <button
                class="btn btn-sm btn-icon"
                :class="{ active: toolbarMenuOpen === 'device' }"
                aria-haspopup="menu"
                :aria-expanded="toolbarMenuOpen === 'device'"
                aria-label="设备更多操作"
                title="新增 / 设置 / 安装应用 / 强制重连 / 删除设备"
                @click.stop="toggleToolbarMenu('device', $event)"
              ><UiIcon name="more" /></button>
            </div>
            <label class="tb-display-mode">
              <span class="tb-label">画面</span>
              <select class="select" aria-label="画面模式" :value="workspace.state.gridSize" @change="changeGridSize(Number($event.target.value), $event.target)">
                <option :value="1">单屏</option>
                <option :value="4">4 格</option>
                <option :value="9">9 格</option>
              </select>
            </label>
            <div v-if="!isSingleScreen || canStartTarget(store.deviceId) || workspace.getRun(store.deviceId)" class="device-run-actions" role="group" :aria-label="isSingleScreen ? '当前目标脚本' : '当前网格脚本'">
              <template v-if="isSingleScreen">
                <button v-if="!workspace.getRun(store.deviceId)" class="btn btn-sm btn-primary" :disabled="!canStartTarget(store.deviceId) || !!workspace.state.pending[store.deviceId]" @click="startOneTarget(store.deviceId)" title="运行当前目标保存的脚本与参数"><UiIcon name="play" />运行脚本</button>
                <button v-else class="btn btn-sm btn-danger" :disabled="batchBusy" @click="stopOneTarget(store.deviceId)"><UiIcon name="stop" />停止脚本</button>
              </template>
              <template v-else>
                <button class="btn btn-sm btn-primary" :disabled="batchBusy || !visibleTargets.some(canStartTarget)" @click="startVisibleTargets" :title="'运行当前可见的 ' + visibleTargets.length + ' 个目标各自保存的脚本；未配置目标会跳过'"><UiIcon name="play" />批量运行</button>
                <button v-if="visibleRunningCount" class="btn btn-sm btn-danger" :disabled="batchBusy" @click="stopVisibleTargets" :title="'停止当前可见的 ' + visibleRunningCount + ' 个运行'"><UiIcon name="stop" />批量停止</button>
              </template>
            </div>
            <span v-if="hiddenCount" class="tb-hidden-targets" role="status" :title="hiddenCount + ' 个目标在后台，其中 ' + hiddenRunningCount + ' 个脚本运行中'">后台 {{ hiddenCount }}<span v-if="hiddenRunningCount"> · {{ hiddenRunningCount }} 运行</span></span>
          </div>
          <div class="stage-package">
            <PackageContextBar :context="packageContext" compact />
          </div>
          <div v-if="!isGlobalPage" class="toolbar-actions">
            <button class="btn btn-sm" :aria-expanded="controlToolsOpen" @click="controlToolsOpen = !controlToolsOpen"><UiIcon name="settings" />游戏操作</button>
            <button class="btn btn-sm" title="投屏全屏" aria-label="全屏" @click="fullscreen"><UiIcon name="expand" />全屏</button>
            <button class="btn btn-sm plugin-visibility-toggle" :aria-label="paneMode === 'collapsed' ? '展开插件' : '收起插件'" :aria-expanded="paneMode !== 'collapsed'" aria-controls="gamer-plugin-workspace" :title="paneMode === 'collapsed' ? '展开插件工作区' : '收起插件，后台运行继续'" @click="setPaneMode(paneMode === 'collapsed' ? 'docked' : 'collapsed')"><UiIcon name="panel" />{{ paneMode === 'collapsed' ? '展开插件' : '收起插件' }}</button>
          </div>
        </div>
        <div v-show="!isGlobalPage && controlToolsOpen" class="tb-row tb-operation-row" :class="{ 'tb-browser-row': isBrowser }">
          <div v-if="isBrowser" class="tb-group tb-browser-group" role="group" aria-label="浏览器标签页">
            <span class="tb-label">网页</span>
            <select class="select tb-page-select" :value="browserPages.bound" :disabled="!connected || manualInputLocked" aria-label="目标标签页" @change="bindBrowserPage($event.target.value)">
              <option value="">当前绑定标签页</option>
              <option v-for="p in browserPages.pages" :key="p.id" :value="p.id">{{ p.title || p.url }}</option>
            </select>
            <button class="btn btn-sm" :disabled="!connected" title="读取浏览器标签页" @click="refreshBrowserPages"><UiIcon name="refresh" />读取</button>
          </div>
          <div v-if="!isBrowser" class="tb-group tb-app-group" role="group" aria-label="应用控制">
            <span class="tb-label">应用</span>
            <!-- 应用下拉（Android 运行目标）：选中即保存为设备配置包名，启动/脚本共用；
                 选项 = 设备配置包名 ∪ 已安装应用（「读取」拉取），Package 数据上下文与此无关 -->
            <select
              class="select mono tb-app-select"
              :value="current?.pkg || ''"
              :disabled="!current || appSelectSaving || manualInputLocked"
              aria-label="应用（Android 运行目标）"
              title="选择即保存执行目标，选择应用不会启动它"
              @change="onAppSelect"
            >
              <option value="" :disabled="pkgOptions.length > 0">{{ appLoading ? '正在读取应用…' : '请选择应用，或先读取列表' }}</option>
              <option v-for="p in pkgOptions" :key="p" :value="p">{{ packageOptionLabel(p) }}</option>
            </select>
            <button class="btn btn-sm" :disabled="!store.deviceId || appLoading" :title="appLoading ? '正在读取已安装应用…' : '读取设备已安装应用列表（填充应用下拉，强制刷新缓存）'" @click="loadApps({ force: true })"><UiIcon name="refresh" />{{ appLoading ? '读取中…' : '读取列表' }}</button>
            <button class="btn btn-sm" :disabled="!connected || !current?.pkg || manualInputLocked" @click="launchGame" :title="'冷启动当前应用，正在运行时会重启：' + (current?.pkg || '请先选择应用')"><UiIcon name="play" />启动 / 重启</button>
            <button class="btn btn-sm btn-danger" :disabled="!connected || !current?.pkg || manualInputLocked" @click="stopGame()" :title="'关闭当前应用：' + (current?.pkg || '请先选择应用')"><UiIcon name="stop" />停止应用</button>
          </div>
          <div class="tb-group tb-control-group" role="group" aria-label="投屏操作">
            <button class="btn btn-sm" title="截图" aria-label="截图" :disabled="!connected" @click="shot"><UiIcon name="image" />截图</button>
            <button v-if="isBrowser" class="btn btn-sm" :disabled="!connected || manualInputLocked" title="粘贴文字到网页" @click="clipboard()"><UiIcon name="copy" />粘贴</button>
            <button v-if="!isBrowser" class="btn btn-sm" title="返回" aria-label="返回" :disabled="!connected || manualInputLocked" @click="key('BACK')"><UiIcon name="back" />返回</button>
            <button
              class="btn btn-sm keyboard-mode-btn"
              :disabled="manualInputLocked"
              :class="{ active: keyboardMode === 'text' }"
              :title="keyboardMode === 'text' ? '当前为文本模式，字母和空格按文本发送' : '当前为游戏模式，保留按下/释放按键语义'"
              @click="toggleKeyboardMode"
            ><UiIcon name="keyboard" />{{ keyboardMode === 'text' ? '键盘：文本' : '键盘：游戏' }}</button>
            <div class="tb-more-wrap">
              <button
                class="btn btn-sm btn-icon"
                :class="{ active: toolbarMenuOpen === 'actions' }"
                aria-haspopup="menu"
                :aria-expanded="toolbarMenuOpen === 'actions'"
                aria-label="更多投屏功能"
                title="更多操控：粘贴 / 截图 / 按键 / 音量等"
                @click.stop="toggleToolbarMenu('actions', $event)"
              ><UiIcon name="more" /></button>
            </div>
            <span class="tb-label">操控</span>
          </div>
        </div>
      </div>
    <div class="console-body">
    <!-- 左：工具条与投屏画面共用一个键盘焦点区域 -->
    <div
      ref="stageFocusEl"
      v-show="!isGlobalPage && paneMode !== 'expanded'"
      class="stage"
      tabindex="0"
      role="region"
      aria-label="投屏控制区，可接收键盘控制"
      @focusin="onStageFocusIn"
      @focusout="onStageFocusOut"
      @keydown="onStageKeyDown"
      @keyup="onStageKeyUp"
      @click="onStageClick"
    >
      <div v-if="inputControlMessage" class="input-control-hint" :class="{ paused: !manualInputLocked }" role="status" aria-live="polite">{{ inputControlMessage }}</div>

      <!-- 菜单脱离横向滚动行挂到 body，避免窄窗口下被工具条裁掉 -->
      <Teleport to="body">
        <span v-if="toolbarMenuOpen" class="tb-more-mask" @click.stop="closeToolbarMenu"></span>
        <div v-if="toolbarMenuOpen === 'device'" class="tb-more-dropdown tb-more-dropdown-fixed action-menu" :style="toolbarMenuStyle" role="menu">
          <button class="tb-more-item action-menu-item" role="menuitem" :disabled="forceReconnecting" @click="closeToolbarMenu(); startAdd()">新增设备</button>
          <button class="tb-more-item action-menu-item" @click="closeToolbarMenu(); browserEdit = null; browserModal = true">新增浏览器目标</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="!current || forceReconnecting" @click="closeToolbarMenu(); openSettings()">设备设置</button>
          <button v-if="isBrowser" class="tb-more-item action-menu-item" role="menuitem" @click="closeToolbarMenu(); browserEdit = current; browserModal = true">浏览器设置</button>
          <button v-if="isBrowser" class="tb-more-item action-menu-item" role="menuitem" @click="closeToolbarMenu(); closeBrowserTarget()">关闭浏览器</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="!current || apkInstalling || forceReconnecting" :title="apkInstalling ? '正在上传并安装 APK…' : '选择本地 .apk 安装包安装到当前设备'" @click="closeToolbarMenu(); installApk()">安装应用</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="!current || connecting || forceReconnecting || apkInstalling" title="重启电脑端 ADB 服务并重新连接，会中断所有设备的投屏" @click="closeToolbarMenu(); forceReconnect()">{{ forceReconnecting ? '强制重连中…' : '强制重连' }}</button>
          <div class="action-menu-separator" role="separator"></div>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item danger" role="menuitem" :disabled="!current || forceReconnecting" @click="closeToolbarMenu(); removeDevice()">删除设备</button>
          <button v-if="isBrowser" class="tb-more-item action-menu-item danger" role="menuitem" @click="closeToolbarMenu(); removeBrowserTarget()">删除浏览器目标</button>
        </div>
        <div v-if="toolbarMenuOpen === 'actions'" class="tb-more-dropdown tb-more-dropdown-fixed action-menu" :style="toolbarMenuStyle" role="menu">
          <button class="tb-more-item action-menu-item" role="menuitem" :disabled="manualInputLocked" @click="closeToolbarMenu(); clipboard()">粘贴</button>
          <button class="tb-more-item action-menu-item" role="menuitem" @click="closeToolbarMenu(); shot()">截图</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="manualInputLocked" @click="closeToolbarMenu(); key('HOME')">主屏幕</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="manualInputLocked" @click="closeToolbarMenu(); key('BACK')">返回</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="manualInputLocked" @click="closeToolbarMenu(); rotate()">旋转</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="manualInputLocked" @click="closeToolbarMenu(); key('APP_SWITCH')">最近应用</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="manualInputLocked" @click="closeToolbarMenu(); key('VOL_UP')">增大音量</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="manualInputLocked" @click="closeToolbarMenu(); key('VOL_DOWN')">减小音量</button>
          <button v-if="!isBrowser" class="tb-more-item action-menu-item" role="menuitem" :disabled="!isSingleScreen" :title="!isSingleScreen ? '多格模式下全部画面自动静音' : audioMuted ? '取消静音（听游戏声音）' : '静音'" @click="closeToolbarMenu(); toggleAudio()">{{ !isSingleScreen ? '多画面已静音' : audioMuted ? '取消静音' : '静音' }}</button>
          <button v-else class="tb-more-item action-menu-item" role="menuitem" disabled title="浏览器目标只提供画面预览，后台浏览器始终静音">浏览器已静音</button>
        </div>
        <span v-if="targetMenuOpen" class="tb-more-mask" @click.stop="closeTargetMenu"></span>
        <div v-if="targetMenuOpen" class="tb-more-dropdown tb-more-dropdown-fixed action-menu" :style="targetMenuStyle" role="menu" :aria-label="targetName(targetMenuOpen) + ' 的画面操作'">
          <button v-if="!sessions[targetMenuOpen]?.connected.value" class="tb-more-item action-menu-item" role="menuitem" :disabled="sessions[targetMenuOpen]?.connecting.value" @click="sessions[targetMenuOpen]?.connect(true); closeTargetMenu()">连接预览</button>
          <button v-else class="tb-more-item action-menu-item" role="menuitem" @click="closeTargetPreview(targetMenuOpen); closeTargetMenu()">关闭预览</button>
          <div class="action-menu-separator" role="separator"></div>
          <button class="tb-more-item action-menu-item" role="menuitem" title="只移出画面，后台脚本继续运行" @click="removeTargetCell(targetMenuOpen); closeTargetMenu()">移出画面</button>
        </div>
      </Teleport>

      <BrowserTargetModal v-if="browserModal" :target="browserEdit" @close="browserModal = false" @saved="browserSaved" />
      <div v-if="batchResult" class="multiview-result" role="status">{{ batchResult }}</div>
      <div v-show="workspace.state.targetIds.length || stageCtl.view.kind !== 'media'" class="multiview-grid" :class="{ single: isSingleScreen, nine: workspace.state.gridSize === 9 }">
        <section v-for="id in workspace.state.targetIds" :key="id" v-show="visibleTargets.includes(id)"
          class="target-cell" :class="{ selected: isSingleScreen && id === store.deviceId }" :aria-label="targetName(id)">
          <div v-if="!isSingleScreen" class="target-heading" data-keyboard-ignore="true">
              <span class="target-name" :title="targetName(id)">{{ targetName(id) }}</span>
            <span class="target-status" :title="targetStatus(id)">{{ workspace.getRun(id) ? '运行中' : sessions[id]?.connecting.value ? '连接中' : sessions[id]?.connected.value ? '在线' : '未连接' }}</span>
            <button v-if="!workspace.getRun(id)" class="btn btn-sm btn-icon" :disabled="!canStartTarget(id) || !!workspace.state.pending[id]" @click="startOneTarget(id)" title="运行该目标记住的脚本" :aria-label="'运行 ' + targetName(id) + ' 的脚本'"><UiIcon name="play" /></button>
            <button v-else class="btn btn-sm btn-icon btn-danger" @click="stopOneTarget(id)" title="停止该目标脚本" :aria-label="'停止 ' + targetName(id) + ' 的脚本'"><UiIcon name="stop" /></button>
            <button class="btn btn-sm btn-icon" @click="toggleExpanded(id)" title="单屏查看此目标" :aria-label="'单屏查看 ' + targetName(id)"><UiIcon name="expand" /></button>
            <button class="btn btn-sm btn-icon" @click.stop="toggleTargetMenu(id, $event)" title="画面更多操作" :aria-label="targetName(id) + ' 的画面更多操作'" aria-haspopup="menu" :aria-expanded="targetMenuOpen === id"><UiIcon name="more" /></button>
          </div>
          <div v-if="workspace.state.errors[id]" class="target-error" role="status">{{ workspace.state.errors[id] }}</div>
          <div class="target-surface">
      <TargetPreviewSession
        :target-id="id" :selected="isSingleScreen && id === store.deviceId" :force-muted="!isSingleScreen"
        :active="pageVisible && !isGlobalPage && visibleTargets.includes(id) && !(id === store.deviceId && stageCtl.view.kind === 'media')"
        :preview-mode="isSingleScreen ? 'single' : workspace.state.gridSize === 9 ? 'grid9' : 'grid4'"
        @register="registerSession" @connected="onTargetConnected" @control-message="onTargetControlMessage"
        :bridge="deviceStageBridge"
        :current-name="targetName(id)"
        :show-hit="id === store.deviceId && (showHit)"
        :hit-miss="hitMiss"
        :hit-style="hitStyle"
        :hit-label="hitLabel"
        :selecting="id === store.deviceId && (selecting)"
        :sel-style="selStyle"
        :script-fx="id === store.deviceId ? scriptFx : emptyFx"
        :keymap-overlay="id === store.deviceId ? keymapOverlay : []"
        :bridge-overlays="id === store.deviceId ? bridgeOverlayView : []"
        :fx-tap-style="fxTapStyle"
        :fx-swipe-style="fxSwipeStyle"
        :fx-hit-style="fxHitStyle"
        :loupe="id === store.deviceId ? loupe : emptyLoupe"
        :stage="id === store.deviceId ? stageCtl.view : null"
        :selection-mode="id === store.deviceId && (picking || !!cellPick.mode || selecting)"
        :on-mouse-down="event => { if (id === store.deviceId) onMouseDown(event) }"
        :on-mouse-move="event => { if (id === store.deviceId) onMouseMove(event) }"
        :on-mouse-up="event => { if (id === store.deviceId) onMouseUp(event) }"
        :on-wheel="event => { if (id === store.deviceId) onWheel(event) }"
        :on-video-mouse-leave="event => { if (id === store.deviceId) onVideoMouseLeave(event) }"
        @video-mounted="onTargetVideoMounted"
        @wrap-mounted="onTargetWrapMounted"
        @loupe-mounted="el => { if (id === store.deviceId) onLoupeMounted(el) }"
        @media-video-mounted="el => { if (id === store.deviceId) onStageMediaVideoMounted(el) }"
      />
            <button v-if="!isSingleScreen || id !== store.deviceId" class="target-select-shield" :aria-label="'单屏查看 ' + targetName(id) + '，本次点击不会操作设备'" @click.stop="toggleExpanded(id)"><span>点击进入单屏</span></button>
          </div>
        </section>
        <div v-for="slot in emptySlots" :key="'empty-' + slot" class="target-empty">
          <span>{{ isSingleScreen ? '选择一个设备或浏览器开始' : '添加目标到画面 ' + (visibleTargets.length + slot) }}</span>
          <select class="select" aria-label="添加画面目标" value="" @change="addAndSelectTarget($event.target.value); $event.target.value = ''">
            <option value="">选择设备或浏览器…</option>
            <option v-for="target in availableTargets" :key="target.id" :value="target.id">{{ target.name }}</option>
          </select>
        </div>
      </div>
      <DeviceStage v-if="!workspace.state.targetIds.length" v-show="stageCtl.view.kind === 'media'"
        :browser-preview="isBrowser ? browserPreview.view : null"
        :on-browser-loaded="browserPreview.loaded"
        :bridge="deviceStageBridge"
        :connected="connected"
        :connecting="connecting"
        :error-msg="errorMsg"
        :current-name="currentName"
        :audio-muted="audioMuted"
        :show-hit="showHit"
        :hit-miss="hitMiss"
        :hit-style="hitStyle"
        :hit-label="hitLabel"
        :selecting="selecting"
        :sel-style="selStyle"
        :script-fx="scriptFx"
        :keymap-overlay="keymapOverlay"
        :bridge-overlays="bridgeOverlayView"
        :fx-tap-style="fxTapStyle"
        :fx-swipe-style="fxSwipeStyle"
        :fx-hit-style="fxHitStyle"
        :loupe="loupe"
        :stage="stageCtl.view"
        :selection-mode="picking || !!cellPick.mode || selecting"
        :on-mouse-down="onMouseDown"
        :on-mouse-move="onMouseMove"
        :on-mouse-up="onMouseUp"
        :on-wheel="onWheel"
        :on-video-mouse-leave="onVideoMouseLeave"
        :flush-and-connect="flushAndConnect"
        :fullscreen="fullscreen"
        @video-mounted="onVideoMounted"
        @wrap-mounted="onVideoWrapMounted"
        @loupe-mounted="onLoupeMounted"
        @media-video-mounted="onStageMediaVideoMounted"
      />

      <!-- 未启动应用提示：连接不再自动启动应用，画面停在桌面/黑屏时容易被误以为卡住。
           纯提示无按钮；显示几秒自动消失，应用已启动（手动/脚本拉起）或脚本运行中不出现 -->
      <div v-if="!isBrowser && connected && !appHintDismissed" class="app-hint">
        <span>已连接。未启动应用时画面停在桌面/黑屏</span>
      </div>
    </div>

    <!-- 左右分区拖拽条：拖动可手动调整画面区与功能区宽度 -->
    <div
      v-show="!isGlobalPage && paneMode === 'docked'"
      class="panel-resizer"
      :class="{ active: panelResizing }"
      role="separator"
      aria-orientation="vertical"
      aria-label="调整画面区与功能区宽度"
      :aria-valuenow="panelWidth"
      :aria-valuemin="PANEL_MIN_WIDTH"
      :aria-valuemax="PANEL_MAX_WIDTH"
      tabindex="0"
      @pointerdown="startPanelResize"
      @pointermove="onPanelResize"
      @pointerup="stopPanelResize"
      @pointercancel="stopPanelResize"
      @lostpointercapture="stopPanelResize"
      @keydown="onPanelResizeKeydown"
    ><UiIcon name="grip" /></div>
    <!-- 右：动态 Extension Workspace；二次裁切弹窗仍挂在面板层级，任何页签可见。 -->
    <aside id="gamer-plugin-workspace" v-show="isGlobalPage || paneMode !== 'collapsed'" class="panel" :style="{ width: isGlobalPage || paneMode === 'expanded' ? '100%' : `${panelWidth}px` }">
      <div v-if="!isGlobalPage" class="panel-target-context sr-context" role="status" aria-live="polite">
        <strong v-if="isSingleScreen" :title="store.deviceId ? `${targetName(store.deviceId)} · ${store.deviceId}` : '未选择目标'">当前操作目标：{{ store.deviceId ? targetName(store.deviceId) : '未选择目标' }}</strong>
        <strong v-else>多画面预览</strong>
        <span>{{ !isSingleScreen ? '点击画面进入单屏操作' : store.deviceId ? '右侧操作对应当前目标' : '请先选择目标' }}</span>
      </div>
      <PluginWorkspace
        :navigation-target="navigationReady ? '#gamer-main-navigation' : ''"
        :package-context="packageContext"
        :registry="panelRegistry"
        :active-panel="activePanelKey"
        :plugin-names="pluginNames"
        :plugin-targets="pluginTargets"
        :android-package-name="currentApplication?.pkg || ''"
        :context="workspaceContext"
        :lifecycle="workspaceLifecycle"
        :expanded="paneMode === 'expanded'"
        @expand="setPaneMode(paneMode === 'expanded' ? 'docked' : 'expanded')"
        @select="openPanel"
        @fallback="fallbackPanel"
        @extensions-changed="refreshServerExtensions"
      />
      <!-- 面板全部由 PanelRegistry 驱动（上方 PluginWorkspace）：
           gamer.core:tasks|logs|settings 是 Core 自有 UI（core-contributions）；
           扩展业务面板（自动化/函数/模板/映射）随扩展生命周期经服务端
           ui_contributions（runtime=core + component 键，前端
           core-component-registry 解析组件）出现/消失，壳内不再硬编码注册。 -->
      <!-- 二次裁切弹窗：挂面板层级（不在模板页签 v-show 内），从脚本编辑发起框选时不切页签 -->
      <TemplateCropModal :context="templateCaptureContext" :on-crop-mounted="onCropMounted" />
    </aside>
    </div>
    <OperationStatusBar :core="operationFeedback.state.core" :core-statuses="coreStatuses" :plugin="activeOperationFeedback" />
    <!-- 设备设置 / 新增设备弹窗 -->
    <DeviceSettingsModal :context="deviceSettingsContext" />

    <!-- 运行参数弹窗（脚本声明 params 时点运行/从此运行弹出；稀疏 args 提交、400 诊断回填标红） -->
    <RunParamsModal
      :open="runArgsFlow.modal.open"
      :title="runArgsFlow.modal.title"
      :desc="runArgsFlow.modal.desc"
      :submit-label="runArgsFlow.modal.submitLabel"
      :params="runArgsFlow.modal.params"
      :initial-args="runArgsFlow.modal.initialArgs"
      :suggestions="runArgsFlow.modal.suggestions"
      :templates="runArgsFlow.modal.templates"
      :field-errors="runArgsFlow.modal.fieldErrors"
      :general-errors="runArgsFlow.modal.generalErrors"
      :submitting="runArgsFlow.modal.submitting"
      @submit="onRunArgsSubmit"
      @close="runArgsFlow.close()"
    />

    <!-- 设备占用冲突 409 提示（对方脚本/来源/开始时间；仍要查看日志 → 跳控制台对应设备） -->
    <RunConflictModal />
  </div>
</template>

<script setup>
import { TemplateCropModal, RunParamsModal, useConsoleTemplates, pushRunEvent, useConsoleScriptRunner, useConsoleKeymap } from "../workspace/official-plugin-ui"
const confirmDialog = useConfirmDialog()
import { useConfirmDialog } from '../components/ui/useConfirmDialog'
import { STAGE_MEDIA_CONTROLLER_KEY } from '../workspace/context'
import UiIcon from '../components/ui/UiIcon.vue'
import OperationStatusBar from '../workspace/OperationStatusBar.vue'
import { createOperationFeedback, OPERATION_FEEDBACK_KEY } from '../workspace/operation-feedback'

// Console 壳：模板装配 + 各拆分模块接线。逻辑按域拆分至 components/console/ 下的
// composables（设备管理 / 模板裁切 / bridge overlay / 脚本运行 / 按键映射 /
// 传输统计 / workspace 面板接线），本文件保留投屏连接、输入控制与跨模块 glue。
import { computed, nextTick, onMounted, onUnmounted, provide, reactive, ref, shallowReactive, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { store, devicesData, scriptsData, templatesData, useToast, appStartedDevices, projectDeviceRun, getActiveRun } from '../store'
import { toDeviceCoord as mapControlCoord } from '../console/geometry'
import { api } from '../api'
import DeviceStage from '../workspace/DeviceStage.vue'
import TargetPreviewSession from '../components/console/TargetPreviewSession.vue'
import { multiviewWorkspace as workspace } from '../console/multiview-workspace'
import BrowserTargetModal from '../components/console/BrowserTargetModal.vue'
import PackageContextBar from '../workspace/PackageContextBar.vue'
import PluginWorkspace from '../workspace/PluginWorkspace.vue'
import { createPanelRegistry, DEFAULT_PANEL_KEY } from '../workspace/registry'
import { createWorkspaceContext, PANEL_REGISTRY_KEY, WORKSPACE_CONTEXT_KEY } from '../workspace/context'
import { createWorkspaceLifecycle } from '../workspace/lifecycle'
import { registerCoreContributions } from '../workspace/core-contributions'
import { createServerUiContributionAdapter } from '../workspace/plugin-center/adapter/server-ui'
import DeviceSettingsModal from '../components/console/DeviceSettingsModal.vue'
import RunConflictModal from '../components/RunConflictModal.vue'
import { useConsoleRuntime } from '../composables/useConsoleRuntime'
import { usePackageContext } from '../composables/usePackageContext'
import { loadPackages, currentPackageId, selectPackage, packageStore } from '../package-store'
import { useRunningPackageContext } from '../composables/useRunningPackageContext'
import { createKeyboardController, shouldIgnoreKeyboardTarget } from '../keyboard-control'
import { buildTouchPhase, createKeymapController } from '../keymap-control'
import { useConsolePanelResize } from '../components/console/useConsolePanelResize'
import { useConsoleDeviceManager } from '../components/console/useConsoleDeviceManager'
import { useConsoleInputControl } from '../components/console/useConsoleInputControl'
import { useConsoleStage } from '../components/console/useConsoleStage'
import { useConsoleBridgeOverlays } from '../components/console/useConsoleBridgeOverlays'
import { useWebrtcStats } from '../components/console/useWebrtcStats'
import { useConsoleWorkspacePanels } from '../components/console/useConsoleWorkspacePanels'
import { createPluginCallAdapter } from '../components/console/current-api-adapters'

const toast = useToast()
const navigationReady = ref(false)
let consoleDisposed = false
const operationFeedback = createOperationFeedback()
provide(OPERATION_FEEDBACK_KEY, operationFeedback)
const route = useRoute()
const router = useRouter()

// ---------- 共享基础状态（跨拆分模块的连接/画面/包名状态，由本壳统一持有） ----------
// 侧边栏已移除：右侧面板默认 340px，宽度由分隔条手动调整。
const sessions = shallowReactive({})
const selectedSession = computed(() => sessions[store.deviceId] || null)
function sessionRef(key, initial) {
  const fallback = ref(initial)
  return computed({ get: () => selectedSession.value?.[key]?.value ?? fallback.value,
    set: value => { const target = selectedSession.value?.[key]; if (target) target.value = value; else fallback.value = value } })
}
const superseded = sessionRef('superseded', false)
const manualClose = sessionRef('manualClose', false)
const connected = sessionRef('connected', false)
const connecting = sessionRef('connecting', false)
const errorMsg = sessionRef('errorMsg', '')
const fps = ref(0), delay = ref(0), res = sessionRef('resolution', '—'), bitrate = ref('—')
const audioMuted = sessionRef('audioMuted', true)
const consoleEl = ref(null), stageFocusEl = ref(null), toolbarEl = ref(null)
const videoWrap = sessionRef('videoWrap', null)
const videoElement = sessionRef('videoElement', null)
const keyboardFocused = ref(false)
const keyboardMode = ref('game')
const isSingleScreen = computed(() => workspace.state.gridSize === 1)
const keymapPressed = reactive(new Set())
// 当前 Package id（§38/§39）：由 package-store 统一管理，右侧 Package 栏选择；
// 模板/脚本/函数库/映射等数据面板全部消费该上下文。Android 应用（运行目标）与
// 其无关——启动/停止应用使用设备配置的 pkg（useConsoleDeviceManager）。
// 远端 keymap 扩展运行中：鼠标/滚轮/手柄输入改经 keymap 控制器（workspace 轮询写、输入层读）
const remoteKeymapRunning = ref(false)
// 传输统计看门狗所需的时间戳（连接生命周期的一部分，留在本壳）
let videoConnectTs = 0
let lastDragInputAt = 0
let keyboardChannelWarned = false

// ---------- 未启动应用提示（app-hint）：连接时出现、显示几秒自动消失（纯提示无按钮）。
// 应用已启动（手动点过启动按钮 / 脚本 str_app 拉起——按设备记入共享 store，
// 跨 SPA 切页存活）或脚本运行中则完全不出现
const APP_HINT_AUTO_CLOSE_MS = 5000
const appHintDismissed = ref(false)
let appHintTimer = null
watch(connected, (on) => {
  if (appHintTimer) { clearTimeout(appHintTimer); appHintTimer = null }
  if (!on) return
  appHintTimer = setTimeout(() => {
    appHintTimer = null
    appHintDismissed.value = true
  }, APP_HINT_AUTO_CLOSE_MS)
})
watch(() => store.running, (running) => {
  if (!running) return
  if (store.deviceId) appStartedDevices.add(store.deviceId)
  appHintDismissed.value = true
})

// ---------- 左右分区拖拽（面板宽度） ----------
const paneMode = ref('docked'), controlToolsOpen = ref(false)
function setPaneMode(mode) {
  releaseSelectedInput()
  closeToolbarMenu()
  paneMode.value = mode
  nextTick(() => {
    if (mode === 'collapsed') toolbarEl.value?.querySelector('.plugin-visibility-toggle')?.focus()
  })
}
const {
  panelWidth, panelResizing, PANEL_MIN_WIDTH, PANEL_MAX_WIDTH,
  startPanelResize, onPanelResize, stopPanelResize, onPanelResizeKeydown,
} = useConsolePanelResize({ consoleEl, videoWrap })

// ---------- 运行时数据加载/重连编排 ----------
// 壳只拉设备；脚本/模板等业务资源由各自面板实现自加载（ADR-11 知识边界）
const consoleRuntime = useConsoleRuntime({
  api,
  devicesData,
  deviceIdRef: computed(() => store.deviceId),
})

async function loadData() {
  await consoleRuntime.loadData()
}

// 通用目标控制租约：按钮态和所有舞台输入使用同一服务端状态，服务端再次仲裁。
const inputControl = useConsoleInputControl({ deviceId: () => store.deviceId, toast })
const manualInputLocked = computed(() => !isSingleScreen.value || !inputControl.manualAllowed.value)
const inputControlMessage = computed(() => !isSingleScreen.value
  ? '多画面仅预览，点击画面进入单屏后操作'
  : inputControl.message.value)

// ---------- 设备管理（工具条设备控件 + 设置弹窗 + 工具条快捷动作） ----------
const {
  devices, current, currentName, currentApplication,
  mode, form, scanning, configApplying, settingsOpen,
  screenSummary, formDirty,
  loadForm, startAdd, openSettings, cancelSettings, onDeviceSelect, refreshDeviceStatus, refreshDevices,
  saveSettings, flushAndConnect, addDevice, removeDevice, disconnect, loadApps,
  forceReconnecting, forceReconnect,
  appSelectSaving, onAppSelect, appLoading, pkgOptions, packageOptionLabel,
  key, toolbarMenuOpen, toolbarMenuStyle,
  closeToolbarMenu, toggleToolbarMenu, shot, rotate, clipboard, launchGame, stopGame,
  apkInstalling, installApk,
  deviceSettingsContext,
} = useConsoleDeviceManager({
  toast,
  feedback: operationFeedback,
  store,
  devicesData,
  scriptsData,
  templatesData,
  consoleRuntime,
  connected,
  errorMsg,
  appHintDismissed,
  loadData,
  connect,
  cleanup,
  sendControl,
  guardManualInput: () => isSingleScreen.value && inputControl.requireManual(),
  selectTarget: id => addAndSelectTarget(id),
  beforeTargetRemoval: id => beforeTargetChange(null),
  onTargetRemoved: id => removeTargetCell(id),
})

// ---------- 键盘与按键映射控制器（映射层命中时消费事件，未命中才交给 keyboard） ----------
const keyboard = createKeyboardController({
  send: sendKeyboardControl,
  onText: sendControl,
  mode: keyboardMode,
})
// 通过回调读取当前活动模型，因此切换方案不需要重建控制器或刷新页面。
const keymap = createKeymapController({
  getKeymap: () => activeKeymapModel.value,
  sendControl,
  send: sendControl,
  remote: remoteKeymapRunning,
  sendInputEvent: sendControl,
  getVideoSize: () => stageCtl.displaySize(),
  getKeyMetaState: () => keyboard.getMetaState(),
  mode: keyboardMode,
})

function syncKeymapPressed() {
  const codes = typeof keymap.getPressedCodes === 'function' ? keymap.getPressedCodes() : []
  keymapPressed.clear()
  for (const code of codes || []) keymapPressed.add(code)
}

// ---------- 统一舞台来源 StageSource（视频工作台 V1）----------
// 实时/视频来源切换、媒体控制、指定帧捕获与设备输入门禁收敛在 useConsoleStage；
// 壳只接线：工具条按钮态、门禁调用（sendControl/键盘/鼠标路由）与来源切换清理。
const isBrowser = computed(() => store.deviceId?.startsWith('browser-'))
const pageVisible = ref(!document.hidden)
const browserModal = ref(false), browserEdit = ref(null)
const browserPages = ref({ bound: '', pages: [] })
async function refreshBrowserPages() {
  const id = store.deviceId
  try {
    const pages = await api.browserPages(id)
    if (store.deviceId === id) browserPages.value = pages
  } catch (e) { if (store.deviceId === id) toast(e.message, 'error') }
}
async function bindBrowserPage(id) {
  if (!id) return
  if (!inputControl.requireManual()) return
  const target = store.deviceId, session = selectedSession.value
  try {
    await api.bindBrowser(target, id)
    if (store.deviceId === target) releaseSelectedInput()
    session?.close(); await session?.connect(true)
    if (store.deviceId === target) await refreshBrowserPages()
  } catch (e) { toast(e.message, 'error') }
}
const emptyBrowserView = reactive({ src: '', width: 0, height: 0, fps: 0 })
const browserPreview = {
  get view() { return selectedSession.value?.browser.view || emptyBrowserView },
  loaded: event => selectedSession.value?.browser.loaded(event),
  connect: () => selectedSession.value?.browser.connect(),
  close: () => selectedSession.value?.browser.close(),
  release: () => selectedSession.value?.browser.release(),
  send: value => selectedSession.value?.browser.send(value) || false,
  captureFrame: () => selectedSession.value?.browser.captureFrame(),
}
async function browserSaved(id) { browserModal.value = false; await loadData(); await addAndSelectTarget(id) }
async function closeBrowserTarget() {
  const id = store.deviceId, session = selectedSession.value
  if (!await confirmDialog(`关闭 ${targetName(id)} 的浏览器？这会影响此目标正在运行的任务。`, { title: '关闭浏览器', confirmText: '关闭浏览器' })) return
  try { await api.closeBrowser(id); if (store.deviceId === id) releaseSelectedInput(); session?.close(); await refreshDeviceStatus() } catch (e) { toast(e.message, 'error') }
}
async function removeBrowserTarget() {
  const id = store.deviceId
  if (!beforeTargetChange(null) || !await confirmDialog(`删除浏览器目标 ${targetName(id)}？`, { title: '删除浏览器目标', confirmText: '删除' })) return
  if (!beforeTargetChange(null)) return
  try { await api.deleteBrowser(id); await removeTargetCell(id); await loadData() } catch (e) { toast(e.message, 'error') }
}
const stageCtl = useConsoleStage({
  toast,
  deviceId: computed(() => store.deviceId),
  connected,
  liveVideoEl: () => videoElement.value,
  targetCapabilities: () => current.value?.capabilities,
  captureLiveFrame: () => isBrowser.value ? browserPreview.captureFrame() : undefined,
  liveSize: () => isBrowser.value ? { width: browserPreview.view.width, height: browserPreview.view.height } : null,
})
provide(STAGE_MEDIA_CONTROLLER_KEY, stageCtl.view)
const coreStatuses = computed(() => {
  const stage = stageCtl.view
  const statuses = []
  if (stage.kind === 'media' && stage.mediaId) {
    statuses.push(`视频 · ${stage.mediaSizeLabel || '读取尺寸…'} · ${stage.playing ? '播放中' : '已暂停'} · ${stage.timeText} / ${stage.durationText}`)
  } else if (connected.value) {
    if (inputControl.manualAllowed.value) {
      if (keyboardFocused.value && stage.canDeviceInput && !picking.value && !cellPick.mode) statuses.push('键盘控制已启用')
    }
    if (isBrowser.value) {
      statuses.push(`${browserPreview.view.width}×${browserPreview.view.height} · ${browserPreview.view.fps ? `${browserPreview.view.fps} 帧/秒` : '等待画面更新'}`)
      if (errorMsg.value) statuses.push(errorMsg.value)
    } else {
      statuses.push(`${res.value} · ${fps.value} fps · ${delay.value} ms · ${bitrate.value}`)
    }
  }
  if (picking.value || cellPick.mode) statuses.unshift(cellPick.mode === 'color' ? '点击取色 · Esc 取消' : cellPick.mode === 'coord' ? '点击取点 · Esc 取消' : '框选中 · Esc 取消')
  return statuses
})
watch(() => store.deviceId, () => { browserPages.value = { bound: '', pages: [] }; stageCtl.onDeviceChanged(); operationFeedback.setCore(null) })
watch(() => stageCtl.view.sourceId, () => { operationFeedback.setCore(null) })
/** 媒体 <video> 元素挂载/更换（含卸载传 null）：交给舞台组合式挂播放监听 */
function onStageMediaVideoMounted(el) {
  stageCtl.attachMediaVideo(el)
}

// ---------- 模板面板（列表/框选/二次裁切/放大镜/测试匹配/取值工具） ----------
const {
  picking, selecting, selStart, selEnd, showHit, hitLabel, hitMiss, hitStyle, selStyle,
  testThreshold, testRegion, tplSearch, templates, templateNames,
  viewTpl, confirmDelTpl, renaming, renameVal, crop, cropSize, cropZoomPct, saving,
  loupe,
  onLoupeMounted, onCropMounted, setRenameInputEl,
  togglePick, openCrop, selToDeviceRect, hideLoupe, updateLoupe, toDeviceCoord, deviceRectStyle,
  cropMouseDown, cropMouseMove, cropMouseUp, cropMouseLeave, cropWheel,
  saveTemplate, overwriteTemplate, backToCrop, cancelCrop, repick,
  onTplRowClick, onTplThumbClick, onTplNameClick, confirmRename, cancelRename, startRename,
  onTplDeleteClick, onTplMatchClick, onTplUpload,
  tplShortName, tplRegionBadge, tplThumbUrl, testMatch,
  selectRegionForBridge, beginCellPick, cancelCellPick, finishCellPick, cellPick,
  bridgeRegionSelected, finishBridgeRegionSelect, cancelBridgeRegionSelect, closeTplView,
  refreshTemplatesData, templateCaptureContext,
} = useConsoleTemplates({
  toast,
  feedback: operationFeedback,
  store,
  templatesData,
  packageId: currentPackageId,
  connected,
  videoElement,
  videoWrap,
  current,
  // 舞台桥：框选/裁切工作在当前舞台来源上（live=现有视频帧；media=服务端指定帧 PNG）
  stage: stageCtl.templateBridge,
  // 脚本运行 composable 的能力经懒解析箭头注入（规避组合顺序）
  editorMatchThreshold: () => editorMatchThreshold(),
  clearCallParamsCache: () => clearCallParamsCache(),
  refreshScripts: () => refreshScripts(),
  onTemplateRenamed: rename => onTemplateRenamed(rename),
})

// ---------- bridge overlay（sandbox UI 申请的画面叠加框） ----------
const { bridgeOverlayView, showBridgeOverlay, clearBridgeOverlay } = useConsoleBridgeOverlays({
  videoElement,
  deviceRectStyle,
})

// ---------- YAML 自动化面板运行器（脚本/函数面板共享机制 + 各自作用域上下文） ----------
// 壳只拿接线所需的共享机制与两份面板上下文；面板内部状态（编辑模式/选择）不进壳。
const {
  resourcePreview, closeResourcePreview,
  runArgsFlow, onRunArgsSubmit,
  startLogPolling, stopLogPolling,
  clearCallParamsCache, editorMatchThreshold, onTemplateRenamed,
  startRunStatusPoll, restoreRunState, onBeforeUnload,
  refreshScripts,
  scriptPanel, functionsPanel, beforePackageChange, beforeTargetChange, projectTargetConfig,
} = useConsoleScriptRunner({
  toast,
  multiviewWorkspace: workspace,
  packageId: currentPackageId,
  restorePackage: selectPackage,
  consoleRuntime,
  templateNames,
  tplShortName,
  loadData,
})

useRunningPackageContext({
  deviceId: () => store.deviceId,
  activeRun: () => getActiveRun(store.deviceId),
  packages: () => packageStore.packages,
  loaded: () => packageStore.loaded,
  currentPackageId: () => currentPackageId.value,
  selectPackage,
  canSwitch: () => !scriptPanel.sourceEditorDirty.value && !scriptPanel.sourceEditorSaving.value
    && !scriptPanel.startPending.value,
})

// ---------- 按键映射面板（方案选择/保存/导入导出 + 映射可视化） ----------
const {
  activeKeymapModel, keymapOverlay,
  loadKeymaps,
  keymapPanelContext,
} = useConsoleKeymap({
  api,
  toast,
  packageId: currentPackageId,
  keyboardMode,
  keymap,
  keymapPressed,
  stage: stageCtl.templateBridge,
  videoElement,
  videoWrap,
  deviceRectStyle,
  pickCoord: () => beginCellPick('coord'),
})

watch(currentPackageId, pkg => {
  loadKeymaps(pkg)
}, { immediate: true })

// ---------- Package 上下文条（§28 导入/导出/新建/复制/删除） ----------
// 包切换/导入会整体替换资源现场，经 refreshAll 全量重拉（脚本/模板/函数库/映射）
const packageContext = usePackageContext({
  toast,
  feedback: operationFeedback,
  beforePackageChange,
  currentApp: currentApplication,
  currentTargetId: () => store.deviceId,
  refreshAll: async () => {
    const pkg = currentPackageId.value
    await Promise.all([
      Promise.resolve(loadData?.()).catch(() => {}),
      Promise.resolve(refreshScripts?.()).catch(() => {}),
      Promise.resolve(refreshTemplatesData?.()).catch(() => {}),
      Promise.resolve(loadKeymaps(pkg)).catch(() => {}),
    ])
  },
})

// ---------- 传输统计与画面自愈看门狗 ----------
const { startStats, stopStats, resetWatchdogs, resetBlackWatchdog } = useWebrtcStats({
  getPeerConnection: () => webrtcLifecycle.getPeerConnection(),
  connected,
  videoElement,
  fps,
  delay,
  bitrate,
  sendControl,
  handleVideoSilence,
  getVideoConnectTs: () => videoConnectTs,
  getLastDragInputAt: () => lastDragInputAt,
})

// ---------- Frontend Plugin Workspace ----------
// Core panels are contributions too: one plugin may register multiple panels,
// and the workspace never needs to know a panel's component implementation.
// 裸 Core 只保留 任务/日志/设置；自动化/函数/模板/映射面板由扩展 manifest
// （runtime = "core" + component 键）经 server-ui adapter 驱动出现/消失。
const workspaceLifecycle = createWorkspaceLifecycle()
const panelRegistry = createPanelRegistry({ defaultPanelKey: DEFAULT_PANEL_KEY })
// Workspace 以稳定的 pluginId:panelId 作为 URL key；实际导航由
// activePanelKey + PanelRegistry 负责。
const activePanelKey = ref('workbench')
const isGlobalPage = computed(() => activePanelKey.value.startsWith('gamer.core:') || ['plugins', 'packages'].includes(activePanelKey.value))
const activeOperationFeedback = computed(() => {
  void panelRegistry.getPanels()
  return operationFeedback.state.plugins[panelRegistry.resolve(activePanelKey.value)?.pluginId] || null
})
watch(activePanelKey, (key, previous) => {
  for (const owner of Object.keys(operationFeedback.state.plugins)) operationFeedback.clearPlugin(owner)
  const global = value => value?.startsWith('gamer.core:') || ['plugins', 'packages'].includes(value)
  if (global(key) || global(previous)) operationFeedback.setCore(null)
}, { flush: 'sync' })
watch(currentPackageId, () => { for (const owner of Object.keys(operationFeedback.state.plugins)) operationFeedback.clearPlugin(owner) })
// 插件显示名（pluginId → name，来自扩展快照）：插件下拉菜单展示插件名而非面板清单
const pluginNames = ref({})
// 插件 Android Targets（pluginId → packages，来自扩展快照；空声明服务端归一 ['*']）：
// 插件下拉按当前设备应用过滤（* 通用恒显，具体包名需命中）
const pluginTargets = ref({})
registerCoreContributions(panelRegistry, { packageId: currentPackageId })
const serverUiAdapter = createServerUiContributionAdapter(panelRegistry, {
  load: () => api.listExtensions(),
})
const {
  refreshServerExtensions, startExtensionPolling, releaseRemoteGamepads, openPanel, fallbackPanel,
} = useConsoleWorkspacePanels({
  route,
  router,
  panelRegistry,
  serverUiAdapter,
  remoteKeymapRunning,
  keymap,
  connected,
  activePanelKey,
  pluginNames,
  pluginTargets,
})
// keymap 输入控制器接线：与面板注册解耦——挂载即启用本地映射（旧注册 lifecycle
// 的 start 语义），远端停止时释放残留按键（stop 语义）；远端模式本身只看
// remoteKeymapRunning（由扩展轮询写入，控制器内 remoteEnabled 读取）。
keymap.setEnabled(true)
watch(remoteKeymapRunning, running => {
  if (!running) keymap.releaseAll()
})

const workspaceContext = createWorkspaceContext({
  device: current,
  deviceId: computed(() => store.deviceId),
  androidPackageName: computed(() => currentApplication.value?.pkg || ''),
  currentPackageId,
  activePluginId: computed(() => {
    const key = String(activePanelKey.value || '')
    if (!key || ['plugins', 'packages', 'workbench'].includes(key) || key.startsWith('gamer.core:')) return ''
    return key.split(':', 1)[0] || ''
  }),
  connected,
  stage: {
    selectRegion: selectRegionForBridge,
    pickPoint: () => beginCellPick('coord'),
    overlay: { show: showBridgeOverlay, clear: clearBridgeOverlay },
  },
  stageContext: stageCtl.view,
  openPanel,
  toast,
  dialogConfirm: message => confirmDialog(message, { title: '插件请求确认' }),
  // iframe 面板 plugin.call → UI Bridge → 这里转发到 REST /api/extensions/:id/call
  //（declarative 面板不经 bridge，已直连 callExtension）
  pluginCall: createPluginCallAdapter(api),
  operationStatus: (payload, meta) => {
    // Identity comes from the host-owned contribution, never from the payload.
    const active = panelRegistry.resolve(activePanelKey.value)
    if (active?.pluginId !== meta.pluginId || active?.panelId !== meta.panelId) return false
    if (!payload) operationFeedback.clearPlugin(meta.pluginId)
    else operationFeedback.setPlugin(meta.pluginId, { ...payload, actions: (payload.actions || []).map(action => ({ ...action, run: typeof action.panel === 'string' && panelRegistry.resolve(action.panel) ? () => openPanel(action.panel) : undefined })) })
    return true
  },
  core: {
    templateCapture: templateCaptureContext,
    // YAML 自动化扩展的两个面板各自绑定一份作用域上下文（编辑模式/选择互不串台）
    scriptRunner: { scripts: scriptPanel, functions: functionsPanel },
    keymap: keymapPanelContext,
    packageId: currentPackageId,
  },
})
const deviceStageBridge = workspaceContext.stage
provide(PANEL_REGISTRY_KEY, panelRegistry)
provide(WORKSPACE_CONTEXT_KEY, workspaceContext)

// ---------- WebRTC 连接 ----------
const webrtcLifecycle = {
  getPeerConnection: () => selectedSession.value?.rtc.getPeerConnection(),
  getControlChannel: () => selectedSession.value?.rtc.getControlChannel(),
  scheduleReconnect: options => selectedSession.value?.rtc.scheduleReconnect(options),
  cleanup: manual => selectedSession.value?.rtc.cleanup(manual),
}
function scheduleReconnect() { webrtcLifecycle.scheduleReconnect({ superseded }) }
async function connect(manual = false) {
  await nextTick()
  if (consoleDisposed) return
  await selectedSession.value?.connect(manual)
}
/** Close only the current viewer, never the backend target or its run. */
function cleanup() { releaseSelectedInput(); selectedSession.value?.close(); consoleRuntime.cleanup() }

function handleVideoSilence() {
  if (manualClose.value || !connected.value || !store.deviceId) return
  console.warn('[webrtc] video stream silent, treating as disconnected')
  connected.value = false
  scheduleReconnect()
}

// ---------- 控制（走 DataChannel） ----------

const REST_FALLBACK_CONTROL_TYPES = new Set([
  'tap', 'swipe', 'text', 'press', 'home', 'back', 'recents', 'start_app', 'rotate', 'clipboard',
])

/** 键盘是有状态的 DOWN/UP 流，只允许走 DataChannel；不能复用 sendControl 的
 * REST fallback，否则通道断开时一次 keydown 会被错误降级为不兼容的 press。 */
let releasingInput = false
function isReleaseMessage(obj) {
  return (obj.type === 'touch' && obj.action === 'up')
    || (obj.type === 'key' && obj.action === 1)
    || (obj.type === 'input_event' && (['key_up', 'mouse_up'].includes(obj.event?.type)
      || (obj.event?.type === 'gamepad_button' && !obj.event.pressed && !obj.event.value)
      || (obj.event?.type === 'gamepad_axis' && !obj.event.value)))
}
function sendKeyboardControl(obj) {
  if (!isSingleScreen.value && !(releasingInput && isReleaseMessage(obj))) return false
  if (!inputControl.guardDeviceInput(obj)) return false
  // 安全红线：视频来源（媒体模式）为离线只读，舞台产生的键盘/按键映射输入一律拒绝
  if (!(releasingInput && isReleaseMessage(obj)) && !stageCtl.guardDeviceInput(obj)) return false
  if (isBrowser.value) {
    if (obj.type === 'touch') {
      if (obj.pointer_id !== 0) { toast('当前目标不支持持续触控映射', 'warn'); return false }
      return browserPreview.send({ type: 'pointer', action: obj.action, x: obj.x, y: obj.y })
    }
    if (obj.type === 'scroll') return browserPreview.send({ type: 'scroll', x: obj.x, y: obj.y, delta_x: obj.scroll_x || 0, delta_y: obj.scroll_y || 0 })
    if (obj.type === 'input_event' || obj.type === 'text' || obj.type === 'tap') return browserPreview.send(obj)
    toast('此操作不适用于浏览器目标', 'warn'); return false
  }
  const channel = webrtcLifecycle.getControlChannel()
  if (channel && channel.readyState === 'open') {
    channel.send(JSON.stringify(obj))
    keyboardChannelWarned = false
    return true
  }
  if (!keyboardChannelWarned) {
    keyboardChannelWarned = true
    toast('键盘控制通道未连接', 'warn')
  }
  return false
}

function sendControl(obj) {
  if (!isSingleScreen.value && !(releasingInput && isReleaseMessage(obj))) return false
  if (!inputControl.guardDeviceInput(obj)) return false
  // 安全红线：视频来源（媒体模式）为离线只读——鼠标触控/滚轮/按键/启停应用等
  // 舞台产生的设备输入在统一输入路由处拒绝（含 REST fallback 之前的全部路径）
  if (!(releasingInput && isReleaseMessage(obj)) && !stageCtl.guardDeviceInput(obj)) return false
  if (isBrowser.value) {
    if (obj.type === 'touch') return browserPreview.send({ type: 'pointer', action: obj.action, x: obj.x, y: obj.y })
    if (obj.type === 'scroll') return browserPreview.send({ type: 'scroll', x: obj.x, y: obj.y, delta_x: obj.scroll_x || 0, delta_y: obj.scroll_y || 0 })
    if (obj.type === 'input_event' || obj.type === 'text' || obj.type === 'tap') return browserPreview.send(obj)
    toast('此操作不适用于浏览器目标', 'warn'); return false
  }
  // 拖动/滚轮类输入打标（画面停滞看门狗用）：这类操作预期画面变化，
  // 若随后渲染指纹持续冻结则流已病态（见 startStats 处注释）
  if ((obj.type === 'touch' && obj.action === 'move') || obj.type === 'scroll' || obj.type === 'swipe') {
    lastDragInputAt = Date.now()
  }
  const channel = webrtcLifecycle.getControlChannel()
  if (channel && channel.readyState === 'open') {
    channel.send(JSON.stringify(obj))
    return true
  }
  // 有状态消息必须保持在 DataChannel 内：REST 只有一次性动作语义，
  // 不能把 touch down/up 或 key down/up 降级成 press，否则会留下半截状态
  // 或把虚拟触控误发成 Android 物理按键。
  const stateful = obj?.type === 'touch'
    || obj?.type === 'input_event'
    || (obj?.type === 'key' && (obj?.action === 0 || obj?.action === 1))
  if (stateful) {
    console.warn('[control] channel not open, stateful control dropped', JSON.stringify(obj))
    if (!keyboardChannelWarned) {
      keyboardChannelWarned = true
      toast('控制通道未连接', 'warn')
    }
    return false
  }
  if (!REST_FALLBACK_CONTROL_TYPES.has(obj?.type)) {
    console.warn('[control] channel not open, unsupported control dropped', JSON.stringify(obj))
    return false
  }
  console.warn('[control] channel not open, fallback REST', JSON.stringify(obj))
  // fallback：REST API
  api.control(store.deviceId, obj).catch(e => toast('控制失败：' + e.message, 'error'))
  // REST 请求已经接管了一次性动作；返回 true 避免映射层误把同一按键
  // 再交给原始 Android key 控制器。
  return true
}

/** 统一构造触控阶段消息。pointer_id=0 保留给投屏鼠标。 */
function sendTouchPhase(action, pointerId, x, y) {
  return sendControl(buildTouchPhase(action, pointerId, x, y))
}

// ---------- 键盘焦点区域与工具条 ----------

function onStageFocusIn(e) {
  // 视频来源模式不捕获键盘焦点（键盘/按键映射属设备输入，媒体模式拒绝）
  if (shouldIgnoreKeyboardTarget(e?.target)) { releaseStageKeyboard(); return }
  if (!connected.value || manualInputLocked.value || !stageCtl.view.canDeviceInput) return
  keyboardFocused.value = true
}

function onStageFocusOut(e) {
  const next = e?.relatedTarget
  if (next && stageFocusEl.value?.contains(next) && !shouldIgnoreKeyboardTarget(next)) return
  releaseStageKeyboard()
}

function releaseStageKeyboard() {
  if (isBrowser.value) browserPreview.release()
  keyboardFocused.value = false
  keymap.releaseAll()
  syncKeymapPressed()
  keyboard.releaseAll()
}

function onStageKeyDown(e) {
  if (e.defaultPrevented || e.isComposing || shouldIgnoreKeyboardTarget(e.target) || hasOpenInputDialog()
    || !connected.value || manualInputLocked.value || !stageCtl.view.canDeviceInput || picking.value || selecting.value || cellPick.mode || isGlobalEscapeConsumed(e)) return
  if (keyboardMode.value === 'game') {
    const mapped = keymap.handleKeyDown(e)
    syncKeymapPressed()
    if (mapped?.handled || mapped === true) return
  }
  // 控制器只对已映射且未被 UI 过滤的按键 preventDefault；未知按键保留浏览器行为。
  if (isBrowser.value) { if (!shouldIgnoreKeyboardTarget(e.target)) { e.preventDefault(); browserPreview.send({ type: 'key', key: e.key === ' ' ? 'Space' : e.key, action: 'down' }) } return }
  keyboard.handleKeyDown(e)
}

function onStageKeyUp(e) {
  if (shouldIgnoreKeyboardTarget(e.target) || hasOpenInputDialog() || !connected.value || manualInputLocked.value || !stageCtl.view.canDeviceInput) return
  const mapped = keymap.handleKeyUp(e)
  syncKeymapPressed()
  if (mapped?.handled || mapped === true) return
  if (isBrowser.value) { if (!shouldIgnoreKeyboardTarget(e.target)) { e.preventDefault(); browserPreview.send({ type: 'key', key: e.key === ' ' ? 'Space' : e.key, action: 'up' }) } return }
  keyboard.handleKeyUp(e)
}

function onStageClick(e) {
  const target = e?.target
  if (shouldIgnoreKeyboardTarget(target)) return
  if (target?.closest?.('button, input, select, textarea, a, [contenteditable], [role="button"], [role="link"], [role="menuitem"]')) return
  stageFocusEl.value?.focus()
}

function onToolbarClick(e) {
  const target = e?.target
  const button = target?.closest?.('button')
  if (!button || !toolbarEl.value?.contains(button)) return
  // 插件显示开关留在原位置，键盘可连续收起与展开。
  if (button.classList.contains('plugin-visibility-toggle')) return
  // 工具栏按钮执行完动作后把焦点还给组合区域，点击模式切换后可以直接输入。
  nextTick(() => { if (!hasOpenInputDialog()) stageFocusEl.value?.focus() })
}

function hasOpenInputDialog() {
  return browserModal.value || settingsOpen.value || [...document.querySelectorAll('[aria-modal="true"],dialog[open],.modal-mask,.tpl-view-mask')].some(el => el.getClientRects().length > 0)
}

function toggleKeyboardMode() {
  keyboardMode.value = keyboardMode.value === 'game' ? 'text' : 'game'
  nextTick(() => stageFocusEl.value?.focus())
}

/** 静音开关（「功能」菜单）：只切本地播放，音频轨仍在传输；
 *  与 onChannelOpen 建链时的 audio 消息同词表 */
function toggleAudio() {
  if (!isSingleScreen.value) { operationFeedback.setCore({ text: '多画面模式已自动静音' }); return }
  audioMuted.value = !audioMuted.value
  selectedSession.value?.setMuted(audioMuted.value)
  operationFeedback.setCore({ text: audioMuted.value ? '已静音' : '已取消静音' })
}

watch(keyboardMode, mode => {
  if (mode === 'text') {
    keymap.releaseAll()
    syncKeymapPressed()
  }
})

function onWindowBlur() {
  if (isBrowser.value) browserPreview.release()
  releaseRemoteGamepads()
  keymap.releaseAll()
  syncKeymapPressed()
  keyboard.releaseAll()
}

function onVisibilityChange() {
  pageVisible.value = !document.hidden
  if (document.hidden) {
    if (isBrowser.value) browserPreview.release()
    keymap.releaseAll()
    syncKeymapPressed()
    keyboard.releaseAll()
  }
}

/** 全局 Escape 关闭页面 UI 优先；只有没有待关闭 UI 时才把 Escape 转发给设备。 */
function isGlobalEscapeConsumed(e) {
  if (e?.code !== 'Escape' && e?.key !== 'Escape') return false
  return !!(
    cellPick.mode
    || targetMenuOpen.value
    || toolbarMenuOpen.value
    || browserModal.value
    || settingsOpen.value
    || viewTpl.value
    || resourcePreview?.open
    || confirmDelTpl.value
  )
}

/** 全局按键：Esc 关闭工具条菜单 / 设备设置弹窗 / 模板大图 / 资源预览 / 取消删除确认 */
function onGlobalKeydown(e) {
  if (e.key !== 'Escape' || e.defaultPrevented) return
  if (cancelBridgeRegionSelect()) {
    // bridge 框选被 Esc 取消
  } else if (cellPick.mode) {
    cancelCellPick()
  } else if (targetMenuOpen.value) {
    closeTargetMenu()
  } else if (toolbarMenuOpen.value) {
    closeToolbarMenu()
  } else if (browserModal.value) {
    browserModal.value = false
  } else if (settingsOpen.value) {
    cancelSettings()
  } else if (viewTpl.value) {
    closeTplView()
  } else if (resourcePreview?.open) {
    closeResourcePreview()
  } else if (confirmDelTpl.value) {
    confirmDelTpl.value = null
  }
}

// ---------- 脚本运行可视化效果 ----------

// 服务端经 control DataChannel 推送 tap/swipe/hit/miss 事件（设备像素坐标），
// 与手动 alt 反馈状态独立（脚本运行时用户仍可手动操作，两类效果互不覆盖）
const scriptFx = reactive({
  tap: { show: false, x: 0, y: 0 },
  swipe: { show: false, x: 0, y: 0, w: 0, h: 0 },
  hit: { show: false, x: 0, y: 0, w: 0, h: 0, label: '', miss: false },
})
let fxTapTimer = null
let fxSwipeTimer = null
let fxHitTimer = null

/** 服务端→浏览器脚本可视化事件（{"type":"se","ev":"tap"|"swipe"|"hit"|"miss", ...}，设备像素坐标）：
 *  引擎执行 tap/swipe、模板匹配命中/未命中时推送到投屏画面
 *  （样式复用 alt 反馈/测试匹配命中框；miss 显示搜索区域，虚线红框）
 *  同一轮匹配的多个模板事件会互相顶替，显示的是最新一次。
 *  运行结构事件（run/step/call/vision/budget，P12.6）经 pushRunEvent 分发给
 *  运行事件 feed 与步骤高亮，不进投屏标记。 */
function onControlMessage(e) {
  let msg
  try { msg = JSON.parse(e.data) } catch (err) { return }
  if (!msg || msg.type !== 'se') return
  if (pushRunEvent(msg)) return
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

/** 脚本运行可视化效果位置（tap 圆点居中偏移由 .alt-tap 的 transform 处理） */
const fxTapStyle = computed(() => (scriptFx.tap.show ? deviceRectStyle(scriptFx.tap.x, scriptFx.tap.y) : {}))
const fxSwipeStyle = computed(() => (scriptFx.swipe.show
  ? deviceRectStyle(scriptFx.swipe.x, scriptFx.swipe.y, scriptFx.swipe.w, scriptFx.swipe.h)
  : {}))
const fxHitStyle = computed(() => (scriptFx.hit.show
  ? deviceRectStyle(scriptFx.hit.x, scriptFx.hit.y, scriptFx.hit.w, scriptFx.hit.h)
  : {}))

// ---------- 鼠标/滚轮输入（触控、框选、取点、映射输入路由） ----------

// 手动输入坐标属于核心画面链路，不依赖已安装模板插件对 img/video 的支持。
function stageControlPoint(e) {
  const size = stageCtl.displaySize()
  const rect = stageCtl.surfaceEl()?.getBoundingClientRect()
  return mapControlCoord(e.clientX, e.clientY, rect, size.width, size.height)
}

// 触控状态
const touchState = reactive({ active: false, lastX: 0, lastY: 0 })
let gestureOrigin = null

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
  pendingMove = { type: 'touch', action: 'move', pointer_id: 0, x, y }
  if (!moveRaf) moveRaf = requestAnimationFrame(flushPendingMove)
}
function cancelPendingMove() {
  if (moveRaf) { cancelAnimationFrame(moveRaf); moveRaf = 0 }
  pendingMove = null
}

function onMouseDown(e) {
  if (!isSingleScreen.value) return
  // 步骤编辑器取点/取色模式：本次画面点击被消费（不透传触控）
  if (cellPick.mode && connected.value) {
    finishCellPick(e)
    return
  }
  // 框选：实时（已连接）与视频来源（画面就绪）均可工作，坐标随舞台来源
  if (picking.value && stageCtl.view.stageReady) {
    const rect = videoWrap.value.getBoundingClientRect()
    selStart.x = e.clientX - rect.left
    selStart.y = e.clientY - rect.top
    selEnd.x = selStart.x; selEnd.y = selStart.y
    selecting.value = true
    return
  }
  // 设备输入（触控/按键映射）：视频来源为只读，统一拒绝（触控终不发）
  if (!connected.value || manualInputLocked.value || !stageCtl.view.canDeviceInput) return
  cancelPendingMove()
  const { x, y } = stageControlPoint(e)
  if (remoteKeymapRunning.value) {
    keymap.handleInputEvent({ type: 'mousedown', button: e.button, x, y }, 'down', e)
    return
  }
  if (isBrowser.value) { e.preventDefault(); stageFocusEl.value?.focus({ preventScroll: true }); const { x, y } = stageControlPoint(e); browserPreview.send({ type: 'pointer', action: 'down', button: ['left', 'middle', 'right'][e.button] || 'left', x, y }); return }
  gestureOrigin = { x, y }
  touchState.active = true
  touchState.lastX = x; touchState.lastY = y
  // 按下：发 DOWN（拖动时后续 move 事件组成轨迹，up 时收尾）
  sendTouchPhase('down', 0, x, y)
}

function onMouseMove(e) {
  if (!isSingleScreen.value) return
  if (selecting.value) {
    const rect = videoWrap.value.getBoundingClientRect()
    selEnd.x = e.clientX - rect.left
    selEnd.y = e.clientY - rect.top
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 2.5, [selToDeviceRect()])
    return
  }
  if (cellPick.mode) {
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 3, [])
    return
  }
  if (picking.value) {
    updateLoupe(e.clientX, e.clientY, toDeviceCoord(e.clientX, e.clientY), 2.5, [])
    return
  }
  if (manualInputLocked.value) return
  if (remoteKeymapRunning.value && connected.value && stageCtl.view.canDeviceInput) {
    const { x, y } = stageControlPoint(e)
    keymap.handleInputEvent({
      type: 'mousemove', x, y, movementX: e.movementX, movementY: e.movementY,
    }, 'move', e)
    return
  }
  if (isBrowser.value && connected.value && stageCtl.view.canDeviceInput) { const { x, y } = stageControlPoint(e); browserPreview.send({ type: 'pointer', action: 'move', x, y }); return }
  if (!touchState.active || !connected.value) return
  const { x, y } = stageControlPoint(e)
  if (Math.abs(x - touchState.lastX) + Math.abs(y - touchState.lastY) > 6) {
    touchState.lastX = x; touchState.lastY = y
    scheduleMove(x, y)
  }
}

function onMouseUp(e) {
  if (!isSingleScreen.value) return
  if (selecting.value) {
    selecting.value = false
    picking.value = false
    hideLoupe()
    const rect = selToDeviceRect()
    if (rect.w >= 8 && rect.h >= 8) operationFeedback.setCore({ text: '已框选', actions: [{ label: `(${rect.x}, ${rect.y}, ${rect.w}, ${rect.h})`, copy: `[${rect.x}, ${rect.y}, ${rect.w}, ${rect.h}]` }] })
    if (bridgeRegionSelected()) {
      finishBridgeRegionSelect(rect)
      return
    }
    if (rect.w >= 8 && rect.h >= 8) openCrop(rect)
    else toast('框选区域太小，请重新框选', 'warn')
    return
  }
  if (manualInputLocked.value) { cancelPendingMove(); touchState.active = false; return }
  if (remoteKeymapRunning.value && connected.value && stageCtl.view.canDeviceInput) {
    const { x, y } = stageControlPoint(e)
    keymap.handleInputEvent({ type: 'mouseup', button: e.button, x, y }, 'up', e)
    return
  }
  if (isBrowser.value && connected.value && stageCtl.view.canDeviceInput) { const { x, y } = stageControlPoint(e); browserPreview.send({ type: 'pointer', action: 'up', button: ['left', 'middle', 'right'][e.button] || 'left', x, y }); return }
  if (!touchState.active) return
  cancelPendingMove()
  touchState.active = false
  const { x, y } = stageControlPoint(e)
  sendTouchPhase('up', 0, x, y)
  const w = (videoElement.value?.naturalWidth || videoElement.value?.videoWidth), h = (videoElement.value?.naturalHeight || videoElement.value?.videoHeight)
  operationFeedback.setCore({ text: gestureOrigin && Math.hypot(x - gestureOrigin.x, y - gestureOrigin.y) > 6 ? '滑动至' : '点击了', actions: [
    { label: `(${x}, ${y})`, copy: `[${x}, ${y}]` },
    ...(w && h ? [{ label: `(${(x / w).toFixed(4)}, ${(y / h).toFixed(4)})`, copy: `[${(x / w).toFixed(4)}, ${(y / h).toFixed(4)}]` }] : []),
  ] })
}

/** 鼠标离开投屏区域时隐藏取点/框选辅助层。 */
function onVideoMouseLeave() {
  if (isSingleScreen.value && isBrowser.value) browserPreview.release()
  hideLoupe()
}

function onWheel(e) {
  // 滚轮 = 设备输入：视频来源（媒体模式）为只读，统一拒绝
  if (!connected.value || manualInputLocked.value || !stageCtl.view.canDeviceInput) return
  const { x, y } = stageControlPoint(e)
  if (remoteKeymapRunning.value) {
    keymap.handleInputEvent({
      type: 'wheel', x, y, deltaX: e.deltaX, deltaY: e.deltaY,
    }, 'wheel', e)
    return
  }
  sendControl({ type: 'scroll', x, y, scroll_x: e.deltaX, scroll_y: e.deltaY })
}

// 取得自动控制权后清空本页的本地按住状态。实际输入释放由服务端 owner 屏障负责。
watch(inputControl.manualAllowed, allowed => {
  if (allowed) return
  cancelPendingMove()
  touchState.active = false
  gestureOrigin = null
  keymap.releaseAll()
  syncKeymapPressed()
  keyboard.releaseAll()
  keyboardFocused.value = false
}, { flush: 'sync' })

// ---------- 舞台来源切换清理（合同 §4.2）----------
// 切换实时/视频时：绝不自动恢复按键按下状态，清指针（拖拽/待发 move）、键盘焦点
// （keymap/keyboard 残留按下全部释放）、框选进行态与旧来源的叠加层标记
watch(() => stageCtl.view.kind, () => {
  releaseSelectedInput()
}, { flush: 'sync' })

function fullscreen() {
  const target = stageCtl.view.kind === 'media' ? videoWrap.value?.parentElement : videoWrap.value
  if (target?.requestFullscreen) target.requestFullscreen()
}

function onVideoMounted(el) { videoElement.value = el }
function onVideoWrapMounted(el) { videoWrap.value = el }

// ---------- Multi-target layout and atomic focus projection ----------
const visibleTargets = computed(() => workspace.visibleTargetIds)
const visibleRunningCount = computed(() => visibleTargets.value.filter(id => workspace.getRun(id)).length)
const availableTargets = computed(() => devices.value.filter(target => !workspace.state.targetIds.includes(target.id)))
const emptySlots = computed(() => Math.max(0, workspace.state.gridSize - visibleTargets.value.length))
const hiddenCount = computed(() => workspace.state.targetIds.length - visibleTargets.value.length)
const hiddenRunningCount = computed(() => workspace.state.targetIds.filter(id => !visibleTargets.value.includes(id) && workspace.getRun(id)).length)
const batchBusy = ref(false), batchResult = ref('')
const targetMenuOpen = ref(null), targetMenuStyle = ref({})
const emptyFx = Object.freeze({ tap: { show: false }, swipe: { show: false }, hit: { show: false } })
const emptyLoupe = Object.freeze({ show: false })
function targetName(id) { return devices.value.find(device => device.id === id)?.name || id }
function canStartTarget(id) {
  const config = workspace.getConfig(id)
  return !!(id && config?.packageId && config.scriptId && config.runnerId && !workspace.getRun(id)
    && devices.value.find(device => device.id === id)?.status === 'online')
}
function closeTargetMenu() { targetMenuOpen.value = null }
function toggleTargetMenu(id, event) {
  if (targetMenuOpen.value === id) return closeTargetMenu()
  closeToolbarMenu()
  const rect = event.currentTarget.getBoundingClientRect()
  targetMenuStyle.value = { left: `${Math.max(8, Math.min(rect.right - 170, window.innerWidth - 178))}px`, top: `${Math.max(8, Math.min(rect.bottom + 4, window.innerHeight - 112))}px`, width: '170px' }
  targetMenuOpen.value = id
}
function targetStatus(id) {
  const session = sessions[id]
  const run = workspace.getRun(id)
  const preview = session?.suspended.value ? '预览已暂停' : session?.connecting.value ? '连接中' : session?.connected.value ? '预览中' : '预览关闭'
  return `${preview} · ${run ? (run.status === 'paused' ? '脚本已暂停' : '脚本运行中') : '空闲'}`
}
function registerSession(id, session) { if (session) sessions[id] = session; else delete sessions[id] }
function onTargetVideoMounted(id, el) { if (sessions[id]) sessions[id].videoElement.value = el }
function onTargetWrapMounted(id, el) { if (sessions[id]) sessions[id].videoWrap.value = el }
function onTargetControlMessage(id, event) { if (id === store.deviceId) onControlMessage(event) }
function onTargetConnected(id) {
  refreshDeviceStatus()
  if (id !== store.deviceId) return
  startStats(); startLogPolling()
  if (id.startsWith('browser-')) refreshBrowserPages()
  else loadApps({ silent: true })
}
function releaseSelectedInput() {
  releasingInput = true
  try {
  cancelPendingMove()
  if (touchState.active) sendTouchPhase('up', 0, touchState.lastX, touchState.lastY)
  touchState.active = false
  gestureOrigin = null
  browserPreview.release()
  releaseRemoteGamepads()
  keymap.releaseAll(); syncKeymapPressed(); keyboard.releaseAll()
  keyboardFocused.value = false
  selectedSession.value?.setMuted(true)
  picking.value = false; selecting.value = false
  cancelCellPick(); cancelBridgeRegionSelect(); cancelCrop(); hideLoupe()
  scriptFx.tap.show = false; scriptFx.swipe.show = false; scriptFx.hit.show = false
  } finally { releasingInput = false }
}
watch(isSingleScreen, releaseSelectedInput, { flush: 'sync' })
const focusOptions = {
  beforeChange: id => {
    if (saving.value || configApplying.value || forceReconnecting.value || appSelectSaving.value) { toast('当前目标操作尚未结束，请稍后切换', 'warn'); return false }
    return beforeTargetChange(id)
  },
  project: id => {
    // Refusal leaves both the old visual focus and sidebar untouched.
    releaseSelectedInput()
    if (!projectTargetConfig(id)) return false
    stageCtl.onTargetChanged()
    stopStats(); resetWatchdogs(); resetBlackWatchdog(); stopLogPolling()
    videoConnectTs = Date.now(); lastDragInputAt = 0
    store.deviceId = id
    projectDeviceRun(id)
    onLoupeMounted(sessions[id]?.loupeElement.value || null)
    const device = devices.value.find(item => item.id === id)
    if (device) loadForm(device)
    else mode.value = 'edit'
    fps.value = 0; delay.value = 0; bitrate.value = '—'
    if (sessions[id]?.connected.value) onTargetConnected(id)
    return true
  },
}
async function selectTarget(id) {
  const ok = await workspace.selectTarget(id, focusOptions)
  if (ok) closeTargetMenu()
  return ok
}
async function addAndSelectTarget(id) {
  if (!id) return selectTarget(null)
  const added = !workspace.state.targetIds.includes(id)
  if (added && !workspace.addTarget(id)) { toast('最多同时保留 9 个目标，请先移出一个画面', 'warn'); return }
  if (!isSingleScreen.value && !workspace.visibleTargetIds.includes(id)) {
    await workspace.setGridSize(9, focusOptions)
    toast('已切换到 9 格，显示新增目标', 'info')
  }
  await selectTarget(id)
}
async function changeGridSize(size, select) {
  if (await workspace.setGridSize(size, focusOptions)) closeTargetMenu()
  if (select) select.value = workspace.state.gridSize
}
async function toggleExpanded(id) {
  if (await selectTarget(id)) {
    await changeGridSize(1)
    await nextTick()
    stageFocusEl.value?.focus({ preventScroll: true })
  }
}
function closeTargetPreview(id) {
  if (id === store.deviceId) releaseSelectedInput()
  sessions[id]?.close()
}
function closeSelectedPreview() { closeTargetPreview(store.deviceId) }
async function removeTargetCell(id) {
  const running = !!workspace.getRun(id)
  if (!await workspace.removeTarget(id, focusOptions)) return
  if (targetMenuOpen.value === id) closeTargetMenu()
  if (running) toast(`${targetName(id)} 的脚本仍在后台运行，可重新添加此目标查看或停止`, 'info')
}
function reportBatch(results) {
  const labels = { started: '已启动', stopping: '正在停止', skipped: '跳过', error: '失败' }
  batchResult.value = results.length ? results.map(result => `${targetName(result.targetId)}：${labels[result.status] || result.status}${result.reason ? '（' + result.reason + '）' : ''}`).join('；') : '当前网格没有运行中的脚本'
}
async function startOneTarget(id) { reportBatch([await workspace.startTarget(id)]) }
async function startVisibleTargets() {
  if (isSingleScreen.value) return
  batchBusy.value = true
  try { reportBatch(await workspace.startVisible()) } finally { batchBusy.value = false }
}
async function confirmStops(snapshot, { batch = false } = {}) {
  if (!snapshot.length) { reportBatch([]); return }
  const names = snapshot.map(item => targetName(item.targetId)).join('、')
  if (!await confirmDialog(`停止 ${names} 的当前脚本？预览和应用将保留。`, { title: '停止脚本', confirmText: '停止脚本' })) return
  if (batch && isSingleScreen.value) { toast('请先切换为 4 格或 9 格，再批量停止脚本', 'info'); return }
  batchBusy.value = true
  try { reportBatch(await workspace.stopCaptured(snapshot)) } finally { batchBusy.value = false }
}
async function stopVisibleTargets() {
  if (isSingleScreen.value) return
  await confirmStops(workspace.captureVisibleStops(), { batch: true })
}
async function stopOneTarget(id) { await confirmStops(workspace.captureVisibleStops().filter(item => item.targetId === id)) }
watch(connected, value => {
  if (value) { startStats(); startLogPolling(); videoConnectTs = Date.now(); resetBlackWatchdog() }
  else { stopStats(); stopLogPolling(); resetWatchdogs() }
})

// ---------- 生命周期 ----------

watch(connected, value => { if (value && store.deviceId) sessionStorage.setItem('gamer-update-device', store.deviceId) })
function saveUpdateConnection() {
  if (connected.value && store.deviceId) sessionStorage.setItem('gamer-update-device', store.deviceId)
}
watch([manualClose, superseded], ([manual, replaced]) => {
  if (manual || replaced) sessionStorage.removeItem('gamer-update-device')
})
function restoreUpdateConnection() {
  const id = sessionStorage.getItem('gamer-update-device')
  if (!manualClose.value && !superseded.value && id && id === store.deviceId && !connected.value && !connecting.value) {
    sessionStorage.removeItem('gamer-update-device')
    connect(false)
  }
}
onMounted(async () => {
  document.addEventListener('visibilitychange', onVisibilityChange)
  pageVisible.value = !document.hidden
  navigationReady.value = !!document.getElementById('gamer-main-navigation')
  // SPA 内跳转（store 存活）→ 自动重连恢复画面；页面刷新 → localStorage 恢复设备选择；
  // 首次进入仅选中第一台设备，等待用户点连接（不主动建会话，尊重空闲低功耗）
  const spaPreselected = !!store.deviceId
  await loadData()
  if (consoleDisposed) return
  await loadPackages().catch(() => {})
  if (consoleDisposed) return
  await refreshServerExtensions()
  if (consoleDisposed) return
  startExtensionPolling()
  if (!store.deviceId) {
    const saved = localStorage.getItem('gb_device_id')
    store.deviceId = (saved && devices.value.find(d => d.id === saved)) ? saved : (devices.value[0]?.id || null)
  }
  const legacyId = store.deviceId
  if (!workspace.state.targetIds.length && legacyId) {
    workspace.addTarget(legacyId)
    workspace.updateConfig(legacyId, { packageId: currentPackageId.value || '', scriptId: '' })
  }
  const initialTarget = workspace.state.selectedTargetId || workspace.visibleTargetIds[0] || null
  // Restored layout may already mark this target selected; explicitly project once.
  if (initialTarget && projectTargetConfig(initialTarget)) {
    store.deviceId = initialTarget
    projectDeviceRun(initialTarget)
    workspace.state.selectedTargetId = initialTarget
  } else if (initialTarget) {
    // Missing package still allows monitoring; sidebar stays empty until reconfigured.
    store.deviceId = initialTarget
  }
  workspace.startPolling()
  await nextTick()
  if (consoleDisposed) return
  const d = current.value
  if (d) loadForm(d)
  else { mode.value = 'edit'; store.deviceId = null }
  window.addEventListener('keydown', onGlobalKeydown)
  window.addEventListener('beforeunload', onBeforeUnload)
  window.addEventListener('gamer-before-update-reload', onBeforeUnload)
  window.addEventListener('gamer-update-reload', saveUpdateConnection)
  window.addEventListener('gamer-service-restored', restoreUpdateConnection)

  // 刷新恢复运行态：刷新前发起的脚本在服务端继续执行——按设备查询当前活动 run。
  // 当前契约为 active:false 或 active:true + 嵌套完整 RunRecord，含来源标签；
  // 恢复运行状态/选中脚本/状态轮询与日志（不依赖投屏连接是否恢复成功）
  if (store.deviceId) await restoreRunState()
  if (consoleDisposed) return
  // 画面恢复：SPA 内返回（store 存活）或刷新后脚本运行中/设备会话在线（此前正在
  // 投屏）→ 自动连接；设备空闲离线则保持首次进入行为；遇 conflict 不抢（connect 内处理）
  if (store.deviceId && (spaPreselected || store.running || current.value?.status === 'online' || sessionStorage.getItem('gamer-update-device') === store.deviceId)) connect(false)
  for (const id of workspace.state.targetIds) {
    if (id !== store.deviceId && devices.value.find(device => device.id === id)?.status === 'online') sessions[id]?.connect(false)
  }
  sessionStorage.removeItem('gamer-update-device')
  // 其他页面已启动脚本时，本页接管状态轮询（脚本结束后复位运行状态）
  if (store.running && store.runId) startRunStatusPoll()
  window.addEventListener('blur', onWindowBlur)
})

onUnmounted(() => {
  consoleDisposed = true
  window.removeEventListener('keydown', onGlobalKeydown)
  window.removeEventListener('beforeunload', onBeforeUnload)
  window.removeEventListener('gamer-before-update-reload', onBeforeUnload)
  window.removeEventListener('gamer-update-reload', saveUpdateConnection)
  window.removeEventListener('gamer-service-restored', restoreUpdateConnection)
  window.removeEventListener('blur', onWindowBlur)
  document.removeEventListener('visibilitychange', onVisibilityChange)
  keymap.releaseAll()
  syncKeymapPressed()
  keyboard.releaseAll()
  consoleRuntime.cancelReconnect()
  if (appHintTimer) { clearTimeout(appHintTimer); appHintTimer = null }
  if (fxTapTimer) { clearTimeout(fxTapTimer); fxTapTimer = null }
  if (fxSwipeTimer) { clearTimeout(fxSwipeTimer); fxSwipeTimer = null }
  if (fxHitTimer) { clearTimeout(fxHitTimer); fxHitTimer = null }
  for (const session of Object.values(sessions)) session.close()
  workspace.stopPolling()
})
</script>

<style scoped>
.panel-target-context { flex:none; display:flex; flex-direction:column; gap:3px; padding:10px 12px; border-bottom:1px solid var(--border); border-left:1px solid var(--accent); background:var(--bg-1); }
.panel-target-context strong { color:var(--accent); font-size:13px; line-height:1.5; overflow-wrap:anywhere; }
.panel-target-context span { color:var(--text-2); font-size:12px; line-height:1.4; }

.tb-display-mode { display:flex; align-items:center; gap:5px; padding-left:8px; border-left:1px solid var(--border); }
.tb-display-mode .select { width:74px; min-width:74px; height:28px; font-size:12px; }
.device-run-actions { display:flex; align-items:center; gap:5px; }
.tb-hidden-targets { font-size:11px; color:var(--text-2); white-space:nowrap; }
.multiview-result { flex:none; max-height:70px; overflow:auto; padding:6px 10px; font-size:12px; color:var(--text-1); background:var(--bg-2); }
.multiview-grid { flex:1; min-height:0; overflow:auto; display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); grid-auto-rows:minmax(180px,1fr); gap:7px; padding:7px; }
.multiview-grid.nine { grid-template-columns:repeat(3,minmax(0,1fr)); grid-auto-rows:minmax(160px,1fr); }
.multiview-grid.single { grid-template-columns:minmax(0,1fr); grid-template-rows:minmax(0,1fr); grid-auto-rows:minmax(0,1fr); padding:0; gap:0; }
.target-cell { min-width:0; min-height:0; display:flex; flex-direction:column; border:2px solid var(--border); border-radius:7px; overflow:hidden; background:var(--bg-1); }
.target-cell.selected { border-color:var(--accent); box-shadow:0 0 0 1px var(--accent); }
.multiview-grid.single .target-cell { border:0; border-radius:0; box-shadow:none; }
.target-heading { display:flex; align-items:center; gap:4px; padding:5px; border-bottom:1px solid var(--border); }
.target-heading .btn { padding:2px 5px; font-size:11px; }
.target-name { flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--text-0); border:0; background:none; text-align:left; cursor:pointer; }
.target-cell.selected .target-name { color:var(--accent); }
.target-status { font-size:11px; color:var(--text-2); }
.target-heading .btn-icon { flex:none; min-width:23px; }
.target-heading .ui-icon { width:13px; height:13px; }
.target-error { padding:4px 7px; color:var(--danger); font-size:11px; }
.target-surface { position:relative; display:flex; flex:1; min-height:110px; min-width:0; }
.target-surface :deep(.video-wrap) { min-height:110px; }
.target-surface :deep(.player-stage) { min-width:0; width:100%; }
.target-select-shield { position:absolute; inset:0; z-index:9; border:0; background:transparent; color:white; cursor:pointer; }
.target-select-shield span { position:absolute; bottom:8px; left:50%; transform:translateX(-50%); background:#0009; border-radius:4px; padding:4px 8px; font-size:11px; }
.target-empty { min-height:140px; display:flex; align-items:center; justify-content:center; flex-direction:column; gap:10px; padding:12px; border:1px dashed var(--border); color:var(--text-2); font-size:12px; }
.target-empty .select { max-width:100%; }
@media(max-width:1100px) { .multiview-grid.nine { grid-template-columns:repeat(2,minmax(0,1fr)); } }
@media(max-width:1400px) { .target-status { display:none; } }
@media(max-width:600px) { .multiview-grid,.multiview-grid.nine { grid-template-columns:minmax(0,1fr); grid-auto-rows:minmax(230px,1fr); } }
.multiview-grid.single { grid-auto-rows:minmax(0,1fr); }

.tb-operation-row.tb-browser-row { flex-wrap: wrap; }
.tb-browser-group { flex: 1 1 230px; min-width: min(230px, 100%); }
.tb-page-select { field-sizing: fixed; flex: 1; width: 100px; min-width: 0; max-width: 260px; height: 28px; font-size: 13px; white-space: nowrap; text-overflow: ellipsis; }
.tb-browser-row .tb-control-group { flex: 0 1 auto; flex-wrap: wrap; }
.console {
  display: flex; height: 100%; padding: 14px; gap: 14px;
}

/* ===== 画面区 ===== */
.stage {
  flex: 1; display: flex; flex-direction: column; min-width: 0; min-height: 0;
  position: relative; overflow: hidden;
  border: 1px solid var(--border); border-radius: var(--radius); background: var(--bg-1);
  outline: none;
}
.app-hint {
  position: absolute; top: 54px; left: 50%; transform: translateX(-50%); z-index: 6;
  display: flex; align-items: center; gap: 8px; padding: 5px 10px; white-space: nowrap;
  background: rgba(4, 6, 10, .85); border: 1px solid var(--border); border-radius: var(--radius-sm);
  font-size: 12px; color: var(--text-1); backdrop-filter: blur(2px);
}
.input-control-hint { flex:none; padding:7px 10px; border-bottom:1px solid var(--border); background:var(--bg-2); color:var(--accent); font-size:12px; line-height:1.5; }
.input-control-hint.paused { color:#77cbb4; }

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
.crop-hint { font-size: 12px; color: var(--text-2); align-self: flex-start; }
.crop-panel {
  display: flex; flex-direction: column; gap: 10px;
  border-top: 1px solid var(--border); padding-top: 12px;
}
.crop-actions { display: flex; gap: 8px; }
.crop-actions .btn-primary { margin-left: auto; }

/* 两行分别配置上下文与执行操作；窄面板按整组换行，不拆散对象与按钮。 */
.toolbar {
  display:flex; flex-direction:column; align-items:stretch; flex:none;
  background:var(--bg-1); border-bottom:1px solid var(--border);
}
.tb-row { display:flex; align-items:center; flex-wrap:wrap; gap:8px 16px; min-width:0; min-height:40px; padding:6px 10px; }
.tb-row + .tb-row { border-top:1px solid var(--border); }
.tb-group { display:flex; align-items:center; gap:6px; min-width:0; }
.tb-label, .stage-package :deep(.pkg-bar-label) {
  flex:none; align-self:center; font-size:12px; line-height:1; color:var(--text-2);
  writing-mode:vertical-rl; text-orientation:upright; white-space:nowrap;
  letter-spacing:2px; padding-top:2px;
}
.tb-device-group, .tb-app-group { flex:0 1 auto; flex-wrap:wrap; max-width:100%; }
.tb-control-group { flex:0 1 auto; flex-wrap:wrap; justify-content:flex-end; max-width:100%; margin-left:auto; }
.tb-row .btn { flex:none; height:28px; min-width:28px; justify-content:center; padding:3px 7px; }
.tb-dev-select, .tb-app-select { field-sizing:content; flex:0 1 auto; min-width:0; max-width:100%; width:auto; font-size:13px; }
.toolbar .stage-package { margin-left:auto; flex:0 1 auto; min-width:0; max-width:100%; }
.tb-more-wrap { position:relative; display:inline-flex; flex:none; }
.keyboard-mode-btn.active { color:var(--accent-2); }
.tb-more-mask { position: fixed; inset: 0; z-index: 20; }
.tb-more-dropdown-fixed { position: fixed; z-index: 30; }
.btn.active { border-color: var(--accent-2); color: var(--accent-2); }
/* ===== 左右分区与右侧面板 ===== */
.console.is-panel-resizing,
.console.is-panel-resizing * { cursor: col-resize !important; user-select: none !important; }
.panel-resizer {
  position: relative; z-index: 5; flex: 0 0 8px; width: 8px; margin: 0 -11px;
  cursor: col-resize; touch-action: none; outline: none;
}
.panel-resizer::before {
  content: ''; position: absolute; inset: 0 3px; border-radius: 4px; background: transparent;
  transition: background .15s ease;
}
.panel-resizer:hover::before,
.panel-resizer:focus-visible::before,
.panel-resizer.active::before { background: var(--accent); }
.panel {
  width: 340px; flex-shrink: 0; display: flex; flex-direction: column; gap: 10px;
  overflow: hidden;
}

.panel-sec {
  background: var(--bg-1); border: 1px solid var(--border);
  border-radius: var(--radius); padding: 14px; display: flex; flex-direction: column; gap: 10px;
  flex-shrink: 0;
}
.mono { font-family: var(--mono); font-size: 12px; color: var(--text-1); }

.auto-run { display: flex; flex-wrap: wrap; gap: 8px; }
.auto-run .spicker { flex: 1 1 auto; }
.auto-run .select { flex: 1; min-width: 120px; }
.run-actions { display: flex; gap: 8px; }
.run-actions .btn { flex: 1; }
.run-actions .more-wrap { position: relative; flex: 1; }
.run-actions .more-wrap .btn { width: 100%; }
.more-mask { position: fixed; inset: 0; z-index: 20; }

/* 脚本页签 */
.panel-sec.script-tab { flex: 1; min-height: 0; overflow: hidden; }
.panel-sec.tpl-tab { flex: 1; min-height: 0; overflow: hidden; }
.panel-sec.extra-tab { flex: 1; min-height: 0; overflow: hidden; display: flex; flex-direction: column; }
.func-pkg-row { display: flex; align-items: center; gap: 6px; flex-shrink: 0; }
.func-pkg-row .func-pkg { flex: 1; min-width: 0; font-size: 12px; }
.func-pkg-row .btn { flex: none; }
.func-tabs { display: flex; flex-shrink: 0; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow: hidden; background: var(--bg-2); }
.func-tabs button {
  flex: 1; padding: 7px 0; font-size: 12px; text-align: center; cursor: pointer;
  border: none; background: transparent; color: var(--text-1);
}
.func-tabs button + button { border-left: 1px solid var(--border); }
.func-tabs button.active {
  color: var(--accent); background: color-mix(in srgb, var(--accent) 14%, transparent); font-weight: 600;
}
/* 包名下拉：模板/脚本两页签共用，数据随当前包名切换 */
.script-tpl { flex: 4; min-height: 0; display: flex; flex-direction: column; gap: 8px; border-bottom: 1px solid var(--border); padding-bottom: 10px; }
.tpl-top { display: flex; align-items: center; gap: 8px; }
/* 阈值输入 : 区域下拉 : 搜索框 : 框选按钮 : 上传按钮 = 2:4:5:3:3 */
.tpl-top .input { flex: 2 1 0%; min-width: 0; }
.tpl-top .tpl-region { flex: 4 1 0%; min-width: 0; padding: 4px 6px; font-size: 12px; }
.tpl-top .tpl-search { flex: 5 1 0%; min-width: 0; font-size: 12px; }
.tpl-top .btn { flex: 3 1 0%; min-width: 0; }
.tpl-tools { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
.script-run { flex: 6; display: flex; flex-direction: column; gap: 10px; min-height: 0; }
.script-logs { flex: 1; min-height: 120px; max-height: none; }
.run-hint { font-size: 12px; color: var(--text-2); flex-shrink: 0; }
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
  background: color-mix(in srgb, var(--accent) 12%, transparent); color: var(--accent);
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
.edit-actions .btn.active { border-color: var(--accent-2); color: var(--accent-2); background: color-mix(in srgb, var(--accent) 8%, transparent); }
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
.rp-fill { height: 100%; background: linear-gradient(90deg, var(--accent), var(--accent-2)); border-radius: 3px; }
.rp-step { font-size: 12px; color: var(--text-1); }

.live-logs {
  max-height: 180px; overflow: auto; background: var(--bg-0);
  border: 1px solid var(--border); border-radius: var(--radius-sm);
  padding: 8px; display: flex; flex-direction: column; gap: 3px;
}
.live-logs.script-logs { max-height: none; }
.ll { display: flex; gap: 8px; font-size: 12px; line-height: 1.5; }
.ll-time { color: var(--text-2); flex-shrink: 0; }
.ll.info .ll-msg { color: var(--text-1); }
.ll.success .ll-msg { color: var(--ok); }
.ll.warn .ll-msg { color: var(--warn); }
.ll.error .ll-msg { color: var(--danger); }

/* 模板文件列表 */
.tpl-list-wrap { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 4px; }
.tpl-list-head, .tpl-row { display: flex; align-items: center; gap: 8px; padding: 3px 8px; }
.tpl-list-head { font-size: 12px; color: var(--text-2); border-bottom: 1px solid var(--border); flex-shrink: 0; }
.tpl-list { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 2px; min-height: 0; }
.tpl-row {
  cursor: pointer; border-radius: var(--radius-sm); border: 1px solid transparent;
  transition: background .15s;
}
.tpl-row:hover { background: var(--bg-3); }
.tpl-row.del-confirm { background: rgba(248,113,113,.08); border-color: rgba(248,113,113,.35); }
.tpl-row.renaming { background: color-mix(in srgb, var(--accent) 8%, transparent); border-color: color-mix(in srgb, var(--accent) 35%, transparent); }
.tpl-empty { padding: 16px 8px; text-align: center; font-size: 12px; color: var(--text-2); }
.tpl-cell.thumb { width: 40px; flex-shrink: 0; display: flex; align-items: center; }
.tpl-list-head .tpl-cell.thumb { white-space: nowrap; }
.tpl-cell.name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; color: var(--text-0); }
.tpl-cell.ops { display: flex; gap: 6px; flex-shrink: 0; }
.tpl-cell.ops .btn { padding: 2px 8px; font-size: 12px; }
.rename-input { width: 100%; min-width: 0; padding: 2px 6px; font-size: 12px; }
.tpl-region-badge {
  display: inline-block; margin-left: 6px; padding: 0 5px; border-radius: 4px;
  background: var(--bg-3); border: 1px solid var(--border);
  color: var(--accent); font-size: 12px; line-height: 16px; vertical-align: 1px;
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
.console { flex-direction:column; padding:0; gap:0; background:var(--bg-0); }
.console-body { flex:1; min-height:0; display:flex; padding:6px 8px; gap:0; }
.stage { border:1px solid var(--border); border-radius:var(--radius-sm); background:var(--bg-0); }
.panel { padding:0; border:1px solid var(--border); border-radius:var(--radius-sm); min-width:0; max-width:none; background:var(--bg-1); display:flex; flex-direction:column; gap:0; flex:none; }
.panel-resizer { flex:0 0 10px; width:10px; margin:0; border:0; background:transparent; }
.panel-resizer::before { inset:auto; top:calc(50% - 28px); left:-4px; width:18px; height:56px; border-radius:4px; border:1px solid var(--control-border); background:var(--chrome); }
.panel-resizer :deep(.ui-icon) { position:absolute; z-index:1; pointer-events:none; top:50%; left:50%; transform:translate(-50%,-50%); width:14px; height:24px; color:var(--accent); }
.panel-resizer:hover::before,.panel-resizer.active::before,.panel-resizer:focus-visible::before { border-color:var(--accent); background:var(--bg-2); }
.sr-context { position:absolute; width:1px; height:1px; overflow:hidden; clip-path:inset(50%); white-space:nowrap; }
.toolbar { background:var(--chrome); }
.tb-row { min-height:46px; padding:5px 8px; gap:6px 10px; }
.tb-label,.stage-package :deep(.pkg-bar-label) { writing-mode:horizontal-tb; text-orientation:mixed; letter-spacing:0; padding:0; font-size:12px; }
.tb-row .btn { height:34px; min-width:34px; padding:5px 8px; }
.toolbar .select { font-size:14px; }.tb-label,.stage-package :deep(.pkg-bar-label) { font-size:13px; }
.tb-dev-select { field-sizing:fixed; width:250px; height:34px; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
.tb-display-mode { display:flex; align-items:center; gap:6px; }
.tb-display-mode .select { height:34px; }
.toolbar .stage-package { margin-left:0; flex:0 1 330px; min-width:min(260px, 100%); max-width:330px; }
.stage-package :deep(.func-pkg-row) { gap:6px; }
.stage-package :deep(.select) { height:34px; max-width:210px; }
.stage-package :deep(.btn) { min-width:34px; height:34px; }
.stage-package :deep(.btn[aria-label="配置包详情"]) { width:34px; font-size:0; gap:0; padding:0; justify-content:center; }
.toolbar-actions { display:flex; align-items:center; gap:6px; margin-left:auto; }
.app-hint { top:8px; }
.global-page .console-body { padding:0; overflow:hidden; }.global-page .panel { border:0; }
.panel-expanded .panel { flex:1; }
@media(max-width:1000px) { .tb-dev-select { width:180px; }.toolbar .stage-package { max-width:280px; } }
@media(max-width:800px) {
  .console-body { flex-direction:column; overflow:auto; padding:4px; gap:6px; }
  .stage { flex:none; height:38vh; min-height:230px; }
  .panel { width:100%!important; min-height:460px; flex:1; }
  .panel-resizer { display:none; }
  .panel-expanded .panel { min-height:0; height:100%; }
  .global-page .panel { min-height:0; }.global-page .console-body { overflow:hidden; }
  .tb-device-group { flex:1 1 100%; }.tb-dev-select { flex:1; width:160px; min-width:160px; white-space:nowrap; overflow:hidden; }
  .toolbar .stage-package { flex:1 1 280px; min-width:min(280px, 100%); max-width:100%; }
  .stage-package :deep(.func-pkg-row) { flex-wrap:nowrap; }
  .stage-package :deep(.select) { flex:1; max-width:none; }
  .stage-package :deep(.func-pkg) { min-width:130px; white-space:nowrap; overflow:hidden; }
  .tb-row .btn { min-height:36px; }.tb-context-row { gap:6px; }
  .panel-collapsed .stage { flex:1; height:auto; }.panel-collapsed .console-body,.panel-expanded .console-body { overflow:hidden; }
}
@media(pointer:coarse) { .tb-row .btn,.tb-row .select { min-height:44px; } }
</style>
