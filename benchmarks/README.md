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
