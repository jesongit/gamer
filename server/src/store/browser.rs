use super::*;
use crate::browser::BrowserTarget;
use rusqlite::Transaction;

pub(crate) const BROWSER_DDL: &str = "CREATE TABLE browser_targets (id TEXT PRIMARY KEY, profile_id TEXT NOT NULL UNIQUE, config TEXT NOT NULL);";

pub(crate) fn migrate_v5_to_v6(tx: &Transaction<'_>) -> anyhow::Result<()> {
    let start = SCHEMA_V4_DDL.find("CREATE TABLE timer_tasks (").unwrap();
    let end = SCHEMA_V4_DDL.find("CREATE TABLE task_presets (").unwrap();
    tx.execute_batch("ALTER TABLE timer_tasks RENAME TO timer_tasks_v5; DROP INDEX idx_timer_tasks_wakeup; DROP INDEX idx_timer_tasks_app;")?;
    tx.execute_batch(
        &SCHEMA_V4_DDL[start..end].replace("android_package TEXT NOT NULL", "android_package TEXT"),
    )?;
    tx.execute_batch(
        "INSERT INTO timer_tasks SELECT * FROM timer_tasks_v5; DROP TABLE timer_tasks_v5;",
    )?;
    tx.execute_batch(BROWSER_DDL)?;
    Ok(())
}

impl Store {
    pub fn browser_targets(&self) -> anyhow::Result<Vec<BrowserTarget>> {
        self.request(|conn| {
            let mut stmt = conn.prepare("SELECT config FROM browser_targets ORDER BY id")?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|s| Ok(serde_json::from_str(&s)?))
                .collect()
        })
    }
    pub fn save_browser_target(&self, target: BrowserTarget) -> anyhow::Result<()> {
        target.validate()?;
        self.request(move |conn| {
            let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM devices WHERE id=?1)",[&target.id],|r|r.get(0))?;
            anyhow::ensure!(!exists, "目标 ID 已被 Android 设备占用");
            conn.execute("INSERT INTO browser_targets(id,profile_id,config) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET profile_id=excluded.profile_id,config=excluded.config",rusqlite::params![target.id,target.profile_id,serde_json::to_string(&target)?])?;
            Ok(())
        })
    }
    pub fn delete_browser_target(&self, id: &str) -> anyhow::Result<()> {
        let id = id.to_string();
        self.request(move |conn| {
            conn.execute("DELETE FROM browser_targets WHERE id=?1", [id])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_preserves_android_and_accepts_browser_context() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V4_DDL).unwrap();
        conn.execute_batch("INSERT INTO timer_tasks(id,name,device_id,android_package,runner_id,entrypoint,payload_json,schedule_json,state,created_at,updated_at) VALUES('t','t','d','com.test','r','p/a.yaml','{}','{}','active','now','now');").unwrap();
        let tx = conn.transaction().unwrap();
        migrate_v5_to_v6(&tx).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            conn.query_row("SELECT android_package FROM timer_tasks", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "com.test"
        );
        conn.execute(
            "UPDATE timer_tasks SET android_package=NULL,device_id='browser-a'",
            [],
        )
        .unwrap();
        assert_eq!(
            conn.query_row("SELECT android_package FROM timer_tasks", [], |r| r
                .get::<_, Option<String>>(0))
                .unwrap(),
            None
        );
        conn.execute(
            "INSERT INTO browser_targets VALUES('browser-a','a','{}')",
            [],
        )
        .unwrap();
        assert!(conn
            .execute(
                "INSERT INTO browser_targets VALUES('browser-b','a','{}')",
                []
            )
            .is_err());
    }
}
