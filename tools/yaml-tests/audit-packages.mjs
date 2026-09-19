// 只读扫描所有 Package 中的 YAML 自动化资源；不迁移、不执行、不修改业务数据。
import { readFile, readdir, mkdir, writeFile } from 'node:fs/promises'
import { resolve, relative, basename } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createServer } from '../../web/node_modules/vite/dist/node/index.js'

const root = fileURLToPath(new URL('../../', import.meta.url))
const packages = resolve(process.argv[2] || `${root}/server/data/packages`)
const native = JSON.parse(await readFile(new URL('native-functions.json', import.meta.url), 'utf8'))
const vite = await createServer({ root: `${root}/web`, server: { middlewareMode: true }, appType: 'custom' })
async function files(dir) {
  let entries
  try { entries = await readdir(dir, { withFileTypes: true }) } catch (error) { if (error.code === 'ENOENT') return []; throw error }
  const result = []
  for (const entry of entries) {
    const path = resolve(dir, entry.name)
    if (entry.isDirectory()) result.push(...await files(path))
    else if (entry.isFile()) result.push(path)
  }
  return result.sort()
}
try {
  const { validateSource } = await vite.ssrLoadModule('/src/script-editor/validation.ts')
  const { parseFunctionLibrary } = await vite.ssrLoadModule('/src/script-editor/codec.ts')
  const results = []
  for (const pkg of await readdir(packages, { withFileTypes: true })) {
    if (!pkg.isDirectory() || pkg.name.startsWith('.')) continue
    const plugin = resolve(packages, pkg.name, 'plugins/gamer.yaml')
    const all = await files(plugin)
    const libraries = all.filter(path => /[\\/]automations[\\/]/.test(path) && /^_function.*\.yaml$/.test(basename(path)))
    const knownFunctions = new Set(native.map(f => f.name))
    for (const path of libraries) {
      for (const fn of parseFunctionLibrary(await readFile(path, 'utf8')).model.functions) knownFunctions.add(fn.name)
    }
    const templates = new Set(all.filter(path => /[\\/]templates[\\/]/.test(path)).flatMap(path => {
      const name = relative(resolve(plugin, 'templates'), path).replaceAll('\\', '/')
      return [name, name.replace(/#1(?=\.png$)/i, '').replace(/#[^#./]+(?=\.png$)/i, '')]
    }))
    for (const path of all.filter(path => /\.ya?ml$/i.test(path) && /[\\/](automations|functions)[\\/]/.test(path))) {
      const legacyDirectory = /[\\/]functions[\\/]/.test(path)
      const kind = libraries.includes(path) || legacyDirectory ? 'function_library' : 'script'
      const result = validateSource(await readFile(path, 'utf8'), kind, {
        knownFunctions, resolveParams: name => native.find(f => f.name === name)?.params,
        resolveTemplate: name => templates.has(name),
      })
      const diagnostics = legacyDirectory
        ? [{ code: 'yaml.functions.dir.removed', message: '旧 functions/ 目录已删除；当前函数库使用 automations/_function*.yaml' }, ...result.diagnostics]
        : result.diagnostics
      results.push({ path, diagnostics })
      console.log(`${diagnostics.length ? 'FAIL' : 'PASS'} ${relative(packages, path)}${diagnostics.length ? `: ${[...new Set(diagnostics.map(d => d.code))].join(', ')}` : ''}`)
    }
  }
  const report = resolve(root, 'server/target/yaml-acceptance/package-audit.json')
  await mkdir(resolve(report, '..'), { recursive: true })
  await writeFile(report, JSON.stringify({ packages, results }, null, 2))
  console.log(`${results.length} YAML files; ${results.filter(r => r.diagnostics.length).length} failed; report: ${report}`)
  if (results.some(r => r.diagnostics.length)) process.exitCode = 1
} finally { await vite.close() }
