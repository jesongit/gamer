# vision-probe — 设备/视觉能力示例插件

> ⚠️ **当前宿主基线限制（2026-09-07）**：与 `../hello` 相同——任何声明了
> import 的插件在当前服务端上 start 会触发宿主进程 abort（宿主侧缺陷，
> guest 侧写法即宿主修复后的正确写法，无需改动），修复前请勿在长期运行的
> 服务上安装本示例。详见 `docs/evidence/phase3_sdk_examples.md`。

与 `hello` 同一条 `gamer:host/extension@1.0.0` 契约，差别只在 **manifest 声明**：
申请 `device.read` / `vision.match` / `vision.color` / `input.tap` 四项权限后，
guest 对受控 Host API 的调用才能穿过宿主 capability 边界真实执行。
对照 hello 示例的 `probe_denied`（未声明权限 → `kind=denied`），两边合起来
证明权限闭集真实生效：**能力 = manifest 声明 + 宿主逐调用鉴权**。

guest 源码要点（`src/lib.rs`）：

| 动作 | Host API 调用链 | 需要的权限 |
| --- | --- | --- |
| `probe` | `device.resolve` → `vision.capture` → `vision.sample_color` | device.read → vision.match → vision.color |
| `tap` | `device.resolve` → `input.tap`（默认 dry-run，勾选 `confirm_tap` 才真实注入） | device.read → input.tap |

安全设计：`probe` 全程只读（抓一帧 + 采样一个像素，不注入任何输入）；
`tap` 默认 dry-run 只回显将要执行的动作——示例代码绝不替你点真机。

调用失败按阶段结构化返回（`stage` + `kind`），便于对照排查：

| kind | 含义 |
| --- | --- |
| `denied` | 权限没在 manifest 声明（宿主拒绝） |
| `not-found` | 设备 id 不存在 / 资源不存在 |
| `unavailable` | 能力未注册（宿主侧缺设备能力等） |
| `failed` | 能力执行失败（如设备未连接抓不到帧） |

## 构建

```sh
rustup target add wasm32-unknown-unknown   # 首次
pwsh ./build.ps1
# 产物：dist/com.example.visionprobe-1.0.0.gplugin
```

原始命令与 hello 示例完全同构（把 crate 名换成
`vision_probe_plugin_guest`、包名换成 `com.example.visionprobe`），见
`../hello/README.md`。

## 安装与验证权限真实生效

```sh
BASE=http://127.0.0.1:8443
# 登录（同 hello README）

# inspect：确认 permissions/host_api 与预期一致
curl -s -b cookies.txt -X POST "$BASE/api/extensions/inspect" \
  -H 'Content-Type: application/octet-stream' \
  --data-binary @dist/com.example.visionprobe-1.0.0.gplugin

# 安装（权限增量 → 需要 x-gamer-permission-confirm）
curl -s -b cookies.txt -X POST "$BASE/api/extensions" \
  -H 'Content-Type: application/octet-stream' \
  -H 'x-gamer-permission-confirm: true' \
  --data-binary @dist/com.example.visionprobe-1.0.0.gplugin

# 对不存在的设备调用 probe：错误应来自 device.resolve 阶段且 kind != denied
# （= 通过了权限门禁；若 kind=denied 说明权限声明没有生效）
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.visionprobe/call" \
  -H 'Content-Type: application/json' \
  -d '{"action":"probe","values":{"device_id":"selftest-nonexistent","x":10,"y":10}}'

# 对真实在线设备：probe 返回采样像素 rgb；tap 未勾选 confirm_tap 时只 dry-run
curl -s -b cookies.txt -X POST "$BASE/api/extensions/com.example.visionprobe/call" \
  -H 'Content-Type: application/json' \
  -d '{"action":"tap","values":{"device_id":"<真实设备id>","x":100,"y":100}}'
```

注意：`probe`/`tap` 的坐标是设备物理像素（与投屏分辨率一致），示例不做
归一化——真实插件应按自己拿到的帧尺寸换算。
