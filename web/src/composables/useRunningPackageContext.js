import { watch } from 'vue'

/** Follow a newly focused run's resource context, without replacing local drafts. */
export function useRunningPackageContext({ deviceId, activeRun, packages, loaded, currentPackageId, selectPackage, canSwitch = () => true }) {
  let followed = null
  return watch([
    deviceId,
    () => activeRun()?.run_id,
    () => activeRun()?.content_package,
    loaded,
    () => packages().map(item => item.id).join('\n'),
    canSwitch,
  ], () => {
    const device = deviceId()
    const run = activeRun()
    if (!device || !run || run.device_id !== device || !run.content_package) {
      followed = null
      return
    }
    const key = `${device}/${run.run_id}/${run.content_package}`
    if (followed === key || !loaded()) return
    if (!packages().some(item => item.id === run.content_package)) return
    if (currentPackageId() === run.content_package) {
      followed = key
      return
    }
    if (!canSwitch()) return
    if (selectPackage(run.content_package)) followed = key
  }, { immediate: true })
}
