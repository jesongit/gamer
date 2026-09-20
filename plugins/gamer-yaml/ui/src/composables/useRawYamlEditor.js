import { computed, ref } from 'vue'

/**
 * 原文 YAML 编辑会话：保留服务端返回的原始文本，不经过前端 codec，
 * 这样即使 YAML 当前无法解析，也可以直接修复后再提交。
 */
export function useRawYamlEditor({ api } = {}) {
  const kind = ref(null) // 'script' | 'function'
  const resourceId = ref(null)
  const content = ref('')
  const savedContent = ref('')
  const version = ref(null)
  const loading = ref(false)
  const saving = ref(false)

  const dirty = computed(() => content.value !== savedContent.value)

  async function load(nextKind, id) {
    if (nextKind !== 'script' && nextKind !== 'function') {
      throw new Error('不支持的原文资源类型')
    }
    if (!id) throw new Error('原文资源不能为空')
    loading.value = true
    try {
      const data = nextKind === 'script'
        ? await api.getScript(id)
        : await api.getFunction(id)
      kind.value = nextKind
      resourceId.value = data.id || id
      content.value = data.content ?? ''
      savedContent.value = content.value
      version.value = data.version ?? null
      return data
    } finally {
      loading.value = false
    }
  }

  async function save() {
    if (!resourceId.value || !kind.value) return { ok: false, reason: 'empty' }
    saving.value = true
    try {
      const payload = version.value
        ? { content: content.value, expected_version: version.value }
        : { content: content.value, force: true }
      const result = kind.value === 'script'
        ? await api.updateScript(resourceId.value, payload)
        : await api.updateFunction(resourceId.value, payload)
      savedContent.value = content.value
      version.value = result.version ?? version.value
      return { ok: true, result }
    } catch (error) {
      if (error?.status === 409 && error?.data?.code === 'version_conflict') {
        return { ok: false, reason: 'conflict', error }
      }
      if (Array.isArray(error?.data?.diagnostics)) {
        return { ok: false, reason: 'invalid', diagnostics: error.data.diagnostics, error }
      }
      return { ok: false, reason: 'error', error }
    } finally {
      saving.value = false
    }
  }

  function reset() {
    kind.value = null
    resourceId.value = null
    content.value = ''
    savedContent.value = ''
    version.value = null
    loading.value = false
    saving.value = false
  }

  return {
    kind,
    resourceId,
    content,
    version,
    loading,
    saving,
    dirty,
    load,
    save,
    reset,
  }
}
