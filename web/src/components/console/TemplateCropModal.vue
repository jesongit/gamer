<template>
  <!-- 二次裁切弹窗：独立于模板页签挂载（框选可从脚本编辑的「框选」按钮发起，不切页签）；
       确认/取消是独立操作，弹窗形态下列表保持可见 -->
  <div v-if="ctx.crop.active" class="modal-mask" @click.self="ctx.cancelCrop">
    <div class="modal crop-modal" ref="cropSec">
      <div class="modal-head">
        <span class="title">✂️ 二次裁切</span>
        <span class="mono crop-meta">{{ ctx.cropSize }} · {{ ctx.cropZoomPct }}</span>
        <button class="btn btn-ghost btn-sm" @click="ctx.cancelCrop">✕</button>
      </div>
      <div class="modal-body">
        <div class="crop-stage"><canvas ref="cropCanvas" class="crop-canvas" @mousedown="ctx.cropMouseDown" @mousemove="ctx.cropMouseMove" @mouseup="ctx.cropMouseUp" @mouseleave="ctx.cropMouseLeave" @wheel="ctx.cropWheel"></canvas></div>
        <div class="crop-hint">滚轮缩放（50%~800%）· 拖动边框/角调整选框 · Alt 点击任意处 → 取色生成 color 记录</div>
        <input v-model="ctx.crop.name" class="input mono" placeholder="模板名称（默认自动生成，支持中文）" @keydown.enter="ctx.saveTemplate" />
      </div>
      <div class="modal-foot">
        <button class="btn btn-sm" @click="ctx.cancelCrop">取消</button>
        <button class="btn btn-sm btn-ghost" @click="ctx.repick">重新框选</button>
        <button class="btn btn-sm btn-primary" :disabled="ctx.saving" @click="ctx.saveTemplate">{{ ctx.saving ? '保存中…' : '💾 保存模板' }}</button>
      </div>
    </div>
  </div>
</template>
<script setup>
import { nextTick, onMounted, reactive, ref, watch } from 'vue'
const props = defineProps({ context: { type: Object, required: true }, onCropMounted: { type: Function, required: true } })
const ctx = reactive(props.context); const cropSec = ref(null); const cropCanvas = ref(null)
async function emitCropRefs() { if (!ctx.crop.active) return; await nextTick(); props.onCropMounted({ canvas: cropCanvas.value, section: cropSec.value }) }
watch(() => ctx.crop.active, emitCropRefs); onMounted(emitCropRefs)
</script>
<style scoped>
.crop-modal{width:72vw;max-width:1100px;height:70vh;display:flex;flex-direction:column}
.crop-modal .modal-body{flex:1;min-height:0;display:flex;flex-direction:column;gap:8px}
.crop-modal .crop-meta{margin-right:auto}
.crop-modal .modal-foot .btn-primary{margin-left:auto}
.crop-stage{flex:1;min-height:0;display:flex;overflow:auto;border:1px solid var(--border);border-radius:var(--radius-sm);background:#000}
.crop-stage .crop-canvas{margin:auto}
.crop-canvas{border-radius:var(--radius-sm);cursor:crosshair;background:#000;touch-action:none}
.crop-hint{font-size:10px;color:var(--text-2)}
.mono{font-family:var(--mono);font-size:11px;color:var(--text-1)}
</style>
