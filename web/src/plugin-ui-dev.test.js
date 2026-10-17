import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { createServer as createHttpServer } from 'node:http'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer, isFileServingAllowed } from 'vite'
import config from '../vite.config.js'

describe('prebuilt plugin modules in the Vite dev server', () => {
  let directory, privateDirectory, backend, server, origin
  const backendRequests = []
  const testRoot = resolve(tmpdir())
  const sharedModule = fileURLToPath(new URL('../../plugins/ui-shared/backdrop-dismiss.js', import.meta.url))
  const serverPrivateProbe = fileURLToPath(new URL('../../server/__gamer_vite_forbidden_fixture__.toml', import.meta.url))
  const fsUrl = path => `/@fs/${path.replace(/\\/g, '/')}`
  const entry = 'export { value } from "./entry-part.js";\n'
  const chunk = 'export const value = 42;\n'
  const retiredMarker = 'retired fixture bytes must never be served'
  const privateMarker = 'synthetic private fixture must stay outside the allow list'

  beforeAll(async () => {
    directory = await mkdtemp(join(testRoot, 'gamer-plugin-ui-dev-'))
    privateDirectory = await mkdtemp(join(testRoot, 'gamer-plugin-ui-private-'))
    await writeFile(join(privateDirectory, 'config.toml'), privateMarker)
    const assets = join(directory, 'public', 'plugin-ui', 'gamer-example')
    await mkdir(assets, { recursive: true })
    await writeFile(join(assets, 'plugin.js'), entry)
    await writeFile(join(assets, 'entry-part.js'), chunk)
    await writeFile(join(directory, 'public', 'other.js'), 'export const other = true;\n')
    await mkdir(join(directory, 'public', 'plugin-ui', 'gamer-ai'), { recursive: true })
    await mkdir(join(directory, 'public', 'plugins'), { recursive: true })
    await writeFile(join(directory, 'public', 'plugin-ui', 'gamer-ai', 'plugin.js'), retiredMarker)
    await writeFile(join(directory, 'public', 'plugins', 'gamer-ai-1.0.0.gplugin'), retiredMarker)
    await writeFile(join(directory, 'public', 'registry.json'), JSON.stringify({ schema_version: 2, plugins: [
      { id: 'gamer-ai', version: '1.0.0' },
      { id: 'gamer-example', version: '1.0.0', permissions: ['resource.read'] },
    ] }))
    await writeFile(join(directory, 'index.html'), '<div id="app"></div><script type="module">import App from "/App.vue"; console.log(App)</script>')
    await writeFile(join(directory, 'App.vue'), `
      <script>
      import { vBackdropDismiss } from ${JSON.stringify(sharedModule.replace(/\\/g, '/'))}
      export default { directives: { backdropDismiss: vBackdropDismiss } }
      </script>
      <template><div class="fixture" v-backdrop-dismiss="() => {}">fixture-ready</div></template>
      <style scoped>.fixture { color: green; }</style>
    `)
    backend = createHttpServer((request, response) => {
      backendRequests.push(request.url)
      const path = new URL(request.url, 'http://fixture').pathname
      const content = {
        '/api/extensions/gamer-example/ui/plugin.js': entry,
        '/api/extensions/gamer-example/ui/entry-part.js': chunk,
      }[path]
      response.statusCode = content === undefined ? 404 : 200
      response.setHeader('Content-Type', 'application/javascript; charset=utf-8')
      response.end(content || '')
    })
    await new Promise(resolve => backend.listen(0, '127.0.0.1', resolve))
    const backendOrigin = `http://127.0.0.1:${backend.address().port}`
    server = await createServer({
      ...config, configFile: false, root: directory, publicDir: 'public',
      resolve: { ...config.resolve, alias: { vue: fileURLToPath(new URL('../node_modules/vue/dist/vue.runtime.esm-bundler.js', import.meta.url)) } },
      server: {
        ...config.server,
        host: '127.0.0.1', port: 0, strictPort: true, hmr: false,
        fs: { ...config.server?.fs, allow: [...(config.server?.fs?.allow || []), directory] },
        proxy: { '/api': { ...config.server?.proxy?.['/api'], target: backendOrigin } },
      },
      optimizeDeps: { noDiscovery: true, include: [] }, logLevel: 'silent',
    })
    await server.listen()
    origin = `http://127.0.0.1:${server.httpServer.address().port}`
  })

  afterAll(async () => {
    await server?.close()
    if (backend) await new Promise(resolve => backend.close(resolve))
    for (const [fixture, prefix] of [[directory, 'gamer-plugin-ui-dev-'], [privateDirectory, 'gamer-plugin-ui-private-']]) {
      if (!fixture) continue
      if (dirname(fixture) !== testRoot || !basename(fixture).startsWith(prefix)) {
        throw new Error('Refusing to remove a directory outside the test fixture')
      }
      await rm(fixture, { recursive: true, force: true })
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

  it.each([
    ['plugin.js?import&v=1.2.3', entry],
    ['plugin.js?v=1.2.3&import', entry],
    ['entry-part.js?import&v=1.2.3', chunk],
  ])('proxies installed UI %s as original JavaScript while retaining its version query', async (asset, source) => {
    const response = await fetch(`${origin}/api/extensions/gamer-example/ui/${asset}`)
    expect(response.status).toBe(200)
    expect(response.headers.get('content-type')).toContain('javascript')
    expect(await response.text()).toBe(source)
    const request = new URL(backendRequests.at(-1), 'http://fixture')
    expect(request.pathname).toBe(`/api/extensions/gamer-example/ui/${asset.split('?')[0]}`)
    expect(request.search).toBe('?v=1.2.3')
  })

  it('keeps the normal public import restriction outside plugin-ui', async () => {
    const response = await fetch(`${origin}/other.js?import`)
    expect(response.status).toBe(500)
    expect(await response.text()).toContain('This file is in /public')
  })

  it('serves the Vue export helper and real external shared module URLs emitted by Vite', async () => {
    // Request this before importing App.vue: import analysis can otherwise
    // warm Vite's safe-module cache and hide a missing configured fs.allow.
    const coldShared = await fetch(new URL(fsUrl(sharedModule), origin))
    expect(coldShared.status).toBe(200)
    expect(await coldShared.text()).toContain('vBackdropDismiss')
    const page = await fetch(`${origin}/`)
    expect(page.status).toBe(200)
    expect(await page.text()).toContain('/@vite/client')
    const component = await fetch(`${origin}/App.vue`)
    expect(component.status).toBe(200)
    const source = await component.text()
    const helper = source.match(/["']([^"']*plugin-vue:export-helper)["']/)?.[1]
    const shared = source.match(/["'](\/@fs\/[^"']*\/plugins\/ui-shared\/backdrop-dismiss\.js(?:\?[^"']*)?)["']/)?.[1]
    expect(helper).toBe('/@id/__x00__plugin-vue:export-helper')
    expect(shared).toBeTruthy()
    for (const [url, expected] of [[helper, '__vccOpts'], [shared, 'vBackdropDismiss']]) {
      const response = await fetch(new URL(url, origin))
      expect.soft(response.status, url).toBe(200)
      expect.soft(response.headers.get('content-type'), url).toContain('javascript')
      expect.soft(await response.text(), url).toContain(expected)
    }
  })

  it('keeps adjacent private files and the server source directory outside the filesystem allow list', async () => {
    // This nonexistent server pathname checks the resolved Vite policy only;
    // no real server configuration or private file is opened or requested.
    expect(isFileServingAllowed(fsUrl(serverPrivateProbe), server)).toBe(false)
    const response = await fetch(new URL(fsUrl(join(privateDirectory, 'config.toml')), origin))
    expect(response.status).toBe(403)
    expect(await response.text()).not.toContain(privateMarker)
  })

  it('blocks retired public bytes through normal, /public and Vite filesystem URLs', async () => {
    for (const name of ['plugin-ui/gamer-ai/plugin.js', 'plugins/gamer-ai-1.0.0.gplugin']) {
      for (const url of [`/${name}`, `/public/${name}`, fsUrl(join(directory, 'public', ...name.split('/')))]) {
        const response = await fetch(new URL(url, origin))
        expect.soft([400, 403, 404], url).toContain(response.status)
        expect.soft(await response.text(), url).not.toContain(retiredMarker)
      }
    }
  })

  it('never exposes the unfiltered registry through /public or Vite filesystem URLs', async () => {
    for (const url of ['/registry.json', '/public/registry.json', fsUrl(join(directory, 'public', 'registry.json'))]) {
      const response = await fetch(new URL(url, origin))
      if (response.status === 200) {
        expect.soft((await response.json()).plugins.map(plugin => plugin.id), url).toEqual(['gamer-example'])
      } else {
        expect([400, 403, 404], url).toContain(response.status)
      }
    }
  })
})
