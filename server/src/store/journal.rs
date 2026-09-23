//! Persistent, runner-neutral execution history. Event payloads belong to their producer.
use super::*;
use serde_json::Value;

pub(crate) const DDL: &str = r#"
CREATE TABLE run_records (
    run_id TEXT PRIMARY KEY, device_id TEXT NOT NULL, entrypoint TEXT NOT NULL,
    started_at TEXT NOT NULL, finished_at TEXT, record TEXT NOT NULL
);
CREATE INDEX idx_run_records_device ON run_records(device_id, started_at DESC);
CREATE TABLE run_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES run_records(run_id) ON DELETE CASCADE,
    time TEXT NOT NULL, payload TEXT NOT NULL
);
CREATE INDEX idx_run_events_run ON run_events(run_id, id);
"#;

impl Store {
    pub(crate) fn save_run_record(
        &self,
        record: &crate::run_manager::RunRecord,
    ) -> anyhow::Result<()> {
        let record = serde_json::to_value(record)?;
        self.request(move |conn| {
            conn.execute("INSERT INTO run_records(run_id,device_id,entrypoint,started_at,finished_at,record)
                VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(run_id) DO UPDATE SET finished_at=excluded.finished_at,record=excluded.record",
                rusqlite::params![record["run_id"].as_str(), record["device_id"].as_str(), record["entrypoint"].as_str(),
                    record["started_at"].as_str(), record["finished_at"].as_str(), record.to_string()])?;
            Ok(())
        })
    }

    pub(crate) async fn append_run_event(
        &self,
        run_id: String,
        payload: Value,
    ) -> anyhow::Result<()> {
        let time = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let mut payload = payload;
        // Prevent arbitrary returned values from creating unbounded individual records.
        if payload.to_string().len() > 128 * 1024 {
            payload["data"] =
                serde_json::json!({"truncated": true, "message": "内容超过 128 KiB，已省略"});
        }
        self.request_async(move |conn| {
            conn.execute(
                "INSERT INTO run_events(run_id,time,payload) VALUES (?1,?2,?3)",
                rusqlite::params![run_id, time, payload.to_string()],
            )?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn run_history(
        &self,
        device: String,
        entrypoint: Option<String>,
        before: Option<String>,
    ) -> anyhow::Result<Vec<Value>> {
        self.request_async(move |conn| {
            let mut stmt = conn.prepare("SELECT record FROM run_records WHERE device_id=?1 AND (?2 IS NULL OR entrypoint=?2)
                AND (?3 IS NULL OR (started_at,run_id) < (SELECT started_at,run_id FROM run_records WHERE run_id=?3 AND device_id=?1))
                ORDER BY started_at DESC,run_id DESC LIMIT 30")?;
            let rows = stmt.query_map(rusqlite::params![device,entrypoint,before], |r| r.get::<_,String>(0))?;
            rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
        }).await
    }

    pub(crate) async fn stored_run(&self, run_id: String) -> anyhow::Result<Option<Value>> {
        self.request_async(move |conn| {
            use rusqlite::OptionalExtension;
            let row: Option<String> = conn
                .query_row(
                    "SELECT record FROM run_records WHERE run_id=?1",
                    [run_id],
                    |r| r.get(0),
                )
                .optional()?;
            row.map(|s| serde_json::from_str(&s).map_err(Into::into))
                .transpose()
        })
        .await
    }

    pub(crate) async fn run_event_page(&self, run_id: String, after: i64) -> anyhow::Result<Value> {
        self.request_async(move |conn| {
            let mut stmt = conn.prepare("SELECT id,time,payload FROM run_events WHERE run_id=?1 AND id>?2 ORDER BY id LIMIT 501")?;
            let rows = stmt.query_map(rusqlite::params![run_id,after.max(0)], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;
            let mut events = Vec::new();
            for row in rows {
                let (id,time,payload) = row?;
                let mut event: Value = serde_json::from_str(&payload)?;
                event["id"] = id.into(); event["time"] = time.into();
                events.push(event);
            }
            let more = events.len() > 500;
            events.truncate(500);
            let next = events.last().and_then(|e| e["id"].as_i64()).unwrap_or(after);
            Ok(serde_json::json!({"events":events,"next":next,"has_more":more}))
        }).await
    }

    pub(crate) fn recover_run_history(&self) -> anyhow::Result<()> {
        self.request(|conn| {
            let now = Utc::now().to_rfc3339();
            conn.execute("UPDATE run_records SET finished_at=?1, record=json_set(record,'$.state','failed','$.finished_at',?1,'$.error','服务重启，运行已中断') WHERE finished_at IS NULL", [now])?;
            Ok(())
        })
    }

    pub(crate) fn prune_run_history(&self, days: u32) -> anyhow::Result<()> {
        if days == 0 {
            return Ok(());
        }
        let cutoff = (Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        loop {
            let cutoff = cutoff.clone();
            let count = self.request(move |conn| Ok(conn.execute("DELETE FROM run_records WHERE run_id IN (SELECT run_id FROM run_records WHERE finished_at IS NOT NULL AND finished_at<?1 LIMIT 50)", [cutoff])?))?;
            if count == 0 {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(id: &str, device: &str) -> crate::run_manager::RunRecord {
        crate::run_manager::RunRecord {
            run_id: id.into(),
            device_id: device.into(),
            runner_id: "test".into(),
            entrypoint: "p/example".into(),
            script_id: "p/example".into(),
            source: crate::run_manager::RunSource::Manual,
            task_id: None,
            scheduled_at: None,
            state: crate::run_manager::RunState::Starting,
            started_at: Utc::now(),
            finished_at: None,
            error: None,
        }
    }

    #[tokio::test]
    async fn journal_survives_restart_paginates_and_isolates_runs() {
        let dir = std::env::temp_dir().join(format!("gamer-journal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db = Store::open(&cfg).unwrap();
        db.save_run_record(&record("a", "d1")).unwrap();
        db.save_run_record(&record("b", "d2")).unwrap();
        for i in 0..502 {
            db.append_run_event(
                "a".into(),
                serde_json::json!({"ev":"detail","name":"log","data":{"message":i}}),
            )
            .await
            .unwrap();
        }
        db.append_run_event("b".into(), serde_json::json!({"ev":"run_start"}))
            .await
            .unwrap();
        let first = db.run_event_page("a".into(), 0).await.unwrap();
        assert_eq!(first["events"].as_array().unwrap().len(), 500);
        assert_eq!(first["has_more"], true);
        let second = db
            .run_event_page("a".into(), first["next"].as_i64().unwrap())
            .await
            .unwrap();
        assert_eq!(second["events"].as_array().unwrap().len(), 2);
        assert_eq!(second["has_more"], false);
        assert_eq!(
            db.run_history("d1".into(), Some("p/example".into()), None)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(db
            .run_history("d1".into(), Some("other".into()), None)
            .await
            .unwrap()
            .is_empty());
        drop(db);
        let db = Store::open(&cfg).unwrap();
        db.recover_run_history().unwrap();
        assert_eq!(
            db.stored_run("a".into()).await.unwrap().unwrap()["state"],
            "failed"
        );
        assert_eq!(
            db.run_event_page("b".into(), 0).await.unwrap()["events"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        db.request(|conn| {
            conn.execute(
                "UPDATE run_records SET finished_at='2000-01-01T00:00:00Z' WHERE run_id='a'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        db.prune_run_history(0).unwrap();
        assert!(db.stored_run("a".into()).await.unwrap().is_some());
        db.prune_run_history(1).unwrap();
        assert!(db.stored_run("a".into()).await.unwrap().is_none());
        assert!(db.run_event_page("a".into(), 0).await.unwrap()["events"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(db.stored_run("b".into()).await.unwrap().is_some());
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn v4_upgrade_keeps_existing_rows_and_creates_journal() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::SCHEMA_V4_DDL).unwrap();
        conn.execute("INSERT INTO logs(time,device_id,script_id,level,msg) VALUES ('now','d','s','info','keep')", []).unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
        crate::migrations::run_migrations(&mut conn, 4, crate::migrations::MIGRATIONS).unwrap();
        super::super::validate_schema_v5(&conn).unwrap();
        assert_eq!(
            conn.query_row("SELECT msg FROM logs", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
    }

    #[tokio::test]
    async fn history_cursor_reaches_older_runs_with_tied_times_and_new_arrivals() {
        let dir = tempfile::tempdir().unwrap();
        let db = Store::open(&Config { data_dir: dir.path().into(), ..Default::default() }).unwrap();
        let start = Utc::now();
        for i in 0..65 {
            let mut run = record(&format!("run-{i:03}"), "d1");
            run.started_at = start;
            if i % 2 == 0 { run.entrypoint = "p#function".into(); }
            db.save_run_record(&run).unwrap();
        }
        db.save_run_record(&record("other-device", "d2")).unwrap();
        let first = db.run_history("d1".into(), None, None).await.unwrap();
        assert_eq!(first.len(), 30);
        assert_eq!(first[0]["run_id"], "run-064");
        assert_eq!(first[29]["run_id"], "run-035");
        db.save_run_record(&record("new-arrival", "d1")).unwrap();
        let second = db.run_history("d1".into(), None, Some("run-035".into())).await.unwrap();
        assert_eq!(second.len(), 30);
        assert_eq!(second[0]["run_id"], "run-034");
        assert_eq!(second[29]["run_id"], "run-005");
        let third = db.run_history("d1".into(), None, Some("run-005".into())).await.unwrap();
        assert_eq!(third.len(), 5);
        assert_eq!(third[4]["run_id"], "run-000");
        let functions = db.run_history("d1".into(), Some("p#function".into()), Some("run-035".into())).await.unwrap();
        assert_eq!(functions.len(), 18);
        assert!(functions.iter().all(|r| r["entrypoint"] == "p#function"));
        assert!(db.run_history("d2".into(), None, Some("run-035".into())).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn event_sink_persists_even_when_viewer_delivery_fails() {
        use crate::core::{
            ActivityLease, EventSink, RunContext, RunRequest, RuntimeEvent, RuntimeEventKind,
        };
        use futures_util::future::BoxFuture;
        struct UnusedExecutor;
        impl crate::run_manager::RunExecutor for UnusedExecutor {
            fn prepare<'a>(
                &'a self,
                _: &'a RunContext,
                _: &'a RunRequest,
            ) -> BoxFuture<'a, anyhow::Result<()>> {
                unreachable!()
            }
            fn execute<'a>(
                &'a self,
                _: &'a RunContext,
                _: &'a RunRequest,
                _: bool,
                _: Arc<std::sync::atomic::AtomicBool>,
            ) -> BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
                unreachable!()
            }
            fn acquire(&self, _: &RunContext) -> anyhow::Result<Box<dyn ActivityLease>> {
                unreachable!()
            }
        }
        struct ClosedViewer;
        impl EventSink for ClosedViewer {
            fn emit(&self, _: RuntimeEvent) -> BoxFuture<'_, anyhow::Result<()>> {
                Box::pin(async { anyhow::bail!("viewer closed") })
            }
        }
        let dir = std::env::temp_dir().join(format!("gamer-journal-sink-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db = Arc::new(Store::open(&cfg).unwrap());
        db.save_run_record(&record("a", "d1")).unwrap();
        let sink = crate::run_journal::JournalEventSink {
            db: db.clone(),
            runs: Arc::new(crate::run_manager::RunManager::new(Arc::new(
                UnusedExecutor,
            ))),
            viewer: Arc::new(ClosedViewer),
        };
        let mut event = RuntimeEvent::new(
            crate::core::DeviceId::new("d1").unwrap(),
            RuntimeEventKind::Detail {
                name: "log".into(),
                data: serde_json::json!({"message":"kept"}),
            },
        );
        event.trace = Some(serde_json::json!({"run_id":"a","frame_id":2,"path":"run[1]"}));
        assert!(sink.emit(event).await.is_err());
        let page = db.run_event_page("a".into(), 0).await.unwrap();
        assert_eq!(page["events"][0]["data"]["message"], "kept");
        assert_eq!(page["events"][0]["trace"]["frame_id"], 2);
        drop(sink);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
