# Phase 3 缺陷修复复验：带 import 的通用插件运行时（缺口1）+ 输入来源标注 + SDK 示例复验

> 对应缺口清单：`docs/evidence/phase3_sdk_examples.md` §5 缺口1（阻塞级）、§6 遗留问题
> 日期：2026-09-08 · 分支 main（Wave C 缺陷修复轮，未提交）
> 本轮改动：`server/src/extensions/{wit.rs,wasm.rs}`、`server/src/capabilities/adapters/**`、
> `server/src/extensions/keymap/mod.rs`、`server/src/extensions/gamer_yaml/{runner_adapter.rs,yaml_extension.rs}`、
> 新增 fixture `server/tests/import-guest/**`；`service.rs` 零改动（trap 收敛在 runtime 层完成）

## 1. 缺口1 修复：通用插件带 import 即 trap + 进程 abort

**方案选型 = (a) 全 async 绑定**（宿主函数保持 `*_async` 导出 + guest 入口走 async 变体），
不选 (b)（imports 改同步）：HostState 全部域实现已是 `async fn`，能力层（adb/timer/帧解码）
本身就是 tokio 异步——同步宿主函数内无法 `block_on`（嵌套 runtime panic），要引入阻塞适配层，
风险与改动面都更大。wasmtime 48 起 `Config::async_support` 已废弃为 no-op（async 由
cargo feature `async` 保证，已在 default 依赖里），故引擎配置无需变更，代码内已注释说明。

| 文件 | 改动 |
| --- | --- |
| `server/src/extensions/wit.rs` | `extension-host` bindgen 增加 `exports: { default: async }`（imports 原有 `default: async` 不变）。exports 声明 async 后 `call_run`/`call_call` 生成 async 包装（fiber 入口）；组件类型与 guest 契约不变 |
| `server/src/extensions/wasm.rs` | ① `call_run`/`call_call` 改 `.await`（async import 链接后 store 恒 `async_required`，同步入口必 trap——缺陷根因）；② trap 收敛防护：`catch_unwind` 覆盖实例线程 block_on 全程并注释保证「fiber 内 panic 只损失当前实例」；实例化失败后的线程 panic 载荷改为日志收敛，**删除 `resume_unwind`**（杜绝把 panic 抛回调用方线程形成 double-panic abort） |
| `server/src/extensions/service.rs` | 零改动：start 失败路径 `mark_start_failed` 已记 Failed + last_error；entry trap 只损失实例（进程与状态机不受影响），重启不再有崩溃循环 |

**回归 fixture**：`server/tests/import-guest/`（wit-bindgen 真实生成 + wit-component 编码，
与 `tests/call-guest` 同一工程法；guest import `context.get` + `log.write`，`run` 两者都调，
`call("ping")` 经 import 回读 AppContext）。新增回归测试
`extensions::wasm::tests::guest_with_host_imports_starts_calls_and_stops_without_trap`：
start → entry 完成 → call 断言 context 往返 → stop → call 不可达。
（缺陷形态下该测试必失败：链接期 `async_required` 后同步入口 trap，实例线程 fiber 收尾
panic → abort，测试进程连带垮掉。）

## 2. 缺口2 修复：输入事件来源标注

录制合同 §2.1 词表 `manual|keymap|runner|plugin`。原实现里适配器写死 `"plugin"`，
keymap/runner 来源被错误标注。

- `capabilities/adapters/mod.rs`：新增 task-local 调用方来源上下文
  `with_caller_input_source(source, fut)` / `caller_input_source()`；适配器统一入口
  `with_capability_input_source` = 调用方 scope 优先、缺省 `"plugin"`（向后兼容：
  不 scope 的调用方行为不变）。input/touch 两适配器的 `with_input_source` 调用点改为走它。
- `extensions/keymap/mod.rs`：`CapabilityDeviceActionExecutor::execute` 整体 scope
  `"keymap"`（新增测试 `executor_scopes_capability_input_source_as_keymap`）。
- `extensions/gamer_yaml/runner_adapter.rs`：`EngineExecutor::execute` scope `"runner"`。
- `extensions/gamer_yaml/yaml_extension.rs`：`NativeYamlHost::invoke_json`（guest 的
  capability.invoke 后端）scope `"runner"`——**必须落在这里**：guest 能力调用经
  `block_on_yaml` 派生独立线程，task-local 不跨线程，仅标 runner_adapter 够不到真实路径。
- api/devices.rs 的 `"manual"` 与通用 wasm 插件缺省 `"plugin"` 均不变。
- （`recording/**` 未改动：`with_input_source`/JSONL `source` 字段语义原样。）

## 3. 复验实测（问题3，隔离实例 REST 全记录）

实例：`GB_CONFIG=<tmp>/gamer-phase3fix/config.toml`（port 18443、独立 data_dir、
空 password_hash）+ `GAMER_ADMIN_PASSWORD=phase3fix-admin-2026`，二进制
`server/target/debug/gamer-server.exe`（`CARGO_PROFILE_DEV_DEBUG=0 cargo build -j 4`）。
启动日志 `auth enabled … source=env:GAMER_ADMIN_PASSWORD`、`ready on http://0.0.0.0:18443`。
示例产物沿用 `sdk/examples/*/dist`（sha256 与 phase3 证据一致：hello `91d59697…`、
vision-probe `31b68766…`；`signer inspect` id/version/kind 复核通过）。

### 3.1 hello（带 import：resource/log/context + declarative UI）

```text
== LOGIN ==          POST /api/login {"username":"admin","password":"…"}
→ {"ok":true,"username":"admin"}
== INSPECT ==        POST /api/extensions/inspect（archive 字节）HTTP 200
→ id=com.example.hello version=1.0.0 kind=wasm host_api={log:^1.0,resource:^1.0}
  permission_diff.added=["resource.read","log.write"] ui=[panel hello, declarative]
== INSTALL ==        POST /api/extensions + x-gamer-permission-confirm: true → 201
→ {"id":"com.example.hello","version":"1.0.0","state":"running","last_error":null}
  （缺陷基线：此处进程 abort + 重启崩溃循环；修复后进程存活、Running 干净）
== CALL greet ==     {"action":"greet","values":{"message":"fix-verify-greeting","level":"info"}}
→ {"action":"greet","echo":"fix-verify-greeting","logged_at_level":"info","ok":true}
== CALL probe_denied ==  未授予 device.read
→ {"action":"probe_denied","denied":true,"kind":"denied",
   "message":"插件未获授予权限: device.read","ok":true}
== Package 私有数据 ==  POST /api/packages {id:hello-data} → 201
  PUT /api/packages/hello-data/plugins/com.example.hello/resources/data/state.json
  （body {"content":"{\"count\":13}","force":true}）→ 200 size=12 version=a801e386…
== STOP→START(app_context) ==  stop → enabled；start body
  {"app_context":{"device_id":"selftest","android_package":"com.example.app",
                  "content_package":"hello-data"}} → running
== CALL check_resource ==  guest 经 WIT resources.resolve+open 读私有数据
→ {"action":"check_resource","bytes":12,"found":true,"ok":true,
   "package":"hello-data","path":"data/state.json"}
== CALL 未声明 action ==  {"action":"refresh"} → 400
  {"error":"插件调用被拒绝: action 不在插件 declarative UI 声明的按钮集合内: refresh"}
== STOP ==           → enabled；随后 call → 409（生命周期拒绝）
== UNINSTALL ==      DELETE /api/extensions/com.example.hello/1.0.0 → 204
== 终态 ==            GET /api/extensions → {"extensions":[],…}
```

### 3.2 vision-probe（device/vision/input 权限 + 只读探针）

```text
== INSPECT ==   HTTP 200：id=com.example.visionprobe kind=wasm
  host_api={device:^1.0, vision:^1.0, input:^1.0}
  permission_diff.added=["device.read","vision.match","vision.color","input.tap"]
== INSTALL ==   +x-gamer-permission-confirm: true → 201 state=running last_error=null
== CALL probe（无真机：device_id=no-such-device）==
→ {"action":"probe","kind":"not-found","ok":false,"stage":"device.resolve",
   "message":"capability resource was not found: device no-such-device"}
  链路证明：guest import device.resolve → 宿主 device.read 权限门（放行，否则
  kind=denied）→ DeviceAdapter 真实解析 → NotFound 结构化错误原路返回 guest 分型。
== CALL tap（confirm_tap 缺省）==  dry-run
→ {"action":"tap","dry_run":true,"ok":true,
   "would":{"op":"input.tap","device":"no-such-device","point":[100,100]}}
== STOP ==      → enabled；UNINSTALL → 204；终态 extensions=[]。
```

### 3.3 进程存活与日志

实例自 install 到 uninstall 为同一 PID（27056，`netstat` 复核）；server.log 三次
`WASM extension component started`（hello×2、vision-probe×1），全程零
`entrypoint trapped`、零 abort，所有 REST 调用结果正确。stderr（server-stdout.log）
仍有 3 条已知 Windows GNU fiber 收尾 panic（tokio `c.runtime.get().is_entered()`，
与零 import 时代同源的实例线程收尾问题）——直接 abort）。根治该收尾 panic 属 tokio/wasmtime 上游交互问题，另行跟踪。
本轮 catch_unwind 防护正是命中该场景：
panic 被隔离在实例线程、进程与服务不受影响（缺陷基线同类 panic 走非 unwind 路径
直接 abort）。根治该收尾 panic 属 tokio/wasmtime 上游交互问题，另行跟踪。
结束后实例已停，数据目录在系统临时目录。

## 4. 测试结果（过滤子集，`CARGO_PROFILE_DEV_DEBUG=0`）

| 子集 | 结果 |
| --- | --- |
| `cargo test wasm`（含新回归 `guest_with_host_imports_starts_calls_and_stops_without_trap`） | 41 passed |
| `cargo test keymap`（含新增来源标注测试） | 24 passed |
| `cargo test extensions::` | 174 passed |
| `cargo test recording::` | 15 passed, 1 ignored（既有） |
| `cargo test architecture_guard`（七守卫，白名单双向校验含条目存活） | 7 passed |
| `cargo test adapters::`（含新增 `capability_input_source_defaults_to_none_and_honors_scope`） | 8 passed |

`cargo check --all-targets` 干净（零 warning）。未跑全量（集成者统一跑）。

## 5. 遗留问题

- `sdk/examples/**` 零改动即全链可用（缺口1 修复后 guest 侧契约即正确形态），示例
  `README` 的宿主缺陷警示可在 SDK 轮随下一版本自行摘除（非本轮所有权）。
- 通用 extension-host world 仍未挂 `capability.invoke`（缺口3，维持原状）；
  WIT resources 域仍只读元数据（缺口2，维持原状）。
- entry trap 目前只落日志、生命周期仍为 Running（call 侧可见 trap 错误）；
  如需把 entry trap 反映为 last_error/降级，另开小轮（缺口4 建议，未在本轮范围）。
- 录制 JSONL 的 keymap/runner 来源端到端断言需真机录制会话（Phase 9 E2E 范畴）；
  本轮以适配器观察点单测锁定标注语义。
