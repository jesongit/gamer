# Gamer 0.2.5 发布验收（2026-09-28）

## 范围

移除独立启动器，Windows 完整包使用 `Gamer.exe`；设置页统一更新前后端、运行依赖及符合权限预检的已安装官方配套插件，完成后自动重启。此版不提供旧启动器安装迁移。配套插件：gamer-yaml 0.1.1、gamer-keymap 0.1.3、gamer-video 0.1.1、gamer-live 0.2.5、gamer-package-publisher 0.1.0。

本记录补充 `PORTABLE_UPDATE_2026_09_28.md` 的实现阶段验证；其中当时尚未完成的全量 Clippy 与真实插件更新检查已在本次发布准备中完成。

## 源码与集成验证

- 宿主 `cargo clippy --locked -j1 --all-targets --all-features -- -D warnings` 通过。
- 宿主全量测试：755 通过，16 忽略，0 失败（测试使用本次构建的配套插件，避免旧 WASM ABI 夹具）。
- 更新器严格 Clippy 通过；182 项测试通过，无忽略。
- 无默认 feature 编译通过；版本单源、插件 SDK 七文件哈希检查、发行脚本 AST / 不可变发布 / SBOM 离线检查通过。
- Web 全量测试和构建、文档站点构建由 PR CI 验证通过。
- 插件 PR #5 的独立构建及固定宿主集成均通过；发布使用同一流水线产出的归档。未升级的配置包发布插件复用其已发布字节。

本地全量 Rust 验证使用 `CARGO_BUILD_JOBS=1`、`RUST_TEST_THREADS=4`；较早并行运行曾因内存不足中止，以上结果来自成功的串行重跑。

## 真实更新事务

`release/packaging/test-portable-update.py` 使用真实 Windows debug 程序，在 D 盘临时安装、隔离端口 18462 完成：

1. 安装并启用 gamer-live 0.2.4。
2. 更新主程序与配套插件；根入口预先附加无害 PE 尾部标记，确保测试真的发生根入口字节替换。
3. 核对新 boot_id、版本指针、根入口与版本程序哈希一致、数据保留、gamer-live 0.2.5 处于 Running。
4. 使用故意错误的候选身份 99.0.0，核对回退后旧版服务就绪、版本指针和数据保留。

两条路径均通过，脚本退出码 0。证据保留于 `release/dist/temp/gamer-portable-e2e-on6v5ri9/`。基线采用当前程序与较旧版本指针，用于验证事务，不声称覆盖旧启动器迁移或历史二进制兼容性。测试进程已关闭，原开发服务保留。

前端更新确认、自动刷新和未保存编辑保护见实现阶段证据与相关 Vitest。物理 Android 设备在升级后的投屏重连仍未实测。

## 发行资产

正式发行以 GitHub tag 流水线产物为准；下载后的 SHA256SUMS、manifest、SBOM、完整包逐文件哈希及实际启动由发行流水线和 `tools/verify-external-release.ps1` 验证。发布结果以对应 GitHub Release / Actions 记录为准。
