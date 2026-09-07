# hello — 最小 Gamer 第三方插件示例

> ⚠️ **当前宿主基线限制（2026-09-07）**：服务端通用 extension-host 运行时
> 以 async 链接 import、以同步入口调用 guest，任何**声明了 import** 的插件
> （即任何调用 Host API 的插件）在 start 时会触发 trap 并使宿主进程 abort，
> 重启后 reconcile 还会再次 start 形成**启动循环崩溃**，需手工清数据目录。
> 这是宿主侧缺陷（guest 侧契约用法正确，宿主修复后本示例无需改动即可运行），
> 详情与复现记录见 `docs/evidence/phase3_sdk_examples.md`。在该缺陷修复前，
> 请不要在长期运行的服务上安装本示例；完整可运行的零 import 模板见
> `../echo-minimal`。

一个能真实安装、运行、调用的完整插件，全部源码只有三个你关心的文件：
`manifest.toml`（插件清单）+ `src/lib.rs`（WASM guest，约 180 行）+
`src/bin/componentize.rs`（打包后处理，可直接照抄）。另有一份随示例走的
WIT 契约快照 `wit/gamer/host.wit`（`gamer:host@1.0.0`）。

它演示了第三方插件的全部基础机制：

| 机制 | 在示例里的位置 |
| --- | --- |
| manifest v2（`[execution] kind="wasm"` + 真实 entry） | `manifest.toml` |
| declarative UI 面板（fields + button action） | `manifest.toml` 的 `[[ui.contributions]]` |
| guest 公开 command（`call(action, values-json)` 导出） | `src/lib.rs` 的 `impl Guest` |
| 读取运行上下文（Device/App/Package 三层） | `run()` 里的 `context::get()` |
| 写插件日志（log 能力） | `write_log()` → `log::write` |
| Package 私有数据定位（resource 能力 + 插件数据隔离） | `check_resource()` |
| 权限闭集负向自检（未声明权限被宿主拒绝） | `probe_denied()` |

## 前置工具链

```sh
rustup target add wasm32-unknown-unknown   # guest 目标
```

打包/校验直接复用 Gamer 仓库自带的 `tools/plugin-signer`（免签名，不需要任何密钥）。

## 构建（build → pack → verify）

从 Gamer 仓库检出内（自动定位 signer）：

```sh
pwsh ./build.ps1
# 产物：dist/com.example.hello-1.0.0.gplugin
```

等价的原始命令（跨 shell / 仓库外复制本目录后适用，`$SIGNER` 指向从
Gamer 仓库构建出的 `gamer-plugin-signer.exe`，见 `tools/plugin-signer`）：

```sh
# 1. guest core module（wasm32）
cargo build --release --lib --target wasm32-unknown-unknown
# 2. 包成 WASM Component
cargo run --release --bin componentize -- \
  target/wasm32-unknown-unknown/release/hello_plugin_guest.wasm \
  target/plugin.component.wasm
# 3. 预览 manifest 元数据（id/version/kind）
$SIGNER inspect --manifest manifest.toml
# 4. 打包 .gplugin（manifest.toml + plugin.wasm，无签名）
$SIGNER pack --manifest manifest.toml --wasm target/plugin.component.wasm \
  --out dist/com.example.hello-1.0.0.gplugin
# 5. 产物自检（zip 结构 + manifest + \0asm magic）
$SIGNER verify --archive dist/com.example.hello-1.0.0.gplugin
```

## 安装并调用（curl）

服务端开发模式需先设置 `GAMER_ADMIN_PASSWORD` 环境变量再启动，登录拿会话 cookie：

```sh
BASE=http://127.0.0.1:8443
curl -s -c cookies.txt -X POST "$BASE/api/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"<你的 GAMER_ADMIN_PASSWORD>"}'

# inspect 预览（安装前确认执行形态/权限/宿主要求）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/inspect" \
  -H 'Content-Type: application/octet-stream' \
  --data-binary @dist/com.example.hello-1.0.0.gplugin

# 安装（首次装带权限增量，需要 x-gamer-permission-confirm；安装即自动 enable→start）
curl -s -b cookies.txt -X POST "$BASE/api/extensions" \
  -H 'Content-Type: application/octet-stream' \
  -H 'x-gamer-permission-confirm: true' \
  --data-binary @dist/com.example.hello-1.0.0.gplugin

# 停止 → 带 app_context 重启（把数据上下文指向一个 Package）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.hello/stop"
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.hello/start" \
  -H 'Content-Type: application/json' \
  -d '{"app_context":{"device_id":"selftest","android_package":"com.example.app","content_package":"hello-data"}}'

# 调用公开 command（action 必须是 manifest declarative 按钮集合里的名字）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.hello/call" \
  -H 'Content-Type: application/json' \
  -d '{"action":"greet","values":{"message":"你好","level":"info"}}'
# → {"ok":true,"action":"greet","echo":"你好","logged_at_level":"info"}

# Package 私有数据：先经 Package 资源 REST 写入，再让插件读自己前缀
curl -s -b cookies.txt -X POST "$BASE/api/packages" \
  -H 'Content-Type: application/json' -d '{"id":"hello-data","name":"Hello 数据"}'
curl -s -b cookies.txt -X PUT \
  "$BASE/api/packages/hello-data/plugins/com.example.hello/resources/data/state.json" \
  -H 'Content-Type: application/json' \
  -d '{"content":"{\"counter\":1}"}'
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.hello/call" \
  -H 'Content-Type: application/json' -d '{"action":"check_resource","values":{}}'
# → {"ok":true,"found":true,"bytes":...}；删掉该资源后再调 → found:false

# 权限闭集负向自检（未声明 device.read，宿主必须拒绝）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.hello/call" \
  -H 'Content-Type: application/json' -d '{"action":"probe_denied","values":{}}'
# → {"ok":true,"denied":true,"kind":"denied",...}

# 插件日志可在 Core 日志面板 / GET /api/logs 看到（source 为插件日志）
```

卸载（Running 先 stop；插件 Package 私有数据默认保留）：

```sh
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.hello/stop"
curl -s -b cookies.txt -X DELETE "$BASE/api/extensions/com.example.hello/1.0.0"
```

## 浏览器侧

登录 Web 控制台后，右侧插件面板会出现「Hello 示例」（declarative 面板，
两个按钮分别对应 `greet` 与 `check_resource`/`probe_denied`）。

## 复制起步

把整个目录复制到任意位置（仓库外）即可作为你自己插件的起点：
改 `manifest.toml` 的 `id`/`version`/权限、改 `src/lib.rs` 的 call 分支、
按上面原始命令构建。不需要 Gamer 源码树的任何其他部分。
