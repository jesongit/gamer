<template>
    <template v-if="ctx.form.screen_mode === 'virtual'">
    <div class="form-item"><label>虚拟屏尺寸</label><div class="vd-presets"><button type="button" v-for="p in ctx.vdPresets" :key="p.res" class="vd-opt" :aria-pressed="ctx.form.vd_res === p.res && String(ctx.form.vd_dpi) === String(p.dpi)" :class="{ sel: ctx.form.vd_res === p.res && String(ctx.form.vd_dpi) === String(p.dpi) }" @click="ctx.form.vd_res = p.res; ctx.form.vd_dpi = p.dpi"><span class="vd-res mono">{{ p.res }}</span><span class="vd-dpi">{{ p.dpi }} DPI</span></button></div></div>
    <div class="form-row"><label class="form-item">自定义宽 × 高<input v-model="ctx.form.vd_res" class="input mono" aria-label="虚拟屏尺寸" required pattern="[0-9]+x[0-9]+" placeholder="1920x1080" /></label><div class="form-item"><label for="device-dpi">显示密度 <span class="muted">（0 为自动）</span></label><div class="dpi-box"><input id="device-dpi" v-model.number="ctx.form.vd_dpi" class="input mono" aria-label="显示密度" type="number" min="0" step="1" required /><button type="button" class="btn btn-sm" :class="{ active: !ctx.form.vd_dpi }" @click="ctx.form.vd_dpi = 0">自动</button></div></div></div>
    </template>
    <div class="form-item"><label>视频帧率 <span class="muted">（越高越流畅，也会增加设备与网络负担）</span></label><div class="fps-presets"><button type="button" v-for="f in ctx.fpsPresets" :key="f" class="fps-opt mono" :aria-pressed="ctx.form.fps === f" :class="{ sel: ctx.form.fps === f }" @click="ctx.form.fps = f">{{ f }} FPS</button></div></div>
</template>
<script setup>
import { reactive } from 'vue'
const props = defineProps({ ctx: { type: Object, required: true } })
const ctx = reactive(props.ctx)
</script>
<style scoped>
.form-row{display:grid;grid-template-columns:1fr 1fr;gap:8px}.form-row .form-item{min-width:0}.dpi-box{display:flex;gap:6px}.dpi-box .input{flex:1;min-width:0}.dpi-box .btn{flex-shrink:0}.dpi-box .btn.active{border-color:var(--accent-2);color:var(--accent-2)}.muted{color:var(--text-2);font-weight:400}.vd-presets{display:grid;grid-template-columns:repeat(4,1fr);gap:6px}.vd-opt{display:flex;flex-direction:column;align-items:center;gap:2px;padding:8px 2px;border-radius:var(--radius-sm);border:1px solid var(--border);cursor:pointer}.vd-opt.sel{border-color:var(--accent-2);background:color-mix(in srgb, var(--accent) 6%, transparent)}.vd-res{font-size: 12px;color:var(--text-0)}.vd-dpi{font-size: 12px;color:var(--text-2)}.fps-presets{display:grid;grid-template-columns:repeat(4,1fr);gap:6px}.fps-opt{text-align:center;padding:8px 4px;border-radius:var(--radius-sm);border:1px solid var(--border);cursor:pointer;font-size:12px}.fps-opt.sel{border-color:var(--accent-2);color:var(--accent-2)}.mono{font-family:var(--mono);font-size: 12px;color:var(--text-1)}
.vd-opt,.fps-opt{font:inherit;background:var(--bg-2);color:var(--text-1);min-height:34px}.vd-opt:focus-visible,.fps-opt:focus-visible{outline:2px solid var(--accent);outline-offset:2px}.form-item{display:flex;flex-direction:column;gap:6px;font-size:13px}.form-item .input{width:100%}
</style>
