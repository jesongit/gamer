# 崩坏：星穹铁道配置包 0.1.0 发布验收

2026-09-25，按用户授权将本地配置包发布到公开仓库 `jesongit/gamer-packages`。

## 发布结果

- [公开 Release v0.1.0](https://github.com/jesongit/gamer-packages/releases/tag/v0.1.0)，非草稿、非预发布，作为最新完整配置目录；发布时间 `2026-09-25T13:27:47Z`。
- 包 ID `com.mihoyo.hkrpg`，版本 `0.1.0`；Android 目标 `com.miHoYo.hkrpg`，必需插件 `gamer-yaml`。
- 归档 `com.mihoyo.hkrpg-0.1.0.gamerpkg`，161,432 字节，SHA256 `03fd57ca8a5c4a3c146210e68d7ffc01676e5c84c0cdfcc5bfc6c4e5261ace46`。
- `packages.json` SHA256 `50c6b75780a1dea34b9f3cf4ea659732e7b3c4bfcb60d3ec25cd4e89e1726919`；同 Release 提供 `SHA256SUMS.txt`。
- 原目录中的 `publisher-smoke` 1.0.0 条目及归档逐字节保留（SHA256 `52323a7a1095f98a1c293bca8d2bdddfffa9c9b90c10d9c05d7341f528307f3c`）。旧 Release `catalog-20260925103634-14830bee` 及资产未修改。

## 公开副本

来源为本机 `server/data/packages/com.mihoyo.hkrpg`。原 manifest 的本地版本为 1.0.0，按照本次指定版本，仅发布副本改为 0.1.0；原目录 57 个文件的发布前后路径集合与 SHA256 全部一致。

公开副本保留现用 `_function.yaml` 的 8 个函数、50 张通用模板；去除个人账号截图、固定三账号入口和已经不支持的旧 `functions/` 目录。新的 `日常.yaml` 入口通过必填 `account: template` 参数调用原有 `账号日常` 函数，由使用者采集并选择自己的模板。`shared/README.md` 说明使用方式与验证范围。没有改动本地游戏配置或执行游戏。

用户已授权公开发布；个人账号内容问题未收到选择回复，发布副本按已告知用户的保守默认方案去除个人内容。模板总览人工查看为游戏按钮、标题和图标，未见个人账号信息；此查看不代表真机运行验收。

## 实际执行的验证

使用修复分支 `2a84912` 的服务端代码及固定插件提交 `cc5bc47`。在隔离工作树中临时接入本地 Rust 测试模块，直接调用生产函数；测试结束已移除临时接线，没有把本地包内容或临时测试入口提交到主程序。环境设置 `GAMER_LOCAL_ONLY=1`、`ADB_MDNS=0`，没有启动 HTTP 服务或 ADB。

1. 原生解析、保存内容钩子、函数注册表及调用目标检查通过：8 个函数、1 个自动化入口；全部 29 处不同静态模板引用均唯一命中现有文件，50 张 PNG 通过保存字节校验。
2. 生产 `export_package` 连续两次导出字节完全一致；`PackageEntry::from_archive`、目录 schema 校验及元数据一致性校验通过。此校验与游戏行为验证不同。
3. 通过本机 `gh` 创建新的草稿 Release，上传 4 个资产；逐一核对 GitHub 返回的大小、SHA256 和 uploaded 状态。发布前再次核对旧目录指纹未变，再公开并设为 latest。
4. 公开后使用软件自身 `PublicClient`，未携带 gh 登录凭据，重新发现 latest=v0.1.0、读取目录、下载两个包并校验大小、SHA256 和包内元数据；下载字节与准备资产完全一致。
5. 对实际下载内容调用生产 `extract_archive`，写入新的隔离目录；`PackageStore` 成功识别两个包，星穹铁道包再次通过同一套脚本、函数、模板检查。没有声称执行了浏览器导入按钮或 HTTP 导入接口。
6. 本地原包 57 个文件的 SHA256 与文件集合复查一致。

本地复核材料位于忽略目录 `backups/hkrpg-v0.1.0/`：`prepare.py`、`acceptance.rs`、`original-sha256.json`、`prepare-result.json`、`published-result.json`、`artifacts/`。两个显式 ignored 验收测试分别执行并通过；均未调用游戏流程。

## 相关主程序修复状态

[PR #6](https://github.com/jesongit/gamer/pull/6) 的启动闪窗、自更新中断、后台黑框修复已在 Rust / Web / 版本一致性 / 文档检查通过后合并，合并提交 `26b5aa1cbdc52b988c975a59a3368671d06d09ed`。本次只发布配置包，不发布新的主程序版本，现有官方 0.2.1 不因此获得这些修复。局域网网络许可和完整 GUI 自更新验收限制仍见 [启动器验收记录](LAUNCHER_UPDATE_FIX_2026_09_25.md)。
