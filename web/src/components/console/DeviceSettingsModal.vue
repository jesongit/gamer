<template>
  <div v-if="ctx.settingsOpen" class="modal-mask" @click.self="ctx.cancelSettings">
    <div class="modal dev-modal">
      <div class="modal-head">
        <span class="title">{{ ctx.mode === 'add' ? '新增设备' : '设备设置' }}</span>
        <span v-if="ctx.mode === 'edit' && ctx.formDirty" class="head-sub">有未保存的修改</span>
        <button class="btn btn-ghost btn-sm" @click="ctx.cancelSettings">✕</button>
      </div>
      <div class="modal-body">
        <ConsoleDeviceSummary v-if="ctx.mode === 'edit' && ctx.current" :device="ctx.current" :connected="ctx.connected" :kind-icon="ctx.kindInfo(ctx.current.kind).icon" :kind-label="ctx.kindInfo(ctx.current.kind).label" :screen-summary="ctx.screenSummary" />
        <template v-if="ctx.mode === 'add'">
          <div class="form-item">
            <label>接入方式</label>
            <div class="type-picker">
              <div v-for="t in ctx.types" :key="t.key" class="type-opt" :class="{ sel: ctx.form.kind === t.key }" @click="ctx.form.kind = t.key"><span class="type-icon">{{ t.icon }}</span><span>{{ t.label }}</span></div>
            </div>
          </div>
          <div class="form-item"><label>ADB 地址 <span class="muted">（redroid / 无线 adb / 模拟器需要填写）</span></label><input v-model="ctx.form.addr" class="input mono" placeholder="redroid:5555 或 192.168.1.88:5555" /></div>
        </template>
        <div class="form-item"><label>设备名称</label><input v-model="ctx.form.name" class="input" placeholder="例如：红米 Note12 挂机号" /></div>
        <div class="form-item">
          <label>屏幕模式</label>
          <div class="mode-picker">
            <div class="mode-opt" :class="{ sel: ctx.form.screen_mode === 'mirror' }" @click="ctx.form.screen_mode = 'mirror'"><div class="mode-title">🖥️ 镜像主屏</div><div class="mode-desc">投屏设备物理屏幕，各设备分辨率不同</div></div>
            <div class="mode-opt" :class="{ sel: ctx.form.screen_mode === 'virtual' }" @click="ctx.form.screen_mode = 'virtual'"><div class="mode-title">🖥️ 虚拟屏</div><div class="mode-desc">统一分辨率虚拟屏幕，模板跨设备通用</div></div>
          </div>
        </div>
        <DeviceVirtualFields v-if="ctx.form.screen_mode === 'virtual'" :ctx="ctx" />
        <div class="cfg-hint">{{ ctx.mode === 'add' ? '填写信息后确认添加，新设备会出现在工具条设备下拉中' : (ctx.formDirty ? '保存后生效：投屏参数（屏幕/分辨率/DPI/帧率）变更会自动重连，仅改名称不断开投屏' : '投屏参数（屏幕/分辨率/DPI/帧率）变更保存后自动重连，仅改名称不断开投屏') }}</div>
      </div>
      <div class="modal-foot">
        <button class="btn" @click="ctx.cancelSettings">取消</button>
        <button class="btn btn-primary" :disabled="ctx.configApplying" @click="ctx.saveSettings">{{ ctx.configApplying ? (ctx.mode === 'add' ? '添加中…' : '保存中…') : (ctx.mode === 'add' ? '确认添加' : '💾 保存') }}</button>
      </div>
    </div>
  </div>
</template>
<script setup>
import { reactive } from 'vue'
import ConsoleDeviceSummary from '../ConsoleDeviceSummary.vue'
import DeviceVirtualFields from './DeviceVirtualFields.vue'
const props = defineProps({ context: { type: Object, required: true } })
const ctx = reactive(props.context)
</script>
<style scoped>
.dev-modal { width: 460px; }
.head-sub { flex: 1; text-align: right; margin-right: 10px; font-size: 11px; color: var(--warn); }
.cfg-hint { font-size: 11px; color: var(--text-2); line-height: 1.5; }
.muted { color: var(--text-2); font-weight: 400; }
.type-picker { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px; }
.type-opt { display: flex; flex-direction: column; align-items: center; gap: 5px; padding: 10px 4px; border-radius: var(--radius-sm); border: 1px solid var(--border); cursor: pointer; font-size: 11px; color: var(--text-1); transition: all .15s; text-align: center; }
.type-opt:hover { border-color: #33405e; }
.type-opt.sel { border-color: var(--accent); color: var(--accent); background: rgba(34,211,165,.06); }
.type-icon { font-size: 18px; }
.mode-picker { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.mode-opt { padding: 10px; border-radius: var(--radius-sm); border: 1px solid var(--border); cursor: pointer; transition: all .15s; display: flex; flex-direction: column; gap: 3px; }
.mode-opt:hover { border-color: #33405e; }
.mode-opt.sel { border-color: var(--accent); background: rgba(34,211,165,.06); }
.mode-title { font-size: 12px; font-weight: 600; }
.mode-desc { font-size: 10px; color: var(--text-2); }
.mono { font-family: var(--mono); font-size: 11px; color: var(--text-1); }
</style>
