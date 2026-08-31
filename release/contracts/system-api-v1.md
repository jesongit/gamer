# System API v1 契约（ARC-003）

> 状态：**冻结**（批次 0 契约；字段、枚举、状态码、错误码只能以版本化契约变更——任何字段/枚举/状态码/语义变更必须 bump 到 `system-api-v2` 并单独提交 fixture，不得口头改字段）。
> 依据：`docs/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §6.4（Launcher IPC 与 System API）、§6.5（更新策略与门禁）、§6.6（持久化升级状态机）、§6.8（candidate activation gate）、§11.6（API 与前端验收）。
> 产出位置登记于 `docs/UPDATE_CONTRACT.md` §6 文件地图（`release/contracts/system-api-v1.md`）。
> 配套 fixture：`release/contracts/fixtures/system-api/*.json`（server / launcher / web 三方开发的唯一依据，字段名以 fixture 为准）。

## 0. 适用端点

| 端点 | 方法 | 鉴权 | 幂等性 |
|---|---|---|---|
| `/api/system/info` | GET | 需登录（未登录 401） | 只读 |
| `/api/system/update` | GET | 需登录（未登录 401） | 只读 |
| `/api/system/update/check` | POST | 需登录 + 同源校验 | 幂等（202 + 当前状态） |
| `/api/system/update/download` | POST | 需登录 + 同源校验 | 幂等（202 + 当前状态） |
| `/api/system/update/install` | POST | 需登录 + 同源校验 | 非幂等（并发第二个 409 `update_busy`） |
| `/api/system/update/rollback` | POST | 需登录 + 同源校验 | 非幂等（并发第二个 409 `update_busy`） |
| `/api/system/update/policy` | PUT | 需登录 + 同源校验 | 幂等（整对象替换） |
| `/health/ready` | GET | 匿名 | 只读（向后兼容，见 §8） |

## 1. 通用约定

### 1.1 鉴权与同源

- 与仓库现有 `server/src/auth.rs` 中间件完全一致，本契约不新增鉴权机制：
  - 未登录访问任何受保护端点 → `401`，body `{"error":"unauthorized"}`；
  - 状态变更方法（POST/PUT/DELETE/PATCH）携带的 `Origin` 与 `Host` 不一致（跨站）→ `403`，body `{"error":"forbidden_origin"}`；`Origin` 缺失放行（CLI/curl 场景）；GET 不做 Origin 校验。
- 更新状态变更 API（check/download/install/rollback/policy）必须同时通过登录与同源两道门禁（计划 §11.6）。

### 1.2 错误响应形态

除 401/403 走中间件固定 body 外，本组 API 的**业务错误**统一为：

```json
{ "code": "<错误码>", "message": "<一句话中文/英文描述>", "details": { "<按错误码定义>" } }
```

- `code`：§7 定义的 11 个统一错误码之一（校验类错误用 §6.5 的 `invalid_argument`）。
- `message`：人类可读；**不得**包含盘符路径、用户名、token、密码或完整命令行。
- `details`：可选对象，仅允许 §7 表中列出的键。

### 1.3 泄露禁令（计划 §11.6）

`/api/system/info`、`/api/system/update` 及其所有子端点的**任何响应**（含 `last_error.message`、`details`）禁止出现：

- 盘符路径与绝对路径（如 `C:\Users\...`、`/home/...`）；
- 用户名 / 用户目录；
- token、密码、会话凭据；
- 完整命令行。

依赖与版本信息一律以「状态 + 版本号 + 来源枚举」表达，不给路径。

### 1.4 内容类型

所有请求/响应 body 均为 `application/json`（`/health/ready` 亦然）。

## 2. `GET /api/system/info`

成功 `200`，响应字段冻结如下（fixture：`system-info.success.json`、`system-info.degraded-docker.json`）：

```json
{
  "app": {
    "version": "0.2.0",
    "commit": "0123456789abcdef0123456789abcdef01234567",
    "built_at": "2026-08-31T00:00:00Z",
    "channel": "stable",
    "target": "x86_64-pc-windows-msvc"
  },
  "deployment": { "mode": "launcher", "update_strategy": "managed" },
  "schema": { "db": 1, "file": 1, "rollback_floor": 1 },
  "dependencies": {
    "adb":    { "status": "ready", "version": "34.0.5", "source": "managed", "binding": "runtime" },
    "ffmpeg": { "status": "ready", "version": "6.1.1",  "source": "managed", "binding": "runtime" },
    "scrcpy": { "status": "ready", "version": "3.3.3",  "source": "managed", "binding": "application" }
  },
  "capabilities": { "check": true, "download": true, "install": true, "rollback": true },
  "startup": { "stage": "ready", "boot_id": "3f2c9a58-6d1e-4b7f-9a30-5c8b2e7d1f04" }
}
```

### 2.1 字段定义

| 字段 | 类型 | 冻结说明 |
|---|---|---|
| `app.version` | string | 产品版本，权威源 = `server/Cargo.toml` `package.version`；dev 构建如实显示（如 `0.2.0-dev`），不允许伪装正式版 |
| `app.commit` | string | 7~64 位小写十六进制 git commit；无构建注入时 `unknown` |
| `app.built_at` | string | RFC3339 UTC 时间戳；无注入时 `unknown` |
| `app.channel` | string | 枚举 `stable` \| `beta` \| `dev` \| `unknown` |
| `app.target` | string | Rust target triple（如 `x86_64-pc-windows-msvc`）；`x86_64`+OS 兜底形式（如 `x86_64-windows`）允许 |
| `deployment.mode` | string | 枚举 `launcher`（便携托管）\| `direct`（直跑）\| `docker`（容器） |
| `deployment.update_strategy` | string | 枚举 `managed` \| `external` \| `unsupported`；与 mode 的映射冻结为 `launcher→managed`、`docker→external`、`direct→unsupported` |
| `schema.db` | number | 当前 SQLite schema 版本（当前基线 = 1） |
| `schema.file` | number | 当前文件布局 schema 版本（当前基线 = `data/<pkg>/{yaml,func,tmpl}` = 1） |
| `schema.rollback_floor` | number | 可自动回滚的最低兼容 schema（对应 manifest `rollback_floor`，语义由 ARC-004 的 `schema-policy.md` 冻结） |
| `dependencies.<id>.status` | string | 枚举 `ready` \| `missing` \| `broken`（存在但校验/探针失败）；`unknown` 仅允许在探针尚未完成时出现 |
| `dependencies.<id>.version` | string \| null | 探测到的真实版本；不可得时 `null` |
| `dependencies.<id>.source` | string | 枚举 `managed`（launcher/部署物锁定提供）\| `system`（系统 PATH）\| `custom`（用户显式保存路径）；Docker 模式恒为 `managed`（随镜像提供并锁定） |
| `dependencies.<id>.binding` | string | 枚举 `runtime`（launcher 管理的 `runtime/<id>/<version>/` 独立组件目录）\| `application`（随应用版本目录 `versions/<semver>/assets` 分发）\| `external`（不由部署内组件目录绑定：system/custom 来源、Docker 镜像内置的 adb/ffmpeg）；`scrcpy` 恒为 `application`（与应用版本强绑定，禁止独立升级） |
| `capabilities.*` | boolean | `check` / `download` / `install` / `rollback` 四布尔；仅由 `deployment` 决定：`launcher` 模式且 IPC 通道建立 → 全 true，`docker`/`direct` → 全 false。**策略 `off` 只关闭自动行为，不改变 capability** |
| `startup.stage` | string | 枚举 `starting` \| `maintenance_gate` \| `ready`；对齐计划 §6.8 activation gate——候选进程在闸内（仅探针/健康/激活可达）时报 `maintenance_gate`，业务路由打开后报 `ready` |
| `startup.boot_id` | string | 进程每次启动生成的 UUID v4；重启必变（前端据此判定「服务确实重启过」） |

### 2.2 与现有原型实现的差异（迁移提示，非契约内容）

当前 `server/src/api/system.rs` 原型与本契约存在字段名差异，实现侧在 SYS-001/SYS-002/WEB-005 落地时**以本契约为准**迁移：`app.git_commit → app.commit`、`schema.database/files → schema.db/file`、依赖 `source: "bundled" → "managed"`（并新增 `binding`）。`readiness`/`timezone` 为原型自有字段，不属于本契约（readiness 以 `/health/ready` 为准）；是否随 WEB-005 保留由实现侧决定，但**不得**替换本契约字段。

## 3. `GET /api/system/update`

成功 `200`（fixture：`system-update.success.json` 等）。响应字段冻结：

```json
{
  "state": "staged",
  "detail": "staged",
  "update_id": "upd-20260831-9f3ab2c1",
  "candidate": {
    "version": "0.3.0",
    "channel": "stable",
    "published_at": "2026-09-15T00:00:00Z",
    "size_bytes": 893451200,
    "release_notes_url": "https://example.invalid/releases/v0.3.0"
  },
  "progress": { "bytes_done": 0, "bytes_total": 893451200 },
  "policy": {
    "strategy": "notify",
    "maintenance_window": { "start": "02:00", "end": "06:00" },
    "freeze_window_minutes": 30
  },
  "last_error": null,
  "updated_at": "2026-08-31T12:00:00Z"
}
```

| 字段 | 类型 | 冻结说明 |
|---|---|---|
| `state` | string | §5 的 11 态展示枚举；**前端业务分支只允许依赖此字段** |
| `detail` | string | 计划 §6.6 journal 精细步骤名（诊断展示用，见 §5.2 映射表；前端不得据此分支业务逻辑） |
| `update_id` | string \| null | 当前/最近一次升级事务 id（journal 的 update id）；无事务时 `null`。格式建议 `upd-<yyyymmdd>-<8hex>`（建议值） |
| `candidate` | object \| null | 已知的新版本候选（来自已验签 manifest `release` 块）；无候选时 `null`。`size_bytes` = 应用组件包大小 |
| `progress` | object \| null | 仅 `downloading` 态非空：`bytes_done` / `bytes_total`；其他态恒为 `null` |
| `policy` | object | §6 定义的当前生效策略（`docker`/`direct` 模式同样返回） |
| `last_error` | object \| null | 最近一次失败的 `{ "code": <§7 错误码>, "message": <无泄露描述> }`；无失败时 `null` |
| `updated_at` | string | 状态最后变更时间（RFC3339 UTC） |

## 4. 动作端点：`POST check / download / install / rollback`

### 4.1 通用语义

- **202 = 已受理，进入后台协调器**；body 冻结为 `{ "update_id": "...", "state": "<11 态之一>" }`。浏览器**不能也不需要**等一个长 HTTP 请求（计划 §6.4），后续以轮询 `GET /api/system/update` 获取进展。
- `install` 尤其如此：受理后协调器走「空闲门禁 → 停机 drain → 快照 → 迁移 → 切换 → 候选启动（activation gate）→ 提交/回滚」，期间 **HTTP 服务会重启、连接会断开**。前端断连**不得**显示「安装失败」；重连后以 `GET /api/system/info` 的 `app.version` / `startup.boot_id` 变化 + `GET /api/system/update` 判定结果（计划 §11.6，WEB-004）。
- 每个动作端点可能同步失败（受理前即拒绝），状态码与错误码见 §7；`docker`/`direct` 模式下四个动作端点一律 `409 update_not_managed`。

### 4.2 状态 × 动作受理矩阵（冻结）

行 = `GET /api/system/update` 的当前 `state`，列 = 动作；`202` 表示受理（返回 body 中的 `state` 见括号），其余为同步拒绝。

| 当前 state | check | download | install | rollback |
|---|---|---|---|---|
| `idle` | 202 (checking) | 409 `update_not_available` | 409 `update_not_available` | 409 `rollback_unavailable` |
| `checking` | 202 (checking，同 update_id) | 409 `update_busy` | 409 `update_busy` | 409 `update_busy` |
| `available` | 202 (checking，同 update_id) | 202 (downloading) | 409 `update_not_ready`（staging 未就绪） | 409 `rollback_unavailable` |
| `downloading` | 202 (checking，重启检查) | 202 (downloading，同 update_id) | 409 `update_busy` | 409 `update_busy` |
| `staged` | 202 (checking) | 202 (staged，no-op) | 门禁判定（见 4.3） | 202 (rolling_back) |
| `waiting` | 202 (checking) | 202 (staged) | 门禁判定；手动 install 优先于维护窗口等待（见 4.3） | 202 (rolling_back) |
| `installing` | 409 `update_busy` | 409 `update_busy` | 409 `update_busy` | 409 `update_busy` |
| `restarting` | 409 `update_busy` | 409 `update_busy` | 409 `update_busy` | 409 `update_busy` |
| `failed` | 202 (checking) | 202 (downloading，重试下载) | 门禁判定 | 202 (rolling_back) |
| `rolling_back` | 409 `update_busy` | 409 `update_busy` | 409 `update_busy` | 409 `update_busy` |
| `manual_recovery` | 409 `manual_recovery_required` | 409 `manual_recovery_required` | 409 `manual_recovery_required` | 409 `manual_recovery_required` |

补充规则：

- `failed` 态下的 `install`/`rollback` 重试需先满足各自门禁（staging 完好 / 存在有效回滚点），否则按 4.3/`rollback_unavailable` 拒绝。
- 回滚承诺边界（计划 §6.7）：`rollback` 仅承诺 **committed 之前** 的自动回滚；已 committed 的版本不提供 API 回滚（人工降级走维护手册），故 `idle`（上次事务已提交/无事务）时 rollback 恒为 `409 rollback_unavailable`。

### 4.3 install 门禁（409 `update_not_ready`）

受理 `install`（`staged`/`waiting`/`failed` 态）前逐项检查计划 §6.5 门禁，任一不满足 → `409 update_not_ready`，`details.blocking` 列出全部未满足项，枚举冻结为：

| blocking 值 | 含义 |
|---|---|
| `staging_not_ready` | 新组件未完整下载/验签/校验并位于 staging |
| `active_run` | 存在 active/starting/stopping 的脚本运行 |
| `update_transaction` | 存在另一个升级/回滚/备份/迁移/维护事务 |
| `cron_freeze_window` | 距下一次启用 cron 的触发时间 ≤ 冻结窗口 |
| `launcher_unreachable` | launcher/server IPC 不健康 |
| `insufficient_space` | 空间不足以容纳 staging、当前数据快照、新旧两版本及安全余量 |

全部满足 → `202`，进入后台协调器。viewer 在线**不是**硬门禁：viewer 默认等待并提示（计划 §6.5），由协调器经现有优雅停机链路处理。

## 5. 更新状态机（前端展示枚举，冻结）

### 5.1 11 态定义与允许迁移

| state | 含义 | 允许迁出 |
|---|---|---|
| `idle` | 无进行中的更新事务 | → checking |
| `checking` | 正在检查远端是否有新版本（自动/手动） | → available（有候选）、idle（无候选）、failed |
| `available` | 检查完成、存在已验签候选、尚未下载 | → downloading、checking（重新检查） |
| `downloading` | 后台下载 + 验签 + staging 中 | → staged、failed |
| `staged` | 候选已就位于 staging，等待安装 | → waiting（auto 策略）、installing（手动/门禁满足）、downloading（重新下载）、failed |
| `waiting` | auto 策略：等待维护窗口 + 空闲门禁 | → installing（窗口与门禁满足）、staged（窗口错过/门禁退出） |
| `installing` | 停机 drain → 快照 → 迁移 → 切换 已开始；**服务即将重启** | → restarting、rolling_back、failed |
| `restarting` | 新版本候选进程已拉起、处于 activation gate 或激活中；期间 API 可能短暂不可达 | → idle（committed，重连后确认）、rolling_back（readiness/版本/schema 校验失败） |
| `failed` | 事务失败于 committed 之前，旧版本仍在服务；可由用户重试或回滚 | → checking、downloading、installing、rolling_back、idle（清理后） |
| `rolling_back` | 正在恢复旧程序 + 升级前数据快照 | → idle（恢复成功）、manual_recovery |
| `manual_recovery` | 回滚也失败；保留 journal/快照/新旧版本/quarantine，**停止一切自动重试**（计划 §6.6） | → idle（仅人工恢复完成后；无 API 自动迁出） |

补充冻结：

- `installing`/`restarting` 期间 HTTP 服务可能不可达（socket 拒绝/超时）。前端对这两个态的断连**必须**按「等待重启」处理：有界重连 + 以 `app.version`/`boot_id` 判定结果，不得误报「安装失败」。
- 服务重启后内存态清零：`GET /api/system/update` 由 server 聚合 **launcher journal（经 IPC `status`）** 重建 `state`/`detail`/`update_id`，因此 11 态跨重启稳定。
- `manual_recovery` 是唯一无自动迁出的终态；迁出只能由维护动作（人工恢复流程）完成后复位。

### 5.2 `state` ↔ journal `detail` 映射（冻结）

`detail` 取值为计划 §6.6 的 journal 步骤名，另新增一个驻留值 `checked`（检查完成、有候选、未开始下载——§6.6 链上 `checking` 与 `downloading` 之间的驻留点，LCH-010 实现须对齐）：

| state | 允许的 `detail` 值 |
|---|---|
| `idle` | `idle`、`committed`、`cleaning` |
| `checking` | `checking` |
| `available` | `checked` |
| `downloading` | `downloading`、`verifying` |
| `staged` | `staged` |
| `waiting` | `waiting_idle` |
| `installing` | `draining`、`stopped`、`snapshotting`、`snapshot_verified`、`migrating`、`switched` |
| `restarting` | `candidate_starting`、`candidate_ready`、`activating` |
| `failed` | `failed` |
| `rolling_back` | `rolling_back` |
| `manual_recovery` | `manual_recovery_required` |

## 6. `PUT /api/system/update/policy`

请求 body（整对象替换，幂等）与 `200` 响应 body 同构，均为 `GET /api/system/update` 中的 `policy` 对象：

```json
{
  "strategy": "notify",
  "maintenance_window": { "start": "02:00", "end": "06:00" },
  "freeze_window_minutes": 30
}
```

| 字段 | 冻结说明 |
|---|---|
| `strategy` | 枚举 `off`（不检查）\| `notify`（自动检查、可选后台下载，用户确认安装；**产品默认，建议值**）\| `auto`（后台下载，并在维护窗口 + 空闲门禁满足后安装） |
| `maintenance_window.start` / `.end` | `"HH:MM"` 本地时间（24 小时制）；允许跨午夜（如 `23:00`→`05:00`）；`start == end` 视为非法 |
| `freeze_window_minutes` | 整数，cron 冻结窗口分钟数（安装门禁要求距下一次启用 cron 触发 **大于** 该值）；范围 0~1440，**建议默认 30** |

- 校验失败 → `400 { "code": "invalid_argument", "message": "...", "details": { "field": "<字段名>" } }`。
- `docker`/`direct` 模式允许保存策略（capability 全 false 时策略不产生任何自动行为），不返回 `update_not_managed`。
- 产品默认值（`notify` / `02:00-06:00` / 30 分钟）为**建议值**，可由配置文件覆盖；字段结构或枚举变更需 bump contract 版本。

## 7. 统一错误码（11 个，冻结）

| 错误码 | 触发条件 | HTTP 状态码 | 可重试 | `details` 冻结键 |
|---|---|---|---|---|
| `update_not_managed` | `docker`/`direct` 模式（`update_strategy != managed`）调用 check/download/install/rollback | 409 | 否（部署模式不变则恒定；UI 应隐藏/禁用对应按钮） | 无 |
| `update_busy` | 已有升级/回滚事务进行中，或对非幂等动作（install/rollback）的并发第二个请求（计划 §11.4：两个 install 只有一个取得事务） | 409 | 是（轮询 `GET /api/system/update` 至事务结束后重试） | 无 |
| `update_not_available` | 无已验签候选时请求 download/install（未检查、检查无新版本、候选已清理） | 409 | 条件性（出现新候选后可重试；同参数立即重试无意义） | 无 |
| `update_not_ready` | install 门禁未满足（§4.3 六项之一） | 409 | 是（`details.blocking` 中的门禁满足后重试） | `blocking`: 数组，枚举见 §4.3 |
| `signature_invalid` | manifest Ed25519 detached 验签失败（错 key、篡改、未签名；fail closed） | 422 | 否（同候选重试必然同败；重新 check 发现新 release 后可恢复） | 无。主要出现形态：异步——`state=failed` + `last_error.code` |
| `artifact_invalid` | 下载产物 hash/大小/格式校验失败（截断、篡改、zip-slip 等） | 422 | 条件性（重新 download 可修复传输损坏；产物源本身损坏则需等新版本） | 无。主要出现形态：异步——`state=failed` + `last_error.code` |
| `insufficient_space` | 磁盘空间不足以容纳 staging、数据快照、新旧两版本及安全余量（受理前预检或后台检查失败） | 507 | 是（清理空间后重试） | `required_bytes` / `available_bytes`（整数；不给路径） |
| `schema_incompatible` | 候选目标 schema 超出当前 binary 的 `min_read/max_read` 兼容范围，或低于 `rollback_floor` 约束 | 422 | 否（需等待兼容的新版本） | `candidate_schema` / `supported_range`（整数/二元组） |
| `launcher_unreachable` | launcher named pipe 连接失败/超时/令牌不匹配（launcher 未运行、被杀、IPC 损坏） | 502 | 是（launcher 恢复后有界退避重试） | 无 |
| `rollback_unavailable` | 无有效回滚点（无 previous 版本目录或无已验证快照），或目标事务已 committed（超出自动回滚承诺，计划 §6.7） | 409 | 否（人工介入/维护手册流程） | 无 |
| `manual_recovery_required` | 升级与自动回滚均失败，状态机进入 `manual_recovery`；此后任何更新动作被拒 | 409 | 否（必须人工恢复；保留 journal/快照/新旧版本/quarantine 证据，停止自动循环） | 无 |

非业务校验错误（如 policy 字段非法）使用 `400 invalid_argument`（不计入 11 码，形态见 §6）。

## 8. `/health/ready` 向后兼容（冻结）

- 保持**匿名**可访问、轻量、向后兼容（计划 §6.4）。
- 响应冻结为现有形态：就绪 `200`，未就绪 `503`，body：

```json
{
  "ready": true,
  "checks": {
    "data_dir": { "ok": true },
    "sqlite": { "ok": true },
    "scrcpy_server": { "ok": true },
    "adb": { "ok": true },
    "ffmpeg": { "ok": true }
  }
}
```

- **禁止**向 readiness 塞入远程版本检查、发布说明、盘符路径或任何本机路径（计划 §6.4；launcher 探活使用它，字段膨胀会拖慢闸内探针）。
- 版本、更新状态、依赖详情一律走 `/api/system/info`，不走 readiness。

## 9. fixture 索引（`release/contracts/fixtures/system-api/`）

| fixture | 场景 |
|---|---|
| `system-info.success.json` | launcher 模式 200 全量 |
| `system-info.degraded-docker.json` | docker 模式 200（capability 全 false、external strategy） |
| `system-info.unauthorized.json` | 未登录 401 |
| `system-update.success.json` | GET 200，`state=staged` |
| `system-update.failed-signature-invalid.json` | GET 200，`failed` + `signature_invalid` |
| `system-update.failed-artifact-invalid.json` | GET 200，`failed` + `artifact_invalid` |
| `system-update.manual-recovery.json` | GET 200，`manual_recovery` |
| `system-update.unauthorized.json` | 未登录 401 |
| `update-check.success.json` | 202 checking |
| `update-check.launcher-unreachable.json` | 502 `launcher_unreachable` |
| `update-download.success.json` | 202 downloading |
| `update-download.update-not-available.json` | 409 `update_not_available` |
| `update-download.insufficient-space.json` | 507 `insufficient_space` |
| `update-install.success.json` | 202 installing |
| `update-install.update-busy.json` | 409 `update_busy` |
| `update-install.update-not-ready.json` | 409 `update_not_ready` |
| `update-install.schema-incompatible.json` | 422 `schema_incompatible` |
| `update-install.update-not-managed.json` | 409 `update_not_managed` |
| `update-install.unauthorized.json` | 未登录 401（状态变更方法） |
| `update-install.forbidden-origin.json` | 跨站 403 |
| `update-rollback.success.json` | 202 rolling_back |
| `update-rollback.rollback-unavailable.json` | 409 `rollback_unavailable` |
| `update-policy.success.json` | PUT 200 回显 |
| `update-policy.invalid-argument.json` | 400 `invalid_argument` |
| `health-ready.success.json` | 匿名 200 |
| `health-ready.not-ready.json` | 匿名 503 |

## 10. 建议值汇总（变更需 bump protocol/contract 版本）

| 项 | 值 | 性质 |
|---|---|---|
| 产品默认 `strategy` | `notify` | 建议值（计划 §2 建议默认） |
| 默认维护窗口 | `02:00`–`06:00` | 建议值 |
| 默认 `freeze_window_minutes` | 30 | 建议值 |
| `update_id` 格式 | `upd-<yyyymmdd>-<8hex>` | 建议值 |
| journal 驻留态 `checked` | §6.6 链上 checking→downloading 之间的驻留点 | 契约新增命名（LCH-010 对齐） |

其余字段名、枚举、状态码、错误码、迁移矩阵均为**冻结**内容。
