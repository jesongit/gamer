# 显式远程 MCP 接入（2026-10-28）

局域网实例的 `/api/mcp` 在令牌校验前返回 403 `MCP connections must originate from loopback`，导致其他电脑上的 Agent 无法读取定时任务和运行证据。用户要求支持远程接入。

## 实现

- 新增配置顶层 `mcp_allow_remote`，默认 false；复用管理认证、配置 revision 和原子持久化，通过现有系统设置即时保存。配置重开后保留，不输出凭据。
- MCP 设置页提供“允许远程 MCP 连接”。保存合并最新设置并检查 revision，失败保持原状态；远程地址使用当前网页协议、主机和发布端口，避免 Docker 映射端口与服务内端口混淆。
- 开启后接受实际远程或容器桥接来源、对应域名/IPv4/IPv6 Host；上传前和上传完成后均检查当前策略。关闭后拒绝新远程调用，本机仍可使用；`local_only=true` 始终限制本机。
- 继续要求独立 Bearer，不接受管理 Cookie 或本机管理令牌替代；保留 Host/Origin 检查、转发头拒绝、凭据撤销/到期、scope/对象权限、运行策略与全部控制屏障。没有放开素材读取、本机管理入口或浏览器 CDP。
- 已受理的正式运行、持久任务与令牌生命周期继续遵守原契约；远程开关不删除令牌，也不以此取消已受理任务。

## 验证边界

验证使用临时配置、凭据和合成 HTTP 来源，没有启动生产服务、更新真实安装或操作账号/设备。真实局域网 Agent 和用户实例仍需更新后验收；原定时任务失败原因尚未取得日志，不能据此认定已修复。

- Web 全量 99 文件、885 项通过；发行构建输出到隔离目录；设计检测结果为空。
- 设置持久化与管理 API 相关 8 项、架构守卫 7 项通过。
- MCP 39 项通过，覆盖远程启停、重开配置、令牌撤销、Cookie 隔离、异源拒绝、无效 Host，以及上传途中关闭开关后拒绝派发。
- `cargo clippy --bin gamer-server --all-features -- -D warnings` 与 `git diff --check` 通过。
- 运行记录保留于忽略目录 `server/target/mcp-remote-20261028/`。

## Docker 1.0.8 发行验证

用户随后授权发布新版供其自行更新。应用版本为 0.5.7，发行源码固定 `92447221162665b7b36184c52b483d696b9fada6`，插件 gitlink 仍为 `66fae9611583305e4c7d1fc51ba47630e0b4adbf`。从两个仓库的干净 Git archive 构建，镜像只提供 `linux/amd64`。

- seccomp 四项、Chromium 非 root 真实沙盒启动、英/简中 OCR 及镜像无用户数据检查通过。
- 在独立内部 Docker 网络，用不同容器的真实 TCP 来源验证远程 MCP：默认 403，管理端即时开启后 initialize/tools/list 成功，无效令牌 401、异源/转发头 403、管理 Cookie 不能替代 Bearer。关闭后立即 403，重启保留开关，撤销后 401。
- 在已发布 v1.0.7 中创建合成永久令牌和启用的缺依赖任务，再换同一数据卷运行候选。原配置、凭据密钥、令牌存储逐字节保持，任务字段及 enabled 保持，旧令牌可认证，schema 仍为 7。测试拥有的容器、网络和卷已按标签核对并移除。
- 没有替换用户服务器容器或操作真实设备、游戏和账号。该版本解决远程诊断入口，原任务执行失败仍需更新后读取其实际记录。

详细候选材料在忽略目录 `server/target/docker-release-v1.0.8/`。

## 公开发行

[Docker v1.0.8](https://github.com/jesongit/gamer/releases/tag/docker-v1.0.8) 已公开。[源码 CI 40700723583](https://github.com/jesongit/gamer/actions/runs/40700723583) 的版本、Web、Rust 检查全部成功。`posase/gamer:v1.0.8` 与 `latest` 均为 `sha256:3062428358353b5e5b081b035aefdcce18a5e821d0201bd19d30a89c1aba24a4`。

- 独立空凭据配置匿名拉取成功，镜像层已在本地缓存；匿名 Registry 核对 RootFS、配置、来源标签与受测镜像一致，v1.0 至 v1.0.7 的八个旧标签摘要保持不变。
- 四个公开附件匿名重新下载并逐字节核对，SHA256SUMS 与部署 ZIP 内部清单通过；Docker Hub 说明匿名读回一致。发行标签也已同步内网仓库。
- 公开回执为 [v1.0.8.json](../../deploy/docker/releases/v1.0.8.json)。后续仅提交证据和回执，不因此重建发行物。用户需自行更新其服务器、保留原 `/data` 挂载，并在设置页开启远程 MCP 后继续诊断。
