import { afterEach, describe, expect, it } from 'vitest'
import { createHash } from 'node:crypto'
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import config from '../vite.config.js'
import { copyCurrentPublicAssets, currentPublicAssetsPlugin, filterPublicAssets } from '../retired-public-assets.mjs'

const roots = []
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }) })
function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'gamer-public-filter-'))
  roots.push(root)
  const source = join(root, 'public')
  const output = join(root, 'output')
  const write = (name, content = name) => {
    const file = join(source, name)
    mkdirSync(dirname(file), { recursive: true })
    writeFileSync(file, content)
  }
  const keep = ['assets/logo.svg', 'plugin-ui/gamer-keymap/plugin.js', 'plugin-ui/gamer-ai-helper/plugin.js', 'plugins/gamer-keymap-1.0.0.gplugin', 'plugins/gamer-ai-1helper-1.0.0.gplugin']
  const blocked = ['plugin-ui/gamer-ai/plugin.js', 'plugin-ui/gamer-yaml/plugin.js', 'plugin-ui/gamer-yaml/style.css', 'plugin-ui/gamer-example/plugin.js', 'plugins/gamer-ai-1.0.0.gplugin', 'plugins/gamer-yaml-0.1.0.gplugin', 'plugins/gamer-example-1.0.0.gplugin', 'plugins/unlisted-1.0.0.gplugin']
  for (const name of [...keep, ...blocked]) write(name)
  write('registry.json', JSON.stringify({ provenance: { keep: 'original' }, plugins: [
    { id: 'gamer-ai', version: '1.0.0' },
    { id: 'gamer-yaml', version: '0.1.0', permissions: [' ai.connect '] },
    { id: 'gamer-example', version: '1.0.0', execution: { kind: 'builtin', builtin_id: 'gamer-ai' } },
    { id: 'gamer-keymap', version: '1.0.0', permissions: ['resource.read'] },
    { id: 'gamer-ai-1helper', version: '1.0.0' },
  ] }))
  return { root, source, output, keep, blocked }
}
function hashes(root, path = '') {
  return readdirSync(join(root, path), { withFileTypes: true }).flatMap(entry => {
    const name = path ? `${path}/${entry.name}` : entry.name
    return entry.isDirectory() ? hashes(root, name) : [`${name}:${createHash('sha256').update(readFileSync(join(root, name))).digest('hex')}`]
  }).sort()
}
function request(source, url) {
  const response = { statusCode: 200, headers: {}, setHeader(name, value) { this.headers[name] = value }, end(body) { this.body = body; this.ended = true } }
  filterPublicAssets(source)({ url }, response, error => { if (error) throw error; response.passed = true })
  return response
}

describe('retired public assets stay in preserved source snapshots', () => {
  it('ordinary Vite build uses filtered writeBundle instead of default public copying', () => {
    expect(config.plugins.some(plugin => plugin.name === 'gamer-current-public-assets')).toBe(true)
    const { root, source, output, keep, blocked } = fixture()
    const before = hashes(source)
    const plugin = currentPublicAssetsPlugin()
    expect(plugin.config().build.copyPublicDir).toBe(false)
    plugin.configResolved({ root, publicDir: source, build: { outDir: output, copyPublicDir: false } })
    plugin.writeBundle()
    for (const file of keep) expect(readFileSync(join(output, file))).toEqual(readFileSync(join(source, file)))
    for (const file of blocked) expect(existsSync(join(output, file))).toBe(false)
    const registry = JSON.parse(readFileSync(join(output, 'registry.json'), 'utf8'))
    expect(registry.plugins.map(plugin => plugin.id)).toEqual(['gamer-keymap', 'gamer-ai-1helper'])
    expect(registry.provenance).toEqual({ keep: 'original' })
    expect(hashes(source)).toEqual(before)
    expect(() => plugin.configResolved({ build: { copyPublicDir: true } })).toThrow('Unfiltered')
    expect(() => copyCurrentPublicAssets(source, join(source, 'nested'))).toThrow('outside')
    expect(() => copyCurrentPublicAssets(source, root)).toThrow('outside')
  })

  it('dev and preview middleware refuse retired paths without starting a server', () => {
    const { source, blocked, keep } = fixture()
    const before = hashes(source)
    for (const file of blocked) expect(request(source, `/${file}?import`).statusCode).toBe(404)
    for (const file of keep) expect(request(source, `/${file}`).passed).toBe(true)
    for (const path of ['/PLUGIN-UI/GAMER-AI/plugin.js', '/plugin-ui%5cgamer-ai%5cplugin.js', '/plugins/GAMER-AI-1.0.0.GPLUGIN', '/plugin-ui//gamer-ai/plugin.js', '/plugin-ui/%2fgamer-ai/plugin.js', '//plugin-ui/gamer-ai/plugin.js', '///plugin-ui//gamer-yaml/plugin.js']) {
      expect(request(source, path).statusCode).toBe(404)
    }
    for (const path of ['/%xx', '/plugin-ui/gamer-ai./plugin.js', '/plugin-ui/gamer-ai%20/plugin.js', '/plugin-ui/gamer-ai:stream/plugin.js', '/Registry.json:stream', '/plugin-ui/gamer-ai/plugin.js.', '/plugin-ui/%2e/gamer-ai/plugin.js', '/plugin-ui/%2e%2e/gamer-ai/plugin.js', '/PLUGIN~1/gamer-ai/plugin.js', '/plugins/GAMER-~1.GPL']) {
      expect(request(source, path).statusCode).toBe(400)
    }
    expect(JSON.parse(request(source, '/Registry.json').body).plugins).toHaveLength(2)
    expect(hashes(source)).toEqual(before)
    const plugin = currentPublicAssetsPlugin()
    const handlers = []
    plugin.configureServer({ config: { publicDir: source }, middlewares: { use: handler => handlers.push(handler) } })
    plugin.configurePreviewServer({ config: { root: dirname(source), build: { outDir: source } }, middlewares: { use: handler => handlers.push(handler) } })
    expect(handlers).toHaveLength(2)
  })

  it('a clean version retains the shared UI while the retired archive stays blocked', () => {
    const { source, output } = fixture()
    const file = join(source, 'registry.json')
    const registry = JSON.parse(readFileSync(file, 'utf8'))
    registry.plugins.push({ id: 'gamer-yaml', version: '1.0.0', permissions: ['resource.read'] })
    writeFileSync(file, JSON.stringify(registry))
    const before = hashes(source)
    copyCurrentPublicAssets(source, output)
    expect(readFileSync(join(output, 'plugin-ui/gamer-yaml/plugin.js'))).toEqual(readFileSync(join(source, 'plugin-ui/gamer-yaml/plugin.js')))
    expect(request(source, '/plugin-ui/gamer-yaml/plugin.js').passed).toBe(true)
    expect(request(source, '/plugins/gamer-yaml-0.1.0.gplugin').statusCode).toBe(404)
    expect(hashes(source)).toEqual(before)
  })
})
