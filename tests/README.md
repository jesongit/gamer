# Phase 0 测试边界

`tests/fixtures/` 是跨 Rust/Vue 共享的最小兼容样本。普通测试只读取这些本地文件，
不启动 Android、ADB、scrcpy、ffmpeg 进程，也不依赖网络或真实 WebRTC 对端。

离线护栏入口：

```powershell
powershell -ExecutionPolicy Bypass -File tools\run-phase0.ps1
```

外部依赖边界：

- `cargo test` 覆盖控制协议解析、scrcpy/帧缓存/WebRTC 协议内核和状态机；这些测试不声称设备或浏览器链路可用。
- 固定 PNG/H.264 基准需要本机 `ffmpeg`，通过 `tools\run-perf-benchmark.ps1` 显式运行；缺失时失败，不降级为伪通过。
- 真实设备连接、scrcpy 会话、截图和浏览器 WebRTC 稳定性属于集成/人工验收，不在默认 CI 中运行；运行参数与证据必须单独记录。

夹具文件的清单、用途和 SHA-256 由 `tests/fixtures/manifest.json` 固定，护栏会拒绝缺失、越界路径或哈希漂移。
