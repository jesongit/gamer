# 脚本编辑器重构 · 阶段证据记录

> 依据《脚本录制与可视化编辑器重构计划》（SCRIPT_EDITOR_REDESIGN_PLAN.md）逐阶段追加。
> 说明：计划文件当前版本测试计划止于 §16.4，无 §16.8「fixture 与证据管理」小节；本文件按
> 阶段任务书给定的验收记录要素（命令 / 结果 / 已有失败清单 / 审查人 / 结论）建立「阶段 0」节，
> 后续阶段沿用同一节模板追加，每阶段一节，只追加不回改。

## 阶段 0：冻结契约和测试样例（2026-08-29）

### 交付物清单

| 交付物 | 位置 |
|---|---|
| golden 合法 fixture ×12（v01~v12，含 2 个多文件样例的辅助文件） | `server/tests/fixtures/script_v2/*.yaml` |
| 非法 fixture ×9（i01~i09） | 同上 |
| golden/期望 JSON ×21 + README 索引 | `server/tests/fixtures/script_v2/*.golden.json`、`*.expected.json`、`README.md` |
| 契约文档（五方对照、错误码五域、文字契约、解析层选型） | `docs/SCRIPT_EDITOR_CONTRACT.md` |
| 服务端契约测试（事件树→Model 断言 + 最小预校验 + 单引号样式 PoC） | `server/tests/script_v2_contract/{main.rs,yaml_loader.rs,model.rs,precheck.rs}` |
| 前端 fixture 副本（yaml/ 与 json/ 两目录，逐字节一致） | `web/src/script-editor/__fixtures__/` |
| 前端契约断言（js-yaml 读 YAML 断言 golden + psig1 双实现 + 漂移测试） | `web/src/script-editor/__fixtures__/fixtures.test.js` |
| vitest include 扩展（追加 `src/script-editor/**/*.test.js`） | `web/vitest.config.js` |
| 生产数据只读备份 + SHA256 清单 | `backups/stage0-data/`（含 MANIFEST.sha256，目录已入 .gitignore 不入库） |

覆盖矩阵对照任务书：最小脚本(v01)、全动作(v02)、函数库+return(v03)、七类参数全默认(v04)、
七类参数全必填(v05)、嵌套 if/loop(v06)、match 紧凑缩进+else+timeout(v07)、color 分支(v08)、
脚本 call 带 args(v09)、跨文件 func 调用(v10)、录制输出 find+match→swipe(v11)、
定时任务参数快照+param_signature(v12)；旧顶层格式(i01)、params 未加单引号(i02)、
默认值类型错误(i03)、match 候选重复(i04)、函数路径穿越三形态(i05)、call 自引用环(i06)、
未知顶层键(i07)、`- else` 写进候选(i08)、空默认值(i09)。

### 基线命令与结果

| 命令 | 结果 |
|---|---|
| `cd server && cargo test` | **216 passed / 0 failed / 2 ignored**（单元测试 209 + 集成 `script_v2_contract` 7；改动前后均无失败用例） |
| `cd web && pnpm test:run` | **168 passed / 0 failed**（11 个测试文件，其中新增 `script-editor/__fixtures__/fixtures.test.js` 16 例；vitest include 已扩展，environment 保持 node） |
| `cd web && pnpm build` | **成功**（`✓ built in 1.76s`，产物输出 `server/web-dist/`，该目录已 gitignore） |

其中新增 `script_v2_contract` 的 7 个用例：

1. `poc_scalar_style_is_preserved_by_saphyr` — PoC：saphyr-parser 事件层可区分 `SingleQuoted`/`Plain` 标量样式（params「整条单引号」契约的可行前提）；
2. `serde_yaml_loses_scalar_style` — 反向动机：serde_yaml 0.9 反序列化后单引号与无引号不可区分；
3. `golden_valid_fixtures_match_model` — 12 组合法 fixture 语法解析 → 拟议前端 Model JSON 与 golden 逐字段断言（match 紧凑缩进经 v07/v11 回归）；
4. `task_args_snapshot_shape` — v12 任务快照形态：args 全量类型化 + param_signature 由服务端复算一致；
5. `invalid_fixtures_are_flagged_by_precheck` — 9 个非法样例全部被最小预校验以期望的 code/step_path/field 结构化拒绝；
6. `precheck::tests::function_library_precheck_entry` — parse_function_file 入口最小验证；
7. `precheck::tests::function_library_params_quote_style` — 函数库 params 同受单引号约束。

### 已有失败清单

无。阶段 0 改动前后 `cargo test` / `pnpm test:run` / `pnpm build` 均无失败用例（既有
`docs/PITFALLS.md` 中记录的计时敏感偶发红用例本次未出现，不代表排除其偶发性）。

### 数据备份（只读复制）

- 源：`server/data/`（618 KB，49 个文件：`gamer.db` + `gamer.db-shm` + `gamer.db-wal` + `com.miHoYo.hkrpg` 与 `com.tencent.nrc` 两分区的 tmpl/yaml）；目标：`backups/stage0-data/`；纯 `cp -r` 复制，未改动源文件。
- 清单：`backups/stage0-data/MANIFEST.sha256`（49 条，SHA256，相对备份根路径）。
- 关键校验值摘要：
  - `gamer.db` → `9fac8ae1253b6e74e51966efcbbd2a58bb9413c58809dc9d7ecc05f5ea0caad2`
  - `gamer.db-wal` → `b70b07fcab92a6b8098232093bf2813247af1e9457f8db7ff3e5499560f039f3`
  - `gamer.db-shm` → `f76fa073cc9a1e146b2b45cf7f0df3d8b35b11ae7933a3968d0e0e696c696469`
- `backups/` 已追加进 `.gitignore`（备份不入库；回滚时需同时恢复程序版本与该数据快照，见计划 §19）。
- 声明：本次发布（新目录 `yaml/func/tmpl` + 新语法）**不会加载旧格式文件**，不做运行时迁移或回退（plan §2/§19）；旧分区数据仅以本备份形式留存。

### 解析层选型

**saphyr-parser 0.0.12**（dev-dependency，阶段 2 实装时转 `[dependencies]`）——事件级 + `Span` + `ScalarStyle` 齐备、YAML 1.2、零拷贝、yaml-rust 后继维护；serde_yaml 0.9 丢样式无法校验单引号契约（PoC 已固化），yaml-rust2 0.12 无事件 Span 不选，正则预扫描方案不可靠弃用。完整论证见 `docs/SCRIPT_EDITOR_CONTRACT.md` §2。

### 过程踩坑（已按仓库规则记入 docs/PITFALLS.md）

- js-yaml `load()` 的 plain object 对纯数字色键（`'123456'`）按整数形键重排，映射形态丢 color 候选顺序 → 契约将 color `expect` 冻结为有序列表（单键映射项，与 match 候选同构）。

### 审查人

主线编排复核（非实现者）：复核 CONTRACT.md 与 fixture 索引；复跑 cargo test（216+7 全过）、pnpm test:run（168 全过）、pnpm build 成功

### 结论

通过


## 阶段 1：目录和资源 API

### 交付
- ca15360 feat(data): 三套路径解析（yaml/func/tmpl，拒绝穿越/反斜杠/空段/..）、func/ 函数库存储、浅校验（顶层键合法函数名）、version（SHA-256 前 12 hex）、原子写入（临时文件+rename）
- fb92305 feat(api): /api/functions CRUD（GET?pkg=/POST/GET/:id/PUT/:id/DELETE/:id，id 整体 %2F 编码）、脚本 GET/:id 与 expected_version 409 冲突、导入 dry-run（无 confirm=1 返回 {scripts,functions,templates} 分类报告，invalid 整体拒绝）与导出 zip 三目录布局（兼容旧布局）

### 自动化命令与结果
- cargo test：238 passed / 0 failed / 2 ignored（主线在 HEAD 复验 237+script_v2 中间态亦全绿）；cargo check 0 warning
- 并行验证：S1 在独立 worktree（HEAD+自身 diff）连续两轮全绿

### 人工用例与结果
- API 列表只返回 yaml/ 脚本、函数不进运行/任务选择器：router 集成测试锁死，通过

### 已知限制
- func/ 嵌套子目录：resolver 支持、save/list/导入暂扁平；.yml 兼容仅脚本侧；dry-run 对攻击形态（zip-slip/像素炸弹）整体 400 不进报告（安全优先）；resolve_template_path 生产调用点待阶段 2 接入（#[allow(dead_code)]）
- 回归：Console.vue 导入确认弹窗读旧报告字段（dry.conflicts），已列修复任务

### 审查人
主线编排复核：HEAD 复跑 cargo test 全绿、抽查路由与 resolver 测试

### 结论
通过

## 阶段 3（前半）：编辑器核心模型层

### 交付
- 2c6382d feat(web): model/codec/schema/diagnostics（TS）+ fixture 逐字节往返（14 合法 fixture serialize(parse(x)) 与原文一致，未改 fixture）
- 3d81ef4 feat(web): commands（事务栈）/validation/factories/selection + 116 新测试

### 自动化命令与结果
- pnpm test:run：284 passed / 0 failed（既有 168 零回归）；pnpm build 成功；tsc --noEmit 干净

### 已知限制
- $ 前缀 text 字面量与 ref 语法同形，YAML 层不可区分（一律解析为 ref）；config 未知键暂借 step.field.* 错误码；time 单位大小写不敏感、存储原样
- color else 位置、字母/纯数字色值引号、args 实参引号等隐性规范由 fixture 冻结（已回写 CONTRACT 澄清节）

### 审查人
主线编排复核：复跑 pnpm test:run / build 全绿

### 结论
通过（模型层部分；组件层归阶段 3 后半）


## 阶段 2（前半）：script_v2 严格装载校验与规范序列化

### 交付
- 57c1964 feat(engine): script_v2 模块（model/loader/params/validate/serialize/error，4747 行）——saphyr 事件级样式保留、params 整条单引号强制、分层校验、静态调用环与 32 层深度、规范序列化、ResourceProvider trait + 内存实现

### 自动化命令与结果
- cargo test：269 passed / 0 failed / 2 ignored（主线 HEAD 复验）

### 审计发现
- 前任 Agent 被取消时 mod.rs 漏挂 serialize/tests 两模块（从未参与编译，'235 全绿'不含其 33 个单测）；审计 Agent 修复挂载并补 12 组 fixture 字节级往返与 resource.tmpl.ambiguous 测试

### 已知限制
- ref.func.missing_args 并入 param.args.*（call/func 单一绑定路径）；ref.template.ambiguous 用 resource.tmpl.ambiguous；config.* 独立错误码待执行引擎期定夺

### 审查人
主线编排复核：HEAD 复跑 cargo test 全绿、审计清单逐项核对

### 结论
通过（前半；执行引擎与 RunManager 归后半）

## 阶段 3（后半）：编辑器组件层

### 交付
- d3196af feat(web): StepCard（17 类）/CellEditor（七类类型化控件+值参切换）/ParamEditor/ConfigEditor + commands.ts unwrap 补丁（Vue reactive 不可 structuredClone）
- 4bfc495 feat(web): StepCanvas（锚点/选中/面包屑/诊断定位联动）/BranchContainer（一层内嵌+深层专注视图）/AddStepPanel/ErrorSummary/YamlPreview

### 自动化命令与结果
- pnpm test:run：381 passed / 0 failed（+97）；pnpm build 成功；tsc --noEmit 干净（主线复验）

### 已知限制
- 函数库逐函数 params 编辑待阶段 4/5 扩展命令栈；拖动排序占位（上移/下移按钮已可用）

### 审查人
主线编排复核：复跑双端门禁全绿

### 结论
通过（阶段 3 整体完成）

## 附：回归修复

- 5e01bd5 fix(web): 适配分区快照 dry-run 导入报告并恢复覆盖二次确认（阶段 1 报告形态变更的连带回归；7 新用例；含于 381）


## 阶段 2（后半）：执行引擎与统一 RunTarget

### 交付
- 549ccd5 feat(engine)!: 严格 AST 执行引擎替换 v1 + 统一 RunTarget 运行接口与函数测试端点（单笔合并：engine/api/run_manager/scheduler 编译强耦合无法拆分自洽提交）——exec.rs 17 类步骤逐类执行（find 恒点中心/verify 两击/block 有序、match 每轮单帧有序候选不点击+绑定后重复截图前拒、color 有序判色容差 30、if 布尔严格、loop+10 万步 guard、call 压栈恢复、func 继承 config 走完默认 true、throw 跨链、return 退函数、$name 作用域栈、call+func 合计 32 层）；snapshot.rs 运行源码快照+懒解析缓存；RunManager Script/Function 双目标统一互斥/取消/恢复；API：run 改 {device_id,start_index?,args?}→202 {run_id,state,resolved_args}、新增 POST /api/functions/:id/run、删 v1 func 位置实参

### 自动化命令与结果
- cargo test：271 passed / 0 failed / 2 ignored（新增 39 例：exec 25 语义+12 golden 端到端+函数目标互斥取消+router 用例）；cargo fmt --check 干净、0 warning（主线复验）

### 语义偏差（合理取舍，知悉）
- match 绑定后重复：静态查字面量+运行期查 Ref 解析后，两层互补；resolved_args 提前于阶段 5 进入 202 响应（前端展示用）；loop times:0 沿 v1 语义=无限

### 审查人
主线编排复核：HEAD 复跑 cargo test 全绿

### 结论
通过（阶段 2 整体完成）

## 阶段 4：替换两套编辑入口

### 交付
- 488a35f feat(web)!: 控制台脚本区换壳为共享可视化编辑器紧凑外壳（useScriptEditorShell/useFunctionLibrary composable、SaveConflictModal/ScriptSummary、Alt 类型化工厂、旧文本机制全删）
- c4a80ba feat(web)!: 独立脚本页重构为全屏三页签可视化外壳并纳入函数库（函数级 params 命令、codec 空备注对称修复）

### 自动化命令与结果
- pnpm test:run：396 passed / 0 failed（+15）；pnpm build 成功；webrtc-lifecycle/api-intercept/auth 零改动通过（主线复验）

### 修复的真 bug
- Console cancelEditScript 对 reactive 包装取 .value 静默失效；ScriptRunner 覆盖运行起点选择；codec 序列化端可产出空备注参数串但解析端拒绝（往返必挂）

### 迁移矩阵
- §10 对账见汇报与代码；占位两项：模板页签只读列表+跳转（模板完整能力保留在 Console）、测试函数按钮（阶段 5 对齐 RunTarget+args）

### 审查人
主线编排复核：复跑双端门禁全绿、抽查提交 diff

### 结论
通过


## 阶段 5：参数、函数和任务全链路

### 交付
- aa696bb feat(scheduler)!: 任务持久化参数快照与签名过期门禁（store 加 args_json/param_signature 两列，旧库 ALTER 兜底；task_params 四态门禁+rebind；调度/立即运行全量快照传参；409 param_signature_conflict+reconfirm；GET /api/tasks/:id 详情；日志不记参数值）
- 10b7b14 feat(web): 运行与函数测试参数表单接线（ParamsForm 七类三态、RunParamsModal、useRunArgsFlow、400 诊断字段级回填、resolved_args 来源标注、建议缓存不遮蔽默认值、测试函数激活+「▶测试」start_index）
- 515b698 feat(web): 定时任务参数快照表单与过期重新确认（三列对比表+reconfirm、param_stale 徽标与禁用、启停携带原快照）

### 自动化命令与结果
- cargo test：287 passed / 0 failed / 2 ignored（+16，含 text 值日志防泄露断言；主线复验）
- pnpm test:run：433 passed / 0 failed（+37）；pnpm build 成功（主线复验）

### 契约缺口（实现期冻结）
- param_signature_conflict 采用 snake_case（CONTRACT §5.2 dot 命名空间未列此码）；psig1 实际在 CONTRACT §4.5；409 体 reason/expected/actual/task_id 与 GET /api/tasks/:id 为服务端扩展；reconfirm:true 不带 args 语义=存活参数保留原值+新参数取当前默认值

### 已知限制
- 降级路径：旧程序读新库可运行，但旧程序 upsert 会把两新列写回 NULL（需新版重新保存任务）；回滚说明已写入提交信息与代码注释

### 审查人
主线编排复核：复跑双端门禁全绿

### 结论
通过


## 阶段 6：录制模式

### 交付
- 9566840 feat(web): 录制核心服务（web/src/recording/：gesture 分类阈值 max(8,min×0.005)/600ms、crop 自动 50×50 与 100×100/union+25px 边界数学、queue 单调 seq 占位保序+失败重试/丢弃/坐标降级、service 状态机 idle→recording→stopping 且按下先冻结后透传）
- b4e7235 + ce4f9af feat(web): Console 录制接线（useRecording 接线层、⏺ 录制按钮+状态栏、投屏录制分支优先于 Alt、RecordingCropPanel 全幅底图二次裁切、失败草稿不漏步、离开保护、ScriptRunner 上传中锁画布）
- 主线补: vitest include 纳入 src/recording（E1 按边界约束不能改共享配置）

### 自动化命令与结果
- pnpm test:run：534 passed / 0 failed（454 接线+80 核心全量收集）；pnpm build 成功（主线复验）

### 与 §11 的偏差（实现期冻结）
- `#` 搜索区后缀暂由前端拼接（现 POST /api/templates 仅收完整 name；§11.7 要求服务端组合——归阶段 7 服务端补 short_name+region 参数后切换）
- setCropping 迁移态未用（语义等价 pending→uploading→ready）；暂停换锚点=停止后可换、上传中锁画布等价实现
- 冻结失败（黑屏）→ 失败草稿「画面不可用」，触控仍透传，不假报成功

### 真机验收注意（阶段 8 待用户确认）
- 多指第二指 window 捕获透传的浏览器兼容；同步 toDataURL 冻结 1080p 约 30-60ms 的手感；高分屏/旋转以按下帧尺寸为基准；长按>600ms 生成失败草稿属设计

### 审查人
主线编排复核：复跑门禁全绿（含录制核心测试收集修复）

### 结论
自动化通过；真机录制回放待用户确认（L5 待验）


## 阶段 7：删除旧路径并收口文档

### 交付
- 45d17ff feat(api)!: 模板上传改收 short_name+region（[x1,y1,x2,y2] 相对坐标），服务端组合完整名（×1000 编码同现有），短名冲突 409 改名不覆盖；旧 {name} 形态兼容
- fca868b feat(web): 录制上传直传 short_name+region，删前端 composeRegionName（§11.7 收口）
- e60addf refactor(api)!: 删除 /api/op-templates 端点与 [op_templates] 配置结构；导入浅校验收紧为只认 params/config/steps（v1 语法记 invalid，与严格装载对齐）
- 3107018 refactor(web)!: 删除 script-language/（validate/line-map/fixtures）、op-template.js、op-template.test.js，-1590 行
- d1919e5 docs(yaml)!: 重写 docs/YAML.md（617 行：目录/params/17 步/绑定/运行入口/录制形态/诊断/v1 差异对照）+ AGENTS.md 同步；14 个 YAML 示例全部通过 script_v2 严格装载验证（临时 doccheck 测试验证后移除）
- 主线补：vitest include 移除 script-language 死条目、移除 recording 补收集

### rg 残留扫描（最终态）
- op_template/normalize_top/run_func/start_step 生产代码 0 命中；`$N`/until/cond 仅剩新语法合法使用、显式拒绝测试与非语法命中；全清单见 F1 汇报归档于提交信息

### 最终门禁（主线 HEAD 复验）
- cargo fmt --all --check：干净；cargo test：289 passed / 0 failed / 2 ignored
- pnpm test:run：458 passed / 0 failed（32 文件）；pnpm build：成功；git diff --check：干净
- cargo clippy --all-targets --all-features：9 条 warning（全部为存量，本重构未新增；RC 前如需清零另行立项）

### 已知取舍
- 旧 {name} 上传形态仍可覆盖写（兼容 Console 框选替换），理论上可绕过短名唯一制造歧义候选；如需收口后续把 name 形态也按基名判冲突
- 契约与实现的差异清单见 d1919e5 汇报（run 端点形态/色值引号澄清/完整值引用等，文档均以实现为准）

### 审查人
主线编排复核：HEAD 复跑全部门禁；抽查三路提交 diff 与 rg 清单

### 结论
通过（阶段 7 完成；「真机录制/回滚演练后再宣布发布完成」一项归阶段 8 待人工）


## 阶段 8（自动化部分）：RC 构建

### 产物与环境
- target/release/gamer-server.exe（42.9MB，HEAD 3366241）；web 产物 server/web-dist/（pnpm build ✓）
- rustc/cargo、node v24.14.0、pnpm 11.x、Windows 10.0.26200 x64

### 门禁（HEAD 复验全绿）
- cargo fmt --check ✓；cargo test 289/0/2；pnpm test:run 458/0（32 文件）；pnpm build ✓；git diff --check ✓
- clippy：9 条存量 warning（未新增）

### 待人工（需用户确认后执行）
- §19.1 发布前检查：调度暂停/备份核对/新格式分区快照 dry-run 人工确认
- §17.3 A~E 端到端剧本 + §16.6 真机矩阵（设备已可用）
- §19.3/19.4 回滚演练
- 按仓库规范打破坏性版本标签


## 阶段 8（收口）：§20 核对清单审计、边缘修复与最终复验（2026-08-30）

### §17/§20 只读审计结论

按计划 §20 七条核对清单与 §17 验收标准中可静态验证条目逐条审计（独立只读 Agent，file:line 级证据）：
17 种步骤类型在服务端 AST/loader/白名单、前端 model/factories/kinds、exec 引擎、docs/YAML.md 四方完全一致；
前端仅剩 script-editor/codec.ts 一处 YAML 解析/序列化（script-language 已删、守卫测试在位）；
§10 迁移矩阵两页逐项落点（运行/停止/冲突/恢复/从步骤运行/结构化跳转/409 版本冲突）；
录制 Alt 三分作用域与 §11.2 文案逐字一致；点击→find、滑动→match{swipe+else throw+timeout 30s}
且有测试断言「绝不能生成 find→swipe」；match 紧凑缩进 v07/v11 双向逐字节往返；
无 `$N`/`normalize_top` 残留、目录即类型无内容推断。审计结论：8 条通过、1 条基本通过（参数规则），
另发现 4 处边缘宽严差异，全部当轮修复：

| # | 问题（审计定位） | 修复 | 提交 |
|---|---|---|---|
| 1 | 参数备注空段：codec 解析宽容 → validation 无保存前拦截，可产出服务端必拒的 `- text:tag:` | validation.ts/ParamEditor.vue 补 param.decl.format 保存前诊断 + 行内提示（解析层保持宽容仅为编辑中间态） | 480eada |
| 2 | text 默认值引号：schema.ts 强制双引号，服务端/文档为可选（更严方错位） | schema.ts 改与服务端同构：双引号可选、镜像 unescape_double_quoted 反转义；序列化恒带引号不变 | 480eada |
| 3 | 录制入口未排除脚本运行中（计划 §11.1） | useRecording.js available 增加运行中守卫 + 「脚本运行中不可录制」提示 | 480eada |
| 4 | key 枚举：服务端默认值/args 仅非空校验，运行期未知键 warn+keycode 0 静默降级（计划 §6.2「保存时能过→运行时能用」被破坏） | params.rs KEY_NAMES+is_valid_key 四层拦截（默认值/args/步骤字面量/coerce），exec.rs 未知键改步骤失败；与前端 KEY_ENUM、YAML.md §5.1 三方核对一致 | 25b6363 |

容错取舍保留（知悉即可，非缺陷）：codec 对 color 映射内旧位 `else` 静默迁移（服务端拒载，
仅影响手写旧 YAML 的往返体验，保存即规范形态）；服务端 keycode 按 u32 收紧（超 u32 数字串
前端能过、服务端拒），方向为更安全一侧。

### 最终门禁（修复后 HEAD 复验）

- cargo fmt --all --check 干净；cargo test **294 passed / 0 failed / 2 ignored**（289 基线 + key 枚举 5 新增）
- cargo clippy --all-targets --all-features：9 条 warning 全部存量（7 独立 + 2 汇总），零新增；cargo check 0/0
- pnpm test:run **472 passed / 0 failed**（33 文件，463 基线 + 前端修复 9 新增）；pnpm build 成功
- git diff --check 干净
- 负载偶发说明：并行 release 重编的高负载窗口内 api::auth 计时敏感用例一次出现 7 红
  （`session_sliding_idle_expires_and_renews` 等，1s sleep 断言），负载解除后全量复跑全绿——
  与 PITFALLS 既有「session_lifecycle_absolute_and_sliding 负载误报」同族，已把第二条用例名补入该条

### RC 产物（修复后重建）

- `server/target-rc/release/gamer-server.exe`：42,864,415 B，SHA256
  `a0bc04953bb9531b7f39ef81972cb54d66d2d0cbda200d7af3a28ddcaeead580`
- 构建自含 25b6363+480eada 修复的工作树；`target/release/` 原位产物因正式服务运行中锁定无法重链
  （os error 5），故用独立 CARGO_TARGET_DIR 构建——**部署时以 target-rc 产物为准**；
  前端产物 server/web-dist/ 已随 pnpm build 更新（运行时磁盘托管，不嵌入 exe）
- `server/target-rc/` 已入 .gitignore（6cf8cf7）

### 真机证据（server/1.2026-08-29 运行日志）

- 手动脚本运行 `com.tencent.nrc/verify_a.yml`（七类参数 + if 分支 + $name 引用）×3 次 state=Success
- 函数测试 `com.tencent.nrc/verify_lib.yaml#check_flag` ×2 次 state=Success（RunTarget=Function 链路真机可用）
- WebRTC 信令/viewer 注册正常；§16.4 真机矩阵的「运行脚本时 hit/miss/tap/swipe 可视化」在该时段会话内进行
- 上述两脚本/函数库已入库（6cf8cf7）作为验收产物
- 无记录项：录制上传（日志无 upload/record 事件，服务端不逐条记 API）、定时任务实际调度（scheduler started
  但期间无 cron 触发）——**真机录制回放与任务调度实跑仍待人工确认**；§19.1 发布前检查、§19.3/19.4
  回滚演练、破坏性版本标签同前待人工

### 审查人

主线编排复验：三路并行 Agent（Rust 门禁 / §20 审计 / Web 门禁+RC）交叉复核，修复由两个并行 Agent
落地后主线审查 diff、复跑全部门禁

### 结论

通过——计划阶段 0～8 自动化与静态验收全部完成，§20 核对清单七条全勾；
发布宣布仍以「真机录制回放 + 任务调度实跑 + 回滚演练 + 版本标签」四项人工签核为准（§17.10/§19）。
