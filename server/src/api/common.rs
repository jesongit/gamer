//! Shared API validation, error responses, and blocking boundaries.

use std::sync::{Arc, OnceLock};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

use super::ApiError;

/// 普通 JSON API 请求体上限（256KiB）
pub(super) const BODY_LIMIT_JSON: usize = 256 * 1024;
/// 模板上传 / 脚本保存的 JSON 上限
pub(super) const BODY_LIMIT_UPLOAD: usize = 16 * 1024 * 1024;
/// ZIP 导入请求体上限
pub(super) const BODY_LIMIT_ZIP_IMPORT: usize = 20 * 1024 * 1024;
/// 公开豁免组请求体上限
pub(super) const BODY_LIMIT_PUBLIC: usize = 64 * 1024;
/// API 侧同步文件/SQLite/外部探测任务的并发上限
const API_BLOCKING_CONCURRENCY: usize = 16;

fn api_blocking_limiter() -> &'static Arc<Semaphore> {
    static LIMITER: OnceLock<Arc<Semaphore>> = OnceLock::new();
    LIMITER.get_or_init(|| Arc::new(Semaphore::new(API_BLOCKING_CONCURRENCY)))
}

/// 所有同步文件、SQLite、外部探测与图像处理统一经过 blocking 池。
pub(super) async fn run_blocking_api<T, F>(task: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
{
    let permit = api_blocking_limiter()
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError::internal("blocking worker limiter closed"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        task()
    })
    .await
    .map_err(|e| ApiError::internal(format!("blocking worker failed: {e}")))?
}

pub(super) fn err_response(status: StatusCode, msg: &str) -> Response {
    ApiError::new(status, msg.to_owned()).into_response()
}

pub(super) fn validate_text_field(
    value: &str,
    field: &str,
    max_bytes: usize,
) -> Result<(), ApiError> {
    if value.trim().is_empty() {
        return Err(ApiError::bad_request(format!("{field} 不能为空")));
    }
    if value.len() > max_bytes {
        return Err(ApiError::bad_request(format!(
            "{field} 超过 {max_bytes} 字节"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(ApiError::bad_request(format!("{field} 包含非法控制字符")));
    }
    Ok(())
}

/// 校验必需的 pkg 参数（应用包名 = 分区名）：缺失/空串/非法包名统一为 400。
pub(super) fn require_pkg(raw: Option<&str>) -> Result<String, ApiError> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(crate::core::fs::safe_name)
        .ok_or_else(|| ApiError::bad_request("应用包名非法（只允许字母数字 . _ -）"))
}
