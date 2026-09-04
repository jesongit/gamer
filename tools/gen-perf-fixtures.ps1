# gen-perf-fixtures.ps1 —— PERF-001 可重复基准夹具一键生成器
#
# 用法（仓库根目录执行，或任意位置直接跑）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\gen-perf-fixtures.ps1
# 行为：删除并重建 server/testdata/perf/ 全部内容（幂等，可反复重跑）。
# 前置条件：PATH 上有 ffmpeg 与 ffprobe。
#
# 工具链选型说明：
#   本脚本优先使用本机 ffmpeg（构建需含 libx264 与 geq 滤镜；作者环境为 gyan.dev 9.0.1-full，
#   `ffmpeg -version` 可验证）。若本机无 ffmpeg，可临时用容器兜底，代价：
#   1) 镜像内是另一 ffmpeg 构建 => 二进制产物哈希与 manifest 中「同版本可复现」声明脱钩，
#      仅保目录结构稳定；
#   2) jrottenberg/ffmpeg:6-alpine 入口即 ffmpeg 参数、且镜像内无 ffprobe，
#      IDR 位点校验与 PNG/模板尺寸断言不可运行（需注释掉相关 Assert）。
#   参考命令（PowerShell，在仓库根目录；实际生成参数以 $encArgs 为准）：
#     docker run --rm -v "${PWD}:/w" -w /w jrottenberg/ffmpeg:6-alpine <encArgs...> /w/server/testdata/perf/stream.h264

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# ---------- 定位仓库 ----------
$root = Split-Path -Parent $PSScriptRoot
$outDir = Join-Path $root 'server\testdata\perf'

function Invoke-FfmpegTool {
    param([string]$Exe, [string[]]$Argv)
    # PS5.1 坑：参数名不能叫 $Args（与自动变量冲突导致绑定恒空）；原生 stderr 经 2>&1 变 ErrorRecord，Stop 偏好下直接炸 => 局部 Continue + 转字符串
    $ErrorActionPreference = 'Continue'
    $out = & $Exe @Argv 2>&1 | ForEach-Object { $_.ToString() }
    if ($LASTEXITCODE -ne 0) {
        $tail = ($out | Select-Object -Last 15) -join "`n"
        throw "$Exe failed (exit $LASTEXITCODE):`n$tail"
    }
    return ,@($out)
}

function Assert {
    param([bool]$Cond, [string]$Msg)
    if (-not $Cond) { throw "ASSERT FAILED: $Msg" }
}

function Write-Utf8NoBom {
    param([string]$Path, [string]$Content)
    [System.IO.File]::WriteAllText($Path, $Content, (New-Object System.Text.UTF8Encoding($false)))
}

# ---------- 依赖检查 ----------
foreach ($tool in @('ffmpeg', 'ffprobe')) {
    $cmd = Get-Command $tool -ErrorAction SilentlyContinue
    Assert ($null -ne $cmd) "$tool not found on PATH. Install a libx264-enabled ffmpeg build (see header comment for docker fallback)."
}
$ffVersionFull = (Invoke-FfmpegTool -Exe 'ffmpeg' -Argv @('-hide_banner', '-version'))[0]
Write-Host "[deps] $ffVersionFull"

# ---------- 夹具常量（单一事实来源） ----------
$W = 1080; $H = 1920          # 竖屏帧尺寸
$FPS = 30; $DUR = 3           # 90 帧
$GOP = 30                     # IDR 落在帧序号 0/30/60（第 1/31/61 帧）
$QP = 22                      # 恒定 QP（CQP）：无码率反馈漂移，码率稳定

# 三个目标矩形：declared 为设计坐标（允许越界）；crop 为对关键帧的实际裁切（钳制后）
# 1) 大按钮 >=200x200；2) 小文本块 ~60x40；3) 右下角越界矩形（考验钳制路径，可见部分 40x40）
$rects = @(
    @{ name='perf_btn_primary'; role='big button';      color='0xFF5533';
       declared=@{x=390;y=700;w=300;h=220}; region_suffix='361_365_639_479'; suffix_form='per_mille_xyxy' },
    @{ name='perf_txt_status';  role='small text block'; color='0x35E6FF';
       declared=@{x=140;y=420;w=60;h=40};  region_suffix='130_219_185_240'; suffix_form='per_mille_xyxy' },
    @{ name='perf_corner_menu'; role='bottom-right edge rect overflows frame (clamped to 40x40 visible)'; color='0x7DFF4D';
       declared=@{x=1040;y=1880;w=80;h=80}; region_suffix='dr'; suffix_form='half_code_dr' }
)
$decoyBoxes = @(   # 低对比干扰块（暗色，避免被当成匹配目标）
    'drawbox=x=110:y=180:w=200:h=150:color=0x27324A:t=fill',
    'drawbox=x=770:y=300:w=180:h=200:color=0x33284A:t=fill',
    'drawbox=x=480:y=1450:w=260:h=160:color=0x1F3A2E:t=fill'
)

# geq 伪随机纹理：公式只含 X/Y/N 整数运算 ⇒ 固定即可重建；
# N 相关项让每帧有微小确定差异（P 帧非空、三个关键帧互相可区分）
$geqExpr = ("geq=r='12+mod(31*X+17*Y+53*(X/32)*(Y/32),23)+mod(N*5+(X+Y)/2,7)'" +
            ":g='14+mod(37*X+11*Y+59*(X/24)*(Y/40),25)+mod(N*7+X,9)'" +
            ":b='18+mod(23*X+41*Y+67*(X/48)*(Y/28),27)+mod(N*9+Y,8)'")
$targetBoxes = @($rects | ForEach-Object {
    $d = $_.declared
    "drawbox=x=$($d.x):y=$($d.y):w=$($d.w):h=$($d.h):color=$($_.color):t=fill"
})
$encodeVf = (@('format=rgb24', $geqExpr) + $decoyBoxes + $targetBoxes + @('format=yuv420p')) -join ','

# 单输入单输出：仅用 -vf，无需 -map
$encArgs = @(
    '-hide_banner','-loglevel','error','-y',
    '-f','lavfi','-i',"color=c=0x121A26:s=${W}x${H}:r=${FPS}:d=$DUR",
    '-vf',$encodeVf,
    '-c:v','libx264','-preset','slow','-qp',"$QP",
    '-bf','0','-threads','1','-g',"$GOP",
    '-x264-params',"keyint=${GOP}:min-keyint=${GOP}:scenecut=0:open_gop=0",
    '-an','-f','h264'
)

$extractBase = @('-hide_banner','-loglevel','error','-y')

# ---------- 重建目录 ----------
if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $outDir 'templates') | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $outDir 'tmpl-rgb') | Out-Null

# ---------- 1) 编码 stream.h264 ----------
Write-Host '[1/4] encoding stream.h264 (90 frames, GOP=30, CQP)...'
Invoke-FfmpegTool -Exe 'ffmpeg' -Argv ($encArgs + @((Join-Path $outDir 'stream.h264'))) | Out-Null

# 校验 IDR 位点与总数（bf=0 ⇒ 解码顺序即显示顺序，行号即帧号；csv 里 I 行可能附带 SEI 描述，按前缀匹配）
$rows = Invoke-FfmpegTool -Exe 'ffprobe' -Argv @(
    '-hide_banner','-loglevel','error',
    '-select_streams','v:0','-show_frames',
    '-show_entries','frame=pict_type',
    '-of','csv=p=0',(Join-Path $outDir 'stream.h264'))
$idr = @()
for ($i = 0; $i -lt @($rows).Count; $i++) {
    if ("$($rows[$i])".TrimStart() -match '^I(\b|,|$)') { $idr += $i }
}
Assert (($idr.Count -eq 3) -and ($idr[0] -eq 0) -and ($idr[1] -eq 30) -and ($idr[2] -eq 60)) "IDR frames expected at 0/30/60, got: $($idr -join ',')"
$totalFrames = @($rows | Where-Object { $_ -match '^[IPB]' }).Count
Assert ($totalFrames -eq 90) "expected 90 frames, probed $totalFrames"

# ---------- 2) 导出三张关键帧 PNG ----------
Write-Host '[2/4] extracting keyframes...'
$keyframes = @(0, 30, 60)
$keyFiles = @()
foreach ($idx in $keyframes) {
    $png = Join-Path $outDir ("keyframe_{0:D3}.png" -f ($idx + 1))
    Invoke-FfmpegTool -Exe 'ffmpeg' -Argv ($extractBase + @(
        '-i',(Join-Path $outDir 'stream.h264'),
        '-vf',"select='eq(n,$idx)'",'-frames:v','1',$png)) | Out-Null
    $dim = (Invoke-FfmpegTool -Exe 'ffprobe' -Argv @(
        '-hide_banner','-loglevel','error','-select_streams','v:0',
        '-show_entries','stream=width,height,pix_fmt',
        '-of','csv=p=0',$png))[0]
    Assert ($dim -match "^${W},${H},rgb24") "keyframe $($png) unexpected stream info: $dim"
    $keyFiles += $png
}

# ---------- 3) 裁模板（灰度 + 彩色双版本） ----------
Write-Host '[3/4] cropping templates...'
$src = $keyFiles[0]   # 内容逐帧相同（仅 N 纹理微变），模板统一取自第一张关键帧
foreach ($r in $rects) {
    $d = $r.declared
    $cx2 = [Math]::Min($d.x + $d.w, $W); $cy2 = [Math]::Min($d.y + $d.h, $H)
    $ox = [Math]::Max(0, $d.x); $oy = [Math]::Max(0, $d.y)
    $cw = $cx2 - $ox; $ch = $cy2 - $oy
    Assert (($cw -gt 0) -and ($ch -gt 0)) "rect $($r.name) fully clipped?"
    $fname = "$($r.name)#$($r.region_suffix).png"
    Invoke-FfmpegTool -Exe 'ffmpeg' -Argv ($extractBase + @(
        '-i',$src,'-vf',"crop=${cw}:${ch}:${ox}:${oy}",'-frames:v','1',
        '-pix_fmt','rgb24',(Join-Path $outDir "tmpl-rgb\$fname"))) | Out-Null
    Invoke-FfmpegTool -Exe 'ffmpeg' -Argv ($extractBase + @(
        '-i',$src,'-vf',"crop=${cw}:${ch}:${ox}:${oy},format=gray",'-frames:v','1',
        '-pix_fmt','gray',(Join-Path $outDir "templates\$fname"))) | Out-Null
    foreach ($sub in @("templates\$fname","tmpl-rgb\$fname")) {
        $pi = (Invoke-FfmpegTool -Exe 'ffprobe' -Argv @(
            '-hide_banner','-loglevel','error','-select_streams','v:0',
            '-show_entries','stream=width,height,pix_fmt',
            '-of','csv=p=0',(Join-Path $outDir $sub)))[0]
        $expectPix = if ($sub.StartsWith('tmpl-rgb')) { 'rgb24' } else { 'gray' }
        Assert ($pi -eq "$cw,$ch,$expectPix") "template ${sub} unexpected: $pi"
    }
    $r | Add-Member -NotePropertyName clamped -NotePropertyValue @{x=$ox;y=$oy;w=$cw;h=$ch}
    $r | Add-Member -NotePropertyName center_of_clamped -NotePropertyValue @{
        x=[int](($ox*2+$cw)/2); y=[int](($oy*2+$ch)/2)}
    $r | Add-Member -NotePropertyName template_file -NotePropertyValue "templates/$fname"
    $r | Add-Member -NotePropertyName rgb_twin_file -NotePropertyValue "tmpl-rgb/$fname"
}

# ---------- templates.txt（命名风格对齐 web/src/script-language/fixtures/templates.txt） ----------
$tplLines = @($rects | ForEach-Object { "$($_.name)#$($_.region_suffix).png" } | Sort-Object)
$tplText = ($tplLines -join "`n") + "`n"
Write-Utf8NoBom -Path (Join-Path $outDir 'templates.txt') -Content $tplText

# ---------- 4) manifest.json（内容全量：参数/位点/命令/版本/sha256） ----------
Write-Host '[4/4] manifest + README...'
$hashes = @{}
Get-ChildItem $outDir -Recurse -File | ForEach-Object {
    $rel = $_.FullName.Substring($outDir.Length + 1).Replace('\','/')
    $hashes[$rel] = @{
        sha256 = (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant()
        bytes  = $_.Length }
}

$manifest = [ordered]@{
    schema_version = 1
    purpose = 'PERF-001 repeatable benchmark fixtures: fixed-GOP H.264 stream, keyframe screenshots, template crops'
    reproducibility = 'byte-stable when regenerated by the SAME ffmpeg build (encode pins -threads 1 + CQP + static scene); across builds/pixels may differ but structure is stable. Records below pin the exact toolchain.'
    stream = [ordered]@{
        container = 'raw Annex-B H.264'
        width = $W; height = $H
        fps = $FPS; duration_s = $DUR
        frame_count = 90
        gop = $GOP
        idr_frame_indices_zero_based = @(0, 30, 60)
        idr_frames_one_based = @(1, 31, 61)
        rate_control = 'CQP (constant QP)'
        qp = $QP
        b_frames = 0
        encoder_threads = 1
        content = 'dark base color + deterministic geq pseudo-random texture (integer formula over X,Y,N) + 3 dim decoy blocks + 3 bright target rectangles drawn by drawbox at known coordinates'
    }
    tooling = [ordered]@{
        ffmpeg_version_line = $ffVersionFull
        generator_script = 'tools/gen-perf-fixtures.ps1'
    }
    keyframes = @($keyframes | ForEach-Object {
        [ordered]@{ zero_based_index = $_; one_based_frame = ($_ + 1);
                    file = ('keyframe_{0:D3}.png' -f ($_ + 1)); width = $W; height = $H }
    })
    rects = @($rects | ForEach-Object {
        [ordered]@{
            name = $_.name; role = $_.role; fill_color_hex = ('#' + $_.color.Substring(2))
            declared = $_.declared
            clamped = $_.clamped
            center_of_clamped = $_.center_of_clamped
            region_suffix = $_.region_suffix
            region_suffix_form = $_.suffix_form
            gray_template = $_.template_file
            rgb_template = $_.rgb_twin_file
        }
    })
    templates_txt = 'templates.txt (sorted, LF, no BOM; entries use #suffix region metadata: two per-mille xyxy + one half-code dr)'
    files = @($hashes.Keys | Sort-Object | ForEach-Object {
        [ordered]@{ path = $_; bytes = $hashes[$_].bytes; sha256 = $hashes[$_].sha256 }
    })
    commands = [ordered]@{
        encode_stream = ('ffmpeg ' + ($encArgs -join ' '))
        extract_keyframe_template = 'ffmpeg -hide_banner -loglevel error -y -i stream.h264 -vf "select=''eq(n,{IDX})''" -frames:v 1 keyframe_{NNN}.png  # IDX in {0,30,60}'
        crop_rgb_template = 'ffmpeg -hide_banner -loglevel error -y -i keyframe_001.png -vf "crop={W}:{H}:{X}:{Y}" -frames:v 1 -pix_fmt rgb24 tmpl-rgb/{NAME}#{SUFFIX}.png'
        crop_gray_template = 'ffmpeg -hide_banner -loglevel error -y -i keyframe_001.png -vf "crop={W}:{H}:{X}:{Y},format=gray" -frames:v 1 -pix_fmt gray templates/{NAME}#{SUFFIX}.png'
        verify_idr = 'ffprobe -show_frames -show_entries frame=pict_type,coded_picture_number -of csv=p=0 stream.h264'
    }
}
$json = $manifest | ConvertTo-Json -Depth 8
Write-Utf8NoBom -Path (Join-Path $outDir 'manifest.json') -Content $json

# ---------- README.md ----------
$readme = @'
# perf fixtures（OPTIMIZATION_PLAN / PERF-001）

确定性生成的基准夹具，由 `tools/gen-perf-fixtures.ps1` 一键重建（删除本目录后重跑脚本即可，
条目结构稳定；跨 ffmpeg 构建二进制哈希可能变化，`manifest.json` 记录了生成时的工具链全文）。

## 资产

| 文件 | 说明 |
|---|---|
| `stream.h264` | 1080x1920 竖屏 H.264 裸流（Annex-B），90 帧、固定 GOP=30，IDR 落在第 1/31/61 帧；恒定 QP 编码（qp=22、0 B 帧、threads=1）避免码率漂移 |
| `keyframe_001/031/061.png` | 从上述三个 IDR 解出的 1080x1920 RGB 截图 |
| `templates/*.png` | 三个目标矩形的灰度裁片（与服务端模板重编码后的消费形态一致），文件名带 `#后缀` 区域元数据 |
| `tmpl-rgb/*.png` | 同一区域彩色版孪生（用于验证灰度转换一致性） |
| `templates.txt` | 模板名清单，命名风格对齐 `web/src/script-language/fixtures/templates.txt`；两种区域形态各占其一：×1000 相对坐标（`361_365_639_479`）与半区码（`dr`） |

画面内容：深色底 + 固定公式的伪随机纹理（geq，整数运算含帧号 N，逐帧微变但可复现）
+ 3 个低亮度 decoy 块 + 3 个高亮目标矩形（色彩/亮度显著唯一，供断言命中坐标）。

## 三个目标矩形（详见 manifest.json 的 rects）

| 名字 | 场景覆盖 | declared (x,y,w,h) | 裁切片（钳制后） | 区域后缀 |
|---|---|---|---|---|
| perf_btn_primary | 大按钮 ≥200x200 | 390,700,300,220 | 同左，中心 (540,810) | ×1000 坐标 |
| perf_txt_status | 小文本块 ~60x40 | 140,420,60,40 | 同左，中心 (170,440) | ×1000 坐标 |
| perf_corner_menu | 贴右下角且**越界** 80x80 → 只画出 40x40 | 1040,1880,80,80 | 1040,1880,40,40，中心 (1060,1900) | 半区码 dr |

注意：`#数字后缀` 是 ×1000 相对坐标（每段 ≤999），与绝对像素间存在 ≤1px 量化误差；
基准断言以 manifest.json 中的像素值为准。

## 后续基准（实现属后续任务，此处只约定测什么）

以 `testdata/perf` 为固定输入，分别计量：

1. **decode_latest_png 总耗时**——对应 `FrameCache` 截图路径的端到端耗时；
2. **分段耗时**——ffmpeg 进程启动 / stdin 写入 annex-b 片段 / 解码等待 / PNG 编码输出四段拆分；
3. **PNG 解码与灰度化**——用 `keyframe_*.png` 直接走 image 解码 + 灰度转换的成本；
4. **NCC 匹配**——分别在**区域**（模板名 `#后缀` 解析出的搜索区，引擎语义：
   无后缀回退全屏）与**全屏**两种搜索窗下做归一化互相关；期望命中中心即上表"中心"列，
   全屏 vs 区域的倍差即 PERF-002/003 优化的上限空间；
5. **模板读取预处理**——从磁盘读 PNG 到产出匹配器所需灰度矩阵（不含 NCC 计算）；
6. **find 整轮**——主模板 + N 个 block（block 可复用 `templates/perf_btn_primary#...` 反相裁块或
   decoy 区域裁片）按 YAML 引擎 find 语义完整一轮（一次截图 + 1+N 次 NCC）。

统计口径按 OPTIMIZATION_PLAN.md §11.1：p50 / p95 / 最大值，另记 CPU 与峰值内存；
Windows 与 Docker/Linux 至少各跑一轮。禁止把 README 中任何 `<50ms` 式定性描述当验收依据。

## 消费入口约定

Rust 基准代码统一从 CARGO_MANIFEST_DIR 相对路径取夹具：

```rust
let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/perf");
let stream = dir.join("stream.h264");
```

夹具二进制属于正式提交物，请勿被 .gitignore / LFS 规则排除。
'@
Write-Utf8NoBom -Path (Join-Path $outDir 'README.md') -Content $readme

# 把 manifest/README 也纳入最终哈希清单二次落盘（保证 files 数组完备）
$hashes2 = @{}
Get-ChildItem $outDir -Recurse -File | Where-Object { $_.Name -ne 'manifest.json' } | ForEach-Object {
    $rel = $_.FullName.Substring($outDir.Length + 1).Replace('\','/')
    $hashes2[$rel] = @{ sha256 = (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant(); bytes = $_.Length }
}
$manifest.files = @($hashes2.Keys | Sort-Object | ForEach-Object {
    [ordered]@{ path = $_; bytes = $hashes2[$_].bytes; sha256 = $hashes2[$_].sha256 }
})
Write-Utf8NoBom -Path (Join-Path $outDir 'manifest.json') -Content ($manifest | ConvertTo-Json -Depth 8)

# ---------- 摘要 ----------
Write-Host ''
Write-Host '== fixtures summary =='
Get-ChildItem $outDir -Recurse -File | Sort-Object FullName | ForEach-Object {
    $rel = $_.FullName.Substring($outDir.Length + 1)
    $h = (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant().Substring(0,16)
    '{0,-45} {1,10:N0} B  sha256:{2}...' -f $rel, $_.Length, $h
}
Write-Host ''
Write-Host 'OK: server/testdata/perf regenerated.'
