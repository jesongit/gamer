import { readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { resolve } from 'node:path'
const root = fileURLToPath(new URL('../', import.meta.url))
const lock = JSON.parse(readFileSync(resolve(root, 'plugins/sdk/lock.json'), 'utf8'))
for (const file of lock.files) {
  const bytes = readFileSync(resolve(root, file.source), 'utf8').replaceAll('\r\n', '\n')
  const hash = createHash('sha256').update(bytes).digest('hex')
  if (hash !== file.sha256) throw new Error(`Plugin SDK differs from host: ${file.source}; update the SDK snapshot and run integration tests`)
}
await import('../plugins/tools/verify-sdk.mjs')
console.log('Host and pinned plugin SDK agree')
