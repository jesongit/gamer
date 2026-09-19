/** Shared keyboard behavior for existing modal families, without changing their close guards. */
export function installDialogAccessibility(root = document) {
  const selector = '.modal-mask, .plugin-center-mask, .tpl-view-mask, .crop-modal-mask'
  const focusable = 'button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],[tabindex="0"],summary'
  const visible = element => element.getClientRects().length > 0 && !element.closest('[inert]')
  const frames = new Map()
  function sync() {
    const masks = [...root.querySelectorAll(selector)].filter(visible)
    for (const mask of masks) {
      if (frames.has(mask)) continue
      const dialog = mask.querySelector('[role="dialog"],.modal,.plugin-center,.tpl-view-modal') || mask.firstElementChild
      if (!dialog) continue
      frames.set(mask, root.activeElement)
      dialog.setAttribute('role', 'dialog'); dialog.setAttribute('aria-modal', 'true')
      if (!dialog.hasAttribute('aria-label') && !dialog.hasAttribute('aria-labelledby')) {
        const title = dialog.querySelector('.title,h2,h3,.modal-head')
        dialog.setAttribute('aria-label', title?.textContent?.trim().slice(0, 100) || '操作对话框')
      }
      dialog.tabIndex = -1
      const control = [...dialog.querySelectorAll(focusable)].find(visible)
      ;(control || dialog).focus({ preventScroll: true })
    }
    for (const [mask, previous] of frames) {
      if (mask.isConnected && masks.includes(mask)) continue
      frames.delete(mask)
      if (previous?.isConnected && !masks.length) previous.focus?.({ preventScroll: true })
    }
  }
  function keydown(event) {
    const masks = [...root.querySelectorAll(selector)].filter(visible)
    const top = masks.at(-1)
    if (!top) {
      if (event.key === 'Escape') for (const menu of root.querySelectorAll('details.resource-more[open],details.pkg-more[open]')) { menu.open = false; menu.querySelector('summary')?.focus() }
      return
    }
    if (event.key === 'Escape') {
      event.stopPropagation(); event.preventDefault()
      top.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    } else if (event.key === 'Tab') {
      const controls = [...top.querySelectorAll(focusable)].filter(visible)
      const current = controls.indexOf(root.activeElement)
      if (!controls.length) { event.preventDefault(); return }
      if (event.shiftKey && current <= 0) { event.preventDefault(); controls.at(-1).focus() }
      else if (!event.shiftKey && (current === controls.length - 1 || current < 0)) { event.preventDefault(); controls[0].focus() }
    }
  }
  function dismissMenus(event) {
    for (const menu of root.querySelectorAll('details.resource-more[open],details.pkg-more[open]')) {
      if (!menu.contains(event.target)) menu.open = false
    }
  }
  const observer = new MutationObserver(sync)
  observer.observe(root.body || root, { childList: true, subtree: true })
  root.addEventListener('keydown', keydown, true)
  root.addEventListener('pointerdown', dismissMenus)
  return () => { observer.disconnect(); root.removeEventListener('keydown', keydown, true); root.removeEventListener('pointerdown', dismissMenus) }
}
