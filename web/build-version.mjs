// Fail before Vite writes output if product metadata has drifted. Keep the
// browser's version tied to the frontend it actually builds so mixed deployments
// remain visible at runtime.
export function assertMatchingBuildVersion(webVersion, cargoToml) {
  let section = ''
  let serverVersion
  for (const line of cargoToml.split(/\r?\n/)) {
    const heading = line.match(/^\s*\[([^\]]+)\]\s*(?:#.*)?$/)
    if (heading) section = heading[1].trim()
    if (section === 'package') {
      const version = line.match(/^\s*version\s*=\s*"([^"]+)"\s*(?:#.*)?$/)
      if (version) { serverVersion = version[1]; break }
    }
  }
  if (!serverVersion) {
    throw new Error('[version-check] Cannot read [package].version from server/Cargo.toml')
  }
  if (webVersion !== serverVersion) {
    throw new Error(`[version-check] web/package.json version '${webVersion}' != server/Cargo.toml package.version '${serverVersion}'; synchronize product versions before building`)
  }
  return webVersion
}
