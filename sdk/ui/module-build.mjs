import { readFileSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { dirname, resolve, relative } from 'node:path'
import { createRequire } from 'node:module'

// Host-integrated UI packages share Vue and the explicit SDK with the shell.
// Sandboxed iframe plugins keep using the message-port SDK instead.
export async function pluginUiConfig(configUrl) {
  const root = dirname(fileURLToPath(configUrl))
  const require = createRequire(configUrl)
  const { default: vue } = await import(pathToFileURL(require.resolve('@vitejs/plugin-vue')).href)
  const modules = JSON.parse(readFileSync(new URL('./host-modules.json', import.meta.url), 'utf8'))
  const coreRoot = fileURLToPath(new URL('../../web/src/', import.meta.url))
  const virtualPrefix = '\0gamer-host-sdk:'
  return {
    root,
    plugins: [
      {
        name: 'gamer-host-sdk',
        enforce: 'pre',
        resolveId(source, importer) {
          if (source === 'vue' || source === 'vue-router') return virtualPrefix + source
          if (!importer || !source.startsWith('.')) return null
          const target = resolve(dirname(importer.split('?')[0]), source)
          const key = relative(coreRoot, target).replaceAll('\\', '/')
          if (Object.hasOwn(modules, key)) return virtualPrefix + key
          return null
        },
        load(id) {
          if (!id.startsWith(virtualPrefix)) return null
          const key = id.slice(virtualPrefix.length)
          const names = key === 'vue' ? Object.keys(require('vue')) : modules[key]
          return `const sdk = globalThis.__gamerPluginSdkV1?.[${JSON.stringify(key)}];\n`
            + `if (!sdk) throw new Error('Gamer UI SDK v1 unavailable: ${key}');\n`
            + names.map(name => name === 'default'
              ? 'export default sdk.default;'
              : `export const ${name} = sdk.${name};`).join('\n')
        },
      },
      vue(),
    ],
    build: {
      outDir: '../dist/ui', emptyOutDir: true,
      lib: { entry: resolve(root, 'entry.js'), formats: ['es'], fileName: () => 'plugin.js' },
      cssCodeSplit: false,
      rollupOptions: { output: { assetFileNames: asset => asset.name?.endsWith('.css') ? 'style.css' : '[name][extname]' } },
    },
  }
}
