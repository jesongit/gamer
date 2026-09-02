<template>
  <!-- 二次裁切弹窗：独立于模板页签挂载（框选可从脚本编辑的「框选」按钮发起，不切页签）；
       确认/取消是独立操作，弹窗形态下列表保持可见 -->
  <div v-if="ctx.crop.active" class="modal-mask" @click.self="ctx.cancelCrop">
    <div class="modal crop-modal" ref="cropSec">
      <div class="modal-head">
        <span class="title">{{ ctx.crop.conflict ? '⚠️ 模板短名冲突' : '✂️ 二次裁切' }}</span>
        <span v-if="!ctx.crop.conflict" class="mono crop-meta">{{ ctx.cropSize }} · {{ ctx.cropZoomPct }}</span>
        <button class="btn btn-ghost btn-sm" @click="ctx.cancelCrop">✕</button>
      </div>
      <div v-if="ctx.crop.conflict" class="modal-body crop-conflict-body">
        <div class="crop-conflict-message">
          模板短名 <span class="mono">{{ ctx.crop.conflict.shortName }}</span> 已存在，是否覆盖模板 <span class="mono">{{ ctx.crop.conflict.name }}</span>？
        </div>
        <div class="crop-conflict-hint">两张图片都支持滚轮放大/缩小，方便查看小模板。</div>
        <div class="crop-compare">
          <div class="crop-compare-card">
            <div class="crop-compare-label">当前裁切模板 <span class="mono">{{ Math.round(compareZoom.current * 100) }}%</span></div>
            <div class="crop-compare-image" @wheel.stop.prevent="onCompareWheel($event, 'current')"><img :src="ctx.crop.preview" alt="当前裁切模板" :style="compareImageStyle('current')" /></div>
          </div>
          <div class="crop-compare-card">
            <div class="crop-compare-label">模板库中的 {{ ctx.crop.conflict.name }} <span class="mono">{{ Math.round(compareZoom.existing * 100) }}%</span></div>
            <div class="crop-compare-image" @wheel.stop.prevent="onCompareWheel($event, 'existing')"><img :src="ctx.tplThumbUrl(ctx.crop.conflict.name)" alt="模板库中的模板" :style="compareImageStyle('existing')" /></div>
          </div>
        </div>
      </div>
      <div v-else class="modal-body">
        <div class="crop-stage"><canvas ref="cropCanvas" class="crop-canvas" @mousedown="ctx.cropMouseDown" @mousemove="ctx.cropMouseMove" @mouseup="ctx.cropMouseUp" @mouseleave="ctx.cropMouseLeave" @wheel="ctx.cropWheel"></canvas></div>
        <div class="crop-hint">滚轮缩放（50%~800%）· 拖动边框/角调整选框</div>
        <input v-model="ctx.crop.name" class="input mono" placeholder="模板名称（默认自动生成，支持中文）" @keydown.enter="ctx.saveTemplate" />
        <label class="crop-color-option"><input v-model="ctx.crop.preserveColor" type="checkbox" /> 保留颜色（文件名自动加 <span class="mono">#1</span>）</label>
      </div>
      <div class="modal-foot">
        <template v-if="ctx.crop.conflict">
          <button class="btn btn-sm" :disabled="ctx.saving" @click="ctx.backToCrop">返回修改</button>
          <button class="btn btn-sm btn-primary" :disabled="ctx.saving" @click="ctx.overwriteTemplate">{{ ctx.saving ? '覆盖中…' : '确认覆盖' }}</button>
        </template>
        <template v-else>
          <button class="btn btn-sm" @click="ctx.cancelCrop">取消</button>
          <button class="btn btn-sm btn-ghost" @click="ctx.repick">重新框选</button>
          <button class="btn btn-sm btn-primary" :disabled="ctx.saving" @click="ctx.saveTemplate">{{ ctx.saving ? '保存中…' : '💾 保存模板' }}</button>
        </template>
      </div>
    </div>
  </div>
</template>
<script setup>
import { nextTick, onMounted, reactive, ref, watch } from 'vue'
const props = defineProps({ context: { type: Object, required: true }, onCropMounted: { type: Function, required: true } })
const ctx = reactive(props.context); const cropSec = ref(null); const cropCanvas = ref(null)
const compareZoom = reactive({ current: 1, existing: 1 })
function resetCompareZoom() { compareZoom.current = 1; compareZoom.existing = 1 }
function onCompareWheel(e, side) {
  const key = side === 'existing' ? 'existing' : 'current'
  const next = compareZoom[key] * (e.deltaY < 0 ? 1.2 : 1 / 1.2)
  compareZoom[key] = Math.max(0.5, Math.min(8, next))
}
function compareImageStyle(side) { return { transform: `scale(${compareZoom[side === 'existing' ? 'existing' : 'current']})` } }
async function emitCropRefs() { if (!ctx.crop.active) return; await nextTick(); props.onCropMounted({ canvas: cropCanvas.value, section: cropSec.value }) }
watch(() => ctx.crop.active, emitCropRefs); onMounted(emitCropRefs)
watch(() => !!ctx.crop.conflict, value => { if (value) resetCompareZoom(); else emitCropRefs() })
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
.crop-color-option{display:flex;align-items:center;gap:6px;font-size:11px;color:var(--text-1);user-select:none}
.crop-conflict-body{gap:12px}
.crop-conflict-message{padding:8px 10px;border:1px solid rgba(250,204,21,.4);border-radius:var(--radius-sm);background:rgba(250,204,21,.08);color:var(--text-0);font-size:12px}
.crop-conflict-hint{font-size:10px;color:var(--text-2)}
.crop-compare{flex:1;min-height:0;display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}
.crop-compare-card{min-width:0;min-height:0;display:flex;flex-direction:column;gap:8px;padding:10px;border:1px solid var(--border);border-radius:var(--radius-sm);background:var(--bg-1)}
.crop-compare-label{font-size:11px;color:var(--text-1);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.crop-compare-label .mono{float:right;color:var(--text-2)}
.crop-compare-image{flex:1;min-height:0;display:flex;align-items:center;justify-content:center;overflow:auto;border-radius:var(--radius-sm);background:#000}
.crop-compare-image img{display:block;max-width:100%;max-height:100%;object-fit:contain;transform-origin:center;transition:transform .12s ease}
.mono{font-family:var(--mono);font-size:11px;color:var(--text-1)}
</style>
