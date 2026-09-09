# 插件生命周期与函数体系收尾：独立基线报告

审计日期：2026-09-09（Asia/Shanghai）
审计 HEAD：`a5a175ff06013b3ef6cda9f328eb9ecd285cfc33`
工作树：审计开始时无未提交修改；审计过程中发现其他并行变更（未由本审计修改）：`docs/plans/gamer_plugin_lifecycle_function_closure_plan.md`、`web/src/components/video/yamlCapability.js`、`web/src/components/video/yamlCapability.test.js`。本报告是本审计唯一新增文件。
范围：只读核对收尾计划、生命周期 service、manifest/dependency、call 路径、函数体系及现有测试；未修改源码。

## 结论摘要

| 分类 | 基线判断 | 证据 |
|---|---|---|
| 已存在 | 插件状态机、必需/可选依赖解析、循环检测、Runner/UI 生命周期钩子、启动恢复入口、公开动作清单、`_function*.yaml` 合并及 `find`/`wait_find` 实现均已存在 | `server/src/extensions/service.rs:774-1050`；`server/src/extensions/manifest.rs:694-729`；`server/src/extensions/gamer_yaml/actions.rs:54-159`；`server/src/extensions/gamer_yaml/runner_adapter.rs:220-239`；`server/src/extensions/gamer_yaml/yaml_extension.rs:540-570` |
| 仍存在 | 原生动作授权顺序错误；依赖停用守卫版本匹配对象错误；启动恢复依赖持久化枚举顺序；`caller` 仅声明性；前端探测失败保留旧状态 | 见下方“需调整”证据 |
| 需调整 | Phase 1–5 的生命周期/身份/上下文/前端降级修复，以及针对性回归测试 | 下方每项均列出缺口与落点 |
| 未验证 | 真机/录制/视频完整 E2E、官方市场与 GitHub Release、Windows/Docker、并发竞态、跨插件受控调用、版本切换依赖守卫、pnpm 前端测试 | 本报告未将源码存在或局部单测通过视为验收通过 |

## 仍存在、需调整的证据

1. 原生动作先执行后做 Running 检查。`ExtensionService::call_extension()` 在 `server/src/extensions/service.rs:335-345` 先调用 `native_call_action(...)`，随后才 `snapshot_for()` 并判断 `Running`。因此停用/未启动目标可能先进入 `automation.save_draft` 或 `template.create_from_frame` 的副作用路径；动作实现确实会写 PackageStore（`server/src/extensions/gamer_yaml/actions.rs:231-296`、`405-474`）。现有 `video_draft::tests::native_call_action_gates_by_extension_and_action`（`server/src/extensions/gamer_yaml/video_draft.rs:600-648`）只测扩展 ID/动作清单，不测生命周期门禁或无副作用。

2. 依赖停用守卫比较了错误版本。`ensure_not_required_by_running()` 在 `server/src/extensions/service.rs:522-528` 对依赖声明 `dep.version_req()` 匹配的是 `snapshot.active_version()`，即运行中依赖方自身版本；计划要求比较被依赖目标的实际有效版本。`disable()` 与 `uninstall()` 均调用该守卫（`service.rs:837-840`、`1052-1064`）；`activate_version()` 仅拒绝目标自身 Running（`service.rs:745-771`），未见运行中依赖方兼容性守卫。现有 `required_dependency_gates_start_and_disable_guard`（`service.rs:1963-2038`）只使用 `^1.0.0` 的同版本场景，无法证明 A→B ≥2.0 下 B 2.0/3.0、版本切换等计划场景。

3. 启动恢复没有按必需依赖拓扑排序。`reconcile_startup()` 在 `server/src/extensions/service.rs:1011-1049` 从状态表收集旧 Running 后按 `HashMap`/读取结果枚举顺序逐个清为 Enabled 并 `start()`；未见提供方优先的排序或恢复批次。`start_with_context()` 有依赖门禁（`service.rs:875-897`），因此依赖方可能先失败并被降级，即使提供方随后可恢复。现有 `reconcile_startup_restores_running_extension_and_resumes_tasks`（`service.rs:1562-1664`）只测单插件恢复，未测 A/B 逆序、提供方失败、循环恢复或重复恢复。

4. 跨插件调用方身份与 Package 授权仍未落地。动作清单中的 `caller` 明确被定义为“声明性”字段，模块注释承认进程内没有可验证身份（`server/src/extensions/gamer_yaml/actions.rs:15-18`、`54-70`）。REST `POST /api/extensions/:id/call` 只接收 `action` 与任意 JSON `values`（`server/src/api/extensions.rs:153-172`、`202-208`），并将其直接交给 `call_extension()`；没有 caller、调用租约或受控插件上下文。原生保存动作从请求 values 读取 `package_id` 并据此构造 PackageStore 操作（`actions.rs:233-262`、`407-447`），当前证据不足以证明调用方只能写其授权 Package。REST 会话鉴权存在，但只证明管理入口受保护（`server/src/api/mod.rs:327-334`），不等于插件身份/资源授权。

5. HEAD 中前端能力探测会保留过期成功状态。HEAD 的 `web/src/components/video/yamlCapability.js:41-52` 在 `refresh()` 失败时只注释并保持 `ready/actions` 上一次值；该模块还以 `rep.running === true` 设置 `ready`，但 `hasAction()` 只按动作名称集合判断（HEAD `yamlCapability.js:35-39`）。现有 `web/src/yaml-capability.test.js:41-53` 锁定了“网络抖动保持上一次判定”的旧语义，未覆盖停用/卸载/更新后的异步竞态。审计过程中出现的未提交候选修复已改为清空能力并加入版本号竞态保护（工作树 `web/src/components/video/yamlCapability.js:37-73`，配套新测试 `web/src/components/video/yamlCapability.test.js`），但它不属于当前 HEAD，本报告不将其标为已验证。

## 已存在的函数体系证据

- 函数库路径识别和 `_function*.yaml` 合并已有实现及单测：`server/src/extensions/gamer_yaml/resources.rs:42-48`、`runner_adapter.rs:220-239,464-600`。
- 函数库不能作为任务运行目标的门禁已有：`server/src/extensions/gamer_yaml/timer_yaml.rs:132-135,302-305`。
- 原生函数目录与 `wait_find` 实现已有：`server/src/extensions/gamer_yaml/native_funcs.rs:57-372`、`yaml_extension.rs:352-372,550-570`。
- 前端默认 `_function.yaml` 创建/编辑、拆分库只读语义已有：`web/src/components/console/useConsoleScriptRunner.js:83-109,292-323,363-390`。

上述证据只能标为“已存在”；本轮尚未执行完整函数、导入导出、设备生命周期及视频流程验收。

## 已执行的轻量检查

| 命令 | 结果 |
|---|---|
| `git rev-parse HEAD` | `a5a175ff06013b3ef6cda9f328eb9ecd285cfc33` |
| `git status --short` | 审计开始时为空；结束时另有并行变更：计划文档、`yamlCapability.js`、`yamlCapability.test.js`，以及本报告 |
| `cargo test --manifest-path server/Cargo.toml required_dependency_gates_start_and_disable_guard -- --nocapture` | PASS；1 passed，617 filtered |
| `cargo test --manifest-path server/Cargo.toml reconcile_startup_restores_running_extension_and_resumes_tasks -- --nocapture` | PASS；1 passed，617 filtered |
| `pnpm --dir web exec vitest run src/yaml-capability.test.js --reporter=dot` | NOT_VERIFIED；Corepack 拒绝环境 pnpm `11.24.0`，仓库声明 `11.23.0`；并行新增测试亦未执行 |

未运行全量 Rust/前端测试、构建、真机、市场、发布或长时间测试。

## 下一步验收建议

先修复并补测上述 1–5 项，尤其是“原生动作状态检查必须先于执行”“依赖守卫比较目标实际版本”“恢复按依赖顺序”三项；随后重新记录 HEAD、定向测试命令和未验证项，再进行 Phase 6–7 的真实环境验收。
