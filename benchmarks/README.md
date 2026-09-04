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

## 2026-09-04 真机补测（scrcpy 连接 + WebRTC 稳定性）

- 测量环境：Redmi 25079RPDCC（USB adb，镜像主屏 3008x1880），主树 release
  profile，用户 server（8443）空闲并存（scrcpy 多客户端）。
- `scrcpy_connect_p95_ms` = **829.153**（p50=750.2，5 迭代 5 成功 0 兜底）：
  生产入口 `ScrcpySession::connect` 发起至收到首帧视频数据计时（镜像会话先按
  生产同款唤醒屏幕，计时不含唤醒）；三次运行 p95 829~974ms。
- `webrtc_stability` = **45s 回环 317 帧 / 971 包 / 0 停顿 / max_gap 515ms**：
  进程内 webrtc-rs 一对 peer connection 内存信令互连，推送端按生产 pusher
  语义（关键帧前独立参数集 + 静止期 500ms 补帧）转发真实 scrcpy H.264 流，
  接收端统计 >1s 无包为 stall；浏览器端解码渲染与真实网络未覆盖。
- 测试：`server/src/phase0_tests.rs` `android_bench` 模块
  `phase0_android_scrcpy_connect_first_frame_latency_p50_p95` /
  `phase0_android_webrtc_loopback_stability_45s`，`GAMER_PHASE0_ANDROID=1`
  opt-in（默认全跳过）；`GAMER_PHASE0_ANDROID=1 cargo test --release -- \
  --ignored --nocapture phase0_android_` 复现。模块内 `DEVICE_LOCK` 串行
  （并行会互拆 reverse 隧道产生短暂孤儿 scrcpy server）；结束后轮询校验设备
  无 `app_process` 残留、reverse 隧道清空、屏幕恢复熄灭。
