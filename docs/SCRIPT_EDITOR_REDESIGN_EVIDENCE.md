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
