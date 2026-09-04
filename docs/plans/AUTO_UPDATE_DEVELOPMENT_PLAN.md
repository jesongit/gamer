# GameBot 运行依赖与自动升级开发计划

> 状态：**实施中（批次 0～4 已完成；批次 5 本机可执行项完成——剩余集中在真实 GitHub Release/GHCR/生产签名、Windows clean VM 与发布签核，2026-09-01）**；未完成项与阻塞原因统一见 [docs/REMAINING_BLOCKERS.md](../REMAINING_BLOCKERS.md)  
> 编制日期：2026-08-31  
> 适用范围：Windows x64 便携发行版、运行依赖管理、版本检查与升级、数据迁移与回滚、GitHub Release、GHCR、系统 API、设置页和发布验收  
> 本文只定义设计契约、任务拆分、并行顺序和验收门禁，不代表相关功能已经实现。

## 1. 目标

本计划同时实现两项能力：

1. Windows 完整发行包内置经过锁定和验证的 `adb`、`ffmpeg`、`scrcpy-server.jar`，用户首次运行不需要安装 Rust、Node、adb 或 ffmpeg，也不依赖 PATH。
2. 安装基线版本后，GameBot 能检查、下载、安装后续版本；升级失败时在没有接收新业务写入的前提下自动恢复旧程序和升级前数据。

最终用户路径应为：下载完整 ZIP → 解压 → 启动 `gamer-launcher.exe` → 首次安装内置组件 → 启动服务 → 在设置页查看版本和依赖 → 后续自动检查和后台下载 → 空闲窗口安装 → 健康检查 → 成功提交或自动回滚。

## 2. 已确定的设计结论

以下结论作为首版实施契约，后续任务不得各自发明不同方案：

- Windows 首版只支持 `windows-x86_64` 便携发行版；不在没有真实设备和 CI 证据时承诺 Windows ARM64、macOS 或 Linux 裸机包。
- `adb`、`ffmpeg` 进入 Windows Release 完整包，但大型二进制不提交到 Git 历史，也不使用 Git LFS 作为运行依赖分发方式。
- Docker 镜像继续在构建阶段安装 Linux adb/ffmpeg；容器内永不覆盖自身，Docker 升级由宿主机拉取新镜像并按 digest 回滚。
- `scrcpy-server.jar` 与应用版本绑定。当前客户端协议严格对齐 3.3.3，jar 不得像 adb/ffmpeg 一样独立更新到“最新版”。
- `gamer-launcher.exe` 位于版本目录之外，负责单实例锁、依赖安装/修复、服务进程监管、版本切换、升级 journal、备份和回滚。
- `gamer-server.exe` 不自行替换程序文件，也不接受浏览器传入任意下载 URL；服务端负责业务空闲判断、系统 API、升级策略和向 launcher 发出经过鉴权的动作请求。
- Release manifest 使用 detached Ed25519 签名；签名覆盖 manifest 原始字节。SHA-256 负责内容完整性，不能替代签名和可信公钥。
- 正式版本以 `server/Cargo.toml` 的 package version 为产品版本权威源，Git tag 必须严格等于 `v<version>`；前端不得继续硬编码版本。
- 配置、数据、日志、运行依赖和版本目录完全分离。用户数据不能位于 `versions/<version>/` 内。
- SQLite 和文件布局必须有显式 schema 版本、顺序迁移和兼容范围；当前服务端基线已固定为
  SQLite schema v1，未版本化数据库不自动补齐，文件资源固定使用
  `data/<pkg>/{yaml,func,tmpl}`。
- 候选版本在提交升级前处于 activation gate：不运行 scheduler、不接受业务写请求、不建立设备会话；只有版本、schema、依赖和健康检查全部通过后才激活。
- 自动模式默认允许检查和后台下载；自动安装只在维护窗口、没有活动运行、没有正在更新的事务且近期没有 cron 触发时执行。默认产品策略建议为 `notify`，由用户显式开启 `auto`。
- 首版只承诺“前一稳定基线 → 当前稳定版本”的自动升级和 pre-commit 自动回滚，不承诺跨任意历史大版本升级，也不承诺已经正常使用新版本后的无损数据降级。

## 3. 非目标

首版不包含：

- 自动安装 Rust、Node.js、Git、Visual Studio Build Tools 等开发工具链。
- 在 Git 仓库中保存 adb/ffmpeg 发布二进制。
- 无维护窗口、无空闲检查的强制静默升级。
- 从浏览器指定自定义 URL、关闭验签或忽略哈希继续安装。
- 运行中的脚本被默认硬杀后直接覆盖数据。
- 容器内部自更新、Watchtower 默认接管、依赖可变 tag 完成不可审计升级。
- 首版 delta patch/二进制差分；先使用完整应用组件包，稳定后再评估差分下载。
- 首版同时维护 full/lite 两套公开安装体验。先发布 full；lite 仅作为后续可选优化。
- 未经许可审查直接公开再分发 Android Platform-Tools 或来源、构建选项不明的 ffmpeg 二进制。

## 4. 当前基线与主要缺口

| 范围 | 当前能力 | 自动升级前的缺口 |
|---|---|---|
| 本地启动 | `gamer.ps1` 管理前后端、优雅 shutdown、按需构建和 pnpm install | 面向开发环境；依赖 PATH、Cargo/Node；固定超时和按进程名兜底不适合作为升级器 |
| 运行依赖 | `adb_path`、`ffmpeg_path` 可配置；jar 随仓库提供 | adb/ffmpeg 需用户安装；缺少版本锁、逐文件哈希、修复和离线 seed |
| 健康检查 | `/health/ready` 检查 data、SQLite、jar、adb、ffmpeg | 只返回布尔状态；缺少 app version、boot id、schema 和候选启动阶段 |
| 产品版本 | Cargo 和 web package 均为 0.1.0；服务端日志使用 `CARGO_PKG_VERSION` | Settings/MainLayout/Login 仍硬编码；没有 tag 校验和 build metadata |
| 发布 | CI 已有 Rust/Web 测试构建门禁 | 没有 Release workflow、签名清单、Windows 包、GHCR 和 draft smoke |
| SQLite | 当前基线固定为 schema v1，启动校验 `PRAGMA user_version` | 尚未实现未来 schema 的顺序迁移、兼容范围和整批事务迁移 |
| 文件布局 | 当前只读取 `data/<pkg>/{yaml,func,tmpl}`，不读取旧布局 | 尚未实现带 journal 的可恢复升级迁移 |
| 停机 | `/api/shutdown` 先 drain run、关闭 viewer/session 再退出 | launcher 必须等待准确 PID 完整退出；不能只等 HTTP 或端口释放 |
| Docker | 一体化镜像已包含运行依赖 | compose 使用本地 `build:`；缺少 release compose、GHCR digest 和宿主升级脚本 |
| 设置页 | 有静态设置/关于原型 | 没有真实系统信息、依赖状态、升级状态和策略 API |

## 5. 目标架构

```text
GitHub Release / GHCR
        │
        ├─ signed release-manifest.json + .sig
        ├─ gamer-app-<version>-windows-x64.zip
        ├─ gamer-adb-<version>-windows-x64.zip
        ├─ gamer-ffmpeg-<version>-windows-x64.zip
        └─ GameBot-<version>-windows-x64-full.zip
        │
        ▼
gamer-launcher.exe
        ├─ manifest 验签、组件下载/seed/缓存、依赖修复
        ├─ 单实例锁、current/journal 原子状态、子进程精确监管
        ├─ 快照、迁移编排、版本切换、候选健康检查、回滚
        └─ Windows 本机 IPC
                 │
                 ▼
gamer-server.exe
        ├─ system/update API 与升级策略
        ├─ workload/cron 空闲门禁
        ├─ activation gate 与统一停机协调器
        └─ SQLite/文件 schema 与 maintenance CLI
                 │
                 ▼
Vue Settings
        └─ 版本、依赖、下载、安装、维护窗口、失败和回滚交互
```

### 5.1 安装后目录

```text
GameBot/
├─ gamer-launcher.exe
├─ config/
│  └─ config.toml
├─ data/
├─ logs/
├─ state/
│  ├─ current.json
│  ├─ update-journal.json
│  └─ launcher.lock
├─ manifests/
│  ├─ 0.2.0.json
│  └─ 0.2.0.sig
├─ versions/
│  └─ 0.2.0/
│     ├─ gamer-server.exe
│     ├─ web-dist/
│     └─ assets/scrcpy-server.jar
├─ runtime/
│  ├─ adb/<version>/
│  │  ├─ adb.exe
│  │  ├─ AdbWinApi.dll
│  │  └─ AdbWinUsbApi.dll
│  └─ ffmpeg/<version>/ffmpeg.exe
├─ seeds/                 # full 包离线组件压缩包；可配置保留用于离线修复
├─ cache/artifacts/
├─ staging/
├─ backups/<update-id>/
└─ quarantine/            # 回滚失败或损坏数据的保留区，不静默删除
```

所有 staging 必须与最终目标位于同一卷，避免把跨卷 move 错当成原子 rename。版本目录安装成功后不可原地覆盖；新版本永远写入新的 `versions/<semver>/`。

## 6. 必须先冻结的契约

### 6.1 产品版本与构建信息

- 权威版本：`server/Cargo.toml` `package.version`。
- 正式 tag：`v<semver>`；tag、Cargo version、Release manifest 不一致时 CI 立即失败。
- `web/package.json` 版本只作包元数据，CI 检查一致性，前端运行时显示服务端返回的产品版本。
- 构建注入：`version`、`git_commit`、`built_at`、`channel`、`target`。
- 本地开发必须明确显示 dev/unknown，不允许伪装成正式构建。
- 正式发布至少支持 `stable`、`beta` 两个通道；stable 不选择 prerelease。

### 6.2 Release manifest v1

manifest 建议最低字段如下；实施时应同步提供 JSON Schema、有效 fixture 和无效 fixture：

```json
{
  "schema_version": 1,
  "product": "gamebot",
  "release": {
    "version": "0.2.0",
    "channel": "stable",
    "published_at": "2026-08-31T00:00:00Z",
    "minimum_launcher_version": "0.1.0",
    "minimum_upgrade_version": "0.1.0",
    "data_schema": 2,
    "rollback_floor": 1,
    "release_notes_url": "https://example.invalid/releases/v0.2.0"
  },
  "platforms": {
    "windows-x86_64": {
      "app": {
        "artifact": {
          "name": "gamer-app-0.2.0-windows-x64.zip",
          "url": "https://example.invalid/app.zip",
          "size": 1,
          "sha256": "<64-lowercase-hex>"
        },
        "entrypoint": "gamer-server.exe"
      },
      "components": [
        {
          "id": "adb",
          "version": "<locked-version>",
          "artifact": {"name": "adb.zip", "url": "...", "size": 1, "sha256": "..."},
          "required_files": [
            {"path": "adb.exe", "size": 1, "sha256": "..."},
            {"path": "AdbWinApi.dll", "size": 1, "sha256": "..."},
            {"path": "AdbWinUsbApi.dll", "size": 1, "sha256": "..."}
          ]
        },
        {
          "id": "ffmpeg",
          "version": "<locked-version>",
          "artifact": {"name": "ffmpeg.zip", "url": "...", "size": 1, "sha256": "..."},
          "required_files": [{"path": "ffmpeg.exe", "size": 1, "sha256": "..."}]
        }
      ],
      "resources": {
        "scrcpy_server": {
          "version": "3.3.3",
          "path": "assets/scrcpy-server.jar",
          "sha256": "...",
          "binding": "application"
        }
      }
    }
  }
}
```

校验规则：

- detached signature 覆盖 manifest 原始字节；启动器先验签再解析。
- 签名包含 `key_id`；launcher 内置当前和下一把可信公钥，私钥只存在于受保护的 Release environment。
- hash 固定为小写 SHA-256；大小和文件数也必须受限。
- manifest 中的文件路径只能为规范化相对路径；拒绝绝对路径、盘符、`..`、ADS、Windows 保留名、符号链接/重解析点和大小写重复路径。
- 拒绝未知 schema、未知平台、版本降级、应用/jar 协议绑定不一致。
- URL 可以来自镜像，但信任只来自签名、公钥和内容 hash；浏览器不能覆盖 URL。
- 仓库新增 `release/dependencies.lock.toml`，固定每个依赖的版本、来源、源 hash、文件清单、构建参数和许可证。

### 6.3 稳定路径与启动参数

launcher 启动 server 时至少注入以下绝对路径：

- `GAMER_APP_DIR`
- `GAMER_DATA_DIR`
- `GAMER_ADB_PATH`
- `GAMER_FFMPEG_PATH`
- `GAMER_SCRCPY_SERVER`
- `GB_CONFIG`
- `GB_LOG`

服务端工作目录固定为当前 `versions/<version>`，但业务逻辑不能再依赖“碰巧从 server 目录启动”。配置中的相对 `data_dir` 应相对配置文件目录解析；应用资产默认相对 `GAMER_APP_DIR` 解析。

便携模式默认使用 managed runtime。需要使用系统或自定义工具时，通过 launcher 配置选择 `managed|system|custom`；custom 模式必须显式保存路径，修复器不得覆盖用户文件。

### 6.4 Launcher IPC 与 System API

Windows 首版使用当前用户可访问的 named pipe；建议形态为：

```text
\\.\pipe\gamebot-launcher-<installation-id>
```

- pipe DACL 仅允许当前用户；server 只通过 launcher 注入的 pipe 名和会话凭据连接。
- 使用长度前缀 JSON 请求/响应，包含 `protocol_version`、`request_id`、`operation`。
- 操作至少包括 `status`、`check`、`download`、`prepare_install`、`rollback`、`repair_dependency`。
- launcher 不接受 shell 命令字符串；请求转换为内部枚举。
- 直接运行 server 或 Docker 时使用 `UnsupportedUpdateController`，不因 launcher 不存在而启动失败。

新增受保护 API：

- `GET /api/system/info`
- `GET /api/system/update`
- `POST /api/system/update/check`
- `POST /api/system/update/download`
- `POST /api/system/update/install`
- `POST /api/system/update/rollback`
- `PUT /api/system/update/policy`

`/health/ready` 保持匿名、轻量和向后兼容；远程版本检查、发布说明和绝对路径不能塞进 readiness。

`GET /api/system/info` 至少返回：

- app version/commit/built_at/channel/target；
- deployment mode 和 update strategy；
- DB/file schema、rollback floor；
- adb/ffmpeg/scrcpy 的状态、版本、bundled/system/custom 来源；
- check/download/install/rollback capability；
- startup stage 和 boot id；
- 不返回盘符路径、用户名、token、密码或完整命令行。

统一错误码：

- `update_not_managed`
- `update_busy`
- `update_not_available`
- `update_not_ready`
- `signature_invalid`
- `artifact_invalid`
- `insufficient_space`
- `schema_incompatible`
- `launcher_unreachable`
- `rollback_unavailable`
- `manual_recovery_required`

install 接口返回 `202` 后由后台协调器推进；浏览器不能等待一个会因服务重启而断开的长 HTTP 请求。

### 6.5 更新策略与业务空闲门禁

策略值：

- `off`：不检查。
- `notify`：自动检查、可选后台下载，用户确认安装；建议默认。
- `auto`：后台下载，并在维护窗口满足空闲门禁后安装。

安装门禁至少包括：

- 没有 active run/starting/stopping run；
- 没有另一个升级、回滚、备份、迁移或维护事务；
- 距下一次启用 cron 的触发时间大于冻结窗口；
- launcher 和 server IPC 健康；
- 新组件已完整下载、验签、校验并位于 staging；
- 空间足够容纳 staging、当前数据快照、新旧两个版本和安全余量。

viewer 默认等待并提示，可作为策略项允许在维护窗口接管；设备 session 的拆除必须走现有优雅停机链路。

### 6.6 持久化升级状态机

launcher 使用 `state/update-journal.json` 持久化每一步；每个动作开始前先原子记录意图，动作完成后再推进状态。

```text
idle
 → checking
 → downloading
 → verifying
 → staged
 → waiting_idle
 → draining
 → stopped
 → snapshotting
 → snapshot_verified
 → migrating
 → switched
 → candidate_starting
 → candidate_ready
 → activating
 → committed
 → cleaning
 → idle
```

失败分支：

- `checking..waiting_idle`：旧服务不动，回到 idle/staged。
- `draining`：准确 PID 未退出则默认取消升级；只有用户显式选择强制升级才允许硬杀，并记录 `dirty_shutdown`。
- `stopped` 后快照前：未改数据，直接重启旧版本。
- `snapshot_verified` 后到 committed 前：停止候选、隔离失败数据、恢复快照、切回 previous、启动旧版本并验证。
- 回滚也失败：进入 `manual_recovery_required`，保留 journal、快照、新旧版本和 quarantine，停止自动重试循环。

journal 至少记录 update id、from/to、准确 child PID/创建时间/exe、current/previous、snapshot、schema before/after、最后完成步骤和错误摘要。

### 6.7 Schema、迁移与回滚承诺

- SQLite 使用 `PRAGMA user_version` 或等价 `schema_migrations`；每个迁移单独编号并在事务内完成 DDL、数据修复和版本推进。
- 当前 schema v1 是唯一基线：新库直接创建 v1；`user_version=0` 或缺少版本号的既有库明确拒绝启动，
  不存在 migration 0。后续迁移从 v1→v2 起按编号执行。
- binary 声明 `min_read_schema`、`max_read_schema`、`target_schema`；数据库比 binary 更新时明确拒绝启动。
- 文件迁移采用 plan → staging copy → hash/validate → marker 的顺序；旧源文件至少保留到升级提交和回滚保留期结束。
- 文件迁移有独立 journal，重复运行必须幂等；不得把混合布局误标为成功。
- 新增 server maintenance CLI：`inspect`、`migrate --data-dir <path> --json`，不启动 adb、scheduler、HTTP 或设备扫描。
- 升级前先在数据副本执行 maintenance preflight，再在旧进程完全退出后创建正式离线快照。
- 自动回滚只承诺 committed 之前的事务。committed 后人工降级：schema 兼容时可切 binary；不兼容时恢复旧快照必须明确提示会丢失升级后的数据。

### 6.8 Candidate activation gate

候选启动分为以下阶段：

1. 打开并迁移数据库，验证配置、数据目录和依赖。
2. 绑定端口，但路由处于 maintenance gate；仅允许本机启动器探针、健康信息和激活请求。
3. 不启动 scheduler、设备扫描、watchdog，不接受业务写 API。
4. launcher 校验 boot id、app version、schema 和目标 release 一致后请求 activate。
5. 初始化 DeviceManager；依赖/设备启动条件通过后才初始化 Scheduler。
6. 打开业务路由，再等待正式 `/health/ready`。
7. launcher 写入 committed；之后的新业务写入不再属于自动快照回滚承诺。

## 7. 工作流与任务拆分

### 7.1 契约与基础架构轨

| ID | 任务 | 主要产出/文件 | 前置 | 验收标准 |
|---|---|---|---|---|
| ARC-001 | 冻结目录和组件边界 | 本文、后续 `docs/guides/UPDATE_CONTRACT.md` | 无 | 目录、owner、可写区域、原子切换边界评审通过 |
| ARC-002 | 冻结 manifest 和签名规则 | JSON Schema、valid/invalid fixtures | ARC-001 | fixture 可自动校验；改一字节验签失败 |
| ARC-003 | 冻结 API、IPC、状态和错误码 | OpenAPI/JSON fixtures、IPC protocol v1 | ARC-001 | launcher/server/web 均以同一 fixture 开发 |
| ARC-004 | 冻结 schema 兼容和回滚承诺 | migration policy、compatibility table | ARC-001 | 明确 pre/post commit 行为和人工恢复边界 |
| ARC-005 | 第三方分发决策 | adb/ffmpeg 来源、许可、NOTICE 策略 | 无 | 未完成不得公开 full 包 |
| VER-001 | 产品版本单一来源 | 版本检查脚本、tag 校验 | ARC-001 | tag/Cargo/Web/manifest 不一致时 CI 失败 |
| VER-002 | 构建信息模块 | server build info + tests | VER-001 | 正式/dev 字段明确，无秘密 |
| PATH-001 | 稳定路径契约 | `config.rs` 与路径测试 | ARC-001 | 任意 cwd、空格/中文路径读写正确 |
| PATH-002 | 静态资源路径 | router web-dist 由 app dir 提供 | PATH-001 | 版本目录只读时仍可提供前端 |

### 7.2 数据与服务生命周期轨

| ID | 任务 | 主要产出/文件 | 前置 | 验收标准 |
|---|---|---|---|---|
| DATA-001 | SQLite 编号迁移框架 | `store` migration module | ARC-004 | 单 migration 事务化、重复运行 no-op |
| DATA-002 | 当前数据库 schema v1 baseline | schema v1 fixtures | DATA-001 | 新库得到确定 schema；无版本号旧库明确拒绝且不改数据 |
| DATA-003 | schema 兼容门禁 | binary min/max/target schema | DATA-001 | newer schema 被明确拒绝，错误可诊断 |
| DATA-004 | 可恢复文件迁移框架 | scripts migration plan/journal | ARC-004 | 中断后可 resume/rollback，源未丢 |
| DATA-005 | Maintenance CLI | inspect/migrate JSON 模式 | DATA-002、DATA-004 | 不启动 HTTP/adb/scheduler 即可完整预检 |
| DATA-006 | 数据 migration fixtures | n-1→n、碰撞、失败注入 | DATA-005 | 每个失败点保持可重试或可恢复 |
| OPS-001 | 统一 shutdown coordinator | main/API/signal 共用协调器 | ARC-003 | API、Ctrl+C、SIGTERM 都 drain 同一路径 |
| OPS-002 | 幂等停机和状态查询 | draining/stopping/finished 状态 | OPS-001 | 多次 shutdown 不重复执行，状态可观测 |
| OPS-003 | 精确退出契约 | launcher 等 child handle/PID | OPS-002 | 不按端口或模糊进程名判定完成 |
| OPS-004 | Candidate activation gate | main/system middleware/startup stage | DATA-003、ARC-003 | commit 前 scheduler 和业务写入均为 0 |
| OPS-005 | Workload/cron 空闲摘要 | active runs/viewers/next trigger | ARC-003 | auto 策略可稳定判断是否进入 drain |

### 7.3 Launcher 与依赖管理轨

| ID | 任务 | 主要产出/文件 | 前置 | 验收标准 |
|---|---|---|---|---|
| LCH-001 | 新建独立 launcher crate | `launcher/` 骨架、CLI、日志 | ARC-001 | `start/status/doctor/repair` 命令可运行 |
| LCH-002 | 单实例与原子 state | lock/current/journal store | LCH-001 | 并发仅一个写者；半截 JSON 可恢复 |
| LCH-003 | Manifest parser 和验签 | Ed25519/key id/schema parser | ARC-002、LCH-001 | 未签、篡改、错平台、未知 schema fail closed |
| LCH-004 | 组件库存与深检 | required files/hash/probe 状态 | LCH-001、ARC-002 | 缺 adb DLL、ffmpeg 损坏、版本错误可定位 |
| LCH-005 | 下载和本地 seed/cache | bounded HTTP、临时文件、代理 | LCH-003 | 截断/超时/hash 错不污染安装目录 |
| LCH-006 | 安全解压和原子安装 | staging/validate/rename | LCH-005 | zip-slip、炸弹、链接、大小写碰撞被拒 |
| LCH-007 | 依赖修复编排 | inventory→seed/cache→remote→probe | LCH-004、LCH-006 | full 断网首启；删除依赖后可离线修复 |
| LCH-008 | Server supervisor | env/cwd/child handle/health | PATH-001、VER-002、LCH-007、OPS-002 | PATH 清空仍能启动；无孤儿 server |
| LCH-009 | Launcher IPC server | named pipe + protocol v1 | ARC-003、LCH-002 | 仅当前用户可访问；并发请求幂等 |
| LCH-010 | 升级状态机编排 | journal/check/download/switch | LCH-003、LCH-008、LCH-009 | 每步崩溃后可恢复到稳定状态 |
| LCH-011 | 快照与恢复 | backup manifest/hash/staging swap | DATA-005、OPS-003、LCH-002 | 快照不完整时不迁移；恢复原子且可验证 |
| LCH-012 | 候选启动、提交与回滚 | activation/ready/commit/rollback | OPS-004、LCH-010、LCH-011 | 错版本/错 schema/ready 失败自动回旧版 |
| LCH-013 | Launcher 自更新 trampoline | 临时 helper/版本门禁 | LCH-012 | launcher 被占用时两阶段替换；失败保留旧 launcher |

### 7.4 依赖与发布轨

| ID | 任务 | 主要产出/文件 | 前置 | 验收标准 |
|---|---|---|---|---|
| DEP-001 | 依赖锁文件 | `release/dependencies.lock.toml` | ARC-005 | 版本、URL、源 hash、NOTICE、构建参数齐全 |
| DEP-002 | adb 获取与裁包 | 打包脚本、逐文件 hash | DEP-001 | 包含 exe+两个 DLL；干净 VM 运行 adb 命令 |
| DEP-003 | ffmpeg 来源/精简构建 | 构建脚本、buildconf、source offer | DEP-001 | 真实 H.264 pipe→PNG 命令成功 |
| DEP-004 | scrcpy 强绑定门禁 | 代码常量/jar/manifest 检查 | ARC-002 | 三者版本/hash 不一致发布失败 |
| DEP-005 | 第三方声明 | licenses/NOTICE/SBOM 输入 | DEP-002、DEP-003、DEP-004 | full 包内声明完整且与实际二进制一致 |
| REL-001 | Windows app 组件包 | package-windows 脚本 | PATH-002、VER-002 | clean checkout 生成确定布局 |
| REL-002 | Full bootstrap 包 | launcher+manifest+seeds | LCH-007、DEP-005、REL-001 | 断网、清空 PATH 的首次启动成功 |
| REL-003 | Manifest 生成与签名 | manifest、sig、SHA256SUMS | ARC-002、REL-001、DEP-005 | 发布后重新下载仍能验签 |
| REL-004 | Release workflow | `.github/workflows/release.yml` | VER-001、REL-002、REL-003 | tag 先产 draft，全部 smoke 后发布 |
| REL-005 | GHCR 发布 | version/digest/OCI labels/SBOM | VER-001 | ZIP 与镜像 app version/commit 一致 |
| REL-006 | 供应链证明和密钥轮换演练 | attestation、key rotation runbook | REL-003、REL-004 | 当前/下一公钥验证通过，旧 key 可撤换 |

### 7.5 Server API 与前端轨

| ID | 任务 | 主要产出/文件 | 前置 | 验收标准 |
|---|---|---|---|---|
| SYS-001 | `/api/system/info` | build/deploy/schema/dependencies/capabilities | VER-002、DATA-003、ARC-003 | 鉴权通过且不泄露路径/秘密 |
| SYS-002 | 依赖版本探针 | adb/ffmpeg/scrcpy probe | DEP-001 | 超时有界；真实版本和来源可显示 |
| SYS-003 | UpdateController trait | launcher/unsupported/docker adapters | ARC-003 | 无 launcher 不影响 server 启动 |
| SYS-004 | Update query/action API | 独立 update API 模块 | SYS-003、LCH-009 | 状态机/错误码/鉴权/并发测试通过 |
| SYS-005 | 更新策略协调器 | notify/auto/maintenance window | OPS-005、SYS-004 | busy 时只等待；满足门禁才 prepare install |
| SYS-006 | install 202 与后台停机接线 | update→shutdown coordinator | OPS-002、SYS-005 | 202 先返回，随后才停止服务 |
| WEB-001 | System/update API client | 独立 client/store/composable | ARC-003 | 完全基于冻结 fixture 单测 |
| WEB-002 | 系统与依赖卡片 | 新组件 | WEB-001 | 版本/依赖/来源/损坏态正确显示 |
| WEB-003 | 更新状态卡片 | 新组件 | WEB-001 | 全状态、进度、失败、等待和回滚均可展示 |
| WEB-004 | 安装/回滚确认流程 | modal 与 202 断连语义 | WEB-003 | 重启断连不误报失败；重连按新版本判定 |
| WEB-005 | 重做 Settings | 替换静态原型 | WEB-002、WEB-004 | 删除“已保存（原型）”假交互 |
| WEB-006 | 全站版本收口 | MainLayout/Login/Settings | SYS-001 | 无硬编码 `v0.1.0`，混包有明确警告 |

### 7.6 Docker、测试和文档轨

| ID | 任务 | 主要产出/文件 | 前置 | 验收标准 |
|---|---|---|---|---|
| DKR-001 | Release compose | 独立 compose 使用 `${GAMER_IMAGE}` | REL-005 | 开发 compose 不变；release compose config 通过 |
| DKR-002 | 宿主升级脚本 | pull/backup/switch digest/ready | DKR-001、OPS-001 | 新镜像不健康自动回旧 digest |
| DKR-003 | Docker system capability | external strategy | SYS-003 | UI 不显示无效的“立即安装” |
| DKR-004 | Docker 升降级 E2E | 临时数据卷与 digest 测试 | DKR-002 | 数据保持、SIGTERM 优雅清理、回滚通过 |
| QA-001 | Manifest/path/security fixtures | parser/签名/路径反例 | ARC-002 | 所有拒绝规则自动化 |
| QA-002 | Launcher 本地下载集成 | slow/断流/Range/404/篡改 | LCH-005、LCH-006 | 失败不污染 runtime，重试不死锁 |
| QA-003 | Migration 故障注入 | SQL/文件每个边界失败 | DATA-006 | schema 不越级、源文件不丢 |
| QA-004 | Journal 断电矩阵 | 每个持久状态点 kill/restart | LCH-012 | 只得到新版健康、旧版健康或人工恢复三类结果 |
| QA-005 | Windows clean VM | Win10/11、中文/空格/断网 | REL-002、LCH-012 | full 首启、修复、升级、回滚全通过 |
| QA-006 | 业务空闲竞争 | run/viewer/cron/update 并发 | SYS-006 | auto 不误杀运行，cron 不重复执行 |
| QA-007 | 大数据与磁盘压力 | 1GB DB/大量小文件/磁盘满 | LCH-011 | 空间不足前置拒绝，不产生半快照 |
| QA-008 | RC 外部下载冒烟 | GitHub/GHCR 重新下载 | REL-004、REL-005 | 不只验证 workspace 内产物 |
| DOC-001 | 用户安装升级文档 | README/UPDATE | M1/M2 | 可照文档完成安装、修复、升级、回滚 |
| DOC-002 | 维护者发布手册 | RELEASE/key rotation/manual recovery | REL-006、LCH-012 | 草稿发布和人工恢复可演练 |
| DOC-003 | PITFALLS 持续收口 | `docs/PITFALLS.md` | 实施全过程 | 每个真实坑一句现象+原因+规避 |

## 8. 推荐并行开发安排

### 8.1 四人团队的文件所有权

| 角色/轨道 | 主要任务 | 独占目录/文件面 |
|---|---|---|
| A：Launcher/Updater | LCH-001～013、QA-002/004 | `launcher/**`、launcher fixtures |
| B：Server/Data | PATH、DATA、OPS、SYS | `server/src/config.rs`、migration 模块、system/update API |
| C：Dependencies/Release/Docker | DEP、REL、DKR | `release/**`、`licenses/**`、打包脚本、release workflow/compose |
| D：Web/QA/Docs | WEB、QA fixtures、文档 | `web/src` 新组件、前端测试、测试编排、docs |

指定一名集成人独占以下热点文件的最终接线，其他轨道先通过新模块/fixture 开发，不同时修改热点：

- `server/src/main.rs`
- `server/src/api/mod.rs`
- `web/src/api.js`
- `web/src/views/Settings.vue`
- `.github/workflows/release.yml`
- `gamer.ps1`

### 8.2 批次 0：串行冻结契约

任务：ARC-001～005。

完成门：

- manifest JSON Schema、签名字节规则、目录、IPC/API fixture、状态机和回滚承诺全部评审通过。
- adb 再分发和 ffmpeg 许可路径有明确结论；未完成时只允许内部原型，不允许公开 full 包。
- 后续轨道只能通过版本化契约变更，不得口头改字段。

预计：2～4 人日，日历 1～2 天。

### 8.3 批次 1：四轨基础并行

- A：LCH-001、LCH-002、LCH-003，并同步写 QA-001。
- B：VER-001、VER-002、PATH-001、DATA-001、DATA-004、OPS-001。
- C：DEP-001，然后 DEP-002、DEP-003、DEP-004 并行。
- D：WEB-001～004 基于冻结 fixture 开发；不等待真实 API。

完成门：launcher 能解析/验证 fixture；server 有稳定路径和 migration 骨架；依赖来源已固定；前端状态机不依赖后端临时字段。

预计：10～15 人日，4 人日历 3～5 天。

### 8.4 批次 2：Windows 完整包 MVP 并行

- A：LCH-004～008，完成依赖 inventory、seed、修复和 server supervisor。
- B：PATH-002、DATA-002/003/005、SYS-001/002、OPS-002/003。
- C：DEP-005、REL-001～003。
- D：WEB-002/003 组件测试、QA-002/003 的故障夹具、DOC 初稿。

完成门：从 clean checkout 生成 full ZIP；在 PATH 清空、断网环境解压后可启动；删除 adb 任一 DLL 或 ffmpeg 后能从本地 seed 修复；`/api/system/info` 返回真实版本和依赖。

这是第一个可交付里程碑 M1，可作为一次手工安装的 updater 基线版本。

预计：12～18 人日，4 人日历 4～6 天。

### 8.5 批次 3：自动升级与回滚并行

- A：LCH-009～012，升级 journal、快照、候选和回滚。
- B：OPS-004/005、SYS-003～006、DATA-006。
- C：REL-004～006、REL-005 GHCR、DKR-001。
- D：WEB-005/006、QA-004/006，并把 fixture 切换为真实 API contract tests。

完成门：M1 基线可升级到 M2 候选；install API 先返回 202；candidate commit 前无 scheduler/业务写入；候选错版本、迁移失败或 readiness 失败会恢复旧程序和快照。

预计：16～24 人日，4 人日历 5～8 天。

### 8.6 批次 4：集成和 Docker 轨道

- 指定集成人统一修改热点文件。
- Settings 从 fixture 切换到真实 server/launcher IPC。
- Release workflow 连接 Windows、manifest、签名、GHCR 和 draft smoke。
- 完成 DKR-002～004；容器 SIGTERM 必须走统一 shutdown。
- 完成 launcher 自更新 LCH-013；若首版 launcher 不需要升级，可将其降为 M2 后的独立里程碑，但 manifest 必须从第一版保留最低 launcher 版本字段。

完成门：开发启动、Windows 便携启动和 Docker 三种模式互不破坏；unsupported capability 降级明确。

预计：8～12 人日，4 人日历 3～5 天。

### 8.7 批次 5：RC 与故障注入

- 完成 QA-005/007/008 和全量门禁。
- 使用临时 tag 或 draft release 演练，不拿第一个正式 tag 调试发布脚本。
- 完成 licenses、NOTICE、SBOM、attestation、安装/恢复手册。
- 真机验证升级前运行、升级门禁、升级后投屏/控制/截图/脚本/定时任务。

完成门：§11 所有发布门禁通过，才发布 stable。

预计：10～16 人日，日历 3～6 天，受 clean VM、真机和许可审查影响。

## 9. 任务依赖图与关键路径

```text
ARC-001..005
   ├─ VER/PATH ───────────────┐
   ├─ DATA/OPS ───────────────┤
   ├─ LCH-001→003 ────────────┤
   ├─ DEP-001→002/003/004 ────┤
   └─ WEB fixture 开发 ───────┘
                    │
                    ▼
M1: PATH + DATA baseline + LCH dependency repair + DEP packages + system info + full ZIP
                    │
                    ▼
LCH IPC/journal/snapshot ─┬─ OPS activation gate/workload
                         ├─ SYS update API/policy
                         ├─ Release/GHCR
                         └─ Settings integration
                    │
                    ▼
M2: N-1→N upgrade + pre-commit automatic rollback
                    │
                    ▼
Docker external update + launcher self-update + chaos/VM/real-device RC
```

关键路径通常为：

```text
ARC-002
 → LCH-001
 → LCH-003
 → LCH-005
 → LCH-006
 → LCH-007
 → LCH-008
 → LCH-010
 → LCH-011
 → LCH-012
 → QA-004/005
```

另一个可能阻塞关键路径的是 ARC-005 → DEP-001 → DEP-002/003；应在第一周完成许可和来源决策，不能到发布前才处理。

## 10. 里程碑与工作量

| 里程碑 | 主要能力 | 预计人日 | 说明 |
|---|---|---:|---|
| M0 契约冻结 | manifest/API/IPC/schema/许可结论 | 2～4 | 所有并行工作的入口 |
| M1 Windows full 基线 | 内置依赖、离线首启、修复、launcher 托管、system info | 20～30 | 需要用户手工安装一次，成为后续 N-1 |
| M2 自动升级 | 检查/下载/空闲安装、数据快照、activation gate、自动回滚、UI | 24～36 | 通过 M1→M2 真实演练 |
| M3 Docker/供应链/硬化 | GHCR、digest 回滚、自更新、故障注入、真机 RC | 12～20 | 可按需求拆后续版本 |
| **总计** |  | **58～90 人日** | 含 clean VM、故障注入、许可和发布演练；不等于纯编码时间 |

四人并行时，理想日历时间约 3～5 周。若目标是更快交付，可先在 1.5～2.5 周内完成 M1，再用独立版本完成 M2；不建议为了压缩时间删除 schema、快照、签名或 activation gate。

## 11. 测试矩阵与发布硬门禁

### 11.1 Manifest、供应链与解压

- 未签名、错误 key、未知 key、manifest 改一字节、asset 改一字节全部拒绝。
- stable 不选择 beta；低版本默认不覆盖高版本。
- 绝对路径、`..`、ADS、保留名、大小写碰撞、重复文件、符号链接和 reparse point 拒绝。
- 超出最大压缩包、最大单文件、最大文件数、最大解压总量的 archive 拒绝。
- 签名私钥在 PR/普通 CI 不可用；Release environment 才有权限。
- 同一 tag 已有不同 hash 资产时发布失败，不静默覆盖。

### 11.2 运行依赖

- Windows adb 包包含 `adb.exe`、`AdbWinApi.dll`、`AdbWinUsbApi.dll`，逐文件校验。
- 干净 VM 执行 adb version/start-server/devices/kill-server。
- ffmpeg 不只验证 `-version`，必须执行项目真实 H.264 stdin → PNG stdout 命令。
- jar 常量、版本、hash 与 manifest 不一致时阻断发布。
- PATH 为空且完全断网时 full 包首次启动成功。
- 删除或篡改任一依赖文件后 repair 能恢复；失败不破坏上一份 runtime。
- managed/system/custom 三种模式不互相覆盖。

### 11.3 数据、停机和升级

- `/shutdown` 响应丢失、超时、设备 reverse 清理超时，launcher 不提前复制数据库。
- 正常路径等待准确 child handle/PID 退出；硬杀必须显式且 journal 标记 dirty。
- SQLite 每条 migration 前/后失败，user_version 不越级；重复执行 no-op。
- 文件迁移每个 copy/hash/rename/marker 边界失败，源不丢且可重试。
- 快照磁盘满、权限拒绝、hash 不符时 current 不切换、旧服务可恢复。
- candidate 立即退出、端口占用、ready 永久 503、wrong version/schema/boot id、adb/ffmpeg 损坏均自动回滚。
- 回滚失败停止循环并保留 manual recovery 所需全部证据。
- committed 前 scheduler/业务写 API 零写入；committed 后人工降级遵循兼容表。

### 11.4 业务并发

- 活动脚本永不结束：auto 只等待；manual 明确 drain/取消策略。
- viewer 在线、两个定时任务同秒、cron 恰好到点时不重复运行、不注入控制。
- 两个 install 请求只有一个取得事务；第二个返回稳定 busy 状态。
- 多 launcher 启动只有一个持有锁；其他实例只显示状态。
- 页面卸载停止无意义轮询；只有活动更新时高频查询。

### 11.5 Windows 环境

- Windows 10/11 x64 clean VM。
- 安装路径包含空格、中文、长路径；data 与程序位于不同磁盘。
- 杀毒软件短暂占用 current/journal/exe 时可重试且不误删 previous。
- 用户注销、Windows 重启、launcher 被杀后能从 journal 恢复。
- 版本目录只读；仅 config/data/logs/state/cache/staging/backups 可写。
- 包内没有密码配置、测试 DB、日志、Cargo target、node_modules 或私钥。

### 11.6 API 与前端

- system/info 和 update API 未登录 401；跨站状态变更 403。
- 响应不出现盘符、用户目录、token、password 或完整命令行。
- Docker/direct 模式 install 返回 `update_not_managed`，能力按钮隐藏或禁用。
- install 202 后连接断开不显示“安装失败”；重连后用 app version/boot id 判定结果。
- idle/checking/available/downloading/staged/waiting/installing/restarting/failed/rolling_back/manual recovery 全状态测试。
- MainLayout、Settings、Login 没有硬编码版本；server/web 混包显示明确告警。

### 11.7 Docker 与发布

- 开发 compose、release compose、USB override、redroid profile 全部通过 `docker compose config`。
- `docker stop`/SIGTERM 走统一 drain 和 scrcpy/adb 清理。
- OCI label、API version、tag、digest、ZIP manifest 指向同一 commit。
- 新镜像不健康按旧 digest 恢复，绑定数据目录内容保持。
- 容器 update capability 永远为 external/false。
- Release draft 上传完成后从 GitHub/GHCR 重新下载并冒烟，再转正式发布。

## 12. 提交和合并策略

建议按以下主题提交，每个提交应自洽、可独立审查和回滚：

1. `docs(release): 冻结便携目录、升级清单与系统接口契约`
2. `build(version): 统一产品版本和构建元数据`
3. `refactor(config): 支持启动器注入绝对运行路径`
4. `refactor(data): 引入编号化 SQLite 事务迁移`
5. `refactor(data): 将文件布局迁移改为可恢复事务`
6. `fix(api): 统一信号和接口触发的优雅停机`
7. `feat(server): 增加维护模式与候选激活闸`
8. `feat(api): 暴露系统、依赖与更新控制接口`
9. `feat(launcher): 搭建进程监管和原子状态管理`
10. `feat(launcher): 校验签名清单和组件完整性`
11. `feat(launcher): 实现安全下载、解压和依赖修复`
12. `feat(launcher): 实现快照、版本切换和自动回滚`
13. `build(release): 固定并打包 Windows adb 和 ffmpeg`
14. `build(release): 生成签名的 Windows 完整发行包`
15. `ci(release): 发布 GitHub Release 与 GHCR 镜像`
16. `feat(web): 展示系统依赖和版本升级状态`
17. `feat(docker): 增加镜像升级和按摘要回滚轨道`
18. `test(release): 覆盖离线首启、迁移失败和断电恢复`
19. `docs(release): 补充安装、升级、恢复和第三方声明`

并行开发规则：

- 每条轨道使用独立 branch/worktree，避免共享工作区中一方编译到另一方未完成代码。
- 一个任务只提交自己拥有的路径；不要使用无路径限制的 `git add -A`。
- contract fixture 变更必须单独提交并通知所有轨道同步，不能夹在实现提交里。
- 热点文件由集成人合流，子任务通过新增模块暴露接口，减少同时编辑同一文件。
- 各轨道先跑针对性测试；进入批次集成门后再统一跑全量门禁。
- 开发中踩到的新坑按仓库要求追加到 `docs/PITFALLS.md`，不把流水账写进计划。

## 13. 每批次通用质量门禁

每个批次合流前至少执行：

```powershell
cd server
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

cd ../web
pnpm test:run
pnpm build

cd ..
docker compose config
powershell -NoProfile -ExecutionPolicy Bypass -File tools/verify-release.ps1
```

新增 launcher 后还必须执行 launcher 自身 fmt/clippy/test；建议在仓库根建立统一 workspace 或由 CI 显式进入 launcher crate，不能出现本地测试过而 CI 漏跑的新 crate。

## 14. 发布与回滚策略

建议用两个公开里程碑验证“更新器更新自己管理的应用”而不是在同一个版本里自证：

1. 基线版本：提供 full ZIP、launcher、内置依赖、显式 migration framework 和 system info；首次仍由用户手工安装。
2. 下一版本：通过基线 launcher 自动发现、下载、快照、迁移、切换和回滚，形成真实 N-1→N 证据。

成功升级后至少保留：

- 当前版本；
- previous 版本；
- 与 previous 对应且已验证的升级前快照；
- 当前 manifest、签名、journal 和升级日志。

清理按数量、年龄和磁盘上限执行，但不得删除 current、previous、进行中的 staging、唯一有效 rollback point 或 manual recovery 所需证据。

## 15. 风险与控制

| 风险 | 影响 | 控制 |
|---|---|---|
| adb 再分发许可未确认 | full 包无法公开发布 | ARC-005 作为批次 0 硬门禁；保留内部 seed/lite 退路 |
| ffmpeg 构建含 GPL/nonfree | 许可义务与预期不一致 | 锁定 buildconf、源代码/offer、NOTICE；优先评估 LGPL 最小构建 |
| manifest 只有 hash 没有签名 | 攻击者可同时替换包和 hash | Ed25519 原始字节验签，公钥内置，Release 私钥隔离 |
| 当前路径依赖 cwd | 升级后读错数据、jar 或 web-dist | PATH-001/002 必须先于 launcher 集成 |
| 固定停机超时提前硬杀 | SQLite/adb 未收尾导致损坏或残留 | 等准确 child handle；默认不硬杀；dirty shutdown 另走完整性检查 |
| 候选启动即运行 scheduler | 失败回滚吞掉候选期间新写入 | activation gate；commit 前禁业务写和 scheduler |
| migration 无版本/事务 | 不能判断是否可回滚 | 编号 migration、兼容范围、离线快照和 maintenance CLI |
| Windows 文件占用 | current/exe/journal 替换失败 | 同卷 staging、原子 replace、有界重试、previous 延迟清理 |
| 更新器自身损坏 | 后续无法启动或修复 | 外置 trampoline、最低 launcher 版本、手工恢复包 |
| 并行开发热点冲突 | 合并返工、接口漂移 | 先冻结 fixture、目录所有权、唯一集成人、分批合流 |

## 16. 最终 Definition of Done

只有同时满足以下条件，两个功能才算完成：

1. Windows full ZIP 在无 Node/Rust/adb/ffmpeg、PATH 清空和断网的 clean VM 中首次启动成功。
2. adb、ffmpeg 和 scrcpy 的版本、来源、hash、许可及真实功能探针全部可追溯。
3. 任一依赖缺失或损坏都能安全修复，失败不会破坏上一份 runtime。
4. 前端显示服务端真实版本和依赖状态，没有硬编码产品版本。
5. 基线版本到下一版本完成真实自动升级，活动脚本和临近 cron 能阻止 auto install。
6. 候选版本在 committed 前没有业务写入；启动、迁移或 readiness 失败自动恢复旧程序和数据快照。
7. 状态机每个持久化边界的 kill/restart 测试不会产生双进程、双 current 或无限回滚。
8. Docker 使用外部镜像升级和 digest 回滚，容器内不自更新，SIGTERM 走统一优雅停机。
9. GitHub Release、GHCR、SBOM、签名、attestation、NOTICE 和重新下载冒烟全部通过。
10. Rust、launcher、web、compose、release preflight、Windows clean VM 和至少一台真实 Android 设备验收通过。
11. README、升级/恢复手册和真实新增 PITFALLS 已同步。

## 17. 实施 Checklist

### 17.1 勾选规则

- [ ] 每个任务指定唯一负责人、独立 branch/worktree 和预期合入批次。
- [ ] 任务只有在代码、针对性测试、必要文档和验收证据全部完成后才能勾选。
- [ ] 仅完成代码但未通过前置契约、故障测试或合流门的任务保持未勾选。
- [ ] contract fixture、manifest schema、API/IPC 字段发生变化时，单独提交并通知所有轨道同步。
- [ ] 每个批次结束时记录 commit、测试命令、结果和未完成项，不用口头结论代替证据。
- [ ] 真实踩坑已按“一句话现象 + 原因 + 解决/规避”追加到 `docs/PITFALLS.md`。

### 17.2 批次 0：契约与许可冻结

- [x] ARC-001：冻结便携安装目录、组件边界、可写目录和原子切换边界。（docs/guides/UPDATE_CONTRACT.md）
- [x] ARC-002：完成 Release manifest v1 JSON Schema。（release/contracts/manifest-v1.schema.json）
- [x] ARC-002：提供合法 manifest、签名和平台 fixture。（fixtures/manifest/valid/ 2 组 + 测试公钥）
- [x] ARC-002：提供未知 schema、错误平台、降级、危险路径和篡改签名反例 fixture。（invalid/ 24 条，selftest 28/28）
- [x] ARC-003：冻结 System API 请求、响应、状态和错误码。（release/contracts/system-api-v1.md + 26 fixtures）
- [x] ARC-003：冻结 launcher IPC protocol v1、消息上限、超时和幂等语义。（release/contracts/ipc-v1.md + 8 fixtures）
- [x] ARC-004：冻结 DB/file schema 兼容表和 rollback floor。（release/contracts/schema-policy.md，与 store.rs v1 基线对齐）
- [x] ARC-004：明确 pre-commit 自动回滚与 post-commit 人工降级承诺。（schema-policy.md §6）
- [x] ARC-005：确认 Android Platform-Tools 获取与再分发方式。（官方 platform-tools，Apache-2.0 NOTICE，三件套原字节分发）
- [x] ARC-005：确认 ffmpeg 来源、构建配置和 LGPL/GPL 履约方式。（BtbN win64-lgpl，LGPL-3.0-or-later，禁 gpl/nonfree 门禁）
- [x] ARC-005：冻结第三方 NOTICE、source offer 和 SBOM 策略。（release/contracts/dependency-licensing.md，CycloneDX JSON）
- [x] 批次 0 合流门：所有契约通过评审，四条轨道可只依赖版本化 fixture 开工。

### 17.3 批次 1：四轨基础

#### Launcher/Updater 轨

- [x] LCH-001：建立独立 launcher crate 和 `start/status/doctor/repair` CLI 骨架。（launcher/ 独立 crate，clap CLI + tracing 日志）
- [x] LCH-002：实现安装实例单写锁。（CreateFile 独占 + 崩溃遗留锁接管，status 只读旁路）
- [x] LCH-002：实现 `current.json` 和 `update-journal.json` 原子读写与损坏恢复。（临时文件+同卷 rename，损坏备份 .corrupt-<ts>）
- [x] LCH-003：实现 manifest 原始字节 Ed25519 验签。（ed25519-dalek，fail closed：读字节→验签→解析→语义）
- [x] LCH-003：实现 key id、当前/下一公钥和未知 key 拒绝。（信任库 <keys-dir>/<key_id>.pem，未知 key_id 拒绝）
- [x] LCH-003：实现 schema、平台、SemVer、降级和路径安全校验。（语义错误码 + 路径安全全拒绝规则）
- [x] QA-001：manifest、签名和路径正反例测试全部通过。（launcher 集成测试直接跑 26 个契约 fixtures，单字节翻转必拒）

#### Server/Data 轨

- [x] VER-001：以 Cargo package version 为产品版本权威源。（tools/check-version.ps1）
- [x] VER-001：CI 校验 tag、Cargo、Web 和 manifest 版本一致。（ci.yml 新增 version job，pwsh）
- [x] VER-002：注入 commit、built_at、channel 和 target 构建信息。（server/build.rs + build_info.rs，dev 缺省明确降级）
- [x] PATH-001：支持 launcher 注入的绝对 app/data/tool/jar/config/log 路径。（config.rs PathEnv 环境变量覆盖，未设置行为不变）
- [x] PATH-001：相对 data/config 路径按冻结契约解析，不依赖偶然 cwd。（相对配置文件目录/app dir 解析，中文空格路径测试）
- [x] DATA-001：建立编号化 SQLite migration 框架。（migrations.rs 逐级单事务推进，v1 唯一基线语义不变）
- [x] DATA-004：建立文件迁移 plan、staging、hash、marker 和 journal 骨架。（file_migration.rs 纯库 + 10 单测，未接线）
- [x] OPS-001：API shutdown、Ctrl+C 和 SIGTERM 接入同一停机协调器。（`server/src/main.rs` 已修复统一 drain 安装顺序竞态；三条入口共用 `ShutdownCoordinator`，静态确认 drain 顺序为 `run→viewer→scrcpy/session/adb`；真实 Docker stop/SIGTERM 仍未验收）

#### Dependencies/Release 轨

- [x] DEP-001：建立 `release/dependencies.lock.toml`。
- [x] DEP-001：锁定依赖版本、来源 URL、源 hash、产物文件和许可元数据。（adb 37.0.1 / ffmpeg N-126335-gb32f8d1c23-20260830 / scrcpy 3.3.3 全实算 hash）
- [x] DEP-002：固定并裁剪 Windows adb 组件。（fetch-adb.ps1 下载+裁包+逐文件校验）
- [x] DEP-002：确认 adb 包含 exe 和两个配套 DLL。（clean VM 运行验证留待批次 5 QA-005）
- [x] DEP-003：固定或构建 Windows ffmpeg 组件。（fetch-ffmpeg.ps1，BtbN win64-lgpl）
- [x] DEP-003：归档实际 `-buildconf`、源码版本和许可材料。（BUILD-CONFIG.txt 入 vendor 归档，锁文件记 buildconf 关键行与源码 offer）
- [x] DEP-004：建立 scrcpy 代码常量、jar 版本和 manifest hash 强绑定检查。（tools/check-scrcpy-binding.ps1，正反例实测拦截）

#### Web/QA 轨

- [x] WEB-001：基于冻结 fixture 建立 system/update API client 和 store/composable。（web/src/system/，26 fixture 全量对照测试）
- [x] WEB-002：完成系统和依赖状态卡片的 fixture 驱动实现。（SystemInfoCard.vue，degraded/docker 降级态）
- [x] WEB-003：完成更新状态卡片的 fixture 驱动实现。（UpdateStatusCard.vue，状态×动作受理矩阵）
- [x] WEB-004：完成安装、等待、重启、失败和回滚确认流程。（UpdateConfirmModal.vue + useUpdateFlow.js，202 断连语义）
- [x] 前端覆盖 idle/checking/available/downloading/staged/waiting/installing/restarting/failed/rolling_back 状态。（manual_recovery 同步覆盖，共 11 态）
- [x] 批次 1 合流门：launcher 能验 fixture，server 有路径/迁移骨架，依赖来源锁定，前端不依赖临时字段。（fmt/clippy/test 全绿：launcher 28、server 336、web 556 测试 + 双脚本 PASS）

### 17.4 批次 2：Windows 完整包 MVP

#### Launcher 与依赖

- [x] LCH-004：逐文件 inventory、快速检查和 doctor 深检完成。（--deep/--probe，缺文件/损坏/版本错精确定位）
- [x] LCH-004：缺少 adb 任一 DLL、ffmpeg 损坏和版本错误均可定位。
- [x] LCH-005：实现本地 seed、缓存和远端下载优先级。（三级来源全过 hash 门禁）
- [x] LCH-005：实现下载超时、临时文件、代理、大小和 hash 限制。（.part 原子入库，截断/超时不污染）
- [x] LCH-006：实现安全解压和同卷 staging 原子安装。
- [x] LCH-006：拒绝 zip-slip、解压炸弹、链接、ADS、保留名和大小写碰撞。（zip crate 重复条目折叠坑已用 central directory 独立清点兜底）
- [x] LCH-007：完成 inventory→seed/cache→remote→复验的 repair 流程。（真实 vendor 产物离线恢复实测，损坏目录入 quarantine）
- [x] LCH-008：按冻结路径和环境启动 server，并持有准确 child handle。（PATH 最小集注入，Child::wait 句柄等待）
- [ ] LCH-008：用 version、boot id、schema 和 readiness 验证目标进程。（当前仅 readiness 探测，boot id/schema 校验随批次 3 候选验证 LCH-012 落地）

#### Server/Data/API

- [x] PATH-002：静态 web-dist 从 app dir 提供，版本目录可只读。
- [x] DATA-002：将当前无版本数据库归入明确 baseline migration。（schema 快照 fixture 比对 + 拒绝后字节不变断言）
- [x] DATA-003：实现 min/max/target schema 兼容门禁。（常量化 + 编译期断言 + 可诊断错误）
- [x] DATA-005：实现不启动 adb/scheduler/HTTP 的 maintenance inspect/migrate CLI。（五态判定，missing 不建库，实跑验证）
- [x] OPS-002：停机幂等并暴露 draining/stopping/finished 状态。（/health/shutdown + 重入不重入测试）
- [x] OPS-003：升级器默认等待准确进程退出，不复制 `gamer.ps1` 的固定强杀时序。（launcher 句柄等待，退出码透传）
- [x] SYS-001：实现受保护的 `/api/system/info`。（与契约 fixture 字段集递归比对测试）
- [x] SYS-001：响应不泄露绝对路径、用户名、token、密码或命令行。（泄露禁令测试）
- [x] SYS-002：实现有界 adb、ffmpeg 和 scrcpy 版本/功能探针。（3s 超时 + 60s 缓存，懒执行不阻塞启动）

#### Packaging

- [x] DEP-005：生成与实际组件一致的 licenses、NOTICE 和 SBOM 输入。（licenses/ + CycloneDX 384 组件）
- [x] REL-001：从 clean checkout 生成 Windows app 组件包。（package-app.ps1，zip 条目强制 / 分隔）
- [x] REL-002：生成 launcher + signed manifest + offline seeds 的 full ZIP。（71MB，解压复核+SHA256SUMS 全对）
- [x] REL-003：生成 detached signature 和 `SHA256SUMS`。（dev-ed25519-1，validate-manifest check 验签通过）
- [x] QA-002：慢流、断流、404、篡改、错误 Range 和并发 repair 不污染 runtime。（45 测试；未采用 Range 断点续传，该反例不适用，其余全覆盖）
- [x] Windows PATH 清空、断网、无 Node/Rust/adb/ffmpeg 时 full 包首次启动通过。（E2E 场景 A：死代理断网模拟 + PATH=System32，repair→start→/health/ready 200，见 docs/evidence/UPDATE_M1_EVIDENCE.md）
- [x] 删除 adb.exe、任一 adb DLL 或 ffmpeg.exe 后离线 repair 通过。（E2E 场景 B：删 AdbWinApi.dll → 离线恢复 sha256 与锁一致）
- [x] 项目真实 H.264 stdin→PNG stdout ffmpeg 命令通过。（fetch-ffmpeg.ps1 冒烟，BUILD-CONFIG 归档）
- [x] 批次 2 合流门：完成可手工安装的 M1 基线版本和可复现 full ZIP。（首轮 E2E 暴露 3 阻断缺陷已修复复验，full ZIP 从 HEAD 可重建）

### 17.5 批次 3：自动升级与回滚

#### Launcher/Updater

- [x] LCH-009：实现仅当前用户可访问的 Windows named pipe IPC。
- [x] LCH-009：IPC 请求 id、协议版本、大小限制、超时和幂等测试通过。
- [x] LCH-010：实现检查、下载、验签、staged、waiting 和 switch journal 状态。
- [x] LCH-010：launcher 启动时能恢复每个未完成 journal 状态。
- [x] LCH-011：实现离线数据/config 快照 manifest 和逐文件 hash。
- [x] LCH-011：快照前确认 server PID 已完整退出并验证 SQLite 完整性。
- [x] LCH-011：实现 data 同卷 staging 恢复和失败数据 quarantine。
- [x] LCH-012：实现 candidate start、版本/schema/boot id 验证和 commit。
- [x] LCH-012：实现 pre-commit 自动恢复 snapshot + previous binary。
- [x] LCH-012：回滚失败进入 `manual_recovery_required`，停止自动循环。

#### Server/Data/API

- [x] DATA-006：完成 N-1→N、重复迁移、SQL 失败、文件碰撞和中断 fixtures。
- [x] OPS-004：实现 candidate activation gate。
- [x] OPS-004：commit 前不启动 scheduler、设备扫描或业务写 API。
- [x] OPS-005：暴露 active run、viewer、升级事务和下一 cron 摘要。
- [x] SYS-003：实现 launcher、unsupported 和 Docker UpdateController adapter。
- [x] SYS-004：实现 update query/check/download/install/rollback/policy API。（`api::tests::update` 6 passed）
- [x] SYS-004：更新状态变更 API 继续通过登录、Origin 和并发事务门禁。（`api::tests::update` 6 passed）
- [x] SYS-005：实现 off/notify/auto 和维护窗口策略。
- [x] SYS-005：auto 在活动运行或临近 cron 时只等待，不强制安装。
- [x] SYS-006：install 先返回 202，后台协调器再触发停机。（同 6 个测试覆盖 install 202/后台协调）

#### Release/Web

- [ ] REL-004：tag 触发 Release workflow，先创建 draft。
- [ ] REL-004：所有资产、验签和 smoke 通过后才发布 Release。
- [ ] REL-005：发布 GHCR semver tag 和 immutable digest。
- [ ] REL-006：生成 SBOM、provenance/attestation 并演练 key rotation。
- [x] WEB-005：Settings 从 fixture 切换到真实 system/update API。
- [x] WEB-005：删除静态“设置已保存（原型）”交互。
- [x] WEB-006：MainLayout、Login、Settings 全部移除硬编码产品版本。
- [x] WEB-006：检测 server/web-dist 版本混包并显示明确警告。
- [x] QA-004：升级 journal 每个持久化边界的 kill/restart 测试通过。
- [x] QA-006：run/viewer/cron/install 并发门禁测试通过。
- [x] 批次 3 合流门：M1 基线能够升级到 M2，并在候选失败时自动恢复旧程序和数据。（docs/evidence/UPDATE_M2_EVIDENCE.md：真实 Windows 进程级 E2E 75 PASS/0 FAIL——0.1.0→0.2.0 committed、候选启动失败自动回滚恢复 0.1.0 与升级前快照；复现脚本 `release/packaging/test-upgrade-launcher-e2e.ps1`。边界：单机 Windows 实测非 clean VM；install API 止于 prepare_install，完整接管走 launcher CLI（§E-6 #4））
- 本轮验证：Launcher IPC/升级/快照/候选专项测试 `22+82+20+6 passed`；真实 Windows 重启/PID/DACL/文件占用、Windows clean VM 和真实 Android 设备仍未验收。
- 本轮验证：Server `cargo test migrations::tests` 为 `11 passed`（含新增 SQLite migration failure boundary），`file_migration::tests` 为 `17 passed`，update `69 passed`；此前全库 `432 passed、0 failed、2 ignored`。
- 本轮验证：Release 本地 workflow 与 Web 已通过，Web 保持 `564 tests passed`/build 通过；生产签名私钥/key ID/公钥配置、GitHub release 环境审批、GHCR/Docker/buildx attestation 实跑仍阻塞 REL-004/005/006。
- Release 本地验收已通过：`release/packaging/test-release-workflow.ps1` 与 `release/packaging/test-upgrade-release.ps1` 在 `powershell.exe 5.1` 和 `pwsh` 下均 PASS，相关 AST/Node check 均通过；9 个 PowerShell fixture 已加 UTF-8 BOM，`augment-sbom` 的 `bom-ref` 兼容修复已完成。该结果仍不替代真实 GitHub/GHCR/生产签名环境验收，REL-004/005/006 继续保持 [ ]。
- 本轮验证：`cargo test api::tests::sec_tests::update -- --nocapture` 为 `6 passed`，覆盖 SYS-004 两项及 SYS-006 的 install 202/后台协调；真实 Docker/GHCR/Windows VM/Android 仍缺。
- 2026-09-01 收口：批次 3 合流门已按 UPDATE_M2_EVIDENCE 勾选。REL-004/005/006、QA-008 当前精确阻塞：GitHub 侧 release-sign 签名 secrets 与 release environment 评审人未配置、本机 gh 已安装（2.97.0）但无凭据、docker 未登录 ghcr；临时 tag `v0.2.0-rc-drill1` 演练已真实触发 Release workflow（run 33416440024，因当时远端 HEAD 缺少本地已修的校验脚本修复而失败，tag/draft 零残留），详见 docs/evidence/UPDATE_EXTERNAL_QA_EVIDENCE.md；check-immutable-release 的 refspec 引号缺陷已修复（`a7e5ef1`），test-release-workflow.ps1 双 PS 版本门禁 PASS。

### 17.6 批次 4：热点集成、Docker 与 Launcher 自更新

- [x] 指定唯一集成人修改 `server/src/main.rs` 和 `server/src/api/mod.rs`。（e84d51a 统一接入 system/update 路由与停机协调；main.rs 统一 drain 已由集成 Agent 静态确认）
- [x] 指定唯一集成人修改 `web/src/api.js` 和 `web/src/views/Settings.vue`。（3a4477c 接入真实 system/update API 并移除静态原型）
- [x] 指定唯一集成人修改 release workflow、compose 和 `gamer.ps1` 热点。（release workflow=4dd1469 draft-first 与签名/镜像门禁收紧、compose=ada0ad6 停机宽限 30s；gamer.ps1 经评估无需改动——开发启动链路不涉升级轨道）
- [x] LCH-013：实现 launcher 自更新 trampoline 和最低 launcher 版本门禁。（`upgrade:: --lib` 26 passed）
- [x] LCH-013：launcher 文件被占用或替换失败时继续保留旧 launcher。（`upgrade:: --lib` 26 passed）
- [x] DKR-001：增加基于 `${GAMER_IMAGE}` 的 release compose，保留开发 build compose。（release compose 无 `build`、支持 `GAMER_IMAGE`/digest，config 通过）
- [x] DKR-002：实现宿主 pull→backup→切 digest→ready→失败回旧 digest。（`release/packaging/test-upgrade-release.ps1` 离线 mock 覆盖健康升级与 ready 失败回滚；`pwsh` 与 `powershell.exe` 均 PASS）
- [x] DKR-003：Docker system info 返回 external update strategy，安装能力为 false。（external strategy、`install=false` 及路由/fixture tests 通过）
- [x] DKR-004：Docker 升级/降级过程中绑定数据目录保持不变。（现有静态契约明确覆盖 bind mount `data/` 保持；真实 Docker daemon 验证仍缺）
- [x] Docker stop/SIGTERM 确认走统一 run drain、viewer/session 和 adb 清理。（真实 Docker 实测：带 202 run + 登录态 `docker stop`，日志链 SIGTERM→coordinator draining→force-cancelling active runs→finished，退出码 0、宿主侧 SQLite integrity ok；容器内无真机会话，viewer/scrcpy/adb 清理证据止于 drain 日志层——UPDATE_DOCKER_E2E_EVIDENCE.md）
- [x] 开发模式、Windows portable 和 Docker 三种启动方式分别通过冒烟。（开发=cargo run + 浏览器/真机全功能冒烟（CLEAN_BASELINE_FUNC_EVIDENCE.md）；portable=full ZIP repair→start→升级→回滚多台架实测（M2/QA-005/真机证据）；Docker=release compose 起容器 ready/登录/浏览器状态页（UPDATE_DOCKER_E2E_EVIDENCE.md））
- [x] 批次 4 合流门：三种部署模式能力降级明确，热点文件完成唯一集成。（launcher managed capabilities 全 true、Docker external 且安装能力 false（API+UI 双确认）、直连 unsupported 由 SYS-003 适配器测试覆盖）
- 本轮验证：`cargo test upgrade:: --lib` 为 `26 passed`，覆盖 trampoline、最低版本、占用重试、失败保留旧文件；release compose 静态检查及显式 `GAMER_IMAGE` 下 `docker compose -f docker-compose.release.yml config --quiet` 均通过。DKR-003 的 external strategy、`install=false` 及路由/fixture tests 通过。上述不替代真实 Docker/GHCR/Windows VM/Android 验收，外部环境验收仍缺。
- 本轮验证：唯一集成 Agent 静态确认 `server/src/main.rs` 已修复统一 drain 安装顺序竞态；`/api/shutdown`、Ctrl+C、SIGTERM 共用 `ShutdownCoordinator`，drain 顺序为 `run→viewer→scrcpy/session/adb`。真实 Docker stop/SIGTERM 仍未验收，因此对应 checklist 保持 [ ]。
- 本轮验证：开发 compose、release compose、USB override、redroid profile 的 `docker compose config --quiet` 全部通过；该证据不替代开发模式、Windows portable 和 Docker 三种运行冒烟，也不代表真实 Docker/GHCR 验收完成。
- 2026-09-01 收口：DKR-004 真实 Docker 升级/回滚（不健康候选自动回旧 digest、绑定数据保持）、SIGTERM 统一 drain、Docker 浏览器状态页冒烟、三种启动方式冒烟全部完成（docs/evidence/UPDATE_DOCKER_E2E_EVIDENCE.md）；容器内 WebRTC/ICE 媒体面未做（USB 真机无法透传容器，未擅自改设备网络配置）。

### 17.7 批次 5：RC、故障注入与发布签核

#### Windows 与故障测试

- [x] QA-003：SQLite 每条 migration 前后失败均不越级 schema。（新增 SQLite migration failure boundary；`cargo test migrations::tests` `11 passed`）
- [x] QA-003：文件 migration 每个 copy/hash/rename/marker 边界失败均不丢源文件。（`cargo test file_migration::tests` `17 passed`）
- [ ] QA-005：Windows 10 x64 clean VM 完整测试通过。
- [ ] QA-005：Windows 11 x64 clean VM 完整测试通过。
- [x] QA-005：空格、中文和长路径安装测试通过。（2026-09-01 修复轮：launcher verbatim `\\?\` 路径 + 台架 tar.exe 解压后，LongPathsEnabled=0 下安装根 264/exe 296 的安装、ready、升级全通；中文+空格根在 M2/QA-005/真机台架全程使用；host-only 非 clean VM——UPDATE_WINDOWS_QA_EVIDENCE.md 缺陷修复轮）
- [x] QA-005：程序和 data 位于不同磁盘时测试通过。（data 根 junction 放行 + 树内 reparse 仍拒；C: 安装根 + D: 物理 data 升级 committed、快照逐 hash 复核 0 mismatch；回滚后 data 恢复为实体目录、依赖跨盘存储需重建 junction——已留档）
- [x] QA-007：1GB DB 和大量小文件升级压力测试通过。（真实 1 GiB blob DB 1,075,576,832 B + 4096 小文件经真实升级链路 committed，快照 4099 文件逐 hash 独立复核 0 mismatch ×3 轮、integrity ok；中断恢复与磁盘不足拒绝另行通过——UPDATE_WINDOWS_QA_EVIDENCE.md 场景 1、UPDATE_M2_EVIDENCE.md §E-8）
- [x] QA-007：磁盘空间不足在修改 current/data 前明确拒绝。
- [x] 杀毒软件短暂占用 exe/current/journal 时有界重试且不误删 previous。（FileShare.None 独占句柄模拟：journal/current/exe 三类占用分别明确失败、有界重试成功（rename_with_retry 10×25ms）、committed 前自动回滚不误删 previous，解锁后重试成功；真实杀毒引擎未测——UPDATE_WINDOWS_QA_EVIDENCE.md 场景 2）
- [ ] Windows 重启、用户注销、launcher 强杀后 journal 恢复通过。
- [x] candidate 立即退出、端口占用、ready 503/超时、错误 version/schema/boot id 全部自动回滚。（`launcher/src/upgrade/**` 场景测试 `32 passed`）
- [x] rollback 也失败时保留 snapshot/quarantine/journal 并进入 `manual_recovery_required`。

#### Docker、Release 与真机

- [x] 开发 compose、release compose、USB override、redroid profile 全部 config 通过。（四套均通过 `docker compose config --quiet`；不代表真实 Docker daemon、Windows、Android 或 GHCR 验收）
- [x] DKR-004：新镜像不健康后按旧 digest 自动恢复。（真实 Docker 台架两遍：不健康候选（healthcheck 判死）→ 自动回旧 digest ready → 升级前 API 锚点数据回滚后可查，绑定 bind 目录 marker 全程一致——UPDATE_DOCKER_E2E_EVIDENCE.md）
- [ ] QA-008：从 GitHub Release 重新下载 Windows 资产后验签和启动通过。
- [ ] QA-008：从 GHCR 重新拉取镜像后版本、commit、digest 和 OCI label 一致。
- [ ] 使用临时 tag/draft 完成完整发布演练，没有拿首个 stable tag 调试。
- [x] 至少一台真实 Android 设备完成升级前后 adb、投屏、控制、截图和模板匹配。（小米 25079RPDCC 真机：升级前 19 项/升级后 16 项全 PASS——scan/connect、活画面截图、控制生效、同一模板 NCC 命中（0.9259/0.9430/0.8710）、同一脚本 run success 全部穿越 0.1.0→0.2.0 升级等价；UPDATE_REALDEVICE_EVIDENCE.md）
- [x] 至少一次脚本运行和定时任务升级门禁/升级后恢复验证通过。（活动脚本运行中升级停 waiting_idle 96s、run 自然跑完未被杀；run 结束后升级自动继续 committed（drain 读超时缺陷修复 24039a1 后，修复前 90s 取消）；cron 触发点零丢失——drain 期拒绝、新版启动后按窗口内最近触发点补跑 success；升级后任务下一分钟真实触发 success——UPDATE_REALDEVICE_EVIDENCE.md §R-10）
- [ ] Release 资产包含 manifest、signature、SHA256SUMS、SBOM、NOTICE 和发布说明。
- [ ] Release environment 私钥权限和 key rotation runbook 复核通过。

#### 文档与全量门禁

- [x] DOC-001：README 包含完整包安装、依赖修复、升级和回滚入口。（README「Windows x64 完整包」「完整包依赖修复」「升级与回滚入口」）
- [x] DOC-002：维护者手册包含 draft 发布、密钥轮换和 manual recovery。（`docs/guides/RELEASE.md` §2/§3/§4）
- [x] DOC-003：本轮真实新增坑全部进入 `docs/PITFALLS.md`。（`docs/PITFALLS.md` 2026-08-31 批次 0/1、批次 2/发布链路条目）
- 本轮验证：QA-003 的 SQLite migration failure boundary 已加入并通过；`cargo test migrations::tests` 为 `11 passed`，`file_migration::tests` 为 `17 passed`。这只是专项测试证据，不代表 QA-005/007/008 或真实发布环境验收完成。
- 本轮验证：QA-007 子 Agent 已完成本地替代测试但明确未完成全量验收：本地已通过 1 GiB 稀疏 DB preflight、2048 小文件 snapshot/manifest/hash/SQLite inspect（launcher snapshot 9 passed、server maintenance 9 passed）；真实 1GB 复制仍未执行，故第一条保持 [ ]；第二条空间不足拒绝已由定向测试验证并勾选。
- 本轮验证：launcher Engine 可注入 available-space provider，生产默认 `winutil::free_disk_bytes`，定向测试 1 passed，确认 `current.json`、`data`、`config.toml`、`snapshot/manifest` 不变，journal 保持 idle/failed；真实 1GB 复制仍未执行。
- 本轮验证：最新 launcher Agent 已在 `launcher/src/upgrade/**` 收口 candidate 立即退出、端口占用、ready 503/超时、错误 version/schema/boot id 的自动回滚，以及 rollback 再失败时保留 snapshot/quarantine/journal 并进入 `manual_recovery_required`；场景测试 `32 passed`，QA-004 回归 `6 passed`，`launcher cargo fmt --check` 与 `git diff --check` 通过。以上仍是本地 launcher/故障场景证据；Windows 10/11 clean VM、杀毒软件真实占用、重启/注销/强杀真实恢复、QA-007 第一条真实 1GB 复制与大量小文件升级压力测试、任何真实发布/真机项仍缺；QA-007 第二条已通过定向测试并勾选。
- [x] Launcher fmt、clippy、unit/integration tests 全绿。
- [x] Server fmt、clippy `-D warnings`、cargo test 全绿。
- [x] Web `pnpm test:run` 和 `pnpm build` 全绿。
- [x] Compose config 和 `tools/verify-release.ps1` 全绿。（compose 5 变体 config 全过；verify-release.ps1 默认模式全 PASS——advisory DB 在线刷新成功、严格 cargo audit 两 lockfile 仅存量豁免 RUSTSEC-2025-0141，无需降级离线模式）
- 本轮验证：DOC-001/002/003 条目已按 README、`docs/guides/RELEASE.md` 和 `docs/PITFALLS.md` 现有内容核对；该文档证据不代表真实 GitHub/GHCR、生产密钥、Windows 真机或 Android 验收。
- 本轮验证：全量门禁已通过：Launcher fmt、clippy、unit/integration tests；Server fmt、clippy `-D warnings`、cargo test；Web `pnpm test:run`、`pnpm build`。`tools/verify-release.ps1` 返回 0，但 `cargo audit` 实际出现 crates.io 403/yanked 检查错误及 1 条 `unmaintained` warning，依赖审计未完全成功，因此“Compose config 和 tools/verify-release.ps1 全绿”继续保持 [ ]。REL-004/005/006、批次 5 合流门，以及真实 GitHub/GHCR/生产签名、Docker daemon、Windows clean VM、Android 验收项继续保持 [ ]。
- 2026-09-01 收口（本机可执行项全部完成）：QA-005 长路径/跨盘、QA-007 真实 1 GiB 压力、杀毒占用模拟、强杀恢复、DKR-004 真实回滚、真机升级前后功能与门禁、compose/verify-release 全绿均已实测勾选（证据：UPDATE_WINDOWS_QA_EVIDENCE.md、UPDATE_M2_EVIDENCE.md §E-8、UPDATE_DOCKER_E2E_EVIDENCE.md、UPDATE_REALDEVICE_EVIDENCE.md）。真机轮发现并修复升级阻断缺陷：launcher 对 `/api/shutdown` 的读超时 5s < server 完整 drain 11.6s，handler 被 drop 致升级必取消；修复后活动脚本运行中升级自动继续 committed（24039a1，含 mock 回归测试）。保持未勾项的边界：Windows 10/11 clean VM、真实 Windows 重启/注销、真实杀毒引擎需物理环境；GitHub Release/GHCR/生产签名/QA-008 需仓库 secrets 与凭据（精确阻塞清单见 UPDATE_EXTERNAL_QA_EVIDENCE.md）；§17.8 为发布后观察项。真机轮另记录两个观察项：CLI 接管候选不继承 GAMER_ADMIN_PASSWORD（生产以 config password_hash 认证，不受影响）、cron 冻结窗口在 CLI 手动升级路径以「drain 期拒绝 + 新版补跑」呈现（触发点零丢失），§6.5 的前置策略判断仍以 SYS-005 服务端 auto 策略为准。
- [ ] 批次 5 合流门：§16 Definition of Done 逐条有证据，发布负责人签核 stable。

### 17.8 发布后观察与收口

- [ ] 发布后从干净环境重新安装正式 full ZIP，而不是复用 RC 目录。
- [ ] 发布后检查一次真实更新检查、下载、等待空闲、安装和重连闭环。
- [ ] 观察升级失败率、回滚次数、依赖修复次数和 readiness 失败原因。
- [ ] 确认保留策略没有删除 current、previous 或唯一有效 rollback point。
- [ ] 汇总首个稳定周期的问题，判断 lite 包、delta update 和新平台是否进入下一阶段。
- [ ] 将本计划状态、完成批次、最终版本和证据索引更新到文档顶部。
