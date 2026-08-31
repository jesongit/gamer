# 自动升级便携安装契约（ARC-001）

> 状态：**冻结**（批次 0 契约；后续轨道只能以版本化契约变更，不得口头改字段）。
> 依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §2/§5.1/§6.3/§8.1；与该计划冲突时以计划评审结论为准。
> 范围：Windows x86_64 便携发行版的安装目录、组件边界、稳定路径、原子切换边界。Docker/直跑模式的差异仅在 §3 标注。

## 1. 便携安装目录契约

安装根 = full ZIP 解压根（目录树见计划 §5.1）。安装根可为任意合法 Windows 路径（含空格、中文、长路径），一切内部引用使用绝对路径注入或相对安装根解析，不依赖 cwd、盘符或注册表。

各目录逐项冻结如下。属主含义：**launcher** = 仅升级器写入、业务进程只读；**业务** = server 进程读写；**用户数据** = 升级必须保留的内容。

| 路径 | 用途 | 属主 | 可写 | 生命周期（升级时） |
|---|---|---|---|---|
| `gamer-launcher.exe` | 启动器/升级器本体：单实例锁、依赖安装修复、server 监管、版本切换、journal/备份/回滚、manifest 验签下载 | launcher | 运行期只读（exe 被自身占用） | 仅经 trampoline 两阶段自更新（LCH-013）；失败保留旧 launcher。位于版本目录之外，永不随应用版本重建 |
| `config/config.toml` | 用户配置（auth、adb/ffmpeg 来源模式等） | 用户数据 | 是 | 永久保留；升级前纳入快照；结构变更走显式迁移，不做隐式改写 |
| `data/` | 业务数据：SQLite `gamer.db` + 文件资源 `data/<pkg>/{yaml,func,tmpl}/`（schema v1 布局） | 用户数据 | 是 | 永久保留；升级前离线快照，schema 迁移有编号 journal（DATA-\*） |
| `logs/` | server 日志（`GB_LOG` 指向文件）与 launcher 日志 | 用户数据 | 是 | 保留；可按保留策略轮转清理 |
| `state/current.json` | 当前版本指针（current/previous 版本号）；具体字段由 LCH-002 fixture 冻结 | launcher | launcher 专用 | 原子写；升级切换的唯一入口 |
| `state/update-journal.json` | 升级状态机持久 journal：update id、from/to、child PID、current/previous、snapshot、schema before/after、最后完成步骤、错误摘要（计划 §6.6） | launcher | launcher 专用 | 原子写；每次动作先记意图后执行；崩溃后据此恢复 |
| `state/launcher.lock` | 单实例锁 | launcher | launcher 专用 | 运行期存在，持有进程退出即失效；不跨升级保留 |
| `manifests/<version>.json[.sig]` | 已验签缓存的 release manifest 与 detached 签名 | launcher | launcher 专用（下载写入） | 保留（成功升级后 current/previous 的 manifest 属于必须保留的证据，计划 §14）；可按数量/年龄清理 |
| `versions/<semver>/` | 应用版本目录：`gamer-server.exe`、`web-dist/`、`assets/scrcpy-server.jar`（manifest `entrypoint` 指向 exe） | launcher（写入）/ 业务（只读） | **安装成功后只读** | 新版本写新目录，永不原地覆盖；至少保留 current + previous，其余按数量/年龄/磁盘上限清理，不删 current、previous、唯一 rollback point |
| `runtime/adb/<version>/` | managed adb：`adb.exe` + `AdbWinApi.dll` + `AdbWinUsbApi.dll`，逐文件哈希（计划 §11.2） | launcher | 运行期只读（安装/修复期由 launcher 写） | 与版本目录同理：新版本新目录，不原地覆盖；损坏目录走 §5 quarantine-then-rename 修复 |
| `runtime/ffmpeg/<version>/` | managed ffmpeg：`ffmpeg.exe`（锁定 buildconf） | launcher | 同上 | 同上 |
| `seeds/` | full 包内置离线组件压缩包，供断网首启与离线修复（LCH-007） | launcher（只读使用） | 运行期只读 | 由 full 包预置；是否保留可配置，清理不影响已安装组件 |
| `cache/artifacts/` | 下载产物缓存（seed→cache→远端优先级的中转） | launcher | 是 | 可随时清理重建；容量受限 |
| `staging/` | 组件解压/校验临时区（app 包、依赖包），目标是 `versions/` 与 `runtime/` 的 rename 源 | launcher | 是（可变重试区） | 每次安装前清空重建；升级清理阶段删除 |
| `backups/<update-id>/` | 升级前数据+config 离线快照（manifest + 逐文件 hash，LCH-011） | launcher | 是（快照构建期可重试） | 按更新 id 留存；至少保留与 previous 对应的已验证快照；清理不得删唯一有效 rollback point |
| `quarantine/` | 回滚失败或损坏数据/组件的保留区 | launcher | 是（只增） | **不静默删除**；仅人工恢复或显式清理动作处置 |

可写区域裁决（对应计划 §11.5「版本目录只读」）：运行期业务可写仅 `config/ data/ logs/ state/ cache/ staging/ backups/`；`versions/`、`runtime/`、`manifests/`、`seeds/`、`quarantine/` 对 server 进程恒只读，仅 launcher 在安装/修复/升级阶段写入。

## 2. 版本目录不可变原则

1. `versions/<semver>/` 一经安装成功即只读：不改名、不增删文件、不就地打补丁；运行期由 server 与 launcher 双方只读使用（PATH-002 要求版本目录只读时仍可提供 web-dist）。
2. 新版本永远写入新的 `versions/<semver>/`；版本切换唯一手段是改写 `state/current.json` 指针，不移动、不复用旧目录。
3. 同一 `semver` 目录损坏时不重建同名目录：整体移入 `quarantine/` 后重新安装（§5），避免「半新半旧」目录。
4. `runtime/<依赖>/<version>/` 遵循同一原则（哈希锚定的不可变组件目录）；`scrcpy-server.jar` 不在 runtime，而与应用绑定放 `versions/<semver>/assets/`，随应用版本整体更换（计划 §2）。
5. 所有 staging→最终目标的 rename 必须同卷（跨卷 move 是逐文件复制，不是原子 rename）。顶层 `staging/` 与安装根同卷，只服务 `versions/`、`runtime/` 目标；**待定**：data 位于另一卷（计划 §11.5 测试场景）时数据快照/恢复 staging 的落点需与 data 同卷，不能复用顶层 `staging/`，具体路径由 LCH-011 冻结。

## 3. 组件边界

### 3.1 launcher 职责

- 单实例锁（`state/launcher.lock`）与并发写者唯一性；
- manifest 验签（Ed25519 detached、覆盖原始字节）、组件下载（seed/cache/远端）、安全解压、依赖安装与修复（inventory→seed/cache→remote→probe）；
- server 子进程精确监管：注入环境、持有准确 child handle/PID、按 handle 等待退出（不按端口或进程名猜）；
- 版本切换、升级 journal、快照/备份、候选启动与 activation gate 编排、commit 与自动回滚；
- Windows named pipe IPC server（protocol v1，仅当前用户 DACL），只接受内部枚举操作，不接受 shell 命令字符串。

### 3.2 server 职责

- 业务空闲判断：active run/viewer/升级事务/下一 cron 触发摘要（OPS-005），作为安装门禁输入；
- system/update API 与策略（off/notify/auto、维护窗口），install 先回 202 再由后台协调器走停机；
- 经 IPC 向 launcher 发送**经过鉴权的枚举动作请求**（`status/check/download/prepare_install/rollback/repair_dependency`，计划 §6.4）；
- schema 版本、迁移、maintenance CLI；统一优雅停机协调器。

### 3.3 边界禁令

- server **永不**替换、移动、删除任何程序文件（`versions/`、`runtime/`、`gamer-launcher.exe`）；程序文件写操作只属于 launcher。
- server **永不**接受浏览器传入的下载 URL、镜像地址、验签开关；信任只来自签名、公钥与内容 hash。
- 用户数据（`config/ data/ logs/` 及 `state/` 中业务侧内容）**不得**位于 `versions/<semver>/` 内；删除任一版本目录不得影响用户数据。
- 直跑 server / Docker 模式无 launcher：server 以 `UnsupportedUpdateController` / external strategy 降级（Docker 升级=宿主机换镜像 digest），安装类 API 返回 `update_not_managed`，不得因此启动失败。

## 4. 稳定路径契约（计划 §6.3）

launcher 启动 server 时注入以下**绝对路径**环境变量，server 不再自行拼接这些路径：

| 变量 | 值（便携 managed 模式） |
|---|---|
| `GAMER_APP_DIR` | 当前应用版本目录 `versions/<current>/`（含 `web-dist/`、`assets/`） |
| `GAMER_DATA_DIR` | `<安装根>/data/` |
| `GAMER_ADB_PATH` | `<安装根>/runtime/adb/<version>/adb.exe`（managed） |
| `GAMER_FFMPEG_PATH` | `<安装根>/runtime/ffmpeg/<version>/ffmpeg.exe`（managed） |
| `GAMER_SCRCPY_SERVER` | `<GAMER_APP_DIR>/assets/scrcpy-server.jar`（随应用版本绑定） |
| `GB_CONFIG` | `<安装根>/config/config.toml` |
| `GB_LOG` | `<安装根>/logs/` 下的日志文件绝对路径 |

解析规则：

- server 工作目录固定为当前 `versions/<version>`，但业务逻辑不得依赖「碰巧从 server 目录启动」（PATH-001：任意 cwd、空格/中文路径读写正确）。
- 配置内相对 `data_dir` 相对**配置文件所在目录**解析；应用资产（web-dist、jar 等）相对 `GAMER_APP_DIR` 解析。
- 依赖来源三模式（launcher 配置选择，互不覆盖，计划 §11.2）：
  - `managed`：launcher 安装并修复到 `runtime/<id>/<version>/`，注入其绝对路径；
  - `system`：使用系统 PATH 中的工具；launcher 仅探测与报告版本，不写系统目录、不做修复安装；
  - `custom`：用户显式保存的可执行绝对路径，launcher 原样注入；修复器**永不**覆盖、改写、重装该路径内容。

## 5. 原子切换边界

### 5.1 必须原子的操作

| 操作 | 机制 |
|---|---|
| `state/current.json` 写入 | 同目录临时文件 + 落盘 flush + rename 原子替换；半截 JSON 必须可恢复（LCH-002） |
| `state/update-journal.json` 意图记录 | 同上；顺序固定为「先原子记录意图 → 执行动作 → 推进状态」（计划 §6.6） |
| staging → `versions/<semver>/` 切换 | 同卷目录 rename；前置条件：验签/逐文件校验通过且目标不存在。Windows 上 rename 到已存在目录会失败，因此不存在「覆盖式 rename」 |
| 损坏组件目录修复 | 两步夹 journal：旧目录 rename 入 `quarantine/` → staging 新目录 rename 到位；第二步失败则 rename 回，保持旧目录可用（「失败不破坏上一份 runtime」，计划 §11.2） |
| 数据快照恢复 swap | 恢复 staging 与 data 同卷（见 §2 第 5 条待定项）；快照 hash/marker 验证通过后才 rename 替换，替换前旧数据保留 |
| 下载产物落盘（manifest、artifacts） | 临时文件 + rename；截断/超时/hash 错不污染安装目录（LCH-005） |
| launcher 自更新 | trampoline 两阶段替换（临时 helper + 交换）；任何失败保留旧 launcher（LCH-013） |

### 5.2 允许可变重试的区域

`staging/`（安装失败即清空重建）、`cache/artifacts/`（随时可删）、`backups/<update-id>/` 快照构建中间态（hash/marker 未过不参与切换）、`logs/`。Windows 文件占用（杀毒扫描等）对这些路径和 current/journal/exe 的替换采用**有界重试**，重试耗尽按对应失败分支处理，不得误删 previous（计划 §11.5）。

### 5.3 原子性兜底

任一原子步骤崩溃后，结果只允许三种：新版健康、旧版健康、`manual_recovery_required`（保留 journal/快照/新旧版本/quarantine，停止自动重试循环）——不允许双进程、双 current 或无限回滚（计划 QA-004）。

## 6. 契约产物文件地图

后续任务冻结契约的固定位置（本文件只登记位置，不预写内容）：

| 文件 | 说明 | 任务 |
|---|---|---|
| `release/contracts/manifest-v1.schema.json` | Release manifest v1 JSON Schema（字段、路径安全、hash 规则） | ARC-002 |
| `release/contracts/fixtures/` | manifest 有效/无效 fixture、签名 fixture、危险路径反例，可自动校验 | ARC-002 / QA-001 |
| `release/contracts/system-api-v1.md` | system/update API 请求响应、状态、统一错误码契约 | ARC-003 |
| `release/contracts/ipc-v1.md` | launcher named pipe IPC protocol v1（消息上限、超时、幂等） | ARC-003 |
| `release/contracts/schema-policy.md` | DB/文件 schema 兼容表、rollback floor、pre/post-commit 回滚承诺 | ARC-004 |
| `release/contracts/dependency-licensing.md` | adb/ffmpeg 来源、许可、NOTICE/source offer 策略结论 | ARC-005 |
| `release/dependencies.lock.toml` | 依赖锁：版本、来源 URL、源 hash、文件清单、构建参数、许可证 | DEP-001 |

## 7. 待定项

- 数据快照/恢复 staging 在「data 与程序不同盘」场景下的具体落点（须与 data 同卷，路径待 LCH-011 冻结，见 §2 第 5 条）。
- `state/current.json` 字段集（用途已冻结为 current/previous 版本指针，字段由 LCH-002 fixture 冻结）。
