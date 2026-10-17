<template>
  <div v-if="ctx.settingsOpen" class="modal-mask" v-backdrop-dismiss="ctx.cancelSettings">
    <form class="modal dev-modal" role="dialog" aria-modal="true" aria-label="设备设置" @submit.prevent="ctx.saveSettings">
      <div class="modal-head">
        <span class="title">{{ ctx.mode === 'add' ? '新增设备' : '设备设置' }}</span>
        <span v-if="ctx.mode === 'edit' && ctx.formDirty" class="head-sub">有未保存的修改</span>
        <button type="button" class="btn btn-ghost btn-sm btn-icon" aria-label="关闭设备设置" :disabled="ctx.configApplying" @click="ctx.cancelSettings"><UiIcon name="close" /></button>
      </div>
      <fieldset class="modal-body" :disabled="ctx.configApplying">
        <ConsoleDeviceSummary v-if="ctx.mode === 'edit' && ctx.current" :device="ctx.current" :connected="ctx.connected" :screen-summary="ctx.screenSummary" />
        <p class="cfg-hint">Android 设备通过 ADB 接入。USB 设备可先用顶部刷新扫描；手动添加时填写地址或完整序列号。</p>
        <label class="form-item">设备名称<input v-model="ctx.form.name" class="input" aria-label="设备名称" required placeholder="例如：我的手机 / 模拟器账号一" :disabled="ctx.configApplying" /></label>
        <label class="form-item">ADB 地址或序列号<input v-model="ctx.form.addr" class="input mono" aria-label="ADB 地址或序列号" required placeholder="192.168.1.88:5555 或 emulator-5554" :disabled="ctx.configApplying" /><span class="muted">网络设备填写主机与端口，USB 或模拟器填写扫描到的序列号。</span></label>
        <div class="form-item">
          <label>屏幕模式</label>
          <div class="mode-picker">
            <label class="mode-opt" :class="{ sel: ctx.form.screen_mode === 'mirror' }"><input type="radio" name="screen-mode" v-model="ctx.form.screen_mode" value="mirror" :disabled="ctx.configApplying" /><span class="mode-title">镜像主屏</span><span class="mode-desc">查看设备当前屏幕，画面尺寸随设备变化。</span></label>
            <label class="mode-opt" :class="{ sel: ctx.form.screen_mode === 'virtual' }"><input type="radio" name="screen-mode" v-model="ctx.form.screen_mode" value="virtual" :disabled="ctx.configApplying" /><span class="mode-title">独立虚拟屏</span><span class="mode-desc">使用单独的游戏画面，可设置尺寸；需设备与游戏支持。</span></label>
          </div>
        </div>
        <DeviceVirtualFields :ctx="ctx" />
        <div class="cfg-hint">{{ ctx.mode === 'add' ? '添加后会选中新设备，点击连接开始投屏。' : '地址、屏幕模式、分辨率、DPI 或帧率变更后，已连接的投屏会重连；任务运行期间需等待任务结束后重新连接。仅改名称不断开投屏。' }}</div>
        <p v-if="ctx.formError" class="form-error" role="alert">{{ ctx.formError }}</p>
      </fieldset>
      <div class="modal-foot">
        <button type="button" class="btn" :disabled="ctx.configApplying" @click="ctx.cancelSettings">取消</button>
        <button type="submit" class="btn btn-primary" :disabled="ctx.configApplying">{{ ctx.configApplying ? (ctx.mode === 'add' ? '添加中…' : '保存中…') : (ctx.mode === 'add' ? '添加设备' : '保存设置') }}</button>
      </div>
    </form>
  </div>
</template>
<script setup>
import { vBackdropDismiss } from '../../../../plugins/ui-shared/backdrop-dismiss.js'
import { reactive } from 'vue'
import ConsoleDeviceSummary from '../ConsoleDeviceSummary.vue'
import DeviceVirtualFields from './DeviceVirtualFields.vue'
import UiIcon from '../ui/UiIcon.vue'
const props = defineProps({ context: { type: Object, required: true } })
const ctx = reactive(props.context)
</script>
<style scoped>
.dev-modal { width:min(520px,calc(100vw - 24px)); max-height:calc(100dvh - 24px); display:flex; flex-direction:column; background:var(--bg-1); color:var(--text-0); border:1px solid var(--border); border-radius:var(--radius); }
.modal-head,.modal-foot{display:flex;align-items:center;gap:10px;padding:12px 16px;flex:none}.modal-head{border-bottom:1px solid var(--border)}.title{font-size:17px;font-weight:600}.modal-head .btn{margin-left:auto}.modal-body{display:flex;flex-direction:column;gap:14px;overflow:auto;min-height:0;padding:16px}.modal-foot{justify-content:flex-end;border-top:1px solid var(--border)}.form-item{display:flex;flex-direction:column;gap:6px;font-size:13px}.form-item .input{width:100%}.form-error{font-size:13px;line-height:1.6;color:var(--danger)}
.modal-body{border:0;margin:0;min-width:0}
.head-sub { flex: 1; text-align: right; margin-right: 10px; font-size: 12px; color: var(--warn); }
.cfg-hint { font-size: 12px; color: var(--text-2); line-height: 1.5; }
.muted { color: var(--text-2); font-weight: 400; }
.mode-picker { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.mode-opt { padding: 10px; border-radius: var(--radius-sm); border: 1px solid var(--border); cursor: pointer; transition: all .15s; display: flex; flex-direction: column; gap: 3px; }
.mode-opt:hover { border-color: var(--accent); }
.mode-opt input{align-self:flex-start;accent-color:var(--accent)}.mode-opt:focus-within{outline:2px solid var(--accent);outline-offset:2px}
.mode-opt.sel { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 6%, transparent); }
.mode-title { font-size: 12px; font-weight: 600; }
.mode-desc { font-size: 12px; color: var(--text-2); }
.mono { font-family: var(--mono); font-size: 12px; color: var(--text-1); }
</style>
