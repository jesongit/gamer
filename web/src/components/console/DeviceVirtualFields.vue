<template>
    <div class="form-item"><label>虚拟屏分辨率</label><div class="vd-presets"><div v-for="p in ctx.vdPresets" :key="p.res" class="vd-opt" :class="{ sel: ctx.form.vd_res === p.res && String(ctx.form.vd_dpi) === String(p.dpi) }" @click="ctx.form.vd_res = p.res; ctx.form.vd_dpi = p.dpi"><span class="vd-res mono">{{ p.res }}</span><span class="vd-dpi">@{{ p.dpi }}dpi</span></div></div></div>
    <div class="form-row"><div class="form-item"><label>宽 × 高</label><input v-model="ctx.form.vd_res" class="input mono" placeholder="1920x1080" /></div><div class="form-item"><label>DPI <span class="muted">（0=自动）</span></label><div class="dpi-box"><input v-model.number="ctx.form.vd_dpi" class="input mono" type="number" placeholder="0" /><button class="btn btn-sm" :class="{ active: !ctx.form.vd_dpi }" @click="ctx.form.vd_dpi = 0">自动</button></div></div></div>
    <div class="form-item"><label>视频帧率 <span class="muted">（scrcpy 帧率上限：越高越流畅、越耗性能）</span></label><div class="fps-presets"><div v-for="f in ctx.fpsPresets" :key="f" class="fps-opt mono" :class="{ sel: ctx.form.fps === f }" @click="ctx.form.fps = f">{{ f }}</div></div></div>
</template>
<script setup>
import { reactive } from 'vue'
const props = defineProps({ ctx: { type: Object, required: true } })
const ctx = reactive(props.ctx)
</script>
<style scoped>
.form-row{display:grid;grid-template-columns:1fr 1fr;gap:8px}.form-row .form-item{min-width:0}.dpi-box{display:flex;gap:6px}.dpi-box .input{flex:1;min-width:0}.dpi-box .btn{flex-shrink:0}.dpi-box .btn.active{border-color:var(--accent-2);color:var(--accent-2)}.muted{color:var(--text-2);font-weight:400}.vd-presets{display:grid;grid-template-columns:repeat(4,1fr);gap:6px}.vd-opt{display:flex;flex-direction:column;align-items:center;gap:2px;padding:8px 2px;border-radius:var(--radius-sm);border:1px solid var(--border);cursor:pointer}.vd-opt.sel{border-color:var(--accent-2);background:rgba(56,189,248,.06)}.vd-res{font-size:11px;color:var(--text-0)}.vd-dpi{font-size:10px;color:var(--text-2)}.fps-presets{display:grid;grid-template-columns:repeat(4,1fr);gap:6px}.fps-opt{text-align:center;padding:8px 4px;border-radius:var(--radius-sm);border:1px solid var(--border);cursor:pointer;font-size:12px}.fps-opt.sel{border-color:var(--accent-2);color:var(--accent-2)}.mono{font-family:var(--mono);font-size:11px;color:var(--text-1)}
</style>
