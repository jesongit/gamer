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
