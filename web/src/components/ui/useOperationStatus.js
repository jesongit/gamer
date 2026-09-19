import { inject, onActivated, onDeactivated, onUnmounted, ref, watch } from 'vue'
import { OPERATION_FEEDBACK_KEY } from '../../workspace/operation-feedback'

/** Builtin panels publish only their current operation, with a host-injected owner. */
export function useOperationStatus(readMessage) {
  const feedback = inject(OPERATION_FEEDBACK_KEY, null)
  const owner = inject('gamer-panel-owner', ref(''))
  const active = ref(true)
  let published = null, publishedOwner = ''
  function clearOwned() {
    if (feedback && published && publishedOwner && feedback.state.plugins[publishedOwner] === published) feedback.clearPlugin(publishedOwner)
    published = null
  }
  watch(() => [active.value, owner.value, readMessage()], ([, , message]) => {
    if (message === undefined) { clearOwned(); return }
    if (active.value && feedback && owner.value) {
      feedback.setPlugin(owner.value, message)
      publishedOwner = owner.value
      published = feedback.state.plugins[owner.value]
    }
  }, { immediate: true, deep: true })
  onActivated(() => { active.value = true })
  onDeactivated(() => { active.value = false; clearOwned() })
  onUnmounted(() => { active.value = false; clearOwned() })
}
