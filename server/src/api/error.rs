use std::borrow::Cow;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

/// API 统一错误：把 HTTP 状态码与 JSON 错误体收束在一个地方。
///
/// 错误体形如 `{"error": "<msg>"}`；个别链路可附机器码 `code`（如导出
/// preflight 失败的 `preflight_failed`），前端按 `error` 文本消费不受影响。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiError {
    status: StatusCode,
    message: Cow<'static, str>,
    code: Option<Cow<'static, str>>,
}

#[allow(dead_code)]
impl ApiError {
    pub(crate) fn new(status: StatusCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            status,
            message: message.into(),
            code: None,
        }
    }

    /// 附机器码（稳定标识，前端据此分支；不影响既有 `error` 字段读取）。
    pub(crate) fn with_code(mut self, code: &'static str) -> Self {
        self.code = Some(Cow::Borrowed(code));
        self
    }

    pub(crate) fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    pub(crate) fn unauthorized(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message)
    }

    pub(crate) fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::FORBIDDEN, message)
    }

    pub(crate) fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    pub(crate) fn conflict(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::CONFLICT, message)
    }

    pub(crate) fn bad_gateway(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message)
    }

    pub(crate) fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    pub(crate) fn service_unavailable(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }

    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // code 仅在显式设置时输出，保持既有错误体形状不变
        let mut body = serde_json::json!({"error": self.message});
        if let Some(code) = self.code {
            body["code"] = serde_json::Value::String(code.into_owned());
        }
        (self.status, Json(body)).into_response()
    }
}
