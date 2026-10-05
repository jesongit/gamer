import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { createServer } from 'vite'
import config from '../vite.config.js'

describe('prebuilt plugin modules in the Vite dev server', () => {
  let directory, server, origin
  const testRoot = resolve(tmpdir())
  const entry = 'export { value } from "./entry-part.js";\n'
  const chunk = 'export const value = 42;\n'

  beforeAll(async () => {
    directory = await mkdtemp(join(testRoot, 'gamer-plugin-ui-dev-'))
    const assets = join(directory, 'public', 'plugin-ui', 'gamer-example')
    await mkdir(assets, { recursive: true })
    await writeFile(join(assets, 'plugin.js'), entry)
    await writeFile(join(assets, 'entry-part.js'), chunk)
    await writeFile(join(directory, 'public', 'other.js'), 'export const other = true;\n')
    server = await createServer({
      ...config, configFile: false, root: directory, publicDir: 'public',
      server: { host: '127.0.0.1', port: 0, strictPort: true, hmr: false },
      optimizeDeps: { noDiscovery: true, include: [] }, logLevel: 'silent',
    })
    await server.listen()
    origin = `http://127.0.0.1:${server.httpServer.address().port}`
  })

  afterAll(async () => {
    await server?.close()
    if (directory) {
      if (dirname(directory) !== testRoot || !basename(directory).startsWith('gamer-plugin-ui-dev-')) {
        throw new Error('Refusing to remove a directory outside the test fixture')
      }
      await rm(directory, { recursive: true, force: true })
    }
  })

  it.each([
    ['plugin.js?import', entry],
    ['plugin.js?v=1.2.3&import', entry],
    ['entry-part.js?import&v=1.2.3', chunk],
    ['entry-part.js', chunk],
  ])('serves %s as its original JavaScript without Vite transforms', async (asset, source) => {
    const response = await fetch(`${origin}/plugin-ui/gamer-example/${asset}`)
    expect(response.status).toBe(200)
    expect(response.headers.get('content-type')).toContain('javascript')
    expect(await response.text()).toBe(source)
  })

  it('keeps the normal public import restriction outside plugin-ui', async () => {
    const response = await fetch(`${origin}/other.js?import`)
    expect(response.status).toBe(500)
    expect(await response.text()).toContain('This file is in /public')
  })
})
