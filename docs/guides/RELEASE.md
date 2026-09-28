# Gamer 发布与恢复

当前开发版只发行完整便携包及其更新归档，无独立启动器产物。版本源为 `server/Cargo.toml`，`web/package.json` 和 `updater/Cargo.toml` 必须一致。不要覆盖已发布标签或资产。

## 打包顺序

1. 固定 submodule、运行 `node tools/check-plugin-sdk.mjs`，完成前后端及 updater 的测试、格式和 Clippy 检查。
2. `fetch-adb.ps1`、`fetch-ffmpeg.ps1` 按依赖锁取包；`fetch-plugins.ps1` 按插件发行锁取包。
3. `package-app.ps1` 构建并打包服务端、前端和 scrcpy；`package-components.ps1` 打包运行依赖。
4. `gen-manifest.ps1` 生成 schema v2 清单，不再包含 launcher 组件。
5. `package-full.ps1 -SkipBuild` 从已校验归档展开完整安装布局，写 `Gamer.exe` 与当前版本指针，生成内部 SHA256SUMS 并执行启动冒烟。
6. 生成许可包、SBOM 与发行级 SHA256SUMS；发布 workflow 先生成 draft，重新下载核验，运行便携启动冒烟，再发布。

脚本均在 `release/packaging/`。生产构建必须绑定实际 commit、时间、版本、目标平台，完整包根入口必须和应用归档中的程序一致。

当前固定资产共 10 个内容资产 + SHA256SUMS：app、adb、ffmpeg、scrcpy-server、official-plugins、full、licenses 七个 ZIP，版本清单及固定发现入口两份 JSON，SBOM 一份。验证器按清单对组件哈希，完整包按内部哈希核验。

## 变更验收

见 [UPDATE.md](UPDATE.md) 的开发验证命令。`test-portable-update.py` 用临时安装、独立端口验证真实进程重启、数据保留和失败回退；不会覆盖开发实例。真实发布仍需要对发行构建执行完整包冒烟，开发 debug 程序验收不等于发布已完成。

## 恢复

先保留安装目录及日志。候选提交前的自动失败恢复由更新事务完成；中断后再次启动按 journal 恢复。进入 manual_recovery 时保留 `state/`、`backups/`、隔离目录和新旧程序，检查失败原因再处置。禁止在新版本已接受用户写入后自动恢复旧快照。安装布局及事务边界见 [UPDATE_CONTRACT.md](UPDATE_CONTRACT.md)。
