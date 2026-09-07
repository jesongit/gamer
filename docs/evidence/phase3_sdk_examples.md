# Phase 3 验收证据：SDK、示例与「用户自己写插件自己用」

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §6（Phase 3）
> 日期：2026-09-07 · 分支 main · 基线 HEAD `eae786c` 工作树（免签名 + manifest v2 已入库；
> 并行存在 Phase 4 agent 对 `server/src/extensions/keymap`、`service.rs`、
> `tools/build-plugins.ps1`、`.gitignore` 的未提交改动，本轮未触碰）
> 文件所有权：仅新建 `sdk/**`、`docs/guides/**`、`docs/reference/PLUGIN_API.md`（未改 server/web/tools）

## 1. 完成项对照（计划 §6.1/§6.2/§6.3）

| 计划条目 | 状态 | 落点 |
| --- | --- | --- |
| 最小可用 SDK：示例（manifest + 真实 guest + 运行上下文 + 日志 + 受控 Host API 调用 + 公开 command + declarative 面板 + Package 私有数据） | 完成（3 个示例） | `sdk/examples/{echo-minimal,hello,vision-probe}` |
| Hello World 不需要复制 YAML/Keymap 代码 | 完成 | `echo-minimal` guest 60 余行、`hello` 约 180 行，工程结构同构 |
| 至少一个需要设备/视觉能力的示例，验证权限声明与 Host API 可实际调用 | 部分完成（受宿主缺陷阻塞，见 §5 缺口1） | `vision-probe`（权限声明→安装→inspect 实测通过；capability 实际执行被宿主 runtime 缺陷阻断） |
| build/pack/inspect/install 可重复命令，无需签名密钥 | 完成 | 各示例 `build.ps1` + README 原始命令；复用 `tools/plugin-signer` v0.2.0（`pack`/`inspect`/`verify`，零密钥） |
| 构建产物自包含 guest 与 UI 资产 | 完成 | `.gplugin` = manifest.toml + plugin.wasm（示例未用 iframe UI；`pack --file` 支持附加资产） |
| 文档：SDK 整理、稳定/实验标注、调试诊断 | 完成 | `docs/guides/plugin-dev.md` + `docs/reference/PLUGIN_API.md`（19 项权限闭集逐项标注稳定/实验） |
| 仓库外视角构建安装 | 完成 | 示例目录按独立工程组织（自带 WIT 快照与 Cargo.lock），仅打包依赖仓库内 signer |

## 2. 交付目录结构

```text
sdk/
├── README.md                      # SDK 总览 + 快速开始 + 宿主缺陷警示
├── .gitignore                     # examples/*/target|dist
└── examples/
    ├── echo-minimal/              # 零 import 零权限：全生命周期可运行模板（guest ~60 行）
    ├── hello/                     # declarative UI + call + Package 私有数据 + 日志 + 上下文 + 权限自检
    └── vision-probe/              # device/vision/input 权限 + capture/sample-color 只读探针 + tap dry-run
    （每个示例：manifest.toml + Cargo.toml + build.ps1 + wit/gamer/host.wit + src/{lib.rs,bin/componentize.rs} + README.md）
docs/
├── guides/plugin-dev.md           # 从零到安装运行教程（manifest 全字段/权限闭集/调试对照表/更新语义/能力边界）
└── reference/PLUGIN_API.md        # WIT world / Host API 域与版本 / 19 项权限闭集 / REST 面（新增文件）
```

## 3. 构建（仓库外视角）

环境：rustup 已有 `wasm32-unknown-unknown`；signer 构建一次
（`cargo build --release --manifest-path tools/plugin-signer/Cargo.toml` →
`gamer-plugin-signer.exe`；注意 bin 名是 crate 名 `gamer-plugin-signer`）。

三个示例各自执行（以 hello 为例；echo-minimal / vision-probe 同构，产物
sha256 记录于当次运行输出）：

```sh
cd sdk/examples/hello
cargo build --release --lib --target wasm32-unknown-unknown --target-dir target   # PASS 54s
cargo run --release --bin componentize --target-dir target -- \
  target/wasm32-unknown-unknown/release/hello_plugin_guest.wasm \
  target/plugin.component.wasm                                                    # PASS（组件 129,641B）
pwsh ./build.ps1                                                                   # PASS
#   inspect: id=com.example.hello version=1.0.0 kind=wasm entry=plugin.wasm
#   pack:    sha256=91d59697… size=53558
#   verify:  id/version/kind/sha256/size 全部一致 → "OK: …\dist\com.example.hello-1.0.0.gplugin"
```

vision-probe 产物 55,218B（首次 pack sha256 `017186e0…`，build.ps1 复跑
`31b68766…`），echo-minimal 49,475B（`a6c0c78b…`）。`signer verify` 三包全过。
（已知：.gplugin 非字节可复现——zip entry mtime 取当前时间，两次构建 sha
不同，与 PITFALLS P12.8 一致。）

## 4. 真实安装验证（REST 全记录）

### 4.1 实例环境

8443 被并行 agent 的 gamer-server（PID 2120，凭据未知）占用；为不干扰并行
工作、不改 `server/**` 与 `server/data/`，用环境变量拉起**隔离实例**：
`GB_CONFIG=<tmp>/gamer-phase3/config.toml`（port 18443、独立 data_dir、空
`password_hash`）+ `GAMER_ADMIN_PASSWORD=phase3-admin-2026` +
`GB_LOG=<tmp>/server3.log`，二进制 `server/target/debug/gamer-server.exe`
（`CARGO_PROFILE_DEV_DEBUG=0 cargo build -j 4`，1m25s PASS）。启动日志
`auth enabled … source=env:GAMER_ADMIN_PASSWORD`、`ready on http://0.0.0.1:18443`。

### 4.2 请求/响应摘录（curl，隔离实例 18443）

```text
== LOGIN ==            POST /api/login {"username":"admin","password":"…"}
→ {"ok":true,"username":"admin"}

== INSPECT hello ==    POST /api/extensions/inspect  (archive 字节)
→ {"id":"com.example.hello","version":"1.0.0","execution":{"kind":"wasm"},
   "host_api":{"log":"^1.0","resource":"^1.0"},
   "permission_diff":{"added":["resource.read","log.write"],…},
   "ui":[{"panel_id":"hello","runtime":"declarative","title":"Hello 示例",…}]}
== INSTALL hello ==    POST /api/extensions + x-gamer-permission-confirm: true
→ 201 {"state":"running", …}                       （安装即自动 enable→start）
```

随后 start 触发宿主缺陷（见 §5 缺口1）：进程 abort。重启复现启动循环 →
确认为数据驱动的**重启崩溃循环**后，手工清除
`data/extensions/com.example.hello` + `state.json` 记录解除（用户侧等价操作：
删除插件数据目录）。

清除后以 `echo-minimal` 完成全生命周期验证（同一实例）：

```text
== INSPECT echo ==     → {"id":"com.example.echo","version":"1.0.0","kind wasm","host_api":{},…}
== INSTALL echo ==     POST /api/extensions（无权限增量，无需确认头）
→ 201 {"state":"running","permissions":[],"ui":[{panel_id:"echo",runtime:"declarative",schema:{…fields…}}]}
== CALL sum ==         POST /api/extensions/com.example.echo/call {"action":"sum","values":{"a":19,"b":55}}
→ {"action":"sum","ok":true,"sum":74.0}            （真实 guest 在 wasmtime 内执行）
== CALL echo ==        同上 {"text":"第三方插件回显","n":42}（UTF-8 文件体）
→ {"action":"echo","echo":{"n":42,"text":"第三方插件回显"},"ok":true}
== CALL evil ==        未声明 action → 400 {"error":"插件调用被拒绝: action 不在插件 declarative UI 声明的按钮集合内: evil"}
== 重复安装同版本 ==    → 409 {"error":"插件 com.example.echo@1.0.0 已安装"}
== GET /api/extensions/ui == → declarative 贡献已注册（含完整 fields schema）
== STOP ==             → state enabled；随后 call → 409（生命周期拒绝）
== START + app_context == {"app_context":{"device_id":"selftest","android_package":"com.example.app","content_package":"echo-data"}} → running；call 复通
== UPDATE 1.0.1 ==     stop（→enabled）→ POST update → active_version=1.0.1, installed_versions=[1.0.0,1.0.1], state=enabled
== ACTIVATE 1.0.0 ==   → active_version=1.0.0
== UNINSTALL 守卫 ==    Running 时 DELETE 1.0.0 → 409；stop 后 → 204；非激活版 1.0.1 → 204
== 终态 ==              GET /api/extensions → {"extensions":[],…}
```

诊断错误对照实测（供 `docs/guides/plugin-dev.md` §8，服务端原文）：

```text
v1 manifest 安装     → "插件 manifest 无效: manifest_version=1 已不受支持，请升级插件包到 manifest_version=2"
sha256 钉不匹配      → "宿主归档 sha256 校验失败: 期望 000…0，实际 a6c0c78b…"
禁区权限 filesystem  → "插件权限错误: 插件权限默认拒绝且不可授予: filesystem.all"
权限增量未确认       → 409 "插件权限变更需要用户确认: 新增权限: resource.read, log.write"
```

Package 私有数据通道（REST 侧）：`POST /api/packages` 建 `hello-data` →
`PUT /api/packages/hello-data/plugins/com.example.hello/resources/data/state.json`
→ 200（落盘 `data/packages/hello-data/plugins/com.example.hello/data/state.json`，
13B，内容版本短码返回）。guest 侧经 WIT 读取/写入的验证受 §5 缺口1 阻断
（`hello` 的 `check_resource` 动作已实现：resolve + open 字节长度 + not-found
分型）。UI 面板浏览器可见性属浏览器验收，未逐项点检（ declarative 贡献
已随 `GET /api/extensions/ui` 下发，schema 完整；不阻塞）。

结束处置：隔离实例已停；隔离 data 目录整体位于系统临时目录，未混入仓库。

## 5. 发现的 server 侧能力缺口（按严重度）

1. **【阻塞级】通用 extension-host 无法执行任何携带 import 的插件，且失败即进程 abort + 重启崩溃循环。**
   `extensions/wit.rs` 对 `world extension-host` 的 bindgen 用
   `imports: { default: async }`，而 `extensions/wasm.rs::LazyWasmtimeRuntime`
   用 `Config::new()`（未开 async_support）并以**同步** `call_run`/`call_call`
   进入 guest。async 宿主函数在链接进 store 时置
   `store.async_state.async_required = true`（wasmtime 48
   `set_async_required(Asyncness::Yes)`，链接期一次性、不可逆），此后任何同步
   入口都触发 `store configuration requires that *_async functions are used
   instead`。实测：install→running 后日志
   `WASM extension entrypoint trapped error=store configuration requires …`，
   随即实例线程在 fiber 收尾中 `panic in a destructor during cleanup` →
   **non-unwinding panic → abort**（`use of std::thread::current() is not
   possible after the thread's local data has been destroyed` +
   `c.runtime.get().is_entered()` 断言链）；重启时 `reconcile_startup` 对遗留
   Running 记录再次 start → 再次 abort → **启动循环崩溃**，须手工清理数据目录
   才能起服。现有测试未覆盖：WAT fixture 与 call-guest 均零 import（不触发
   链接期 async 标记）。对照组：yaml/keymap 走各自 world 的**同步** bindgen，
   不受影响。修复建议（集成者裁决）：入口改 `*_async` 变体（实例线程本就是
   current-thread runtime `block_on`，可直接 `.await`）并按需
   `Config::async_support(true)`，或改 bindgen 为同步 import + 宿主侧阻塞
   适配；同时给实例线程 trap 收敛加防护（现网任何 guest trap 可打死整个
   服务进程）。**修复后 hello/vision-probe/文档无需改动即可用**（guest 侧
   契约用法即正确形态）。
2. **【能力缺口】WIT `resources` 域对 guest 只读且仅元数据**：`resolve`
   （存在性，文件缺失 → not-found）+ `open`（字节长度）；无字节内容读取、
   无写入。插件私有数据的字节级读写目前只能走 Package 资源 REST（操作者
   视角）。建议后续 host API 版本补 `read(handle) -> list<u8>` 与
   `write(namespace, name, bytes)`（带 `resource.read` / 新权限）。
3. **【小缺口】`capability.invoke` 未挂进 `world extension-host`**（host.wit
   已定义该 interface 但 world 未 import）——通用插件没有 YAML guest 那样的
   私有事件通道（如 `__event` 运行可视化）；如需插件事件上报，评估挂载。
4. **【已记录非缺口】`install auto-start` 对启动失败只降级 Enabled + last_error**
   （响应可能 201/Enabled 形态）；对 import 型插件而言这掩盖了「Running 但
   实例入口已 trap」的观感——缺口1 修复时建议一并考虑把 entry trap 反映到
   生命周期状态。

## 6. 遗留问题

- `signer pack` 不输出 id/version（inspect 才有），示例 build.ps1 需要两次
  调用拼产物名；`pack --meta-out` 已可一次取回元数据，脚本可再收敛（非阻塞）。
- declarative 面板浏览器端点检（面板渲染、按钮触发 call、执行形态徽标）
  未做，属浏览器验收（Phase 9 E2E 范畴）。
- 示例 vision-probe 的 capability 实测（capture/sample-color 真实取值、
  tap dry-run/confirm 两分支）待缺口1 修复后补跑；权限声明→inspect→安装
  链路本轮已实测。
- `hello` 示例对 `data/state.json` 的写侧演示依赖缺口2（WIT 无写通道），
  当前以 REST 写入 + guest resolve/open 验证存留性；文档已如实标注边界。
