//! IPC named pipe 服务端主循环（tokio）：accept �?每连接任�?�?//! 帧（上限/超时）→ Dispatcher �?回帧。单帧交换超�?30s；超大帧立即断开
//! 不回帧；unauthorized/unsupported_protocol_version 回帧后断开�?
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::NamedPipeServer;

use super::dacl::create_pipe_server;
use super::dispatch::{Dispatcher, Reply};
use super::frames::{encode_frame, FrameError, PREFIX_LEN};
use super::{FRAME_TIMEOUT, MAX_FRAME_BYTES};

/// 服务端配置�?#[derive(Debug, Clone)]
pub struct IpcServerConfig {
    /// 完整 pipe 名（�?`\\.\pipe\` 前缀）�?    pub pipe_name: String,
    /// 本次启动会话令牌（逐帧校验）�?    pub token: String,
    /// 单帧上限（默�?1 MiB）�?    pub frame_limit: usize,
    /// 单帧交换超时（默�?30s）�?    pub frame_timeout: Duration,
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

/// 服务端主循环；正常情况永不返回（进程退�?任务�?drop 时结束）�?pub async fn run_server(dispatcher: Arc<Dispatcher>, cfg: IpcServerConfig) -> std::io::Result<()> {
    let mut server = create_pipe_server(&cfg.pipe_name, true)?;
    tracing::info!(pipe = %cfg.pipe_name, "IPC pipe 已创建（DACL=当前用户+SYSTEM�?);
    loop {
        server.connect().await?;
        let client = server;
        // 立刻准备下一个监听实例（多连接并发）
        server = match create_pipe_server(&cfg.pipe_name, false) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(pipe = %cfg.pipe_name, error = %e, "重建 pipe 实例失败，IPC 退�?);
                return Err(e);
            }
        };
        let dispatcher = Arc::clone(&dispatcher);
        let cfg = cfg.clone();
        tokio::spawn(async move {
            handle_client(client, dispatcher, cfg).await;
        });
    }
}

async fn handle_client(
    server: NamedPipeServer,
    dispatcher: Arc<Dispatcher>,
    cfg: IpcServerConfig,
) {
    tracing::debug!("IPC 客户端接�?);
    let (mut reader, mut writer) = tokio::io::split(server);
    loop {
        // 读帧（单帧超时；超大帧立即断开不回帧）
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
                tracing::warn!(size = n, limit = cfg.frame_limit, "IPC 超大帧，立即断开（不回帧�?);
                break;
            }
            Ok(Err(FrameError::UnexpectedEof)) | Ok(Err(FrameError::Io(_))) => break,
            Ok(Ok(bytes)) => bytes,
        };
        let Reply { frame: reply, disconnect } = dispatcher.handle(&frame, &cfg.token);
        let payload = reply.to_string();
        let bytes = encode_frame(payload.as_bytes());
        if tokio::time::timeout(cfg.frame_timeout, writer.write_all(&bytes)).await.is_err() {
            break;
        }
        let _ = tokio::time::timeout(cfg.frame_timeout, writer.flush()).await;
        if disconnect {
            tracing::debug!("IPC 协议错误，响应后断开");
            break;
        }
    }
}

/// tokio 版读帧：�?4 字节前缀（可�?EOF=对端关闭），超限立即 `Oversize`�?async fn read_frame_async<R: tokio::io::AsyncRead + Unpin>(
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
    reader.read_exact(&mut payload).await.map_err(FrameError::Io)?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_frame_async_roundtrip_and_limits() {
        let mut buf = std::io::Cursor::new(encode_frame(br#"{"x":1}"#));
        let frame = read_frame_async(&mut buf, MAX_FRAME_BYTES).await.unwrap();
        assert_eq!(frame, br#"{"x":1}"#);
        let mut oversize = std::io::Cursor::new((MAX_FRAME_BYTES as u32 + 1).to_le_bytes().to_vec());
        match read_frame_async(&mut oversize, MAX_FRAME_BYTES).await {
            Err(FrameError::Oversize(n)) => assert_eq!(n, MAX_FRAME_BYTES + 1),
            other => panic!("应报 Oversize，实�?{other:?}"),
        }
        let mut empty = std::io::Cursor::new(Vec::new());
        assert!(matches!(
            read_frame_async(&mut empty, MAX_FRAME_BYTES).await,
            Err(FrameError::UnexpectedEof)
        ));
    }
}
