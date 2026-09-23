# 启动器与发行包本机验收（2026-09-24）

本轮在开发 Windows 主机运行完整自动化检查、真实进程集成测试和重新组装的离线包验收。修复后，下列已执行项目全部通过。此记录不代表干净 VM、真实 Android 设备或生产发布验收已经完成。

## 范围与隔离

- 本体及启动器版本 0.2.0；官方插件 gamer-yaml 3.3.13、gamer-keymap 1.0.1、gamer-video 1.0.2。
- 代码基线 `1111632`，加本轮修复；本地包用开发密钥签名，没有发布到 GitHub。
- 安装、数据库、配置和媒体均位于 `%TEMP%/gamer-full-qa-197cceb6/` 的独立子目录；使用独立端口，不接管开发环境 8443 服务。
- 测试服务已退出，开发服务保持运行。临时目录保留用于复核，不作为产品数据使用。

## 自动化结果

| 范围 | 最终结果 |
| --- | --- |
| Launcher 常规测试 | 199 通过；fmt、Clippy（warnings as errors）、release 构建通过 |
| Launcher 真实进程补测 | 两项 desktop worker 测试、一项离线插件安装 API 测试通过 |
| Server 常规测试 | 703 通过；fmt、Clippy、无默认 feature 编译、官方 YAML guest 构建及 Component 校验、release 构建通过 |
| Server 默认忽略测试 | 6 项均通过；其中匹配基准修复夹具预处理后单独复跑通过 |
| Web 与插件 UI | 1,023 通过（Core Web 773、YAML UI 243、Video UI 7）；冻结依赖安装及构建通过 |
| 发行契约 | Manifest 28 项自测、工作流静态/离线契约（含不可变资产、SBOM、密钥轮换）通过 |
| 最终完整包 | 签名及文件清单校验通过；真实 `repair --probe` 和 `doctor --deep --probe` 通过，0 失败、0 警告 |

Keymap UI 当前没有独立测试文件，不能把其空测试集退出成功计为测试覆盖。Launcher 的 `materialize_demo_install_root` 是旧演示环境生成器，本轮未执行；其余三项环境依赖测试已显式执行。旧 `test-upgrade-launcher-e2e.ps1` 带固定旧版本假设，未作为本轮升级通过证据。

Server 忽略测试涉及真实 FFmpeg 导入/帧提取、非法媒体、录制 MP4 的 FFprobe 检查、解码/匹配/日志基准。性能测试以 debug 构建和 `GAMER_PERF_ITERS=5` 执行，只证明链路与断言正常，不构成性能指标验收。

## 实际安装、升级与窗口行为

1. **从零离线安装**：签名清单指定的本体及全部组件从 seeds 安装；中文和空格路径通过。首次插件选择只装已选 YAML/Video，Keymap 保持未安装；重复提交后追加 Keymap，三款均 Running。
2. **修复与校验缓存**：故意损坏安装后的网页文件，自动启动前修复成功，个人数据标记保留；再次启动未变化的文件不重写校验记录。主动完整修复也通过。
3. **暂停与续传**：真实 worker 安装暂停时不切换 current，继续后完成安装；HTTP 集成测试覆盖传输中暂停、Range 恢复、200 重下、错误范围/哈希及变化的 ETag。
4. **真实旧版升级**：使用 `jesongit/gamer` 的实际 `v0.1.1` Release 完整包，验证 0.1.1 → 0.2.0、SQLite schema 1 → 5，用户数据标记保留。CLI 升级与 GUI worker 升级分别验证；后者确认升级后仍出现首次插件选择，可全部跳过。
5. **更新优先与失败回滚**：已缓存可信更新时，即使发现端点离线也不会自动启动旧版；用户仍可明确选择启动旧版。测试候选故意声明 0.2.1 而包含实际 0.2.0，版本身份门禁拒绝激活，快照恢复个人数据与旧版服务。该候选仅为故障注入，不是实际新版本发布。
6. **实际 Windows 窗口**：从不同工作目录启动仍使用 EXE 所在目录；健康安装自动启动服务并隐藏窗口，launcher 继续驻留。第二次启动唤回原实例、不产生第二个服务；原生关闭/最小化窗口事件隐藏窗口而不中止服务。
7. **退出与监管**：真实 worker 在运行中接收 Exit，等待所监管服务正常退出；测试结束无隔离服务遗留。窗口消息复验在实际 GUI EXE 上完成；没有以物理鼠标逐项点击托盘菜单。
8. **最终包媒体链路**：用包内 FFmpeg 生成 1 秒、320×240、10 fps 测试视频，真实 API 导入后返回 10 帧（PTS 0–900000 μs），成功取得首帧 PNG；包内 FFprobe 路径正常。

## 本轮发现并修复

- 校验缓存按路径寻址使 staging 原子移动到版本目录后重复计算哈希。改用卷标识和文件身份、目标哈希及大小寻址，仍核对修改时间等元数据；新增移动复用、替换失效和记录损坏回归。
- HTTP 206 续传未明确拒绝响应 ETag 与原强 ETag 不同的情况。现在追加前拒绝，并保留原部分文件；新增对应 HTTP 回归。
- 旧版升级成功后直接隐藏窗口，跳过尚未完成的首次插件选择。现在进入同一插件选择流程，真实旧版升级测试验证。
- 修正既有 Rust 格式与 Copy 类型冗余 clone，恢复质量门禁。
- 匹配基准旧 FFmpeg 灰度夹具与现行 `image::to_luma8` 预处理不同，低方差样本导致命中断言失败。测试改用同源 RGB 夹具按产品路径灰度化；保留命中阈值，不改产品匹配算法。

## 复核命令与日志

常规检查：

```powershell
./tools/ci-local.ps1 -SkipWeb
./tools/ci-local.ps1 -SkipRust
cd launcher
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

真实 worker 测试须使用可丢弃的隔离目录和非 8443 端口。安装测试的 `GAMER_DESKTOP_SMOKE_ROOT` 指向全新完整包解压目录；旧版测试的 `GAMER_DESKTOP_UPGRADE_ROOT` 指向已安装真实 0.1.1、并已复制 0.2.0 签名清单、公钥及 seeds 的目录。设置 `GAMER_LAUNCHER_RELEASE_MANIFEST` 为隔离目录内不存在的文件，模拟离线发现。不能对日常安装目录执行这些故障注入测试。

```powershell
cargo test --release --lib real_desktop_install_repair_restart_update_and_rollback -- --ignored --nocapture
cargo test --release --lib real_desktop_upgrade_preserves_data_and_offers_first_plugin_selection -- --ignored --nocapture
# 新鲜安装已启动并处于插件选择阶段，设置 GAMER_OFFLINE_SMOKE_ROOT 后：
cargo test --test offline_install -- --ignored
```

日志位于本机 `%TEMP%`，前缀 `gamer-full-`：`launcher-tests.log`、`server-ci.log`、`web-ci.log`、`server-ignored.log`（保留初次失败）、`matcher-perf.log`（修复后通过）、`server-final-clippy.log`、`release-tests.log`、`real-upgrade.log`、`desktop-upgrade.log`、`desktop-final-rerun.log`、`final-package-install.log`、`final-package-doctor.log`、`final-plugins.log`、`final-media.log`。最后一轮 worker 测试 28.97 秒，真实旧版 worker 升级测试 14.21 秒。

## 可交付的本地测试包

- 路径：`release/dist/Gamer-0.2.0-windows-x64-full.zip`
- 大小：153,816,750 字节
- SHA-256：`ebc621c89752425d124c46879a9282ff0ec975027c20f9f19b29c283c5a4e3db`
- 包内主程序 SHA-256：`6b5d0481683d32b0b66f6254b7ca30bbc95a33ffa3971ce3c40816a50c76568c`
- 开发签名测试产物，正式在线资产和稳定渠道入口尚未发布。包构建时修复尚未提交，构建元数据可能仍显示基线提交；用上述文件摘要标识本次验收的实际字节。

## 仍需正式发行前验证

干净 Windows / 不同系统版本、无开发工具环境的在线下载安装、真实 Android 投屏/触控/截图、物理托盘菜单交互、断电或系统重启后的恢复、完整长路径/磁盘满/目录权限故障矩阵，以及生产签名和发布后重新下载的资产验收。现有模拟和单元测试不替代这些外部场景。插件仓库拆分仍属于后续 P5，本轮没有迁移或推送。
