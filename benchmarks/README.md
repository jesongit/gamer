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

## keymap E2E（Phase 6，真机 opt-in）

`results/keymap-e2e.json` 由真机基准
`phase0_android_keymap_e2e_latency_native_vs_wasm` 自动生成（不手填）：

```powershell
$env:GAMER_PHASE0_ANDROID="1"
cd server; cargo test --release -- --ignored --nocapture phase0_android_keymap_e2e
```

### 测量方法

- **链路**：进程内 webrtc-rs DataChannel 客户端（浏览器替身）→ 真实 SCTP/DTLS
  DataChannel → 生产同构 control worker（单消费者串行）→ `handle_control_msg`
  → keymap（native 直通 / WASM `gamer.keymap` 组件）→ DeviceAction → scrcpy
  control socket 写。ICE/DTLS/SRTP/SCTP 全真实；**浏览器 JS 开销不在测量
  范围**（计划 8.3 允许 Browser RTT 与 Server 内部阶段分开统计）。
- **两轮同环境**：同一 scrcpy 会话 + 同一 DataChannel。Native 轮无 keymap
  实例（事件 pass-through 直通）；WASM 轮安装并启动 fixture guest，profile
  用 `raw_key` 复刻 `android_keycode` 的 W/A/S/D/Space/ShiftLeft 映射——两轮
  最终 scrcpy 控制写完全一致，差异只在映射层。
- **阶段**（`input_event` 信封可选 `trace_id`/`client_send_ts`，服务端
  `KeymapTraceRecord` 记录）：`browser_to_server`（仅进程内时钟同源可直接
  相减；真实浏览器须分开统计）、`server_receive_to_wasm_begin`、
  `wasm_execution`、`wasm_end_to_device_action`、`device_action_to_scrcpy_write`、
  `server_internal_total`、`full_chain_in_process`；`delta_wasm_minus_native_us`
  为计划 8.5 的关注点。native 链路的 wasm 阶段记为 native_mapping（即查即决，
  ≈0）。
- **场景**（计划 8.6）：普通按键（W/A/S/D/Space × 20 轮 down/up）、长按
  （每键 1s/2s 轮流）、组合键（ShiftLeft+KeyW 按住 200ms × 10）、连续 burst
  （6 轮 × 5 键 × down/up = 60 事件不间歇发送）。正确性断言：不丢事件、
  顺序一致、KeyUp 全部送达、scrcpy 写单调不降；**不断言任何延迟数值**
  （计划 8.8：先建 baseline，防 flaky）。
- **trace 门禁**：默认关闭零开销；基准在进程内安装收集器，
  `GAMER_KEYMAP_E2E_TRACE=1` 时生产路径额外以 tracing 日志输出记录。
- 设备端 Android 输入管线到游戏的延迟不在测量范围内（终点点为 scrcpy
  control write 完成），对两轮等同。
