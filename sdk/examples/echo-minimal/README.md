# echo-minimal — 零 import 最小可运行插件

三个示例里唯一**零 import、零权限**的一个，也是当前服务端基线上可以
完整走通「安装 → 启动 → 调用 → 更新 → 卸载」全生命周期的最小模板。
它证明写一个 Gamer 插件的最小成本：一个 manifest v2 + 一个导出
`run`/`call` 的 WASM Component（本例 guest 源码 60 余行）。

适用形态：纯 declarative 面板 + guest 内计算的轻量工具（表单校验、
文本处理、配置生成、格式转换等）。需要调用设备/视觉/输入/资源/日志等
Host API 时，改用 `hello` / `vision-probe` 的写法（声明权限 + host_api 域，
guest 内 `use gamer::host::...`）。

## 构建

```sh
rustup target add wasm32-unknown-unknown   # 首次
pwsh ./build.ps1
# 产物：dist/com.example.echo-1.0.0.gplugin
```

原始命令（跨 shell / 仓库外同样适用，`$SIGNER` =
Gamer 仓库 `tools/plugin-signer/target/release/gamer-plugin-signer.exe`）：

```sh
cargo build --release --lib --target wasm32-unknown-unknown
cargo run --release --bin componentize -- \
  target/wasm32-unknown-unknown/release/echo_minimal_plugin_guest.wasm \
  target/plugin.component.wasm
$SIGNER inspect --manifest manifest.toml
$SIGNER pack --manifest manifest.toml --wasm target/plugin.component.wasm \
  --out dist/com.example.echo-1.0.0.gplugin
$SIGNER verify --archive dist/com.example.echo-1.0.0.gplugin
```

## 安装与调用（curl）

```sh
BASE=http://127.0.0.1:8443
curl -s -c cookies.txt -X POST "$BASE/api/login" \
  -H 'Content-Type: application/json' \
  -d '{"username":"admin","password":"<你的 GAMER_ADMIN_PASSWORD>"}'

# 安装（无权限增量 → 不需要确认头；安装即自动 enable→start）
curl -s -b cookies.txt -X POST "$BASE/api/extensions" \
  -H 'Content-Type: application/octet-stream' \
  --data-binary @dist/com.example.echo-1.0.0.gplugin

# 调用公开 command（action 必须是 manifest 按钮集合里的名字）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.echo/call" \
  -H 'Content-Type: application/json' \
  -d '{"action":"sum","values":{"a":19,"b":55}}'
# → {"action":"sum","ok":true,"sum":74.0}

# 未声明的 action 被宿主拒绝（declarative 按钮白名单）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.echo/call" \
  -H 'Content-Type: application/json' -d '{"action":"evil","values":{}}'
# → 400 插件调用被拒绝

# 卸载（Running 先 stop）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.echo/stop"
curl -s -b cookies.txt -X DELETE "$BASE/api/extensions/com.example.echo/1.0.0"
```

浏览器侧：右侧插件面板出现「Echo 示例」（declarative 表单：回显 / 求和）。
