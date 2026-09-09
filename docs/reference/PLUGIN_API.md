# 插件 API 参考（gamer:host / manifest v2 / 权限闭集）

> 权威实现：`server/wit/gamer/host.wit`（WIT 契约原件）、
> `server/src/extensions/{manifest,permissions,host_api,wasm}.rs`。
> 本文为面向插件开发者的快照（2026-09-07，契约版本 `gamer:host@1.0.0`，
> 宿主 `HOST_API_VERSION = 1.0.0`）。教程见
> [docs/guides/plugin-dev.md](../guides/plugin-dev.md)。

## 1. WIT world：`extension-host`

```haskell
package gamer:host@1.0.0;

world extension-host {
  import device;    // 设备解析与应用生命周期
  import vision;    // 抓帧 / 模板匹配 / 取色
  import input;     // tap / swipe / key / text
  import touch;     // 多点触控 begin/move/end
  import resources; // Package 资源定位（插件私有前缀）
  import run;       // 运行提交/取消/状态
  import runtime;   // sleep / cancelled
  import log;       // 插件日志
  import context;   // 运行上下文（无权限门禁）

  export extension; // run + call（两个导出都必须存在）
}
```

命名注意：资源域 WIT 接口名为 `resources`（`resource` 是 WIT 保留字），
manifest `[host_api]` 键与权限名仍叫 `resource` / `resource.read`。

另有三个独立契约不归第三方使用：`world yaml-extension-host`（gamer.yaml
专用，request/response 形态）、`server/wit/keymap/keymap.wit`（gamer.keymap
专用 world）、`interface media`（已契约化、**未挂进 world extension-host**，
guest 尚不可消费，权威是宿主内 Rust `MediaDomain` facade）。

### 1.1 共享类型

```haskell
interface types {
  type device-handle = u64;  type frame-handle = u64;
  type resource-handle = u64; type run-handle = u64; type touch-handle = u64;

  enum host-error-kind { denied, unavailable, invalid-request, not-found, cancelled, failed }
  record host-error { kind: host-error-kind, message: string }
}
```

`denied` = 权限未声明（宿主 capability 边界拒绝）；`unavailable` = 能力未
注册；`invalid-request` = 句柄/参数无效；`not-found` = 设备/资源不存在；
`cancelled` = 插件运行已被取消；`failed` = 能力执行失败。

### 1.2 各域函数（签名摘要）

| 域 | 函数 | 所需权限 |
| --- | --- | --- |
| `device` | `resolve(id) -> device-handle`；`start-app(device, app)`；`stop-app(device, app)` | `device.read`；`device.app` |
| `vision` | `capture(device) -> frame-handle`；`match-template(frame, template) -> found(match-box)/not-found`；`sample-color(frame, point) -> (u8,u8,u8)` | `vision.match`（前两者）；`vision.color` |
| `input` | `tap(device, point)`；`swipe(device, start, end, duration-ms≤60s)`；`key(device, code, action: "down"/"up"/"press")`；`text(device, value)` | `input.tap` / `input.swipe` / `input.key` / `input.text` |
| `touch` | `begin(device, point{pressure}) -> touch-handle`；`move(handle, point)`；`end(handle)` | `touch` |
| `resources` | `resolve(namespace, name) -> resource-handle`（文件必须存在；plugin 维度由宿主钉死为调用方 id）；`open(handle) -> byte-len` | `resource.read` |
| `run` | `submit(device, entry-resource) -> run-handle`；`cancel(run)`；`status(run) -> queued/running/succeeded/failed/cancelled` | `run.submit`；`run.control` |
| `runtime` | `sleep(ms≤3_600_000)`（取消位立即中断并返回 cancelled）；`cancelled() -> bool` | `runtime.sleep`（cancelled 免权限） |
| `log` | `write(level: trace/debug/info/warn/error, message, device?, run?)` | `log.write` |
| `context` | `get() -> app-context { device-id?, android-package?, content-package? }` | 无（随实例注入；缺省全 None） |

句柄语义：全部是宿主侧 opaque token（u64），每个实例独立编号；**永远不是
宿主文件路径或传输层 id**。模板短名约定（`icon.png` → `icon#区域.png` 唯一
消歧）在宿主 `resources.resolve` 内实现。

### 1.3 导出接口 `extension`

```haskell
interface extension {
  run: func();                     // 实例化后调用一次；返回后实例驻留等 call
  call: func(action: string, values-json: string) -> result<string, string>;
}
```

`call` 约束：宿主在派发前校验（1）插件 Running（2）`action` 在 manifest
declarative 按钮集合内（否则 400 `CallRejected`）。成功值与 `Err` 值都必须是
**JSON 字符串**（宿主把成功值 parse 成 JSON 对象返回给调用方；`Err` 原样进入
错误信息）。

## 2. manifest v2 字段与校验

严格解析（`deny_unknown_fields`；安装/更新只接受 v2，存量读端容忍 v1）。
完整字段表与 declarative schema 规则见
[plugin-dev.md §4](../guides/plugin-dev.md#4-manifest-v2-全字段参考)，此处只列
易错点：

- `id` 语法：ASCII `[A-Za-z0-9._-]`、不以 `.` 开头/结尾、非 Windows 保留名、
  ≤128B；插件 id 与 Android 包名、Package id 是三个无关命名空间，不互相推导。
- `[targets.android].packages`（可缺省）：插件支持的 Android 应用声明，
  `*` = 通用（全部应用）；**缺省/空声明等价 `*`**。与 package.toml 的
  `[targets.android]` 同形，仅作运行目标声明、宿主不做硬门禁——Console
  壳按当前设备应用过滤插件入口（不命中不显示，`*` 恒显示），快照与
  inspect 响应把空声明归一为 `["*"]` 透传。
- `entry`：`.wasm` 后缀、不得指向 `manifest.toml`；zip 内必须存在且 ≥4 字节、
  `\0asm` magic。builtin（`[execution] kind="builtin"` + `builtin_id`）必须
  **没有** `entry`，且包内不得携带 `plugin.wasm`（防伪装执行类型）。
- `[host_api]` 九域：`device/vision/input/touch/resource/run/runtime/log/media`，
  值为 SemVer range；宿主当前全域 1.0.0；不满足在安装期返回
  `unsupported_host_api`（required/supported 结构化字段）。
- UI：`location` 仅 `console.right`；`runtime` 三档
  `declarative | iframe | core`；iframe `entry` 限 `ui/` 下（经认证端点
  `GET /api/extensions/:id/ui/*path` 提供，不挂宿主 origin 之外的域）；
  `core` 是宿主预置组件专用（component 键由前端 core-component-registry
  解释，第三方无可挂载组件）。
- `[[dependencies]]`（可缺省，简化计划 Phase 3）：插件依赖声明
  `{id, version?, required?}`——`version` 为 SemVer range（缺省 `*`），
  `required` 缺省 `true`（可选依赖必须显式 `required = false`）。语义：
  **必需依赖 = 启动门禁**（缺失/版本不兼容/未启用 → enable 的 start 阶段
  结构化拒绝，扩展保留 Enabled + last_error，处置后可重试；必需依赖循环
  拒绝启动）；**可选依赖 = 能力降级提示**（缺失不阻止启动，消费方按能力
  发现接口自行降级相关功能入口）。不做自动下载/自动启用/级联停用：停用或
  卸载被运行中插件必需依赖的插件会被拒绝（提示先停用依赖方）。依赖声明
  **不授予任何权限**，与 package.toml `[plugins]`（Package 依赖）是两个
  不同层面的概念。

## 2.1 能力发现与跨插件调用（简化计划 Phase 4）

- `GET /api/extensions/:id/capabilities` → `{id, state, running,
  actions:[{action, version, surface, summary}]}`：目标插件对外公开的动作
  集合 = declarative 按钮集合（surface `declarative`）∪ 原生公开动作清单
  （gamer.yaml 的 `template.create_from_frame` 等，surface `native/rest/
  frontend`）。其他插件/前端在调用前据此查询——**动作存在 ≠ 可调用**，
  分发时仍须目标 Running + 公开集合门禁 + 权限/上下文校验。
- 跨插件调用统一走 `POST /api/extensions/:id/call`（目标必须 Running），
  不为每一组插件增加专用 ID 分支；不做分布式 RPC/服务发现/消息总线。

## 3. 权限闭集（19 项，默认拒绝）

来源：`server/src/extensions/permissions.rs`（封闭枚举 + 显式禁区）。
权限 → Host API 域映射与状态标注见
[plugin-dev.md §5](../guides/plugin-dev.md#5-权限闭集默认拒绝显式申请)。

- 14 项稳定：`device.read` `device.app` `vision.match` `vision.color`
  `input.tap` `input.swipe` `input.key` `input.text` `touch` `resource.read`
  `run.submit` `run.control` `runtime.sleep` `log.write`；
- 5 项实验（media.*，WIT 未挂 world，guest 当前不可消费）：`media.read`
  `media.import` `media.record` `media.write` `media.events.read`；
- 显式禁区（声明即拒，不可授予）：`filesystem*` `network*` `shell*`
  `process*` `device.shell*`。

授权链：manifest 声明 → 安装时权限增量需用户确认（`x-gamer-permission-confirm`）
→ 运行时宿主在每次 capability 调用前检查（未声明 → `HostError{kind:denied}`）。
签名/来源不参与授权。

## 4. REST 端点（安装与调用面）

| 端点 | 说明 |
| --- | --- |
| `POST /api/extensions/inspect` | 安装预览：id/版本/执行形态/权限与增量/host_api/UI/sha256/是否已装；支持 `x-expected-sha256` 完整性钉 |
| `POST /api/extensions` | 安装（zip 字节）；自动 enable→start（启动失败降级 Enabled + last_error）；权限增量需确认头 |
| `POST /api/extensions/:id/update` | 装新版本并激活（Running 拒绝）；版本回退 = 卸载后重装旧归档（历史版本仅展示） |
| `POST /api/extensions/:id/enable|disable` | 用户生命周期（V1 收敛：enable = 启用意图 + 直接启动，幂等；可带 `{app_context}`，keymap 另支持 `profile`；disable 运行中自动 stop；`/start` `/stop` `/activate` 细粒度端点已删除，内部保留原语） |
| `POST /api/extensions/:id/call` | `{action, values}` → 公开动作集合门禁（declarative 按钮白名单 ∪ 原生动作清单）→ 常驻实例 `call` / 原生实现；返回 JSON |
| `GET /api/extensions/:id/capabilities` | 能力发现：`{id, state, running, actions}` 公开动作清单（跨插件调用前查询，简化计划 Phase 4） |
| `DELETE /api/extensions/:id/:version` | 卸载（Running 拒绝；被运行中插件必需依赖引用时拒绝；最后一版才清状态；不删 Package 数据） |
| `GET /api/extensions` | 列表 + `runtime_available` + UI 贡献注册表 + 各插件 `targets.android`（空声明归一 `["*"]`）+ 依赖实时状态 `dependencies[{id,version_req,required,installed,version,state,satisfied,note}]` |
| `GET /api/extensions/management` | 管理视图（执行形态/权限/宿主 API/依赖任务） |
| `GET /api/extensions/ui` 、`GET /api/extensions/:id/ui/*path` | UI 贡献注册表 / iframe 资产 |
| `GET|POST /api/packages/:pkg/plugins/:plugin/resources[/*path]`、`PUT|DELETE …/*path`、`POST …/rename` | Package 插件资源（文本 JSON 乐观并发 / 模板 PNG 字节）；第三方插件数据读写通道 |

## 5. 版本与兼容语义

- `gamer:host` 包版本、manifest `manifest_version`、`HOST_API_VERSION` 三者
  独立演进：manifest 结构破坏性变更升 `manifest_version`；Host API 域 ABI
  变更升 `gamer:host` / `HOST_API_VERSION`（guest 的 wit 快照须同步重编，
  旧 guest 实例化失败进 Failed 态，报错可见）。
- `[execution] host_version` 是 advisory 声明（仅展示，无硬门禁）。
- 定期核对：`sdk/examples/*/wit/gamer/host.wit` 应与 `server/wit/gamer/host.wit`
  逐字一致；不一致 = 示例契约快照过期。
