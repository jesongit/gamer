# Phase 0 baseline

`baseline.json` 由 `tools/generate-phase0-baseline.ps1` 生成，不手填性能数字。

```powershell
# 只记录已有 release 工件
powershell -ExecutionPolicy Bypass -File tools\generate-phase0-baseline.ps1 -BuildRelease

# 用固定 PNG/H.264 夹具采集真实 p95（需要 ffmpeg）
powershell -ExecutionPolicy Bypass -File tools\generate-phase0-baseline.ps1 -RunPerf -BuildRelease

# CI/提交前只校验基线结构
powershell -ExecutionPolicy Bypass -File tools\generate-phase0-baseline.ps1 -ValidateOnly
```

设备连接、空闲 RSS/CPU、真实截图、scrcpy 会话和 WebRTC 稳定性没有离线可比值，
在 JSON 中以 `value: null`、`status: not_measured` 和原因保留；后续阶段只能用同一
入口补写实测结果，不能用 0 或估计值占位。

## 2026-09-03 回填记录（worktree 实测）

- 测量环境：隔离 worktree `D:/code/gamer-bench`（HEAD `e1e542c`，独立
  `CARGO_TARGET_DIR=target-bench`），Ryzen 7 5800X / 32GB / Windows 10.0.26200，
  release profile（`cargo build --release`，默认 feature）。
- 已测指标：`screenshot_p95_ms`（340.218，固定 H.264 夹具 `decode_latest_png`）、
  `find_p95_ms` / `match_many_p95_ms`（109.133，夹具 `find_round`，NCC 代理）、
  `gop_cache_peak_bytes`（1186681，夹具单 GOP 30 帧喂入 FrameCache）、
  `db_log_write_p95_ms`（0.554，8 并发 × 250 条 critical 日志端到端 2000 样本）、
  `server_idle_rss_mb`（13.094）与 `server_idle_cpu_percent`（0.0，60s×3s 采样中位数）。
  性能项经 `generate-phase0-baseline.ps1 -RunPerf -Iterations 30`（pwsh）采集，
  其余两项由仅存在于该 worktree 的临时基准测试补测，详情见 JSON 内 `backfill` 与
  各 metric 的 `reason`/`source_test`。
- `keymap-latency.json`：Phase 7 要求的 keymap native vs WASM 延迟基线
  （native dispatch 亚微秒级；wasmtime 组件链路 p50=3µs / p95=3.7µs），由
  `hot_path_dispatch_p95_is_bounded` 与
  `real_wit_input_to_native_chain_p95_stays_bounded`（`--features wasm-runtime`）
  两个测试实测输出登记。
- 仍为 null 的项（`scrcpy_connect_p95_ms`、`webrtc_stability`）需要真实 Android
  设备 / 浏览器 peer，status 标为 `requires_hardware`，待真机环境补测。
- 已知坑：`-RunPerf` 解析 PERF 行的正则是贪婪匹配，会被 frames 分段基准行尾的
  `cpu_*_us=未实测` 占位段捕获，PowerShell 5.1 下直接抛错中断；回填时在 worktree
  改为显式字段匹配并用 **pwsh 7** 执行。主树脚本修复前建议一律用 pwsh 跑 `-RunPerf`。
