# Local AI automation acceptance

Final 2026-10-05 run: **23/23 checks passed** using host 0.3.0 and runtime
contract 2. See the [focused result summary](ai-automation-local-results.md) and
[complete machine-readable result](ai-automation-local-results.json).

`tools/verify-ai-automation-local.py` is a Python 3.11+ standard-library HTTP
acceptance runner. It starts an already-built Gamer server with a new data
directory and a loopback-only ephemeral port, authenticates normally, installs
current plugins through the production router, and exercises the actual sample
parser, generation service, visual YAML parser, shared interpreter, native
functions, NCC matcher, revision transactions, and public trace routes.

It does not build, publish, change a production configuration, connect to ADB,
launch a browser, or call a real model. Fixture names, images, goals, events,
recording metadata and model responses are all synthetic. A deliberately
nonexistent ADB executable prevents device discovery/control. Update polling is
disabled in this isolated test instance.

## Prerequisites and execution

1. Build the current server and current YAML WASM guest with the repository's
   documented Rust toolchain. Componentize the guest using its existing
   `componentize` binary, then package the YAML, video and AI plugins using
   `plugins/sdk/plugin-packer`. Mirror `plugins/build.ps1` and use an isolated
   artifact output directory. Do not replace a tracked market registry.
2. Use freshly built plugin UI assets if running browser acceptance too. The
   HTTP runner does not claim to test the packaged UI.
3. Ensure `ffmpeg` is on `PATH` and the repository's scrcpy server JAR exists. The
   JAR is a startup requirement only; no device is connected.
4. Run, adapting artifact paths to your machine:

```sh
python tools/verify-ai-automation-local.py \
  --server server/target/debug/gamer-server \
  --plugin-archive gamer-yaml=/tmp/gamer-plugins/gamer-yaml.gplugin \
  --plugin-archive gamer-video=/tmp/gamer-plugins/gamer-video.gplugin \
  --plugin-archive gamer-ai=/tmp/gamer-plugins/gamer-ai.gplugin \
  --output /tmp/gamer-local-acceptance
```

Alternatively, pass `--yaml-component /tmp/gamer-yaml.component.wasm` instead of
all three `--plugin-archive` arguments. The runner then makes isolated unsigned
ZIP archives from that fresh component, the current manifests and existing
`dist/ui` files, matching the official packer archive shape. This requires no
packer compilation or registry access. The real installer still validates every
archive; this option does not bypass lifecycle or permission checks.

For a separate supported-browser review, add `--serve-seconds 1200`. After the
checks it keeps the same isolated server running, writes a mode-0600 local
`browser-access.json` containing the random test login, and waits at most the
requested duration (maximum 1800 seconds). Creating `browser.done` in the output
directory stops it early. The access file is removed on shutdown and is not
included in the report. Never share that file as an artifact. This option does
not launch a browser or change what the HTTP tests claim.

The executable is copied into the isolated directory before launch and removed
after the isolated process has exited; its SHA-256 remains in the report. A later
build can replace the original binary without disrupting browser review. Use
`--build-label` to record which source/build snapshot the caller supplied.

The output directory must be empty. If omitted, the runner creates a fresh
system temporary directory. It retains `report.json`, a redacted `server.log`,
the generated archives and the isolated test data for diagnosis. It prints the
report location and exits nonzero for any failed assertion. The server and local
provider shut down even when a check fails; subprocess and request waits are
bounded. Secrets are random local-only test values and are removed from logs and
reports. The output directory is private to its owner. Delete the temporary
output directory when it is no longer useful.

`--skip-model-stub` intentionally skips the model-protocol portion and
`--skip-clip-replay` skips archived-video runtime replay. Either is a partial run,
not complete acceptance, and is recorded in the report. The runner refuses archives whose parsed
manifest differs from the current source manifest and records server/archive/
WASM SHA-256 hashes in its report. This verifies what was tested; producing a
fresh WASM build remains the caller's responsibility.

## Fixture execution settings

The synthetic demonstrations include real visual evidence through 1000ms. The
runner reads and asserts the isolated YAML plugin's actual production defaults:
`before_click_ms: 300`, `after_click_ms: 300`, and
`default_timeout_secs: 10`. It does not replace those delays with a validator-only
shortcut. The report records the settings that the production runner and offline
validator share. A further assertion changes the real setting after validation
and requires final save to reject the outdated execution configuration, then
restores the production defaults and revalidates.

## Assertions

- Real login is necessary; a clean server has no devices
- Actual synthetic MP4 import, indexed frame extraction and `sample.create`
  preserve a nonzero original capture PTS independently of normalized media PTS
- Sample creation includes genuine before/after frames, supports both PNG-only
  and clip-inclusive exports, and a portable archive remains readable after its
  synthetic recording metadata is removed
- A separate lossless 20fps clip fixture uses PNG anchors extracted by the real
  media API. After removing the original media and fixture files, the validator
  must observe actual archived-video frames absent from the PNG manifest, retain
  their original media/index/PTS identity, and repeat deterministically. A rehashed
  PNG contradicting the archived clip must fail rather than override the video
- The authoritative portable parser accepts canonical sample manifests, rejects
  tampered files, and round-trips bytes into another package
- One candidate passes A and C with confirmation present and B with it absent;
  C moves targets to different coordinates. Real NCC events and consumed action
  counts are asserted, and a repeat run has the exact same validation report
- No running video plugin is needed for portable sample replay
- No AI plugin is needed for editing/source validation or sample replay;
  generation readiness and `generation.start` fail closed without AI
- Wrong action, missing template, wrong completion template, missing `finish`,
  mandatory confirmation absent only in B, and a negative END after an earlier
  apparent success cannot become validated saves. Reports retain all samples;
  a draft does not create a production automation resource
- Candidate revision conflicts, edit/settings-driven report invalidation, atomic
  script/template save, joint rollback and concurrent resource-change rejection
- A local deterministic Responses server handles the genuine nonce/image/tool
  capability-probe protocol. It reads the first pixel of synthetic probe images
  and returns a fixed proposal containing real sample crop references
- Production generation runs that proposal through the real validator, including
  clip-backed validation with the original media removed; bad
  proposals reach a bounded retry limit. The retry fixture explicitly allows enough
  synthetic token budget for two requests; a separate 2048-token fixture must stop
  before making any model request. A proposal cannot remove a selected
  failing sample. Cancellation after submission marks unreported usage unknown,
  blocks another automatic provider request, and still permits manual validation.
  Cancellation and a configuration change
  during a deliberately held provider response cannot apply the late result or
  create candidates in another package. Disabling AI also discards its late result,
  while ordinary sample validation still works
- Held-out synthetic D is created only after generation and never sent to the
  provider. Manual validation reuses byte-identical generated template pixels and
  the exact YAML. Crop-source samples are retained solely for the existing crop
  provenance API, while D's result is reported separately. Original A/B/C
  selection stays unchanged. This is synthetic replay coverage, not evidence of
  real-model generalization
- Trace route authentication, missing evidence and invalid retention responses

Every model response is a test double. A successful local probe demonstrates
protocol plumbing and version binding, **not image understanding, autonomous
model quality, real-provider reliability or real cost accounting**.

## Intentionally separate acceptance

Real Android/CDP recording and timestamp alignment, actual game input and
lifecycle, real-model behavior/cost, and live-target Trace image capture remain
unverified by this runner. Host/router Rust tests cover synthetic Trace storage,
authentication, expiration and image behavior separately. Supported-browser UI
acceptance is a separate pass; this tool never starts Chromium from a shell.

## 本地 Jpegli 构建

宿主新增 BSD 许可的 `jpegli` 和 `jpegli-sys`，Google 原生编码器静态链接进后端。构建环境需要 CMake 和 C++ 编译器，运行端无需压缩网站或压缩工具。Windows GNU 构建使用 `CMAKE_GENERATOR=MinGW Makefiles`；Windows MSVC 可使用开发者编译环境中的单配置 Ninja 生成器。`jpegli-sys` 在开发档也使用优化构建，防止未优化的 Windows GNU SIMD 路径崩溃。

模型输入保留整图分辨率，附加录制点击周围 320 像素的无损局部图。原图、正式 PNG 模板与回放来源保持不变。素材前检、图片准备、模型请求和回放共用任务时限，图片在同一修正任务中只准备一次。HTTP 验收包含 JPEG 输入、视图坐标转换和无效 JSON 输出的已知用量保留；合成供应商测试不代表真实模型识别通过。

累计 Token 默认不限制（0），有限预算与未知用量保护仍可显式使用。完整多图模型请求使用现有 SSE 适配器，原始源码裁图不受请求编码影响。图像与本机集成结果见 [Jpegli 验收记录](ai-automation-jpegli-results.json)。
