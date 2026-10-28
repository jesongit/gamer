# Docker 生产部署

同一 Linux 容器包含 Gamer、静态前端、Chromium、FFmpeg、ADB 和本地 OCR。
云游戏不需要额外的浏览器/VNC 容器。Windows、macOS 使用 Docker Desktop 的 Linux 引擎，
Linux 使用 Docker Engine，可直接使用 `docker run`。宿主休眠、关机或 Docker 未启动时不能执行定时任务。

已有 Docker 安装换机器，优先使用[简单迁移步骤](../../deploy/docker/MIGRATION.md)：
本机停机备份，上传数据包与部署包，目标机运行恢复脚本；检查账号后单独开启调度。
简单迁移采用纯 `docker run`，无需 Compose；支持精简的 `production` 传输包和 gzip 压缩。
精简只影响传输副本，不删除源卷的备份/排查资料，不丢弃应用历史或账号资料。
后文 Compose 示例为可选部署方式；直接运行命令及资源设置以简单迁移文档为准。

## 已发布镜像

Docker Hub：<https://hub.docker.com/r/posase/gamer>。

```sh
docker pull posase/gamer:v1.0.8
```

`v1.0.8` 内应用版本为 `0.5.7`，新增可即时保存的远程 MCP 开关，并沿用网页主机、协议和映射端口显示连接地址；默认仅本机，独立 Bearer 和原有权限检查保持有效。
该发布仅提供 `linux/amd64`，对应 Intel/AMD 的 x86_64 机器。
摘要与源码版本见 `deploy/docker/releases/v1.0.8.json`；已发行旧版本保持不变。

若使用可选 Compose，将部署目录 `.env` 中 `GAMER_IMAGE` 设为 `posase/gamer:v1.0.8`；
其他数据卷、端口与时区设置保持需要的值。模板为 `deploy/docker/production.env.example`。
镜像不含用户数据；已有安装须另外迁移完整数据卷，迁移前不要在目标机器初始化新密码。

## 构建与首次部署

```sh
git submodule update --init --recursive
docker build --build-arg GAMER_GIT_COMMIT=<commit> -t gamer:<version> .
```

根目录 `.env` 只保存部署选择，不放密码、令牌或密钥：

```dotenv
GAMER_IMAGE=gamer:<version>
GAMER_DATA_VOLUME=gamer-production-data
GAMER_BIND_ADDRESS=127.0.0.1
GAMER_PORT=18443
TZ=Asia/Shanghai
```

全新安装先交互设置管理员密码（已有数据迁移跳过此步）：

```sh
docker volume create gamer-production-data
docker run --rm -it --memory 512m --memory-swap 512m --entrypoint python3 --mount type=volume,src=gamer-production-data,dst=/data posase/gamer:v1.0.2 /app/tools/gamer_docker.py init
docker run -d --name gamer --init --restart unless-stopped --stop-timeout 90 --shm-size 1g --security-opt seccomp=./deploy/docker/chromium-seccomp.json -p 18443:8443 -e GAMER_PROFILE=prod -e GAMER_COOKIE_SECURE=false -e TZ=Asia/Shanghai --mount type=volume,src=gamer-production-data,dst=/data --log-driver json-file --log-opt max-size=10m --log-opt max-file=5 posase/gamer:v1.0.2
docker ps --filter name=gamer
```

打开 `http://localhost:18443`，使用刚设置的密码。没有默认密码。容器的生产模式要求
Argon2id 密码哈希；初始化在本地终端隐藏输入，配置只保存哈希。新配置的自动调度默认关闭，
准备完插件、配置包和任务后，在 `/data/config.toml` 将 `scheduler_enabled` 改为 `true` 并重启容器。
这项开关保留任务的 enabled 状态；关闭时仍可手动试跑。

### prod 模式的 HTTP / HTTPS 登录（v1.0.2 起）

生产运行模式和登录 Cookie 的传输策略分开设置。局域网 HTTP 部署保留
`GAMER_PROFILE=prod`，在 `docker run` 增加 `-e GAMER_COOKIE_SECURE=false`；
HTTPS 入口使用 `-e GAMER_COOKIE_SECURE=true`。也可在配置文件 `[auth]` 中设置
`cookie_secure = false/true`，环境变量优先。配置文件修改后重启服务；Docker 的环境
变量修改需保留同一数据卷并重新创建容器。此设置不改变密码、
认证、同源校验、HttpOnly、SameSite、生产日志/诊断、任务或数据。

未配置时保持原有缺省：prod 使用 Secure Cookie，dev 不使用。新 Docker 配置模板
为 HTTP 显式设置 false。旧 `posase/gamer:v1.0.1` 不包含这个新开关；
已有数据卷需使用 v1.0.2 并显式设置环境变量，当前生产容器不会自动更新。

镜像默认零业务资源，不把开发数据、配置、浏览器资料或凭据打进镜像。
正式插件安装记录、WASM 和业务数据位于 `/data`，与镜像升级分离。
容器内不使用应用自更新，更新策略为 `off`，通过替换镜像发布。

### 已有 v1.0.1 更新为 v1.0.2（Bash）

在目标机原部署目录执行。先等任务结束，再冷备份整个数据卷；本步骤不执行 init/restore/activate，
保留密码、任务、自动调度、插件安装及浏览器资料。备份辅助容器限制 512 MiB。

```bash
docker pull posase/gamer:v1.0.2
umask 077
backup="gamer-before-v1.0.2-$(date +%Y%m%d-%H%M%S).tar.gz"
docker stop --timeout 90 gamer
docker run --rm --memory 512m --memory-swap 512m --entrypoint python3 \
  --mount type=volume,src=gamer-production-data,dst=/data,readonly \
  posase/gamer:v1.0.2 /app/tools/volume-transfer.py export --profile full --gzip > "$backup.partial" &&
  mv -- "$backup.partial" "$backup"
```

仅在备份命令成功且文件非空后继续。失败则 `docker start gamer` 恢复旧服务并排查，
不要将 `.partial` 视为可用备份。旧容器改名保留并关闭自动重启，避免两个版本同时使用数据卷：

```bash
test -s "$backup" && docker update --restart=no gamer && docker rename gamer "${backup%.tar.gz}"
docker run -d --name gamer --init --restart unless-stopped --stop-timeout 90 \
  -p 18443:8443 --shm-size 1g \
  --security-opt "seccomp=$PWD/deploy/docker/chromium-seccomp.json" \
  -e GAMER_PROFILE=prod -e GAMER_COOKIE_SECURE=false -e TZ=Asia/Shanghai \
  --mount type=volume,src=gamer-production-data,dst=/data \
  --log-opt max-size=10m --log-opt max-file=5 posase/gamer:v1.0.2
```

检查 `docker ps` 显示 healthy，重新登录后确认原 4 个任务和调度状态；不需要重新激活。
安全配置沿用已经应用 runtime-fix 的文件；也可从新镜像的 `/app/chromium-seccomp.json` 提取更新。
v1.0.2 无数据库迁移。需要回滚时停止新容器，将其改名保留，再把上述旧容器改名回 gamer，
恢复 `--restart unless-stopped` 并启动。备份和原卷均保留；若需恢复数据，将备份恢复到**新空卷**，
核验后切换挂载，禁止覆盖现有卷。

## Windows 现有数据迁移

以下操作由原 Windows 用户执行；迁移工具需要 Python 3.11+、`cryptography` 和
`websocket-client`。先确认没有运行中的任务。路径示例请替换为实际位置。

1. 原服务仍在运行时，只为已有定时任务对应的 Gamer 浏览器资料导出登录态：

```powershell
python tools/gamer_browser_sessions.py export `
  --data D:/code/gamer/server/data `
  --token-file "$env:LOCALAPPDATA/gamer/admin-token" `
  --bundle D:/Gamer/session-transfer/sessions.enc `
  --key D:/Gamer/session-transfer/sessions.key
```

工具通过 Gamer API 打开各自专属浏览器，让浏览器正常读取自己的 Cookie，经本机 CDP
导出后立即关闭。不会执行任务，也不会读取个人 Chrome 资料。Cookie 全程不写明文文件、不输出值。
导出文件和配套密钥属于敏感数据，放在受当前用户保护的目录。

2. 停止原服务，制作完整备份和独立迁移副本：

```powershell
./gamer.ps1 stop
python tools/gamer_docker.py prepare `
  --source D:/code/gamer/server/data `
  --config D:/code/gamer/server/config.toml `
  --destination D:/Gamer/migration-YYYYMMDD
python tools/gamer_docker.py load `
  --prepared D:/Gamer/migration-YYYYMMDD `
  --volume gamer-production-data --image gamer:<version>
docker compose up -d --no-build
```

目标目录必须不存在。`backup/` 保存所有原始用户文件以及 SQLite Backup API 生成的完整一致数据库；
`backup-manifest.json` 保存每个文件的 SHA-256。原配置另存为 `source-config.toml`。
`data/` 是独立工作副本，只有它会导入新命名卷。完整备份不作运行目录。

原始 Windows 浏览器资料在备份和迁移副本的 `migration-originals/windows-browser-profiles/`
中完整保留；Linux 活动资料使用新的 Chromium 目录，并复制站点 Local Storage、IndexedDB、Session Storage。
Windows DPAPI/App-Bound Cookie 数据库不能直接跨系统使用，通过下一步浏览器导入恢复登录。

迁移只重新加密活动通知/OCR/推流配置，并修改 OCR 的 Linux 工具路径；源文件和备份保持原字节。
退休插件的私密数据原样保留。所有任务、预设、配置包、媒体、工作稿历史、发布事务、Trace、日志、旧版本安装都保留。

3. 把加密登录态送入刚创建的容器并导入：

```powershell
$container = 'gamer'
docker cp D:/Gamer/session-transfer/sessions.enc "${container}:/data/state/browser-sessions.enc"
docker cp D:/Gamer/session-transfer/sessions.key "${container}:/data/state/browser-sessions.key"
docker exec $container python3 /app/tools/gamer_browser_sessions.py import `
  --data /data --token-file /data/state/local-admin-token `
  --bundle /data/state/browser-sessions.enc --key /data/state/browser-sessions.key
```

通过工作台确认账号仍登录、截图/OCR 正常并逐任务试跑。站点若要求重新登录或验证，在工作台完成，
不能把 Cookie 复制成功当成真实任务通过。验收完成后只在生产副本启用自动调度；北京时间任务保持 `TZ=Asia/Shanghai`。

开发环境必须改用独立数据库和浏览器资料目录，设置 `scheduler_enabled=false`。
不要重新启动仍指向旧生产数据的开发实例。一个生产数据卷只能有一个 Gamer 服务使用。

## 更新、备份与回滚

先本地开发和测试，构建新的固定版本标签；在独立数据副本验证，再安排无运行任务的更新时间。
不要使用滚动 `latest` 标签。稳定生产实例不挂载源码或开发目录。

```sh
docker compose stop gamer
python tools/gamer_docker.py backup --volume gamer-production-data --image gamer:<old-version> --output /safe/path/gamer-before-update.tar
# 修改 .env 中 GAMER_IMAGE，保留旧镜像和备份
docker compose up -d --no-build
docker compose ps
```

备份工具要求没有容器在使用该卷。备份包括配置、数据库、全部用户文件和凭据密钥；仅备份数据库不足以恢复。
跨机器恢复或回滚数据库迁移时，导入一个全新卷，不覆盖当前卷：

```sh
python tools/gamer_docker.py restore --input /safe/path/gamer-before-update.tar --volume gamer-restored-data --image gamer:<old-version>
# 修改 .env：GAMER_IMAGE=gamer:<old-version>，GAMER_DATA_VOLUME=gamer-restored-data
docker compose up -d --no-build
```

新卷不为空时拒绝导入。失败的部分卷保留供检查；工具不执行卷删除。
生产卷声明为 external，由迁移/初始化命令显式创建，Compose 不负责删除；不要清理生产卷。
旧镜像 + 对应完整备份构成回滚点。

2026-10-18 的本机诊断优化已通过独立副本验收并以 `docker run` 更新生产，标签为
`gamer:diagnostics-20261018`，没有覆盖 Docker Hub 的 `posase/gamer:v1.0`。
新镜像、旧镜像和完整备份均已另存，实际卷名、检查结果及简化回滚入口见
[诊断优化升级证据](../evidence/PRODUCTION_DIAGNOSTICS_UPGRADE_2026_10_18.md)。
回滚使用旧镜像配套的旧卷；升级后的新卷保留，不直接交给旧镜像运行。

随后从已提交源码重新构建并发布了 `posase/gamer:v1.0.1`。它与本机验收镜像使用不同构建身份，
经过独立空卷、插件安装、加密凭据/浏览器资料恢复、重启和诊断策略验证；
发布摘要与边界见[补丁发布证据](../evidence/DOCKER_HUB_V1_0_1_2026_10_18.md)。

主机备份/恢复工具的临时容器限制为 512 MiB（包含文件缓存），不使用额外 swap；
它不会改变生产容器的运行参数。大批量迁移保持串行，并关注 Windows 宿主可用内存；
Docker/WSL 总内存上限和自动回收属于部署机器的系统设置。

## 平台、安全与能力边界

- Dockerfile 支持 `linux/amd64` 和 `linux/arm64`。`.github/workflows/docker.yml` 在各自原生 runner 构建并验证浏览器沙箱/OCR。
  单机跨架构可用 `docker buildx build --platform linux/amd64,linux/arm64 ... --push`；QEMU 编译 Rust 很慢，优先原生构建器。
  工作流只构建检查，不自动发布镜像。实际通过的平台见部署证据，未运行的架构不能视为已验收。
- 启动器只负责卷初始化与权限归属，然后以 UID 1000 运行服务及 Chromium。浏览器启用沙箱，
  Compose 使用附带的 Chromium seccomp 配置并分配 1 GiB `/dev/shm`；不需要 privileged 或关闭沙箱。
- 凭据使用 AES-256-GCM 认证加密，安装密钥 `/data/state/credentials.key` 为 0600，必须与数据一同备份。
  密钥丢失不能恢复凭据；启动器不会替已有加密凭据悄悄生成新密钥。Windows 原生未配置安装密钥时继续使用原 DPAPI。
- 默认只发布本机 HTTP 端口。远程访问需 HTTPS 反向代理及防火墙；生产 Cookie 为 Secure。
  容器桥接来源不算服务端回环，本机管理令牌仍仅允许真实回环。内部维护用 `docker exec`。
  宿主或其他容器中的 Agent 使用 MCP 时，在设置中开启“允许远程 MCP 连接”（持久配置 `mcp_allow_remote = true`），通过已发布的端口 `/api/mcp` 连接并提供独立 Bearer 令牌；对象与工具权限保持有效，默认仍仅本机。
- 浏览器 CDP 仅监听容器内回环，不发布调试端口。云游戏通常使用浏览器软件解码；多账号同时游戏可能增加 CPU，
  GPU 透传因宿主与驱动而异，不属于跨平台默认方案。空闲 30 分钟回收 Chromium，下次运行自动打开。
- Android 网络 ADB 可以配置宿主可达地址；USB 透传、WebRTC NAT/TURN/UDP 端口按目标环境单独配置与验收。
  Docker Desktop 不会自动把宿主 USB 或手机发现广播传入容器。
- 镜像提供 `gh`，配置包发布插件的 GitHub CLI 登录仍需在新环境由用户自行完成；不复制宿主的全局 GitHub 凭据。
  容器用户 HOME 位于 `/data/state/home`，之后的 CLI 登录和 ADB 密钥随数据卷保留。

实际迁移与验收记录：[2026-10-18 生产部署证据](../evidence/DOCKER_PRODUCTION_2026_10_18.md)。
