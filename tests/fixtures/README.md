# Phase 0 最小兼容夹具

这些文件是跨后端和前端的代表性合法输入，不是运行时数据目录的替代品：

- `scripts/phase0_smoke.yaml`：覆盖启动/停止应用、tap、swipe、key、text、wait、日志和模板 find 的脚本。
- `keymaps/phase0_combat.yaml`：覆盖 tap、swipe、raw_key、hold 四种持久化动作。
- `templates/`：同一帧中三个不同尺寸、不同区域编码的灰度模板。
- `screenshots/match-success.png` / `match-failure.png`：确定命中与确定未命中的 1080×1920 截图。
- `tasks/phase0_daily.json`：无参数典型定时任务请求样本。

二进制文件沿用现有固定图像基准的真实模板；失败截图由固定的黑色 lavfi 源生成。修改夹具时必须同步
`manifest.json`，并运行 Rust/Vue 的 Phase 0 护栏。
