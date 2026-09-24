import { spawnSync } from 'node:child_process'
import { cpSync, mkdirSync, existsSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'

const repo = fileURLToPath(new URL('../', import.meta.url))
const installOnly = process.argv.includes('--install-only')
const requested = process.argv.slice(2).filter(value => value !== '--install-only')
const ids = requested.length ? requested : ['gamer-yaml', 'gamer-keymap', 'gamer-video']
for (const id of ids) {
  if (!/^gamer-[a-z0-9-]+$/.test(id)) throw new Error('Invalid plugin id')
  const plugin = resolve(repo, 'plugins', id)
  for (const args of (installOnly ? [['install', '--frozen-lockfile']] : [['install', '--frozen-lockfile'], ['build']])) {
    // pnpm is a cmd shim on Windows; arguments here are fixed, never user text.
    const result = spawnSync('pnpm', args, { cwd: resolve(plugin, 'ui'), stdio: 'inherit', shell: process.platform === 'win32' })
    if (result.status !== 0) process.exit(result.status || 1)
  }
  if (installOnly) continue
  for (const base of ['web/public/plugin-ui', 'server/web-dist/plugin-ui']) {
    if (base.startsWith('server/web-dist') && !existsSync(resolve(repo, 'server/web-dist'))) continue
    const target = resolve(repo, base, id)
    mkdirSync(target, { recursive: true })
    cpSync(resolve(plugin, 'dist/ui'), target, { recursive: true })
  }
}
