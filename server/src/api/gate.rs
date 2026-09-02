//! candidate activation gate 路由（OPS-004）。
//!
//! `GAMER_ACTIVATION_GATE=1` 时 main 以本路由启动 HTTP 服务：
//! - 放行 `/health/live`、`/health/ready`（503 ready:false 契约 not-ready 形态）、
//!   `/health/shutdown`、`POST /api/system/activate`（X-Launcher-Token ==
//!   GAMER_LAUNCHER_IPC_TOKEN，仅回环）；
//! - 其余一切路径（业务读写 API）→ 503 `{"code":"update_not_ready",...}`；
//! - activate 成功 → main 的初始化任务完成完整初始化 → [`GateShared`] 换入完整
//!   路由 → 所有请求（除 activate 幂等回执）转发完整路由。
//!
//! 换入机制：顶层 `from_fn` 中间件在完整路由就位后转发请求（含 /health/ready
//! 翻转为真实探针）；`/api/system/activate` 恒留在本路由以便 launcher 重复调用
//! 幂等回执（200 + 当前 stage）。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use tower::ServiceExt;

use crate::config::Config;
use crate::shutdown::ShutdownCoordinator;
use crate::store::Db;
use crate::update::gate::{set_stage, ActivateReject, StartupGate, STAGE_MAINTENANCE_GATE};

/// 完整路由的换入槽（activate 初始化任务写，请求中间件读）
#[derive(Default)]
pub struct GateShared {
    full: std::sync::RwLock<Option<Router>>,
}

impl GateShared {
    pub fn set(&self, router: Router) {
        *self.full.write().unwrap() = Some(router);
    }

    fn get(&self) -> Option<Router> {
        self.full.read().unwrap().clone()
    }
}

/// 闸内路由的请求态（依赖最小化：探针/停机状态/激活判定三件）
#[derive(Clone)]
pub struct GateDeps {
    pub cfg: Config,
    pub db: Db,
    pub shutdown: Arc<ShutdownCoordinator>,
    pub gate: Arc<StartupGate>,
}

/// 闸内 /health/ready：503 + 契约 §8 冻结字段集（ready:false；闸内依赖探针
/// 未运行——data_dir/sqlite/scrcpy 为廉价检查，adb/ffmpeg 探针延后到完整路由）
async fn gate_health_ready(State(deps): State<GateDeps>) -> Response {
    let data_dir_ok = deps.cfg.data_dir.is_dir();
    let scrcpy_ok = deps.cfg.scrcpy_server.is_file();
    let db_ok = deps.db.health_check_async().await.is_ok();
    let body = serde_json::json!({
        "ready": false,
        "checks": {
            "data_dir": { "ok": data_dir_ok },
            "sqlite": { "ok": db_ok },
            "scrcpy_server": { "ok": scrcpy_ok },
            "adb": { "ok": false },
            "ffmpeg": { "ok": false },
        }
    });
    (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
}

/// 闸内 /health/shutdown：与完整路由同形（复用协调器状态，依赖最小化）
async fn gate_shutdown_state(State(deps): State<GateDeps>) -> Response {
    let state = deps.shutdown.state();
    Json(serde_json::json!({
        "state": state.as_str(),
        "drained": state == crate::shutdown::ShutdownState::Finished,
    }))
    .into_response()
}

/// POST /api/system/activate：令牌 + 回环校验 → 唤醒初始化任务（幂等）。
/// 响应体 `{ok, stage}`；403 = 令牌不匹配 / 非回环。
async fn gate_activate(
    State(deps): State<GateDeps>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Response {
    let token = headers
        .get("x-launcher-token")
        .and_then(|v| v.to_str().ok());
    match deps.gate.verify(Some(remote), token) {
        Ok(()) => {
            deps.gate.activate();
            Json(serde_json::json!({
                "ok": true,
                "stage": crate::update::gate::stage_str(),
            }))
            .into_response()
        }
        Err(reject) => reject_response(reject),
    }
}

fn reject_response(reject: ActivateReject) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "code": "forbidden",
            "message": reject.message(),
        })),
    )
        .into_response()
}

/// 业务 API 的闸内固定拒绝（503 update_not_ready）
async fn gate_not_ready() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "code": "update_not_ready",
            "message": "服务处于候选激活闸内，业务接口尚未开放；等待激活完成后重试",
        })),
    )
        .into_response()
}

/// 转发中间件：完整路由就位后接管一切请求（activate 除外，保持幂等回执）。
async fn forward_when_ready(
    State(shared): State<Arc<GateShared>>,
    req: Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Response {
    if req.uri().path() != "/api/system/activate" {
        if let Some(full) = shared.get() {
            // Router 的 Service Error = Infallible
            return full.oneshot(req).await.unwrap_or_else(|e| match e {});
        }
    }
    next.run(req).await
}

/// 组装闸内路由（main 的 gate 启动路径唯一入口）
pub fn build_gate_router(
    cfg: Config,
    db: Db,
    shutdown: Arc<ShutdownCoordinator>,
    gate: Arc<StartupGate>,
    shared: Arc<GateShared>,
) -> Router {
    // 闸内 stage 投影：进入本函数即处于 maintenance_gate（ready 在激活后设置）
    set_stage(STAGE_MAINTENANCE_GATE);
    let deps = GateDeps {
        cfg,
        db,
        shutdown,
        gate,
    };
    Router::new()
        .route("/health/live", get(|| async { (StatusCode::OK, "ok") }))
        .route("/health/ready", get(gate_health_ready))
        .route("/health/shutdown", get(gate_shutdown_state))
        .route("/api/system/activate", post(gate_activate))
        .fallback(gate_not_ready)
        .layer(axum::middleware::from_fn_with_state(
            shared,
            forward_when_ready,
        ))
        .with_state(deps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tower::ServiceExt;

    fn test_app(gate: Arc<StartupGate>, shared: Arc<GateShared>) -> (Router, std::path::PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("gamer-gate-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = Config {
            data_dir: dir.clone(),
            ..Default::default()
        };
        let db: Db = Arc::new(crate::store::Store::open(&cfg).unwrap());
        let shutdown = Arc::new(ShutdownCoordinator::new(Arc::new(|| Box::pin(async {}))));
        let app = build_gate_router(cfg, db, shutdown, gate, shared);
        (app, dir)
    }

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request as HttpRequest, StatusCode as HttpStatus};

    fn with_remote(b: axum::http::request::Builder, addr: &str) -> axum::http::request::Builder {
        b.extension(ConnectInfo::<SocketAddr>(addr.parse().unwrap()))
    }

    async fn send(app: &Router, r: HttpRequest<Body>) -> axum::http::Response<Body> {
        app.clone().oneshot(r).await.unwrap()
    }

    async fn json_of(resp: axum::http::Response<Body>) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    /// 放行矩阵：业务 API 503 update_not_ready；探针/停机/激活放行
    #[tokio::test]
    async fn gate_admission_matrix_blocks_business_and_allows_probes() {
        let gate = Arc::new(StartupGate::new(true, Some("tok".into())));
        let shared = Arc::new(GateShared::default());
        let (app, dir) = test_app(gate, shared);

        // 业务读写：503 {"code":"update_not_ready"}
        let resp = send(
            &app,
            HttpRequest::builder()
                .uri("/api/devices")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::SERVICE_UNAVAILABLE);
        assert_eq!(json_of(resp).await["code"], "update_not_ready");
        let resp = send(
            &app,
            HttpRequest::builder()
                .method("POST")
                .uri("/api/scripts/x/run")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::SERVICE_UNAVAILABLE);

        // /health/ready：503 ready:false（契约 not-ready 字段集）
        let resp = send(
            &app,
            HttpRequest::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::SERVICE_UNAVAILABLE);
        let body = json_of(resp).await;
        assert_eq!(body["ready"], false);
        for name in ["data_dir", "sqlite", "scrcpy_server", "adb", "ffmpeg"] {
            assert!(body["checks"][name]["ok"].is_boolean(), "{name}");
        }

        // /health/shutdown：200 running
        let resp = send(
            &app,
            HttpRequest::builder()
                .uri("/health/shutdown")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::OK);
        assert_eq!(json_of(resp).await["state"], "running");

        // /api/system/info 也被闸住（只放行探针/激活，不含 info）
        let resp = send(
            &app,
            HttpRequest::builder()
                .uri("/api/system/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::SERVICE_UNAVAILABLE);

        let _ = std::fs::remove_dir_all(dir);
    }

    /// activate：token 错 403；正确 + 回环 200；重复幂等
    #[tokio::test]
    async fn activate_token_loopback_and_idempotency() {
        let gate = Arc::new(StartupGate::new(true, Some("tok".into())));
        let shared = Arc::new(GateShared::default());
        let (app, dir) = test_app(gate.clone(), shared);

        let post_activate = |token: Option<&str>| {
            let mut b = HttpRequest::builder()
                .method("POST")
                .uri("/api/system/activate");
            if let Some(t) = token {
                b = b.header("x-launcher-token", t);
            }
            with_remote(b, "127.0.0.1:51000")
                .body(Body::empty())
                .unwrap()
        };

        // token 错 → 403
        let resp = send(&app, post_activate(Some("wrong"))).await;
        assert_eq!(resp.status(), HttpStatus::FORBIDDEN);
        assert_eq!(json_of(resp).await["code"], "forbidden");
        assert!(!gate.is_active(), "token 错不得放行");

        // 正确 token + 回环 → 200，闸放行
        let resp = send(&app, post_activate(Some("tok"))).await;
        assert_eq!(resp.status(), HttpStatus::OK);
        let body = json_of(resp).await;
        assert_eq!(body["ok"], true);
        assert!(gate.is_active());

        // 重复 activate 幂等：仍 200（不重复触发初始化信号）
        let resp = send(&app, post_activate(Some("tok"))).await;
        assert_eq!(resp.status(), HttpStatus::OK);

        // 非回环 → 403
        let b = with_remote(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/system/activate")
                .header("x-launcher-token", "tok"),
            "10.1.2.3:51000",
        )
        .body(Body::empty())
        .unwrap();
        let resp = send(&app, b).await;
        assert_eq!(resp.status(), HttpStatus::FORBIDDEN);

        let _ = std::fs::remove_dir_all(dir);
    }

    /// activate 前后 ready 翻转：闸内 503 ready:false → 完整路由换入后 200
    #[tokio::test]
    async fn ready_flips_after_full_router_swap() {
        let gate = Arc::new(StartupGate::new(true, Some("tok".into())));
        let shared = Arc::new(GateShared::default());
        let (app, dir) = test_app(gate.clone(), shared.clone());

        // 闸内：ready false
        let resp = send(
            &app,
            HttpRequest::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::SERVICE_UNAVAILABLE);

        // 模拟 main 激活初始化：activate → stage ready → 换入完整路由（stub）
        gate.activate();
        set_stage(crate::update::gate::STAGE_READY);
        shared.set(Router::new().route(
            "/health/ready",
            get(|| async {
                (
                    HttpStatus::OK,
                    Json(serde_json::json!({"ready": true, "checks": {}})),
                )
            }),
        ));

        // 换入后：/health/ready 走完整路由（翻转 200）
        let resp = send(
            &app,
            HttpRequest::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(resp.status(), HttpStatus::OK);
        assert_eq!(json_of(resp).await["ready"], true);

        // activate 恒留闸路由：幂等回执不受换入影响
        let b = with_remote(
            HttpRequest::builder()
                .method("POST")
                .uri("/api/system/activate")
                .header("x-launcher-token", "tok"),
            "127.0.0.1:51000",
        )
        .body(Body::empty())
        .unwrap();
        let resp = send(&app, b).await;
        assert_eq!(resp.status(), HttpStatus::OK);
        assert_eq!(json_of(resp).await["stage"], "ready");

        let _ = std::fs::remove_dir_all(dir);
    }
}
