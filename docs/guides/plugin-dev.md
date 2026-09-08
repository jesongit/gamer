# Gamer 插件开发指南（从零到安装运行）

> 适用基线：2026-09-07（免签名安装 + manifest v2；HEAD `eae786c` 工作树）。
> 本文所有字段、端点、错误文案均与当前实现逐条核对；配套 API 参考见
> [docs/reference/PLUGIN_API.md](../reference/PLUGIN_API.md)，可运行的完整示例在
> [sdk/examples/](../../sdk/examples/)（`echo-minimal` / `hello` / `vision-probe`）。
>
> ⚠️ **当前基线的宿主缺陷（2026-09-07）**：通用 extension-host 运行时以
> async 形态链接 Host API import、以同步入口调用 guest。任何**声明了 import**
> 的插件（即任何调用 Host API 的插件）在 start 时触发 trap 并使宿主进程
> abort，重启后还会形成启动循环崩溃。这是宿主侧缺陷，guest 侧写法（本文
> 所教）即宿主修复后的正确写法、无需改动；复现记录见
> [docs/evidence/phase3_sdk_examples.md](../evidence/phase3_sdk_examples.md)。
> 缺陷修复前，可完整运行的最小模板是 `sdk/examples/echo-minimal`（零 import）。

## 1. 你需要什么

| 工具 | 用途 | 安装 |
| --- | --- | --- |
| Rust (stable) + `wasm32-unknown-unknown` target | 编写并编译 guest | `rustup target add wasm32-unknown-unknown` |
| `plugin-signer`（Gamer 仓库 `tools/plugin-signer`） | 打包/校验 `.gplugin`（`pack`/`inspect`/`verify`，免签名） | `cargo build --release --manifest-path tools/plugin-signer/Cargo.toml` |
| 运行中的 Gamer 服务端（8443） | inspect / install / call | `GAMER_ADMIN_PASSWORD=...` 环境变量（开发模式口令）或 `[auth].password_hash` |

不需要：签名密钥、GitHub 仓库、市场登记、修改 Gamer 源码。

一个插件 = 一个 `.gplugin`（zip）= `manifest.toml` + `plugin.wasm`
（WASM Component，真实 guest 字节）+ 可选 UI 资产。

## 2. 五分钟走通最小插件

```sh
# 1) 复制模板（仓库内或复制到任意位置皆可）
cp -r sdk/examples/echo-minimal my-plugin && cd my-plugin
#    改 Cargo.toml 的 [package].name、manifest.toml 的 id/version/name

# 2) 构建（或直接 pwsh ./build.ps1）
cargo build --release --lib --target wasm32-unknown-unknown
cargo run --release --bin componentize -- \
  target/wasm32-unknown-unknown/release/my_plugin_guest.wasm \
  target/plugin.component.wasm

# 3) 打包 + 自检（SIGNER 指向 tools/plugin-signer/target/release/gamer-plugin-signer.exe）
$SIGNER inspect --manifest manifest.toml          # id=com.example.xxx version=… kind=wasm
$SIGNER pack --manifest manifest.toml --wasm target/plugin.component.wasm \
  --out dist/my-plugin-1.0.0.gplugin              # 输出 sha256=/size=
$SIGNER verify --archive dist/my-plugin-1.0.0.gplugin

# 4) 安装（安装即自动 enable→start）
BASE=http://127.0.0.1:8443
curl -s -c cookies.txt -X POST "$BASE/api/login" -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"<GAMER_ADMIN_PASSWORD>"}'
curl -s -b cookies.txt -X POST "$BASE/api/extensions" \
  -H 'Content-Type: application/octet-stream' \
  -H 'x-gamer-permission-confirm: true' \
  --data-binary @dist/my-plugin-1.0.0.gplugin

# 5) 调用（action 必须是 manifest declarative 按钮集合里的名字）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/<插件id>/call" \
  -H 'Content-Type: application/json' -d '{"action":"echo","values":{"k":"v"}}'
```

安装成功的响应里 `"state":"running"`；浏览器右侧插件面板出现你的
declarative 面板。安装是幂等受控的：同版本重复安装 409（`插件 … 已安装`）。

## 3. guest 怎么写（WIT 契约）

guest 面向 `gamer:host@1.0.0` 的 `world extension-host`（契约快照在
`sdk/examples/*/wit/gamer/host.wit`，权威原件在 `server/wit/gamer/host.wit`）：

- **导入**（按需使用，每个都是受控 + 逐调用鉴权的 Host API 域）：
  `device` / `vision` / `input` / `touch` / `resources` / `run` / `runtime` /
  `log` / `context`。
- **导出**（world 强制要求两者都有）：

```haskell
interface extension {
  run: func();                      // 实例化后调用一次；declarative 插件通常立即返回
  call: func(action: string, values-json: string) -> result<string, string>;
}                                   // call 的成功/失败返回值都必须是 JSON 字符串
```

Rust guest 骨架（与 `sdk/examples/hello/src/lib.rs` 同构）：

```rust
wit_bindgen::generate!({ path: "wit/gamer", world: "extension-host" });
use exports::gamer::host::extension::Guest;
use gamer::host::{context, log, resources};

struct MyPlugin;

impl Guest for MyPlugin {
    fn run() { /* 可选初始化；返回后实例停命令循环等 call */ }

    fn call(action: String, values_json: String) -> Result<String, String> {
        let values = serde_json::from_str::<serde_json::Value>(&values_json)
            .unwrap_or(serde_json::Value::Null);
        match action.as_str() {
            "echo" => Ok(serde_json::json!({ "ok": true, "echo": values }).to_string()),
            "check" => {
                let ctx = context::get();                       // 运行上下文（无需权限）
                let pkg = ctx.content_package;                  // Package 数据上下文（可缺省）
                // log.write 需要 manifest 声明 "log.write"
                let _ = log::write("info", "hello", None, None);
                Ok(serde_json::json!({ "ok": true, "package": pkg }).to_string())
            }
            other => Err(serde_json::json!({ "ok": false, "error": "unknown_action" }).to_string()),
        }
    }
}

export!(MyPlugin);
```

生命周期模型：`start` 时宿主实例化组件并调用一次 `run`；`run` 返回后实例
驻留（专用线程 + 命令循环），每个 `POST /api/extensions/:id/call` 派发一次
`call(action, values-json)`；`stop` 优雅收尾实例。**调用模型是同步
canonical ABI**，`log::write` 这类 import 直接调用即可。

## 4. manifest v2 全字段参考

文件名必须是 `manifest.toml`（zip 首个条目由打包器写入）。未知字段一律拒绝
（`deny_unknown_fields`），写错键名安装即报错。

| 字段 | 类型 | 必填 | 说明 |
| --- | --- | --- | --- |
| `manifest_version` | u32 | ✅ | 必须 `= 2`（v1 安装/更新已拒绝；存量读端容忍） |
| `id` | string | ✅ | 插件 id：ASCII 字母数字 + `.``_``-`，不以 `.` 开头/结尾，≤128B；建议反域名（`com.example.myplugin`）。**禁用大写** |
| `version` | string | ✅ | SemVer，禁 build metadata（`+`），如 `1.0.0` |
| `name` | string | ✅ | 显示名，非空 ≤256B、无控制字符 |
| `description` | string | — | 可选说明 |
| `entry` | string | wasm ✅ | 包内 `.wasm` 路径（惯例 `plugin.wasm`）；安装时校验存在 + `\0asm` magic。**builtin 执行类型必须缺省** |
| `permissions` | [string] | — | 权限闭集 19 项（见 §5）；缺省 = 无权限。写 `filesystem.*`/`network`/`shell`/`process`/`device.shell` 直接拒绝 |
| `[host_api]` | table | — | 声明用到的 Host API 域版本要求：`device`/`vision`/`input`/`touch`/`resource`/`run`/`runtime`/`log`/`media`，值是 SemVer range（如 `"^1.0"`）；宿主当前全域 `1.0.0`，不满足 → 安装期结构化报错 |
| `[targets.android].packages` | [string] | — | 支持的 Android 应用包名列表；`*` = 通用（全部应用），**缺省/空声明等价 `*`**。仅作运行目标声明，宿主不做硬门禁：Console 壳按当前设备应用过滤插件入口（`*` 恒显示，具体包名需命中）。与 package.toml 的 `[targets.android]` 同形 |
| `[[ui.contributions]]` | array | — | 面板贡献，见下 |

`[execution]`（执行类型 = 后端形态；**与 `ui.contributions.runtime` 界面渲染类型是两回事**）：

| 字段 | 说明 |
| --- | --- |
| `kind` | `"wasm"`（缺省）：独立插件，携带真实 guest，`entry` 必填；`"builtin"`：宿主预置实现，`entry` 必须缺省、`builtin_id` 必填且必须在服务端注册表中已注册（第三方装不进去，未注册报 `host_feature_unavailable`） |
| `builtin_id` | 仅 kind=builtin |
| `host_version` | advisory 宿主版本要求（如 `">=0.6.0"`），仅展示，不做硬门禁 |

`[[ui.contributions]]`（第三方实用的是 `declarative` 与 `iframe`；`location` 当前仅支持 `"console.right"`）：

| 字段 | 适用 runtime | 说明 |
| --- | --- | --- |
| `panel_id` | 全部 | 面板 id（安全路径段） |
| `title` | 全部 | 面板标题（非空 ≤256B） |
| `icon` / `order` / `preferred_width` | 全部 | 图标文本 / 排序 i32 / 面板宽度 200..=800 |
| `requires_device` | 全部 | 布尔 |
| `runtime` | — | `declarative` \| `iframe` \| `core`。**core = 宿主预置 Vue 组件**（component 键由前端 core-component-registry 解释），第三方没有可挂载的宿主组件，勿用 |
| `entry` | iframe | 必须 `ui/` 下（如 `ui/index.html`），文件随包携带 |
| `component` | core | 宿主组件键；declarative/iframe 带 component 视为拼写错误 |

declarative 表单 schema（宿主原生渲染，按钮值经 `plugin.call` 发给你的 guest）：

```toml
[[ui.contributions]]
panel_id = "main"
title = "我的面板"
runtime = "declarative"
description = "可选面板说明"
[[ui.contributions.fields]]
type = "text"            # text | number | boolean | select | button
name = "message"         # 值键（提交给 guest 的字段名）；button 无 name
label = "消息"
placeholder = "可选"
default = "hello"        # 必须与控件类型匹配
[[ui.contributions.fields.options]]   # 仅 select
value = "fast"
label = "快速"
[[ui.contributions.fields]]
type = "button"
label = "执行"
action = "run"           # call(action, …) 的 action；宿主强制白名单
```

规则：text/number/boolean/select 必须有 `name`；全部必须有非空 `label`；
button 必须有 `action` 且禁 `name/default/options`；select 必须非空
`options`（值不重复）且 `default` 必须在 options 内。

## 5. 权限闭集（默认拒绝、显式申请）

权限是**封闭枚举**（`server/src/extensions/permissions.rs`），不在闭集内的
名字一律拒绝；`filesystem`/`network`/`shell`/`process`/`device.shell` 是
**显式禁区**（声明即报「默认拒绝且不可授予」）。未声明权限的调用在宿主
capability 边界收到 `kind=denied`。

| 权限 | 守护的 Host API | 状态 |
| --- | --- | --- |
| `device.read` | `device.resolve` | 稳定（host API 1.0.0） |
| `device.app` | `device.start-app` / `stop-app` | 稳定 |
| `vision.match` | `vision.match-template` / `vision.capture` | 稳定 |
| `vision.color` | `vision.sample-color` | 稳定 |
| `input.tap` | `input.tap` | 稳定 |
| `input.swipe` | `input.swipe` | 稳定 |
| `input.key` | `input.key` | 稳定 |
| `input.text` | `input.text` | 稳定 |
| `touch` | `touch.begin/move/end` | 稳定 |
| `resource.read` | `resources.resolve` / `open` | 稳定（当前只暴露元数据级读，见 §6） |
| `run.submit` | `run.submit` | 稳定 |
| `run.control` | `run.cancel` / `status` | 稳定 |
| `runtime.sleep` | `runtime.sleep`（上限 1h/次，随取消位中断） | 稳定 |
| `log.write` | `log.write`（级别：trace/debug/info/warn/error） | 稳定 |
| `media.read` | media 域查询 | 实验（WIT media interface 已契约化、未挂 world——guest 尚不可消费） |
| `media.import` | media 导入 | 实验（同上） |
| `media.record` | 录制 start/stop/cancel | 实验（同上） |
| `media.write` | media 写侧 | 实验（同上） |
| `media.events.read` | 录制事件读取 | 实验（同上） |

安装/更新时的权限**增量**必须经 `x-gamer-permission-confirm: true` 请求头
确认，否则安装返回 409「插件权限变更需要用户确认: 新增权限: …」。

## 6. Package 私有数据

插件的数据目录是 `data/packages/<package-id>/plugins/<你的插件 id>/`
（三元组 `(package_id, plugin_id, path)`）。**plugin 维度由宿主注入调用方
身份**——guest 在 WIT 里只能传 `(namespace=package id, name=相对路径)`，
永远落在自己的前缀内，无法读写其他插件目录（越权寻址在 `ResourceId`
构造层就无效）。

当前边界（host API 1.0.0 实况，写插件前务必知道）：

- guest 侧 WIT `resources` 域只有 `resolve(namespace, name) -> handle`
  （文件必须存在，否则 `not-found`）与 `open(handle) -> byte-len`——
  **没有字节内容读取，也没有写入**。
- 字节级读写走 **Package 资源 REST**（面向操作者/前端）：
  `GET|PUT|DELETE /api/packages/:pkg/plugins/<你的插件id>/resources/*path`
  （文本为 JSON `{content, expected_version?, force?}`，乐观并发；未注册
  ResourceHandler 的第三方插件 = 裸 Core 语义，内容不做业务校验）。
- `resources.resolve` + `open` 的组合可验证「我的私有文件是否存在、多大」
  （`hello` 示例的 `check_resource` 动作即此用法）；`context.get()` 的
  `content_package` 就是可用的 namespace。
- 目录内部结构归插件自己解释；插件卸载**不删除**其 Package 数据。

## 7. 更新与版本语义

- 已安装版本目录不可变；`update` = 装新版本 + 切 `active_version`，
  **Running 状态拒绝更新**（先 `POST /api/extensions/:id/stop`）。
- `POST /api/extensions/:id/activate` `{version}` 切换激活版本（不复制不删除）。
- 卸载：`DELETE /api/extensions/:id/:version`；Running 拒绝（409）；
  删掉最后一个版本才清状态记录。非激活的旧版本可直接删。
- 本地导入不会自动获得更新来源；来源与可更新来源是两回事。

## 8. 调试与常见错误对照表

调试顺序建议：`signer inspect/verify`（包级）→ `POST /api/extensions/inspect`
（安装预览）→ 安装 → `GET /api/extensions`（state/last_error）→
`POST .../call`（guest 逐动作）。

| 现象 / 错误文案（服务端原文） | 根因 | 处置 |
| --- | --- | --- |
| `插件 manifest 无效: manifest_version=1 已不受支持…` | 旧 v1 manifest | 升级到 `manifest_version = 2` |
| `插件 manifest 无效: manifest_version=3 不受支持…（需要升级 Gamer 宿主）` | manifest 比宿主新 | 用宿主支持的版本号 |
| `插件 manifest 无效: wasm 执行类型需要 entry…` / `归档缺少 entry plugin.wasm` / `entry 不是 WASM 二进制` | entry 缺失/改名/不是真实组件 | manifest `entry` 与实际文件一致；用 `signer pack --wasm` 打包 |
| `插件权限错误: 插件权限默认拒绝且不可授予: filesystem.*` 等 | 声明了禁区权限 | 删掉；插件没有任意文件系统/网络/shell 通道（见 §9） |
| `插件权限错误: 未知插件权限: xxx` | 权限名拼错 | 对照 §5 的 19 项闭集 |
| `插件权限变更需要用户确认: 新增权限: …`（409） | 权限增量未确认 | 安装请求加 `x-gamer-permission-confirm: true` |
| `宿主归档 sha256 校验失败: 期望 … 实际 …` | `x-expected-sha256` 钉住的哈希不符 | 用 `signer pack` 输出的实际 sha256 |
| `宿主 API 不兼容`（`unsupported_host_api`：required/supported） | `[host_api]` 版本要求高于宿主（宿主全域 1.0.0） | 放宽为 `"^1.0"` 或升级宿主 |
| `host_feature_unavailable` | manifest 声明 `kind="builtin"` 但 `builtin_id` 不在服务端注册表 | 第三方插件用 `kind="wasm"`；builtin 是宿主预置实现专用，不可伪装 |
| `插件 … 已安装`（409） | 同 id 同版本重复安装 | bump version（更新语义见 §7） |
| `插件调用被拒绝: action 不在插件 declarative UI 声明的按钮集合内: …`（400） | call 的 action 没在 manifest 声明 | 按钮动作全部写进 `[[ui.contributions.fields]]`（type=button） |
| `插件 … 的生命周期不允许执行 call/uninstall: 当前状态为 …`（409） | 插件不在 Running | call 需 Running；卸载需先 stop |
| `插件 call 返回值不是 JSON` / `插件 call trap: …` | guest 返回值不是 JSON 字符串 / guest 执行 trap | 成功与失败分支都返回 JSON；查 guest 日志与服务端日志 |
| guest 侧 `denied`（`HostError.kind`） | 调用未在 manifest 声明权限 | 补声明 + 重装（新权限需确认） |
| guest 侧 `not-found` | 设备 id 不存在 / 资源不存在 | 核对 `context.get()` 与设备列表；资源先经 REST 写入 |
| `组件实例化失败: …`（start 失败，列表 `last_error` 可见） | guest 与 world 契约不匹配（如缺 `call` 导出、WIT 版本不符） | 用随示例的 wit 快照与 wit-bindgen 版本重编 |
| **`store configuration requires that *_async functions are used instead` + 宿主进程 abort** | 当前基线宿主缺陷（§开头警告）：任何 import 型插件 start 即触发 | 待宿主修复；修复前 guest 无需改动，先不要安装 import 型插件 |

结构化诊断是产品设计：所有安装/调用失败都返回带语义的中文错误（HTTP 4xx
为主），不是笼统的「插件运行失败」。guest 的 `log::write` 会落进 Core 日志
（`GET /api/logs` / 日志面板），是 guest 内排障的第一入口。

## 9. 能力边界（哪些永远不会开放给普通插件）

普通插件**只能**通过上述受控 Host API 工作。以下能力不开放，声明即拒绝、
运行时也无通道：任意文件系统读写、网络访问、shell/进程执行、设备 shell、
动态加载原生代码（DLL/so）、读取其他插件的私有资源目录、伪造调用方身份
（plugin 维度由宿主注入，guest 无法指定）。需要新能力时，走 Core 的版本化
Host API 演进（新域/新函数 + 权限闭集扩充），而不是给单个插件开口子。

## 10. 分享与分发

`.gplugin` 是自包含的（manifest + guest + UI 资产），直接把文件发给其他
用户即可在插件中心本地导入使用；不需要密钥、仓库或市场。若要发布到官方
市场（`web/public/registry.json` 体系），使用 `tools/build-plugins.ps1` 的
registry v2 流程（sha256 完整性清单；哈希只证明文件一致，不代表发布者身份）。
