# 2026-10-28 三仓与正式发行同步

用户要求将主程序、插件、配置包及 Docker 同步到最新版本，并明确同意将 HSR `1.0.2-test.2` 晋升为正式 `1.0.2`。本次仅发布源码与发行物，没有替换真实安装、启动生产服务或操作真实账号/设备。

## 固定源码与发行关系

| 发行 | 版本 | 固定源码 |
| --- | --- | --- |
| Windows | `v0.5.2` | 主仓 `59ed13c5e387cf573ff574153c76b890cdd29678` |
| Docker | `v1.0.5`，应用 `0.5.2` | 同一主仓提交；`docker-v1.0.5` 解引用后与 `v0.5.2` 相同 |
| 官方插件目录 | `gamer-yaml-v0.3.26` | 插件仓 `66fae9611583305e4c7d1fc51ba47630e0b4adbf` |
| HSR | `hsr-v1.0.2` | 配置包仓 `b00c84cfd68ee3d3cdedd78699408a4cc60aa592` |

官方插件版本：YAML `0.3.26`、Live `0.2.19`、Notify `0.1.2`、Keymap `0.1.6`、Video `0.2.6`、Package Publisher `0.1.0`。未改动的三款插件复用旧公开归档原始字节。SDK 固定在已通过完整 CI 的宿主 `ea8d1052638128ea5afcf6aea652564628fb8d16`，主仓 gitlink 与发行锁固定插件仓 `66fae961`。

三仓 `main` 与发行标签均推送 GitHub 及 `192.168.31.10:2222/posase/` 对应仓库。主仓 submodule URL 为 `../gamer-plugins.git`；从 LAN 递归浅克隆 `v0.5.2`，实际得到主仓 `59ed13c5`、插件 `66fae961`，插件 remote 为 LAN 仓库。

## 本地与 CI 验证

- 主仓最终源码 CI [40593992741](https://github.com/jesongit/gamer/actions/runs/40593992741) 全部通过：版本门禁、Web 测试/构建、Rust fmt/clippy/测试/release 构建。
- 插件主仓 CI [40591193708](https://github.com/jesongit/gamer-plugins/actions/runs/40591193708) 与发行 [40591206960](https://github.com/jesongit/gamer-plugins/actions/runs/40591206960) 全部通过，包含完整宿主集成及显式启用的 FFmpeg 回环测试。
- 本地 Web 壳 868、YAML UI 100、Live UI 63、Notify UI 4、Video UI 19、Publisher UI 3 项通过；Live 后端 137 项通过。
- 配置 22、本机只读素材实际路由 6、架构 7、扫码登录状态 2 项通过；其中登录状态包含真实 WASM 调用。Clippy 全目标/全部 feature 且拒绝 warning 通过。
- 发布前修正了格式门禁和 SDK 宿主固定提交缺少原生函数目录快照的问题；候选标签 `gamer-yaml-v0.3.24` / `0.3.25` 保留，未改写旧标签、未公开失败候选发行物。

## 官方插件与 HSR 的公开复核

[官方插件目录](https://github.com/jesongit/gamer-plugins/releases/tag/gamer-yaml-v0.3.26) 的 9 个资产匿名重新下载，逐字节与发行归档一致；目录、SHA256SUMS 和六插件 bundle 校验通过。目录 SHA256 `374c88d26120dba6d9b084350a723cd1c769a8d4513985b593814b7cdd77abb6`，bundle SHA256 `9690156ba1b65d6fc269c73da2dac6a797634cdecd69f969ba2b0adeebd3e2ef`。主仓从公开发行获取后，通过 SDK 和共享插件 UI 快照检查。

[HSR 1.0.2](https://github.com/jesongit/gamer-packages/releases/tag/hsr-v1.0.2) 的 3 个资产匿名复核通过。配置包为 675215 字节，SHA256 `a4197383f1626f4a3d3977b7340e0c08aadb4dd966a294e710e1406ee0342353`。脚本与模板保持 `1.0.2-test.2` 原字节，仅 manifest 版本及 revision 晋升正式版。当前离线 probe：26 单元、23 周次数边界、48 入口场景通过；实际 Core 导入、哈希拒绝、冲突确认及覆盖导入通过。细节见 [HSR 发布证据](HSR_PACKAGE_1_0_2_2026_10_28.md)。

## Docker 公开发行与隔离验收

[Docker v1.0.5](https://github.com/jesongit/gamer/releases/tag/docker-v1.0.5) 使用干净 Git 归档构建，平台为 `linux/amd64`。Docker Hub `posase/gamer:v1.0.5` 与 `posase/gamer:latest` 均为：

```text
sha256:9cd5a768085521526c88350b884165f6ccc276b0f237e8c8a13686c9646cc57e
```

- 使用独立空 Docker 认证配置完成匿名拉取；镜像层已在本地缓存。匿名 Registry 检查确认远端 RootFS、配置及源码 label 与受测本地镜像一致。
- 五个既有版本 `v1.0` 至 `v1.0.4` 摘要逐一与推送前快照一致。`latest` 单独核对与 `v1.0.5` 相同。
- seccomp 契约检查、Chromium 非 root 真实沙盒启动、英文/简体中文 OCR、镜像静态内容检查通过；没有 Node/Cargo 运行环境或用户业务数据。
- 无网络、只读根文件系统、临时数据目录、无真实设备的实例完成六款正式插件的实际安装（全部 HTTP 201），并确认新登录状态原生函数可发现。
- `v1.0.4 → v1.0.5 → 重启 → 冷备份还原 → v1.0.4` 的完整隔离检查通过：四个合成启用任务、调度设置、配置、密钥、密码、Cookie 和旧插件/工作稿字节保留，凭据可解密，schema 仍为 7。
- HTTP 登录/鉴权/注销撤销、异源拒绝、HTTPS Secure Cookie 显式策略及生产默认策略通过。HSR 1.0.2 实际导入 201、错误哈希 400、已有包冲突 409。
- 四个 Docker 公开资产匿名重新下载，SHA256SUMS 与部署 ZIP 内部 manifest 全部匹配；Docker Hub 说明更新后匿名读回一致。
- 本次创建的隔离容器与命名卷已收尾；验收不访问真实安装和账号。

固定公开回执见 [v1.0.5.json](../../deploy/docker/releases/v1.0.5.json)。

## Windows 发行复核

Windows 发行流水线 [40594006279](https://github.com/jesongit/gamer/actions/runs/40594006279) 七个阶段全部成功。构建及附件复核通过后，按用户明确发布授权，经既有 `release` 环境审批门批准 portable smoke；保护规则未修改。原始完整包便携启动、前端入口、更新能力和正常关闭检查通过后，流水线将草稿转为正式 [v0.5.2](https://github.com/jesongit/gamer/releases/tag/v0.5.2)，并设为 GitHub latest。

- 11 个公开资产匿名重新下载，发布级 SHA256SUMS 覆盖全部 10 个内容附件，逐项一致。
- 完整包为 152450434 字节；包内 SHA256SUMS、内外 manifest、发现入口、应用及组件归档的大小/哈希全部一致。
- 应用 ZIP 为 24774483 字节，SHA256 `9b831e0756cfff8a91e0712e6d0b3ef9970a667020ebf52cfc8999a70871d5a5`；与本机事务测试使用的草稿下载字节一致。
- 发行 manifest v2、CycloneDX 1.5 SBOM、ADB/FFmpeg/scrcpy 三个固定运行依赖的来源及逐文件 hash 检查通过；官方插件 bundle 与独立插件发行原字节相同。
- 本机隔离事务使用真实发行二进制、公开插件及独立安装根，验证下载、快照、实际重启、current 指针、入口替换和数据保留；YAML `0.3.23 → 0.3.26` 后保持启用。故意构造版本身份不符的候选后，实际恢复旧服务，测试文件仍在，终态及健康检查符合预期。
- 本机事务的 ADB 组件明确替换为无设备/无网络的专用 PE 替身，并在测试 manifest 中记录替身哈希；只接受 `version` 和空 `devices -l`，实际调用记录也仅含这两项。此替身不进入任何公开发行物。原始未替换完整包的便携启动由 GitHub 干净 runner 完成。
- 本机事务基线使用当前真实二进制构造旧版本安装指针，只验证更新事务，不声称旧版 Windows 程序兼容性。真实旧镜像升级兼容性由上文 Docker `v1.0.4` 路径覆盖。

本轮详细逐资产回执、隔离安装根及日志保留在忽略目录 `server/target/sync-release-20261028/`，Docker 对应材料在 `server/target/docker-release-v1.0.5/`。后续仅提交本证据和 Docker 固定回执，发行标签与镜像的源码仍固定 `59ed13c5`。

## 验收边界

以上源码、保存帧、WASM、HTTP、沙盒、合成任务和发行物验证不代表真实平台直播、观众互动、扫码登录、游戏导航、战斗或真实账号完整日常流程通过。HSR 晋升已获得用户明确同意，该真实全流程验收仍未完成。现有生产容器、原生安装、账号和设备没有自动更新。
