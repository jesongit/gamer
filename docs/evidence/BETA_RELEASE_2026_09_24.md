# 首次公开 beta 发布验收（2026-09-24）

## 插件先发布

- 插件仓标签：`gamer-yaml-v0.1.0-beta.1`，提交 `5c88d10f6f0c4358a551b7eb2b4a71c0daf8d9ed`。
- 三款插件均为 `0.1.0-beta.1`；首次公开版本重新编号，旧 1.x/3.x 为内部测试版本，不自动降级。
- 独立 Windows 构建和 Linux 固定宿主集成测试通过：<https://github.com/jesongit/gamer-plugins/actions/runs/35964871209>。
- 下载草稿的 6 个资产，逐项验证 SHA256；合集 ZIP 内三款插件与独立 `.gplugin` 字节一致，manifest 版本和 UI 入口正确后公开为 Pre-release。
- 发布：<https://github.com/jesongit/gamer-plugins/releases/tag/gamer-yaml-v0.1.0-beta.1>。

## 主仓发布前本地验收

- 产品 / 启动器：`0.2.0-beta.1`；插件提交、目录清单和合集的 URL / size / SHA256 在 `release/plugins.lock.json` 固定。
- `fetch-plugins.ps1` 通过公开 HTTPS 下载并验证发布资产；生成本地市场副本和离线种子。发行清单中的官方插件下载 URL 指向插件仓固定标签。
- 服务端全量：704 passed，6 ignored；前端壳 773、自动化 UI 243、视频 UI 7 项全部通过。
- 启动器全量：201 passed，4 ignored（另新增真实安装验收测试默认 ignored，下面显式执行）；Clippy all-targets / deny warnings 通过。
- beta 发现回归覆盖：SemVer 数字顺序、跳过草稿、跳过缺清单/非官方 URL、正式版高于对应 beta、空发布列表拒绝。
- release workflow 静态与 immutable/SBOM 离线检查通过；版本门禁、manifest 校验、完整包 SHA256 和 doctor 检查通过。
- `test-published-plugins.ps1 -RemotePlugins`：在全新隔离目录移出插件 seed，调用真实 launcher repair，确认官方合集从公开插件 Release 下载并进入 SHA256 缓存；doctor deep/probe 无失败无警告。
- 启动真实 server，调用启动器 `official_plugins::choices/install` 完成三款插件安装，再次提交同一选择成功；三款插件均为 running，UI 模块 HTTP 200，本体版本匹配，最后通过认证 shutdown 正常退出。
- 不带 `-RemotePlugins` 重跑完整离线种子安装和相同插件验收，通过。未停用网络，但组件均使用本地 seed，不依赖组件下载。
- 验收目录：`backups/beta-release/online-verified`、`backups/beta-release/offline-verified`；日志位于同目录及 `backups/beta-release/`，个人安装和数据未改动。

## 范围

插件市场仍使用本体发行锁定的插件快照；独立插件新版本动态发现不在本次发布中。beta 启动器通过 GitHub 公开发布列表发现本体版本，稳定启动器仍使用 latest。设备实机投屏、触控和用户脚本未作为本次发行验收执行。

## beta.2 修订

- beta.1 主发布在干净 runner 下载 FFmpeg 时被大小门禁阻断：上游 latest 已滚动，锁定归档不再可取；保留失败标签和草稿，不公开、不重打标签。
- 改用 `autobuild-2026-08-31-13-27` 的固定月末构建，版本 `N-126342-gf88b741dbf-20260831`；从上游重新下载，核对 GitHub 资产 digest、归档及两份 EXE 哈希，同步来源和源码 offer。
- `fetch-ffmpeg.ps1` 版本/LGPL/buildconf/H.264 stdin→PNG 探针全部通过；重新构建 `0.2.0-beta.2` 本体、启动器和完整包，校验和与 doctor 通过。
- `test-published-plugins.ps1 -RemotePlugins` 在 `backups/beta-release/beta2-verified` 重跑通过：真实公开插件下载、首次安装和重复选择、三插件 Running/UI、本体版本、正常退出全部成功。
- beta.2 仅变更版本元数据和锁定依赖来源/字节，不改业务逻辑；beta.1 的全量回归记录继续适用，新依赖另外走上述实际安装和媒体探针。

## beta.3 最终候选

- 启动器启动检查、桌面后台 IPC 和 CLI 托管服务的网页“检查更新”统一使用 `ManifestSource::configured()`；缺省走同一官方 stable/beta 发现逻辑，显式测试源仍可覆盖。每次检查重新发现，避免把启动时版本 URL 永久冻结。
- 启动器全量 202 passed，静态检查通过；新增默认官方源/空覆盖/显式 URL 与路径覆盖回归。
- 完整包 hash、清单、doctor 通过；真实桌面后台测试在 `backups/beta-release/beta3-desktop` 通过，包含暂停续装、首次选择两款插件、损坏前端文件自动修复、校验缓存复用、显式 repair 保留用户数据、发现候选时不自动启动、故意错误版本候选的自动回滚、恢复原服务和正常退出。
- beta.2 不公开，保留为候选记录；最终发布使用 v0.2.0-beta.3。

## beta.4 首次安装修订

- beta.3 的 Windows CI 构建和完整包真实验收通过并公开后，追加纯在线空目录验收发现首装只生成 `port`，服务端因缺少 `data_dir` 等必填配置退出；已在 beta.3 Release 显著标明限制，不替换其已发布资产。
- `release/config.default.toml` 成为完整包与启动器的共同模板；桌面与 CLI 仅在配置不存在时生成，保留已有用户设置。服务端测试直接反序列化该模板，锁定真实必填字段契约。
- 启动器全量 203 passed，6 ignored；服务端配置 18 项通过，launcher Clippy all-targets / deny warnings、版本一致性及插件 SDK 快照检查通过。
- `test-published-plugins.ps1 -RemotePlugins -FreshConfig` 使用实际 beta.4 EXE，在隔离目录移出完整包配置和插件种子，验证首次配置生成、实际本体就绪、公开插件下载、三插件首次/重复安装、Running 与 UI 资源及认证退出。
- 真实桌面后台验收同样移出配置，从无配置状态开始：暂停续装、首装启动和插件选择、损坏修复、缓存复用、用户数据保留、失败候选回滚与退出全部通过（87.69s）。
- 启动器真实成功升级 `beta.3 → beta.4` 通过（20.25s），配置逐字节不变，用户数据保留，更新后服务正常退出。
- 补测发现网页更新的现有缺口：`prepare_install` 仅复验 staging，未接实际版本切换；网页发现与下载通过，但网页安装不能宣称成功。测试版 Release 已列明限制：通过启动器“更新”执行切换。
- 主仓 CI 全绿：<https://github.com/jesongit/gamer/actions/runs/35970508687>（Linux 服务端 702 passed / 6 ignored，平台专属测试数与 Windows 不同）。

## 最终公开结果

- 主仓公开 Pre-release：<https://github.com/jesongit/gamer/releases/tag/v0.2.0-beta.4>，发行提交 `e3677a1e7ad8f10a3a47b461e28ac857254a3a4d`。流水线 <https://github.com/jesongit/gamer/actions/runs/35970510540> 全部成功，13 个发布资产。stable latest 仍为 `v0.1.1`。
- 下载 CI 实际 MSVC 完整包，确认与草稿 Release digest 相同，再以 `-RemotePlugins -FreshConfig` 通过真实安装、配置生成、本体启动、三插件首次/重复安装及 UI、正常退出，随后放行发布环境。
- 完整包 SHA256：`a1e211fb8113cf5675adc5b920f9c45ea01bb756ba07fd4696c4879237768362`（131209908 bytes）；独立 EXE：`a677ff9cc81ba7f3bf61823effcf25bff9563bc94a3992ae45f2be4fe29ba75f`（12175360 bytes）。本地 `release/dist/` 的对应完整包与 EXE 已替换为这些实际发布字节。
- 公开后在 `backups/beta-release/public-beta4-online` 从空目录验收，无清单覆盖、无 seeds、无预置配置：默认 beta 发现通过，公开 EXE 下载及文件哈希通过，真实启动器从公开 Release 下载全部六个组件归档并逐一校验 SHA256；自行生成配置后本体就绪，三插件运行且 UI 可用；网页检查更新得到当前 beta.4，无新版本；认证关闭成功。
- beta.3 Release 已标明首装问题及 beta.4 修复链接；插件 Release 推荐宿主同步为 beta.4。已发布标签和资产未重写。
