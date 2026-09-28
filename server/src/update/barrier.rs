//! Serialize update admission with synchronous reservations of new business work.
use std::sync::{Mutex, MutexGuard};
static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PAUSED: Mutex<bool> = Mutex::new(false);

pub fn reservation() -> Result<MutexGuard<'static, bool>, &'static str> {
    let guard = PAUSED.lock().unwrap_or_else(|e| e.into_inner());
    if *guard {
        Err("软件更新中，暂不接受新的操作")
    } else {
        Ok(guard)
    }
}

pub fn freeze_if_idle(check: impl FnOnce() -> bool) -> bool {
    let mut paused = PAUSED.lock().unwrap_or_else(|e| e.into_inner());
    if *paused || ACTIVE.load(std::sync::atomic::Ordering::SeqCst) != 0 || !check() {
        return false;
    }
    *paused = true;
    true
}

pub fn pause() {
    *PAUSED.lock().unwrap_or_else(|e| e.into_inner()) = true;
}
pub fn resume() {
    *PAUSED.lock().unwrap_or_else(|e| e.into_inner()) = false;
}
pub fn paused() -> bool {
    *PAUSED.lock().unwrap_or_else(|e| e.into_inner())
}

pub async fn middleware(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = req.uri().path();
    if paused()
        && (path.starts_with("/ws/")
            || !matches!(
                *req.method(),
                axum::http::Method::GET | axum::http::Method::HEAD
            ))
        && !matches!(
            path,
            "/api/system/activate" | "/api/system/commit" | "/api/system/update/plugins"
        )
        && path != "/api/shutdown"
        && path != "/api/login"
        && path != "/api/logout"
    {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"code":"update_not_ready", "message":"正在更新，请等待服务恢复"}))).into_response();
    }
    next.run(req).await
}

/// Lifetime of asynchronous media output, including its connection/startup phase.
pub struct Activity;
pub fn activity() -> Result<Activity, &'static str> {
    let _guard = reservation()?;
    ACTIVE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    Ok(Activity)
}
impl Drop for Activity {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}
