//! System update API（SYS-004 / release/contracts/system-api-v1.md §3–§7 冻结）。
//!
//! 端点（全部挂在受保护组：登录 + 同源两道门禁由 auth_guard 统一执行）：
//! - `GET  /api/system/update`          → 200 状态聚合（11 态 + blocking 详情走这里）
//! - `POST /api/system/update/check`    → 202 受理（幂等）
//! - `POST /api/system/update/download` → 202 受理（幂等）
//! - `POST /api/system/update/install`  → 202 受理（非幂等；门禁/并发 409）
//! - `POST /api/system/update/rollback` → 202 受理（非幂等；并发第二个 409）
//! - `PUT  /api/system/update/policy`   → 200 整对象替换（幂等）
//!
//! 错误体统一 `{code, message, details?}`（§1.2），11 个业务错误码的状态码
//! 映射冻结（§7）；policy 校验失败走 400 `invalid_argument`（§6，不计 11 码）。
//! fixture 键集递归比对测试见文件尾 contract_tests。

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::AppState;
use crate::update::policy::UpdatePolicy;
use crate::update::service::error_json;

/// 业务错误 → HTTP 响应（状态码映射冻结在 UpdateErrorCode::http_status）
fn error_response(err: &crate::update::ipc::UpdateError) -> Response {
    let status = StatusCode::from_u16(err.code.http_status()).unwrap_or(StatusCode::CONFLICT);
    (status, Json(error_json(err))).into_response()
}

/// GET /api/system/update（只读；launcher 不可达时以最近缓存降级，永不 5xx）
pub(super) async fn api_get_update(State(st): State<AppState>) -> Response {
    let body = st.update.status_body().await;
    (StatusCode::OK, Json(body)).into_response()
}

/// POST /api/system/update/check
pub(super) async fn api_update_check(State(st): State<AppState>) -> Response {
    handle_action(st.update.request_check().await)
}

/// POST /api/system/update/download
pub(super) async fn api_update_download(State(st): State<AppState>) -> Response {
    handle_action(st.update.request_download().await)
}

/// POST /api/system/update/install（门禁判定 + 202 先行返回；SYS-006 后台接线）
pub(super) async fn api_update_install(State(st): State<AppState>) -> Response {
    handle_action(st.update.request_install().await)
}

/// POST /api/system/update/rollback
pub(super) async fn api_update_rollback(State(st): State<AppState>) -> Response {
    handle_action(st.update.request_rollback().await)
}

fn handle_action(result: Result<serde_json::Value, crate::update::ipc::UpdateError>) -> Response {
    match result {
        Ok(body) => (StatusCode::ACCEPTED, Json(body)).into_response(),
        Err(e) => error_response(&e),
    }
}

/// PUT /api/system/update/policy：整对象替换（幂等）；校验失败 400 invalid_argument
pub(super) async fn api_update_policy(
    State(st): State<AppState>,
    Json(policy): Json<UpdatePolicy>,
) -> Response {
    match st.update.set_policy(policy).await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(validation) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": "invalid_argument",
                "message": "策略字段非法",
                "details": { "field": validation.field },
            })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::config::Config;
    use crate::device::DeviceManager;
    use crate::scheduler::Scheduler;
    use crate::scripts::ScriptStore;
    use crate::store::Db;
    use crate::update::controller::mock::MockController;
    use crate::update::ipc::{Candidate, LastErrorCodeMessage, LauncherUpdateStatus};
    use crate::update::model::{UpdateErrorCode, UpdateState};
    use crate::update::policy::{PolicyStore, PolicyValidationError, UpdatePolicy};
    use crate::update::service::{status_json, UpdateService, UpdateTxn, WorkloadProvider};
    use crate::update::workload::Workload;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    /// fixture 读取（request/response 包装拆开）
    fn fixture(name: &str) -> serde_json::Value {
        let path = format!("../release/contracts/fixtures/system-api/{name}");
        let raw =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse fixture {name}: {e}"))
    }

    fn fixture_body(name: &str) -> serde_json::Value {
        fixture(name)["response"]["body"].clone()
    }

    /// 递归比对「字段集」：对象键集必须完全一致（多字段/少字段都算契约破坏），
    /// 数组按元素比对；标量只比对类型存在性（值由运行环境决定）
    fn assert_same_field_sets(fixture: &serde_json::Value, actual: &serde_json::Value, path: &str) {
        match (fixture, actual) {
            (serde_json::Value::Object(f), serde_json::Value::Object(a)) => {
                let fk: BTreeSet<&String> = f.keys().collect();
                let ak: BTreeSet<&String> = a.keys().collect();
                assert_eq!(
                    fk, ak,
                    "字段集不一致 @ {path}: fixture={fk:?} actual={ak:?}"
                );
                for (key, fv) in f {
                    assert_same_field_sets(fv, &a[key], &format!("{path}.{key}"));
                }
            }
            (serde_json::Value::Array(f), serde_json::Value::Array(a)) => {
                assert_eq!(f.len(), a.len(), "数组长度不一致 @ {path}");
                for (i, (fv, av)) in f.iter().zip(a.iter()).enumerate() {
                    assert_same_field_sets(fv, av, &format!("{path}[{i}]"));
                }
            }
            (serde_json::Value::Object(_), _) | (serde_json::Value::Array(_), _) => {
                panic!("结构类型不一致 @ {path}: fixture={fixture} actual={actual}");
            }
            _ => {}
        }
    }

    fn policy() -> UpdatePolicy {
        UpdatePolicy::default()
    }

    fn candidate() -> Candidate {
        Candidate {
            version: "0.3.0".into(),
            channel: "stable".into(),
            published_at: Some("2026-09-15T00:00:00Z".into()),
            size_bytes: Some(893_451_200),
            release_notes_url: Some("https://example.invalid/releases/v0.3.0".into()),
        }
    }

    const UPDATED_AT: &str = "2026-08-31T12:00:00Z";
    const UPD_ID: &str = "upd-20260831-9f3ab2c1";

    /// GET 200 staged 形态（system-update.success）
    #[test]
    fn get_body_matches_success_fixture_field_set() {
        let status = LauncherUpdateStatus {
            state: Some(UpdateState::Staged),
            detail: Some("staged".into()),
            update_id: Some(UPD_ID.into()),
            candidate: Some(candidate()),
            progress: None,
            last_error: None,
        };
        let body = status_json(&status, &policy(), UPDATED_AT);
        assert_same_field_sets(&fixture_body("system-update.success.json"), &body, "$");
        assert_eq!(body["state"], "staged");
        assert_eq!(body["update_id"], UPD_ID);
        assert_eq!(body["progress"], serde_json::Value::Null);
        assert_eq!(body["last_error"], serde_json::Value::Null);
    }

    /// GET 200 failed + signature_invalid / artifact_invalid（异步失败形态）
    #[test]
    fn get_body_matches_failed_fixtures_field_sets() {
        for (name, code) in [
            (
                "system-update.failed-signature-invalid.json",
                "signature_invalid",
            ),
            (
                "system-update.failed-artifact-invalid.json",
                "artifact_invalid",
            ),
        ] {
            let status = LauncherUpdateStatus {
                state: Some(UpdateState::Failed),
                detail: Some("failed".into()),
                update_id: Some(UPD_ID.into()),
                candidate: Some(candidate()),
                progress: None,
                last_error: Some(LastErrorCodeMessage {
                    code: code.into(),
                    message: "下载产物完整性校验失败".into(),
                }),
            };
            let body = status_json(&status, &policy(), UPDATED_AT);
            assert_same_field_sets(&fixture_body(name), &body, "$");
            assert_eq!(body["last_error"]["code"], code);
        }
    }

    /// GET 200 manual_recovery（唯一无自动迁出终态）
    #[test]
    fn get_body_matches_manual_recovery_fixture_field_set() {
        let status = LauncherUpdateStatus {
            state: Some(UpdateState::ManualRecovery),
            detail: Some("manual_recovery_required".into()),
            update_id: Some(UPD_ID.into()),
            candidate: Some(candidate()),
            progress: None,
            last_error: Some(LastErrorCodeMessage {
                code: "manual_recovery_required".into(),
                message: "请按维护手册执行人工恢复".into(),
            }),
        };
        let body = status_json(&status, &policy(), UPDATED_AT);
        assert_same_field_sets(
            &fixture_body("system-update.manual-recovery.json"),
            &body,
            "$",
        );
    }

    /// 动作 202 受理体（check/download/install/rollback 同构 {update_id, state}）
    #[test]
    fn accepted_body_matches_all_four_success_fixtures() {
        let cases = [
            ("update-check.success.json", UpdateState::Checking),
            ("update-download.success.json", UpdateState::Downloading),
            ("update-install.success.json", UpdateState::Installing),
            ("update-rollback.success.json", UpdateState::RollingBack),
        ];
        for (name, state) in cases {
            let body = crate::update::service::accepted_json(UPD_ID, state);
            assert_same_field_sets(&fixture_body(name), &body, "$");
            assert_eq!(body["state"], state.as_str());
        }
    }

    /// 409 update_busy（并发第二个 install）
    #[test]
    fn busy_body_matches_fixture_field_set() {
        let err = crate::update::ipc::UpdateError::new(
            UpdateErrorCode::UpdateBusy,
            "已有升级事务正在进行，请等待其结束后再试",
        );
        let body = error_json(&err);
        assert_same_field_sets(&fixture_body("update-install.update-busy.json"), &body, "$");
        assert_eq!(body["code"], "update_busy");
        assert_eq!(err.code.http_status(), 409);
    }

    /// 409 update_not_managed（docker/direct 固定拒绝）
    #[test]
    fn not_managed_body_matches_fixture_field_set() {
        let err = crate::update::ipc::UpdateError::new(
            UpdateErrorCode::UpdateNotManaged,
            crate::update::service::NOT_MANAGED_MESSAGE,
        );
        let body = error_json(&err);
        assert_same_field_sets(
            &fixture_body("update-install.update-not-managed.json"),
            &body,
            "$",
        );
        assert_eq!(err.code.http_status(), 409);
    }

    /// 409 update_not_ready（blocking 全量列出）
    #[test]
    fn not_ready_body_matches_fixture_field_set() {
        let err =
            crate::update::ipc::UpdateError::new(UpdateErrorCode::UpdateNotReady, "安装条件未满足")
                .with_details(
                    serde_json::json!({ "blocking": ["active_run", "cron_freeze_window"] }),
                );
        let body = error_json(&err);
        assert_same_field_sets(
            &fixture_body("update-install.update-not-ready.json"),
            &body,
            "$",
        );
        assert_eq!(body["details"]["blocking"][0], "active_run");
    }

    /// 422 schema_incompatible（candidate_schema + supported_range 冻结键）
    #[test]
    fn schema_incompatible_body_matches_fixture_field_set() {
        let err = crate::update::ipc::UpdateError::new(
            UpdateErrorCode::SchemaIncompatible,
            "候选版本的数据 schema 超出当前程序可升级范围",
        )
        .with_details(serde_json::json!({
            "candidate_schema": 4,
            "supported_range": [1, 3],
        }));
        let body = error_json(&err);
        assert_same_field_sets(
            &fixture_body("update-install.schema-incompatible.json"),
            &body,
            "$",
        );
        assert_eq!(err.code.http_status(), 422);
    }

    /// 401/403 中间件固定体（unauthorized / forbidden_origin 逐字段一致）
    #[test]
    fn unauthorized_and_forbidden_origin_bodies_match_fixtures() {
        assert_eq!(
            fixture_body("update-install.unauthorized.json"),
            serde_json::json!({ "error": "unauthorized" })
        );
        assert_eq!(
            fixture_body("system-update.unauthorized.json"),
            serde_json::json!({ "error": "unauthorized" })
        );
        assert_eq!(
            fixture_body("update-install.forbidden-origin.json"),
            serde_json::json!({ "error": "forbidden_origin" })
        );
    }

    /// 502 launcher_unreachable（无 details）
    #[test]
    fn launcher_unreachable_body_matches_fixture_field_set() {
        let err = crate::update::ipc::UpdateError::new(
            UpdateErrorCode::LauncherUnreachable,
            "无法连接升级器，请确认 launcher 正在运行后重试",
        );
        let body = error_json(&err);
        assert_same_field_sets(
            &fixture_body("update-check.launcher-unreachable.json"),
            &body,
            "$",
        );
        assert_eq!(err.code.http_status(), 502);
    }

    /// PUT policy 200 回显 + 400 invalid_argument（details.field）
    #[test]
    fn policy_bodies_match_fixtures() {
        let saved = UpdatePolicy {
            strategy: crate::update::policy::UpdateStrategy::Auto,
            ..Default::default()
        };
        let body = PolicyStore::to_json(&saved);
        assert_same_field_sets(&fixture_body("update-policy.success.json"), &body, "$");
        assert_eq!(body["strategy"], "auto");

        let validation = PolicyValidationError {
            field: "maintenance_window",
        };
        let body = serde_json::json!({
            "code": "invalid_argument",
            "message": "维护窗口起止时间不能相同",
            "details": { "field": validation.field },
        });
        assert_same_field_sets(
            &fixture_body("update-policy.invalid-argument.json"),
            &body,
            "$",
        );
    }

    /// 507 insufficient_space（required/available 冻结键，无路径）
    #[test]
    fn insufficient_space_body_matches_fixture_field_set() {
        let err = crate::update::ipc::UpdateError::new(
            UpdateErrorCode::InsufficientSpace,
            "磁盘空间不足",
        )
        .with_details(serde_json::json!({
            "required_bytes": 2_684_354_560i64,
            "available_bytes": 1_073_741_824i64,
        }));
        let body = error_json(&err);
        assert_same_field_sets(
            &fixture_body("update-download.insufficient-space.json"),
            &body,
            "$",
        );
        assert_eq!(err.code.http_status(), 507);
    }

    /// 409 update_not_available（download 无候选）
    #[test]
    fn not_available_body_matches_fixture_field_set() {
        let err = crate::update::ipc::UpdateError::new(
            UpdateErrorCode::UpdateNotAvailable,
            "当前没有可下载的更新候选，请先执行检查更新",
        );
        let body = error_json(&err);
        assert_same_field_sets(
            &fixture_body("update-download.update-not-available.json"),
            &body,
            "$",
        );
    }

    // ---------- 路由级集成（mock controller 注入） ----------

    struct TestApp {
        app: axum::Router,
        dir: std::path::PathBuf,
    }

    impl Drop for TestApp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn build_app_with_controller(tag: &str, controller: Arc<MockController>) -> TestApp {
        let dir = std::env::temp_dir().join(format!(
            "gamer-update-api-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let scripts = Arc::new(ScriptStore::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone()));
        let executor = Arc::new(crate::extensions::gamer_yaml::engine::EngineExecutor::new(
            Arc::new(crate::extensions::gamer_yaml::engine::Runner::new(
                devices.clone(),
                Arc::new(crate::webrtc::ViewerEventSink::new(viewers.clone())),
                scripts.clone(),
            )),
            devices.clone(),
            db.clone(),
        ));
        let runs = Arc::new(crate::run_manager::RunManager::new(executor));
        let scheduler = Arc::new(Scheduler::new(db.clone()));
        let auth = Arc::new(super::super::auth::AuthState::new(
            super::super::auth::parse_password_hash(
                &super::super::auth::hash_password("admin123").unwrap(),
            )
            .unwrap(),
            Default::default(),
            false,
            Some("test-token".into()),
        ));
        let shutdown = Arc::new(crate::shutdown::ShutdownCoordinator::new(Arc::new(|| {
            Box::pin(async {})
        })));

        // 更新服务：mock controller（managed 能力）
        let policy_store = PolicyStore::load_blocking(&cfg.data_dir, UpdatePolicy::default());
        let workload: WorkloadProvider = Arc::new(Workload::default);
        let update = Arc::new(UpdateService::new(
            controller,
            policy_store,
            Arc::new(UpdateTxn::default()),
            workload,
            db.clone(),
        ));

        let app = super::super::build_router(
            db, devices, runs, scheduler, cfg, viewers, scripts, shutdown, auth, update,
        );
        TestApp { app, dir }
    }

    use axum::body::Body;
    use axum::http::{header, Request as HttpRequest, StatusCode as HttpStatus};
    use tower::ServiceExt;

    fn req(
        method: &str,
        uri: &str,
        headers: &[(header::HeaderName, &str)],
        body: Option<String>,
    ) -> HttpRequest<Body> {
        let mut b = HttpRequest::builder().method(method).uri(uri);
        for (k, v) in headers {
            b = b.header(k, *v);
        }
        match body {
            Some(s) => b.body(Body::from(s)).unwrap(),
            None => b.body(Body::empty()).unwrap(),
        }
    }

    async fn send(app: &axum::Router, r: HttpRequest<Body>) -> axum::http::Response<Body> {
        app.clone().oneshot(r).await.unwrap()
    }

    async fn json_of(resp: axum::http::Response<Body>) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 256 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    async fn login(app: &axum::Router) -> String {
        let resp = send(
            app,
            req(
                "POST",
                "/api/login",
                &[(header::CONTENT_TYPE, "application/json")],
                Some(r#"{"username":"admin","password":"admin123"}"#.into()),
            ),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::OK);
        resp.headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    fn cookie_header(sid: &str) -> (&'static header::HeaderName, String) {
        (&header::COOKIE, sid.to_string())
    }

    /// 未登录 401 / 跨站 403（状态变更端点同受 auth_guard 约束）
    #[tokio::test]
    async fn update_endpoints_require_login_and_same_origin() {
        let controller = Arc::new(MockController::new());
        let t = build_app_with_controller("guards", controller).await;

        // 未登录：GET 与 POST 一律 401 {"error":"unauthorized"}
        for (method, uri) in [
            ("GET", "/api/system/update"),
            ("POST", "/api/system/update/install"),
            ("PUT", "/api/system/update/policy"),
        ] {
            let resp = send(&t.app, req(method, uri, &[], None)).await;
            assert_eq!(resp.status(), HttpStatus::UNAUTHORIZED, "{method} {uri}");
            assert_eq!(json_of(resp).await["error"], "unauthorized");
        }

        // 已登录但 Origin≠Host：状态变更 403 {"error":"forbidden_origin"}
        let sid = login(&t.app).await;
        let resp = send(
            &t.app,
            req(
                "POST",
                "/api/system/update/install",
                &[
                    (header::COOKIE, sid.as_str()),
                    (header::ORIGIN, "https://evil.example"),
                    (header::HOST, "gamebot.local:8443"),
                ],
                Some("{}".into()),
            ),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::FORBIDDEN);
        assert_eq!(json_of(resp).await["error"], "forbidden_origin");
    }

    /// Docker external 模式保留状态查询；四个动作一律 409 update_not_managed。
    #[tokio::test]
    async fn docker_external_mode_exposes_status_query_and_rejects_actions_with_409() {
        let dir = std::env::temp_dir().join(format!(
            "gamer-update-api-unmanaged-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let scripts = Arc::new(ScriptStore::open(&cfg).unwrap());
        let viewers: crate::webrtc::ViewerMap =
            Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let devices = Arc::new(DeviceManager::new(db.clone(), cfg.clone()));
        let executor = Arc::new(crate::extensions::gamer_yaml::engine::EngineExecutor::new(
            Arc::new(crate::extensions::gamer_yaml::engine::Runner::new(
                devices.clone(),
                Arc::new(crate::webrtc::ViewerEventSink::new(viewers.clone())),
                scripts.clone(),
            )),
            devices.clone(),
            db.clone(),
        ));
        let runs = Arc::new(crate::run_manager::RunManager::new(executor));
        let scheduler = Arc::new(Scheduler::new(db.clone()));
        let auth = Arc::new(super::super::auth::AuthState::new(
            super::super::auth::parse_password_hash(
                &super::super::auth::hash_password("admin123").unwrap(),
            )
            .unwrap(),
            Default::default(),
            false,
            Some("test-token".into()),
        ));
        let shutdown = Arc::new(crate::shutdown::ShutdownCoordinator::new(Arc::new(|| {
            Box::pin(async {})
        })));
        let policy_store = PolicyStore::load_blocking(&cfg.data_dir, UpdatePolicy::default());
        let workload: WorkloadProvider = Arc::new(Workload::default);
        let update = Arc::new(UpdateService::new(
            Arc::new(crate::update::controller::DockerController),
            policy_store,
            Arc::new(UpdateTxn::default()),
            workload,
            db.clone(),
        ));
        let app = super::super::build_router(
            db, devices, runs, scheduler, cfg, viewers, scripts, shutdown, auth, update,
        );

        let sid = login(&app).await;
        let (name, value) = cookie_header(&sid);
        let resp = send(
            &app,
            req(
                "GET",
                "/api/system/update",
                &[(name.clone(), value.as_str())],
                None,
            ),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::OK);
        assert_eq!(json_of(resp).await["state"], "idle");

        for uri in [
            "/api/system/update/check",
            "/api/system/update/download",
            "/api/system/update/install",
            "/api/system/update/rollback",
        ] {
            let (name, value) = cookie_header(&sid);
            let resp = send(
                &app,
                req(
                    "POST",
                    uri,
                    &[(name.clone(), value.as_str())],
                    Some("{}".into()),
                ),
            )
            .await;
            assert_eq!(resp.status(), HttpStatus::CONFLICT, "{uri}");
            let body = json_of(resp).await;
            assert_eq!(body["code"], "update_not_managed", "{uri}: {body}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// GET 聚合经 launcher mock（staged 全量 → 200 契约形态）
    #[tokio::test]
    async fn get_update_aggregates_launcher_status() {
        let controller = Arc::new(MockController::new());
        controller.set_status(LauncherUpdateStatus {
            state: Some(UpdateState::Staged),
            detail: Some("staged".into()),
            update_id: Some(UPD_ID.into()),
            candidate: Some(candidate()),
            progress: None,
            last_error: None,
        });
        let t = build_app_with_controller("get-staged", controller).await;
        let sid = login(&t.app).await;
        let (name, value) = cookie_header(&sid);
        let resp = send(
            &t.app,
            req(
                "GET",
                "/api/system/update",
                &[(name.clone(), value.as_str())],
                None,
            ),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::OK);
        let body = json_of(resp).await;
        assert_same_field_sets(&fixture_body("system-update.success.json"), &body, "$");
        assert_eq!(body["state"], "staged");
    }

    /// PUT policy：合法保存回显 200；非法（start==end）400 invalid_argument + field
    #[tokio::test]
    async fn policy_put_roundtrip_and_validation() {
        let controller = Arc::new(MockController::new());
        let t = build_app_with_controller("policy", controller).await;
        let sid = login(&t.app).await;
        let (name, value) = cookie_header(&sid);

        // 合法：auto + 跨午夜窗口
        let resp = send(
            &t.app,
            req(
                "PUT",
                "/api/system/update/policy",
                &[
                    (name.clone(), value.as_str()),
                    (header::CONTENT_TYPE, "application/json"),
                ],
                Some(
                    serde_json::json!({
                        "strategy": "auto",
                        "maintenance_window": { "start": "23:00", "end": "05:00" },
                        "freeze_window_minutes": 15,
                    })
                    .to_string(),
                ),
            ),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::OK, "{}", json_of(resp).await);
        let body = json_of(resp).await;
        assert_same_field_sets(&fixture_body("update-policy.success.json"), &body, "$");
        assert_eq!(body["strategy"], "auto");

        // 幂等回读：GET 的 policy 块与保存一致
        let resp = send(
            &t.app,
            req(
                "GET",
                "/api/system/update",
                &[(name.clone(), value.as_str())],
                None,
            ),
        )
        .await;
        let body = json_of(resp).await;
        assert_eq!(body["policy"]["strategy"], "auto");
        assert_eq!(body["policy"]["maintenance_window"]["start"], "23:00");
        assert_eq!(body["policy"]["freeze_window_minutes"], 15);

        // 非法：start == end → 400 invalid_argument + details.field
        let resp = send(
            &t.app,
            req(
                "PUT",
                "/api/system/update/policy",
                &[
                    (name.clone(), value.as_str()),
                    (header::CONTENT_TYPE, "application/json"),
                ],
                Some(
                    serde_json::json!({
                        "strategy": "auto",
                        "maintenance_window": { "start": "02:00", "end": "02:00" },
                        "freeze_window_minutes": 30,
                    })
                    .to_string(),
                ),
            ),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::BAD_REQUEST);
        let body = json_of(resp).await;
        assert_same_field_sets(
            &fixture_body("update-policy.invalid-argument.json"),
            &body,
            "$",
        );
        assert_eq!(body["details"]["field"], "maintenance_window");
    }
}
