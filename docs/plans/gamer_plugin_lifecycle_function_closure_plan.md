# Gamer 插件生命周期与函数体系收尾开发计划

> 原始状态：待实施
> 当前执行状态：IMPLEMENTED_WITH_NOT_VERIFIED
> 项目：`jesongit/gamer`
> 执行基线：`a5a175ff06013b3ef6cda9f328eb9ecd285cfc33a`
> 适用阶段：开发期，允许必要的破坏性修改，不新增旧架构兼容层

## 1. 目标与范围

本轮以修复上一轮审查发现的实际问题、完成函数体系回归和发布验收为目标，不进行大规模架构重构。Core/Extension/Package/Task 的既有边界、YAML V1 唯一语法、免签名插件分发和视频工作台 V1 合同继续有效。

需要完成：

1. 原生动作在实际执行前完成插件状态、公开能力、权限和上下文检查。
2. 修复必需依赖的停用、卸载和版本切换守卫。
3. 修复服务端重启后的依赖拓扑恢复和状态一致性。
4. 补齐跨插件调用的可信调用方、Package Context 和资源权限边界。
5. 修复前端可选依赖/能力探测过期和异步竞态。
6. 回归 `_function.yaml`、`_function*.yaml`、基础函数、便利函数及设备生命周期。
7. 验证官方市场、视频工作台、插件构建与正式发布链路。
8. 更新开发文档、实施证据和最终验收报告，明确所有 `NOT_VERIFIED` 项。

不在范围：重写 YAML 解释器或函数库模型；增加多文件函数库管理 UI；建设复杂依赖管理器、RPC/服务发现、自动安装/自动启用；恢复旧 YAML、旧目录或强制签名；开放任意 Native 动态库加载。

## 2. 阶段计划

### Phase 0：最新基线与问题复现

状态：DONE（基线为 `a5a175f`，详细记录见 `docs/evidence/plugin_lifecycle_closure_baseline.md`）。

- 记录分支、HEAD、工作树和未提交修改。
- 阅读最新 `AGENTS.md`、`docs/reference/YAML.md`、插件开发指南及上一轮报告。
- 核对 `ExtensionService::call_extension`、原生动作分发、依赖守卫、恢复、跨插件调用和前端能力探测。
- 对仍存在的问题先建立最小复现测试，再修改实现。

验收：报告列出问题的 `仍存在 / 已修复 / 需调整 / 未验证` 状态、修改落点和测试命令。

### Phase 1：原生动作调用生命周期

- 将原生动作的公开识别与实际执行分离。
- 执行前检查目标已安装、active 版本有效、状态为 Running、动作属于公开清单。
- 复用 Host API/PackageStore 权限与路径校验；不信任请求伪造的插件或 Package 身份。
- Native、WASM、declarative 调用遵守一致的生命周期门禁。
- 以每插件调用闸门或等价租约解决 stop/disable/uninstall 与调用的竞态：新调用在停用开始后拒绝，已进入调用安全完成；不持有全局生命周期锁等待长耗时操作。
- 保存失败不得留下可正常使用的半成品。

必须回归：停用/未启动时 `automation.save_draft`、`template.create_from_frame` 无副作用；未公开动作拒绝；正常 Running 下模板、草稿和 WASM 动作仍可用；长耗时调用不死锁。

### Phase 2：依赖停用与版本守卫

- 依赖版本要求必须与被依赖插件的实际 active 版本比较，而非依赖方自身版本。
- stop、disable、uninstall、activate_version 统一遵守运行中必需依赖守卫。
- 版本切换不得通过先换版本再停用绕过守卫；新 active 版本必须保持所有运行中依赖方的要求。
- 可选依赖不阻止停用/卸载；拒绝时返回具体依赖方和处理提示，不自动级联。
- 保持现有循环检测和状态机，不引入第二套生命周期。

必须覆盖：必需依赖 B `>=2.0` 且 B 为 2.0/3.0；可选依赖；依赖方已停用；不兼容版本切换；必需依赖循环；卸载非 active 旧版本。

### Phase 3：服务端重启恢复

- 持久化的旧 `Running` 只表示上次进程状态，不代表当前有真实实例或 Runner。
- 恢复按必需依赖拓扑排序，提供方先于依赖方；不依赖持久化记录顺序。
- 提供方缺失、版本不兼容或启动失败时，依赖方保持可诊断的非 Running 状态。
- 可选依赖恢复失败不阻止基础插件恢复。
- Runner 注册与真实启动成功一致；恢复失败保留插件、任务、Package 数据和明确错误。
- 恢复与手动启用不能重复创建实例或 Runner；完成后刷新 UI 和依赖快照。

必须覆盖：A→B 记录顺序为 A、B；B 恢复失败；可选依赖失败；循环；重复 reconcile；Runner 注册失败；状态与真实可调用性一致。

### Phase 4：跨插件调用权限边界

- 区分用户管理调用、宿主内部调用和插件运行实例调用。
- 调用方身份由服务端的真实运行实例或受控上下文确定，不信任请求字段中的插件 ID。
- 目标插件只能执行公开能力；调用方不能借目标插件获得额外设备、媒体或 Package 权限。
- Package、插件私有资源、路径和资源 ID 必须在授权上下文内；越界访问结构化拒绝且无副作用。
- 复用用户会话、PackageStore 和 Host API 权限机制，不建设新账号/RPC/权限系统。
- 文档明确 Native、REST、Frontend 三种 surface 的授权入口；纯前端导航不作为服务端业务能力。

如果当前第三方 WASM 尚无通用互调入口，先收紧 YAML/Video 现有调用链，再用最小受控入口测试扩展性。

### Phase 5：前端能力状态与可选依赖降级

- 统一以目标 Running、能力存在、版本兼容和当前上下文判断可用性。
- 探测失败时不得继续使用过期成功结果；新响应序号必须淘汰旧响应。
- `ready` 与 `hasAction()` 必须一致；YAML 停用、卸载、更新、恢复失败后视频制作入口即时降级，恢复后重新探测。
- 视频导入、播放、逐帧、标记、校准等不依赖 YAML 的基础功能保持可用。

必须覆盖：Running→停用、停用→恢复、探测失败、响应乱序、缺少 YAML 时视频基础流程。

### Phase 6：函数体系与设备生命周期回归

最终约定固定为：

```text
Package 函数库默认文件：automations/_function.yaml
扩展加载规则：_function*.yaml
前端：只编辑默认 _function.yaml
函数来源：插件函数 + 当前 Package 函数
```

- 默认函数库可创建、编辑、保存、加载；手动增加的 `_function_common.yaml` 等可加载。
- 普通自动化不误识别为函数库；函数库不进入任务运行选择器；同名函数明确报错。
- 调用名不随函数库文件名/目录变化；`find` 是单次匹配，`wait_find` 是等待轮询。
- `wait_find`、`wait_disappear`、`tap_template`、预算、取消、递归深度和参数校验回归。
- Package 导入/导出保留函数资源，不恢复旧 `functions/` 兼容目录。
- `stop_app` 只停止指定应用，不断开 ADB/scrcpy、不停止 YAML Runner；viewer 断开不等同于设备断开；空闲回收遵守设备模式和活跃消费者规则；录制/脚本运行中不被普通回收误断开。

### Phase 7：发布、E2E 与最终验收

按当前仓库脚本和 CI 执行：Rust fmt/clippy/test、前端 test/lint/build、Architecture Guard、Core shell boundary、官方插件构建/manifest/registry 校验、YAML/Keymap/Video 安装启停更新卸载、无签名第三方 WASM、本地 Package 导入导出和 dormant 数据保留。

视频纵向流程：

```text
连接设备 → 录制 → 停止 → Video Project → 精确逐帧/标记/校准
→ 定帧创建模板 → 离线匹配 → 生成并保存 YAML 草稿
→ 打开编辑器 → 显式选择真实设备验证
```

正式发布还需从指定提交构建三枚插件及 `sha256sums.txt`，确认无签名私钥/Registry proof 依赖，发布真实 GitHub Release，验证资产、SHA-256、安装、更新、卸载、404、哈希错误和版本不兼容路径。无法执行的真机、平台、性能或发布步骤必须写成 `NOT_VERIFIED`，不得用源码存在或单元测试通过替代真实验收。

## 3. 实施顺序、任务所有权与并行拆分

```text
Phase 0 基线
  → Phase 1 原生动作生命周期
  → Phase 2 依赖守卫
  → Phase 3 启动恢复
  → Phase 4 跨插件权限
  → Phase 5 前端降级
  → Phase 6 函数/设备回归
  → Phase 7 发布/E2E/文档
```

- 生命周期核心（Phase 1–3）由集成者统一修改 `server/src/extensions/service.rs`，避免多个 Agent 覆盖同一热点；先修复和补测试，再进入 Phase 4。
- 前端 Agent 独占 `web/src/components/video/yamlCapability.js` 与对应测试。
- 基线/验收 Agent 独占 `docs/evidence/plugin_lifecycle_closure_baseline.md`。
- 集成者负责 `extensions/mod.rs`、`gamer_yaml/actions.rs`、生命周期测试、跨插件上下文收口、总计划和最终报告。
- 每阶段必须记录实际 HEAD、修改文件、设计偏差、测试命令/退出码、数据迁移说明、剩余问题和下一阶段依赖。
- 不把未运行的测试标成通过；共用热点文件只由集成者写入。

## 4. 最终验收清单

- [ ] 原生动作执行前完成状态、能力、权限和上下文检查。
- [ ] 停用后没有新的未授权写入或设备操作，竞态安全且无死锁。
- [ ] 必需依赖的停用、卸载、版本切换和循环守卫正确；可选依赖降级不阻塞基础功能。
- [ ] 重启恢复按依赖顺序执行，状态、实例、Runner、UI 和任务一致。
- [ ] 跨插件调用不能伪造身份或越权访问其他 Package/插件私有资源。
- [ ] 前端能力状态与服务端调用门禁一致，失败和恢复无需重启整个 Gamer。
- [ ] `_function.yaml`/`_function*.yaml`、`find`/`wait_find`、便利函数和统一解释器回归通过。
- [ ] `stop_app` 不会意外断开设备或停止 Runner，设备空闲/录制生命周期回归通过。
- [ ] YAML、Keymap、Video、任务、Package、市场、构建和本地无签名插件流程有实际证据。
- [ ] 视频完整制作流程、正式 GitHub Release、平台和性能验证按实际结果记录；未执行项明确 `NOT_VERIFIED`。
- [ ] README、AGENTS、插件开发指南、YAML/插件权限文档、视频指南、发布说明和本计划与实现一致。

## 5. 执行记录

| 阶段 | 状态 | 实际 HEAD/证据 | 备注 |
| --- | --- | --- | --- |
| Phase 0 | DONE | `a5a175f`; `docs/evidence/plugin_lifecycle_closure_baseline.md` | 基线审计与问题复现完成 |
| Phase 1 | DONE | `server/src/extensions/service.rs`; service tests 17/17 | 原生动作先过生命周期门禁；每插件读/写调用闸门排空竞态 |
| Phase 2 | DONE | `server/src/extensions/service.rs`, `error.rs`; `cargo test --locked`: 641 passed, 6 ignored, 0 failed, exit 0 | 依赖方要求改与 provider active version 比较；补齐版本切换和非 active 旧版本卸载守卫 |
| Phase 3 | DONE | `server/src/extensions/service.rs`; `cargo test --locked`: 641 passed, 6 ignored, 0 failed, exit 0 | 启动恢复按必需依赖拓扑排序，失败降级并保留诊断 |
| Phase 4 | DONE | `service.rs`, `gamer_yaml/actions.rs`, `PLUGIN_API.md`; trusted caller test passed | REST 用户调用与插件互调分面；插件互调校验 Running、caller 契约和 Package Context |
| Phase 5 | DONE | `web/src/components/video/yamlCapability.js`; `pnpm.cmd test:run`: 64 files / 708 tests, exit 0 | 探测失败清空过期能力，响应序号淘汰乱序结果，`ready` 与 `hasAction()` 一致 |
| Phase 6 | DONE (automated) | `cargo test --locked`: 641 passed, 6 ignored, 0 failed, exit 0; `pnpm.cmd test:run`: 64 files / 708 tests, exit 0 | 函数库、YAML V1、设备/任务/视频相关自动化回归通过；真机行为见 NOT_VERIFIED |
| Phase 7 | PARTIAL | `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --locked`, 直接执行 `pnpm.cmd install --frozen-lockfile`（`web/`）成功：Already up to date，退出码 0，`pnpm.cmd test:run`, `pnpm.cmd build`, `tools/build-plugins.ps1` | Rust 三项均退出 0；前端 test/build 退出 0；`tools/ci-local.ps1` 的 web install 关卡单独失败，原因是脚本调用 `pnpm.ps1` 时发生命令解析异常；真实设备、GitHub Release、跨平台/性能链路未执行 |

### 5.1 本轮实现摘要

- 核心生命周期修改集中在 `server/src/extensions/service.rs`，没有引入第二套状态机、迁移层或兼容语法。
- 原生动作的 catalog lookup 已与有副作用的 dispatcher 分离；停用/卸载拿 per-extension write lease，调用拿 read lease。
- `call_extension_from_plugin` 是最小宿主内受控互调入口：caller 不从 JSON 推导，目标动作必须声明 caller，Package 写入必须与 `content_package` 一致；当前没有无契约 declarative 跨插件互调。
- 前端能力探测状态已改为失败关闭（fail closed），保留视频基础功能不依赖 gamer.yaml 的既有降级结构。
- 未新增数据迁移；Package 数据、旧版本归档和 dormant 插件目录均按现有 V1 语义保留。

### 5.2 验证记录与 NOT_VERIFIED

已执行：

- P1 后 `cargo fmt --all -- --check`：退出码 0。
- P1 后 `cargo clippy --all-targets --all-features -- -D warnings`：退出码 0。
- P1 后 `cargo test --locked`（`server/`）：641 passed，6 ignored，0 failed；退出码 0。
- 直接执行 `pnpm.cmd install --frozen-lockfile`（`web/`）成功：Already up to date，退出码 0；`tools/ci-local.ps1` 的 web install 关卡单独失败，原因是脚本调用 `pnpm.ps1` 时发生命令解析异常。
- `pnpm.cmd test:run`（`web/`）：64 files、708 tests；退出码 0。
- `pnpm.cmd build`（`web/`）：退出码 0；Vite 仅报告既有大 chunk warning。
- `tools/build-plugins.ps1` 使用临时输出目录：三枚官方 `.gplugin`、registry schema v2、无签名、SHA/staging 自检全部通过。

明确未验证：

- `NOT_VERIFIED: 真机 Android、ADB、scrcpy、WebRTC、录制和真实浏览器交互，包括宿主真实设备上的 stop_app、空闲回收、录制/脚本运行中断连保护和完整视频制作纵向流程`。
- `NOT_VERIFIED: Docker/NAT、跨平台运行和性能基准`。
- `NOT_VERIFIED: GitHub Release 创建、Release 资产上传、线上安装/更新/卸载/404/哈希错误链路`；本轮未进行外部发布操作。

详细结果见 `docs/evidence/plugin_lifecycle_closure_final.md`。

## 6. 参考实现文档

- `AGENTS.md`
- `docs/reference/YAML.md`
- `docs/reference/PLUGIN_API.md`
- `docs/guides/plugin-dev.md`
- `docs/plans/gamer_v1_simplification_plan.md`
- `docs/plans/gamer_v3_package_frontend_architecture_plan.md`
- `docs/plans/gamer_video_workbench_contracts.md`
- `server/src/extensions/service.rs`
- `server/src/extensions/gamer_yaml/actions.rs`
