import { copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from 'node:fs'
import { basename, dirname, isAbsolute, relative, resolve, sep } from 'node:path'

const retired = plugin => plugin.id === 'gamer-ai' || plugin.execution?.builtin_id === 'gamer-ai' || plugin.permissions?.some(value => typeof value === 'string' && value.trim() === 'ai.connect')

function normalizedPublicPath(path) {
  if (path.length > 8192 || /[\0-\x1f\x7f]/.test(path)) return null
  const parts = path.replace(/\\/g, '/').split('/').filter(Boolean)
  // Reject filesystem aliases rather than let Windows resolve a different
  // file from the one authorized here (trailing dots/spaces and ADS syntax).
  if (parts.some(part => part === '.' || part === '..' || /[. ]$/.test(part) || /[:~]/.test(part))) return null
  return parts.join('/').toLowerCase()
}

// Keep immutable public snapshots on disk. Only responses and newly built
// output omit retired distributions, including their old generated UI modules.
export function publicAssetPolicy(publicDir) {
  const registryPath = resolve(publicDir, 'registry.json')
  const registry = existsSync(registryPath) ? JSON.parse(readFileSync(registryPath, 'utf8')) : null
  const currentArchives = new Set()
  const currentIds = new Set()
  const retiredIds = new Set(['gamer-ai'])
  for (const plugin of registry?.plugins || []) {
    if (retired(plugin)) { retiredIds.add(plugin.id.toLowerCase()); continue }
    currentArchives.add(`${plugin.id}-${plugin.version}.gplugin`.toLowerCase())
    currentIds.add(plugin.id.toLowerCase())
  }
  for (const id of currentIds) if (id !== 'gamer-ai') retiredIds.delete(id)
  return {
    blocked(path) {
      const normalized = normalizedPublicPath(path)
      if (normalized === null) return true
      const parts = normalized.split('/')
      return (parts[0] === 'plugin-ui' && retiredIds.has(parts[1])) ||
        (((parts[0] === 'plugins' && parts.length > 1) || normalized.endsWith('.gplugin')) &&
          (parts.length !== 2 || parts[0] !== 'plugins' || !currentArchives.has(basename(normalized))))
    },
    registry: registry ? { ...registry, plugins: (registry.plugins || []).filter(plugin => !retired(plugin)) } : null,
  }
}

function relativePublicFile(publicDir, file) {
  const source = existsSync(publicDir) ? realpathSync(publicDir) : resolve(publicDir)
  const target = existsSync(file) ? realpathSync(file) : resolve(file)
  const path = relative(source, target)
  return !path || (path !== '..' && !path.startsWith(`..${sep}`) && !isAbsolute(path)) ? path : null
}

function publicRequestPath(decoded, publicDir, devRoot) {
  const path = '/' + decoded.replace(/\\/g, '/').split('/').filter(Boolean).join('/')
  if (devRoot && path.startsWith('/@id/__x00__')) {
    // Vite's pure virtual module IDs contain ':' but never name disk files.
    // Keep traversal/alias checks; only the virtual namespace allows colons.
    if (normalizedPublicPath(path.replace(/:/g, '_')) === null) throw new Error('Invalid virtual module path')
    return null
  }
  if (devRoot && path.startsWith('/@fs/')) {
    const tail = path.slice('/@fs/'.length)
    const file = process.platform === 'win32' ? tail : `/${tail}`
    // Windows drive letters are valid in Vite filesystem URLs. ADS, trailing
    // dots/spaces, short names and dot segments elsewhere remain forbidden.
    if (!isAbsolute(file) || normalizedPublicPath(file.replace(/^[a-z]:\//i, '/')) === null) {
      throw new Error('Invalid filesystem module path')
    }
    return relativePublicFile(publicDir, file)
  }
  const normalized = normalizedPublicPath(path)
  if (normalized === null) throw new Error('Invalid request path')
  if (devRoot) {
    // Vite also serves public files through their source-root /public/... URL.
    // Apply the same policy to every disk alias, including existing symlinks.
    const publicFile = relativePublicFile(publicDir, resolve(devRoot, `.${path}`))
    if (publicFile !== null) return publicFile
  }
  return normalized
}

export function filterPublicAssets(publicDir, { devRoot } = {}) {
  return (req, res, next) => {
    let path
    try {
      // Incoming HTTP URLs are origin-form paths. URL(raw, base) would treat
      // a leading // as an authority and hide the real static path from us.
      if (typeof req.url !== 'string' || !req.url.startsWith('/')) throw new Error('Invalid request path')
      path = publicRequestPath(decodeURIComponent(req.url.split(/[?#]/, 1)[0]), publicDir, devRoot)
      if (path === null) { next(); return }
    }
    catch { res.statusCode = 400; res.end(); return }
    try {
      const policy = publicAssetPolicy(publicDir)
      if (policy.blocked(path)) { res.statusCode = 404; res.end(); return }
      if (normalizedPublicPath(path) === 'registry.json' && policy.registry) {
        res.setHeader('Content-Type', 'application/json; charset=utf-8')
        res.end(JSON.stringify(policy.registry))
        return
      }
      next()
    } catch (error) { next(error) }
  }
}

export function copyCurrentPublicAssets(publicDir, outDir) {
  const source = resolve(publicDir)
  const target = resolve(outDir)
  const within = relative(source, target)
  const sourceWithin = relative(target, source)
  const contained = path => !path || (!path.startsWith(`..${sep}`) && path !== '..' && !isAbsolute(path))
  if (contained(within) || contained(sourceWithin)) {
    throw new Error('Public output must be outside its source snapshot')
  }
  const policy = publicAssetPolicy(source)
  function copy(directory, path = '') {
    if (lstatSync(directory).isSymbolicLink()) throw new Error('Public snapshots cannot contain symlinks')
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const name = path ? `${path}/${entry.name}` : entry.name
      if (policy.blocked(name)) continue
      const from = resolve(source, name)
      const to = resolve(target, name)
      if (entry.isSymbolicLink()) throw new Error('Public snapshots cannot contain symlinks')
      if (entry.isDirectory()) copy(from, name)
      else if (entry.isFile()) {
        mkdirSync(dirname(to), { recursive: true })
        if (name.toLowerCase() === 'registry.json' && policy.registry) writeFileSync(to, JSON.stringify(policy.registry, null, 2) + '\n')
        else copyFileSync(from, to)
      }
    }
  }
  if (existsSync(source)) copy(source)
}

export function currentPublicAssetsPlugin() {
  let config
  return {
    name: 'gamer-current-public-assets',
    config: () => ({ build: { copyPublicDir: false } }),
    configResolved(value) {
      config = value
      if (value.build.copyPublicDir !== false) throw new Error('Unfiltered public copying is disabled')
    },
    configureServer(server) {
      if (!server.config.publicDir) return
      server.middlewares.use(filterPublicAssets(server.config.publicDir, { devRoot: server.config.root }))
    },
    configurePreviewServer(server) {
      server.middlewares.use(filterPublicAssets(resolve(server.config.root, server.config.build.outDir)))
    },
    writeBundle() {
      if (config.publicDir) copyCurrentPublicAssets(config.publicDir, resolve(config.root, config.build.outDir))
    },
  }
}
