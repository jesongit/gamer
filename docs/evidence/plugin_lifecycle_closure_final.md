# Gamer 插件生命周期与函数体系收尾验收记录

> 验收日期：2026-09-09（Asia/Shanghai）
> 执行基线：`a5a175ff06013b3ef6cda9f328eb9ecd285cfc33a`
> 计划：`docs/plans/gamer_plugin_lifecycle_function_closure_plan.md`
> 当前仓库状态：HEAD 仍为 `a5a175ff`；工作树有未提交改动。本记录不代表已发布。

## 已完成实现

- 原生动作的公开清单查询与有副作用分发已分离；插件未进入 `Running` 时，
  `automation.save_draft` 等动作在执行前拒绝且不写 Package 数据。
- 每个插件新增调用读/写闸门：已进入的调用可完成，停用/停止/卸载会等待调用租约，
  新调用不能越过生命周期转移；全局生命周期锁不跨长耗时 guest 调用持有。
- 必需依赖守卫改为比较被依赖插件的 active 版本；补齐 provider 版本切换、停用、
  active 卸载和非 active 旧版本卸载测试。
- 启动恢复按必需依赖拓扑排序，失败保持可诊断的 `Enabled + last_error` 语义。
- 增加宿主内受控 `call_extension_from_plugin`：调用方必须是实际 Running 扩展，
  目标动作必须声明 caller，Package 写入必须匹配宿主传入的 `content_package`。
- 前端 YAML 能力探测在失败/非 Running 时清空旧能力，以版本号淘汰乱序响应，
  `hasAction()` 只有在 `ready` 时才返回 true。
- 开发指南和插件 API 参考已说明 REST 用户调用、宿主内插件互调及当前无独立
  `start`/`stop`/`activate` REST 端点的生命周期语义。

## 自动化证据

| 检查 | 结果 |
| --- | --- |
| P1 后 `cargo test --locked`（`server/`） | 641 passed，6 ignored，0 failed；退出码 0 |
| P1 后 `cargo fmt --all -- --check` | 退出码 0 |
| P1 后 `cargo clippy --all-targets --all-features -- -D warnings` | 退出码 0 |
| 直接执行 `pnpm.cmd install --frozen-lockfile`（`web/`） | 成功：Already up to date，退出码 0；`tools/ci-local.ps1` 的 web install 关卡单独失败，原因是脚本调用 `pnpm.ps1` 时发生命令解析异常 |
| `pnpm.cmd test:run`（`web/`） | 64 files / 708 tests；退出码 0 |
| `pnpm.cmd build`（`web/`） | 退出码 0；仅有既有 chunk size warning |
| `tools/build-plugins.ps1` 临时输出 | 三枚官方插件（gamer.keymap 1.0.1、gamer.video 1.0.0、gamer.yaml 3.1.1）；registry schema v2；无签名；SHA/staging 自检通过 |

新增服务端回归覆盖：

- 原生动作生命周期门禁与无副作用；调用租约和生命周期写闸门竞态。
- 必需依赖版本比较、provider 版本切换、循环、启动拓扑恢复和非 active 旧版本卸载。
- gamer.video → gamer.yaml 的 caller/Package Context 校验，以及伪造 caller/跨 Package 拒绝。

## NOT_VERIFIED

以下项目没有用源码或单元测试替代真实验收：

- `NOT_VERIFIED`：真机 Android、ADB、scrcpy、WebRTC、录制和真实浏览器交互，包括完整视频纵向流程。
- `NOT_VERIFIED`：Docker/NAT、跨平台运行和性能基准。
- `NOT_VERIFIED`：GitHub Release 创建与资产上传、线上安装/更新/卸载、404、哈希错误和版本不兼容链路。

本轮没有执行外部发布，也没有新增数据库迁移或 Package 数据迁移。
