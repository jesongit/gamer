# Phase 4 §7.1 归属审计：YAML / Keymap（含 video 对照）

> 计划：`docs/plans/gamer_plugin_ecosystem_video_finalization_plan.md` §7.1（官方插件归属收口）
> 日期：2026-09-07
> 基线：HEAD `eae786c`（Wave B 免签名 + manifest v2 + builtin 注册表已入库），工作树含并行任务（media/recording/web/sdk）在途改动，本任务未触碰。
> 性质：逐文件真实代码审计 + 本轮可执行项落地；「后续做」清单在 §5。

## 1. 归属总表（文件/函数级）

图例：**所有者**=业务语义归属的扩展边界；**随宿主发布**=必须编译进 Gamer 主程序才能工作；**归类**：A=保留宿主（机制/边界层）、B=应迁 guest/插件资产、C=应抽通用机制、D=已迁插件资产（本轮完成）。

### 1.1 gamer.yaml（YAML v3 扩展）

| 文件/函数 | 所有者 | 随宿主发布 | 归类 | 说明 |
| --- | --- | --- | --- | --- |
| `gamer_yaml/yaml_vnext.rs`（v3 parse+lower+函数库解析） | gamer.yaml | 是（A） | A | v3 唯一语法方案；Native 参考解释器与 WASM guest 共用的前端（ADR-11：语法语义归扩展，但参考实现在宿主 Rust 内是刻意的双执行模型一半） |
| `gamer_yaml/yaml_extension.rs`（NativeYamlHost + capability invoker + key_code 词表） | gamer.yaml | 是（A） | A | 原生参考解释器；与 guest 行为同源锁定（计划 §7.1「已在宿主实现、确实属于该插件的业务由其扩展模块拥有」） |
| `gamer_yaml/wasm_host.rs`（LazyYamlWasmtimeRuntime，按调用惰性实例化） | gamer.yaml | 是（A） | A | 按调用执行模型的运行时；guard §14.1 白名单豁免文件 |
| `gamer_yaml/runner_adapter.rs`（v3-only EngineExecutor） | gamer.yaml | 是（A） | A | 统一执行 `/api/runs` 的执行器；组合根装配（main.rs 白名单） |
| `gamer_yaml/timer_yaml.rs`（YamlTimerRunner + Registrar，`executes_without_instance=true`） | gamer.yaml | 是（A） | A | ADR-13 Runner 注册缝的扩展侧实现；执行模型由 registrar 自声明，**非 service.rs 特判**（已通用） |
| `gamer_yaml/task_params.rs` / `entrypoint_descriptor.rs` / `params.rs` / `error.rs` / `run_target.rs` | gamer.yaml | 是（A） | A | 参数门禁/schema 描述器/诊断五元组——脚本业务语义，随扩展边界在宿主 |
| `gamer_yaml/resources.rs`（automations/functions/templates 内容钩子） | gamer.yaml | 是（A） | A | 经 Core `ResourceHandler` 注册表挂接（组合根注册，无 id 特判） |
| `gamer_yaml/video_draft.rs::native_call_action`（`automation.create_draft`） | gamer.yaml | 是（A） | A/C | 归属正确（gamer.yaml 拥有 YAML 生成）；分发缝见 §2 特判表 #4 |
| `server/guests/yaml-guest`（gamer-yaml-guest，yaml-extension-host world） | gamer.yaml | **否（D）** | D | guest 内 v3 解释执行；P12.8 已迁正式目录，本轮不动 |
| 前端 `script-editor/`、自动化/函数/模板面板 | gamer.yaml | 否（前端资产） | A | runtime=core 宿主组件，`core-component-registry` 白名单解释；独立分发属后续轮次（Phase 3 SDK 路线） |

### 1.2 gamer.keymap（Keymap 扩展）

| 文件/函数 | 所有者 | 随宿主发布 | 归类 | 说明 |
| --- | --- | --- | --- | --- |
| `keymap/mod.rs` 输入信封（`InputEvent`/`InputResult`/`DeviceAction`/`decode_input_event`/`ScreenSize`） | gamer.keymap | 是（A） | A | transport-neutral wire 契约（`gamer-input@1`），host↔guest 共享 ABI |
| `keymap/mod.rs` E2E trace（`KeymapTraceContext`/sink/env 门禁） | gamer.keymap | 是（A） | A | Phase 6 基准埋点，零开销默认关 |
| `keymap/mod.rs` `CapabilityDeviceActionExecutor`（动作→能力 SDK 执行） | gamer.keymap | 是（A） | A | 授权+能力执行的宿主侧一半（计划 §7.1：guest 产出动作、宿主授权执行，不复制业务） |
| `keymap/mod.rs` `LazyKeymapWasmRuntime`（keymap WIT world 常驻实例运行时） | gamer.keymap | 是（A） | A | keymap-host world 独立于通用 extension-host（同步 ABI，见 PITFALLS） |
| `keymap/mod.rs` profile 通道（`PackageKeymapSource`/`load_user_profile`/`KEYMAP_PROFILE_PREFIX`） | gamer.keymap | 是（A） | A | 从 PackageStore 三元组读 `mappings/*.yaml` 原文交 guest；插件数据隔离由 Core 保证 |
| `keymap/dsl.rs`（keymap YAML parse/serialize/strict 校验） | gamer.keymap | 是（A） | A | 保存期校验 = `ResourceHandler` 钩子（组合根注册）；规则语义本体已全在 guest |
| `keymap/mod.rs::android_keycode`（词表） | gamer.keymap | 是（A） | B（低优） | 与 guest 内精简词表双份维护；收敛方案（guest 回传 or 生成共享 crate）记后续 |
| **`server/guests/keymap-guest`（gamer-keymap-guest，映射引擎 WASM guest）** | gamer.keymap | **否（D）** | **D（本轮迁出 tests/）** | 映射规则唯一引擎（tap/swipe/raw_key/hold + profile 覆盖 + WASD 内置默认）；本轮自 `server/tests/keymap-guest` 转正，见 §3 |
| `guests/keymap-guest/ui/index.html` | gamer.keymap | 否 | B（遗留） | 遗留 iframe UI 资产；官方包 runtime=core **不携带**，仅测试造包用（验证 `read_ui_file` 服务路径）。后续轮次可删除或转纯测试 fixture |
| 前端 `KeymapPanel.vue` / `useConsoleKeymap.js` / `gamer-keymap-extension.js` | gamer.keymap | 否（前端资产） | A | runtime=core 宿主组件；同 YAML 面板，独立分发属后续 |

### 1.3 gamer.video（对照，只列不改）

| 文件/函数 | 所有者 | 随宿主发布 | 归类 | 说明 |
| --- | --- | --- | --- | --- |
| `video/mod.rs`（VIDEO_EXTENSION_ID + manifest v2 常量 + 打包同步锁测试） | gamer.video | 是（A） | A | builtin 执行类型；`[execution] kind="builtin"` → 宿主注册表 |
| `extensions/builtin.rs`（BUILTIN_EXTENSIONS 静态注册表） | Core 机制 | 是（A） | A | 计划 §5.2 收口已完成（取代 `is_native_extension` 字符串特判）；下载包不可扩展 |
| `media/`、`recording/`、vision/device Core | Core | 是（A） | A | 通用机制（计划 §7.2 明确保留 Core） |
| 前端 `components/video/`（VideoWorkbench 三区 + videoApi） | gamer.video | 否（前端资产） | B（后续） | 计划 §7.2「优先独立 UI 资产」——后续轮次评估 |

## 2. 残留 `if plugin_id == ...` / 插件 id 字面量特判清单与处置

| # | 位置 | 特判对象 | 现状 | 处置 |
| --- | --- | --- | --- | --- |
| 1 | `service.rs::start_with_context`（现 L628） | gamer.keymap | `id.as_str() == KEYMAP_EXTENSION_ID` 选运行时 | **本轮做**：改 `super::keymap::is_keymap_extension(id)`，id 知识收敛到 keymap/mod.rs |
| 2 | `service.rs::stop_running_instance`（现 L928 附近） | gamer.keymap | 同上（stop 路由） | **本轮做**：同上 |
| 3 | `service.rs::dispatch_keymap_input`（现 L278） | gamer.keymap | `ExtensionId::parse(KEYMAP_EXTENSION_ID)` 查实例表 | **本轮做**：改 `super::keymap::keymap_extension_id()`；service.rs 已不再 import `KEYMAP_EXTENSION_ID` |
| 4 | `extensions/mod.rs::native_call_action`（L82-89） | gamer.yaml | 通用缝硬编码分发给 `gamer_yaml::native_call_action`（yaml 侧自判 `id != YAML_EXTENSION_ID \|\| action != AUTOMATION_CREATE_DRAFT` 返回 None） | **后续做**：缝已通用化一半（归属判定在 gamer_yaml 自身、declarative 走按钮白名单）；把缝升级为「扩展自注册 native action 描述符表」需动 extensions/mod.rs + 契约测试，留下一轮 |
| 5 | `api/extensions.rs` L119 | gamer.keymap | `id != KEYMAP_EXTENSION_ID` 门禁 start 的 `profile` 参数（“仅 keymap 支持 profile”） | **后续做**（api 层非本轮所有权）；方案 = start 请求参数按执行世界声明（manifest 或运行时能力查询）判定，而不是 api 层 id 比较 |
| 6 | `service.rs::instance_free` | gamer.video | `builtin::is_builtin_extension(id)`（静态注册表） | **已完成**（Wave B）；注册表 + registrar `executes_without_instance` 就是通用机制，无需再动 |
| 7 | `maintenance.rs` L612（cfg(test)） | gamer.yaml | 测试夹具造合规包目录用 `plugins/gamer.yaml/automations` 字面量 | 无需处置（测试夹具，非业务分支） |
| 8 | `main.rs` L320-341（组合根） | gamer.yaml | `register_resource_handlers` / EngineExecutor / Registrar 装配 | 无需处置（guard §14.1/§14.2 白名单的唯一装配点） |

结论：机制层（service.rs）已不再持有任何插件 id 字面量——三个执行模型判定全部经扩展边界或静态注册表的通用谓词（`builtin::is_builtin_extension` / `keymap::is_keymap_extension` / registrar `executes_without_instance`）；剩余特判都在 api 装配层（#4/#5），归属判定均在扩展模块自身。

## 3. 本轮改动清单

1. **Keymap guest 转正（git mv 保历史）**：`server/tests/keymap-guest/` → `server/guests/keymap-guest/`（与 yaml-guest 同级的正式归属）。
   - 包名 `gamer-keymap-fixture` → `gamer-keymap-guest`（cdylib 产物 `gamer_keymap_fixture.wasm` → `gamer_keymap_guest.wasm`）；Cargo.lock 随构建再生成（仅包名行变化）。
   - guest `src/lib.rs` 头注释转产品化；组件实现 struct `Fixture` → `KeymapGuest`（导出面由 WIT world 决定，不变）。**规则引擎零改动**（功能等价迁移）。
   - WIT 路径 `../../wit/keymap` 新旧目录深度相同（都指 `server/wit/keymap`），无需变更。
2. `server/src/extensions/keymap/mod.rs`：`build_guest_fixture_component` 内部路径/产物名更新（**函数名保留**——`api/tests/packages_dormant.rs`、`api/tests/packages_states.rs` 经 `extensions/mod.rs` re-export 按名消费，改名会越权改非本任务文件）；`package_guest_fixture_gplugin` 与 UI 断言的 `include_bytes!` 路径同步；新增 `is_keymap_extension`/`keymap_extension_id` 边界谓词 + 单元测试 `runtime_routing_predicate_is_owned_by_the_keymap_boundary`。
3. `server/src/extensions/service.rs`（仅 keymap 特判收敛相关行）：§2 #1/#2/#3 三处改调 keymap 边界谓词；imports 移除 `KEYMAP_EXTENSION_ID`。
4. `tools/build-plugins.ps1`：`$GuestRecipes['gamer.keymap']` Dir→`guests\keymap-guest`、Lib→`gamer_keymap_guest.wasm`（+ 同一脚注与 .DESCRIPTION 里指向旧路径的注释同步，共 3 行，均为 keymap 路径提及）。
5. `.gitignore`：`server/tests/keymap-guest/target/` → `server/guests/keymap-guest/target/`（迁移连锁，否则新 target 目录成未跟踪噪音）。
6. 新建本证据文档。
7. 测试 fixture 与产品 guest 分离评估：**无需**另立最小 fixture——`package_guest_fixture_gplugin`（测试造包器，dev-only）继续以产品 guest 源码现场构建；测试专用的是「造包 manifest（1.0.0/iframe UI/权限集可调）」而非 guest 字节。

## 4. 迁移等价性证据与测试结果

### 4.1 打包产物一致性（不比整体 sha256——plugin-signer zip 带 mtime，非字节可复现，见 PITFALLS P12.8）

- 干跑产物（临时目录，未触碰 `web/public`）：`gamer.keymap-1.0.1.gplugin` 内条目 = `manifest.toml` + `plugin.wasm`（与迁移前官方包条目布局一致）；`manifest.toml` 与打包源 `tools/plugins/gamer.keymap/manifest.toml` **逐字节相同**（diff 空）。
- registry v2 干跑：三插件齐备——`gamer.keymap@1.0.1(kind=wasm, sha256=6b5b5908…) / gamer.video@1.0.0(builtin) / gamer.yaml@3.1.1(wasm, d6e343e4…)`，schema_version=2，signer verify 自检全过。

### 4.2 组件字节对比（迁移前源码 @HEAD 原样重建 vs 迁移后构建）

- 内层 wasm module：代码段（前 ~240KB）**逐字节相同**；全部差异集中在 wasm name 自定义节（`gamer_keymap_fixture.wasm` → `gamer_keymap_guest.wasm` 及 crate 名派生的符号 mangling，长度差 -135B）与外层组件 LEB 尺寸前缀——即**可执行语义零差异，差异仅为包名/符号名元数据**。
- 结论：迁移为功能等价迁移，无需重验映射语义（且 §4.3 行为套件全绿双保险）。

### 4.3 测试结果（本机，`CARGO_PROFILE_DEV_DEBUG=0`，`-j 2`）

| 命令 | 结果 |
| --- | --- |
| `cargo check -j 4`（全部 Rust 编辑完成后） | PASS（11.9s）。**注**：其后并行任务的在途 `media/mod.rs` 改动引入 `E0004 MediaErrorKind::FrameNotFound`（非本任务文件，按约束不修不复跑全量；集成者合流后需复跑） |
| `cargo test -j 2 keymap` | **23 passed / 0 failed**（含真实组件测试：`real_keymap_gplugin_invokes_wit_and_native_capabilities`、`real_keymap_guest_consumes_user_profile_yaml`、权限拒绝、p95 延迟界；均自新路径现场构建 guest） |
| `cargo test -j 2 architecture_guard` | **7 passed / 0 failed**（§14.1-§14.7 全绿，含 keymap 输入直通隔离与生命周期全链） |
| `cargo test -j 2 extensions::` | **171 passed / 0 failed**（manifest/archive/builtin/video/service 对账 + 双 guest 组件测试） |
| `cargo test -j 2 -- extension_rest_lifecycle core_runtime_contributions declarative_plugin_call_roundtrip official_plugin_market` | **4 passed**（REST 生命周期/UI 贡献仅 Running/declarative call/官方市场端到端） |
| `cargo test -j 2 -- draft native_call_action` | **7 passed**（automation.create_draft 分发缝回归） |
| `build-plugins.ps1 -OutputDir <tmp> -RegistryFile <tmp>` | PASS：keymap 1.0.1 自 `guests/keymap-guest` 打包成功，yaml 3.1.1 同过，video builtin 打包，signer verify + sha/size 自检全过 |
| wasm32-unknown-unknown target | 已安装（无环境缺口） |

## 5. 遗留问题与「后续做」清单

1. **§2 #4**：`extensions/mod.rs::native_call_action` 硬编码 gamer.yaml 分发 → 升级为扩展自注册的 native action 描述符表（牵 extensions/mod.rs + 守卫测试，跨任务文件）。
2. **§2 #5**：`api/extensions.rs` L119 的 keymap profile 门禁 id 比较 → 改按执行世界/manifest 声明判定（api 层）。
3. **keymap `android_keycode` 双份词表**（host `keymap/mod.rs` 与 guest 精简版已漂移：host 多出 PageUp/Home/End 等、guest 多 Home=3/Back=4 语义差异——guest 走 profile 校验路径、host 走动作执行路径，当前无冲突但应收敛为单一来源或生成共享）。
4. **`guests/keymap-guest/ui/index.html`**：遗留 iframe UI 资产，官方包不携带，仅测试使用；后续可删除并把 `read_ui_file` 服务路径测试改用内联 HTML。
5. **前端 core 面板（自动化/函数/模板/映射/视频工作台）独立分发**：runtime=core 宿主组件仍随宿主前端发布；独立 UI 资产化归 Phase 3 SDK / Phase 7 联动轮次（计划 §7.1「对能独立交付的业务和 UI 逐步改为插件包携带」）。
6. **`build_guest_fixture_component`/`package_guest_fixture_gplugin` 命名**：函数名仍带 "fixture"（历史 API，被 api tests 按名消费，改名需动 `api/tests/*`）；语义已注释澄清（构建的是产品 guest 源码）。
7. 并行任务在途的 `media/mod.rs` 编译错（`MediaErrorKind::FrameNotFound` E0004）非本任务范围，集成者合流后须全量复跑。
