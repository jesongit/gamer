// Run against a disposable portable installation, never the user's normal server.
const path = require('node:path')
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright')

async function main() {
  const base = process.argv[2] || 'http://127.0.0.1:18461'
  if (new URL(base).port === '8443') throw new Error('Refusing development port')
  const browser = await chromium.launch({ channel: 'msedge', headless: true })
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } })
  try {
    const setup = await context.request.get(base + '/api/auth/setup')
    if ((await setup.json()).setup_required) {
      const response = await context.request.post(base + '/api/auth/setup', { data: { password: 'portable-e2e-test-only', confirm_password: 'portable-e2e-test-only' } })
      if (!response.ok()) throw new Error(`setup failed: ${response.status()}`)
    } else {
      const response = await context.request.post(base + '/api/login', { data: { username: 'admin', password: 'portable-e2e-test-only' } })
      if (!response.ok()) throw new Error(`login failed: ${response.status()}`)
    }
    const page = await context.newPage()
    const errors = []
    page.on('pageerror', e => errors.push(e.message))
    // Verify the not-yet-downloaded candidate action without applying another real update.
    await context.route('**/api/system/update', async route => {
      const response = await route.fetch()
      const update = await response.json()
      update.state = 'available'; update.last_error = null; update.apply_requested = false
      await route.fulfill({ response, json: update })
    })
    let applyCalls = 0
    await context.route('**/api/system/update/apply', async route => {
      applyCalls++
      await route.fulfill({ status: 202, contentType: 'application/json', body: JSON.stringify({ update_id: 'ui-test-apply', state: 'available' }) })
    })
    await page.goto(base + '/#/console?panel=gamer.core%3Asettings')
    await page.getByRole('button', { name: '更新并重启', exact: true }).waitFor()
    if (!(await page.getByText('便携版', { exact: false }).count())) throw new Error('portable label missing')
    await page.getByRole('button', { name: '更新并重启', exact: true }).click()
    await page.locator('[data-testid="confirm-btn"]').click()
    await page.locator('.modal-mask').waitFor({ state: 'hidden' })
    if (applyCalls !== 1) throw new Error('one-click update did not submit exactly once')
    await page.getByRole('button', { name: '更新并重启', exact: true }).scrollIntoViewIfNeeded()
    await page.screenshot({ path: path.resolve('release/dist/portable-settings.png'), fullPage: true })
    await context.unroute('**/api/system/update')
    await context.unroute('**/api/system/update/apply')
    await page.reload()
    // Same-origin real UI, mocked future identity: proves the global monitor reloads the page.
    await page.waitForTimeout(11000)
    let changed = false
    await context.route('**/api/system/info', async route => {
      const response = await route.fetch()
      const info = await response.json()
      if (changed) info.app.commit = 'future-build-for-reload-test'
      await route.fulfill({ response, json: info })
    })
    await context.route('**/api/system/update', async route => {
      const response = await route.fetch()
      const update = await response.json()
      update.state = 'idle'; update.last_error = null
      await route.fulfill({ response, json: update })
    })
    const reloaded = page.waitForEvent('load', { timeout: 20000 })
    changed = true
    await reloaded
    await page.getByRole('button', { name: '更新并重启', exact: true }).waitFor()
    if (errors.length) throw new Error(errors.join('\n'))
    console.log('PASS settings UI and automatic reload on committed build identity change; no page errors')
  } finally {
    await browser.close()
  }
}
main().catch(error => { console.error(error); process.exitCode = 1 })
