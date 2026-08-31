//! IPC named-pipe server (ipc-v1).
//!
//! The Windows transport is behind `cfg(windows)`. The frame reader and its
//! tests are platform independent so CI can exercise the protocol everywhere
//! without pretending that a Unix socket is the production transport.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::dispatch::Dispatcher;
use super::frames::{FrameError, PREFIX_LEN};
use super::{FRAME_TIMEOUT, MAX_FRAME_BYTES};

/// Server configuration shared by the Windows implementation and tests.
#[derive(Debug, Clone)]
pub struct IpcServerConfig {
    /// Complete `\\.\pipe\...` name.
    pub pipe_name: String,
    /// Per-launch session token.
    pub token: String,
    /// Maximum JSON payload bytes.
    pub frame_limit: usize,
    /// Per-frame read/write deadline.
    pub frame_timeout: Duration,
}

impl Default for IpcServerConfig {
    fn default() -> Self {
        Self {
            pipe_name: String::new(),
            token: String::new(),
            frame_limit: MAX_FRAME_BYTES,
            frame_timeout: FRAME_TIMEOUT,
        }
    }
}

/// Run the named-pipe accept loop on Windows.
#[cfg(windows)]
pub async fn run_server(dispatcher: Arc<Dispatcher>, cfg: IpcServerConfig) -> std::io::Result<()> {
    use super::dacl::create_pipe_server;

    let mut server = create_pipe_server(&cfg.pipe_name, true)?;
    tracing::info!(pipe = %cfg.pipe_name, "IPC pipe 已创建（DACL=仅当前用户+SYSTEM）");
    loop {
        server.connect().await?;
        let client = server;
        server = create_pipe_server(&cfg.pipe_name, false)?;
        let dispatcher = Arc::clone(&dispatcher);
        let cfg = cfg.clone();
        tokio::spawn(async move {
            handle_connection(client, dispatcher, cfg).await;
        });
    }
}

/// Non-Windows builds can compile and test the launcher, but cannot provide
/// the product's Windows named-pipe endpoint.
#[cfg(not(windows))]
pub async fn run_server(
    _dispatcher: Arc<Dispatcher>,
    _cfg: IpcServerConfig,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Windows named-pipe IPC is unavailable on this platform",
    ))
}

/// Serve one already-connected byte-mode stream.
///
/// The production Windows listener passes a `NamedPipeServer` here. Keeping
/// the request/response loop generic makes the exact same framing, timeout,
/// size-limit, and disconnect behavior runnable with an in-memory duplex
/// stream on non-Windows CI.
pub(crate) async fn handle_connection<S>(
    stream: S,
    dispatcher: Arc<Dispatcher>,
    cfg: IpcServerConfig,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut seen_request_ids = HashSet::new();
    loop {
        let frame = match tokio::time::timeout(
            cfg.frame_timeout,
            read_frame_async(&mut reader, cfg.frame_limit),
        )
        .await
        {
            Err(_) => {
                tracing::debug!("IPC 读帧超时，断开");
                break;
            }
            Ok(Err(FrameError::Oversize(n))) => {
                tracing::warn!(
                    size = n,
                    limit = cfg.frame_limit,
                    "IPC 超大帧，立即断开（不回帧）"
                );
                break;
            }
            Ok(Err(FrameError::UnexpectedEof)) | Ok(Err(FrameError::Io(_))) => break,
            Ok(Ok(bytes)) => bytes,
        };

        let reply = dispatcher.handle_with_seen(&frame, &cfg.token, &mut seen_request_ids);
        let payload = reply.frame.to_string();
        // Replies are also bounded. A pathological status/result must not
        // bypass the bidirectional protocol limit.
        let bytes = match super::frames::encode_frame_limited(payload.as_bytes(), cfg.frame_limit) {
            Ok(bytes) => bytes,
            Err(FrameError::Oversize(n)) => {
                tracing::error!(size = n, limit = cfg.frame_limit, "IPC 响应超过帧上限");
                break;
            }
            Err(_) => unreachable!("payload length was already checked by encode_frame_limited"),
        };
        match tokio::time::timeout(cfg.frame_timeout, writer.write_all(&bytes)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => break,
        }
        match tokio::time::timeout(cfg.frame_timeout, writer.flush()).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => break,
        }
        if reply.disconnect {
            tracing::debug!("IPC 协议错误，响应后断开");
            break;
        }
    }
}

/// Read one length-prefixed payload. The length is checked before allocation
/// or payload reads, enforcing the v1 oversize-disconnect rule.
pub(crate) async fn read_frame_async<R: AsyncRead + Unpin>(
    reader: &mut R,
    max: usize,
) -> Result<Vec<u8>, FrameError> {
    let mut prefix = [0u8; PREFIX_LEN];
    match reader.read_exact(&mut prefix).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(FrameError::UnexpectedEof)
        }
        Err(e) => return Err(FrameError::Io(e)),
    }
    let len = u32::from_le_bytes(prefix) as usize;
    if len > max {
        return Err(FrameError::Oversize(len));
    }
    let mut payload = vec![0u8; len];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(FrameError::Io)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::frames::encode_frame;

    #[tokio::test]
    async fn read_frame_async_roundtrip_and_limits() {
        let mut buf = std::io::Cursor::new(encode_frame(br#"{"x":1}"#));
        let frame = read_frame_async(&mut buf, MAX_FRAME_BYTES).await.unwrap();
        assert_eq!(frame, br#"{"x":1}"#);

        let mut oversize =
            std::io::Cursor::new((MAX_FRAME_BYTES as u32 + 1).to_le_bytes().to_vec());
        match read_frame_async(&mut oversize, MAX_FRAME_BYTES).await {
            Err(FrameError::Oversize(n)) => assert_eq!(n, MAX_FRAME_BYTES + 1),
            other => panic!("应报 Oversize，实际 {other:?}"),
        }

        let mut empty = std::io::Cursor::new(Vec::new());
        assert!(matches!(
            read_frame_async(&mut empty, MAX_FRAME_BYTES).await,
            Err(FrameError::UnexpectedEof)
        ));
    }

    #[tokio::test]
    async fn oversize_does_not_consume_payload() {
        let mut bytes = (MAX_FRAME_BYTES as u32 + 1).to_le_bytes().to_vec();
        bytes.extend_from_slice(b"payload");
        let mut reader = std::io::Cursor::new(bytes);
        assert!(matches!(
            read_frame_async(&mut reader, MAX_FRAME_BYTES).await,
            Err(FrameError::Oversize(_))
        ));
        assert_eq!(reader.position(), PREFIX_LEN as u64);
    }
}
