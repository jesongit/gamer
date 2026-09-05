<template>
  <div class="gy-editor" data-testid="gamer-yaml-payload-editor">
    <!-- 脚本参数未就绪前不渲染 ParamsForm：其 rebuild 会在挂载瞬间回放一次
         change（args 取当前 initialArgs），若在参数声明到达前渲染会以空 args
         覆盖掉正在采用的任务快照 -->
    <div v-if="params.length" class="form-item">
      <label>运行参数</label>
      <ParamsForm
        ref="pfEl"
        :params="params"
        :initial-args="initialArgs"
        :templates="templateNames"
        @change="onArgsChange"
      />
    </div>
    <div v-else-if="loadFailed" class="gy-empty gy-err" data-testid="gy-load-failed">
      参数声明加载失败{{ loadError ? `：${loadError}` : '' }}（任务仍可保存）。
    </div>
    <div v-else-if="paramsLoaded" class="gy-empty" data-testid="gy-no-params">该脚本未声明参数，可直接保存。</div>
    <div v-else-if="!entrypoint" class="gy-empty" data-testid="gy-pick-first">请先选择执行目标（脚本）。</div>
    <div v-else class="gy-empty">正在加载脚本参数…</div>
  </div>
</template>

<script setup lang="ts">
/**
 * gamer.yaml runner 的 payload 编辑器（RunnerEditorContribution V1 首个实现）。
 *
 * 以下全是贡献内部事务，TaskBoard 不感知：
 * - 参数声明经服务端 entrypoint schema API（P12.3 契约 §7，前端不为取参数而解析
 *   YAML）获取 → entrypointParams.schemaToParamDecls 适配 → ParamsForm 渲染
 *   类型化控件（稀疏 args 三态：默认/覆盖/必填）；响应 signature（psig1）本期
 *   仅透传不消费，供后续过期预检；
 * - tmpl 控件的模板短名候选来自 store.templatesData 按脚本分区过滤（自 TaskBoard 迁入）；
 * - store.scriptsData/templatesData 为空时自行拉取填充（ScriptPicker 是纯 store 消费者）；
 * - 切换执行目标后原 payload.args 不再适用：清空带入并整体替换 payload。
 *
 * payload 形状固定 { args }（稀疏覆盖映射），经 update:payload 上报给 TaskBoard 保存。
 */
import { computed, ref, watch, onMounted } from 'vue'
import type { PropType } from 'vue'
import ParamsForm from '../../script-editor/components/ParamsForm.vue'
import type { ParamDecl } from '../../script-editor/model'
import { schemaToParamDecls } from '../../script-editor/entrypointParams'
import { cloneArg } from '../../script-editor/params'
import { api } from '../../api'
import { GAMER_YAML_RUNNER_ID } from '../../gamer-yaml-runner'
import { templatesData } from '../../store'
import {
  ensureGamerYamlResources, scriptPackageOf, templateShortName,
} from './gamer-yaml-resources'
import type { RunnerEditorContext, RunnerEditorIssue } from './runner-editors'

const props = defineProps({
  entrypoint: { type: String, default: '' },
  payload: { type: Object as PropType<Record<string, unknown>>, default: () => ({}) },
  ctx: { type: Object as PropType<RunnerEditorContext>, default: () => ({ androidPackage: null, deviceId: '' }) },
})
const emit = defineEmits(['update:payload'])

const pfEl = ref<InstanceType<typeof ParamsForm> | null>(null)
const params = ref<ParamDecl[]>([])
const paramsLoaded = ref(false)
const loadFailed = ref(false)
const loadError = ref('')
// 已采用的执行目标：仅在该目标上把 payload.args 带入表单一次；此后 entrypoint 变化即清空
const adoptedEntrypoint = ref('')
const initialArgs = ref<Record<string, unknown>>({})
let loadSeq = 0

/** 当前 entrypoint 的脚本分区 → 模板短名候选（tmpl 控件下拉）。 */
const templateNames = computed<string[]>(() => {
  const pkg = scriptPackageOf(props.entrypoint)
  if (!pkg) return []
  return templatesData.value
    .filter((t) => t.pkg === pkg)
    .map((t) => templateShortName(t.name))
})

async function loadScript(entrypoint: string): Promise<void> {
  const seq = ++loadSeq
  paramsLoaded.value = false
  loadFailed.value = false
  loadError.value = ''
  params.value = []
  if (!entrypoint) return
  try {
    const descriptor = await api.getEntrypointParams(GAMER_YAML_RUNNER_ID, entrypoint)
    if (seq !== loadSeq) return // 迟到响应：执行目标已再切换，丢弃
    params.value = schemaToParamDecls(descriptor?.schema)
    paramsLoaded.value = true
    if (!params.value.length) {
      // 无参数声明的脚本：快照 args 不再有意义，清空（与旧 TaskBoard 行为一致）
      emit('update:payload', { args: {} })
    }
  } catch (e) {
    if (seq !== loadSeq) return
    paramsLoaded.value = true
    loadFailed.value = true
    // 结构化错误优先（404 不存在 / 400 invalid_script 诊断首条），其余退回原始消息
    const err = e as {
      data?: { error?: string; resource?: string; diagnostics?: Array<{ message?: string }> }
      message?: string
    }
    const firstDiag = err.data?.diagnostics?.find((d) => typeof d?.message === 'string' && d.message)?.message ?? ''
    loadError.value = firstDiag
      || (err.data?.error === 'not_found' ? `脚本不存在：${err.data.resource || entrypoint}` : '')
      || (typeof err.message === 'string' && err.message ? err.message : '未知错误')
  }
}

watch(() => props.entrypoint, (ep) => {
  if (ep === adoptedEntrypoint.value) return
  // 切换执行目标：原 payload.args 不再适用，清空带入并整体替换 payload
  adoptedEntrypoint.value = ep
  initialArgs.value = {}
  emit('update:payload', { args: {} })
  loadScript(ep)
})

function onArgsChange(change: { args: Record<string, unknown> }): void {
  emit('update:payload', { args: cloneArg(change.args) })
}

/** 校验问题（空数组 = 通过）；TaskBoard 保存前调用。 */
function validate(): RunnerEditorIssue[] {
  const errs = pfEl.value?.validate?.() ?? []
  return errs.map((e) => ({ name: e.name, message: e.message }))
}

defineExpose({ validate })

onMounted(() => {
  adoptedEntrypoint.value = props.entrypoint
  // 编辑既有任务：payload.args 整体带入覆盖态（resolve 语义：本次采用=payload 值）
  const adopted = props.payload?.args
  initialArgs.value = adopted && typeof adopted === 'object' ? cloneArg(adopted) : {}
  void ensureGamerYamlResources()
  loadScript(props.entrypoint)
})
</script>

<style scoped>
.gy-editor { display: flex; flex-direction: column; gap: 6px; }
.gy-empty { font-size: 12px; color: var(--text-2); padding: 2px 0; }
.gy-err { color: var(--warn); }
</style>
