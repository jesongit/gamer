# 批次 4（DKR-001~004）Docker daemon 真实验收证据索引

> 用途：批次 4「Docker stop/SIGTERM 统一 drain」「DKR-004 新镜像不健康按旧 digest
> 自动恢复」「Docker readiness/时区收口」的真实 Docker daemon 端到端证据。
> 仿照 `docs/UPDATE_M2_EVIDENCE.md` 结构。依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md`
> DKR-001~004、§11.7（Docker 与发布硬门禁）、§17.6/§17.7 对应条目。
> 状态：**已实测（2026-09-01）**——Docker daemon 29.7.2 实跑，构建/冒烟、健康升级、
> 不健康候选自动回滚、SIGTERM 统一停机、数据锚点与 SQLite 完整性全部 PASS。
> 复现脚本：`release/packaging/test-docker-e2e.ps1`（幂等，支持 `-Cleanup`，
> `-ArtifactsRoot D:\qa-agentB-docker\dkr004 -HostPort 19543`，本轮实测跑通）。

## 实测环境

- 日期：2026-09-01；代码基线：commit `6f7792a0d33aea6e38a5da2faad8708e4f4890a9`
  （2026-08-31 19:26 +0800）+ 工作树大量未提交改动（镜像构建于 19:51 +0800，
  二进制内 `shutdown coordinator: draining (runs, viewers, device sessions)` 等当前
  停机字符串经 `grep -ac` 验证存在，见 E-1）
- Docker：Client/Server 29.7.2（Docker Desktop，desktop-linux），Compose v5.3.1
- 宿主：Windows x64，Windows PowerShell 5.1（未装 pwsh 也可跑通——见 E-6 的 5 项
  5.1 兼容修复）；python 3.14.3（宿主 SQLite integrity 校验用）
- 端口约束：宿主 8443 被其他进程占用，本次全部容器端口映射收敛到 127.0.0.1:19543
  （TCP+UDP）；临时目录 `D:\qa-agentB-docker\`
- 凭据：`GAMER_ADMIN_PASSWORD` 为一次性随机值（进程内 Argon2id PHC，不落盘配置），
  结束后已全部清理（容器/网络/镜像/临时目录），明文不记录

## 与验收范围的边界（先读）

1. **真实 Android 设备不参与**（真机已被并行任务占用）：scrcpy 会话、WebRTC
   ICE/媒体面、adb reverse 隧道清理无法在容器内端到端验证。§11.7「docker stop 走
   scrcpy/adb 清理」的证据止步于 drain 日志层（E-3）：run 级 drain 已实证
   （活动 run 被 10s 宽限等待 + `force-cancelling active runs forced=1` 强停），
   viewer/scrcpy 层面的清理由 `server/src/shutdown.rs` `drain_sessions` ②③ 静态
   代码路径 + 本次无会话场景的 coordinator finished 日志佐证。
2. **镜像版本注入缺失（REL-005 缺口，非本轮范围）**：`Dockerfile` 没有
   version/commit 构建参数，`/api/system/info` 的 `app.commit="dev"`、
   `app.built_at="unknown"`。因此「新旧版本」用同一镜像的不同 LABEL fixture
   （不同 digest）表达 digest 切换链路，不表达应用版本变化。

## E-1 镜像构建与版本语义（任务 1a）

- **结论：PASS（含边界 2）**
- **实测**：
  - `docker build --pull=false -f D:\code\gamer\Dockerfile -t gamebot:e2e-0.1.0 D:\code\gamer`
    → exit 0（全层缓存命中，镜像 CreatedAt 2026-08-31 19:51:12 +0800 与 HEAD 提交
    时间自洽）；image ID `1ebeb41adf5d`
  - 二进制与当前源码一致性：`docker run --rm --entrypoint sh gamebot:e2e-0.1.0 -c
    "grep -ac 'shutdown coordinator: draining...' /usr/local/bin/gamer-server"` →
    `2`（`shutdown signal: SIGTERM`=1、`shutdown coordinator: finished`=2、
    `server exited`=2，全部非 0）
  - Dockerfile 无版本注入 ARG → `docker tag gamebot:e2e-0.1.0 gamebot:e2e-0.2.0`，
    两 tag 同一 image ID `sha256:1ebeb41adf5d...`；DKR-004 的新旧语义由
    LABEL fixture（`org.opencontainers.image.revision=dkr004-old/new/bad`）推入
    本地临时 registry 后的**真实不同 digest**表达（E-2）

## E-2 release compose 冒烟（DKR-001/003 + 任务 1b/1c）

- **结论：PASS**
- **实测**（`docker-compose.release.yml` + `GAMER_IMAGE=gamebot:e2e-0.1.0` +
  冒烟 override：`container_name` 隔离、`ports: !override` 127.0.0.1:19543
  TCP+UDP、数据/配置/日志三个宿主 bind 目录在 `D:\qa-agentB-docker\smoke\`；
  `GAMER_ADMIN_PASSWORD` 一次性随机值；TZ=Asia/Shanghai）：
  1. `docker compose ... config --quiet` → 通过；渲染结果 TCP 8443→8443 +
     UDP 8443→8443（`GAMER_WEBRTC_UDP_PORT` 默认）+ `GAMER_DEPLOYMENT_MODE: docker`
     + `stop_grace_period: 30s` + healthcheck `/health/ready`
  2. `/health/ready` → 启动 ~2s 内 **200**：
     `{"checks":{"adb":{"ok":true},"data_dir":{"ok":true},"ffmpeg":{"ok":true},
     "scrcpy_server":{"ok":true},"sqlite":{"ok":true}},"ready":true}`；
     `docker inspect` health `healthy`
  3. `POST /api/login`（admin + GAMER_ADMIN_PASSWORD）→ **200**
     `{"ok":true,"username":"admin"}`
  4. `GET /api/system/info`（登录会话）→ 关键字段（完整快照存
     `smoke/system-info.json`，已随清理删除，字段值摘录如下）：
     - `deployment.mode = "docker"`，`deployment.update_strategy = "external"`
     - `capabilities = {check:false, download:false, install:false, rollback:false}`
     - `app.version="0.1.0" target="x86_64-linux"`（commit=dev 见边界 2）
     - 依赖三件套 `status=ready`：adb 1.0.41 / ffmpeg 7.1.5-0+deb13u1（deb 内置）/
       scrcpy 3.3.3（binding=application）
     - `schema.db=1 schema.file=1 rollback_floor=1`；启动日志
       `update stack online (controller + policy + coordinator) mode="docker"
       strategy="external"`
  5. **TZ 生效**（任务 4a）：容器内 `date` → `Tue Sep 1 00:46:27 CST 2026`，
     `date -u` → `Mon Aug 31 16:46:28 UTC`（差恰 8h）；`printenv TZ` →
     `Asia/Shanghai`；无 TZ 的裸容器基线 `docker run --rm --entrypoint date
     gamebot:e2e-0.1.0` → UTC。`/api/system/info` 冻结契约中无时区字段，
     日志时间戳为 UTC RFC3339（`2026-08-31T16:45:41.618656Z`）——容器视角时区
     证据以 `docker exec date` 为准

## E-3 SIGTERM 统一优雅停机（OPS-001 + §17.6 收口项 + 任务 3）

- **结论：PASS（run 级 drain 实证；viewer/scrcpy 层为无会话日志佐证，见边界 1）**
- **实测**（冒烟容器：已登录 + 活动脚本运行场景——设备锚点指向不可达地址
  `10.255.255.1:5555`，`POST /api/scripts/qa.docker.test%2Flongloop.yaml/run`
  （脚本 = loop 600×(wait 1s+log)）→ 202 `{"run_id":"a2df8f2d-...","state":"starting"}`
  → **130ms 后** `docker stop -t 30`）：
  - `docker stop` 总耗时 **12.752s**；`docker inspect` →
    `exit=0 status=exited oom=false`
  - 容器日志（bind 挂载文件，UTC）关键序列：

    ```text
    16:49:49.643  run accepted run_id=a2df8f2d-... script=qa.docker.test/longloop.yaml source=Manual
    16:49:49.988  shutdown signal: SIGTERM
    16:49:49.988  shutdown signal received; requesting coordinated drain
    16:49:49.988  shutdown coordinator: draining (runs, viewers, device sessions)
    16:49:59.998  WARN shutdown timeout: force-cancelling active runs forced=1
    16:50:02.029  shutdown coordinator: finished
    16:50:02.029  graceful shutdown: http server draining
    16:50:02.029  server exited
    ```

  - 时间线：SIGTERM → 10.0s run drain 宽限（活动 run 卡在 connect）→ 强停
    （`forced=1`）→ 0.5s settle + 会话清理 + HTTP drain → 进程内 SIGTERM→exited
    **12.04s**，与 drain 实现（`shutdown.rs`：①RunManager drain 10s → ②踢 viewer →
    ③`devices.shutdown_all()`）一致
  - **数据完整性**：停机后宿主侧 `python sqlite3` 对 `gamer.db` →
    `integrity_check = ok`，`user_version = 1`；设备表仍含锚点行；
    镜像自带二进制 `gamer-server inspect --data-dir /app/data --json` →
    `status=ok / user_version=1 / file_layout_v1=true / pending_migrations=[]`

## E-4 DKR-004 真实升级与自动回滚（任务 2）

- **结论：PASS（两次完整跑通；第二次带 `-KeepArtifacts` 固化证据）**
- **实测**（`release/packaging/test-docker-e2e.ps1` 全流程，输出逐条：
  compose static contracts → offline substitute → 本地临时 registry
  `localhost:58340/gamebot-dkr004`（old/new/bad 三个 LABEL fixture 推送后取真实
  RepoDigest）→ 5 套 compose `config --quiet` → 19 项 PASS →
  `REAL E2E PASS` → `SCRIPT_EXIT=0`）：
  1. 旧 digest `up -d` → healthcheck healthy（container `85f335f0…`，最终轮
     `9af3b24c…` 见下）
  2. `docker stop` SIGTERM → drain → exit 0；SQLite integrity ok（E-3 同链路）
  3. `compose start` → ready；HTTP `/health/ready` 200（127.0.0.1:19543）；
     登录 200；**升级前数据锚点**：`POST /api/devices {"name":"dkr004-anchor-…"}`
     → id `430e00051527488a8b57c4a006cae113`（固化轮
     `dkr004-anchor-99434cd9`）；三个 bind 目录静态 marker 就位
  4. **健康升级**：`upgrade-release.ps1 -NewDigest <new@sha256> -CurrentDigest
     <old@sha256>` → `pull OK → backup OK → up --force-recreate → ready: healthy`
     （container `cb2a98e9…`），`Config.Image` == 新 digest；bind mount 指纹不变；
     **锚点设备仍可查**（升级后重新登录 200，GET /api/devices 命中锚点 id）
  5. **不健康候选自动回滚**：`upgrade-release.ps1 -NewDigest <bad@sha256>
     -CurrentDigest <new@sha256>` —— bad fixture = 同基础镜像覆盖
     `ENTRYPOINT ["sh","-c","trap 'exit 0' TERM INT; sleep 3600"]`，进程活着但
     `/health/ready` 永不通过 → Compose healthcheck `start_period 15s +
     interval 30s × retries 3`（判定窗口 ~105–125s）判死 → 脚本捕获 readiness
     失败 → `compose rm -s -f` 停掉候选（SIGTERM 路径）→ 按旧 digest
     `up --force-recreate` → `旧镜像回滚 ready: healthy` → `旧 digest 已恢复并
     ready` → 子进程 exit 1（语义正确：升级失败）→ 容器 `Config.Image` ==
     回滚目标 digest → **锚点设备仍可查**；backup 快照 ≥2 个、全部含
     `BACKUP_READY`/`backup.json`/`MANIFEST.sha256`
  6. 全程 data/config/log 三个 bind 目录 marker 文件内容逐一比对一致；
     升级/回滚各生成一份独立备份快照
- **固化轮 digest 与备份证据**（`-KeepArtifacts` 后从宿主侧持久化文件读取）：
  - `release-image-state.json`（升级成功后才写入）：
    `currentImage = localhost:58340/gamebot-dkr004@sha256:55eaf7f6a8a1639a4632c554b4115179b001d4d14ba54229baa16718c5813dd9`
    `previousImage = localhost:58340/gamebot-dkr004@sha256:8507b528da825a7b051adc98546c3a2cf399d6f63f36e12b300ed50df5f4337a`
    （schemaVersion=1，data dir `D:\qa-agentB-docker\dkr004\data`）
  - 备份快照（两个，均含 `BACKUP_READY`/`backup.json`/`MANIFEST.sha256`/三目录副本）：
    `docker-20260831T172223266Z-b096c060`（健康升级前快照，8 条目：gamer.db
    45,056B + wal/shm + 三个 marker + config/log）、
    `docker-20260831T172236141Z-a3bc4cae`（失败候选尝试前快照，两者相隔 13s）
  - 回滚后数据目录终态：宿主 python sqlite3 → `integrity_check=ok`；devices 表
    含升级前锚点 `dkr004-anchor-99434cd9`
- bad fixture 自身 digest 为第三个临时 fixture（随 registry 销毁未固化字面值）；
  其引用格式（`repo@sha256:<64hex>`）与回滚断言（`Config.Image` == 回滚目标
  digest、`旧 digest 已恢复并 ready`）由脚本断言链保证
- **幂等复跑**：同参数（`-ArtifactsRoot D:\qa-agentB-docker\dkr004
  -HostPort 19543`）连续两轮全 PASS（每轮新 GUID project/registry 端口，互不
  残留）；`-Cleanup` 开关单独实测（清理 `gamer-dkr004-*` 前缀遗留后 PASS 退出）

## E-5 compose 端口/UDP 发布一致性（任务 4b/4c）

- **结论：PASS（compose 层面）+ 边界（媒体面未实测，见边界 1）**
- **实测**：release compose 渲染（E-2 第 1 条）与开发 `docker-compose.yml` 的
  端口块逐行一致：`"8443:8443"` TCP + `"${GAMER_WEBRTC_UDP_PORT:-8443}:8443/udp"`
  UDP；注释口径一致（「若使用自定义 rtc_udp_port，容器侧端口必须同步修改」）。
  与 `server/config.example.toml` 的 rtc 三键说明互洽：`rtc_udp_port` 必须与
  `rtc_external_ip` 成对配置（启动校验强制）；容器侧固定 8443 时需用户在挂载的
  config.toml 里配 `rtc_udp_port = 8443`（+NAT 场景 `rtc_external_ip/rtc_external_port`）
  媒体才会走发布端口，默认 `rtc_udp_port = 0`（临时端口）时发布端口不承载媒体
  ——compose 注释已明确，属文档化行为，无需改 compose
- docker-compose.usb.yml / local override / release override example 的
  `docker compose config --quiet` 每轮 E2E 均随跑随过（5 套全 PASS）

## E-6 本轮真实链路暴露的缺陷与修复

以下缺陷全部在 `release/packaging/test-docker-e2e.ps1` 内修复（该文件为本轮唯一
允许修改的脚本面）；均为「脚本此前只在 pwsh 下跑过、从未在 Windows PowerShell 5.1 +
真实 Docker daemon 下实跑」暴露的问题，修复后 `powershell.exe 5.1` 全流程跑通：

1. **UTF-8 BOM 缺失**：无 BOM 时 5.1 按 ANSI/GBK 读脚本，中文注释/字符串直接
   语法错误（`Try 语句缺少自己的 Catch 或 Finally 块`）。修复：写入 UTF-8 BOM
   （仓库打包脚本既有惯例）。
2. **参数名被脚本变量重置覆盖**：脚本状态块里 `$script:ArtifactsRoot = ''` 在
   参数绑定后执行，把 `-ArtifactsRoot` 参数实际清空（工件全部落到系统临时目录）。
   修复：状态变量改名 `$script:ArtifactsDir`，与参数解耦。
3. **PS 5.1 数组字面量拼接陷阱**：`@('a: ' + $var, ...)` 换行分隔的数组字面量会把
   `'a: ' + $var` 拆成两个元素（实测），生成的 compose override 因此 YAML 非法
   （密码值落在无缩进的下一行）。修复：先拼进变量 `$pwLine` 再放数组。
4. **PS 5.1 原生 stderr × EAP=Stop**：`$ErrorActionPreference='Stop'` 下原生命令经
   `2>&1` 重定向的**第一行 stderr**（docker buildx 进度输出）会变成终止性
   NativeCommandError，直接炸掉构建步骤。修复：`Invoke-Docker`/`Invoke-ChildPowerShell`
   及 python 调用在原生调用期间临时降回 `Continue`。
5. **PS 5.1 ConvertFrom-Json 折叠对象数组**：内联 `@(ConvertFrom-Json …)`
   （`-InputObject` 或管道两种形式都会）把对象数组收敛成单个「属性为数组」的对象
   （实测 bare 赋值=3、内联 @()=1；pwsh 无此问题），bind mount 断言全部 miss
   （`缺少唯一 bind mount /app/data actual=0`）。修复：bare 赋值后再
   `-is [array]` 归一化（Get-MountFingerprint / Get-RepoDigest / Assert-DeviceAnchor）。
6. **PS 5.1 向原生命令传参弄丢嵌入双引号**：`python -c "...\"PRAGMA\"..."` 直接
   语法错误。修复：校验代码写入临时 .py 文件再执行。
7. **（行为记录，非缺陷）容器重建丢弃进程内 session**：升级/回滚 force-recreate
   后，升级前登录的会话 Cookie 立即 401（session 存储不落库）。数据（设备锚点）
   不受影响；脚本在每次锚点断言前重新登录。

## E-7 未能完成 / 仍缺证据

- **GHCR 外部 digest**：本次升级/回滚使用本地临时 registry 的真实 digest；
  `ghcr.io/<owner>/gamebot@sha256:...` 的外网拉取、OCI label 与 commit 一致性、
  SBOM/attestation 验证仍缺（REL-005 + QA-008，见 §17.7 未勾选项）
- **WebRTC/ICE/媒体端到端**（§11.7）：无真实设备，未做容器内外浏览器↔服务端
  媒体连通；UDP 发布仅到 compose config 层
- **Docker 升级期间的运行守卫**（脚本运行中拒绝断会话）在容器场景未单独构造
  （依赖真实 scrcpy 会话）；Windows 侧已由 launcher E2E 覆盖同类门禁语义
- **三种部署模式互不破坏**（批次 4 合流门）：开发模式（gamer.ps1）与 Windows
  portable（M1/M2 证据）已有各自证据；本轮补齐 Docker 真实冒烟——三者合并结论
  待主控收口

## 踩坑记录（供主控收口 PITFALLS）

- PS 5.1 跑含中文的 .ps1 必须 UTF-8 **带 BOM**，否则按 GBK 解析直接语法错误；
  本轮 `test-docker-e2e.ps1`（未跟踪新脚本）因此从未在 5.1 下真正跑过
- PS 5.1 上 `@('a: ' + $var, ...)` 换行分隔数组字面量会把表达式拆成两个元素；
  字符串拼接先落变量再进数组
- PS 5.1 + `$ErrorActionPreference='Stop'`：原生命令 `2>&1` 后首行 stderr 变成
  终止性 NativeCommandError（docker buildx/任何进度输出即触发）；原生调用段临时
  降回 Continue
- PS 5.1 内联 `@(ConvertFrom-Json $json)` 会把 JSON 对象数组折叠成单个
  「属性为数组」对象（bare 赋值不受影响）；先 bare 赋值再 `-is [array]` 归一化
- PS 5.1 向原生命令传参会弄丢嵌入双引号（python -c 内联代码直接语法错误）；
  改写成临时脚本文件执行
- Git Bash 调 `docker run -v D:\path:/container` 会被 MSYS 路径改写（挂载点变成
  Git 安装目录下的路径），需 `MSYS_NO_PATHCONV=1`

## 状态页浏览器冒烟（2026-09-01，Agent G2）

对应 CLEAN_BASELINE_PARALLEL_PLAN §10.4 G「本机与 Docker 状态页冒烟测试通过」的 Docker 半边：
真实 Chrome（CDP）从登录到设置页状态卡片的端到端核验，并与登录态 `curl /api/system/info` 逐项对照。

### 构建与容器配置

- 镜像：`docker build -t gamebot:statuspage-smoke --build-arg GAMER_GIT_COMMIT=6f7792a0d33aea6e38a5da2faad8708e4f4890a9 .`
  （HEAD = 6f7792a "feat(release): 完成自动升级发布校验与 Docker 回滚链路"）；全部层命中
  此前 Agent B 构建的缓存（image config sha256:4a14e927…）
- 运行（docker run 等价 release compose）：`-p 19543:8443`、`TZ=Asia/Shanghai`、
  `GAMER_DEPLOYMENT_MODE=docker`、一次性随机 `GAMER_ADMIN_PASSWORD`（进程内转 Argon2id，不落盘）、
  bind mount 全部指向临时目录 `D:\qa-agentG2-tmp\{config,data,logs}`（未触碰仓库 `server/data/`）；
  config 为最小完整 config.toml（显式 GB_CONFIG 走严格解析，`port` 等字段必填——只写注释的
  config 直接启动失败，属预期行为）
- 登录用户名固定 `admin`（`attempt_login` 硬编码单管理员，任意其他用户名一律 invalid_credentials）

### 关键发现：build-arg 未注入（Dockerfile 缺 ARG 声明）

- 当前工作区 Dockerfile **没有任何 `ARG` 声明**，「Dockerfile 已加构建注入 ARG」的改动未落在本工作区；
  `--build-arg GAMER_GIT_COMMIT=…` 因此完全无效（buildkit 无警告、层缓存照常命中）
- Run A（仅 build-arg）：`/api/system/info` → `app.commit="dev"`，证实二进制内无 commit
  （构建容器内无 `.git`，build.rs 自动探测降级为 dev，符合设计）
- Run B（文档化兜底：`build_info.rs` 支持「编译期缺失时运行期同名环境变量注入」）：
  追加 `-e GAMER_GIT_COMMIT=6f7792a0…` 重建容器 → `app.commit="6f7792a0d33aea6e38a5da2faad8708e4f4890a9"` ✓
- **风险**：镜像级注入链路（CI build-arg → Dockerfile ARG/ENV → 二进制）在 Dockerfile 补上
  ARG 声明并真实走一次无缓存构建前，发布镜像的 commit 只能靠运行期 env 兜底

### 设置页状态卡片逐项核对（Run B，浏览器 DOM vs API）

| 页面显示 | API 对应值 | 结论 |
|---|---|---|
| GameBot 0.1.0（无硬编码，web/src grep 无 0.1.0 字面量） | `app.version="0.1.0"`（Cargo.toml） | ✓ |
| commit 6f7792a0 | `app.commit="6f7792a0…"`（注入值，非 dev/unknown） | ✓ |
| dev / 开发构建 标记、构建于 未知（开发构建） | `channel="dev"`、`built_at="unknown"`（未注入，如实显示） | ✓ |
| x86_64-linux | `app.target` | ✓ |
| 部署：容器（Docker） | `deployment.mode="docker"` | ✓ |
| 升级策略：外部管理 | `deployment.update_strategy="external"` | ✓ |
| 数据库 schema v1 / 文件布局 v1 / 回滚下限 v1 | `schema{db:1,file:1,rollback_floor:1}` | ✓ |
| adb 正常 1.0.41 托管/外部；ffmpeg 正常 7.1.5-0+deb13u1；scrcpy 正常 3.3.3 随应用分发 | `dependencies.*` 三项 status=ready | ✓ |
| 更新能力 检查/下载/安装/回滚 全灰、按钮禁用 | `capabilities` 全 false（docker 非 managed） | ✓ |
| update_not_managed 提示：Docker 模式请在宿主机更换镜像 | 卡片与更新卡片均渲染禁用说明 | ✓ |
| boot 2258b3b0 | `startup.boot_id` 前缀一致、stage=ready | ✓ |

- 证据：截图 `D:\qa-agentG2-tmp\settings-statuspage.png`（全页）；
  API 快照 `D:\qa-agentG2-tmp\runA-system-info.json`（commit=dev，缺陷证据）与
  `runB-system-info.json`（commit=注入值）；登录→Console→设置→任务全程浏览器控制台 0 报错
- Console 页（无真机）：`/#/console` 正常加载，设备下拉空态、工具条完整、无 JS 错误（仅页面加载验证）

### 任务页时区口径

`/#/tasks` 页面无显式时区标识；头部文案为「服务端 cron 调度 · Docker 内 7×24 运行 · 浏览器关闭
不影响执行」，无任务列表空态。容器内 `date` = CST（Asia/Shanghai 生效）；服务端日志时间戳为
UTC（Z 后缀，按 UTC 日期滚动日志文件名），页面展示口径以服务端为准——本轮未构造定时任务验证
「下次执行」渲染值，时区一致口径仅到容器系统层。

### 清理确认

容器已 rm、镜像 `gamebot:statuspage-smoke` 已 rmi（含 untag + 层删除记录）、`docker ps -a`/`docker
images gamebot` 均为空；临时目录中随机密码、cookie、凭据比对文件已删，仅保留两份 API 快照与截图。
