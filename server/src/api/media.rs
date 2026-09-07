//! 媒体 REST（视频工作台 V1）。实施合同：`docs/plans/gamer_video_workbench_contracts.md` §1。
//!
//! 路由与处理器由媒体服务实施者填充；本文件只承载媒体路由组，
//! 认证与 body 限额组装配在 [`super::build_router_with_extensions`]。
//! 所有响应的素材寻址一律用 media id，不接受/返回服务端绝对路径。
//!
//! 服务层 [`crate::media::MediaService`] 的同步方法（探测/抽帧/文件 IO）
//! 一律经 [`super::common::run_blocking_api`] 执行，不阻塞 Tokio 工作线程。
//! 错误体按合同：`{"error":"<machine_code>"}`（`media_not_found` /
//! `media_referenced` / `media_unsupported`），人读细节进日志。

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::Stream;
use serde::Deserialize;
use serde_json::json;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncSeekExt, ReadBuf, SeekFrom};

use super::common::run_blocking_api;
use super::{ApiError, AppState};
use crate::media::{
    service as media_service, FrameRequest, MediaError, MediaErrorKind, MediaId, MediaRef,
};

/// 媒体路由组（挂进受保护组；导入大字节限额由组层统一设定）。
/// 合同端点：
/// - `POST /api/media/import?name=<filename>`（raw bytes → 201 metadata）
/// - `GET  /api/media`（列表）/ `GET|DELETE /api/media/:id`
/// - `GET  /api/media/:id/file`（Range 播放流）
/// - `GET  /api/media/:id/frame?pts_us=|index=&max_width=`（PNG 确定帧）
/// - `POST /api/media/:id/refs`（引用登记/解除）
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/media/import", post(api_media_import))
        .route("/api/media", get(api_media_list))
        .route(
            "/api/media/:id",
            get(api_media_get).delete(api_media_delete),
        )
        .route("/api/media/:id/file", get(api_media_file))
        .route("/api/media/:id/frame", get(api_media_frame))
        .route("/api/media/:id/refs", post(api_media_refs))
}

/// 合同错误映射：kind → (状态码, 机器码)；错误体 `{"error":"<machine_code>"}`，
/// 人读细节只进日志（Invalid 走人类可读 400 文案，合同未钉码）。
pub(super) fn map_media_err(err: anyhow::Error) -> ApiError {
    if let Some(me) = err.downcast_ref::<MediaError>() {
        let (status, code) = match me.kind() {
            MediaErrorKind::NotFound => (StatusCode::NOT_FOUND, "media_not_found"),
            MediaErrorKind::Referenced => (StatusCode::CONFLICT, "media_referenced"),
            MediaErrorKind::Unsupported => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "media_unsupported")
            }
            MediaErrorKind::Invalid => return ApiError::bad_request(me.message().to_owned()),
        };
        tracing::warn!(code, detail = %me, "media api error");
        return ApiError::new(status, code);
    }
    tracing::warn!(error = %err, "media api internal error");
    ApiError::internal(format!("{err:#}"))
}

// ---------- POST /api/media/import?name= ----------

#[derive(Deserialize)]
struct ImportQuery {
    name: Option<String>,
}

async fn api_media_import(
    State(st): State<AppState>,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> Response {
    let Some(name) = q.name.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return ApiError::bad_request("name query 参数必填（导入文件名）").into_response();
    };
    let svc = media_service(&st.cfg);
    let name = name.to_string();
    match run_blocking_api(move || svc.import_bytes(&name, &body).map_err(map_media_err)).await {
        Ok(meta) => (StatusCode::CREATED, Json(meta)).into_response(),
        Err(err) => err.into_response(),
    }
}

// ---------- GET /api/media ----------

async fn api_media_list(State(st): State<AppState>) -> Response {
    let svc = media_service(&st.cfg);
    match run_blocking_api(move || svc.list().map_err(map_media_err)).await {
        Ok(items) => Json(json!({ "media": items })).into_response(),
        Err(err) => err.into_response(),
    }
}

// ---------- GET|DELETE /api/media/:id ----------

async fn api_media_get(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let svc = media_service(&st.cfg);
    match run_blocking_api(move || svc.get(&MediaId(id)).map_err(map_media_err)).await {
        Ok(meta) => Json(meta).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn api_media_delete(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    let svc = media_service(&st.cfg);
    match run_blocking_api(move || svc.delete(&MediaId(id)).map_err(map_media_err)).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => err.into_response(),
    }
}

// ---------- GET /api/media/:id/file（播放流，支持单段 Range） ----------

const ACCEPT_RANGES: HeaderName = header::ACCEPT_RANGES;
const CONTENT_RANGE: HeaderName = header::CONTENT_RANGE;

/// Range 解析结果：Full（无/不可解析/多段 Range——HTTP 允许服务器忽略 Range）、
/// Slice(起,闭)、Unsatisfiable（416）。
enum RangeReply {
    Full,
    Slice(u64, u64),
    Unsatisfiable,
}

/// 单段 `bytes=a-b` / `bytes=a-` / `bytes=-n` 解析（size = 资源总长）。
fn parse_range_header(value: Option<&HeaderValue>, size: u64) -> RangeReply {
    let Some(raw) = value.and_then(|v| v.to_str().ok()) else {
        return RangeReply::Full;
    };
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        return RangeReply::Full;
    };
    // 多段 Range V1 不支持：整体忽略 → 200 全量
    if spec.contains(',') {
        return RangeReply::Full;
    }
    let Some((start_s, end_s)) = spec.trim().split_once('-') else {
        return RangeReply::Full;
    };
    let parse = |s: &str| s.trim().parse::<u64>().ok();
    if size == 0 {
        return RangeReply::Unsatisfiable;
    }
    let last = size - 1;
    match (
        start_s.trim().is_empty(),
        end_s.trim().is_empty(),
        parse(start_s),
        parse(end_s),
    ) {
        // 前缀起始越界（start ≥ size）→ 416（RFC 9110 语义）
        (false, _, Some(start), _) if start >= size => RangeReply::Unsatisfiable,
        (false, false, Some(start), Some(end)) if start <= end => {
            RangeReply::Slice(start, end.min(last))
        }
        (false, true, Some(start), _) => RangeReply::Slice(start, last),
        // 后缀 `bytes=-n`：最后 n 字节；n ≥ 总长 → 整文件
        (true, false, _, Some(n)) if n > 0 => {
            RangeReply::Slice(size.saturating_sub(n.min(size)), last)
        }
        _ => RangeReply::Full,
    }
}

/// `tokio::fs::File` 的有界字节流（futures Stream → axum Body::from_stream）。
/// 只产出 `remaining` 字节；EOF 提前（文件被外部截断）即提前收尾。
struct FileRangeStream {
    file: tokio::fs::File,
    remaining: u64,
}

const STREAM_CHUNK_BYTES: usize = 64 * 1024;

impl Stream for FileRangeStream {
    type Item = std::io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.remaining == 0 {
            return Poll::Ready(None);
        }
        let len = STREAM_CHUNK_BYTES.min(self.remaining as usize);
        let mut buf = vec![0u8; len];
        let mut read_buf = ReadBuf::new(&mut buf);
        match Pin::new(&mut self.file).poll_read(cx, &mut read_buf) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Ready(Ok(())) => {
                let filled = read_buf.filled().len();
                if filled == 0 {
                    return Poll::Ready(None); // EOF 提前
                }
                let n = filled.min(self.remaining as usize);
                self.remaining -= n as u64;
                Poll::Ready(Some(Ok(Bytes::copy_from_slice(&read_buf.filled()[..n]))))
            }
        }
    }
}

/// 容器 → 播放 Content-Type（未知容器走 octet-stream，`<video>` 仍可播）。
fn container_mime(container: &str) -> &'static str {
    match container {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "matroska" | "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "mpegts" | "ts" => "video/mp2t",
        _ => "application/octet-stream",
    }
}

async fn api_media_file(
    State(st): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let svc = media_service(&st.cfg);
    let resolved = run_blocking_api(move || {
        // 先过元数据门（不存在 → 404 语义），再解析磁盘路径与长度
        let meta = svc.get(&MediaId(id.clone())).map_err(map_media_err)?;
        let path = svc.file_path(&MediaId(id)).map_err(map_media_err)?;
        let size = std::fs::metadata(&path)
            .map_err(|e| anyhow::anyhow!("original 文件不可读: {e}"))
            .map_err(map_media_err)?
            .len();
        Ok((meta.container, path, size))
    })
    .await;
    let (container, path, size) = match resolved {
        Ok(v) => v,
        Err(err) => return err.into_response(),
    };
    let mime = container_mime(&container);

    // 打开 + 定位（Range 起点）；文件打开失败 = 服务端读路径问题（500）
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(e) => {
            return ApiError::internal(format!("打开原文件失败: {e}")).into_response();
        }
    };
    let (status, start, end) = match parse_range_header(headers.get(header::RANGE), size) {
        RangeReply::Unsatisfiable => {
            let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            resp.headers_mut().insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{size}")).expect("static header"),
            );
            resp.headers_mut()
                .insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            return resp;
        }
        RangeReply::Full => (StatusCode::OK, 0, size.saturating_sub(1)),
        RangeReply::Slice(start, end) => {
            if let Err(e) = file.seek(SeekFrom::Start(start)).await {
                return ApiError::internal(format!("定位播放起点失败: {e}")).into_response();
            }
            (StatusCode::PARTIAL_CONTENT, start, end)
        }
    };
    let length = if size == 0 { 0 } else { end - start + 1 };
    let stream = FileRangeStream {
        file,
        remaining: length,
    };
    let mut resp = Response::builder()
        .status(status)
        .header(ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, length)
        .body(Body::from_stream(stream))
        .expect("static response build");
    if status == StatusCode::PARTIAL_CONTENT {
        resp.headers_mut().insert(
            CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{size}")).expect("static header"),
        );
    }
    resp
}

// ---------- GET /api/media/:id/frame ----------

fn parse_frame_query(q: &HashMap<String, String>) -> Result<FrameRequest, ApiError> {
    let mut req = FrameRequest::default();
    for (key, value) in q {
        let value = value.trim();
        match key.as_str() {
            "pts_us" if !value.is_empty() => {
                req.pts_us = Some(
                    value
                        .parse()
                        .map_err(|_| ApiError::bad_request("pts_us 必须是非负整数（微秒）"))?,
                );
            }
            "index" if !value.is_empty() => {
                req.index = Some(
                    value
                        .parse()
                        .map_err(|_| ApiError::bad_request("index 必须是非负整数"))?,
                );
            }
            "max_width" if !value.is_empty() => {
                req.max_width = Some(
                    value
                        .parse()
                        .map_err(|_| ApiError::bad_request("max_width 必须是非负整数"))?,
                );
            }
            _ => {}
        }
    }
    Ok(req)
}

async fn api_media_frame(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let req = match parse_frame_query(&q) {
        Ok(req) => req,
        Err(err) => return err.into_response(),
    };
    let svc = media_service(&st.cfg);
    match run_blocking_api(move || {
        svc.extract_frame_png(&MediaId(id), &req)
            .map_err(map_media_err)
    })
    .await
    {
        Ok(png) => {
            // 帧内容按 (media, pts/index, max_width) 确定：允许中间缓存
            let mut resp = (StatusCode::OK, png).into_response();
            let headers = resp.headers_mut();
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=3600"),
            );
            resp
        }
        Err(err) => err.into_response(),
    }
}

// ---------- POST /api/media/:id/refs ----------

#[derive(Deserialize)]
struct RefsBody {
    refs: Vec<MediaRef>,
}

async fn api_media_refs(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RefsBody>,
) -> Response {
    let svc = media_service(&st.cfg);
    let refs = body.refs;
    let get_id = id.clone();
    match run_blocking_api(move || {
        svc.set_refs(&MediaId(id), &refs).map_err(map_media_err)?;
        // 返回更新后的元数据（引用状态回显）
        svc.get(&MediaId(get_id)).map_err(map_media_err)
    })
    .await
    {
        Ok(meta) => Json(meta).into_response(),
        Err(err) => err.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range_header(value: &str) -> Option<HeaderValue> {
        Some(HeaderValue::from_str(value).unwrap())
    }

    #[test]
    fn range_parsing_covers_open_closed_suffix_and_invalid() {
        assert!(matches!(
            parse_range_header(range_header("bytes=0-99").as_ref(), 1000),
            RangeReply::Slice(0, 99)
        ));
        assert!(matches!(
            parse_range_header(range_header("bytes=500-").as_ref(), 1000),
            RangeReply::Slice(500, 999)
        ));
        assert!(matches!(
            parse_range_header(range_header("bytes=-100").as_ref(), 1000),
            RangeReply::Slice(900, 999)
        ));
        // 后缀超过总长 → 整文件
        assert!(matches!(
            parse_range_header(range_header("bytes=-9999").as_ref(), 1000),
            RangeReply::Slice(0, 999)
        ));
        // end 截断到末尾
        assert!(matches!(
            parse_range_header(range_header("bytes=0-99999").as_ref(), 1000),
            RangeReply::Slice(0, 999)
        ));
        // 起点越界（start ≥ size）→ 416
        assert!(matches!(
            parse_range_header(range_header("bytes=1000-").as_ref(), 1000),
            RangeReply::Unsatisfiable
        ));
        assert!(matches!(
            parse_range_header(range_header("bytes=5000-").as_ref(), 1000),
            RangeReply::Unsatisfiable
        ));
        assert!(matches!(
            parse_range_header(range_header("bytes=1000-1005").as_ref(), 1000),
            RangeReply::Unsatisfiable
        ));
        // 多段/非法/缺失 → 忽略 Range → 200 全量
        assert!(matches!(
            parse_range_header(range_header("bytes=0-1,5-9").as_ref(), 1000),
            RangeReply::Full
        ));
        assert!(matches!(
            parse_range_header(range_header("bytes=9-5").as_ref(), 1000),
            RangeReply::Full
        ));
        assert!(matches!(
            parse_range_header(range_header("bytes=abc").as_ref(), 1000),
            RangeReply::Full
        ));
        assert!(matches!(parse_range_header(None, 1000), RangeReply::Full));
        // 空资源不可满足任何 range
        assert!(matches!(
            parse_range_header(range_header("bytes=0-").as_ref(), 0),
            RangeReply::Unsatisfiable
        ));
    }

    #[test]
    fn container_mime_mapping() {
        assert_eq!(container_mime("mp4"), "video/mp4");
        assert_eq!(container_mime("webm"), "video/webm");
        assert_eq!(container_mime("matroska"), "video/x-matroska");
        assert_eq!(container_mime("something"), "application/octet-stream");
    }

    #[tokio::test]
    async fn file_range_stream_yields_exactly_remaining_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        let blob: Vec<u8> = (0..=255u8).cycle().take(256).collect();
        std::fs::write(&path, &blob).unwrap(); // 256 字节
        let mut file = tokio::fs::File::open(&path).await.unwrap();
        file.seek(SeekFrom::Start(100)).await.unwrap();
        let mut stream = FileRangeStream {
            file,
            remaining: 50,
        };
        use futures_util::StreamExt;
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(collected.len(), 50);
        let all = std::fs::read(&path).unwrap();
        assert_eq!(collected, &all[100..150]);
    }
}
