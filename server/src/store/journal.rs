//! Persistent, runner-neutral execution history. Event payloads belong to their producer.
use super::*;
use serde_json::Value;
use std::time::{Duration, Instant};

pub(super) struct RepeatedObservation {
    key: String,
    payload: Value,
    count: u32,
    first_at: String,
    last_at: String,
    max_score: Option<f64>,
    since: Instant,
}

impl RepeatedObservation {
    pub(super) fn finish(mut self) -> Option<(String, Value)> {
        if self.count == 0 {
            return None;
        }
        self.payload["aggregation"] = serde_json::json!({
            "additional_observations": self.count,
            "first_at": self.first_at, "last_at": self.last_at,
            "max_score": self.max_score,
        });
        Some((self.last_at, self.payload))
    }
}

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
    pub(crate) fn diagnostic_record(&self, id: String) -> anyhow::Result<Option<Value>> {
        self.request(move |conn| {
            use rusqlite::OptionalExtension;
            conn.query_row(
                "SELECT record FROM run_records WHERE run_id=?1",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|text| serde_json::from_str(&text).map_err(Into::into))
            .transpose()
        })
    }

    pub(crate) fn diagnostic_events(
        &self,
        id: String,
        after: i64,
    ) -> anyhow::Result<Vec<(i64, String, Value)>> {
        self.request(move |conn| {
            let mut statement = conn.prepare("SELECT id,time,payload FROM run_events WHERE run_id=?1 AND id>?2 ORDER BY id LIMIT 50")?;
            let rows = statement.query_map(rusqlite::params![id,after], |r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?;
            rows.map(|row|{ let (id,time,payload)=row?; Ok((id,time,serde_json::from_str(&payload)?)) }).collect()
        })
    }
    /// Exact identities are supplied by the owning producer; Core does not
    /// interpret runner-specific script versions or payloads.
    pub(crate) async fn run_records_for_scope(
        &self,
        runner: String,
        entrypoint: String,
        package: String,
    ) -> anyhow::Result<Vec<Value>> {
        self.request_async(move |conn| {
            let mut stmt = conn.prepare("SELECT record FROM run_records WHERE entrypoint=?1 AND json_extract(record,'$.runner_id')=?2 AND json_extract(record,'$.content_package')=?3")?;
            let rows = stmt.query_map(rusqlite::params![entrypoint,runner,package], |r| r.get::<_,String>(0))?;
            rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
        }).await
    }

    pub(crate) fn delete_terminal_run_records(&self, ids: Vec<String>) -> anyhow::Result<usize> {
        self.request(move |conn| {
            let tx = conn.transaction()?;
            let mut removed = 0;
            for id in ids {
                let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM run_records WHERE run_id=?1 AND (finished_at IS NULL OR COALESCE(json_extract(record,'$.state'),'') NOT IN ('success','failed','cancelled')))", [&id], |r| r.get(0))?;
                anyhow::ensure!(!active, "run_active");
                let protected: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM run_records WHERE run_id=?1 AND (json_extract(record,'$.task_id') IS NOT NULL OR COALESCE(json_extract(record,'$.source'),'')!='manual'))", [&id], |r| r.get(0))?;
                anyhow::ensure!(!protected, "run_protected");
                // Explicitly remove events even on databases without FK enforcement.
                tx.execute("DELETE FROM run_events WHERE run_id=?1", [&id])?;
                removed += tx.execute("DELETE FROM run_records WHERE run_id=?1", [&id])?;
            }
            tx.commit()?;
            Ok(removed)
        })
    }
    pub(crate) fn save_run_record(
        &self,
        record: &crate::run_manager::RunRecord,
    ) -> anyhow::Result<()> {
        let mut record = serde_json::to_value(record)?;
        record["_diagnostics_version"] = crate::diagnostics::POLICY_VERSION.into();
        let build = crate::build_info::build_info();
        record["diagnostics"]["host"] = serde_json::json!({"version":build.version,"commit":build.commit,"target":build.target,"built_at":build.built_at});
        if let Some(identity) = self
            .trace
            .identity(record["run_id"].as_str().unwrap_or_default())
        {
            record["diagnostics"]["identity"] = identity;
        }
        self.trace.record(
            record["run_id"].as_str().unwrap_or_default(),
            record.clone(),
        );
        self.trace.register(
            record["run_id"].as_str().unwrap_or_default(),
            record["finished_at"].as_str().map(str::to_string),
        );
        self.request(move |conn| {
            conn.execute("INSERT INTO run_records(run_id,device_id,entrypoint,started_at,finished_at,record)
                VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(run_id) DO UPDATE SET finished_at=excluded.finished_at,
                record=CASE WHEN json_extract(run_records.record,'$._diagnostics_version')=1 THEN excluded.record
                ELSE json_remove(excluded.record,'$._diagnostics_version') END",
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
        if payload.to_string().len() > 128 * 1024 {
            // Opaque producers can place a large value outside `data` too.
            let original = payload.to_string();
            payload = serde_json::json!({"ev":"detail","name":"diagnostic_truncation",
                "data":{"truncated":true,"original_bytes":original.len(),"sha256":crate::diagnostics::digest(original.as_bytes())}});
        }
        let mut repeats = self.event_repeats.lock().await;
        let key = self
            .history_retention
            .aggregate_repeated_events
            .then(|| crate::diagnostics::observation_key(&payload))
            .flatten();
        let mut rows = Vec::new();
        if let Some(mut prior) = repeats.remove(&run_id) {
            if key.as_deref() == Some(prior.key.as_str())
                && prior.since.elapsed() < Duration::from_secs(2)
                && prior.count < 1024
            {
                if prior.count == 0 {
                    prior.first_at = time.clone();
                }
                prior.count += 1;
                prior.last_at = time;
                prior.max_score = match (prior.max_score, payload["score"].as_f64()) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                prior.payload = payload;
                repeats.insert(run_id, prior);
                return Ok(());
            }
            if let Some((time, payload)) = prior.finish() {
                rows.push((run_id.clone(), time, payload));
            }
        }
        if let Some(key) = key {
            // Bound buffers independently of caller cardinality; no live input
            // or error event is ever delayed by this observation-only path.
            if repeats.len() >= 64 {
                if let Some(oldest) = repeats
                    .iter()
                    .min_by_key(|(_, r)| r.since)
                    .map(|(id, _)| id.clone())
                {
                    if let Some((time, payload)) = repeats
                        .remove(&oldest)
                        .and_then(RepeatedObservation::finish)
                    {
                        rows.push((oldest, time, payload));
                    }
                }
            }
            repeats.insert(
                run_id.clone(),
                RepeatedObservation {
                    key,
                    payload: payload.clone(),
                    count: 0,
                    first_at: time.clone(),
                    last_at: time.clone(),
                    max_score: None,
                    since: Instant::now(),
                },
            );
        }
        rows.push((run_id, time, payload));
        self.request_async(move |conn| {
            let tx = conn.transaction()?;
            for (run_id, time, payload) in rows {
                tx.execute(
                    "INSERT INTO run_events(run_id,time,payload) VALUES (?1,?2,?3)",
                    rusqlite::params![run_id, time, payload.to_string()],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub(crate) async fn run_history(
        &self,
        device: Option<String>,
        entrypoint: Option<String>,
        before: Option<String>,
        task: Option<String>,
    ) -> anyhow::Result<Vec<Value>> {
        self.request_async(move |conn| {
            let mut stmt = conn.prepare("SELECT record FROM run_records WHERE (?1 IS NULL OR device_id=?1) AND (?2 IS NULL OR entrypoint=?2)
                AND (?4 IS NULL OR json_extract(record,'$.task_id')=?4)
                AND (?3 IS NULL OR (started_at,run_id) < (SELECT started_at,run_id FROM run_records WHERE run_id=?3
                    AND (?1 IS NULL OR device_id=?1) AND (?4 IS NULL OR json_extract(record,'$.task_id')=?4)))
                ORDER BY started_at DESC,run_id DESC LIMIT 30")?;
            let rows = stmt.query_map(rusqlite::params![device,entrypoint,before,task], |r| r.get::<_,String>(0))?;
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
        self.flush_run_events(&run_id).await?;
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
            use rusqlite::OptionalExtension;
            let expired = conn.query_row("SELECT json_extract(record,'$.diagnostics.details_status') FROM run_records WHERE run_id=?1",
                [&run_id], |r| r.get::<_,Option<String>>(0)).optional()?.flatten();
            Ok(serde_json::json!({"events":events,"next":next,"has_more":more,"status":expired.unwrap_or_else(||"available".into())}))
        }).await
    }

    pub(crate) async fn flush_run_events(&self, run_id: &str) -> anyhow::Result<()> {
        let mut repeats = self.event_repeats.lock().await;
        if let Some((time, payload)) = repeats.remove(run_id).and_then(RepeatedObservation::finish)
        {
            let id = run_id.to_string();
            self.request_async(move |conn| {
                conn.execute(
                    "INSERT INTO run_events(run_id,time,payload) VALUES (?1,?2,?3)",
                    rusqlite::params![id, time, payload.to_string()],
                )?;
                Ok(())
            })
            .await?;
        }
        Ok(())
    }

    pub(crate) fn recover_run_history(&self) -> anyhow::Result<()> {
        self.request(|conn| {
            let now = Utc::now().to_rfc3339();
            conn.execute("UPDATE run_records SET finished_at=?1, record=json_set(record,'$.state','failed','$.finished_at',?1,'$.error','服务重启，运行已中断') WHERE finished_at IS NULL", [now])?;
            Ok(())
        })
    }

    pub(crate) fn prune_run_history(&self, days: u32) -> anyhow::Result<()> {
        let policy = self.history_retention.clone();
        self.request(move |conn| {
            for (state, retain_days) in [("success", policy.success_detail_retain_days), ("failure", policy.failure_detail_retain_days)] {
                if retain_days == 0 { continue; }
                let cutoff = (Utc::now() - chrono::Duration::days(retain_days as i64)).to_rfc3339();
                loop {
                    let tx = conn.transaction()?;
                    let ids = {
                        let mut statement = tx.prepare("SELECT run_id FROM run_records WHERE finished_at IS NOT NULL AND finished_at<?1
                            AND json_extract(record,'$._diagnostics_version')=1
                            AND COALESCE(json_extract(record,'$.diagnostics.details_status'),'')!='expired'
                            AND ((?2='success' AND json_extract(record,'$.state')='success') OR (?2='failure' AND json_extract(record,'$.state') IN ('failed','cancelled'))) LIMIT 50")?;
                        let rows = statement.query_map(rusqlite::params![cutoff,state], |r|r.get::<_,String>(0))?;
                        rows.collect::<Result<Vec<_>,_>>()?
                    };
                    if ids.is_empty() { break; }
                    for id in ids {
                        tx.execute("DELETE FROM run_events WHERE run_id=?1", [&id])?;
                        tx.execute("UPDATE run_records SET record=json_set(record,'$.diagnostics.details_status','expired') WHERE run_id=?1", [&id])?;
                    }
                    tx.commit()?;
                }
            }
            Ok(())
        })?;
        if days == 0 {
            return Ok(());
        }
        let cutoff = (Utc::now() - chrono::Duration::days(days as i64)).to_rfc3339();
        loop {
            let cutoff = cutoff.clone();
            let count = self.request(move |conn| Ok(conn.execute("DELETE FROM run_records WHERE run_id IN (SELECT run_id FROM run_records WHERE finished_at IS NOT NULL AND finished_at<?1 AND json_extract(record,'$._diagnostics_version')=1 LIMIT 50)", [cutoff])?))?;
            if count == 0 {
                self.trace.prune_history(days);
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn temp_config(name: &str) -> (Config, PathBuf) {
        let dir = std::env::temp_dir().join(format!("gamer-{name}-{}", uuid::Uuid::new_v4()));
        (
            Config {
                data_dir: dir.clone(),
                ..Default::default()
            },
            dir,
        )
    }
    #[test]
    fn first_policy_start_preserves_old_plain_logs_across_restarts() {
        let root = tempfile::tempdir().unwrap();
        let cfg = Config {
            data_dir: root.path().into(),
            ..Default::default()
        };
        let db = Store::open(&cfg).unwrap();
        db.request(|conn|{conn.execute("INSERT INTO logs(time,device_id,script_id,level,msg) VALUES ('2000-01-01','d','s','info','old-original')",[])?;Ok(())}).unwrap();
        drop(db);
        std::fs::remove_file(root.path().join("diagnostics-policy.json")).unwrap(); // Synthetic pre-policy installation.
        let db = Store::open(&cfg).unwrap();
        db.request(|conn|{conn.execute("INSERT INTO logs(time,device_id,script_id,level,msg) VALUES ('2000-01-01','d','s','info','new-expired')",[])?;Ok(())}).unwrap();
        assert_eq!(db.prune_logs(14).unwrap(), 1);
        drop(db);
        let db = Store::open(&cfg).unwrap();
        let messages: Vec<String> = db
            .request(|conn| {
                let mut statement = conn.prepare("SELECT msg FROM logs ORDER BY id")?;
                let rows = statement.query_map([], |row| row.get(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        assert_eq!(messages, vec!["old-original"]);
    }
    #[tokio::test]
    async fn production_retention_separates_summary_details_and_preserves_unmarked_history() {
        let (cfg, dir) = temp_config("diagnostic-retention");
        let db = Store::open(&cfg).unwrap();
        for (id, state, age) in [
            ("success", crate::run_manager::RunState::Success, 20),
            ("failed", crate::run_manager::RunState::Failed, 20),
            ("old", crate::run_manager::RunState::Failed, 400),
            ("legacy", crate::run_manager::RunState::Success, 400),
        ] {
            let mut run = record(id, "d");
            if id == "success" {
                db.save_run_record(&run).unwrap();
                db.trace.snapshot(id,serde_json::json!({"_diagnostic_identity":{"source_sha256":"original-version"}})).unwrap();
            }
            run.state = state;
            run.finished_at = Some(Utc::now() - chrono::Duration::days(age));
            db.save_run_record(&run).unwrap();
            db.append_run_event(
                id.into(),
                serde_json::json!({"ev":"step_end","ok":false,"error":"FIXTURE_ERROR"}),
            )
            .await
            .unwrap();
        }
        db.request(|conn|{conn.execute("UPDATE run_records SET record=json_remove(record,'$._diagnostics_version') WHERE run_id='legacy'",[])?;Ok(())}).unwrap();
        db.prune_logs(14).unwrap();
        assert!(db.stored_run("success".into()).await.unwrap().is_some());
        assert_eq!(
            db.stored_run("success".into()).await.unwrap().unwrap()["diagnostics"]["identity"]
                ["producer"]["source_sha256"],
            "original-version"
        );
        assert_eq!(
            db.run_event_page("success".into(), 0).await.unwrap()["status"],
            "expired"
        );
        assert_eq!(
            db.run_event_page("failed".into(), 0).await.unwrap()["events"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(db.stored_run("old".into()).await.unwrap().is_none());
        assert!(db.stored_run("legacy".into()).await.unwrap().is_some());
        assert_eq!(
            db.run_event_page("legacy".into(), 0).await.unwrap()["events"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn repeat_buffers_preserve_counts_scores_cursor_order_and_error_boundaries() {
        let (cfg, dir) = temp_config("observation-repeats");
        let db = Store::open(&cfg).unwrap();
        db.save_run_record(&record("r", "d")).unwrap();
        for score in [0.1, 0.8, 0.2, 0.3] {
            db.append_run_event("r".into(),serde_json::json!({"ev":"vision","template":"a","found":false,"score":score,"trace":{"frame_id":7}})).await.unwrap();
        }
        db.append_run_event(
            "r".into(),
            serde_json::json!({"ev":"step_end","ok":false,"error":"FIND_TIMEOUT"}),
        )
        .await
        .unwrap();
        let page = db.run_event_page("r".into(), 0).await.unwrap();
        let rows = page["events"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1]["aggregation"]["additional_observations"], 3);
        assert_eq!(rows[1]["aggregation"]["max_score"], 0.8);
        assert_eq!(rows[1]["score"], 0.3);
        assert_eq!(rows[2]["error"], "FIND_TIMEOUT");
        let after = rows[0]["id"].as_i64().unwrap();
        assert_eq!(
            db.run_event_page("r".into(), after).await.unwrap()["events"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        db.append_run_event(
            "r".into(),
            serde_json::json!({"ev":"vision","template":"a","found":false,"trace":{"frame_id":8}}),
        )
        .await
        .unwrap();
        db.append_run_event(
            "r".into(),
            serde_json::json!({"ev":"vision","template":"a","found":true,"trace":{"frame_id":8}}),
        )
        .await
        .unwrap();
        assert_eq!(
            db.run_event_page("r".into(), 0).await.unwrap()["events"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        drop(db);
        let _ = std::fs::remove_dir_all(dir);
    }
    fn record(id: &str, device: &str) -> crate::run_manager::RunRecord {
        crate::run_manager::RunRecord {
            run_id: id.into(),
            device_id: device.into(),
            runner_id: "test".into(),
            entrypoint: "p/example".into(),
            content_package: Some("p".into()),
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
    async fn exact_terminal_cleanup_is_transactional_and_protects_active_and_task_history() {
        let root = tempfile::tempdir().unwrap();
        let db = Store::open(&Config {
            data_dir: root.path().into(),
            ..Default::default()
        })
        .unwrap();
        let mut ended = record("ended", "d");
        ended.state = crate::run_manager::RunState::Success;
        ended.finished_at = Some(Utc::now());
        db.save_run_record(&ended).unwrap();
        db.save_run_record(&record("active", "d2")).unwrap();
        db.append_run_event("ended".into(), serde_json::json!({"ev":"detail"}))
            .await
            .unwrap();
        assert!(db
            .delete_terminal_run_records(vec!["ended".into(), "active".into()])
            .unwrap_err()
            .to_string()
            .contains("run_active"));
        assert!(db.stored_run("ended".into()).await.unwrap().is_some());
        assert_eq!(
            db.run_event_page("ended".into(), 0).await.unwrap()["events"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let mut task = ended.clone();
        task.run_id = "task".into();
        task.task_id = Some("task-id".into());
        db.save_run_record(&task).unwrap();
        assert!(db
            .delete_terminal_run_records(vec!["ended".into(), "task".into()])
            .unwrap_err()
            .to_string()
            .contains("run_protected"));
        assert_eq!(
            db.delete_terminal_run_records(vec!["ended".into()])
                .unwrap(),
            1
        );
        assert!(db.stored_run("ended".into()).await.unwrap().is_none());
        assert_eq!(
            db.run_event_page("ended".into(), 0).await.unwrap()["events"],
            serde_json::json!([])
        );
        assert!(db.stored_run("task".into()).await.unwrap().is_some());
        assert!(db.stored_run("active".into()).await.unwrap().is_some());
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
            db.run_history(Some("d1".into()), Some("p/example".into()), None, None)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(db
            .run_history(Some("d1".into()), Some("other".into()), None, None)
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
        super::super::validate_schema_v7(&conn).unwrap();
        assert_eq!(
            conn.query_row("SELECT msg FROM logs", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            crate::migrations::TARGET_SCHEMA
        );
    }

    #[test]
    fn v6_tasks_gain_empty_optional_configuration_without_data_loss() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::SCHEMA_V4_DDL).unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
        let before_v7: Vec<_> = crate::migrations::MIGRATIONS
            .iter()
            .filter(|m| m.to <= 6)
            .map(|m| crate::migrations::Migration {
                from: m.from,
                to: m.to,
                description: m.description,
                apply: m.apply,
            })
            .collect();
        crate::migrations::run_migrations(&mut conn, 4, &before_v7).unwrap();
        conn.execute("INSERT INTO timer_tasks(id,name,device_id,runner_id,entrypoint,payload_json,schedule_json,enabled,state,created_at,updated_at) VALUES ('keep','原任务','device','runner','entry','{}','{}',1,'active','2026-09-30','2026-09-30')", []).unwrap();
        crate::migrations::run_migrations(&mut conn, 6, crate::migrations::MIGRATIONS).unwrap();
        super::super::validate_schema_v7(&conn).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT name,extensions_json FROM timer_tasks WHERE id='keep'",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            )
            .unwrap(),
            ("原任务".into(), "{}".into())
        );
    }

    #[tokio::test]
    async fn history_cursor_reaches_older_runs_with_tied_times_and_new_arrivals() {
        let dir = tempfile::tempdir().unwrap();
        let db = Store::open(&Config {
            data_dir: dir.path().into(),
            ..Default::default()
        })
        .unwrap();
        let start = Utc::now();
        for i in 0..65 {
            let mut run = record(&format!("run-{i:03}"), "d1");
            run.started_at = start;
            if i % 2 == 0 {
                run.entrypoint = "p#function".into();
            }
            db.save_run_record(&run).unwrap();
        }
        db.save_run_record(&record("other-device", "d2")).unwrap();
        let first = db
            .run_history(Some("d1".into()), None, None, None)
            .await
            .unwrap();
        assert_eq!(first.len(), 30);
        assert_eq!(first[0]["run_id"], "run-064");
        assert_eq!(first[29]["run_id"], "run-035");
        db.save_run_record(&record("new-arrival", "d1")).unwrap();
        let second = db
            .run_history(Some("d1".into()), None, Some("run-035".into()), None)
            .await
            .unwrap();
        assert_eq!(second.len(), 30);
        assert_eq!(second[0]["run_id"], "run-034");
        assert_eq!(second[29]["run_id"], "run-005");
        let third = db
            .run_history(Some("d1".into()), None, Some("run-005".into()), None)
            .await
            .unwrap();
        assert_eq!(third.len(), 5);
        assert_eq!(third[4]["run_id"], "run-000");
        let functions = db
            .run_history(
                Some("d1".into()),
                Some("p#function".into()),
                Some("run-035".into()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(functions.len(), 18);
        assert!(functions.iter().all(|r| r["entrypoint"] == "p#function"));
        assert!(db
            .run_history(Some("d2".into()), None, Some("run-035".into()), None)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn task_history_filters_before_pagination_and_survives_target_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Store::open(&Config {
            data_dir: dir.path().into(),
            ..Default::default()
        })
        .unwrap();
        let start = Utc::now();
        for i in 0..35 {
            let mut run = record(
                &format!("task-run-{i:03}"),
                if i < 10 {
                    "old-device"
                } else {
                    "current-device"
                },
            );
            run.task_id = Some("task-a".into());
            run.started_at = start;
            if i < 10 {
                run.entrypoint = "p/previous".into();
            }
            db.save_run_record(&run).unwrap();
        }
        for i in 0..40 {
            let mut run = record(&format!("unrelated-{i:03}"), "current-device");
            run.started_at = start + chrono::Duration::seconds(1);
            if i % 2 == 0 {
                run.task_id = Some("task-b".into());
            }
            db.save_run_record(&run).unwrap();
        }
        let first = db
            .run_history(None, None, None, Some("task-a".into()))
            .await
            .unwrap();
        assert_eq!(first.len(), 30);
        assert!(first.iter().all(|run| run["task_id"] == "task-a"));
        assert_eq!(first[0]["run_id"], "task-run-034");
        let older = db
            .run_history(
                None,
                None,
                Some("task-run-005".into()),
                Some("task-a".into()),
            )
            .await
            .unwrap();
        assert_eq!(older.len(), 5);
        assert!(older
            .iter()
            .all(|run| run["device_id"] == "old-device" && run["entrypoint"] == "p/previous"));
        assert!(db
            .run_history(
                None,
                None,
                Some("unrelated-038".into()),
                Some("task-a".into())
            )
            .await
            .unwrap()
            .is_empty());
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
