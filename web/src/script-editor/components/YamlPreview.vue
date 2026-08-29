<template>
  <div class="yaml-preview" @click.stop>
    <div class="preview-head">
      <span class="preview-title">生成 YAML（只读诊断预览）</span>
      <span class="preview-actions">
        <button type="button" class="mini-btn" @click="copyYaml">{{ copied ? '已复制' : '复制' }}</button>
        <button type="button" class="mini-btn" @click="downloadYaml">下载</button>
        <button type="button" class="mini-btn" @click="emit('close')">关闭</button>
      </span>
    </div>
    <pre class="yaml-pre">{{ yaml }}</pre>
  </div>
</template>

<script setup lang="ts">
/**
 * YAML 只读诊断预览（plan §8.3 / §10）：codec.serialize 规范输出，不可编辑；
 * 提供复制 / 下载按钮用于排查问题。编辑一律回画布，此处不做任何反向解析。
 */
import { computed, ref, type PropType } from 'vue'
import type { EditorModel } from '../commands'
import { serialize } from '../codec'

const props = defineProps({
  model: { type: Object as PropType<EditorModel>, required: true },
  /** 下载文件名。 */
  filename: { type: String, default: 'script.yaml' },
})

const emit = defineEmits(['close'])

const yaml = computed(() => serialize(props.model))
const copied = ref(false)

async function copyYaml(): Promise<void> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(yaml.value)
    } else {
      const ta = document.createElement('textarea')
      ta.value = yaml.value
      document.body.appendChild(ta)
      ta.select()
      document.execCommand?.('copy')
      ta.remove()
    }
    copied.value = true
    setTimeout(() => {
      copied.value = false
    }, 1500)
  } catch {
    // 剪贴板不可用（非安全上下文/无权限）时静默失败，不打断预览
  }
}

function downloadYaml(): void {
  try {
    const blob = new Blob([yaml.value], { type: 'text/yaml;charset=utf-8' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = props.filename
    a.click()
    URL.revokeObjectURL(url)
  } catch {
    // 环境不支持 Blob/URL 时忽略
  }
}
</script>

<style scoped>
.yaml-preview {
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--bg-1);
  box-shadow: var(--shadow);
  overflow: hidden;
}
.preview-head {
  display: flex; align-items: center; justify-content: space-between; gap: 8px;
  padding: 8px 10px; border-bottom: 1px solid var(--border);
}
.preview-title { font-size: 13px; font-weight: 600; }
.preview-actions { display: inline-flex; gap: 6px; }
.mini-btn {
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 4px; font-size: 11px; padding: 3px 8px; cursor: pointer;
}
.mini-btn:hover { color: var(--accent); border-color: var(--accent); }
.yaml-pre {
  margin: 0; padding: 10px 12px;
  font-family: var(--mono); font-size: 12px; line-height: 1.6;
  color: var(--text-0); background: var(--bg-0);
  max-height: 420px; overflow: auto;
  user-select: text; cursor: text;
}
</style>
