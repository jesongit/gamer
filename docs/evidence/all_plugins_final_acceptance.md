# Gamer 全插件正确性与交互优化：最终验收证据

> 验收日期：2026-09-09（Asia/Shanghai）
> 项目：`jesongit/gamer`
> 分支：`main`
> 审查基线：`b242c0d586c19bb0fdb996e05879e3f0459ef9ff`
> 验收结论：`PASS（自动化验收）`
> 真实环境边界：浏览器、Android、WebRTC、远端发布与下载安装均为 `NOT_VERIFIED`

## 1. 结论摘要

当前工作树的代码、自动化回归、构建和官方插件候选产物校验已完成。未验证的真实环境项目没有被折算为通过，详见第 5 节。

本次验收保留以下工作区事实：

- 工作树仍有未提交改动，未创建提交、未推送远端；
- 原始用户数据目录 `E:/code/gamer/server/data/` 保持原状，未删除、迁移或覆盖；
- 本报告及相关证据只描述当前工作树和实际命令结果，不代表已发布版本或真实设备运行结果。

## 2. 后端质量门禁

命令均在 `E:/code/gamer/server` 执行：

| 命令 | 结果 |
| --- | --- |
| `cargo test --all-targets` | `648 passed, 6 ignored, 0 failed` |
| `cargo clippy --all-targets --all-features -- -D warnings` | 退出码 `0`，无警告 |
| `cargo check --locked --no-default-features` | 退出码 `0`，无 WASM 退出路径通过 |
| `cargo build --release` | 退出码 `0`，release 构建通过 |
| `cargo fmt --all -- --check` | 退出码 `0` |

覆盖包括扩展生命周期、依赖门禁、资源版本与目录隔离、Package/媒体引用、架构守卫以及现有服务端测试。专项报告中关于“早期全量命令未保留退出结果”的说明属于此前阶段记录；本报告以之后保留的最终 `648 passed / 6 ignored` 结果为准。后端具体专项结果见 [all_plugins_phase6_server.md](all_plugins_phase6_server.md)。

## 3. Web 质量门禁

命令在 `E:/code/gamer/web` 执行：

| 命令 | 结果 |
| --- | --- |
| `pnpm test:run` | `82/82` 测试文件通过，`802/802` 测试通过，退出码 `0` |
| `pnpm run build` | 退出码 `0`，Vite 正式构建通过 |

前端旧 P6-WEB 证据中的 `3 failed / 5 failed` 已被最终集成测试收口，当前结论以 [all_plugins_phase6_web.md](all_plugins_phase6_web.md) 的 `82/82`、`802/802` 为准。happy-dom 的 `URL is not a constructor` 与本机 `127.0.0.1:3000` / `::1:3000` 连接拒绝仅为非阻塞运行噪声，不计为测试失败，也不作为真实链路通过依据。

仓库根目录执行：

```text
git diff --check
```

退出码：`0`。

## 4. 官方插件候选产物

使用 `tools/build-plugins.ps1` 在临时目录构建，未把临时产物写入仓库业务数据：

| 检查项 | 结果 |
| --- | --- |
| 官方插件临时构建 | 退出码 `0`；`gamer.keymap@1.0.1`、`gamer.video@1.0.0`、`gamer.yaml@3.1.1` 均成功 |
| Registry | `schema_version=2`，3 个条目，条目与根对象无 `signature` 字段 |
| `.gplugin` 独立 verify | 3/3 通过，退出码均为 `0` |
| SHA-256 完整性清单 | `sha256sum -c sha256sums.txt` 通过，4/4 `OK`，退出码 `0` |
| 包结构 | WASM 包含 `manifest.toml + plugin.wasm`；builtin 视频包仅含 `manifest.toml`；无 `signature.sig` |

发布路径修复及完整产物明细见 [all_plugins_phase6_release.md](all_plugins_phase6_release.md)。本地临时构建不等同于远端发布或远端下载安装。

## 5. NOT_VERIFIED 清单

以下项目本轮没有实际环境证据，明确保持 `NOT_VERIFIED`：

- 真实浏览器中的完整安装/启用/更新/卸载、Package、YAML、Keymap 和 Video 用户流程；
- 真实 Android/ADB 设备上的触控、按键释放、应用启停、录制和设备会话行为；
- 真实 WebRTC 视频/数据通道、Stage live/media 切换和跨页面接管；
- 真实部署下的 HTTP、跨进程恢复、多进程/磁盘故障注入和录制后台链路；
- 真实媒体样本的 VFR/PTS、平台兼容、性能与长时间录制矩阵；
- 公开发布平台、远端 registry/CDN、远端下载安装及线上更新/卸载链路。

自动化测试、release build 或临时插件构建均不替代以上验证。

## 6. 最终判定

自动化验收条件满足：后端 `648 passed / 6 ignored`、clippy `-D warnings`、无 WASM 检查、release build、fmt、差异检查、Web `82/82` 与 `802/802`、Web build、官方三插件 registry v2/verify/SHA 校验全部有结果记录。计划状态可标为“已完成（自动化验收通过；真实环境项 NOT_VERIFIED）”；未验证项目不得标为通过。
