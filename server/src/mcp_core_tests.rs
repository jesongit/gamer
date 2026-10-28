use super::*;
use crate::mcp::{AllowedObjects, CreateTokenRequest, McpService};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

include!("mcp_core_media_tests.rs");

#[tokio::test]
async fn mcp_schedule_preview_uses_the_registered_provider_without_saving_or_running() {
    let f = fixture().await;
    let p = principal(&f, &["task.read"]);
    let args = json!({"schedule":{"provider_id":"cron","config":{"expression":"*/10 * * * * * *"}},"after":"2026-10-28T00:00:01Z","count":2});
    let preview = data(media_call(&f, &p, "gamer.core.schedule_preview", args.clone()).await);
    assert_eq!(preview["valid"], true);
    assert_eq!(
        preview["next_times"],
        json!(["2026-10-28T00:00:10+00:00", "2026-10-28T00:00:20+00:00"])
    );
    assert_eq!(preview["next_times_local"].as_array().unwrap().len(), 2);
    for (expression, provider, expected) in [
        ("invalid", "cron", "invalid_schedule"),
        ("*", "missing", "schedule_provider_unavailable"),
    ] {
        let preview = data(
            media_call(
                &f,
                &p,
                "gamer.core.schedule_preview",
                json!({"schedule":{"provider_id":provider,"config":{"expression":expression}}}),
            )
            .await,
        );
        assert_eq!(preview["valid"], false);
        assert_eq!(preview["error"], expected);
        assert_eq!(preview["next_times"], json!([]));
    }
    for (field, value) in [
        ("count", json!(11)),
        ("after", json!("2026-10-28T08:00:00")),
    ] {
        let mut invalid = args.clone();
        invalid[field] = value;
        assert!(
            media_call(&f, &p, "gamer.core.schedule_preview", invalid)
                .await
                .is_error
        );
    }
    let denied = principal(&f, &["observe"]);
    assert!(
        media_call(&f, &denied, "gamer.core.schedule_preview", args)
            .await
            .is_error
    );
    assert!(f.core.db.list_timer_tasks_async().await.unwrap().is_empty());
    assert_eq!(f.accepted.0.load(Ordering::SeqCst), 0);
}

struct NoIo;
impl crate::run_manager::RunExecutor for NoIo {
    fn prepare<'a>(
        &'a self,
        _: &'a crate::core::RunContext,
        _: &'a RunRequest,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async { panic!("MCP authorization fixture must never prepare real I/O") })
    }
    fn execute<'a>(
        &'a self,
        _: &'a crate::core::RunContext,
        _: &'a RunRequest,
        _: bool,
        _: Arc<AtomicBool>,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<(String, String)>>> {
        Box::pin(async { panic!("MCP authorization fixture must never execute real I/O") })
    }
    fn acquire(
        &self,
        _: &crate::core::RunContext,
    ) -> anyhow::Result<Box<dyn crate::core::ActivityLease>> {
        panic!("MCP authorization fixture must never acquire real I/O")
    }
}
struct Accepted(AtomicUsize);
#[async_trait::async_trait]
impl crate::timer_core::TimerRunner for Accepted {
    fn runner_id(&self) -> &str {
        "fixture-runner"
    }
    fn supports_external_execution(&self, id: &str) -> bool {
        id == "fixture-runner"
    }
    async fn submit(
        &self,
        _: RunRequest,
        _: &str,
        _: Option<i64>,
        _: crate::timer_core::TimerCompletionHook,
    ) -> Result<crate::timer_core::TimerRun, crate::timer_core::TimerRunnerError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(crate::timer_core::TimerRun::new(
            uuid::Uuid::new_v4().simple().to_string(),
        ))
    }
    async fn cancel(&self, _: &str) -> Result<(), crate::timer_core::TimerRunnerError> {
        panic!("No actual run in this fixture")
    }
}
struct Fixture {
    service: Arc<McpService>,
    core: CoreTools,
    accepted: Arc<Accepted>,
}
async fn fixture() -> Fixture {
    let cfg = crate::config::Config {
        data_dir: std::env::temp_dir().join(format!("gamer-mcp-core-{}", uuid::Uuid::new_v4())),
        ..Default::default()
    };
    std::fs::create_dir_all(&cfg.data_dir).unwrap();
    let db = Arc::new(crate::store::Store::open(&cfg).unwrap());
    let devices = Arc::new(crate::device::DeviceManager::new(db.clone(), cfg.clone()));
    devices
        .upsert_device(&crate::store::Device {
            id: "fixture-device".into(),
            name: "synthetic only".into(),
            addr: "127.0.0.1:1".into(),
            screen_mode: crate::store::ScreenMode::Mirror,
            vd_res: None,
            vd_dpi: None,
            pkg: Some("com.example.synthetic".into()),
            fps: None,
            created_at: "fixture".into(),
        })
        .await
        .unwrap();
    let packages = Arc::new(crate::resources::PackageStore::open(&cfg).unwrap());
    packages.ensure_default_package().unwrap();
    packages.duplicate_package("default", "other").unwrap();
    let runs = Arc::new(crate::run_manager::RunManager::new(Arc::new(NoIo)));
    let scheduler = Arc::new(crate::scheduler::Scheduler::new(db.clone()));
    let accepted = Arc::new(Accepted(AtomicUsize::new(0)));
    scheduler
        .runner_registry()
        .register_runner("fixture-runner", "fixture-plugin", accepted.clone())
        .unwrap();
    let capabilities = crate::capabilities::adapters::build_registry(
        devices.clone(),
        packages.clone(),
        db.clone(),
        runs.clone(),
    );
    let extensions = Arc::new(crate::extensions::ExtensionService::for_data_root(
        cfg.data_dir.clone(),
        capabilities.clone(),
    ));
    let (service, control) = assemble(
        &cfg,
        db.clone(),
        devices.clone(),
        packages.clone(),
        runs.clone(),
        scheduler.clone(),
        extensions.clone(),
        capabilities,
    )
    .unwrap();
    Fixture {
        service,
        accepted,
        core: CoreTools {
            db,
            devices,
            packages,
            runs,
            scheduler,
            extensions,
            control,
            media: crate::media::service(&cfg),
            recordings: crate::recording::service(&cfg),
        },
    }
}
fn principal(f: &Fixture, scopes: &[&str]) -> crate::mcp::ExternalPrincipal {
    let created = f
        .service
        .credentials()
        .create(CreateTokenRequest {
            label: "fixture".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            allowed: AllowedObjects {
                devices: ["fixture-device".into()].into(),
                packages: ["default".into()].into(),
                plugins: ["fixture-plugin".into()].into(),
                runners: ["fixture-runner".into()].into(),
                ..Default::default()
            },
            expires_in_seconds: 3600,
        })
        .unwrap();
    f.service
        .credentials()
        .authenticate(&created.token)
        .unwrap()
}
fn submission() -> Value {
    json!({"device_id":"fixture-device","package_id":"default","runner_id":"fixture-runner","entrypoint":"default/test.yaml","operation_id":"submit-once"})
}
fn task() -> Value {
    json!({"name":"fixture","device_id":"fixture-device","package_id":"default","runner_id":"fixture-runner","entrypoint":"default/test.yaml","schedule":{"provider_id":"cron","config":{"expression":"0 0 0 * * * *"}},"operation_id":"save-task"})
}

#[tokio::test]
async fn core_submission_checks_nested_scope_and_records_only_real_acceptance() {
    let f = fixture().await;
    let p = principal(&f, &["run.submit"]);
    let mut args = submission();
    args["payload"] = json!({"args":{"package_id":"other"}});
    args["operation_id"] = json!("nested-foreign");
    let result = f
        .service
        .call_tool(p.clone(), "gamer.core.run_submit", args)
        .await
        .unwrap();
    assert!(result.is_error);
    assert_eq!(f.accepted.0.load(Ordering::SeqCst), 0);
    let mut args = submission();
    args["entrypoint"] = json!("other/test.yaml");
    args["operation_id"] = json!("foreign-entry");
    assert!(
        f.service
            .call_tool(p.clone(), "gamer.core.run_submit", args)
            .await
            .unwrap()
            .is_error
    );
    let result = f
        .service
        .call_tool(p.clone(), "gamer.core.run_submit", submission())
        .await
        .unwrap();
    assert!(!result.is_error, "{:?}", result);
    let id = result.structured_content.as_ref().unwrap()["run_id"]
        .as_str()
        .unwrap();
    let ownership = f
        .service
        .receipts()
        .object(ObjectKind::Run, id)
        .unwrap()
        .unwrap();
    assert_eq!(ownership.token_id, p.token_id);
    assert_eq!(ownership.scope.package_id.as_deref(), Some("default"));
    let retry = f
        .service
        .call_tool(p, "gamer.core.run_submit", submission())
        .await
        .unwrap();
    assert_eq!(result, retry);
    assert_eq!(f.accepted.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn task_write_does_not_grant_execution_and_other_tokens_cannot_take_ownership() {
    let f = fixture().await;
    let p = principal(&f, &["task.write", "task.control", "task.read"]);
    let mut enabled = task();
    enabled["enabled"] = json!(true);
    enabled["operation_id"] = json!("enabled-save");
    assert!(
        f.service
            .call_tool(p.clone(), "gamer.core.task_save", enabled)
            .await
            .unwrap()
            .is_error
    );
    assert!(f.core.db.list_timer_tasks_async().await.unwrap().is_empty());
    let saved = f
        .service
        .call_tool(p.clone(), "gamer.core.task_save", task())
        .await
        .unwrap();
    assert!(!saved.is_error, "{saved:?}");
    let saved = saved.structured_content.unwrap();
    let id = saved["id"].as_str().unwrap();
    assert_eq!(saved["enabled"], false);
    let run = f
        .service
        .call_tool(
            p.clone(),
            "gamer.core.task_control",
            json!({"task_id":id,"action":"run","operation_id":"run-disabled"}),
        )
        .await
        .unwrap();
    assert!(run.is_error);
    assert_eq!(f.accepted.0.load(Ordering::SeqCst), 0);
    let other = principal(
        &f,
        &["task.read", "task.control", "task.write", "run.submit"],
    );
    assert!(f
        .service
        .call_tool(other, "gamer.core.task_get", json!({"task_id":id}))
        .await
        .is_err());
    assert!(f
        .service
        .call_tool(p, "gamer.core.task_get", json!({"task_id":id}))
        .await
        .is_ok());
}

#[tokio::test]
async fn resource_context_cannot_be_used_to_read_another_package() {
    let f = fixture().await;
    let p = principal(&f, &["resource.read"]);
    assert!(f
        .service
        .call_tool(
            p.clone(),
            "gamer.core.resource_read",
            json!({"package_id":"other","plugin_id":"fixture-plugin","path":"x.yaml"})
        )
        .await
        .is_err());
    let result=f.service.call_tool(p,"gamer.core.resource_read",json!({"package_id":"default","plugin_id":"fixture-plugin","path":"../../package.toml"})).await.unwrap();
    assert!(result.is_error);
    assert_eq!(f.accepted.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn task_snapshot_cas_rejects_a_target_changed_after_authorization_even_with_the_same_timestamp(
) {
    let f = fixture().await;
    let p = principal(
        &f,
        &["task.write", "task.control", "task.read", "run.submit"],
    );
    let saved = f
        .service
        .call_tool(p.clone(), "gamer.core.task_save", task())
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let id = saved["id"].as_str().unwrap();
    let authorized = f.core.db.get_timer_task_async(id).await.unwrap().unwrap();
    let ctx = crate::mcp::InvocationContext::new(
        p.clone(),
        "synthetic-cas".into(),
        Some("cas".into()),
        f.service.credentials(),
        f.service.receipts(),
    );
    ctx.authorize_task(id).unwrap();
    f.core.authorize_task_scope(&ctx, &authorized).unwrap();
    let mut foreign = authorized.clone();
    foreign.app.content_package = Some(AppPackageId::new("other").unwrap());
    foreign.entrypoint = "other/changed.yaml".into();
    // A coarse clock or imported update must not turn timestamp equality into
    // authority for a different execution target.
    foreign.updated_at = authorized.updated_at;
    f.core.db.upsert_timer_task_async(&foreign).await.unwrap();
    let mut enabled = authorized.clone();
    enabled.enabled = true;
    enabled.state = crate::timer_core::TaskState::Active;
    enabled.updated_at = chrono::Utc::now();
    assert_eq!(
        f.core
            .persist_task_snapshot(&ctx, Some(&authorized), &enabled)
            .await
            .unwrap_err()
            .code,
        "task_changed"
    );
    assert_eq!(
        f.core.db.get_timer_task_async(id).await.unwrap().unwrap(),
        foreign
    );
    let result = f
        .service
        .call_tool(
            p,
            "gamer.core.task_control",
            json!({"task_id":id,"action":"enable","operation_id":"foreign-enable"}),
        )
        .await;
    assert!(result.is_err() || result.unwrap().is_error);
    assert_eq!(f.accepted.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_task_saves_with_one_expected_updated_at_have_one_winner() {
    let f = fixture().await;
    let p = principal(&f, &["task.write", "task.read"]);
    let saved = f
        .service
        .call_tool(p.clone(), "gamer.core.task_save", task())
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let id = saved["id"].as_str().unwrap();
    let updated = saved["updated_at"].as_str().unwrap();
    let mut left = task();
    left["task_id"] = json!(id);
    left["expected_updated_at"] = json!(updated);
    left["name"] = json!("left");
    left["operation_id"] = json!("concurrent-left");
    let mut right = left.clone();
    right["name"] = json!("right");
    right["operation_id"] = json!("concurrent-right");
    let (left, right) = tokio::join!(
        f.service.call_tool(p.clone(), "gamer.core.task_save", left),
        f.service.call_tool(p, "gamer.core.task_save", right)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left.is_error, right.is_error);
    let winner = if left.is_error { right } else { left };
    let current = f.core.db.get_timer_task_async(id).await.unwrap().unwrap();
    assert_eq!(
        current.name,
        winner.structured_content.unwrap()["name"].as_str().unwrap()
    );
}

#[tokio::test]
async fn task_cancel_scope_is_bound_to_the_actual_child_run_without_requiring_a_separate_run_grant()
{
    let f = fixture().await;
    let p = principal(&f, &["task.write", "task.control"]);
    let saved = f
        .service
        .call_tool(p.clone(), "gamer.core.task_save", task())
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let id = saved["id"].as_str().unwrap();
    let task = f.core.db.get_timer_task_async(id).await.unwrap().unwrap();
    let ctx = crate::mcp::InvocationContext::new(
        p,
        "synthetic-cancel".into(),
        Some("cancel".into()),
        f.service.credentials(),
        f.service.receipts(),
    );
    ctx.authorize_task(id).unwrap();
    let record = json!({"run_id":"scheduled-child","task_id":id,"device_id":"fixture-device","content_package":"default","runner_id":"fixture-runner","entrypoint":"default/test.yaml"});
    assert!(ctx.authorize_run("scheduled-child").is_err());
    f.core.authorize_task_run(&ctx, &task, &record).unwrap();
    for (key, value) in [
        ("task_id", "another-task"),
        ("device_id", "foreign-device"),
        ("content_package", "other"),
        ("runner_id", "foreign-runner"),
        ("entrypoint", "default/another.yaml"),
    ] {
        let mut changed = record.clone();
        changed[key] = json!(value);
        assert!(
            f.core.authorize_task_run(&ctx, &task, &changed).is_err(),
            "{key}"
        );
    }
}

#[tokio::test]
async fn revoked_admission_waiting_on_a_real_sqlite_write_lock_never_saves_the_task() {
    let f = fixture().await;
    let p = principal(&f, &["task.write"]);
    let saved = f
        .service
        .call_tool(p.clone(), "gamer.core.task_save", task())
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let task = f
        .core
        .db
        .get_timer_task_async(saved["id"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    let mut next = task.clone();
    next.name = "must never commit".into();
    next.updated_at = chrono::Utc::now();
    let ctx = crate::mcp::InvocationContext::new(
        p.clone(),
        "synthetic-admission".into(),
        Some("admission".into()),
        f.service.credentials(),
        f.service.receipts(),
    );
    let checks = Arc::new(AtomicUsize::new(0));
    let observed = checks.clone();
    let check: crate::store::TaskWriteAdmission = Arc::new(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        ctx.check_active().map_err(anyhow::Error::new)
    });
    let lock = rusqlite::Connection::open(f.core.packages.data_root().join("gamer.db")).unwrap();
    lock.execute_batch("BEGIN IMMEDIATE").unwrap();
    let save = f
        .core
        .db
        .compare_save_timer_task_async(&next, Some(&task), Some(check));
    let revoke = async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(
            checks.load(Ordering::SeqCst),
            0,
            "Admission must wait until the actual database write lock is acquired"
        );
        f.service.credentials().revoke(&p.token_id).unwrap();
        lock.execute_batch("ROLLBACK").unwrap();
    };
    let (saved, _) = tokio::join!(save, revoke);
    assert!(saved.is_err());
    assert_eq!(checks.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.core
            .db
            .get_timer_task_async(&task.id)
            .await
            .unwrap()
            .unwrap(),
        task
    );
}

#[tokio::test]
async fn missing_task_runner_requires_a_matching_frozen_owner_or_explicit_all_plugins() {
    let f = fixture().await;
    let p = principal(&f, &["task.write", "task.read"]);
    let saved = f
        .service
        .call_tool(p.clone(), "gamer.core.task_save", task())
        .await
        .unwrap()
        .structured_content
        .unwrap();
    let task = f
        .core
        .db
        .get_timer_task_async(saved["id"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    f.core
        .scheduler
        .runner_registry()
        .unregister_runner("fixture-runner")
        .unwrap();
    let owned = crate::mcp::InvocationContext::new(
        p.clone(),
        "synthetic-owner".into(),
        None,
        f.service.credentials(),
        f.service.receipts(),
    );
    assert_eq!(
        f.core
            .authorize_task_scope(&owned, &task)
            .unwrap()
            .plugin_id
            .as_deref(),
        Some("fixture-plugin")
    );
    let mut unowned = task;
    unowned.id = "preexisting-task".into();
    for wildcard in [false, true] {
        let mut allowed = p.allowed.clone();
        allowed.tasks.insert(unowned.id.clone());
        if wildcard {
            allowed.plugins = ["*".into()].into();
        }
        let token = f
            .service
            .credentials()
            .create(CreateTokenRequest {
                label: "historical".into(),
                scopes: ["task.read".into()].into(),
                allowed,
                expires_in_seconds: 60,
            })
            .unwrap();
        let principal = f.service.credentials().authenticate(&token.token).unwrap();
        let ctx = crate::mcp::InvocationContext::new(
            principal,
            "synthetic-owner".into(),
            None,
            f.service.credentials(),
            f.service.receipts(),
        );
        assert_eq!(
            f.core.authorize_task_scope(&ctx, &unowned).is_ok(),
            wildcard
        );
    }
}

#[tokio::test]
async fn history_cursor_advances_over_a_page_without_disclosing_unauthorized_records() {
    let f = fixture().await;
    let mut allowed = principal(&f, &["run.read"]).allowed;
    allowed.runs = ["authorized-old-run".into()].into();
    let token = f
        .service
        .credentials()
        .create(CreateTokenRequest {
            label: "history pagination".into(),
            scopes: ["run.read".into()].into(),
            allowed,
            expires_in_seconds: 60,
        })
        .unwrap();
    let principal = f.service.credentials().authenticate(&token.token).unwrap();
    for offset in 0..31 {
        let record = crate::run_manager::RunRecord {
            run_id: if offset == 30 {
                "authorized-old-run".into()
            } else {
                format!("hidden-{offset}")
            },
            device_id: "fixture-device".into(),
            runner_id: "fixture-runner".into(),
            entrypoint: "default/test.yaml".into(),
            content_package: Some("default".into()),
            script_id: "default/test.yaml".into(),
            source: crate::run_manager::RunSource::Manual,
            task_id: None,
            scheduled_at: None,
            state: crate::run_manager::RunState::Success,
            started_at: chrono::Utc::now() - chrono::Duration::seconds(offset),
            finished_at: Some(chrono::Utc::now()),
            error: None,
        };
        f.core.db.save_run_record(&record).unwrap();
    }
    let page = f
        .service
        .call_tool(
            principal.clone(),
            "gamer.core.run_history",
            json!({"device_id":"fixture-device"}),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(page["runs"], json!([]));
    let cursor = page["next_before"]
        .as_str()
        .expect("A filtered page must still be traversable");
    let denied = f
        .service
        .call_tool(
            principal.clone(),
            "gamer.core.run_status",
            json!({"run_id":cursor}),
        )
        .await;
    assert!(
        denied.is_err() || denied.unwrap().is_error,
        "A cursor grants no run permission"
    );
    let page = f
        .service
        .call_tool(
            principal.clone(),
            "gamer.core.run_history",
            json!({"device_id":"fixture-device","before":cursor}),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(page["runs"].as_array().unwrap().len(), 1);
    assert_eq!(page["runs"][0]["run_id"], "authorized-old-run");
    let end = f
        .service
        .call_tool(
            principal,
            "gamer.core.run_history",
            json!({"device_id":"fixture-device","before":page["next_before"]}),
        )
        .await
        .unwrap()
        .structured_content
        .unwrap();
    assert_eq!(end["runs"], json!([]));
    assert!(end["next_before"].is_null());
}

#[tokio::test]
async fn missing_run_runner_rejects_incomplete_or_mismatched_frozen_owner_receipts() {
    let f = fixture().await;
    let mut allowed = principal(&f, &["run.read"]).allowed;
    allowed.runners = ["*".into()].into();
    let token = f
        .service
        .credentials()
        .create(CreateTokenRequest {
            label: "frozen history owner".into(),
            scopes: ["run.read".into()].into(),
            allowed,
            expires_in_seconds: 60,
        })
        .unwrap();
    let p = f.service.credentials().authenticate(&token.token).unwrap();
    let ctx = InvocationContext::new(
        p.clone(),
        "historical-owner".into(),
        None,
        f.service.credentials(),
        f.service.receipts(),
    );
    f.core
        .scheduler
        .runner_registry()
        .unregister_runner("fixture-runner")
        .unwrap();
    for (id, owner, runner, accepted) in [
        ("no-owner", None, Some("fixture-runner"), false),
        ("empty-owner", Some(""), Some("fixture-runner"), false),
        (
            "wrong-runner",
            Some("fixture-plugin"),
            Some("another-runner"),
            false,
        ),
        ("no-runner", Some("fixture-plugin"), None, false),
        (
            "matching-owner",
            Some("fixture-plugin"),
            Some("fixture-runner"),
            true,
        ),
    ] {
        f.service
            .receipts()
            .record_object(
                ObjectKind::Run,
                id,
                &p.token_id,
                &ObjectScope {
                    device_id: Some("fixture-device".into()),
                    package_id: Some("default".into()),
                    plugin_id: owner.map(str::to_owned),
                    runner_id: runner.map(str::to_owned),
                },
            )
            .unwrap();
        let record = json!({"run_id":id,"device_id":"fixture-device","content_package":"default","runner_id":"fixture-runner"});
        assert_eq!(
            f.core.authorize_record(&ctx, &record).is_ok(),
            accepted,
            "{id}"
        );
    }
}
