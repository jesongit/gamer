# Phase 0 基线证据：插件生态简化与视频工作台收尾

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §3（Phase 0）
> 日期：2026-09-07
> 性质：全部结论基于当前工作树真实代码审计（逐文件核对，未照搬计划假设）；与计划假设不符之处在 §9 单列。

## 1. 仓库基线

| 项 | 值 |
| --- | --- |
| HEAD | `31b0eff61e7771f0a5800a3c23dd5f3b99dd3d43`（feat(web): 主导航「市场/插件」改下拉二级菜单） |
| 分支 | `main` |
| 未提交 | 仅两个未跟踪项：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md`（本计划文件）、`server/data/`（运行数据，按「零业务资源入库」基线属预期，不入库） |
| 已跟踪文件修改 | 无（工作树干净） |
| 编译基线 | `CARGO_PROFILE_DEV_DEBUG=0 cargo check -j 4`（server/）**PASS**，2m11s（2026-09-07 本机实测；内存墙规避见 PITFALLS 2026-09-07 条目） |

模块归属：本次只读审计，未修改任何业务文件；唯一新增即本证据文档。

## 2. 插件安装 / 生命周期真实调用链

### 2.1 生命周期全景（`server/src/extensions/service.rs`）

```
REST（api/extensions.rs / api/extensions_management.rs）
  → ExtensionService（operation_lock 串行化全部状态迁移）
     install(_with_context)   L491-508   inspect → store.install_archive → state.json 落 Installed
     update(_with_context)    L523-557   Running 拒绝；装新版本 + 切 active_version
     activate_version         L562-589   切 active_version 指针（不复制不删除）
     enable                   L591-608   Installed/Disabled/Failed → Enabled（enable ≠ start）
     start(_with_context)     L645-742   Enabled → Running；instance_free 分支不启动实例
     stop                     L744-760   Running → Enabled + 注销 runner
     disable                  L614-636   Running 先 stop；→ Disabled
     uninstall                L809-855   Running 拒绝；删版本目录；剩最后版本时清 state 记录
     reconcile_startup        L768-807   重启对账：遗留 Running 降 Enabled 再走 start 恢复
     guest_for_run            L273-294   按调用执行模型取 guest 字节 + HostApi
     call_extension           L329-375   native_call_action 缝 + declarative 按钮白名单 + 常驻实例 call
```

REST 路由（`api/mod.rs` L393-448）：`GET /api/extensions`、`POST /api/extensions`（安装，`api_install_extension`，安装即自动 enable→start、失败降级 Enabled 不回 201 Failed——`auto_start_installed` L59）、`POST /api/extensions/inspect`、`POST /api/extensions/:id/update|enable|disable|activate|start|stop|call`、`DELETE /api/extensions/:id/:version`、`GET /api/extensions/ui|contributions`、`GET /api/extensions/:id/ui/*path`、`GET /api/extensions/management`。

### 2.2 (a) 官方来源强制签名/proof 的精确位置

全部收敛在 **`ExtensionService::inspect_with_context`（service.rs L387-418）**，inspect / install / update 三个入口共用：

| 行号 | 行为 |
| --- | --- |
| L393-395 | `context.official && context.registry_proof.is_none()` → `ExtensionError::RegistryProofRequired` |
| L396 | `self.signature.verify_archive(archive)`——无签名归档返回 `SignatureInfo::unsigned()`，**本地路径此处从不阻断** |
| L397-401 | `context.official && signature.status != Valid` → `InvalidSignature("官方插件必须带有可验证的 manifest.toml Ed25519 签名")` |
| L402-409 | 有 proof 时 `SignatureVerifier::verify_registry_proof`（signature.rs L290-333：id/version/download_url(https 或同源 `/`)/sha256 四绑定 + Ed25519 验签） |

`official` 标志来源：`api/extensions_management.rs::install_context`（L158-185）解析请求头 `x-gamer-extension-source: official`、`x-gamer-registry-proof`（base64 RegistryProof JSON）、`x-gamer-permission-confirm`。

**本地导入无签名路径**：`context.official=false` 时 L393/L397 两个分支都不触发，verify_archive 对无 signature.sig 的包返回 Unsigned 放行；`install_with_context`（L491-508）只再要求权限增量确认（`ensure_permission_confirmation` L1121-1132）。即**服务端本地免签安装已存在，无需新建**。

签名验签本体：`signature.rs::SignatureVerifier::verify_archive` L219-254（zip 内 `signature.sig`/兼容 `manifest.toml.sig`，magic `gamebot-gplugin-sig-1 <key_id>`，Ed25519 覆盖 manifest.toml 原始字节）；信任锚 = 内置 `BUNDLED_DEV_PUBLIC_KEY_PEM`（L34-38，对应 `tools/plugin-signing/gamer-dev-1.pem`，测试 `bundled_anchor_pem_stays_in_sync_with_committed_public_key` L451-455 锁同步）+ `GAMER_PLUGIN_TRUST_DIR`/`<data>/plugin-trust/*.pem` 覆盖。

### 2.3 (b) manifest 解析结构体全字段现状

`ExtensionManifest`（manifest.rs L18-28，`RawManifest` L334-351，`deny_unknown_fields`）：

| 字段 | 类型/约束 | 备注 |
| --- | --- | --- |
| manifest_version | u32 | `MANIFEST_VERSION=1`（L13）；非 1 拒绝（L476-481）；alias `format_version` |
| id | ExtensionId | `[a-z0-9]` 安全名，≤128B（model.rs L23-33） |
| version | ExtensionVersion | SemVer，禁 build metadata `+`（model.rs L77-92） |
| name | String | 非空、≤256B、无控制字符 |
| description | Option\<String> | 可缺省 |
| entry | ExtensionPath | **强制 `.wasm` 后缀（L498-502）；禁指向 manifest.toml**。当前无 execution/builtin 概念 |
| host_api | HostApiRequirements | `[host_api]` 表；RawHostApiRequirements（L425-442）仅 8 域：device/vision/input/touch/resource/run/runtime/log。**没有 media 键**（见 §9 偏差5） |
| permissions | PermissionSet | 闭集枚举 19 项（permissions.rs L12-31：device/vision/input/touch/resource/run/runtime/log 13 项 + media.* 5 项）；filesystem/network/shell/process/device.shell 显式 Forbidden |
| ui | Vec\<UiContribution> | panel_id/title/icon/order/location(**仅 `console.right`**，L546-551)/runtime(core\|declarative\|iframe)/requires_device/preferred_width(200..=800)/entry(仅 iframe 且限 ui/ 下)/component(仅 core 必填，服务端不白名单组件名)/schema(仅 declarative：fields text/number/boolean/select/button) |

**结论：当前 manifest 无任何 execution / entry 语义区分，`entry` 恒为 WASM 路径。**

### 2.4 (c) 归档校验对 entry 必须是存在 WASM 的要求位置

两处独立检查（Phase 2 都要按执行类型分支）：

1. **`archive.rs::inspect_archive` L31-80**（安装 inspect 与 extract 共用）：
   - L36-40 entry 条目不存在 → `"manifest entry 不存在"`；
   - L41-45 entry 是目录拒绝；
   - L46-50 声明尺寸 <4 字节拒绝（`"WASM entry 太小"`）；
   - L52-60 读前 4 字节校验 `\0asm` magic → `"entry 不是 WASM 二进制"`。
2. **`store.rs::list_installed` L109-114**（已安装版本目录扫描）：`<id>/<version>/` 下 entry 文件必须存在 → `"WASM entry 不存在: {id}@{version}/{entry}"`。**注意此处在 list 路径上：若直接给已安装的 video 版本目录删掉占位 wasm，连列表都会报错。**

### 2.5 (d) 已安装签名元数据的存储与读取点（移除 verifier 的影响面）

**存储形态：签名不在 state.json 里。**`ExtensionRecord`（model.rs L241-247）只有 `id/active_version/state/last_error`。签名 = 独立文件 `signature.sig`（或 legacy `manifest.toml.sig`）存放在版本目录 `<data>/extensions/<id>/<version>/` 内，随包安装解压落盘。

读取点清单（移除 verifier 时会**编译失败/必须改写**的位置）：

| 位置 | 调用 |
| --- | --- |
| service.rs L148 | `ExtensionService.signature: SignatureVerifier` 字段 |
| service.rs L167/182/191-192/239 | 构造路径（`SignatureVerifier::default()` / `from_data_root`） |
| service.rs L1069-1090 | `snapshot_from_versions` → `signature.verify_installed(active.root(), …)`——`list()`/`snapshot_for()` 每次调用都重读重验 |
| service.rs L63/L73 | `ExtensionSnapshot.signature` / `ExtensionInspection.signature` 字段 |
| service.rs L396-409 | inspect_with_context 的验签/proof 分支（§2.2） |
| api/extensions.rs `snapshot_json` | JSON 字段 `"signature": snapshot.signature()`（插件列表/安装/更新响应） |
| api/extensions_management.rs L59、L141 | management 响应与 inspect 响应的 `"signature"` 字段 |

**数据层结论**：不再调用 `verify_installed` 后，存量版本目录里的 `signature.sig` 只是死文件，无解析、无崩溃路径，state.json 无 schema 迁移需求。计划 §4.1「不得因为移除 verifier 而导致列表/启动崩溃」在数据层面天然满足，工作量为上述编译面清理 + 前端展示字段处理。

### 2.6 service.rs / mod.rs 中的插件 ID 特判现状

| 位置 | 特判对象 | 行为 |
| --- | --- | --- |
| service.rs `instance_free` **L258-264** | `super::video::is_native_extension(id)`（video/mod.rs L25-27，字符串比较 `gamer.video`） | Native 扩展不启动 WASM 实例，start 只迁 Running |
| service.rs L669 / L959 | `KEYMAP_EXTENSION_ID`（keymap/mod.rs L31） | keymap 走独立 KeymapWasmRuntime（profile 参数、独立实例表） |
| extensions/mod.rs `native_call_action` **L84-91** | 分发到 `gamer_yaml::native_call_action` | **特判的是 gamer.yaml**（`automation.create_draft`，video_draft.rs L41-50：id==gamer.yaml 且 action==AUTOMATION_CREATE_DRAFT 才应答），不是 gamer.video；gamer.video 自身没有任何 call 动作 |
| api/extensions.rs L119-121 | `KEYMAP_EXTENSION_ID` | start body 的 `profile` 仅 keymap 接受 |
| gamer_yaml 侧 | `YamlTimerRunnerRegistrar::executes_without_instance`（service.rs L35-54 的 registrar trait 缺省 false） | gamer.yaml「按调用执行」由组合根 registrar 声明，非 service.rs 硬编码 |

## 3. gamer.video 现状与最小修复清单

### 3.1 模块结构

- `server/src/extensions/video/mod.rs`（163 行，单文件）：`VIDEO_EXTENSION_ID`（L21）、`is_native_extension`（L25-27）、`VIDEO_EXTENSION_MANIFEST_TOML`（L37-60）、3 个测试（manifest 与 tools 打包源逐字同步锁 L73-80、manifest 解析 L83-113、Native 生命周期 L118-161）。
- 无 guest 字节、无常驻实例、无 Runner、无 `[host_api]`；permissions 声明 media.* 五项（media.read/import/record/write/events.read）。
- UI：单条 `[[ui.contributions]]` `runtime="core"`、`component="VideoWorkbench"`、panel_id=`video`、order=28。
- manifest 内容（tools/plugins/gamer.video/manifest.toml 与常量逐字相同）：**仍声明 `entry = "plugin.wasm"`**（L42），但该目录**只有 manifest.toml 一个文件，没有 plugin.wasm**。
- 生命周期测试造包方式：zip 写入 manifest + 8 字节假 WASM `b"\0asm\x01\0\0\0"`（L136），即计划 §1.1-4 所述「最小 WASM 字节作测试包」——并且该假字节是**安装路径（archive.rs magic 校验）的硬性要求**，不是可选优化。
- service.rs 里的注册方式：无常量注册、无注册表，仅 §2.6 的 `instance_free` 字符串特判 + UI 贡献随 Enabled|Running 出现（refresh_ui_registry service.rs L896-903）。

### 3.2 让 video 作为无 guest 宿主预置插件正常安装的最小改动清单

1. **manifest 契约**（Phase 2 最小集，与 Phase 1 合并执行）：manifest 增加 execution 声明（计划 §5.1 `manifest_version=2` + `[execution] kind="builtin"`，或最小变体：builtin 类型允许 entry 缺省/保留占位语义）。涉及 `manifest.rs`（RawManifest/parse_manifest L471-528）、`archive.rs::inspect_archive` L31-80（builtin 跳过 WASM magic/存在校验）、`store.rs::list_installed` L109-114（builtin 跳过 entry 存在性检查）。
2. **宿主注册表**：`video/mod.rs::is_native_extension` 换成 BuiltinExtensionDescriptor 注册表（id→实现/宿主版本/生命周期），`service.rs::instance_free` L258-264 改查注册表；安装 `kind="builtin"` 时校验 id 已注册，未注册返回 `host_feature_unavailable` 类结构化错误。
3. **打包工具**：`tools/plugin-signer/src/main.rs::pack`（L143-185）`--key`/`--key-id` 必填（L146-147）且硬编码写 `plugin.wasm` 条目（L165）——需要无签名 pack 路径 + 允许无 wasm entry（或独立 pack 命令）。
4. **构建清单**：`tools/build-plugins.ps1` `$packages` 数组（L133-148）目前只有 keymap/yaml 两条；加 video 条目（无 Component），并在 registry.json 生成段（L183-220）加对应条目（name/permissions/ui 等目前硬编码在 ps1）。
5. **测试**：`video/mod.rs` 生命周期测试（L118-161）改用真实 builtin 包（不再需要假 wasm 字节）；api 侧无需新端点（安装路径不变）。
6. 不需要动：`call_extension`（video 无 call 动作）、媒体 REST（Core 直供）、前端面板（core-component-registry 已注册 `VideoWorkbench`）。

## 4. 官方市场链路

### 4.1 tools/build-plugins.ps1（231 行）流程与私钥依赖

| 步骤 | 位置 | 依赖私钥 tools/plugin-signing/ |
| --- | --- | --- |
| 1. 构建 plugin-signer | L69-74 | 否 |
| 2. 检查/生成签名密钥 | L77-97 | **是**：`<key-id>.key` 缺则 `signer keygen` 自动生成，并提示必须把公钥同步进 `signature.rs` 内置锚或 plugin-trust 目录（Phase 1 删除对象） |
| 3. guest → WASM Component | L101-120 | 否。**keymap guest 源 = `server/tests/keymap-guest`**（L117，产物 `gamer_keymap_fixture.wasm`）；yaml guest = `server/guests/yaml-guest`（L120，`gamer_yaml_guest.wasm`） |
| 4. pack + 签名 .gplugin | L122-181 | **是**：`signer pack --key … --key-id …`（L154-161），产物落 `web/public/plugins/`；同时产出 Registry proof（L169-175）（Phase 1 删除对象） |
| 5. 生成 registry.json | L183-230 | 间接：signature 字段值 = proof（L195/210）（Phase 1 改造对象） |

版本来源：`Get-ManifestVersion`（L126-131）从 tools/plugins/*/manifest.toml 读 version；但 registry 条目的 name/description/publisher/permissions/host_api/ui 全部**硬编码在 ps1**（L186-219，与 manifest.toml 双份维护，见 §9 偏差4）。

### 4.2 registry.json schema 现状（web/public/registry.json，schema_version=1）

```jsonc
{
  "schema_version": 1,
  "generated_at": "…Z",
  "host_api": "1.0.0",
  "plugins": [{
    "id": "gamer.keymap", "version": "1.0.0", "name": "…", "description": "…",
    "publisher": "gamer.dev",
    "download_url": "/plugins/gamer.keymap-1.0.0.gplugin",
    "sha256": "<64hex>", "size": 99383,
    "signature": { "status": "valid", "key_id": "gamer-dev-1", "algorithm": "ed25519",
                   "value": "<base64(RegistryProof JSON)>" },   // proof 绑定 id/version/download_url/sha256
    "permissions": ["…"], "host_api": {"input":"^1.0"},
    "ui": { "contributions": [ { "panel_id","title","runtime","component" } ] }
  } /*, gamer.yaml@3.1.0 —— 仅此两个条目，无 gamer.video */
  ]
}
```

`web/public/plugins/` 现有产物：`gamer.keymap-1.0.0.gplugin`、`gamer.yaml-3.1.0.gplugin` 两个文件。

### 4.3 前端对 signature.status=valid 的硬依赖

| 位置 | 行为 |
| --- | --- |
| `web/src/workspace/plugin-center/plugin-service.ts` `installPolicy` **L203-216** | `allowed: !(official && normalized.status !== 'valid')`——官方来源非 valid 直接阻断安装；local/url 归一为 unsigned 仅警告不阻断 |
| 同文件 `registryProofFor` **L80-104** | 从 registry 条目构造 proof（`value` 信封原样透传 / 裸 `signature` 包对象）；缺 key_id 或签名 → null → canInstall=false |
| `web/src/workspace/plugin-center/PluginCenter.vue` `canInstallMarket` **L220-223** | `!!registryProofFor(entry) && installPolicy(...).allowed` |
| 同文件 `installMarket` L301-314 | 阻断文案「官方插件必须具备已验证签名…」（L303）；official 安装上传 `installArchive` L278-281 附 `{source:'official', registryProof}` → api.js 映射为三个 `X-Gamer-*` 请求头 |
| 同文件 `inspectAndConfirm` L254-276 | `installPolicy(source, source.signature || inspection.signature)`，`!policy.allowed` throw |
| `web/src/workspace/plugin-center/registry-client.ts` | L72 signature 规范化已可选；L29 `signature.status` 为必填文本（规范到条目级：有 signature 对象则 status 必填）；`downloadFixedVersion` L160-162 official 缺 sha256 拒绝下载、L171-173 哈希不符拒绝（**保留**） |
| 市场页 `web/src/workspace/MarketView.vue` | 无签名硬依赖（只读已装清单 + Package 市场），插件安装全部委托 PluginCenter 弹层 |
| 锁定测试 | `web/src/plugin-center.test.js` L76-79（official+unsigned blocked）、L88-113（proof 透传/包装）；`plugin-center-activate.test.js` L26 |

### 4.4 服务端安装端点对签名的要求

- `POST /api/extensions/inspect` → `extensions_management.rs::api_inspect_extension` L100-151（`inspect_with_context`，official 时要求签名+proof）
- `POST /api/extensions` → `api/extensions.rs::api_install_extension` L45-64（`install_with_context`）
- `POST /api/extensions/:id/update` → `api/extensions.rs::api_update_extension` L66-82（`update_with_context`）
- 三者共用 `install_context`（extensions_management.rs L158-185）。签名门禁本体只有 §2.2 一处（service.rs inspect_with_context），服务端放开只需改那里 + 清 §2.5 读取面。

## 5. 视频工作台完成度核对

### 5.1 后端

| 能力 | 状态 | 位置 |
| --- | --- | --- |
| 媒体导入（临时目录→ffprobe 探测→原子提交） | 完成 | `server/src/media/mod.rs`（1226 行）`import_bytes` L194；`service()` 进程级单例 L558 |
| 元数据（`data/media/<id>/metadata.json` 为源，不落 SQLite） | 完成 | media/mod.rs MediaMetadata L63；状态机 importing→ready/missing |
| 精确帧提取（pts/index 可重复，max_width 等比） | 完成 | `extract_frame_png` L343 / FrameRequest L89 |
| Range 播放流 | 完成 | `api/media.rs` `api_media_file`（`GET /api/media/:id/file`） |
| 引用保护删除（MediaRef 登记 + delete 检查） | 完成（机制）/部分（消费方） | `set_refs` L356、`delete` L296；`POST /api/media/:id/refs` 端点存在，但 Package 侧业务写入方（视频项目）尚未建，refs 无人写入 → 待验证 |
| 录制（scrcpy 帧按需订阅、等 IDR、分段、设备独占、幂等 stop/cancel） | 完成（代码+单测）/真实设备链路待验证 | `server/src/recording/mod.rs`（1642 行）start L815/stop L952/cancel L961/`on_device_session_boundary` L1003；SegmentReason L65 |
| 内置 MP4 muxer（保留真实 PTS） | 完成（单测 4 项） | `recording/mp4.rs`（768 行） |
| InputObserver（inject_* 收敛点、operation_id 去重、text 脱敏、事件 JSONL） | 完成 | 观察点 `device/scrcpy.rs` L502（touch）/L550（key）/L571（text）；`InputEventRecord` recording/mod.rs L106-124（source: manual/keymap/runner/plugin；timeline_us=服务端单调钟，「录像起录时建立与媒体 PTS 的映射」为注释声明，**映射质量待验证**） |
| `/api/media` 端点组 | 完成 | `api/media.rs` L41-49：import/list/get/delete/file/frame/refs |
| `/api/recording` 端点组 | 完成 | `api/recording.rs` L30-35：start/active/:id/stop/cancel/status/events |
| 离线视觉 vision/test（media_id+pts_us，不触设备） | 完成 | `api/vision.rs`：`POST /api/capabilities/vision/test`，media_id 与 device_id 互斥（L69-75），离线路径走 `extract_frame_png`（L96-106） |
| automation.create_draft（录制事件→YAML v3 草稿文本） | 完成（V1：文本返回、不落盘不执行） | `extensions/gamer_yaml/video_draft.rs`（594 行）：`native_call_action` L41-50、`create_draft` L62、`build_draft` L99（纯函数）；事件源 = `RecordingService::events`（L89 附近 load_events）；诊断不静默丢弃 |

### 5.2 前端

| 能力 | 状态 | 位置 |
| --- | --- | --- |
| VideoWorkbench 三区装配（素材库/时间轴/草稿） | 完成（V1 范围） | `web/src/components/video/VideoWorkbench.vue`（65 行）；core-component-registry 注册名 `VideoWorkbench` |
| 素材库（列表/导入/删除两段确认/录制入口/active 轮询） | 完成 | `MediaLibrary.vue`（304 行） |
| 时间轴（`<video>` 预览 + 服务端精确帧 PNG + 逐帧步进） | 完成（V1）/33ms 为预览近似 | `VideoTimeline.vue`（186 行）；`stepFrame` L142-163 |
| 草稿区（事件勾选 → create_draft → YAML 文本+诊断，常驻「不自动执行」标注） | 完成 | `VideoDraft.vue`（238 行） |
| videoApi REST 封装（直调 fetch，含错误形态） | 完成 | `videoApi.js`（188 行） |
| StageSource（live/media 双来源、generation 过期防护、媒体模式舞台输入统一拒绝、指定帧 captureFrame） | 完成（V1 范围） | `components/console/useConsoleStage.js`（453 行）：`guardDeviceInput`、`DEVICE_INPUT_TYPES` L38-41、`FRAME_STEP_SECONDS` L40 |
| **Video Project / 标记 / 注释** | 未完成 | 计划 Phase 6；当前无项目数据结构 |
| **校准（有效画面区域/旋转/像素比例）** | 未完成 | 计划 Phase 6 |
| **模板制作联动（从选定帧创建模板）** | 部分 | 精确帧 PNG 已可取（Timeline/StageSource captureFrame），但「选定帧→模板保存」工作流未接 TemplateCrop 的 media 分支之外的闭环（TemplateCropModal 已支持 media=服务端确定帧，联动入口待 Phase 7 收口） |
| **离线模板匹配 UI** | 部分 | 后端 vision/test 离线路径完成；工作台面板内无匹配测试入口（现有入口在 Console 模板面板） |
| **事件↔媒体 PTS 精确映射** | 待验证 | 事件 timeline_us 为服务端单调钟；与媒体 PTS 的映射由录制侧声明，VFR/相邻帧定位未做展示帧索引映射 |

### 5.3 时间轴 33ms 步进的精确位置

- `web/src/components/video/videoApi.js` **L83**：`export const FRAME_STEP_SECONDS = 0.033`（注释明说「预览端按 ±33ms seek 后再取服务端精确帧」）；
- `web/src/components/console/useConsoleStage.js` **L40**：同名常量重复定义（舞台控制条用）；
- `VideoTimeline.vue` `stepFrame` L142-163 消费之。三处为独立字面量，非单一来源——Phase 6 改真实展示帧索引时需一并收口。

## 6. 插件签名 vs launcher/主程序更新签名（删除范围界定）

| | 插件签名（**Phase 1 删除范围**） | launcher/主程序更新签名（**不在删除范围**） |
| --- | --- | --- |
| 验签代码 | `server/src/extensions/signature.rs`（magic `gamebot-gplugin-sig-1`、registry claim `gamebot-gplugin-registry-entry-1`） | `launcher/src/manifest/sig.rs`（magic `gamebot-manifest-sig-1`）+ `launcher/src/manifest/mod.rs` L107-123 验签；`server/src/update/*` 仅透传 `signature_invalid` 错误码（model.rs L119/L135、ipc.rs 测试） |
| 工具 | `tools/plugin-signer/`（keygen/pack/registry-proof）、`tools/build-plugins.ps1`、`tools/plugin-signing/`（dev keypair：`.key` 私钥不入库、`.pem` 公钥入库） | `tools/verify-release.ps1`（compose/parser/cargo metadata/cargo audit 发布预检，**与插件签名无关**）、`tools/verify-external-release.ps1`、launcher 信任目录 `<keys-dir>/<key_id>.pem` |
| 信任锚 | `signature.rs` 内嵌 `BUNDLED_DEV_PUBLIC_KEY_PEM` + `plugin-trust/` 目录 | launcher 内置信任库（当前+下一把） |

两者 magic、密钥、信任库、工具链完全独立；删除插件签名链路不触碰任何 launcher/update 文件。

## 7. Keymap guest 现状

- **`server/tests/keymap-guest` 就是官方发布产物源**：`build-plugins.ps1` L117 直接以 `Build-Guest 'keymap-guest' …\tests\keymap-guest` 构建官方 `gamer.keymap-1.0.0.gplugin`。包名 `gamer-keymap-fixture`（Cargo.toml），`src/lib.rs` + `src/bin`（componentize）+ `ui/index.html`；测试侧另有 `build_guest_fixture_component` / `package_guest_fixture_gplugin`（extensions/mod.rs L47-48 re-export）现场造包。
- 对照结构 `server/guests/yaml-guest`：包名 `gamer-yaml-guest`，P12.8 已迁出 tests 目录（独立 Cargo.toml，注释声明被 build-plugins.ps1 与 yaml_extension 测试链路双消费）——即 yaml 已完成「正式归属」迁移，keymap 尚未（Phase 4 任务：迁 `server/guests/keymap-guest` 或等效正式位置，保持功能与 CI wasm32 门禁）。

## 8. 测试基线

### 8.1 运行方式

- Rust（`tools/ci-local.ps1` L60-70）：`cargo fmt --all -- --check` → `cargo clippy --all-targets --all-features -- -D warnings` → `cargo check --locked --no-default-features` → guest wasm32 构建 + componentize 校验（yaml-guest）→ `cargo test` → `cargo build --release`。
- Web（`web/package.json` scripts + ci-local L74-76）：`pnpm install --frozen-lockfile` → `pnpm test:run`（= `vitest run`）→ `pnpm build`。vitest 默认 include 只收 `src/*.test.js` 与 `src/script-editor/**/*.test.js`（PITFALLS 2026-09-05）。
- 注意：`cargo test --lib` 不可用（bin-only crate，PITFALLS）；本机内存墙用 `CARGO_PROFILE_DEV_DEBUG=0` + 降 `-j`，禁止 `cargo clean`（PITFALLS 2026-09-07）。
- **本次已实测**：`CARGO_PROFILE_DEV_DEBUG=0 cargo check -j 4` PASS（2m11s）。全量 `cargo test` 未跑（Phase 0 不要求，且本机有 wasmtime 编译内存墙与 wasm32 target 缺口会假红——PITFALLS 2026-09-07：缺 `wasm32-unknown-unknown` 时 32 个 guest 测试全数失败属环境缺口）。

### 8.2 插件相关现有测试文件

服务端：
- `server/src/extensions/mod.rs` tests（生命周期状态机/manifest/host_api/archive 安全/权限闭包/wit 契约）
- `server/src/extensions/service.rs` tests（启动对账两例：恢复+降级）
- `server/src/extensions/signature.rs` tests（验签/篡改/proof 绑定/信任锚同步）
- `server/src/extensions/video/mod.rs` tests（Native 生命周期 + 打包 manifest 同步锁）
- `server/src/extensions/manifest.rs` tests（declarative/core/iframe schema 全矩阵）
- `server/src/api/tests/extensions.rs`（REST 全生命周期 + inspect unsigned 语义 + 管理视图）
- `server/src/architecture_guard_tests.rs`（扩展生命周期全链测试用 `YAML_EXTENSION_MANIFEST_TOML` 现场解析版本，勿硬编码——L1010-1176；源码白名单 `EXTENSION_SOURCE_DIRS` L59）
- keymap/yaml 侧：`extensions/keymap/mod.rs`、`extensions/gamer_yaml/*`（数量大，不展开）

前端：
- `web/src/plugin-center.test.js`、`plugin-center-activate.test.js`（installPolicy/proof 语义锁，Phase 1 必改）
- `core-component-registry.test.js`、`contribution-manager.test.js`、`declarative-panel-host.test.js`、`iframe-plugin-call.test.js`、`gamer-keymap-extension.test.js`、`core-shell-boundary.test.js`

### 8.3 视频相关现有测试文件

- 服务端（模块内联 tests）：`media/mod.rs`（16 项）、`recording/mod.rs`（11 项）、`recording/mp4.rs`（4 项）；api 层无独立 media/recording 集成测试文件（`api/tests/` 无 media.rs/recording.rs）
- 前端：`video-workbench.test.js`、`video-api.test.js`、`console-stage.test.js`

## 9. 与计划假设不符的偏差清单（重要）

1. **前端是第二道签名门禁**：计划 §1.1-6 只提到服务端 `inspect_with_context` 强制签名；实际 `plugin-service.ts::installPolicy` L203-216 + `PluginCenter.vue::canInstallMarket` L220-223 在浏览器侧就阻止 official+unsigned。服务端放开而前端不改，市场安装仍会被拦——Phase 1 必须前后端同步。
2. **签名元数据不在「已安装记录」**：计划 §4.1 说「现有签名元数据若仍在已安装记录中」——实际 state.json（ExtensionRecord）不含签名字段，签名是版本目录里的 `signature.sig` 文件。移除 verifier 后旧文件被静默忽略，无崩溃路径、无状态迁移；要处理的是 §2.5 列出的编译面读取点。
3. **`native_call_action` 特判的是 gamer.yaml 而非 gamer.video**：计划 §5.2 说「service.rs 中视频 ID 特判收敛到通用注册机制」——video 的特判只有 `instance_free → is_native_extension`（经 video/mod.rs，一行字符串比较）；service.rs 内的字面量特判另有一处 keymap（L669/L959）。视频草稿的原生动作缝挂在 gamer.yaml 名下（跨插件：video 面板 → gamer.yaml 动作）。
4. **registry.json 的元数据硬编码在 ps1**：计划 §4.2 要求「由 manifest 或统一插件描述读取 ID/版本，禁止多处硬编码版本号」——现状只有 version 读自 manifest.toml，name/description/publisher/permissions/host_api/ui 全部硬编码在 build-plugins.ps1 L186-219，与 tools/plugins/*/manifest.toml 双份维护，改造时一并收敛。
5. **manifest 无法声明 media 域 host_api 要求**：`RawHostApiRequirements`（manifest.rs L425-442）只有 8 域，`HostApiDomain::Media` 存在于 catalog/权限闭集但不接受 `[host_api] media=…` 声明（video manifest 也刻意不声明）。Phase 2 做 host_api 契约或 WIT media 挂 world 时需同步补该键，否则 media 域版本要求无入口。
6. **video 的 stop 后 UI 贡献不消失**：`refresh_ui_registry`（service.rs L896-903）对 Enabled|Running 都注册贡献；video/mod.rs 测试 L154-158 明确断言 stop→Enabled 后贡献仍可见、disable 后才消失。计划 §4.4 验收写「stop/disable 后 UI 消失」——与现行为不一致，Phase 1 验收标准需按实际语义（disable 即消失）或改 refresh 语义，二选一先定。
7. **视频包当前完全打不出来**：除计划已知的「无 video 条目」外，`plugin-signer pack` 的 `--key`/`--key-id`/`--wasm` 全部必填且硬编码 `plugin.wasm` 条目名——即使想打一个带假 wasm 的 video 包也得先改 signer。最小修复清单见 §3.2。
8. **keymap-guest 的 ui/index.html 存在但当前打包不带 ui 资产**：build-plugins.ps1 L134-136 注释说明两个官方插件面板均为 runtime=core、`Files=@()`——tests/keymap-guest/ui/ 目录是遗留 iframe 资产，Phase 4 迁移时可一并清理。
9. **已安装版本目录扫描会因 entry 缺失直接报错**（store.rs L109-114 在 list 路径）：Phase 2 改造 builtin 支持时，若先手工删除占位 wasm 再升级服务端，`GET /api/extensions` 整体 500（InvalidArchive）——迁移说明需覆盖此形态。

## 10. 保留 / 修改 / 删除清单（Phase 1/2 视角）

**保留（不动）**：
- 生命周期状态机、版本不可变存储、原子安装、activate_version、启动对账（service.rs 主体）
- 权限闭集（permissions.rs）、Host API 版本门禁（host_api.rs）、UI contribution 三档（ui.rs/manifest.rs）
- 归档安全校验主体（archive.rs：中央目录/穿越/重复/加密/解压限额）——仅 entry 检查按执行类型分支
- 浏览器下载完整性（registry-client.ts `downloadFixedVersion` sha256 校验、大小上限 20MiB）
- 全部媒体/录制/vision 后端与前端工作台 V1 实现（§5 全部「完成」项）
- launcher/主程序更新签名全链（§6 右列）
- 本地免签安装路径（已存在，§2.2）

**修改**：
- `service.rs::inspect_with_context`（去官方强制分支）、`snapshot_from_versions`（签名展示面）、`ExtensionService.signature` 字段链
- `api/extensions.rs::snapshot_json`、`api/extensions_management.rs`（signature 字段处置：留作历史信息或删）
- `manifest.rs`/`archive.rs`/`store.rs`（execution 类型感知，Phase 2）
- `video/mod.rs`（is_native_extension → 注册表）
- `plugin-signer`（pack 与签名解耦）、`build-plugins.ps1`（去 keygen/proof/签名门禁、加 video、元数据收敛）
- `plugin-service.ts`/`PluginCenter.vue`/`registry-client.ts`（signature 可选化）+ `plugin-center*.test.js` 语义翻转
- registry schema（signature 不再必需；schema_version 升级，开发期不需双格式兼容）

**删除**：
- `extensions/signature.rs` 的 verifier 本体（或整体模块，视展示面决定）
- `build-plugins.ps1` 步骤 2（keygen/密钥检查）、registry-proof 生成、signature.sig 写入
- `tools/plugin-signing/*.key` 私钥（公钥 pem 与 signature.rs 内嵌锚随 verifier 一起退役）
- 前端 installPolicy 的 official 签名阻断分支与 registryProofFor 的安装门禁用法（registry proof 数据结构可整体移除）

## 11. Phase 1 / Phase 2 精确代码落点表

### Phase 1（免签名 + 市场修复 + video 发布）

| # | 文件 | 函数/位置 | 行为变更 |
| --- | --- | --- | --- |
| 1 | server/src/extensions/service.rs | `inspect_with_context` L387-418 | 删 L393-409（official proof 必需 + 签名 Valid 强制 + proof 验证）；保留 sha256 计算与 permission_diff |
| 2 | server/src/extensions/service.rs | 字段 L148、构造 L157-247 | 移除/退役 SignatureVerifier（或留空实现仅展示） |
| 3 | server/src/extensions/service.rs | `snapshot_from_versions` L1069-1090 | 不再调 verify_installed；SignatureInfo 展示改 Unsigned/移除 |
| 4 | server/src/api/extensions.rs | `snapshot_json` | `"signature"` 字段处置（删或恒 unsigned） |
| 5 | server/src/api/extensions_management.rs | L59、L141、`install_context` L158-185 | 同上；`x-gamer-registry-proof` 头解析退役（保留 permission confirm） |
| 6 | server/src/extensions/signature.rs | 整体 | verifier 删除；RegistryProof/TrustStore 随迁；测试 `bundled_anchor…` 同步删 |
| 7 | tools/plugin-signer/src/main.rs | `pack` L143-185 | `--key` 可选；支持无 wasm（builtin 包）或新增 pack 模式 |
| 8 | tools/build-plugins.ps1 | 步骤2 L77-97、步骤4 签名 L154-175、registry L183-230 | 删 keygen/proof/签名；packages 增 video；条目元数据收敛 |
| 9 | web/public/registry.json + plugins/ | 产物 | 无签名重打，增 video 条目 |
| 10 | web/src/workspace/plugin-center/plugin-service.ts | `installPolicy` L203-216、`registryProofFor` L80-104 | official 不再按签名阻断（保留来源标注与权限确认）；proof 构造退役 |
| 11 | web/src/workspace/plugin-center/PluginCenter.vue | `canInstallMarket` L220-223、`installMarket` L301-314、`installArchive` L278-281、`inspectAndConfirm` L262-273 | 放开 official；确认文案去掉 proof 语义，保留权限增量/来源展示 |
| 12 | web/src/workspace/plugin-center/registry-client.ts | `normaliseSignature` L25-36 | signature 缺省容许（已部分可选，收口 status 必填分支） |
| 13 | web/src/plugin-center.test.js 等 | L76-79、L88-113 | 断言语义翻转（official+unsigned → 允许+来源标注） |
| 14 | server/src/extensions/video/mod.rs + manifest.rs/archive.rs/store.rs | §3.2 清单 | video 以 builtin 包发布（与 Phase 2 最小集合并执行，计划 §13） |

### Phase 2（执行类型 / 宿主注册表 / Bridge 收口）

| # | 文件 | 位置 | 行为 |
| --- | --- | --- | --- |
| 1 | server/src/extensions/manifest.rs | `MANIFEST_VERSION` L13、`RawManifest` L334-351、`parse_manifest` L471-528 | manifest_version=2 + `[execution]`（kind=builtin\|wasm、builtin_id）；entry 校验按 kind 分支（L498-502） |
| 2 | server/src/extensions/archive.rs | `inspect_archive` L31-80 | builtin：跳过 entry 存在/magic 校验；wasm：维持全检（防绕过，计划 §5.2） |
| 3 | server/src/extensions/store.rs | `list_installed` L109-114 | builtin 跳过 entry 存在性 |
| 4 | server/src/extensions/video/mod.rs（或新模块） | 新增 BuiltinExtensionDescriptor 注册表 | id→实现/宿主版本/生命周期；替代 `is_native_extension` |
| 5 | server/src/extensions/service.rs | `instance_free` L258-264、`inspect_compatible` L857-861 | 按注册表判定；安装/启动未知 builtin id → `host_feature_unavailable` 结构化错误 |
| 6 | server/src/extensions/mod.rs | `native_call_action` L84-91 | 审计 declarative/`plugin.call`/native 动作缝 → 统一命令注册接口（§5.3）；keymap/yaml 特判收敛评估 |
| 7 | server/src/extensions/manifest.rs + 前端 core-component-registry | core contribution component 键 L554-578 | 第三方不得任意挂宿主组件（服务端按 builtin 贡献白名单或维持前端白名单+限制 core runtime 声明来源） |
| 8 | architecture_guard_tests.rs / video、extensions 测试 | 全链测试 | 「假 WASM」例外清理；builtin/WASM 双形态生命周期用例 |

## 12. 结论

- 基线可编译（cargo check PASS）；服务端本地免签安装已存在，强制签名只在「official 来源」一条路径 + 前端一道策略门禁，Phase 1 的破坏面比计划预估的更小且完全定位。
- video 插件发布闭环的硬阻塞 = manifest 无 builtin 语义（archive/store 双重 entry 校验）+ signer pack 强制 key/wasm + build-packages 无 video 条目，最小改动清单见 §3.2。
- 视频工作台 V1 合同范围已全部落地且有测试；缺口集中在 Video Project/标记/校准/事件-PTS 映射（Phase 5-7 对象），无重复开发风险。
- 需要计划方在 Phase 1 验收前拍板的两个语义点：stop 后 UI 贡献是否保留（偏差6）、registry schema 升级是否允许直接改 schema_version=2（计划 §4.2 已表态允许，前端 `REGISTRY_SCHEMA_VERSION=1` 同步升）。
