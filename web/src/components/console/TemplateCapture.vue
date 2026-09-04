<template>
  <div v-if="!ctx.activePkg" class="pkg-empty">
    暂无应用分区：请先在右侧包名下拉中选择包名（模板与脚本按应用包名分区存储）
  </div>
  <template v-else>
    <div class="script-tpl">
      <div class="tpl-top">
        <input v-model.number="ctx.testThreshold" class="input input-sm mono" type="number" min="0" max="1" step="0.01" placeholder="测试阈值 0~1" />
        <select v-model="ctx.testRegion" class="select mono tpl-region">
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
        <input v-model="ctx.tplSearch" class="input input-sm mono tpl-search" placeholder="🔍 模糊/拼音首字母搜索…" />
        <button class="btn btn-sm" :class="{ active: ctx.picking }" :disabled="!ctx.connected" @click="ctx.togglePick">✂️ 框选</button>
        <button class="btn btn-sm" @click="tplUpload.click()">⬆️ 新建</button>
        <input ref="tplUpload" type="file" accept="image/png,image/jpeg" hidden @change="ctx.onTplUpload" />
        <input ref="tplReplaceUpload" type="file" accept="image/png,image/jpeg" hidden @change="onReplaceUpload" />
      </div>

      <div class="tpl-list-wrap">
        <div class="tpl-list-head">
          <span class="tpl-cell thumb">缩略图</span>
          <span class="tpl-cell name">文件名</span>
          <span class="tpl-cell ops">操作</span>
        </div>
        <div class="tpl-list">
          <div
            v-for="t in ctx.templates"
            :key="t.name"
            class="tpl-row"
            :class="{ renaming: ctx.renaming === t.name }"
            @click="ctx.onTplRowClick($event, t)"
          >
            <span class="tpl-cell thumb" @click.stop="ctx.onTplThumbClick($event, t)">
              <button type="button" class="tpl-thumb" title="查看大图" @click.stop="ctx.onTplThumbClick($event, t)">
                <img :src="ctx.tplThumbUrl(t.name)" alt="" loading="lazy" @error="e => e.target.style.visibility = 'hidden'" />
              </button>
            </span>
            <span class="tpl-cell name mono" :title="ctx.renaming === t.name ? '' : '点击复制短名'" @click.stop="ctx.onTplNameClick($event, t)">
              <input
                v-if="ctx.renaming === t.name"
                :ref="el => ctx.setRenameInputEl(el)"
                v-model="ctx.renameVal"
                class="input rename-input mono"
                @keydown.enter="ctx.confirmRename(t)"
                @keydown.esc="ctx.cancelRename"
                @blur="ctx.cancelRename"
                @click.stop
              />
              <template v-else>
                {{ ctx.tplShortName(t.name) }}<span v-if="ctx.tplRegionBadge(t.name)" class="tpl-region-badge">{{ ctx.tplRegionBadge(t.name) }}</span>
              </template>
            </span>
            <span class="tpl-cell ops">
              <button class="btn btn-sm" @click.stop="ctx.onTplMatchClick(t)">匹配</button>
              <span class="tpl-more-wrap">
                <button class="btn btn-sm" :class="{ active: moreOpenName === t.name }" @click.stop="toggleMore(t.name)">更多 ▾</button>
                <span v-if="moreOpenName === t.name" class="tpl-more-mask" @click.stop="closeMore"></span>
                <span v-if="moreOpenName === t.name" class="tpl-more-dropdown">
                  <button class="tpl-more-item" @click.stop="openRename(t)">重命名</button>
                  <button class="tpl-more-item danger" @click.stop="deleteTemplate(t)">删除</button>
                  <button class="tpl-more-item" @click.stop="replaceTemplate(t)">替换</button>
                </span>
              </span>
            </span>
          </div>
          <div v-if="!ctx.templates.length" class="tpl-empty">{{ ctx.tplSearch.trim() ? '没有匹配的模板' : '暂无模板，点击「框选」或「新建」创建' }}</div>
        </div>
      </div>
    </div>

    <!-- 二次裁切弹窗已拆到 TemplateCropModal.vue（挂在面板层级，任何页签下框选都可见） -->
    <div v-if="ctx.viewTpl" class="tpl-view-mask" @click.self="ctx.closeTplView">
      <div class="tpl-view-modal">
        <button class="tpl-view-close" @click="ctx.closeTplView">✕</button>
        <div class="tpl-view-img"><img :src="ctx.tplThumbUrl(ctx.viewTpl)" alt="模板预览" /></div>
        <div class="tpl-view-name mono">{{ ctx.viewTpl }}</div>
      </div>
    </div>
  </template>
</template>

<script setup>
import { reactive, ref } from 'vue'

const props = defineProps({ context: { type: Object, required: true } })
const ctx = reactive(props.context)
const tplUpload = ref(null)
const tplReplaceUpload = ref(null)
const replaceTarget = ref(null)
const moreOpenName = ref(null)

function closeMore() {
  moreOpenName.value = null
}

function toggleMore(name) {
  moreOpenName.value = moreOpenName.value === name ? null : name
}

function openRename(t) {
  closeMore()
  ctx.startRename(t)
}

async function deleteTemplate(t) {
  await ctx.onTplDeleteClick(t)
  closeMore()
}

function replaceTemplate(t) {
  closeMore()
  replaceTarget.value = t
  tplReplaceUpload.value?.click()
}

async function onReplaceUpload(e) {
  const file = e.target.files?.[0]
  e.target.value = ''
  const target = replaceTarget.value
  replaceTarget.value = null
  if (file && target) await ctx.replaceTemplateImage(target, file)
}
</script>

<style scoped>
.pkg-empty{flex:none;padding:24px 10px;text-align:center;font-size:12px;color:var(--text-2)}
.script-tpl{flex:1;min-height:0;display:flex;flex-direction:column;gap:8px}.tpl-top{display:flex;align-items:center;gap:8px}.tpl-top .input{flex:2 1 0%;min-width:0}.tpl-top .tpl-region{flex:4 1 0%;min-width:0;padding:4px 6px;font-size:11px}.tpl-top .tpl-search{flex:5 1 0%;min-width:0;font-size:11px}.tpl-top .btn{flex:3 1 0%;min-width:0}.tpl-list-wrap{flex:1;min-height:0;display:flex;flex-direction:column;gap:4px}.tpl-list-head,.tpl-row{display:flex;align-items:center;gap:8px;padding:3px 8px}.tpl-list-head{font-size:11px;color:var(--text-2);border-bottom:1px solid var(--border);flex-shrink:0}.tpl-list{flex:1;overflow:auto;display:flex;flex-direction:column;gap:2px;min-height:0}.tpl-row{cursor:pointer;border-radius:var(--radius-sm);border:1px solid transparent}.tpl-row:hover{background:var(--bg-3)}.tpl-row.del-confirm{background:rgba(248,113,113,.08);border-color:rgba(248,113,113,.35)}.tpl-row.renaming{background:rgba(56,189,248,.08);border-color:rgba(56,189,248,.35)}.tpl-empty{padding:16px 8px;text-align:center;font-size:11px;color:var(--text-2)}.tpl-cell.thumb{width:40px;flex-shrink:0;display:flex;align-items:center}.tpl-cell.name{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px;color:var(--text-0)}.tpl-cell.ops{display:flex;gap:6px;flex-shrink:0}.tpl-cell.ops .btn{padding:2px 8px;font-size:11px}.tpl-thumb{display:inline-flex;align-items:center;justify-content:center;width:28px;height:28px;padding:0;border:1px solid transparent;border-radius:4px;background:transparent;cursor:pointer}.tpl-thumb:hover{border-color:var(--accent)}.tpl-thumb img{width:24px;height:24px;object-fit:contain}.rename-input{width:100%;min-width:0;padding:2px 6px;font-size:12px}.tpl-region-badge{display:inline-block;margin-left:6px;padding:0 5px;border-radius:4px;background:var(--bg-3);border:1px solid var(--border);color:var(--accent);font-size:10px;line-height:16px}.tpl-more-wrap{position:relative;display:inline-flex}.tpl-more-mask{position:fixed;inset:0;z-index:20}.tpl-more-dropdown{position:absolute;right:0;top:calc(100% + 4px);z-index:30;display:flex;flex-direction:column;min-width:92px;padding:4px;gap:2px;background:var(--bg-2);border:1px solid var(--border);border-radius:var(--radius-sm);box-shadow:0 8px 24px rgba(0,0,0,.4)}.tpl-more-item{display:flex;align-items:center;text-align:left;white-space:nowrap;padding:6px 10px;border:none;background:none;border-radius:var(--radius-sm);color:var(--text-0);font-size:12px;cursor:pointer}.tpl-more-item:hover{background:var(--bg-3)}.tpl-more-item.danger:hover{color:var(--danger)}.tpl-view-mask{position:fixed;inset:0;z-index:100;display:flex;align-items:center;justify-content:center;background:rgba(8,10,16,.78)}.tpl-view-modal{position:relative;display:flex;flex-direction:column;gap:8px;max-width:92vw;max-height:92vh}.tpl-view-img{position:relative;align-self:center}.tpl-view-img img{display:block;max-width:92vw;max-height:82vh;object-fit:contain;border-radius:var(--radius-sm);border:1px solid var(--border);background:#000}.tpl-view-close{position:absolute;top:8px;right:8px;width:28px;height:28px;background:var(--bg-2);border:1px solid var(--border);border-radius:50%;color:var(--text-1);cursor:pointer;z-index:1}.tpl-view-name{text-align:center;font-size:12px;color:var(--text-1);word-break:break-all}
.mono{font-family:var(--mono);font-size:11px;color:var(--text-1)}
</style>
