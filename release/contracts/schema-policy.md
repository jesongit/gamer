# GameBot DB/文件 Schema 兼容与回滚承诺契约（ARC-004）

> 状态：**冻结**（批次 0 契约；变更须按 §8 与 DATA 轨代码、manifest 字段同步提交）
> 编制日期：2026-08-31
> 依据：`docs/plans/AUTO_UPDATE_DEVELOPMENT_PLAN.md` §6.6/§6.7/§6.8/§11.3/§15/§17.2（ARC-004 两个 checklist 项）；目录属主见 `docs/guides/UPDATE_CONTRACT.md` §1，本文即其 §6 文件地图登记的 `release/contracts/schema-policy.md`
> 现状锚点：`server/src/store.rs`（`SCHEMA_VERSION=1`、`ensure_schema`、`apply_schema_migrations`、`validate_schema_v1`）——本文规则与该实现现状一致，后续迁移框架（DATA-001/003）不得偏离

## 1. DB schema 版本规则

- `PRAGMA user_version` 是数据库 schema 的**唯一权威版本标记**；不引入 `schema_migrations` 表，不用文件名/旁路标记推断版本。
- 当前 **v1 是唯一基线**（`SCHEMA_VERSION=1`）。v1 = 四表 `devices/tasks/logs/scheduled_runs` + 索引，结构校验由 `validate_schema_v1` 完成。判定路径（启动与 maintenance CLI 共用同一实现）：
  - **新库**（文件不存在）：直接创建 v1 DDL 并写 `user_version=1`，校验后开放业务；新库若意外带非 0 版本 → 报错拒绝。
  - **`user_version=0`（无版本旧库）**：**明确拒绝启动，不自动补齐、不改写任何数据，不存在 migration 0**；错误信息指引"备份后删除 gamer.db 重建"。
  - **`user_version=1`**：结构校验通过后开放业务（no-op）。
  - **`user_version>1`**：进入唯一显式迁移入口 `apply_schema_migrations`；当前实现直接拒绝（expected schema v1），后续版本在此挂编号迁移。
- 后续迁移从 **v1→v2 起逐级编号**执行。schema 只前进不后退：**不存在 down migration**，降级兼容完全由 §3 的读取范围表达。

## 2. 迁移规则

- 每个迁移有唯一编号 `v(n-1)→vn`，在二进制内静态注册；禁止运行期动态拼装迁移来源。
- 每个迁移在**单个 SQLite 事务**内完成全部 DDL + 数据修复 + `PRAGMA user_version=vn` 推进；事务保证失败整体回滚，user_version 不变，可安全重试（幂等安全）。
- **user_version 不越级**（计划 §11.3）：一次只推进一级，逐级执行到 target；已达标再跑 = no-op。
- 迁移只在**离线单写者**上下文执行：server 启动早期（业务写/scheduler/设备扫描/watchdog 均未启动）或 maintenance CLI（§7）；候选升级场景在 activation gate 阶段 1（计划 §6.8），committed 前无业务写入。
- 迁移前的数据快照由 **launcher 编排**（快照格式/落点/hash/同卷 staging 归 LCH-011 冻结，本文不重复定义）。server 侧契约：migrate 正常结束时不残留未合并写事务（执行 `wal_checkpoint(TRUNCATE)`），主库文件自洽，不依赖 `-wal/-shm` 即可被快照或旧 binary 打开。
- 迁移内禁止顺手做静默数据丢弃或语义改写；破坏性数据变换必须随该迁移编号在 release notes 与本契约变更中显式声明。

## 3. binary 兼容声明（min/max/target）

每个 server binary 声明三个编译期常量（随 VER-002 构建信息注入并可在 diagnostics 输出）：

| 常量 | 含义 |
|---|---|
| `min_read_schema` | 本 binary 可打开并继续迁移的最低 user_version |
| `max_read_schema` | 可打开的最高 user_version；**高于此值拒绝启动** |
| `target_schema` | 启动/迁移完成后数据库达到的版本 |

冻结约束：`min_read_schema ≤ target_schema ≤ max_read_schema`；当前形态取 `target_schema = max_read_schema`（启动即迁满）。行为判定（启动路径与 §7 CLI 共用）：

- `user_version > max_read_schema` → **拒绝启动**，不改写数据；错误信息必须含实际 user_version、支持范围 `[min, max]` 与 target，可诊断（DATA-003）。
- `min_read_schema ≤ user_version < target_schema` → 先逐级执行缺失迁移到 target，**之后才开放业务**（路由、scheduler、设备扫描均不得先于迁移完成启动）。
- `user_version = 0` 永远拒绝（unversioned 不属于任何兼容区间，与 min 取值无关，见 §1）。

取值表：

| Release | min_read_schema | max_read_schema | target_schema | 说明 |
|---|---|---|---|---|
| v0.1.0（当前代码） | 1 | 1 | 1 | v1 唯一基线：=1 校验后开放；=0 / >1 拒绝 |
| v0.2.0（假设示例） | 1 | 2 | 2 | 引入迁移 1→2；可从 v1 库升级；DB=2 正常打开；DB≥3 拒绝 |

manifest 对应：`release.data_schema` = `target_schema`；`release.rollback_floor` 语义见 §6。

## 4. 兼容表

**硬规则：数据库比 binary 新（`user_version > max_read_schema`）→ 拒绝启动。** 任何 release 不得引入静默降读、临时写旧 schema 或"带病运行"路径。

| DB user_version | v0.1.0 binary（min=1 / max=1 / target=1） | v0.2.0 binary 假设例（min=1 / max=2 / target=2） | 能否直接升级 v0.1.0→v0.2.0 | 能否降级回旧 binary |
|---|---|---|---|---|
| 1（当前唯一基线） | 打开并校验，开放业务（no-op） | 迁移 1→2 后开放业务 | **允许**（候选 activation gate 内完成迁移） | DB 仍为 1：允许直接切回（无损）；已迁至 2：**拒绝**，须恢复快照 |
| 2（预留示例） | **拒绝启动**（DB 新于 binary） | 打开并校验，开放业务（no-op） | —（已是 target） | 切回 v0.1.0 **拒绝**（超其 max=1）；仅允许恢复升级前快照并明确提示丢失升级后数据（§6） |
| 0（无版本旧库） | 明确拒绝，不改数据 | 明确拒绝，不改数据 | 不允许（**不存在 migration 0**） | — |
| ≥3（假想，比 binary 新） | 拒绝启动（硬规则） | 拒绝启动（硬规则） | — | — |

补充规则：

- 升级只能**逐级迁移**：DB=1 到 target=3 的 binary 必须依次执行 1→2→3，每级独立事务；不允许跨级、不允许跳级。
- 上表 "v0.2.0" 为**假设示例行**，用于演示表的使用方式；真实取值随首个引入 v2 迁移的 release 按 §8 同步更新本表。

## 5. 文件布局 schema

- 文件布局与 DB schema **同轨版本化**：schema v1 同时定义 DB 结构与文件布局 `data/<pkg>/{yaml,func,tmpl}/`。分区名 = 设备配置的应用包名（pkg），目录名即资源类型（yaml=可运行脚本、func=函数库、tmpl=模板图片）；**跨分区不解析，无 default 兜底**；模板为 8-bit 灰度 PNG。
- **无旧布局兼容读取**：服务端只识别 v1 布局；不满足布局的目录/文件不读取、不解析、不静默搬移或改名；不做任何旧布局探测与自动转换。
- 未来若引入文件布局迁移（DATA-004 可恢复文件迁移框架），固定顺序：**plan → staging copy → hash/validate → marker**：
  1. plan：生成迁移计划（源→目标清单），先记意图后执行；
  2. staging copy：复制到与 data 同卷的暂存区，源文件全程不动；
  3. hash/validate：逐文件哈希与结构校验；
  4. marker：校验全通过后原子落标记，声明迁移完成。
  - **独立 journal**（文件迁移 journal ≠ `state/update-journal.json`），每个 copy/hash/rename/marker 边界失败源不丢、可重试（计划 §11.3）。
  - **重复运行幂等**：marker 存在且校验通过 → no-op；发现新旧**混合布局不得误标成功**（保持 in-progress 状态，禁止写出 marker）。
  - **旧源文件保留**到升级提交 + 回滚保留期结束（清理遵循 `backups/`、`quarantine/` 保留策略，不得提前删除唯一回滚依据）。

## 6. 回滚承诺边界（核心）

### 6.1 pre-commit（journal 未达 committed）：自动回滚承诺成立

覆盖范围：candidate 启动即退出、迁移失败、readiness 永久失败、wrong version/schema/boot id、依赖损坏等（计划 §6.6/§11.3）。由于 activation gate（计划 §6.8）保证 committed 前 scheduler 与业务写 API **零写入**，此窗口内数据只有迁移差异、无业务写入，恢复快照零丢失。

动作序列（固定）：停止候选进程（按准确 PID）→ 隔离失败数据入 `quarantine/` → 恢复升级前快照（LCH-011）→ `state/current.json` 切回 previous → 启动旧版本并验证。恢复后 DB `user_version` 必须等于 journal 记录的 `schema_before`。

**回滚也失败 → `manual_recovery_required`**：停止自动重试循环；保留 update-journal、快照、新旧版本目录、quarantine 全部证据，等人工处置；任何自动流程不得清理上述证据路径。

### 6.2 post-commit（已 committed）：不承诺无损自动降级

- DB 版本在旧 binary 兼容范围内（`user_version ≤ 旧binary.max_read_schema` 且 ≥ 其 `min_read_schema`）：允许直接切回旧 binary（current.json 指回），数据无损。
- 超出兼容范围：不存在降级 migration（schema 只前进）；唯一途径是**恢复升级前快照**，且**必须明确提示将丢失 committed 之后产生的全部业务数据**；升级后数据无自动保留承诺。
- 兼容判断一律用 §7 `inspect` 的输出，不做人工推算。Docker/直跑模式无 launcher、无自动回滚执行者：升级=换镜像 digest，回滚=切回旧 digest，同样受本兼容表与"too_new 拒绝启动"硬规则约束。

### 6.3 rollback_floor 语义

manifest `release.rollback_floor` = **本 release 承诺可回滚的最低 DB schema 版本**：

- 升级前 DB 版本（journal `schema_before`）≥ `rollback_floor` → 该升级路径处于回滚承诺覆盖内（§6.1 自动回滚与 §6.2 快照恢复承诺均成立）。
- `schema_before < rollback_floor` → launcher 拒绝直接升级，要求先逐级升到中间版本（DB 达到 floor 以上）再升级。
- 冻结约束：`rollback_floor ≤ data_schema`，且 floor 值必须 ≥ previous 链上 binary 的 `min_read_schema`（保证回滚目标真的打得开）。
- 示例：v0.2.0 `data_schema=2, rollback_floor=1`——从 DB=1 升级全程可回滚；committed 后 DB=2 超出 v0.1.0 max=1，降级=恢复快照并提示丢数据。

## 7. maintenance CLI 契约

形态冻结（具体子命令名可微调，形态不得变）：

```
gamebot-server inspect  [--data-dir <path>] [--json]
gamebot-server migrate --data-dir <path> [--json]
```

共同约束：

- **零后台服务**：不启动 adb、ffmpeg/scrcpy、scheduler、HTTP、设备扫描、watchdog/idle_power_loop，不建立 WebRTC/DataChannel，不触发连接生命周期。
- 数据目录由 `--data-dir` 显式指定（缺省按 PATH-001 配置解析规则回落）；错误经非 0 退出码区分（成功 0；too_new / unversioned / missing / 迁移失败各自可辨），`--json` 时错误同样输出结构化 JSON。

`inspect`（只读诊断，任何情况下不写数据）：

- 输出：数据目录、DB 存在性、实际 `user_version`、binary `min/max/target`、兼容判定（`ok` / `needs_migration` / `too_new` / `unversioned` / `missing`）、待执行迁移清单（from→to 逐级）、文件布局 v1 符合性结果。
- DB 不存在 → 判定 `missing`，**不创建数据库文件**。

`migrate`（执行迁移）：

- 对 `[min, target)` 内缺失版本**逐级**执行 §2 迁移；输出 JSON 结果（形态）：`{"from": n, "to": target, "applied": [{"from": .., "to": ..}], "ok": true}`，失败附 `"error"`。
- `too_new` / `unversioned` / `missing` → 拒绝执行，诊断信息与启动拒绝路径一致；单级失败即停、该级整体回滚、可重试。
- 正常结束执行 `wal_checkpoint(TRUNCATE)`（见 §2）。
- launcher 用途：升级前先在**数据副本**上跑 inspect+migrate 做 preflight（计划 §6.7），正式迁移发生在候选 activation gate；preflight 与实迁共用同一 binary、同一判定路径，保证结论一致。

## 8. 变更规则

- 本文件的**兼容表、min/max/target 取值、rollback_floor 语义**属于版本化契约：任何变更必须与 **DATA 轨代码**（store 迁移框架、binary 常量、maintenance CLI）和 **manifest 的 `data_schema` / `rollback_floor` 字段**同步提交（同一变更集内同时更新，或按仓库提交规范拆分但同批次合入且互相引用），不得只改一侧；契约变更单独成 commit 并通知 launcher/server/web/qa 四轨（计划 §17.1）。
- 新增迁移时的固定顺序：先更新本表（新增 DB 版本行 + 新 release 取值行）→ 随迁移代码同批合入 → 同步更新 DATA-006 迁移 fixtures 与 manifest schema fixture（ARC-002）。
- CI 校验（VER-001 一致性 + DATA-003 门禁落地）发现代码常量、manifest 字段与本表不一致时必须失败。
