# Launcher IPC protocol v1 契约（ARC-003）

> 状态：**冻结**（批次 0 契约；帧格式、字段、操作枚举、上限/超时建议值只能以版本化契约变更——任何变更必须 bump 到 `ipc-v2` 并单独提交 fixture，通知 server/launcher/web 三轨同步）。
> 依据：`docs/plans/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §6.4（Launcher IPC 与 System API）、§6.6（持久化升级状态机）；`docs/guides/UPDATE_CONTRACT.md` §3.1（launcher 职责：Windows named pipe IPC server，protocol v1，仅当前用户 DACL，只接受内部枚举操作）。
> 产出位置登记于 `docs/guides/UPDATE_CONTRACT.md` §6 文件地图（`release/contracts/ipc-v1.md`）。
> 配套 fixture：`release/contracts/fixtures/ipc/*.json`（帧的 JSON 载荷部分；长度前缀为二进制，不在 fixture 内）。
> HTTP 侧的错误码/状态机见同目录 `system-api-v1.md`；IPC 与 HTTP 共享同一套业务错误码。

## 1. 寻址与安全

### 1.1 pipe 名

```text
\\.\pipe\gamebot-launcher-<installation-id>
```

- `<installation-id>`：launcher 首次初始化时生成、此后恒定的安装实例标识；字符集建议 `[a-z0-9]`、长度 8~32（**建议值**），仅用于组成 pipe 名，不含机器敏感信息。
- launcher 启动 server 时通过环境变量 `GAMER_LAUNCHER_PIPE` 注入**完整 pipe 名**（server 现有 `deployment_mode` 探测已读取该变量）；server **不得**自行猜测/拼接 pipe 名（installation-id 只有 launcher 知道）。
- launcher 同时通过环境变量 `GAMER_LAUNCHER_IPC_TOKEN` 注入本次启动随机生成的会话令牌（**建议值**为变量名；令牌本身为 ≥32 字节随机数的 hex/base64 文本）。两个变量名或传递方式变更需 bump contract。

### 1.2 DACL

- named pipe 的 `SECURITY_ATTRIBUTES` DACL **仅允许当前用户 SID 与 SYSTEM** 读写；不授予 Everyone/Anonymous 任何权限。
- server 是 launcher 的同用户子进程，凭当前用户凭据连接即可；每个请求帧仍须携带 §3 的 `auth` 令牌，双重校验（DACL + 令牌），任一失败即拒绝并断开。
- launcher 对令牌不匹配的连接：返回协议错误帧后立即断开，并写入 launcher 日志；不做重试提示。

### 1.3 传输模式

- 字节模式（byte-mode）named pipe；一请求一响应，不支持服务端主动推送；同连接可顺序多次请求。
- server 侧持有单条长连接 + 有界重连（失败后建议退避 1s/2s/5s，封顶 30s——**建议值**）；launcher 不在场时 server 走 §7 降级，不做重连风暴。

## 2. 帧格式

```text
+----------------------+----------------------------------+
| u32 little-endian    |  UTF-8 JSON 载荷（定长）          |
| = JSON 载荷字节数    |                                  |
+----------------------+----------------------------------+
```

- **长度前缀为 u32 little-endian**（明确端序），值 = 紧随其后的 UTF-8 JSON 载荷的**精确字节数**，不含前缀自身 4 字节。
- JSON 序列化形式（空白/键序）不限；双方只依赖解析结果。
- **单帧上限 1 MiB（1048576 字节）**（建议值）：收到的长度前缀 > 上限时，接收方**立即断开连接**（不回错误帧、不缓冲、不读取载荷），并记录本地日志；该上限双向适用。
- 一个逻辑请求 = 恰好一帧请求 + 恰好一帧响应；无分片、无流式。

## 3. 帧字段

### 3.1 请求帧

```json
{
  "protocol_version": 1,
  "request_id": "0b7f8c1e-5b6a-4a57-9a2e-2f1c3d4b5a6f",
  "auth": "<GAMER_LAUNCHER_IPC_TOKEN>",
  "operation": "status",
  "payload": {}
}
```

| 字段 | 类型 | 冻结说明 |
|---|---|---|
| `protocol_version` | number | 本协议恒为 `1`；不匹配 → 协议错误帧 `unsupported_protocol_version`（见 §6） |
| `request_id` | string | 非空、≤64 字符、同连接内唯一；建议 UUID v4（**建议值**）。**同一逻辑请求的重发必须复用同一 `request_id`**（幂等键，§5） |
| `auth` | string | launcher 注入的会话令牌；逐帧校验 |
| `operation` | string | §4 操作枚举之一；未知值 → `unknown_operation` |
| `payload` | object | 各操作冻结的载荷；**未知/多余字段一律拒绝**（fail closed）→ `invalid_payload` |

### 3.2 成功响应帧

```json
{
  "protocol_version": 1,
  "request_id": "<与请求相同>",
  "ok": true,
  "result": { "<按操作定义>" }
}
```

### 3.3 错误响应帧（冻结形态）

```json
{
  "protocol_version": 1,
  "request_id": "<与请求相同；无法定位请求时填空字符串>",
  "ok": false,
  "code": "<错误码>",
  "message": "<一句话描述，不含路径/用户名/命令行>"
}
```

- `code` 两类：**业务错误码**与 HTTP API 共享同一套 11 个统一错误码（`system-api-v1.md` §7），server 可 1:1 映射进 `last_error`；**协议级错误码**仅存在于 IPC（见 §6.2）。
- `request_id` 无法关联（如 JSON 解析失败、令牌不匹配）时填 `""`，且响应后立即断开。

## 4. 操作枚举（冻结，6 个）

操作一律是**内部枚举**；launcher **永不**接受 shell 命令字符串、任意 URL、路径参数（UPDATE_CONTRACT §3.1）。

| operation | payload（冻结） | 语义 | 同步/长操作 |
|---|---|---|---|
| `status` | `{}` | 查询升级状态机/journal 快照、当前/上一版本、schema、依赖健康（§4.1） | 同步 |
| `check` | `{}` | 检查远端 release（通道来自 launcher 配置，不接受请求指定），验签 manifest | 长操作 |
| `download` | `{}` | 下载当前候选的应用/组件包至 cache→staging 并校验 | 长操作 |
| `prepare_install` | `{}` | 对最近下载的候选做安装前整备（复验 staging 完整性、标记可切换） | 长操作 |
| `rollback` | `{}` | 触发 committed 之前的自动回滚（恢复 previous + 已验证快照） | 长操作 |
| `repair_dependency` | `{ "dependency": "adb" \| "ffmpeg" }` | 依赖修复编排（inventory→seed/cache→remote→probe）；`scrcpy` 不可修（随应用版本整体更换） | 长操作 |

- 除 `repair_dependency` 外 payload 恒为 `{}`；`repair_dependency` 的 `dependency` 为内部枚举，**不是**路径/命令字符串。
- 长操作统一**受理即回**：launcher 在 ≤30s 内回 `{ "ok": true, "result": { "accepted": true, ... } }`，随后由调用方以 `status` 轮询进展（§5.2）；任何操作都不允许长时间占住一帧响应。

### 4.1 `status` 的 `result`（冻结）

```json
{
  "launcher_version": "0.1.0",
  "installation_id": "a1b2c3d4e5f6a7b8",
  "protocol_version": 1,
  "versions": { "current": "0.2.0", "previous": "0.1.0" },
  "schema": { "db": 1, "file": 1, "rollback_floor": 1 },
  "update": {
    "state": "downloading",
    "detail": "downloading",
    "update_id": "upd-20260831-9f3ab2c1",
    "candidate": { "version": "0.3.0", "channel": "stable", "published_at": "2026-09-15T00:00:00Z" },
    "progress": { "bytes_done": 402650112, "bytes_total": 893451200 },
    "last_error": null
  },
  "dependencies": {
    "adb": { "status": "ready", "version": "34.0.5" },
    "ffmpeg": { "status": "ready", "version": "6.1.1" }
  }
}
```

- `update.state`：与 HTTP API 相同的 11 态展示枚举；`update.detail`：journal 精细步骤（映射表见 `system-api-v1.md` §5.2）。server 据此直接合成 `GET /api/system/update`。
- `progress` 仅 `downloading` 态非空；`candidate` 无候选时 `null`；`last_error` 形态 = `{ "code", "message" }`，code 属于 11 个业务错误码。
- `dependencies.*.status` 枚举与 `/api/system/info` 的 `dependencies.<id>.status` 一致（`ready|missing|broken|unknown`），供 `repair_dependency` 受理后轮询修复结果。

### 4.2 长操作受理 `result`（冻结）

```json
{ "accepted": true, "operation": "download", "update_id": "upd-20260831-9f3ab2c1", "state": "downloading" }
```

`state` 为受理时状态机进入的 11 态值；`update_id` 为本次升级事务 id。

## 5. 超时、轮询与幂等

### 5.1 超时（均为**建议值**，变更需 bump protocol 版本）

| 项 | 值 |
|---|---|
| 单帧交换超时（请求发出→响应帧收齐，双向适用） | 默认 **30s** |
| pipe 连接建立超时 | 5s |
| 长操作受理时限 | 与单帧交换超时相同（30s 内必须回受理帧或错误帧） |

### 5.2 轮询

- 长操作（check/download/prepare_install/rollback/repair_dependency）受理后，server 以 `status` 轮询进展；**建议**活跃事务期间 1s 一次、空闲 5s 一次（**建议值**）。
- server → 前端的进展暴露仍走 `GET /api/system/update`；前端轮询节流由 WEB 侧契约（`system-api-v1.md`）约束，IPC 轮询不直接穿透到浏览器。

### 5.3 幂等语义（冻结）

1. **同 `request_id` 重发**（超时重试、连接重连后重发）：launcher 返回**相同结果**——同步操作返回相同 `result`；长操作返回原受理帧（或当前最新状态的受理帧），**绝不**触发第二次事务。
2. 幂等去重窗口建议 **10 分钟**或至该事务离开受理态（以先到者为准）（**建议值**）；窗口外的旧 `request_id` 视为新请求。
3. **不同 `request_id` 的同类长操作**并发到达：指向同一升级事务的（如 download 进行中再次 download）→ 返回当前状态受理帧、复用同一 `update_id`，不开新事务。
4. **冲突操作**并发到达（如 download 进行中收到 rollback）：launcher 只有一个升级事务，冲突方回错误帧 `update_busy`。
5. `status` 为只读，无幂等副作用。

## 6. 错误码

### 6.1 业务错误码（与 HTTP API 共享，11 个）

`update_not_managed`、`update_busy`、`update_not_available`、`update_not_ready`、`signature_invalid`、`artifact_invalid`、`insufficient_space`、`schema_incompatible`、`launcher_unreachable`、`rollback_unavailable`、`manual_recovery_required`——触发条件见 `system-api-v1.md` §7；launcher 侧产生、经错误帧回传后由 server 1:1 映射为 HTTP 错误或 `last_error`。

### 6.2 协议级错误码（仅 IPC，冻结）

| code | 触发 | 处置 |
|---|---|---|
| `unsupported_protocol_version` | `protocol_version != 1` | 回错误帧后断开 |
| `unknown_operation` | `operation` 不在 §4 枚举 | 回错误帧；连接可保留 |
| `invalid_payload` | payload 缺字段/多字段/枚举值非法 | 回错误帧；连接可保留 |
| `unauthorized` | `auth` 令牌缺失/不匹配 | 回错误帧后立即断开并记日志 |
| `internal_error` | launcher 内部非预期失败 | 回错误帧；事务状态以 `status` 为准 |

## 7. 降级：UnsupportedUpdateController（冻结）

- **直跑 server / Docker 模式无 launcher**：不注入 `GAMER_LAUNCHER_PIPE`/`GAMER_LAUNCHER_IPC_TOKEN`，server 以 `UnsupportedUpdateController`（Docker 为 external strategy 适配器）降级——**从不创建 IPC 连接**，所有更新动作 API 返回 `update_not_managed`（HTTP 409），capability 全 false（UPDATE_CONTRACT §3.3、计划 §6.4）。
- launcher 模式下 launcher 进程死亡/pipe 消失：server 的 UpdateController 降级为「不可达」态——更新动作返回 `launcher_unreachable`（502），`/api/system/info` 的 capability 按 IPC 通道实际健康置 false；**server 不因 launcher 不在而启动失败、不退出、不自动拉起 launcher**。

## 8. fixture 索引（`release/contracts/fixtures/ipc/`）

| fixture | 内容 |
|---|---|
| `status.json` | `status` 请求帧 + 成功响应帧（result 全量示例） |
| `check.json` | `check` 请求帧 + 受理响应帧 |
| `download.json` | `download` 请求帧 + 受理响应帧（含进度轮询中的 `status` 响应示例） |
| `prepare_install.json` | `prepare_install` 请求帧 + 受理响应帧 |
| `rollback.json` | `rollback` 请求帧 + 受理响应帧 |
| `repair_dependency.json` | `repair_dependency`（枚举 payload）请求帧 + 受理响应帧 |
| `error-frame.json` | 一个业务错误帧示例（`signature_invalid`） |
| `error-frames-protocol.json` | 协议级错误帧示例集（`unsupported_protocol_version` / `unknown_operation` / `invalid_payload` / `unauthorized` / `internal_error`） |

## 9. 建议值汇总（变更需 bump protocol/contract 版本）

| 项 | 值 |
|---|---|
| 单帧上限 | 1 MiB（1048576 字节） |
| 单帧交换超时 | 30s（默认） |
| 连接建立超时 | 5s |
| 重连退避 | 1s/2s/5s，封顶 30s |
| `status` 轮询间隔 | 活跃 1s / 空闲 5s |
| `request_id` 去重窗口 | 10 分钟或事务离开受理态 |
| `request_id` 格式 | UUID v4，≤64 字符 |
| `installation-id` 字符集/长度 | `[a-z0-9]`，8~32 |
| 令牌注入环境变量名 | `GAMER_LAUNCHER_IPC_TOKEN`（pipe 名变量 `GAMER_LAUNCHER_PIPE` 已被 server 现有代码读取，随契约一并冻结） |

帧格式、端序、字段名、操作枚举、payload 形态、幂等语义、错误码均为**冻结**内容。
