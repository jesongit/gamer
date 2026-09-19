param([switch]$LiveApi)
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
function Invoke-Check([string]$Directory, [scriptblock]$Command) {
    Push-Location $Directory
    try { & $Command; if ($LASTEXITCODE -ne 0) { throw "检查失败（退出码 $LASTEXITCODE）" } }
    finally { Pop-Location }
}
Invoke-Check (Join-Path $repo 'web') { pnpm test:run }
Invoke-Check (Join-Path $repo 'server') { cargo test }
Invoke-Check (Join-Path $repo 'server') { cargo test --manifest-path guests/yaml-interp/Cargo.toml }
# 独立输出目录，不替换正在提供服务的 web-dist。
Invoke-Check (Join-Path $repo 'web') { pnpm exec vite build --outDir ../server/target/yaml-acceptance/web-dist }
if ($LiveApi) { Invoke-Check $repo { node tools/yaml-tests/live-api.mjs } }
