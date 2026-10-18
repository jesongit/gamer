# 生产诊断优化：本地开发与隔离验证

用户授权本地开发和测试，要求保留现有数据，稳定后再更新生产 Docker。
本轮没有发布镜像、更新生产容器、启动真实日常、操作真实设备或迁移用户数据。

## 实现

- `diagnostics` 独立配置：摘要 365 天，成功详细事件 14 天，失败/取消详细事件 30 天。
- 新策略只管理版本标记为 1 的新运行/图片；原始及未知版本记录、索引、像素不参与新清理。
  `diagnostics-policy.json` 持久保存首启旧文字日志水位，重启不重置；SQLite 保持 schema v7。
- 普通匹配、操作前后、补充观察和 OCR 在 PNG 编码前按 10 秒间隔采样。成功保留首张及最近
  最多 5 条图像记录/24 小时，失败/取消保留最长 168 小时并受容量约束。
- 最后实际匹配或 OCR 帧持续更新；根错误有独立 8 槽优先通道、两种错误图数量及容量预留。
  原始捕获时间、FrameStamp、实际模板、image_id/seq 仍然保留，不拿新截图替代旧判断帧。
- 只聚合相同执行身份下重复未命中的视觉观察，首条即时持久化，追加摘要不改已发出的 ID。
  输入、命中、错误、步骤/调用边界与 producer-owned detail 不聚合。
- 完成运行可在任务历史/运行详情原生下载诊断 ZIP。服务端临时文件及 64 KiB 流式输出，
  一次一个下载；原图最多 128 MiB，事件最多 16 MiB，超预算和到期明确标记。
- 冻结宿主构建身份、源码/模板指纹及实际 WASM SHA-256；源码正文、参数/返回值、已知敏感键不导出。
  不读取用户浏览器资料、Cookie、私密插件配置、凭据密钥或整个数据库。
- 插件通过现有 `api` SDK 出口生成下载地址，没有增加未声明宿主模块、WIT imports 或 MCP 工具。

详细规则：[DIAGNOSTICS.md](../reference/DIAGNOSTICS.md)。

## 本地检查

Windows：

- `cargo check --locked` 通过。
- `runtime_trace` 26 项通过，覆盖采样、OCR 最后判断帧、普通队列/图片/字节预算耗尽后错误图、
  成功精简、失败延长保留、旧索引及像素不变、鉴权/游标/不可跨运行读取、脱敏 ZIP 原始像素。
- `store::` 37 项通过，1 项高压基准按原标记忽略；包含旧日志水位跨重启保留、独立清理窗口、
  摘要身份在详细事件到期后仍可查、聚合次数/最高分/末次结果与游标顺序。
- 架构守卫 7 项、YAML runner adapter 7 项、config 20 项、diagnostics 单元 2 项、settings 相关 7 项通过。
- 宿主前端全量 95 文件/854 项、YAML UI 全量 12 文件/100 项通过；SDK 出口修正后相关 API/历史
  24 项、YAML 运行记录 19 项复验通过。
- 全部插件 UI 构建、宿主 `pnpm build`、`node tools/check-plugin-sdk.mjs`、`git diff --check` 通过。
- Impeccable 检测无报告项。普通浏览器工具不可用（仅提供 MCP Apps），没有完成桌面/移动视觉验收；
  未将组件测试或构建当作视觉确认。临时预览进程与 3 个临时文件已清理。

插件源码按分仓规则本地提交 `fd7619e`，父提交 `9fff23e` 保留。主仓 gitlink 更新；没有 push 或发布。
主仓仍保留本轮之前的 Docker 开发与用户已有改动，没有覆盖 `gamer.ps1` 或其既有测试变更。

## Linux 候选镜像

- 仅本地标签：`gamer:diagnostics-dev-20261018`，`linux/amd64`，Rust Release 构建。
- 本地镜像 ID：`sha256:1e238ede1b6d8737504e8a254d1047507b34b7e0e6f1aa1918d0222b3efbed42`。
- 未覆盖 `posase/gamer:v1.0`，没有上传 Docker Hub。
- `python tools/test_gamer_diagnostics_docker.py --image gamer:diagnostics-dev-20261018 --receipt server/target/diagnostics-docker-smoke.json` 通过。
- 测试使用随机命名、带专用标签的新卷与容器，不发布端口；合成的 schema v7 数据含新/旧运行、
  过期步骤、凭据标记文本与 PNG。真实 HTTP 验证 401、诊断 ZIP（2,218 bytes）、脱敏、保留周期、
  数据库完整性、旧索引 SHA-256 不变及跨多次重启的旧日志水位。测试卷任务数为 0。
- 仅删除本轮合成容器/卷，候选镜像保留供后续验收。回执为非敏感测试结果，不含凭据或用户数据。

这项 Linux 测试验证服务/HTTP/数据生命周期，图像采样路径由隔离 Rust 像素测试覆盖；没有借此声明
真实云游戏、Android、ARM64、浏览器交互、故障恢复效果或长期资源曲线已经通过。

## 生产保持原样

验证后 `gamer-production-gamer-1` 仍 healthy，镜像仍为
`sha256:d7e51ce9ca289e88ec57330538d97496c10d66bd2a8e7d93f0b4b3580acb0bb5`。
启动时间仍为 `2026-10-18T11:27:41.099728816Z`，restart count 为 0。
本轮只读确认 4 个任务仍全部 enabled、调度开启、空闲回收仍 1800 秒；生产卷未创建新策略水位文件。
真实账号的日常与长时间稳定性需后续验收，本轮不切换生产。
