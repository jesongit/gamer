# Phase 1+2（B1 服务端）：免签名安装 + 执行类型/宿主注册表

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §4 / §5（合并执行，§13.1）
> 日期：2026-09-07
> 范围：**服务端 + 三份 tools/plugins manifest + server 测试**。前端（B3）与打包/市场产物（B2）由并行 Agent 承担；本报告给出一者依赖的接口变化点。

## 1. 完成项

### Phase 1（免签名）
- **官方来源强制签名/proof 全链删除**：`ExtensionService::inspect_with_context` 的三分支（`RegistryProofRequired` / official 必须签名 Valid / `verify_registry_proof`）整体移除；official 与本地安装统一无签名，`x-gamer-extension-source: official` 只作来源标注。
- **`server/src/extensions/signature.rs` 整体删除**：`SignatureVerifier` / `TrustStore` / `RegistryProof` / `SignatureInfo` / `SignatureStatus`、内嵌 dev 公钥信任锚、`verify_installed`、`signature.sig` 解析全部退役；`x-gamer-registry-proof` 头不再解析。**已安装包内遗留的 `signature.sig` 是死文件，静默忽略**（state.json 无签名字段，无迁移）。
- **结构清理**：`ExtensionSnapshot` / `ExtensionInspection` 的 `signature` 字段、`snapshot_from_versions` 的每版本重验（原先 list() 每次调用都重读重验签名）全部移除；API 层 `snapshot_json` / management / inspect 响应的 `"signature"` JSON 字段删除。
- **保留**：manifest 校验、版本管理、Host API 权限门禁、ZIP 目录/大小/解压限制、原子提交、生命周期状态机、`x-gamer-permission-confirm` 权限确认、浏览器侧下载 sha256 校验。
- **新增可选完整性钉 `x-expected-sha256`**（inspect/install/update 三入口共用 `install_context`，与包级导入 `X-Expected-Sha256` 同名同义）：64-hex 校验，头非法或与归档摘要不符 → 400 `ArchiveSha256Mismatch`；缺省不阻断本地安装。
- **更新流程**（`POST /api/extensions/:id/update`）同样无签名，走同一 `inspect_with_context`；哈希钉随上下文生效。

### Phase 2（执行类型 / 宿主注册表）
- **manifest v2**（`extensions/manifest.rs`）：`MANIFEST_VERSION = 2`；新增可选 `[execution]` 表：`kind = "wasm" | "builtin"`（缺省 wasm）、`builtin_id`（builtin 必填，id 语法校验）、`host_version`（可选 advisory 字符串，inspect/snapshot 原样透传，服务端不做硬门禁）。
  - kind=wasm：`entry` 必填（`.wasm` 后缀、禁指 manifest.toml），不得声明 `builtin_id`。
  - kind=builtin：`entry` 必须缺省，`builtin_id` 必填。
  - **v1 只在安装/更新/inspect 的严格解析中拒绝**（提示升级包到 v2；未来版本提示升级宿主）；**读端容忍**：`parse_manifest_installed` 让 `store.rs::list_installed` 接受存量 v1 安装（等价 wasm 执行类型），快照/启动路径不因 manifest_version 重验而崩。
- **BuiltinExtensionDescriptor 注册表**（新模块 `extensions/builtin.rs`）：静态 const 注册表（`BUILTIN_EXTENSIONS`），记录 id/显示名/描述/advisory 最低宿主版本；**gamer.video 是首个注册项**；无任何注册 API，下载包不可扩展。
  - `service.rs::instance_free` 对 video 的 `is_native_extension` 字符串特判收敛为 `builtin::is_builtin_extension`（gamer.yaml 的按调用执行仍由组合根 registrar 声明，不变）。
  - 安装/更新/inspect 时（`inspect_compatible`）校验 builtin_id 已注册，未注册 → 结构化错误 **`host_feature_unavailable`**（409，Display 前缀即错误码，提示升级宿主）。
- **entry 假设按执行类型分支**：
  - `archive.rs::inspect_archive`：wasm 维持全部检查（存在/非目录/≥4B/`\0asm` magic——不允许借 builtin 绕过 guest 校验）；builtin 要求包内**不得携带 `plugin.wasm`**（发现即拒绝，防伪装）。
  - `store.rs::list_installed`：仅 wasm 做 entry 存在性检查；`InstalledExtension::wasm_path()` 变 `Option`，builtin 的 `read_wasm()` 返回结构化错误（组合根 instance_free 语义保证不会触达）。
- **host_api 第 9 域 `media`**：`RawHostApiRequirements` 补 `media` 键（catalog/权限闭集已有 Media 域，此前 manifest 无声明入口——Phase 0 偏差 5 收口）。
- **UI 贡献仅 Running 可见**（`refresh_ui_registry` 语义收紧，Enabled|Running → 仅 Running）：stop/disable/uninstall 均撤销面板与 ui 资产（`read_ui_file` 同步要求可见）；Enabled 不再出现面板、不再服务 ui 资产。相关测试断言全部翻转（video/keymap/yaml/extensions api/guard）。

### 版本冻结与 manifest 落点（B2 依赖）
| 插件 | tools/plugins/<id>/manifest.toml | 内嵌常量（逐字同步锁） |
| --- | --- | --- |
| gamer.video | `manifest_version=2`、`version="1.0.0"`、`[execution] kind="builtin" builtin_id="gamer.video"`、**无 entry**、permissions 与 `[[ui.contributions]]`（core/VideoWorkbench）不变 | `video/mod.rs::VIDEO_EXTENSION_MANIFEST_TOML` |
| gamer.yaml | `manifest_version=2`、`version="3.1.1"`、`[execution] kind="wasm"`（entry 不变） | `gamer_yaml/yaml_extension.rs::YAML_EXTENSION_MANIFEST_TOML` |
| gamer.keymap | `manifest_version=2`、`version="1.0.1"`、`[execution] kind="wasm"`（entry 不变） | `keymap/mod.rs::KEYMAP_EXTENSION_MANIFEST_TOML` |

guard 测试的 `yaml_market_version()` 从常量现场解析版本（未硬编码），升版天然兼容。三份常量均有 include_str! 逐字同步锁测试，全部通过。

## 2. 修改文件清单

新增：`server/src/extensions/builtin.rs`；删除：`server/src/extensions/signature.rs`。

修改（server/**，均在授权范围内）：
- `extensions/manifest.rs`、`error.rs`、`archive.rs`、`store.rs`、`service.rs`、`mod.rs`、`ui.rs`（fixture v2）、`wasm.rs`（fixture v2）
- `extensions/video/mod.rs`、`extensions/keymap/mod.rs`、`extensions/gamer_yaml/yaml_extension.rs`（manifest 常量 + 测试翻转 + fixture v2）
- `api/extensions.rs`、`api/extensions_management.rs`、`api/tests/extensions.rs`
- `architecture_guard_tests.rs`（生命周期 UI 断言翻转；源码边界白名单无需新条目——builtin.rs/signature.rs 不涉及 YAML/Keymap 禁符白名单）
- `tools/plugins/gamer.video|gamer.yaml|gamer.keymap/manifest.toml`

文档：本报告 + `docs/PITFALLS.md` 追加一条（MSYS 参数转换坑）。

## 3. 接口变化点（给 B3 前端）

**inspect 响应**（`POST /api/extensions/inspect`）：
- 删除 `signature` 字段。
- 新增 `execution`：`{"kind":"wasm"}` 或 `{"kind":"builtin","builtin_id":"gamer.video","host_version":">=0.1.1"}`（builtin_id/host_version 缺省时省略键）。
- 其余字段不变（id/version/name/description/archive_sha256/source/publisher/permissions/permission_diff/host_api/ui/already_installed）。

**snapshot JSON**（`GET /api/extensions` 列表、install/update/enable/start/stop/disable 响应）：
- 删除 `signature`；新增 `execution`（同上形态）；`entry` 对 builtin 包为 `null`（此前恒为字符串）。

**management 响应**（`GET /api/extensions/management`）：同样删 `signature`、增 `execution`。

**请求头**：安装/inspect/update 新增可选 `x-expected-sha256`（64-hex；非法或与归档不符 → 400）；`x-gamer-registry-proof` 不再解析（发了也被忽略）；`x-gamer-extension-source: official` 保留但只影响响应 `source` 标注，**不再触发任何门禁**——前端 installPolicy 的「官方必须 valid 签名」阻断分支可以整体退役。

**错误形态**：builtin_id 未注册 → 409，message 以 `host_feature_unavailable:` 开头（「需要升级 Gamer 宿主」）；manifest v1 包安装 → 400 提示升级到 manifest_version=2。

**行为变化（UI 可见性）**：`GET /api/extensions/ui` 仅在插件 Running 时返回其贡献；stop 后面板立即消失（旧语义 stop→Enabled 面板保留）。安装即用降级为 Enabled（桩 wasm start 失败）的插件面板不再出现。

## 4. 测试命令与结果（Windows 内存墙：CARGO_PROFILE_DEV_DEBUG=0 + -j 4，未跑全量）

| 命令（cwd=server/） | 结果 |
| --- | --- |
| `CARGO_PROFILE_DEV_DEBUG=0 cargo check -j 4` | PASS |
| `CARGO_PROFILE_DEV_DEBUG=0 cargo check -j 4 --tests` | PASS |
| `CARGO_PROFILE_DEV_DEBUG=0 cargo check -j 4 --no-default-features` | PASS（无 WASM 退出路径） |
| `CARGO_PROFILE_DEV_DEBUG=0 cargo test -j 4 extensions -- --test-threads=2` | **177 passed, 0 failed**（含 api extensions REST、video/builtin/manifest/service/yaml/keymap 单测、官方市场端到端） |
| `… cargo test -j 4 -- --test-threads=2 architecture_guard` | **7 passed**（含翻转后的全链生命周期） |
| `… cargo test -j 4 -- --test-threads=2 manifest` | 25 passed |
| `… cargo test -j 4 -- --test-threads=2 dormant plugin_state` | 3 passed（keymap fixture 包 dormant 数据回归） |
| `… cargo test -j 4 -- --test-threads=2 keymap` | 22 passed |
| `… cargo test -j 4 -- --test-threads=2 video` / `builtin` / `extensions::service extensions::ui` | 11 / 9 / 5 passed |
| `rustfmt`（仅本任务改动文件） | 已格式化；`cargo fmt --all -- --check` 在 api/devices.rs 等非本任务文件仍有存量偏差，留给集成者 |

说明：官方市场端到端测试（`official_plugin_market_end_to_end_with_committed_artifacts`）已改为免签名语义并直接验收 B2 的在库产物（registry schema_version=2 + `gamer.video-1.0.0.gplugin` 等）：哈希核对 → official + `x-expected-sha256` 无 proof 安装 → video builtin 包安装即 Running → Enabled 的 yaml 不出现面板 → 全量卸载，**PASS**。

## 5. 设计偏差与裁决记录

1. **`host_feature_unavailable` 映射 409**（sha256 不匹配映射 400，与包级导入一致）：错误 Display 以错误码为前缀，前端可按前缀识别。
2. **builtin 伪装检测按文件名**：契约只冻结「不得携带 plugin.wasm」，包内其他 `.wasm` 文件不拒（未来若需要可收紧为任意 `*.wasm`）。
3. **v1 存量安装的读端容忍实现为 `parse_manifest_installed`**（v1|v2 双版本），严格 `parse_manifest` 只收 v2——安装/更新立即收紧，已安装旧包不崩（基线 §9-9 的 500 风险消除）。
4. **`host_version` advisory 不做 semver 语法强校验**（仅长度/控制字符清洗），按契约「服务端暂不做硬门禁」；注册表内 gamer.video 记录 `">=0.1.1"`（当前宿主版本 0.1.1）。
5. **`server/Cargo.toml` 未删除 `ed25519-dalek`/`base64` 依赖**（signature.rs 删除后已无人使用）：删除需同步根 `Cargo.lock`（在 server/** 之外），为避免与并行 Agent 的锁文件冲突留给集成者一次性处理。
6. **clippy/fmt 现状**：本任务改动文件全部 clippy 干净；`cargo clippy --all-targets -- -D warnings` 仍报 main.rs（重复 `#[cfg(test)]`，他人编辑中）、scheduler.rs、gamer_yaml/*（needless_borrow/clone_on_copy）、api/tests/update.rs——均非本任务文件，未越权抢修，留给集成者在全部 Agent 落地后统一收口。

## 6. 行为变化清单（用户可见）

- 官方/本地插件安装一律无需签名与 proof；市场安装只需来源标注 + 权限确认（+ 可选哈希钉）。
- **面板只在插件 Running 时可见**：stop 即从导航消失，start 恢复；Enabled（如安装即用降级态）不再出现「点了没内容」的半启用面板。
- gamer.video 现在可以作为**真实的 builtin 包**（无 plugin.wasm）安装、start 进入 Running、点亮视频工作台面板；冒充 builtin（携带 plugin.wasm 或未注册 builtin_id）被拒。
- 存量 v1 安装包继续可用（列表/启动/运行）；再安装新版本必须用 v2 包（gamer.yaml 3.1.1 / gamer.keymap 1.0.1 / gamer.video 1.0.0 由 B2 重打）。

## 7. 遗留问题

- 集成者统一处理：根 `Cargo.lock` 依赖清理（见偏差 5）、全仓 fmt/clippy 收口（见偏差 6）、全量 `cargo test` 回归（按并行约束本任务未跑）。
- api/tests/extensions.rs 的桩 wasm 用例借直写 state.json 模拟 Running 验证 UI 可见性（`mark_extension_state` helper）——与 activate 测试既有技术一致；待 Phase 3 有可运行的最小第三方 guest 后可换成真实启动。
- registry v2 的 `execution` 字段前端消费（面板徽章/安装确认展示）归 B3；服务端已透传全部所需数据。
