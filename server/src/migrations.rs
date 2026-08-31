//! SQLite 编号迁移框架（DATA-001 / release/contracts/schema-policy.md §1–§3）。
//!
//! 与契约逐条对齐的规则：
//! - `PRAGMA user_version` 是唯一权威版本标记；**不引入** `schema_migrations` 表，
//!   不用文件名/旁路标记推断版本；
//! - v1 是唯一基线（[`TARGET_SCHEMA`] = 1），迁移表当前为空；后续迁移从
//!   v1→v2 起逐级编号，静态注册于 [`MIGRATIONS`]，禁止运行期动态拼装；
//! - `user_version=0`（无版本旧库）在进入本模块前即被 [`crate::store`] 明确拒绝
//!   ——不存在 migration 0，本框架永不补齐/改写无版本库；
//! - 每个迁移 v(n-1)→vn 在**单个事务**内完成 DDL + 数据修复 + `user_version`
//!   推进；任一步失败整体回滚（版本不变，可安全重试）；
//! - 只前进不后退：无 down migration；一次只推进一级，逐级执行到 target；
//!   已达标再跑 = no-op；
//! - `user_version > target`（数据库比 binary 新）→ 明确拒绝启动，错误含实际
//!   版本与支持范围（契约 §3 硬规则，不做静默降读）。
//!
//! 兼容常量（契约 §3，DATA-003 常量化）：[`MIN_READ_SCHEMA`] / [`MAX_READ_SCHEMA`] /
//! [`TARGET_SCHEMA`] 是本 binary 的兼容声明唯一取值源——启动路径（[`crate::store`]）、
//! maintenance CLI（DATA-005 inspect/migrate）与 `/api/system/info` 的 schema 字段
//! 全部引用这组常量，禁止各处自抄数字。当前形态 `min = target = max = 1`；
//! 取值变更必须与 release/contracts/schema-policy.md §3 取值表同步提交（§8）。
//!
//! 生产路径：[`crate::store`] 的 `apply_schema_migrations` 先跑 [`run_migrations`]
//! 再做 `validate_schema_v1`。测试可用临时迁移表直接驱动 [`run_migrations`]，
//! 不触碰对外"无版本库拒绝"行为。

use rusqlite::{Connection, Transaction};

/// 本 binary 可打开并继续迁移的最低 user_version（v1 基线 → 1；0 永远拒绝）
pub const MIN_READ_SCHEMA: i64 = 1;
/// 可打开的最高 user_version；高于此值拒绝启动（schema-policy §3/§4 硬规则）
pub const MAX_READ_SCHEMA: i64 = 1;
/// 本 binary 迁移完成后的目标 schema 版本（v1 唯一基线；当前形态 target = max）
pub const TARGET_SCHEMA: i64 = 1;

/// 契约 §3 冻结约束 `min_read ≤ target ≤ max_read` 编译期固化：取值漂移在
/// 编译期即失败，不等运行期诊断
const _: () = assert!(
    MIN_READ_SCHEMA <= TARGET_SCHEMA && TARGET_SCHEMA <= MAX_READ_SCHEMA,
    "schema-policy §3 frozen constraint violated: min_read_schema <= target_schema <= max_read_schema",
);

/// 单条迁移的执行体：在迁移事务内完成该级全部 DDL/数据修复。
/// `user_version` 推进由框架统一负责（与 DDL 同事务），迁移体不得自行推进。
pub(crate) type MigrationFn = fn(&Transaction<'_>) -> anyhow::Result<()>;

/// 一条编号迁移 v(from)→v(to)。`to == from + 1`（不越级、不跳级）。
pub(crate) struct Migration {
    pub from: i64,
    pub to: i64,
    pub description: &'static str,
    pub apply: MigrationFn,
}

/// 静态注册表：v1 是唯一基线，当前为空。首个 v1→v2 迁移在此追加，
/// 并按 schema-policy §8 同步更新契约文档兼容表。
pub(crate) static MIGRATIONS: &[Migration] = &[];

/// 把 `current` 逐级迁移到目标版本。目标 = `migrations` 注册表覆盖到的最高
/// 版本（生产路径注册表为空 → 即 [`TARGET_SCHEMA`]）；测试注入临时
/// 迁移链验证框架逻辑，不改静态表、不触碰无版本库拒绝语义。
pub(crate) fn run_migrations(
    conn: &mut Connection,
    current: i64,
    migrations: &[Migration],
) -> anyhow::Result<()> {
    let target = migrations
        .iter()
        .map(|m| m.to)
        .max()
        .unwrap_or(TARGET_SCHEMA);
    // 数据库比 binary 新：硬规则拒绝（契约 §3，DATA-003——错误必须含实际
    // user_version、支持范围 [min, max] 与 target，可诊断）
    if current > target {
        anyhow::bail!(
            "unsupported database schema version {current}: newer than the highest version \
             this binary supports (supported range [{MIN_READ_SCHEMA}, {MAX_READ_SCHEMA}], \
             target {TARGET_SCHEMA}, registry target {target}); downgrade is not supported; \
             restore the snapshot taken before the upgrade"
        );
    }
    if current < MIN_READ_SCHEMA {
        // user_version=0 的对外拒绝在 store::ensure_schema（含"备份后重建"指引），
        // 此处为防御性兜底
        anyhow::bail!(
            "database schema version {current} is below the minimum supported version \
             {MIN_READ_SCHEMA}; unversioned databases are never migrated"
        );
    }

    let mut version = current;
    while version < target {
        let Some(migration) = migrations.iter().find(|m| m.from == version) else {
            anyhow::bail!(
                "missing registered migration for schema v{version} -> v{}; \
                 static migration table is incomplete",
                version + 1
            );
        };
        anyhow::ensure!(
            migration.to == version + 1,
            "invalid migration registration v{} -> v{}: versions must advance exactly one step",
            migration.from,
            migration.to
        );
        let tx = conn.transaction()?;
        (migration.apply)(&tx)?;
        // user_version 推进与 DDL 同事务：失败整体回滚，版本不变，可安全重试
        tx.pragma_update(None, "user_version", migration.to)?;
        tx.commit()?;
        tracing::info!(
            from = migration.from,
            to = migration.to,
            description = migration.description,
            "database schema migration applied"
        );
        version = migration.to;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 带版本标记的最小库（框架测试只关心 user_version 机制，不依赖 v1 全量 DDL）
    fn versioned_memory_db(version: i64) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE anchor (x TEXT); PRAGMA user_version = {version};"
        ))
        .unwrap();
        conn
    }

    fn user_version(conn: &Connection) -> i64 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    fn step(from: i64, to: i64, body: MigrationFn) -> Migration {
        Migration {
            from,
            to,
            description: "test migration",
            apply: body,
        }
    }

    /// DATA-003 门禁：静态注册表与兼容常量必须一致（schema-policy §8——
    /// 代码常量、迁移注册表与契约取值表同批提交，任何一侧漂移在此失败）
    #[test]
    fn static_registry_and_compat_constants_are_coherent() {
        let registry_target = MIGRATIONS
            .iter()
            .map(|m| m.to)
            .max()
            .unwrap_or(TARGET_SCHEMA);
        assert_eq!(
            registry_target, MAX_READ_SCHEMA,
            "MIGRATIONS 注册表覆盖目标必须等于 MAX_READ_SCHEMA（schema-policy §8 同步规则）"
        );
        assert_eq!(
            TARGET_SCHEMA, MAX_READ_SCHEMA,
            "当前形态 target = max（启动即迁满，契约 §3）"
        );
        // 注册表必须从 MIN 起连续编号，不缺级
        for v in MIN_READ_SCHEMA..registry_target {
            assert!(
                MIGRATIONS.iter().any(|m| m.from == v && m.to == v + 1),
                "missing registered migration v{v} -> v{}",
                v + 1
            );
        }
    }

    #[test]
    fn empty_table_at_target_is_noop() {
        // 空迁移表 + u=1（已达 target）：no-op，结构校验语义不变
        let mut conn = versioned_memory_db(TARGET_SCHEMA);
        run_migrations(&mut conn, TARGET_SCHEMA, MIGRATIONS).unwrap();
        assert_eq!(user_version(&conn), 1);
    }

    #[test]
    fn single_migration_applies_and_advances_one_level() {
        fn create_t2(tx: &Transaction<'_>) -> anyhow::Result<()> {
            tx.execute_batch("CREATE TABLE t2 (x TEXT);")?;
            Ok(())
        }
        let migrations = [step(1, 2, create_t2)];
        let mut conn = versioned_memory_db(1);
        run_migrations(&mut conn, 1, &migrations).unwrap();
        assert_eq!(user_version(&conn), 2);
        let t2: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='t2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(t2, 1);
    }

    #[test]
    fn chain_migrates_stepwise_without_skipping_levels() {
        fn add_a(tx: &Transaction<'_>) -> anyhow::Result<()> {
            tx.execute_batch("ALTER TABLE anchor ADD COLUMN a TEXT;")?;
            Ok(())
        }
        fn add_b(tx: &Transaction<'_>) -> anyhow::Result<()> {
            tx.execute_batch("ALTER TABLE anchor ADD COLUMN b TEXT;")?;
            Ok(())
        }
        let migrations = [step(1, 2, add_a), step(2, 3, add_b)];
        let mut conn = versioned_memory_db(1);
        run_migrations(&mut conn, 1, &migrations).unwrap();
        assert_eq!(user_version(&conn), 3);
        // 从中间版本启动（u=2）只补缺失的 2→3，不重跑 1→2
        let mut conn = versioned_memory_db(2);
        run_migrations(&mut conn, 2, &migrations).unwrap();
        assert_eq!(user_version(&conn), 3);
    }

    #[test]
    fn failed_migration_rolls_back_version_and_ddl() {
        fn broken(tx: &Transaction<'_>) -> anyhow::Result<()> {
            tx.execute_batch("CREATE TABLE t3 (x TEXT);")?;
            anyhow::bail!("injected migration failure")
        }
        let migrations = [step(1, 2, broken)];
        let mut conn = versioned_memory_db(1);
        let err = run_migrations(&mut conn, 1, &migrations).unwrap_err();
        assert!(err.to_string().contains("injected migration failure"));
        // 事务回滚：user_version 不变、半截 DDL 不残留 → 可安全重试
        assert_eq!(user_version(&conn), 1);
        let t3: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='t3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(t3, 0);
    }

    #[test]
    fn too_new_database_is_rejected_with_actual_version_and_range() {
        for too_new in [2i64, 5, 99] {
            let mut conn = versioned_memory_db(too_new);
            let err = run_migrations(&mut conn, too_new, MIGRATIONS).unwrap_err();
            let msg = err.to_string();
            // DATA-003：错误必须可诊断——含实际 user_version、支持范围 [min, max]
            // 与 target（取值直接引用兼容常量，不重复手写数字）
            assert!(
                msg.contains(&format!("unsupported database schema version {too_new}")),
                "{msg}"
            );
            assert!(
                msg.contains(&format!(
                    "supported range [{MIN_READ_SCHEMA}, {MAX_READ_SCHEMA}]"
                )),
                "错误应含支持范围: {msg}"
            );
            assert!(
                msg.contains(&format!("target {TARGET_SCHEMA}")),
                "错误应含 target: {msg}"
            );
        }
    }

    #[test]
    fn missing_registered_step_is_rejected() {
        fn noop(_tx: &Transaction<'_>) -> anyhow::Result<()> {
            Ok(())
        }
        // 注册表缺 1→2（只有 2→3）：缺级必须显式失败，不允许静默不动
        let migrations = [step(2, 3, noop)];
        let mut conn = versioned_memory_db(1);
        let err = run_migrations(&mut conn, 1, &migrations).unwrap_err();
        assert!(err.to_string().contains("missing registered migration"));
        assert_eq!(user_version(&conn), 1);
    }

    #[test]
    fn invalid_registration_not_advancing_one_level_is_rejected() {
        fn noop(_tx: &Transaction<'_>) -> anyhow::Result<()> {
            Ok(())
        }
        let migrations = [step(1, 3, noop)];
        let mut conn = versioned_memory_db(1);
        let err = run_migrations(&mut conn, 1, &migrations).unwrap_err();
        assert!(err.to_string().contains("exactly one step"));
        assert_eq!(user_version(&conn), 1);
    }
}
